#!/usr/bin/env python3
"""Prints the tables of the B2 report from `results.json` (the output of `run_b2.py`).

    python3 -I boot-model/render_report.py target/boot-model/results.json
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import evaluate  # noqa: E402
import labels  # noqa: E402

SETS = ("validation", "test", "validation-renamed", "test-renamed", "validation-decoy", "test-decoy")


def pct(v):
    return evaluate.format_percent(v, 2)


def headline(results):
    rows = ["| set | candidates | family | protocol | role | all three | disks exact | disks safe | false kernels | ours missed |",
            "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for name in SETS:
        s = results["sets"][name]
        o = s["outputs"]
        disks, exact, safe = s["disks"]
        sf = s["safety"]
        rows.append(f"| {name} | {s['candidates']} | {pct(o['family']['accuracy'])} | {pct(o['protocol']['accuracy'])} | "
                    f"{pct(o['role']['accuracy'])} | {pct(s['exact_decision'])} | {exact}/{disks} | {safe}/{disks} | "
                    f"{sf['false_kernel']}/{sf['not_kernels']} | {sf['missed_ours']}/{sf['ours_kernels']} |")
    return "\n".join(rows)


def intervals(results, name="test"):
    rows = ["| output | accuracy | 95 % interval |", "|---|---:|---|"]
    for out, v in results["sets"][name]["outputs"].items():
        lo, hi = v["interval"]
        rows.append(f"| {out} | {pct(v['accuracy'])} | {pct(lo)} to {pct(hi)} |")
    rows.append(f"| group, files of entries only | {pct(results['sets'][name]['group_accuracy_of_entry_files'])} | |")
    return "\n".join(rows)


def main(path):
    results = json.loads(Path(path).read_text())
    print("### Headline\n\n" + headline(results))
    print("\n### Test intervals\n\n" + intervals(results))
    test = results["sets"]["test"]
    for out in ("family", "protocol", "role"):
        print(f"\n### Test, {out}\n\n" + evaluate.class_table(test["outputs"][out]["per_class"], labels.names_of(out)))
        print("\n" + evaluate.confusion_table(test["outputs"][out]["confusion"], labels.names_of(out)))
    print("\n### Test, by system (candidates, all three labels right)\n")
    for system, (n, ok) in sorted(test["by_system"].items()):
        print(f"- {system}: {ok}/{n}")
    print("\n### Role confusion, test-decoy\n\n" + evaluate.confusion_table(
        results["sets"]["test-decoy"]["outputs"]["role"]["confusion"], labels.ROLES))
    print("\n### Role confusion, test-renamed\n\n" + evaluate.confusion_table(
        results["sets"]["test-renamed"]["outputs"]["role"]["confusion"], labels.ROLES))
    nd = results["new_distros"]
    print(f"\n### Distributions never seen in training ({', '.join(nd['held_out_systems'])})\n")
    print(f"all three labels: {pct(nd['exact_decision'])}, safety {nd['safety']}, disks {nd['disks']}\n")
    print(evaluate.confusion_table(nd["outputs"]["role"]["confusion"], labels.ROLES))
    print("\n" + evaluate.class_table(nd["outputs"]["role"]["per_class"], labels.ROLES))
    print("\n### Ablations (test, all three labels)\n")
    for k, v in results["ablations"].items():
        print(f"- {k}: {pct(v['exact_decision'])} (per output {', '.join(f'{n} {pct(a)}' for n, a in v['per_output'].items())})")
    print("\n### Tree ensemble (test)\n")
    for k, v in results["trees"].items():
        print(f"- {k}: {v}")
    print("\n### Pairs\n\n" + json.dumps(results["pairs"], indent=1))
    print("\n### Misc\n\n" + json.dumps({k: results[k] for k in ("distinct", "novel_share_of_test", "group_analysis", "model",
                                                                    "corpus_fingerprint", "train_seconds")}, indent=1))
    print("\n### Novel windows only (test)\n")
    nv = results["sets"]["test"]["novel"]
    print(f"{nv['candidates']} candidates; exact decision {pct(nv['exact_decision'])}; safety {nv['safety']}")
    print("\n### Errors of the test set (first 60 in order)\n")
    seen = {}
    for e in results["sets"]["test"]["worst_errors"]:
        key = (e["system"], e["name"][:22], tuple(e["truth"]), tuple(e["predicted"]))
        seen[key] = seen.get(key, 0) + 1
    for (system, name, truth, pred), n in sorted(seen.items(), key=lambda kv: -kv[1])[:20]:
        print(f"- {n}x {system} `{name}`: truth {[labels.names_of(o)[i] for o, i in zip(('family', 'protocol', 'role'), truth)]}"
              f" predicted {[labels.names_of(o)[i] for o, i in zip(('family', 'protocol', 'role'), pred)]}")


if __name__ == "__main__":
    main(sys.argv[1])
