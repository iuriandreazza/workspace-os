"""Check the action plan schema against its examples (ADR 0011, ADR 0014, ADR 0018).

Run: uv run --with jsonschema python docs/schemas/validate.py

examples/valid/*.json         must pass the schema and the semantic rules.
examples/invalid/schema-*.json   must be refused by the schema.
examples/invalid/semantic-*.json must pass the schema and be refused by the
                                 semantic rules, which JSON Schema cannot express.
"""
import json
import sys
from pathlib import Path

from jsonschema import Draft202012Validator

HERE = Path(__file__).parent
PATH_PRODUCERS = {"fs.list", "fs.find_largest"}
GROUP_OPERATIONS = {"add_to_group", "remove_from_group"}


def semantic_errors(doc):
    if doc["kind"] != "plan":
        return []
    steps = doc["steps"]
    ids = [s["id"] for s in steps]
    errors = []
    if len(set(ids)) != len(ids):
        errors.append("duplicate step id")
    by_id = {s["id"]: s for s in steps}
    for s in steps:
        for dep in s["depends_on"]:
            if dep == s["id"]:
                errors.append(f"{s['id']} depends on itself")
            elif dep not in by_id:
                errors.append(f"{s['id']} depends on unknown step {dep}")
        for value in s["args"].values():
            if isinstance(value, dict) and "from_step" in value:
                source = value["from_step"]
                if source not in s["depends_on"]:
                    errors.append(f"{s['id']} uses {source} without depending on it")
                elif source in by_id and by_id[source]["action"] not in PATH_PRODUCERS:
                    errors.append(f"{source} does not produce paths")
            if isinstance(value, dict):
                paths = [value.get("path"), (value.get("in") or {}).get("path") if isinstance(value.get("in"), dict) else None]
                for text in paths:
                    if text is not None and (len(text.encode()) > 4096 or "\0" in text):
                        errors.append(f"{s['id']}: a path has 1 to 4096 bytes and no NUL")
                name = value.get("name", value.get("new_name"))
                if name is not None and (name in (".", "..") or "/" in name or "\0" in name or len(name.encode()) > 255):
                    errors.append(f"{s['id']}: {name!r} is not a single file name")
        if s["action"] == "user.manage":
            has_group = s["args"]["group"] is not None
            if has_group != (s["args"]["operation"] in GROUP_OPERATIONS):
                errors.append(f"{s['id']}: group must be set exactly for group operations")
    if not errors and has_cycle(by_id):
        errors.append("dependency cycle")
    return errors


def has_cycle(by_id):
    state = {}

    def visit(node):
        if state.get(node) == "done":
            return False
        if state.get(node) == "active":
            return True
        state[node] = "active"
        cyclic = any(visit(dep) for dep in by_id[node]["depends_on"])
        state[node] = "done"
        return cyclic

    return any(visit(node) for node in by_id)


def main():
    schema = json.loads((HERE / "action-plan.schema.json").read_text())
    Draft202012Validator.check_schema(schema)
    validator = Draft202012Validator(schema)
    failures = []
    for path in sorted((HERE / "examples/valid").glob("*.json")):
        doc = json.loads(path.read_text())
        problems = [e.message for e in validator.iter_errors(doc)] or semantic_errors(doc)
        if problems:
            failures.append(f"{path.name}: should pass, got {problems[0]}")
    for path in sorted((HERE / "examples/invalid").glob("*.json")):
        doc = json.loads(path.read_text())
        schema_problems = list(validator.iter_errors(doc))
        if path.name.startswith("schema-"):
            if not schema_problems:
                failures.append(f"{path.name}: schema should refuse it")
        elif path.name.startswith("semantic-"):
            if schema_problems:
                failures.append(f"{path.name}: schema should accept it")
            elif not semantic_errors(doc):
                failures.append(f"{path.name}: semantic rules should refuse it")
        else:
            failures.append(f"{path.name}: name must start with schema- or semantic-")
    for line in failures:
        print("FAIL", line)
    count = sum(1 for _ in HERE.glob("examples/*/*.json"))
    print(f"{count} examples checked, {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
