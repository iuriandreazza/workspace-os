#!/usr/bin/env python3
"""The full B2 run: corpus, training, export, evaluation (offline, on the host).

    uv run --no-project --with numpy==2.5.5 --with scikit-learn==1.9.1 \\
        python boot-model/run_b2.py --out target/boot-model

Needs the corpus cache (`fetch.py fetch`, `build_ours.py`) and, for the QEMU device
trees, `qemu-system-aarch64`. Not part of the gate. Seeds are arguments; with the
defaults and single-threaded BLAS (`OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1
VECLIB_MAXIMUM_THREADS=1`) the data and the corpus are the same every time; the trained weights
are not byte-identical between runs (the report gives the spread).
"""
import argparse
import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import catalogue  # noqa: E402
import dataset  # noqa: E402
import evaluate  # noqa: E402
import labels  # noqa: E402
import layouts  # noqa: E402
import mdl  # noqa: E402
import synthetic  # noqa: E402
import training  # noqa: E402

NEW_DISTROS = {"fedora", "alma", "rocky", "opensuse"}


def log(message):
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


def build_disks(assets, args):
    junk = {name: synthetic.junk_assets(400, args.seed + offset)
            for name, offset in (("train", 1), ("validation", 2), ("test", 3))}
    return {
        "train": layouts.generate(assets, junk["train"], "train", args.train_disks, args.seed + 10),
        "validation": layouts.generate(assets, junk["validation"], "validation", args.test_disks, args.seed + 11),
        "validation-renamed": layouts.generate(assets, junk["validation"], "validation", 600, args.seed + 12, "renamed"),
        "validation-decoy": layouts.generate(assets, junk["validation"], "validation", 600, args.seed + 13, "decoy"),
        "test": layouts.generate(assets, junk["test"], "test", args.test_disks, args.seed + 14),
        "test-renamed": layouts.generate(assets, junk["test"], "test", 600, args.seed + 15, "renamed"),
        "test-decoy": layouts.generate(assets, junk["test"], "test", 600, args.seed + 16, "decoy"),
    }


def predict_int(shape, tensors, x):
    return training.decode_all(training.int_forward(shape, tensors, x))


def tuples(array):
    return [tuple(int(v) for v in row) for row in array]


def report_for(flat, y, predicted, disks):
    """Summaries for one evaluation set, from the arrays."""
    summary = evaluate.summarise(tuples(y), tuples(predicted))
    start, truth_disks, predicted_disks = 0, [], []
    for disk in disks:
        truth_disks.append(tuples(y[start:start + len(disk)]))
        predicted_disks.append(tuples(predicted[start:start + len(disk)]))
        start += len(disk)
    summary["disks"] = evaluate.per_disk(truth_disks, predicted_disks)
    return summary


def pick_subset(flat, y, predicted, mask):
    idx = np.flatnonzero(mask)
    return [flat[i] for i in idx], y[idx], predicted[idx]


def distinct(x):
    return len(set(dataset.window_hashes(x)))


def group_analysis(flat, x, y):
    """Is the group a function of the window? The share of entry windows seen with more than one group."""
    seen = {}
    for h, c, row in zip(dataset.window_hashes(x), flat, y):
        if row[3] != layouts.NO_ENTRY:
            seen.setdefault(h, set()).add(int(row[3]))
    conflicts = sum(len(v) > 1 for v in seen.values())
    return {"entry_windows": len(seen), "windows_with_several_groups": conflicts}


def tree_comparison(x_train, y_train, x_test, y_test, seed):
    from sklearn.ensemble import HistGradientBoostingClassifier
    out = {}
    for head, (name, _) in enumerate(labels.SLICES):
        model = HistGradientBoostingClassifier(max_iter=60, max_leaf_nodes=15, early_stopping=False,
                                               random_state=seed)
        model.fit(x_train, y_train[:, head])
        nodes = sum(len(p.nodes) for stage in model._predictors for p in stage)
        out[name] = {"accuracy": float((model.predict(x_test) == y_test[:, head]).mean()),
                     "nodes": int(nodes), "comparisons_per_candidate": int(sum(
                         p.get_max_depth() for stage in model._predictors for p in stage))}
    out["bytes_at_16_per_node"] = 16 * sum(v["nodes"] for v in out.values() if isinstance(v, dict))
    return out


def pair_experiment(params, sets, arrays_):
    """Can a small head decide which files belong to one entry, from the trunk's own features?"""
    from sklearn.linear_model import LogisticRegression
    features = {}
    pairs = {}
    for name in ("train", "test"):
        flat, x, y = arrays_[name]
        h2 = training.forward(params, x)[2] if len(x) < 4096 else np.concatenate(
            [training.forward(params, x[i:i + 2048])[2] for i in range(0, len(x), 2048)])
        rows, labels_, disk_of, same_dir = [], [], [], []
        by_disk = {}
        for i, c in enumerate(flat):
            by_disk.setdefault(c.disk, []).append(i)
        for disk, members in by_disk.items():
            for a in members:
                if y[a, 2] not in (labels.ROLES.index("kernel"), labels.ROLES.index("loader")):
                    continue
                for b in members:
                    if y[b, 2] not in (labels.ROLES.index(r) for r in ("initrd", "dtb", "entry")):
                        continue
                    rows.append(np.concatenate([h2[a] * h2[b], np.abs(h2[a] - h2[b])]))
                    labels_.append(int(flat[a].entry is not None and flat[a].entry == flat[b].entry))
                    disk_of.append(disk)
                    same_dir.append(int(flat[a].path.rsplit("\\", 1)[0] == flat[b].path.rsplit("\\", 1)[0]))
        pairs[name] = (np.array(rows), np.array(labels_), disk_of, np.array(same_dir))
    model = LogisticRegression(max_iter=2000, C=1.0).fit(pairs["train"][0], pairs["train"][1])
    x_t, y_t, d_t, dir_t = pairs["test"]
    predicted = model.predict(x_t)
    tp = int(((predicted == 1) & (y_t == 1)).sum())
    by_disk = {}
    for d, truth, guess in zip(d_t, y_t, predicted):
        by_disk[d] = by_disk.get(d, True) and truth == guess
    rule_tp = int(((dir_t == 1) & (y_t == 1)).sum())
    rule_disks = {}
    for d, truth, guess in zip(d_t, y_t, dir_t):
        rule_disks[d] = rule_disks.get(d, True) and truth == guess
    rule = {"accuracy": float((dir_t == y_t).mean()), "precision": rule_tp / max(1, int(dir_t.sum())),
            "recall": rule_tp / max(1, int(y_t.sum())), "disks_all_pairs_right": int(sum(rule_disks.values()))}
    return {"same_directory_rule": rule, "pairs": int(len(y_t)), "positives": int(y_t.sum()), "accuracy": float((predicted == y_t).mean()),
            "precision": tp / max(1, int((predicted == 1).sum())), "recall": tp / max(1, int((y_t == 1).sum())),
            "disks_with_pairs": len(by_disk), "disks_all_pairs_right": int(sum(by_disk.values())),
            "baseline_all_negative_accuracy": float(1 - y_t.mean())}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--seed", type=int, default=20261123)
    parser.add_argument("--train-disks", type=int, default=4000)
    parser.add_argument("--test-disks", type=int, default=1500)
    parser.add_argument("--epochs", type=int, default=12)
    parser.add_argument("--group-weight", type=float, default=0.05,
                        help="weight of the group head's loss; 1.0 gives the head a fair chance (ADR 0034)")
    parser.add_argument("--out", default="target/boot-model")
    parser.add_argument("--skip", default="", help="comma list of: ablations, newdistro, trees, pairs")
    args = parser.parse_args()
    skip = set(filter(None, args.skip.split(",")))
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    assets = catalogue.pinned_assets() + catalogue.qemu_dtb_assets()
    log(f"{len(assets)} assets, corpus fingerprint {catalogue.fingerprint(assets)}")
    sets = build_disks(assets, args)
    arrays_ = {name: dataset.arrays(disks) for name, disks in sets.items()}
    for name, (flat, x, y) in arrays_.items():
        log(f"{name}: {len(sets[name])} disks, {len(flat)} candidates, {distinct(x)} distinct windows")
    results = {"args": vars(args), "corpus_fingerprint": catalogue.fingerprint(assets), "sets": {}}
    _, x_train, y_train = arrays_["train"]
    results["distinct"] = {n: distinct(a[1]) for n, a in arrays_.items()}
    results["novel_share_of_test"] = float(dataset.novel_mask(x_train, arrays_["test"][1]).mean())
    results["group_analysis"] = group_analysis(*arrays_["train"])

    log("training the model")
    start = time.time()
    params = training.train(x_train, y_train, args.seed, epochs=args.epochs, log=log,
                            head_weights=(1.0, 1.0, 1.0, args.group_weight))
    results["train_seconds"] = time.time() - start
    shape, tensors = training.quantise(params, x_train[:4000])
    blob = mdl.write(shape, tensors)
    (out / "WSOS.MDL").write_bytes(blob)
    shape2, tensors2 = mdl.read(blob)
    assert shape2 == shape and tensors2 == tensors
    results["model"] = {"sha256": hashlib.sha256(blob).hexdigest(), "bytes": len(blob),
                        "weights": shape.weight_bytes(), "macs": shape.macs(), "shifts": [shape.shift1, shape.shift2],
                        "budget_violation": mdl.within_budget(shape)}
    log(f"model {results['model']}")

    # The pure Python reference and the numpy integer path must agree bit for bit.
    sample = arrays_["test"][1][:3]
    numpy_scores = training.int_forward(shape, tensors, sample)
    for row, expected in zip(sample, numpy_scores):
        if mdl.forward(shape, tensors, bytes(row)) != [int(v) for v in expected]:
            raise RuntimeError("the pure Python forward pass and the numpy integer path disagree")
    results["reference_check"] = "pure Python forward equals the numpy integer path on 3 windows"

    for name, (flat, x, y) in arrays_.items():
        if name == "train":
            continue
        predicted = predict_int(shape, tensors, x)
        float_predicted = training.decode_all(training.float_predict(params, x))
        entry = report_for(flat, y, predicted, sets[name])
        matrix = entry["outputs"]["group"]["confusion"]
        rows = sum(sum(r) for r in matrix[:layouts.NO_ENTRY])
        entry["group_accuracy_of_entry_files"] = sum(matrix[i][i] for i in range(layouts.NO_ENTRY)) / max(1, rows)
        entry["float_exact_decision"] = float((float_predicted[:, :3] == y[:, :3]).all(axis=1).mean())
        if name in ("validation", "test"):
            novel = dataset.novel_mask(x_train, x)
            _, y_n, p_n = pick_subset(flat, y, predicted, novel)
            entry["novel"] = evaluate.summarise(tuples(y_n), tuples(p_n))
            errors = np.flatnonzero((predicted[:, :3] != y[:, :3]).any(axis=1))
            entry["worst_errors"] = [
                {"name": flat[i].name, "system": flat[i].system, "release": flat[i].release,
                 "truth": [int(v) for v in y[i, :3]], "predicted": [int(v) for v in predicted[i, :3]]}
                for i in errors[:60]]
            by_system = {}
            for i, c in enumerate(flat):
                bucket = by_system.setdefault(c.system, [0, 0])
                bucket[0] += 1
                bucket[1] += int((predicted[i, :3] == y[i, :3]).all())
            entry["by_system"] = by_system
        results["sets"][name] = entry
        log(f"{name}: exact decision {entry['exact_decision']:.4f} (float {entry['float_exact_decision']:.4f}), "
            f"disks {entry['disks']}, safety {entry['safety']}")
    (out / "results.json").write_text(json.dumps(results, indent=1, default=str))

    if "ablations" not in skip:
        results["ablations"] = {}
        for label, mask in (("name only", dataset.name_only), ("content only", dataset.content_only)):
            log(f"ablation: {label}")
            p = training.train(mask(x_train), y_train, args.seed, epochs=args.epochs, log=lambda m: None)
            s, t = training.quantise(p, mask(x_train)[:4000])
            x_test = arrays_["test"][1]
            predicted = predict_int(s, t, mask(x_test))
            y_test = arrays_["test"][2]
            results["ablations"][label] = {
                "exact_decision": float((predicted[:, :3] == y_test[:, :3]).all(axis=1).mean()),
                "per_output": {n: float((predicted[:, i] == y_test[:, i]).mean()) for i, (n, _) in enumerate(labels.SLICES)}}
            log(f"{label}: {results['ablations'][label]}")
        (out / "results.json").write_text(json.dumps(results, indent=1, default=str))

    if "newdistro" not in skip:
        log("fold: distributions never seen in training")
        junk_train = synthetic.junk_assets(400, args.seed + 1)
        junk_test = synthetic.junk_assets(400, args.seed + 3)
        train_disks = layouts.generate([a for a in assets if a.system not in NEW_DISTROS], junk_train, "train",
                                       args.train_disks, args.seed + 20)
        test_disks = layouts.generate([a for a in assets if a.system in NEW_DISTROS or a.system in ("alpine-rootfs", "qemu")],
                                      junk_test, "test", 800, args.seed + 21, systems=NEW_DISTROS)
        _, xf, yf = dataset.arrays(train_disks)
        flat_t, xt, yt = dataset.arrays(test_disks)
        p = training.train(xf, yf, args.seed, epochs=args.epochs, log=lambda m: None)
        s, t = training.quantise(p, xf[:4000])
        predicted = predict_int(s, t, xt)
        results["new_distros"] = report_for(flat_t, yt, predicted, test_disks)
        results["new_distros"]["held_out_systems"] = sorted(NEW_DISTROS)
        log(f"new distros: {results['new_distros']['exact_decision']:.4f} {results['new_distros']['safety']}")
        (out / "results.json").write_text(json.dumps(results, indent=1, default=str))

    if "trees" not in skip:
        log("tree ensemble")
        results["trees"] = tree_comparison(x_train, y_train, arrays_["test"][1], arrays_["test"][2], args.seed)
        log(f"trees: {results['trees']}")
        (out / "results.json").write_text(json.dumps(results, indent=1, default=str))

    if "pairs" not in skip:
        log("pair head")
        results["pairs"] = pair_experiment(params, sets, arrays_)
        log(f"pairs: {results['pairs']}")
        (out / "results.json").write_text(json.dumps(results, indent=1, default=str))
    log("done")


if __name__ == "__main__":
    main()
