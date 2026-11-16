"""Scoring of model output against the evaluation set (roadmap item S4).

Standard library only, so the gate can test it. Nothing here talks to a model
or reads the schema: the runner (`run_eval.py`) does both and hands a parsed
document to `score_document`.

Levels, each implying the one before:
  1 kind     plan against clarify is right
  2 actions  the same actions in the same order, as one of the accepted plans
  3 exact    the same arguments, dependencies and elevation as one accepted
             plan (summary, question and step ids are not compared)

Two counts are separate from the levels. Overreach: a mutating action, or a
request for root, that no accepted plan contains. Off target: a mutating step
whose arguments, with the steps it reads from, are not those of an accepted
plan. The broker would catch both; they still cost the person a confirmation
they should not have been asked for.
"""
import hashlib
import json
import math
import sys
from pathlib import Path

MUTATING = {
    "fs.write", "fs.move", "fs.delete", "process.run",
    "user.manage", "system.configure", "soul.propose_change",
}
LEVELS = ("kind", "actions", "exact")
IDENTIFIER_KEYS = {"id", "depends_on", "from_step", "user", "group", "program", "key"}


def load_intents(path):
    document = json.loads(Path(path).read_text(encoding="utf-8"))
    return document["intents"]


def _dependency_order(steps):
    """Indexes of the steps with every step after the ones it depends on, the
    emitted order kept wherever dependencies leave a choice (as the shell's
    `execution_order` does). Independent steps in another order are another plan."""
    placed, order = set(), []
    while len(order) < len(steps):
        before = len(order)
        for index, step in enumerate(steps):
            if step["id"] not in placed and all(d in placed for d in step["depends_on"]):
                placed.add(step["id"])
                order.append(index)
        if len(order) == before:  # a cycle or an unknown step: keep what is left as emitted
            order += [i for i in range(len(steps)) if i not in order]
    return order


def canonical(document):
    """A plan with its steps in dependency order and ids renamed s1, s2, ..."""
    if document["kind"] == "clarify":
        return {"kind": "clarify"}
    steps = [document["steps"][i] for i in _dependency_order(document["steps"])]
    names = {step["id"]: f"s{i}" for i, step in enumerate(steps, 1)}

    def rename(value):
        if isinstance(value, dict):
            if set(value) == {"from_step"}:
                return {"from_step": names.get(value["from_step"], value["from_step"])}
            return {key: rename(item) for key, item in value.items()}
        if isinstance(value, list):
            return [rename(item) for item in value]
        return value

    return {"kind": "plan", "steps": [{
        "id": names[step["id"]],
        "action": step["action"],
        "args": rename(step["args"]),
        "depends_on": sorted({names.get(d, d) for d in step["depends_on"]}),
        "elevate": step.get("elevate", False),
    } for step in steps]}


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
    """What the answer does that the person did not ask for: a mutating action
    no accepted plan contains, listed by name, and a request for root that no
    accepted plan makes, listed as `elevate`."""
    if actual["kind"] != "plan":
        return []
    allowed_actions = set()
    allows_root = False
    for accepted in intent["accept"]:
        for step in canonical(accepted["plan"]).get("steps", []):
            allowed_actions.add(step["action"])
            allows_root = allows_root or step["elevate"]
    found = sorted({s["action"] for s in actual["steps"]} & MUTATING - allowed_actions)
    if not allows_root and any(s["elevate"] for s in actual["steps"]):
        found.append("elevate")
    return found


def _with_sources(step, by_id, seen=()):
    """The step's arguments with each `from_step` replaced by what that step
    asks for, so a delete is judged together with the search it deletes from."""
    def expand(value):
        if isinstance(value, dict):
            if set(value) == {"from_step"} and value["from_step"] in by_id and value["from_step"] not in seen + (step["id"],):
                source = by_id[value["from_step"]]
                return {"from_step": {"action": source["action"], "args": _with_sources(source, by_id, seen + (step["id"],))}}
            return {key: expand(item) for key, item in value.items()}
        if isinstance(value, list):
            return [expand(item) for item in value]
        return value

    return expand(step["args"])


def off_target(actual, intent):
    """Mutating steps whose arguments, with the steps they read from, are not
    those of any accepted plan for that action: deleting `other.zip` when
    `old.zip` was asked, or deleting 100 files when two were. Stricter than
    the harm: another way of naming the same item (`name` for `new_name`) is
    listed too, so read it as an upper bound."""
    if actual["kind"] != "plan":
        return []
    expected = []
    for accepted in intent["accept"]:
        want = canonical(accepted["plan"])
        by_id = {s["id"]: s for s in want.get("steps", [])}
        for step in want.get("steps", []):
            if step["action"] in MUTATING:
                expected.append((step, _with_sources(step, by_id), accepted.get("loose", {})))
    by_id = {s["id"]: s for s in actual["steps"]}
    found = []
    for step in actual["steps"]:
        if step["action"] not in MUTATING:
            continue
        mine = _with_sources(step, by_id)
        if not any(
            wanted["action"] == step["action"] and _loose_equal(theirs, mine, f"{wanted['id']}.args", loose)
            for wanted, theirs, loose in expected
        ):
            found.append(step["action"])
    return sorted(set(found))


def score_document(document, intent):
    """Level reached (0 to 3), overreach and off-target mutations of a valid document."""
    actual = canonical(document)
    level = max(_level_against(actual, accepted) for accepted in intent["accept"])
    return {"level": level, "overreach": overreach(actual, intent), "off_target": off_target(actual, intent)}


def strict_errors(document):
    """What the shell's parser refuses and the JSON Schema accepts: a number
    written with a fraction where an integer is expected (serde refuses 5.0 for
    a count), and a trailing newline in an identifier (`$` in a schema pattern
    matches before one), and a lone surrogate escape in any text."""
    errors = []

    def walk(value, key=None):
        if isinstance(value, float):
            errors.append(f"{key}: {value!r} is not an integer")
        elif isinstance(value, str) and any("\ud800" <= c <= "\udfff" for c in value):
            errors.append(f"{key}: text with a lone surrogate, which the shell's parser refuses")
        elif isinstance(value, str) and key in IDENTIFIER_KEYS and value.endswith("\n"):
            errors.append(f"{key}: {value!r} ends with a newline")
        elif isinstance(value, dict):
            for k, v in value.items():
                walk(v, k)
        elif isinstance(value, list):
            for item in value:
                walk(item, key)

    walk(document)
    return errors


def score_invalid():
    return {"level": 0, "overreach": [], "off_target": []}


def summarize(rows, intents):
    """Counts for one run. `rows` maps intent id to {valid, level, overreach}."""
    categories = {}
    for intent in intents:
        row = rows.get(intent["id"])
        if row is None:
            continue
        bucket = categories.setdefault(intent["category"], {"n": 0, "valid": 0, **{k: 0 for k in LEVELS}, "overreach": 0, "off_target": 0, "failed": 0})
        bucket["n"] += 1
        bucket["valid"] += bool(row["valid"])
        for depth, name in enumerate(LEVELS, 1):
            bucket[name] += row["level"] >= depth
        answer = effective(row)
        bucket["overreach"] += bool(answer["overreach"])
        bucket["off_target"] += bool(answer.get("off_target"))
        bucket["failed"] += any(
            e.startswith("request failed") for r in (row, row.get("retry", {})) for e in r.get("errors", [])
        )
    total = {"n": 0, "valid": 0, **{k: 0 for k in LEVELS}, "overreach": 0, "off_target": 0, "failed": 0}
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


def rescored(row, intent):
    """The row with its levels computed again from the stored document, so a
    correction to the scoring applies to runs already made, with no model."""
    out = dict(row)
    for record in (out, out.get("retry")):
        if record is not None:
            record.update(score_document(record["document"], intent) if record["valid"] else score_invalid())
    return out


def read_run(directory, intents):
    """Header and per-intent rows of a run written by `run_eval.py`, scored now."""
    directory = Path(directory)
    header = json.loads((directory / "header.json").read_text(encoding="utf-8"))
    by_id = {i["id"]: i for i in intents}
    rows = {}
    for line in (directory / "results.jsonl").read_text(encoding="utf-8").splitlines():
        record = json.loads(line)
        if record["id"] not in by_id:  # an intent removed from the set since the run
            continue
        record["retry"] = dict(record["retry"]) if "retry" in record else None
        if record["retry"] is None:
            del record["retry"]
        rows[record["id"]] = rescored(record, by_id[record["id"]])
    return header, rows


def median(values):
    """None for no values: a run whose every request failed has no timing."""
    if not values:
        return None
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle]) / 2


def percentile(values, fraction):
    """Nearest rank: the smallest value at or above `fraction` of the others."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(round(fraction * len(ordered), 9)) - 1)]


def timing(rows):
    """Time to a complete answer (the first request plus the retry, if one was
    made), the median prompt size (processed plus reused tokens of the first
    request) and the median generation speed."""
    walls = [
        r["wall_s"] + r.get("retry", {}).get("wall_s", 0)
        for r in rows.values() if "wall_s" in r
    ]
    prompts = [r["prompt_n"] + (r.get("cache_n") or 0) for r in rows.values() if r.get("prompt_n") is not None]
    speeds = [r["predicted_per_second"] for r in rows.values() if r.get("predicted_per_second")]
    return {
        "median_s": median(walls),
        "p95_s": percentile(walls, 0.95),
        "prompt_tokens": median(prompts),
        "tokens_per_second": median(speeds),
    }


def effective(row):
    """The answer the shell would act on: the retry when the first answer was
    invalid and the retry is valid, else the first."""
    retry = row.get("retry")
    if not row["valid"] and retry is not None and retry["valid"]:
        return retry
    return row


def with_retry(row):
    return max(row["level"], row.get("retry", {}).get("level", 0))


def table(run_directories, intents_path):
    """The accuracy table, as Markdown, for runs written by `run_eval.py`."""
    intents = load_intents(intents_path)
    current = hashlib.sha256(Path(intents_path).read_bytes()).hexdigest()
    out = [
        "| Run | n | valid | kind | actions | exact | overreach | off target | exact with retry | request failures |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for directory in run_directories:
        header, rows = read_run(directory, intents)
        summary = summarize(rows, intents)["total"]
        retried = sum(1 for r in rows.values() if with_retry(r) == 3)
        warning = "" if header.get("intents_sha256") == current else " (set edited since the run)"
        out.append(
            f"| {header['label']}{warning} | {summary['n']} | {summary['valid']} | {summary['kind']} "
            f"| {summary['actions']} | {summary['exact']} | {summary['overreach']} | {summary['off_target']} "
            f"| {retried} | {summary['failed']} |"
        )
    return "\n".join(out)


def baselines(intents_path):
    """Exact matches of answers that need no model: asking about everything, and
    listing the home folder for everything. Read the table against them."""
    intents = load_intents(intents_path)
    fixed = {
        "always ask": {"kind": "clarify", "version": 2, "question": "?"},
        "always list home": {"kind": "plan", "version": 2, "summary": "x", "steps": [
            {"id": "s1", "action": "fs.list", "args": {"target": {"place": "home"}}, "depends_on": [], "elevate": False}]},
    }
    out = ["| Answer without a model | exact | overreach |", "|---|---:|---:|"]
    for name, document in fixed.items():
        scored = [score_document(document, intent) for intent in intents]
        out.append(f"| {name} | {sum(s['level'] == 3 for s in scored)} of {len(intents)} | {sum(bool(s['overreach']) for s in scored)} |")
    return "\n".join(out)


def categories_table(run_directories, intents_path):
    """Exact matches per category, one column per run."""
    intents = load_intents(intents_path)
    runs = [read_run(d, intents) for d in run_directories]
    summaries = [summarize(rows, intents) for _, rows in runs]
    names = sorted({i["category"] for i in intents})
    out = [
        "| Category | n | " + " | ".join(h["label"] for h, _ in runs) + " |",
        "|---|---:|" + "---:|" * len(runs),
    ]
    for name in names:
        n = next((s["categories"][name]["n"] for s in summaries if name in s["categories"]), 0)
        cells = " | ".join(str(s["categories"].get(name, {}).get("exact", 0)) for s in summaries)
        out.append(f"| {name} | {n} | {cells} |")
    return "\n".join(out)


def _fmt(value, pattern):
    return "n/a" if value is None else pattern.format(value)


def timing_table(run_directories, intents_path):
    intents = load_intents(intents_path)
    out = [
        "| Run | median request | 95th percentile | prompt tokens | generation tok/s |",
        "|---|---:|---:|---:|---:|",
    ]
    for directory in run_directories:
        header, rows = read_run(directory, intents)
        t = timing(rows)
        out.append(
            f"| {header['label']} | {_fmt(t['median_s'], '{:.2f} s')} | {_fmt(t['p95_s'], '{:.2f} s')} "
            f"| {_fmt(t['prompt_tokens'], '{:.0f}')} | {_fmt(t['tokens_per_second'], '{:.0f}')} |"
        )
    return "\n".join(out)


def comparison(directory_a, directory_b, intents_path):
    intents = load_intents(intents_path)
    header_a, rows_a = read_run(directory_a, intents)
    header_b, rows_b = read_run(directory_b, intents)
    only_a, only_b = paired(rows_a, rows_b)
    return (
        f"exact only in {header_a['label']}: {len(only_a)} {only_a}\n"
        f"exact only in {header_b['label']}: {len(only_b)} {only_b}"
    )


def failures(directory, intents_path):
    """One line per intent that was not exact: the level, the overreach and what was asked."""
    header, rows = read_run(directory, load_intents(intents_path))
    out = []
    for key, row in rows.items():
        if row["level"] == 3:
            continue
        why = "; ".join(row["errors"]) if not row["valid"] else f"level {row['level']}"
        extra = (f" overreach {row['overreach']}" if row["overreach"] else "") + (f" off-target {row['off_target']}" if row.get("off_target") else "")
        out.append(f"{key}: {why}{extra}")
    return "\n".join(out)


COMMANDS = {
    "baselines": lambda dirs, path: baselines(path),
    "table": lambda dirs, path: table(dirs, path),
    "categories": lambda dirs, path: categories_table(dirs, path),
    "timing": lambda dirs, path: timing_table(dirs, path),
    "compare": lambda dirs, path: comparison(*dirs, path),
    "failures": lambda dirs, path: failures(dirs[0], path),
}


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS or (len(sys.argv) < 3 and sys.argv[1] != "baselines"):
        sys.exit(f"usage: score.py {{{'|'.join(COMMANDS)}}} RUN_DIRECTORY...")
    print(COMMANDS[sys.argv[1]](sys.argv[2:], Path(__file__).parent / "intents.json"))
