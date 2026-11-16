"""Unit tests for the prompts and the native tool calls of the evaluation (no model, no network)."""
import importlib.util
import json
import sys
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # set before the import, so the gate leaves no .pyc beside the script

spec = importlib.util.spec_from_file_location("modes", Path(__file__).with_name("modes.py"))
modes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(modes)

SCHEMA = json.loads((modes.ROOT / "docs/schemas/action-plan.schema.json").read_text(encoding="utf-8"))
TOOLS, BY_NAME = modes.build_tools(SCHEMA)


def call(name, **arguments):
    return {"function": {"name": name, "arguments": json.dumps(arguments)}}


def calls(*items):
    return {"content": "", "tool_calls": list(items)}


class Prompts(unittest.TestCase):
    def test_the_real_shell_prompt_is_found(self):
        prompt = modes.shell_prompt()
        self.assertTrue(prompt.startswith("You turn the user's request into one JSON document"))
        self.assertIn("Do not ask a question when the request is clear.", prompt)

    def test_only_the_raw_string_is_taken(self):
        source = 'const SYSTEM_PROMPT: &str = r#"line one\n"quoted"\nline three"#;\nconst OTHER: &str = r#"no"#;'
        self.assertEqual(modes.shell_prompt(source), 'line one\n"quoted"\nline three')

    def test_a_missing_prompt_is_an_error_not_an_empty_prompt(self):
        with self.assertRaises(ValueError):
            modes.shell_prompt("fn main() {}")

    def test_the_tools_prompt_drops_the_json_document_instructions_and_keeps_the_rules(self):
        prompt = modes.tools_prompt(modes.shell_prompt())
        self.assertNotIn("You turn the user's request into one JSON document", prompt)
        self.assertNotIn('"kind":"plan"', prompt)
        self.assertIn("Things are never named by a path.", prompt)
        self.assertIn("There is no current directory", prompt)
        self.assertIn("depends_on [] (an empty list)", prompt)

    def test_a_prompt_without_the_reference_rules_is_an_error(self):
        with self.assertRaises(ValueError):
            modes.tools_prompt("something else")


class Tools(unittest.TestCase):
    def test_there_is_one_tool_per_action_and_one_to_ask(self):
        names = [t["function"]["name"] for t in TOOLS]
        self.assertEqual(len(names), 12)
        self.assertEqual(names[-1], "clarify")
        self.assertEqual(sorted(i["action"] for i in BY_NAME.values()), sorted(
            s["properties"]["action"]["const"] for s in (SCHEMA["$defs"][n] for n in modes.STEP_NAMES)))

    def test_no_reference_is_left_in_a_tool(self):
        self.assertNotIn("$ref", json.dumps(TOOLS))

    def test_the_actions_without_an_elevate_field_get_none(self):
        for name in ("user_manage", "system_configure", "soul_propose_change"):
            self.assertFalse(BY_NAME[name]["has_elevate"])
            parameters = next(t for t in TOOLS if t["function"]["name"] == name)["function"]["parameters"]
            self.assertNotIn("elevate", parameters["properties"])
        self.assertTrue(BY_NAME["fs_delete"]["has_elevate"])

    def test_every_required_argument_of_the_schema_is_required_of_the_tool(self):
        parameters = next(t for t in TOOLS if t["function"]["name"] == "fs_find_largest")["function"]["parameters"]
        self.assertEqual(sorted(parameters["required"]), ["count", "depends_on", "elevate", "target"])
        self.assertEqual(parameters["properties"]["count"]["maximum"], 100)


class Calls(unittest.TestCase):
    LIST = call("fs_list", target={"place": "downloads"}, depends_on=[], elevate=False)

    def test_a_call_becomes_a_step_and_calls_are_numbered_in_order(self):
        document, error = modes.document_from_calls(calls(
            self.LIST, call("fs_delete", target={"from_step": "s1"}, depends_on=["s1"], elevate=True)), BY_NAME)
        self.assertIsNone(error)
        self.assertEqual([s["id"] for s in document["steps"]], ["s1", "s2"])
        self.assertEqual(document["steps"][1], {
            "id": "s2", "action": "fs.delete", "args": {"target": {"from_step": "s1"}},
            "depends_on": ["s1"], "elevate": True})

    def test_a_reply_with_no_call_is_a_failure_not_a_question(self):
        document, error = modes.document_from_calls({"content": "Which file?", "tool_calls": None}, BY_NAME)
        self.assertIsNone(document)
        self.assertIn("plain text", error)

    def test_clarify_alone_is_a_question_and_mixed_with_a_step_is_refused(self):
        document, _ = modes.document_from_calls(calls(call("clarify", question="which?")), BY_NAME)
        self.assertEqual(document, {"kind": "clarify", "version": 2, "question": "which?"})
        document, error = modes.document_from_calls(calls(call("clarify", question="which?"), self.LIST), BY_NAME)
        self.assertIsNone(document)
        self.assertIn("mixed", error)

    def test_nothing_is_repaired(self):
        for missing in ("depends_on", "elevate"):
            arguments = {"target": {"place": "downloads"}, "depends_on": [], "elevate": False}
            del arguments[missing]
            document, error = modes.document_from_calls(calls(call("fs_list", **arguments)), BY_NAME)
            self.assertIsNone(document)
            self.assertIn(missing, error)

    def test_an_argument_the_schema_does_not_have_stays_for_the_schema_to_refuse(self):
        document, _ = modes.document_from_calls(calls(call(
            "fs_list", target={"place": "downloads"}, path="/etc", depends_on=[], elevate=False)), BY_NAME)
        self.assertEqual(document["steps"][0]["args"]["path"], "/etc")

    def test_an_elevate_on_an_action_that_has_none_stays_for_the_schema_to_refuse(self):
        document, _ = modes.document_from_calls(calls(call(
            "user_manage", operation="add", user="bia", group=None, depends_on=[], elevate=True)), BY_NAME)
        self.assertIn("elevate", document["steps"][0])

    def test_unknown_tools_and_bad_arguments_are_failures(self):
        self.assertIn("unknown tool", modes.document_from_calls(calls(call("rm_rf", depends_on=[])), BY_NAME)[1])
        bad = {"tool_calls": [{"function": {"name": "fs_list", "arguments": "{not json"}}]}
        self.assertIn("not JSON", modes.document_from_calls(bad, BY_NAME)[1])
        listed = {"tool_calls": [{"function": {"name": "fs_list", "arguments": "[]"}}]}
        self.assertIn("not an object", modes.document_from_calls(listed, BY_NAME)[1])

    def test_arguments_may_arrive_already_parsed(self):
        message = {"tool_calls": [{"function": {"name": "fs_list", "arguments": {"target": {"place": "home"}, "depends_on": [], "elevate": False}}}]}
        document, error = modes.document_from_calls(message, BY_NAME)
        self.assertIsNone(error)
        self.assertEqual(document["steps"][0]["args"], {"target": {"place": "home"}})


class Derived(unittest.TestCase):
    TOOLS_DERIVED, BY_NAME_DERIVED = modes.build_tools(SCHEMA, derive_depends=True)

    def test_the_tools_have_no_depends_on_and_the_prompt_does_not_ask_for_one(self):
        self.assertNotIn("depends_on", json.dumps(self.TOOLS_DERIVED))
        self.assertNotIn("depends_on", modes.tools_prompt(modes.shell_prompt(), derive_depends=True).split("\n", 1)[0])

    def test_dependencies_come_from_the_references(self):
        document, error = modes.document_from_calls(calls(
            call("fs_find_largest", target={"place": "downloads"}, count=2, elevate=False),
            call("fs_stat", target={"from_step": "s1"}, elevate=False),
            call("fs_move", **{"from": {"from_step": "s1"}, "to": {"place": "documents"}, "elevate": False}),
        ), self.BY_NAME_DERIVED, derive_depends=True)
        self.assertIsNone(error)
        self.assertEqual([s["depends_on"] for s in document["steps"]], [[], ["s1"], ["s1"]])


class Names(unittest.TestCase):
    def test_the_action_name_with_a_dot_is_the_same_tool(self):
        document, error = modes.document_from_calls(calls(call("fs.list", target={"place": "home"}, depends_on=[], elevate=False)), BY_NAME)
        self.assertIsNone(error)
        self.assertEqual(document["steps"][0]["action"], "fs.list")

    def test_clarify_with_another_argument_is_refused(self):
        document, error = modes.document_from_calls(calls(call("clarify", question="?", extra=1)), BY_NAME)
        self.assertIsNone(document)
        self.assertIn("other than question", error)


class Body(unittest.TestCase):
    SETTINGS = {"temperature": 0, "max_tokens": 1024, "response_format": {"type": "json_schema"}, "tools": TOOLS}

    def test_the_grammar_request_is_what_the_shell_sends(self):
        body = modes.build_body("grammar", "SYSTEM", "list files", None, self.SETTINGS)
        self.assertEqual(body, {
            "temperature": 0, "max_tokens": 1024, "cache_prompt": True,
            "messages": [{"role": "system", "content": "SYSTEM"}, {"role": "user", "content": "Request: list files"}],
            "response_format": {"type": "json_schema"},
        })

    def test_a_retry_adds_the_refused_answer_and_the_reason_as_the_shell_does(self):
        body = modes.build_body("grammar", "S", "x", {"previous": "{bad", "error": "not JSON"}, self.SETTINGS)
        self.assertEqual(body["messages"][2], {"role": "assistant", "content": "{bad"})
        self.assertEqual(body["messages"][3]["content"], "That document was refused: not JSON. Reply with a corrected document.")

    def test_the_tools_request_lets_the_model_decide_and_allows_several_calls(self):
        body = modes.build_body("tools", "S", "x", None, self.SETTINGS)
        self.assertEqual((body["tool_choice"], body["parallel_tool_calls"]), ("auto", True))
        self.assertNotIn("response_format", body)

    def test_thinking_off_and_the_sampler_are_sent_only_when_asked(self):
        plain = modes.build_body("grammar", "S", "x", None, self.SETTINGS)
        self.assertNotIn("chat_template_kwargs", plain)
        tuned = modes.build_body("grammar", "S", "x", None, {**self.SETTINGS, "no_thinking": True, "sampling": {"top_k": 20}})
        self.assertEqual(tuned["chat_template_kwargs"], {"enable_thinking": False})
        self.assertEqual(tuned["top_k"], 20)


if __name__ == "__main__":
    unittest.main()
