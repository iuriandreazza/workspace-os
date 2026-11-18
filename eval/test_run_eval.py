"""Wiring tests for the runner (no model, no network, and no `jsonschema`: a stand-in that accepts every
document is put in its place, because the gate has the standard library only)."""
import argparse
import importlib.util
import io
import json
import sys
import types
import unittest
import urllib.error
from pathlib import Path

sys.dont_write_bytecode = True  # set before the import, so the gate leaves no .pyc beside the script

stub = types.ModuleType("jsonschema")
stub.Draft202012Validator = type("Draft202012Validator", (), {
    "__init__": lambda self, schema: None,
    "iter_errors": lambda self, document: [],
    "check_schema": staticmethod(lambda schema: None),
})
exceptions = types.ModuleType("jsonschema.exceptions")
exceptions.best_match = lambda errors: None
sys.modules.setdefault("jsonschema", stub)
sys.modules.setdefault("jsonschema.exceptions", exceptions)

spec = importlib.util.spec_from_file_location("run_eval", Path(__file__).with_name("run_eval.py"))
run_eval = importlib.util.module_from_spec(spec)
spec.loader.exec_module(run_eval)

SCHEMA_TEXT = run_eval.SCHEMA_PATH.read_text(encoding="utf-8")
SET = run_eval.score.load_set(Path(__file__).with_name("intents.json"))
INTENT = next(i for i in SET["intents"] if i["id"] == "list-downloads")
PLAN = {"kind": "plan", "version": 3, "summary": "list", "steps": [
    {"id": "s1", "action": "fs.list", "args": {"target": {"place": "downloads"}}, "depends_on": [], "elevate": False}]}


def runner(mode="grammar", **overrides):
    args = argparse.Namespace(mode=mode, derive_depends=False, prompt_extra=None, temperature=0, max_tokens=1024,
                              sampling="{}", no_thinking=False, retry=False, url="http://unused", timeout=1)
    for key, value in overrides.items():
        setattr(args, key, value)
    return run_eval.Runner(args, SCHEMA_TEXT, json.loads(SCHEMA_TEXT), SET["home"])


def reply(message):
    return {"choices": [{"message": message, "finish_reason": "stop"}], "timings": {"prompt_n": 10, "predicted_n": 5, "predicted_per_second": 50.0}}


def serving(replies):
    """Stands in for `post`: each call returns, or raises, the next item; the bodies it was sent are kept."""
    sent = []

    def post(url, body, timeout):
        sent.append(body)
        item = replies.pop(0)
        if isinstance(item, Exception):
            raise item
        return item, 0.25

    return post, sent


def http_error(code, text):
    return urllib.error.HTTPError("http://unused", code, "error", {}, io.BytesIO(text.encode()))


class Attempts(unittest.TestCase):
    def setUp(self):
        self.original = run_eval.post

    def tearDown(self):
        run_eval.post = self.original

    def attempt(self, runner, *replies):
        run_eval.post, self.sent = serving(list(replies))
        return runner.attempt(INTENT)

    def test_a_good_constrained_answer_is_scored_exact(self):
        record = self.attempt(runner(), reply({"content": json.dumps(PLAN)}))
        self.assertTrue(record["valid"])
        self.assertEqual(record["level"], 3)

    def test_the_request_names_the_directory_the_person_stands_in(self):
        self.attempt(runner(), reply({"content": json.dumps(PLAN)}))
        self.assertEqual(self.sent[0]["messages"][1]["content"], "Current directory: ~ (/home/user)\nRequest: list my downloads")

    def test_the_constrained_request_carries_the_schema_and_no_tools(self):
        self.attempt(runner(), reply({"content": json.dumps(PLAN)}))
        body = self.sent[0]
        self.assertEqual(body["response_format"]["json_schema"]["schema"], json.loads(SCHEMA_TEXT))
        self.assertNotIn("tools", body)
        self.assertEqual((body["temperature"], body["max_tokens"]), (0, 1024))

    def test_the_tools_request_carries_the_tools_and_derived_ones_have_no_depends_on(self):
        self.attempt(runner("tools"), reply({"content": "text"}))
        body = self.sent[0]
        self.assertEqual((body["tool_choice"], body["parallel_tool_calls"]), ("auto", True))
        self.assertNotIn("response_format", body)
        self.assertIn("depends_on", json.dumps(body["tools"]))
        self.attempt(runner("tools", derive_depends=True), reply({"content": "text"}))
        self.assertNotIn("depends_on", json.dumps(self.sent[0]["tools"]))

    def test_thinking_off_and_the_sampler_reach_the_request(self):
        self.attempt(runner(no_thinking=True, sampling='{"top_k": 20}'), reply({"content": json.dumps(PLAN)}))
        self.assertEqual(self.sent[0]["chat_template_kwargs"], {"enable_thinking": False})
        self.assertEqual(self.sent[0]["top_k"], 20)

    def test_a_document_the_schema_refuses_is_invalid_with_the_schemas_message(self):
        original = run_eval.best_match
        run_eval.best_match = lambda errors: types.SimpleNamespace(message="'x' is not valid")
        self.addCleanup(setattr, run_eval, "best_match", original)
        record = self.attempt(runner(), reply({"content": json.dumps(PLAN)}))
        self.assertEqual((record["valid"], record["errors"]), (False, ["'x' is not valid"]))

    def test_a_good_tool_call_is_scored_exact(self):
        call = {"function": {"name": "fs_list", "arguments": json.dumps({"target": {"place": "downloads"}, "depends_on": [], "elevate": False})}}
        record = self.attempt(runner("tools"), reply({"content": "", "tool_calls": [call]}))
        self.assertEqual((record["valid"], record["level"]), (True, 3))

    def test_text_that_is_not_json_is_an_invalid_answer(self):
        record = self.attempt(runner(), reply({"content": "sure!"}))
        self.assertFalse(record["valid"])
        self.assertIn("not JSON", record["errors"][0])

    def test_a_tool_call_nested_without_end_is_an_invalid_answer_not_a_failed_request(self):
        record = self.attempt(runner("tools"), http_error(400, '{"error":{"message":"input is nested more than 64 levels deep"}}'))
        self.assertEqual(record["errors"], [run_eval.modes.NESTED_WITHOUT_END])
        self.assertFalse(record["valid"])

    def test_the_same_server_error_in_the_constrained_mode_is_a_failed_request(self):
        record = self.attempt(runner(), http_error(400, '{"error":{"message":"input is nested more than 64 levels deep"}}'))
        self.assertTrue(record["errors"][0].startswith("request failed"))

    def test_any_other_server_error_is_a_failed_request(self):
        record = self.attempt(runner("tools"), http_error(500, "boom"))
        self.assertTrue(record["errors"][0].startswith("request failed"))

    def test_a_reply_without_a_message_is_a_failed_request(self):
        record = self.attempt(runner(), {"choices": []})
        self.assertTrue(record["errors"][0].startswith("request failed"))


class Retries(unittest.TestCase):
    def setUp(self):
        self.original = run_eval.post

    def tearDown(self):
        run_eval.post = self.original

    def test_a_refused_constrained_answer_is_sent_back_once_with_the_reason(self):
        run_eval.post, sent = serving([reply({"content": "sure!"}), reply({"content": json.dumps(PLAN)})])
        record = runner(retry=True).run(INTENT)
        self.assertFalse(record["valid"])
        self.assertTrue(record["retry"]["valid"])
        self.assertEqual(sent[1]["messages"][2], {"role": "assistant", "content": "sure!"})
        self.assertIn("That document was refused: not JSON", sent[1]["messages"][3]["content"])

    def test_a_good_first_answer_is_not_retried_and_tools_mode_never_is(self):
        run_eval.post, sent = serving([reply({"content": json.dumps(PLAN)})])
        self.assertNotIn("retry", runner(retry=True).run(INTENT))
        self.assertEqual(len(sent), 1)
        run_eval.post, sent = serving([reply({"content": "text"})])
        self.assertNotIn("retry", runner("tools", retry=True).run(INTENT))


if __name__ == "__main__":
    unittest.main()
