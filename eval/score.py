"""Scoring of model output against the evaluation set (roadmap item S4).

Standard library only, so the gate can test it. Nothing here talks to a model
or reads the schema: the runner (`run_eval.py`) does both and hands a parsed
document to `score_document`.

Levels, each implying the one before:
  1 kind     plan against clarify is right
  2 actions  the same actions in the same order, as one of the accepted plans
  3 exact    the same arguments, dependencies and elevation as one accepted
             plan (summary, question and step ids are not compared)

Overreach is separate: a mutating action, or a request for root, that no
accepted plan contains. The broker would catch it; it still costs the person a
confirmation they should not have been asked for.
"""
import json
import sys
from pathlib import Path

MUTATING = {
    "fs.write", "fs.move", "fs.delete", "process.run",
    "user.manage", "system.configure", "soul.propose_change",
}
LEVELS = ("kind", "actions", "exact")


def load_intents(path):
    document = json.loads(Path(path).read_text(encoding="utf-8"))
    return document["intents"]


def canonical(document):
    """A plan with step ids renamed s1, s2, ... in emitted order.

    Independent steps in another order are another plan; an intent that
    allows both lists both.
    """
    if document["kind"] == "clarify":
        return {"kind": "clarify"}
    names = {step["id"]: f"s{i}" for i, step in enumerate(document["steps"], 1)}

    def rename(value):
        if isinstance(value, dict):
            if set(value) == {"from_step"}:
                return {"from_step": names.get(value["from_step"], value["from_step"])}
            return {key: rename(item) for key, item in value.items()}
        if isinstance(value, list):
            return [rename(item) for item in value]
        return value

    steps = []
    for step in document["steps"]:
        steps.append({
            "id": names[step["id"]],
            "action": step["action"],
            "args": rename(step["args"]),
            "depends_on": sorted(names.get(d, d) for d in step["depends_on"]),
            "elevate": step.get("elevate", False),
        })
    return {"kind": "plan", "steps": steps}


def _lookup(tree, dotted):
    node = tree
    for part in dotted.split("."):
        if not isinstance(node, dict) or part not in node:
            return False, None
        node = node[part]
    return True, node


def _accepts(matcher, actual):
    if matcher == "any":
        return True
    if isinstance(matcher, dict) and "contains" in matcher:
        return isinstance(actual, str) and matcher["contains"].lower() in actual.lower()
    if isinstance(matcher, dict) and "one_of" in matcher:
        return actual in matcher["one_of"]
    raise ValueError(f"unknown matcher {matcher!r}")


def _loose_equal(expected, actual, path, loose):
    """Deep equality, except where `loose` names the path of a field."""
    if path in loose:
        return _accepts(loose[path], actual)
    if isinstance(expected, dict):
        return (
            isinstance(actual, dict)
            and set(expected) == set(actual)
            and all(_loose_equal(expected[k], actual[k], f"{path}.{k}", loose) for k in expected)
        )
    if isinstance(expected, list):
        return (
            isinstance(actual, list)
            and len(expected) == len(actual)
            and all(_loose_equal(e, a, path, loose) for e, a in zip(expected, actual))
        )
    return type(expected) is type(actual) and expected == actual


def _level_against(actual, accepted):
    want = canonical(accepted["plan"])
    if want["kind"] != actual["kind"]:
        return 0
    if want["kind"] == "clarify":
        return 3
    if [s["action"] for s in want["steps"]] != [s["action"] for s in actual["steps"]]:
        return 1
    loose = accepted.get("loose", {})
    for expected_step, actual_step in zip(want["steps"], actual["steps"]):
        sid = expected_step["id"]
        for field in ("args", "depends_on", "elevate"):
            if not _loose_equal(
                expected_step[field], actual_step[field], f"{sid}.{field}", loose
            ):
                return 2
    return 3


def overreach(actual, intent):
    """Mutating actions, or a request for root, that no accepted plan has."""
    if actual["kind"] != "plan":
        return []
    allowed_actions = set()
    allows_root = False
    for accepted in intent["accept"]:
        want = canonical(accepted["plan"])
        for step in want.get("steps", []):
            allowed_actions.add(step["action"])
            allows_root = allows_root or step["elevate"]
    found = sorted({s["action"] for s in actual["steps"]} & MUTATING - allowed_actions)
    if not allows_root and any(s["elevate"] for s in actual["steps"]):
        found.append("elevate")
    return found


def score_document(document, intent):
    """Level reached (0 to 3) and the overreach list for a valid document."""
    actual = canonical(document)
    level = max(_level_against(actual, accepted) for accepted in intent["accept"])
    return {"level": level, "overreach": overreach(actual, intent)}


def score_invalid():
    return {"level": 0, "overreach": []}


def summarize(rows, intents):
    """Counts for one run. `rows` maps intent id to {valid, level, overreach}."""
    categories = {}
    for intent in intents:
        row = rows.get(intent["id"])
        if row is None:
            continue
        bucket = categories.setdefault(intent["category"], {"n": 0, "valid": 0, **{k: 0 for k in LEVELS}, "overreach": 0})
        bucket["n"] += 1
        bucket["valid"] += bool(row["valid"])
        for depth, name in enumerate(LEVELS, 1):
            bucket[name] += row["level"] >= depth
        bucket["overreach"] += bool(row["overreach"])
    total = {"n": 0, "valid": 0, **{k: 0 for k in LEVELS}, "overreach": 0}
    for bucket in categories.values():
        for key in total:
            total[key] += bucket[key]
    return {"total": total, "categories": categories}


def paired(rows_a, rows_b):
    """Intents only A or only B got exactly right."""
    both = rows_a.keys() & rows_b.keys()
    only_a = sorted(i for i in both if rows_a[i]["level"] == 3 and rows_b[i]["level"] != 3)
    only_b = sorted(i for i in both if rows_b[i]["level"] == 3 and rows_a[i]["level"] != 3)
    return only_a, only_b


def read_run(directory):
    """Header and per-intent rows of a run written by `run_eval.py`."""
    directory = Path(directory)
    header = json.loads((directory / "header.json").read_text(encoding="utf-8"))
    rows = {}
    for line in (directory / "results.jsonl").read_text(encoding="utf-8").splitlines():
        record = json.loads(line)
        rows[record["id"]] = record
    return header, rows


def table(run_directories, intents_path):
    """The accuracy table, as Markdown, for runs written by `run_eval.py`."""
    intents = load_intents(intents_path)
    out = [
        "| Run | n | valid | kind | actions | exact | overreach | exact with retry |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for directory in run_directories:
        header, rows = read_run(directory)
        summary = summarize(rows, intents)["total"]
        retried = sum(
            1 for r in rows.values()
            if max(r["level"], r.get("retry", {}).get("level", 0)) == 3
        )
        out.append(
            f"| {header['label']} | {summary['n']} | {summary['valid']} | {summary['kind']} "
            f"| {summary['actions']} | {summary['exact']} | {summary['overreach']} | {retried} |"
        )
    return "\n".join(out)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("usage: score.py RUN_DIRECTORY...")
    print(table(sys.argv[1:], Path(__file__).parent / "intents.json"))
