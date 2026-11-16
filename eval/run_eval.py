"""Run the evaluation set against one `llama-server` and write the evidence.

    uv run --with jsonschema python eval/run_eval.py --url http://127.0.0.1:8089 \
        --mode grammar --label bonsai-8b-grammar --model-file PATH.gguf

Two modes, the model being the same:

  grammar  The shipped path (ADR 0011): the system prompt of
           intent-shell/src/model.rs, read from that file so the two cannot
           drift, and the schema file sent as `response_format`.
  tools    Native tool calls: one function per action, built from the same
           schema file, and a `clarify` function. The harness numbers the calls
           s1, s2, ... in the order the model made them and fills the summary.

Each run directory holds `header.json` (what was run, with hashes) and
`results.jsonl` (one record per intent, with the raw output), so every figure
in the report can be recomputed with `eval/score.py`.
"""
import argparse
import datetime
import hashlib
import http.client
import importlib.metadata
import importlib.util
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from jsonschema import Draft202012Validator
from jsonschema.exceptions import best_match

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
SCHEMA_PATH = ROOT / "docs/schemas/action-plan.schema.json"

sys.dont_write_bytecode = True


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


score = load_module("score", HERE / "score.py")
rules = load_module("validate", ROOT / "docs/schemas/validate.py")
modes = load_module("modes", HERE / "modes.py")

def sha256(data):
    return hashlib.sha256(data).hexdigest()


def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def check_document(document, validator):
    """Schema errors, else the semantic rules, else what only the shell's parser
    refuses (see `score.strict_errors`)."""
    refusal = best_match(validator.iter_errors(document))
    if refusal is not None:
        return [refusal.message[:300]]
    try:
        return rules.semantic_errors(document) or score.strict_errors(document)
    except UnicodeError as error:  # a lone surrogate in a name, which the shell's parser refuses too
        return [f"text that cannot be encoded: {error}"]


def post(url, body, timeout):
    request = urllib.request.Request(
        url + "/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    started = time.monotonic()
    with urllib.request.urlopen(request, timeout=timeout) as response:
        reply = json.load(response)
    return reply, time.monotonic() - started


def get(url, path):
    try:
        with urllib.request.urlopen(url + path, timeout=10) as response:
            return json.load(response)
    except (urllib.error.URLError, json.JSONDecodeError, OSError):
        return None


class Runner:
    def __init__(self, args, schema_text, schema):
        self.args = args
        self.schema = schema
        self.validator = Draft202012Validator(schema)
        self.system = modes.shell_prompt()
        if args.prompt_extra:
            self.system += "\n" + Path(args.prompt_extra).read_text(encoding="utf-8").rstrip("\n")
        if args.mode == "grammar":
            self.response_format = {"type": "json_schema", "json_schema": {"name": "action_plan", "schema": json.loads(schema_text)}}
        else:
            self.tools, self.by_name = modes.build_tools(schema, args.derive_depends)
            self.system = modes.tools_prompt(self.system, args.derive_depends)

    def body(self, text, retry=None):
        return modes.build_body(self.args.mode, self.system, text, retry, {
            "temperature": self.args.temperature,
            "max_tokens": self.args.max_tokens,
            "sampling": json.loads(self.args.sampling),
            "no_thinking": self.args.no_thinking,
            "response_format": getattr(self, "response_format", None),
            "tools": getattr(self, "tools", None),
        })

    def attempt(self, intent, retry=None):
        try:
            reply, wall = post(self.args.url, self.body(intent["text"], retry), self.args.timeout)
        except (urllib.error.URLError, OSError, http.client.HTTPException, ValueError, RecursionError) as error:
            return {"valid": False, "level": 0, "overreach": [], "errors": [f"request failed: {error}"], "output": None}
        message = reply["choices"][0]["message"]
        timings = reply.get("timings", {})
        record = {
            "output": message.get("content") if self.args.mode == "grammar" else {"content": message.get("content"), "tool_calls": message.get("tool_calls")},
            "reasoning": bool(message.get("reasoning_content")),
            "finish_reason": reply["choices"][0].get("finish_reason"),
            "wall_s": round(wall, 3),
            "prompt_n": timings.get("prompt_n"),
            "cache_n": timings.get("cache_n"),
            "predicted_n": timings.get("predicted_n"),
            "predicted_per_second": round(timings["predicted_per_second"], 1) if timings.get("predicted_per_second") else None,
        }
        document, error = self.read(message)
        problems = [error] if error else check_document(document, self.validator)
        record["errors"] = problems
        record["valid"] = not problems
        if record["valid"]:
            record["document"] = document
            record.update(score.score_document(document, intent))
        else:
            record.update(score.score_invalid())
        return record

    def read(self, message):
        if self.args.mode == "tools":
            return modes.document_from_calls(message, self.by_name, self.args.derive_depends)
        text = message.get("content") or ""
        try:
            document = json.loads(text)
        except (ValueError, RecursionError) as error:  # JSONDecodeError is a ValueError
            return None, f"not JSON: {error}"
        if not isinstance(document, dict) or "kind" not in document:
            return None, "not a plan or a question"
        return document, None

    def run(self, intent):
        first = self.attempt(intent)
        record = {"id": intent["id"], "text": intent["text"], **first}
        if self.args.retry and not first["valid"] and self.args.mode == "grammar" and first["output"] is not None:
            record["retry"] = self.attempt(intent, {"previous": first["output"], "error": first["errors"][0]})
        return record


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--url", required=True)
    parser.add_argument("--mode", choices=("grammar", "tools"), required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--model-file", help="the GGUF the server loaded, for its SHA-256")
    parser.add_argument("--server-binary", help="the llama-server executable, for its SHA-256")
    parser.add_argument("--server-flags", default="", help="the llama-server command line, recorded as given")
    parser.add_argument("--out", help="directory for the run (default eval/results/LABEL)")
    parser.add_argument("--no-thinking", action="store_true", help='send chat_template_kwargs {"enable_thinking": false}')
    parser.add_argument("--derive-depends", action="store_true", help="tools mode: the harness works out depends_on from the from_step references, and the model is not asked for it")
    parser.add_argument("--retry", action="store_true", help="one retry after an invalid answer, as the shell does (grammar mode)")
    parser.add_argument("--prompt-extra", help="text appended to the system prompt (S8: a personality preset)")
    parser.add_argument("--temperature", type=float, default=0)
    parser.add_argument("--sampling", default="{}", help="more sampler fields as a JSON object (top_p, top_k, presence_penalty, ...)")
    parser.add_argument("--max-tokens", type=int, default=1024)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--only", nargs="+", help="run only these intent ids")
    args = parser.parse_args()

    schema_text = SCHEMA_PATH.read_text(encoding="utf-8")
    schema = json.loads(schema_text)
    Draft202012Validator.check_schema(schema)
    validator = Draft202012Validator(schema)
    intents = score.load_intents(HERE / "intents.json")
    for intent in intents:
        for accepted in intent["accept"]:
            problems = check_document(accepted["plan"], validator)
            if problems:
                sys.exit(f"run_eval: the set is broken, {intent['id']} accepts an invalid plan: {problems[0]}")
    if args.only:
        known = {i["id"] for i in intents}
        missing = set(args.only) - known
        if missing:
            sys.exit(f"run_eval: unknown intent ids {sorted(missing)}")
        intents = [i for i in intents if i["id"] in args.only]

    runner = Runner(args, schema_text, schema)
    out = Path(args.out) if args.out else HERE / "results" / args.label
    if (out / "results.jsonl").exists():
        sys.exit(f"run_eval: {out} already holds a run; remove it or choose another --label")
    out.mkdir(parents=True, exist_ok=True)
    props = get(args.url, "/props") or {}
    header = {
        "label": args.label,
        "mode": args.mode,
        "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "repository_commit": subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip(),
        "inputs_modified_since_commit": bool(subprocess.run(
            ["git", "-C", str(ROOT), "status", "--porcelain", "--", "eval", ":!eval/results", "docs/schemas", "intent-shell/src/model.rs"],
            capture_output=True, text=True).stdout.strip()),
        "server_build": props.get("build_info"),
        "server_context": (props.get("default_generation_settings") or {}).get("n_ctx"),
        "python_version": sys.version.split()[0],
        "server_binary_sha256": file_sha256(args.server_binary) if args.server_binary else None,
        "chat_template_sha256": sha256(props["chat_template"].encode()) if props.get("chat_template") else None,
        "jsonschema_version": importlib.metadata.version("jsonschema"),
        "server_flags": args.server_flags,
        "model_file": Path(args.model_file).name if args.model_file else None,
        "model_sha256": file_sha256(args.model_file) if args.model_file else None,
        "system_prompt_sha256": sha256(runner.system.encode()),
        "schema_sha256": sha256(schema_text.encode()),
        "intents_sha256": sha256((HERE / "intents.json").read_bytes()),
        "tools_sha256": sha256(json.dumps(runner.tools, sort_keys=True).encode()) if args.mode == "tools" else None,
        "temperature": args.temperature,
        "sampling": json.loads(args.sampling),
        "max_tokens": args.max_tokens,
        "no_thinking": args.no_thinking,
        "retry": args.retry and args.mode == "grammar",
        "derive_depends": args.derive_depends and args.mode == "tools",
        "prompt_extra_sha256": sha256(Path(args.prompt_extra).read_bytes()) if args.prompt_extra else None,
        "intents": len(intents),
    }
    (out / "header.json").write_text(json.dumps(header, indent=1) + "\n", encoding="utf-8")
    with open(out / "results.jsonl", "w", encoding="utf-8") as results:
        for number, intent in enumerate(intents, 1):
            record = runner.run(intent)
            results.write(json.dumps(record, ensure_ascii=False) + "\n")
            results.flush()
            mark = ("-", "k", "a", "E")[record["level"]] if record["valid"] else "x"
            print(f"{number:3}/{len(intents)} {mark} {record['id']} ({record['wall_s'] if 'wall_s' in record else '?'} s)", flush=True)
    print(score.table([out], HERE / "intents.json"))
    failed = score.summarize(score.read_run(out, intents)[1], intents)["total"]["failed"]
    if failed:
        sys.exit(f"run_eval: {failed} requests failed; the figures of this run measure the server, not the model")


if __name__ == "__main__":
    main()
