"""The two ways a model is asked for a plan (roadmap item S4).

Standard library only, so the gate can test it; `run_eval.py` does the I/O.

  grammar  The shipped path (ADR 0011): the system prompt of
           intent-shell/src/model.rs and the schema file as `response_format`.
  tools    Native tool calls: one function per action, built from the same
           schema file, and a `clarify` function.
"""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODEL_RS = ROOT / "intent-shell/src/model.rs"

NESTED_WITHOUT_END = "the model's tool call nests without end (the server refused to parse it)"
STEP_NAMES = [
    "fsList", "fsStat", "fsRead", "fsFindLargest", "fsWrite", "fsMove",
    "fsDelete", "sessionCd", "processRun", "userManage", "systemConfigure", "soulProposeChange",
]
DESCRIPTIONS = {
    "fs.list": "List what is in a place or a folder.",
    "fs.stat": "Show the size and dates of one file or folder.",
    "fs.read": "Read the text of one file.",
    "fs.find_largest": "Find the largest files in a place or folder, biggest first.",
    "fs.write": "Create a file, or overwrite or append to an existing one.",
    "fs.move": "Move or rename a file or folder.",
    "fs.delete": "Delete a file or folder that exists.",
    "session.cd": "Change the current directory for the next request. A plan of its own: never together with another step.",
    "process.run": "Run a program by name with an argument list (never a shell string).",
    "user.manage": "Add or remove a user, or add or remove a user from a group.",
    "system.configure": "Set a system setting.",
    "soul.propose_change": "Propose a change to the assistant's SOUL (its personality text). Only records a proposal.",
}
# The shell prompt shows `"depends_on":[]` in its JSON example; the last sentence
# of the first preamble gives the tools mode the same information.
TOOLS_PREAMBLE = (
    "You act on the user's request by calling the provided tools, and call no other tool. "
    "To ask a question instead of acting, call `clarify`. "
    "The actions named below with a dot are the tools named with an underscore (fs.list is the tool fs_list). "
    "Your calls are numbered s1, s2, ... in the order you make them: list in depends_on the numbers of the calls a call needs, "
    "and use {\"from_step\":\"s1\"} to refer to what an earlier call found. "
    "A call that needs no earlier call has depends_on [] (an empty list)."
)
# The variant in which the harness works out the dependencies from the references,
# so the model does not have to follow the numbering convention.
DERIVED_PREAMBLE = (
    "You act on the user's request by calling the provided tools, and call no other tool. "
    "To ask a question instead of acting, call `clarify`. "
    "The actions named below with a dot are the tools named with an underscore (fs.list is the tool fs_list). "
    "Your calls are numbered s1, s2, ... in the order you make them: use {\"from_step\":\"s1\"} to refer to what an earlier call found."
)


def shell_prompt(source=None):
    """The system prompt exactly as the shell sends it."""
    source = source if source is not None else MODEL_RS.read_text(encoding="utf-8")
    found = re.search(r'const SYSTEM_PROMPT: &str = r#"(.*?)"#;', source, re.DOTALL)
    if not found:
        raise ValueError("SYSTEM_PROMPT not found in intent-shell/src/model.rs")
    return found.group(1)


def tools_prompt(prompt, derive_depends=False):
    """The shell prompt without its JSON-document instructions, for native tools."""
    lines = prompt.splitlines()
    start = next((i for i, line in enumerate(lines) if line.startswith("Name things with a reference")), None)
    if start is None:
        raise ValueError("cannot find the reference rules in the shell prompt")
    rules = "\n".join(lines[start:])
    if derive_depends:
        # The model has no depends_on to fill in, so the lines that talk about it go.
        rules = rules.replace(" (put s1 in depends_on)", "")
        rules = rules.replace("Rules: ids are s1, s2, ...; depends_on lists the step ids a step needs; ", "Rules: ")
        if "depends_on" in rules:
            raise ValueError("the shell prompt now says more about depends_on than the derived variant removes")
    return (DERIVED_PREAMBLE if derive_depends else TOOLS_PREAMBLE) + "\n" + rules


def inline(node, defs):
    """The schema with every $ref replaced by its target (the schema has no recursion)."""
    if isinstance(node, dict):
        if "$ref" in node:
            return inline(defs[node["$ref"].rsplit("/", 1)[1]], defs)
        return {key: inline(value, defs) for key, value in node.items()}
    if isinstance(node, list):
        return [inline(item, defs) for item in node]
    return node


def build_tools(schema, derive_depends=False):
    """One function per action from the schema's own step definitions.

    Returns the OpenAI-style `tools` array and, by function name, the action
    and whether the step has an `elevate` field.
    """
    defs = schema["$defs"]
    tools, by_name = [], {}
    for def_name in STEP_NAMES:
        step = defs[def_name]
        action = step["properties"]["action"]["const"]
        args = inline(step["properties"]["args"], defs)
        properties = dict(args["properties"])
        required = list(args["required"])
        if not derive_depends:
            properties["depends_on"] = inline(step["properties"]["depends_on"], defs)
            required.append("depends_on")
        has_elevate = "elevate" in step["properties"]
        if has_elevate:
            properties["elevate"] = {"type": "boolean"}
            required.append("elevate")
        name = action.replace(".", "_")
        by_name[name] = {"action": action, "has_elevate": has_elevate}
        tools.append({"type": "function", "function": {
            "name": name,
            "description": DESCRIPTIONS[action],
            "parameters": {"type": "object", "properties": properties, "required": required, "additionalProperties": False},
        }})
    tools.append({"type": "function", "function": {
        "name": "clarify",
        "description": "Ask the person a question when the request cannot be done without guessing.",
        "parameters": {"type": "object", "properties": {"question": {"type": "string", "minLength": 1, "maxLength": 300}}, "required": ["question"], "additionalProperties": False},
    }})
    return tools, by_name


def references(value):
    """Every step a value refers to with `from_step`."""
    if isinstance(value, dict):
        found = [value["from_step"]] if set(value) == {"from_step"} and isinstance(value["from_step"], str) else []
        return found + [r for item in value.values() for r in references(item)]
    if isinstance(value, list):
        return [r for item in value for r in references(item)]
    return []


def document_from_calls(message, by_name, derive_depends=False):
    """A plan document from native tool calls, or an error text.

    Nothing is repaired: a missing `depends_on` or `elevate`, or an argument
    the schema does not have, stays in the document for the schema to refuse,
    as it would for a plan written as JSON. With `derive_depends` the harness
    sets `depends_on` to the steps the arguments refer to. A tool called by its
    action name (`fs.list` for `fs_list`) is the same tool.
    """
    calls = message.get("tool_calls") or []
    if not calls:
        return None, "no tool call (plain text reply)"
    parsed = []
    for call in calls:
        name = call["function"]["name"].replace(".", "_")
        arguments = call["function"]["arguments"]
        try:
            arguments = json.loads(arguments) if isinstance(arguments, str) else arguments
        except json.JSONDecodeError as error:
            return None, f"arguments of {name} are not JSON: {error}"
        if not isinstance(arguments, dict):
            return None, f"arguments of {name} are not an object"
        parsed.append((name, arguments))
    if any(name == "clarify" for name, _ in parsed):
        if len(parsed) != 1:
            return None, "clarify mixed with other calls"
        if set(parsed[0][1]) - {"question"}:
            return None, "clarify has arguments other than question"
        return {"kind": "clarify", "version": 3, "question": parsed[0][1].get("question", "")}, None
    steps = []
    for index, (name, arguments) in enumerate(parsed, 1):
        if name not in by_name:
            return None, f"unknown tool {name!r}"
        if not derive_depends and "depends_on" not in arguments:
            return None, f"{name} has no depends_on"
        info = by_name[name]
        if info["has_elevate"] and "elevate" not in arguments:
            return None, f"{name} has no elevate"
        step = {
            "id": f"s{index}",
            "action": info["action"],
            "args": {key: value for key, value in arguments.items() if key not in ("depends_on", "elevate")},
            "depends_on": sorted(set(references(arguments))) if derive_depends else arguments["depends_on"],
        }
        if "elevate" in arguments:
            step["elevate"] = arguments["elevate"]
        steps.append(step)
    return {"kind": "plan", "version": 3, "summary": "(from tool calls)", "steps": steps}, None


def cwd_for_model(written, home):
    """The current directory as the shell tells the model (`SessionState::cwd_for_model`):
    the way the person writes it and the real location, once when they are the same."""
    real = home + written[1:] if written == "~" or written.startswith("~/") else written
    return real if written == real else f"{written} ({real})"


def unusable_output(mode, code, detail):
    """The error text when the server refused its own model's tool call, else None.

    Seen with Gemma 4 in tools mode: the model writes the same open brace until the
    server's parser stops at 64 levels and answers 400. The same request succeeds for
    other intents, so this is an unusable answer, not a failing server. Only tools
    mode qualifies: in grammar mode the server builds the output from the schema.
    """
    if mode == "tools" and code == 400 and "nested more than" in detail:
        return NESTED_WITHOUT_END
    return None


def build_body(mode, system, text, retry, settings):
    """The body of one chat request, as `intent-shell/src/model.rs` builds it
    (system prompt, then `Current directory: ...` and `Request: ...`, then the
    refused answer and why).

    `settings` carries cwd (as `cwd_for_model` writes it), temperature, max_tokens,
    sampling (extra fields), no_thinking, and either `response_format` (grammar) or `tools` (tools).
    """
    messages = [
        {"role": "system", "content": system},
        {"role": "user", "content": f"Current directory: {settings['cwd']}\nRequest: {text}"},
    ]
    if retry:
        messages.append({"role": "assistant", "content": retry["previous"]})
        messages.append({"role": "user", "content": f"That document was refused: {retry['error']}. Reply with a corrected document."})
    body = {
        "temperature": settings["temperature"],
        "max_tokens": settings["max_tokens"],
        "cache_prompt": True,
        "messages": messages,
    }
    if mode == "grammar":
        body["response_format"] = settings["response_format"]
    else:
        body["tools"] = settings["tools"]
        body["tool_choice"] = "auto"
        body["parallel_tool_calls"] = True
    body.update(settings.get("sampling", {}))
    if settings.get("no_thinking"):
        body["chat_template_kwargs"] = {"enable_thinking": False}
    return body
