#!/usr/bin/env python3
"""Tables of the B2b report, from the runs in the cache (`b2b.py`). Prints Markdown.

    uv run --no-project --with numpy==2.5.5 --with scikit-learn==1.9.1 python boot-model/b2b_report.py step1
    ... folds [--configs base A] ; calibration ; ablation

A change counts as a gain only if the seeds' ranges do not overlap in the favourable direction and the means differ by
one point (`verdict`): the first part was fixed before any run was read, the one-point floor after the first tables.
"""
import json
import statistics
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import assembly  # noqa: E402
import b2b  # noqa: E402
import calibration  # noqa: E402
import labels  # noqa: E402
import layouts  # noqa: E402
import run_b2  # noqa: E402
import training  # noqa: E402

SEEDS = (0, 1, 2)
ROLE_NAMES = labels.ROLES


def pct(value, digits=1):
    return "n/a" if value is None else f"{100 * value:.{digits}f}"


def load(fold, config, seed):
    path = b2b.RUNS / f"{fold}__{config}__s{seed}.json"
    return json.loads(path.read_text()) if path.is_file() else None


def values(fold, config, getter, seeds=SEEDS):
    out = []
    for seed in seeds:
        run = load(fold, config, seed)
        if run is not None:
            v = getter(run)
            if v is not None:
                out.append(v)
    return out


def spread(vals):
    """`mean ± sd (min–max)` in percent."""
    if not vals:
        return "—"
    sd = statistics.stdev(vals) if len(vals) > 1 else 0.0
    return f"{pct(statistics.mean(vals))} ± {pct(sd)} ({pct(min(vals))}–{pct(max(vals))})"


def verdict(base, other, higher_is_better):
    """`gain`, `loss` or `within spread`: whether the seeds' ranges are apart."""
    if not base or not other:
        return "n/a"
    if abs(statistics.mean(other) - statistics.mean(base)) < 0.01:
        return "within spread"  # under one point is not material, whatever the ranges (added after seeing 0.0 to 0.2 flagged)
    if higher_is_better:
        return "gain" if min(other) > max(base) else "loss" if max(other) < min(base) else "within spread"
    return "gain" if max(other) < min(base) else "loss" if min(other) > max(base) else "within spread"


def getter(set_name, level, metric, subset=None):
    def get(run):
        entry = run["sets"][set_name]
        entry = entry[subset] if subset else entry
        return entry[level][metric]
    return get


# --- step 1: the group rules on the fixed generator ---------------------------------------------------------------

def rule_rows(disks, predicted_roles=None):
    rows = {}
    for name, rule in assembly.RULES.items():
        rows[name] = assembly.score(disks, rule, predicted_roles)
    return rows


def predicted_role_names(fold, config, seed, set_name, disks):
    path = b2b.RUNS / f"{fold}__{config}__s{seed}.npz"
    scores = np.load(path)[f"{set_name}__scores"]
    decoded = training.decode_all(scores)
    names, start = [], 0
    for disk in disks:
        names.append([ROLE_NAMES[int(r)] for r in decoded[start:start + len(disk)][:, 2]])
        start += len(disk)
    return names


def split_disks(disks):
    with_text = [d for d in disks if any(c.role == "entry" for c in d)]
    shared = []
    for d in disks:
        kernels = [(c.entry[:2], assembly.directory(c.path)) for c in d if c.role == "kernel" and c.entry]
        if len(kernels) != len(set(kernels)):
            shared.append(d)
    return {"all disks": disks, "disks with entry text": with_text, "disks with two kernels in one directory": shared}


def step1():
    assets = b2b.load_assets()
    disks = b2b.eval_disks("indist", assets)["test"]
    groups = split_disks(disks)
    print("### Group rules, true roles (the ceiling of each rule; no model)\n")
    print("| Disks | Rule | Pair precision | Pair recall | Disks with pairs | Disks with every pair right | Spurious on pairless | Pairs missed, by companion | Pairs wrong, by companion |")
    print("|---|---|---:|---:|---:|---:|---:|---|---|")
    for gname, subset in groups.items():
        for rname, r in rule_rows(subset).items():
            print(f"| {gname} ({len(subset)}) | {rname} | {pct(r['precision'])} % | {pct(r['recall'])} % | {r['disks_with_pairs']} | "
                  f"{r['disks_all_right']} ({pct(r['disks_all_right'] / max(1, r['disks_with_pairs']))} %) | {r['spurious_on_pairless']} | "
                  f"{r['missed_by_role']} | {r['wrong_by_role']} |")
    print("\n### Group rules, roles predicted by the model (3 seeds)\n")
    print("| Roles from | Disks | Rule | Pair precision | Pair recall | Disks with every pair right (of disks with pairs) |")
    print("|---|---|---|---|---|---|")
    index = {id(d): i for i, d in enumerate(disks)}
    for config in ("base", "A"):
        predicted = {s: predicted_role_names("indist", config, s, "test", disks) for s in SEEDS if load("indist", config, s)}
        for gname, subset in groups.items():
            for rname in assembly.RULES:
                per_seed = []
                for s, names in predicted.items():
                    sub_roles = [names[index[id(d)]] for d in subset]
                    per_seed.append(assembly.score(subset, assembly.RULES[rname], sub_roles))
                if not per_seed:
                    continue
                ok = [r["disks_all_right"] / max(1, r["disks_with_pairs"]) for r in per_seed]
                print(f"| `{config}` | {gname} | {rname} | {spread([r['precision'] for r in per_seed])} | {spread([r['recall'] for r in per_seed])} | {spread(ok)} |")
    print("\n### Pair head (logistic regression on the trunk's 128 features), true roles, per seed\n")
    print("| Seed | Pair precision | Pair recall | Disks with pairs | Disks with every pair right |")
    print("|---|---:|---:|---:|---:|")
    for s in SEEDS:
        params_path = b2b.RUNS / f"indist__base__s{s}__params.npz"
        if not params_path.is_file():
            continue
        params = {k: v for k, v in np.load(params_path).items()}
        train = b2b.train_disks("indist", {}, s, assets, 4000)
        flat_t, x_t, y_t = b2b.disks_to_arrays(train)
        flat, x, y = b2b.disks_to_arrays(disks)
        out = run_b2.pair_experiment(params, None, {"train": (flat_t, x_t, y_t), "test": (flat, x, y)})
        print(f"| {s} | {pct(out['precision'])} % | {pct(out['recall'])} % | {out['disks_with_pairs']} | {out['disks_all_pairs_right']} |")


# --- steps 2 to 4: configurations against the base, fold by fold --------------------------------------------------

METRICS = (("kernel_recall", True), ("initrd_recall", True), ("false_kernel", False), ("all_three", True))


def folds_table(configs, folds, set_name="heldout", level="files", subset=None):
    print(f"| Fold | Metric ({level}) | " + " | ".join(configs) + " |")
    print("|---|---|" + "---|" * len(configs))
    for fold in folds:
        for metric, high in METRICS:
            cells = []
            base = values(fold, configs[0], getter(set_name, level, metric, subset))
            for config in configs:
                vals = values(fold, config, getter(set_name, level, metric, subset))
                mark = "" if config == configs[0] else f" **{verdict(base, vals, high)}**" if verdict(base, vals, high) != "within spread" else ""
                cells.append(spread(vals) + mark)
            print(f"| {fold} | {metric} | " + " | ".join(cells) + " |")


# --- step 5: confidence ----------------------------------------------------------------------------------------

_EVAL = {}


def eval_rows(fold):
    """Truth, file identity and per-candidate weight (1 / appearances of its file) of a fold's evaluation disks."""
    if fold not in _EVAL:
        assets = b2b.load_assets()
        set_name = "test" if fold == "indist" else "heldout"
        flat, _, y = b2b.disks_to_arrays(b2b.eval_disks(fold, assets)[set_name])
        ids = [c.file_id for c in flat]
        counts = {}
        for f in ids:
            counts[f] = counts.get(f, 0) + 1
        _EVAL[fold] = (y, np.array([1.0 / counts[f] for f in ids]), flat)
    return _EVAL[fold]


def role_scores(fold, config, seed):
    set_name = "test" if fold == "indist" else "heldout"
    scores = np.load(b2b.RUNS / f"{fold}__{config}__s{seed}.npz")[f"{set_name}__scores"]
    start, end = labels.slice_bounds()["role"]
    return scores[:, start:end].astype(np.int64)


def margins_of(scores):
    top = np.sort(scores, axis=1)
    return top[:, -1] - top[:, -2]


def threshold_table(config, calib_fold, report_folds, quantiles=(0.0, 0.1, 0.25, 0.5)):
    """False kernels removed and real kernels lost when predicted kernels with a margin below a threshold are
    called unverified. The threshold is the `q` quantile of the margins of the kernels the model predicted on the
    calibration fold (so it is fitted there, never on the fold it is reported on)."""
    kernel = ROLE_NAMES.index("kernel")
    print(f"Threshold fitted on `{calib_fold}`, reported on the other folds (config `{config}`; weights: one per distinct file).\n")
    print("| Fold | Quantile of the calibration fold's kernel margins | Threshold (seeds) | False kernels removed | Real kernels lost | False kernels left (files) |")
    print("|---|---|---|---|---|---|")
    for fold in report_folds:
        y, w, _ = eval_rows(fold)
        for q in quantiles:
            removed, lost, left, thresholds = [], [], [], []
            for seed in SEEDS:
                if not (b2b.RUNS / f"{fold}__{config}__s{seed}.npz").is_file() or not (b2b.RUNS / f"{calib_fold}__{config}__s{seed}.npz").is_file():
                    continue
                cy, _, _ = eval_rows(calib_fold)
                cs = role_scores(calib_fold, config, seed)
                cm = margins_of(cs)[cs.argmax(axis=1) == kernel]
                t = float(np.quantile(cm, q))
                sc = role_scores(fold, config, seed)
                m, pred = margins_of(sc), sc.argmax(axis=1)
                false = (pred == kernel) & (y[:, 2] != kernel)
                real = (pred == kernel) & (y[:, 2] == kernel)
                false_w, real_w = w[false].sum(), w[real].sum()
                kept_false, kept_real = w[false & (m >= t)].sum(), w[real & (m >= t)].sum()
                thresholds.append(t)
                removed.append(None if not false_w else 1 - kept_false / false_w)
                lost.append(None if not real_w else 1 - kept_real / real_w)
                left.append(kept_false)
            if thresholds:
                print(f"| {fold} | {q:.2f} | {', '.join(f'{t:.0f}' for t in thresholds)} | {spread([v for v in removed if v is not None])} % | "
                      f"{spread([v for v in lost if v is not None])} % | {statistics.mean(left):.1f} |")


def ece_table(configs, calib_fold, report_folds):
    kernel_free = None  # the role head only
    print(f"Role head, candidate level; temperature fitted on `{calib_fold}` (grid, lowest negative log-likelihood).\n")
    print("| Config | Fold | ECE raw scores (T = 1) | T fitted | ECE after temperature | Accuracy of the role |")
    print("|---|---|---|---|---|---|")
    for config in configs:
        for fold in report_folds:
            y, _, _ = eval_rows(fold)
            raw, scaled, temps, acc = [], [], [], []
            for seed in SEEDS:
                if not (b2b.RUNS / f"{fold}__{config}__s{seed}.npz").is_file() or not (b2b.RUNS / f"{calib_fold}__{config}__s{seed}.npz").is_file():
                    continue
                cy, _, _ = eval_rows(calib_fold)
                cs = role_scores(calib_fold, config, seed)
                pick = np.random.default_rng(0).choice(len(cs), min(4000, len(cs)), replace=False)
                t = calibration.fit_temperature(cs[pick].tolist(), cy[pick, 2].tolist())
                sc = role_scores(fold, config, seed)
                ok = (sc.argmax(axis=1) == y[:, 2]).tolist()
                conf = lambda temp: [calibration.confidence(r, temp) for r in sc.tolist()]
                raw.append(calibration.expected_calibration_error(conf(1.0), ok))
                scaled.append(calibration.expected_calibration_error(conf(t), ok))
                temps.append(t)
                acc.append(sum(ok) / len(ok))
            if raw:
                print(f"| {config} | {fold} | {spread(raw)} % | {', '.join(f'{t:.1f}' for t in temps)} | {spread(scaled)} % | {spread(acc)} % |")


def pooled(configs, folds, set_name="heldout"):
    """Mean over folds, per seed, then the spread over seeds: one number per configuration and metric."""
    print("| Metric (files; mean over the folds that have it, then seeds) | " + " | ".join(configs) + " |")
    print("|---|" + "---|" * len(configs))
    for metric, high in METRICS:
        base = None
        cells = []
        for config in configs:
            per_seed = []
            # folds where the metric exists (no initrd in the OpenBSD fold): the same folds for every seed
            used = [f for f in folds if all(load(f, config, s) and load(f, config, s)["sets"][set_name]["files"][metric] is not None
                                             for s in SEEDS)]
            for seed in SEEDS:
                vals = [load(f, config, seed)["sets"][set_name]["files"][metric] for f in used]
                if vals and len(used) == len([f for f in folds if load(f, configs[0], seed) and
                                              load(f, configs[0], seed)["sets"][set_name]["files"][metric] is not None]):
                    per_seed.append(statistics.mean(vals))
            if base is None:
                base = per_seed
            v = verdict(base, per_seed, high)
            cells.append(spread(per_seed) + ("" if config == configs[0] or v == "within spread" else f" **{v}**"))
        print(f"| {metric} | " + " | ".join(cells) + " |")


def ensemble(config, folds):
    """Three seeds as one ensemble (sum of the integer scores: 3 x 4.04 MiB, inside ADR 0027's weights budget)."""
    import measure
    print("| Fold | Single model false kernel (files) | Ensemble false kernel | Single kernel recall | Ensemble kernel recall | "
          "Seeds disagree on the role: share of false kernels | ... of real kernels |")
    print("|---|---|---|---|---|---|---|")
    kernel = ROLE_NAMES.index("kernel")
    for fold in folds:
        y, w, flat = eval_rows(fold)
        sets = [role_scores(fold, config, s) for s in SEEDS if (b2b.RUNS / f"{fold}__{config}__s{s}.npz").is_file()]
        if len(sets) < 3:
            continue
        set_name = "test" if fold == "indist" else "heldout"
        total = sum(np.load(b2b.RUNS / f"{fold}__{config}__s{s}.npz")[f"{set_name}__scores"].astype(np.int64) for s in SEEDS)
        pred = training.decode_all(total)
        out = measure.summarise(b2b.tuples(y), b2b.tuples(pred), [c.file_id for c in flat], [(c.system, c.release) for c in flat])["files"]
        singles = [load(fold, config, s)["sets"][set_name]["files"] for s in SEEDS]
        votes = np.stack([sc.argmax(axis=1) for sc in sets])
        disagree = (votes != votes[0]).any(axis=0)
        ensemble_pred_kernel = pred[:, 2] == kernel
        false = ensemble_pred_kernel & (y[:, 2] != kernel)
        real = ensemble_pred_kernel & (y[:, 2] == kernel)
        share = lambda mask: None if not w[mask].sum() else w[mask & disagree].sum() / w[mask].sum()
        print(f"| {fold} | {spread([s['false_kernel'] for s in singles])} % | {pct(out['false_kernel'])} % | "
              f"{spread([s['kernel_recall'] for s in singles])} % | {pct(out['kernel_recall'])} % | {pct(share(false))} % | {pct(share(real))} % |")


def releases(config, folds):
    """Per release of the held-out family: distinct kernel and initrd files and their recall, mean over seeds."""
    print("| Fold | Release | Kernel files | Kernel recall | Initrd files | Initrd recall | False kernels (non-kernel files) |")
    print("|---|---|---:|---:|---:|---:|---:|")
    for fold in folds:
        runs = [load(fold, config, s) for s in SEEDS]
        runs = [r for r in runs if r]
        if not runs:
            continue
        set_name = "test" if fold == "indist" else "heldout"
        held = b2b.FOLDS[fold]
        for name in sorted(runs[0]["sets"][set_name]["releases"]):
            if held and name.split("/")[0] not in held:
                continue
            per = [r["sets"][set_name]["releases"][name] for r in runs if name in r["sets"][set_name]["releases"]]
            mean = lambda key: statistics.mean(v[key] for v in per if v[key] is not None) if any(v[key] is not None for v in per) else None
            print(f"| {fold} | {name} | {per[0]['kernel_units']} | {pct(mean('kernel_recall'))} % | {per[0]['initrd_units']} | "
                  f"{pct(mean('initrd_recall'))} % | {pct(mean('false_kernel'))} % ({per[0]['not_kernel_units']}) |")


def heldout_table(configs, folds):
    """False kernels and all-three on the files of the held-out family only.

    The fold-level rates of `measure` include the negatives that are in training anyway (the Alpine rootfs, the QEMU
    trees, generated junk); here only the files of the held-out systems count, the real unseen negatives.
    """
    import measure
    print("| Fold | Metric (files of the held-out family only) | " + " | ".join(configs) + " |")
    print("|---|---|" + "---|" * len(configs))
    for fold in folds:
        held = b2b.FOLDS[fold]
        y, _, flat = eval_rows(fold)
        pick = [i for i, c in enumerate(flat) if c.system in held]
        rows = {}
        for config in configs:
            per = []
            for seed in SEEDS:
                path = b2b.RUNS / f"{fold}__{config}__s{seed}.npz"
                if not path.is_file():
                    continue
                scores = np.load(path)["heldout__scores"].astype(np.int64)
                pred = training.decode_all(scores)
                out = measure.summarise(b2b.tuples(y[pick]), b2b.tuples(pred[pick]), [flat[i].file_id for i in pick],
                                        [(flat[i].system, flat[i].release) for i in pick])["files"]
                per.append(out)
            rows[config] = per
        for metric, high in (("false_kernel", False), ("all_three", True)):
            base = [r[metric] for r in rows[configs[0]] if r[metric] is not None]
            cells = []
            for config in configs:
                vals = [r[metric] for r in rows[config] if r[metric] is not None]
                v = verdict(base, vals, high)
                cells.append(spread(vals) + ("" if config == configs[0] or v == "within spread" else f" **{v}**"))
            print(f"| {fold} | {metric} ({rows[configs[0]][0]['not_kernel_units'] if metric == 'false_kernel' else rows[configs[0]][0]['units']} files) | " + " | ".join(cells) + " |")


def main():
    command = sys.argv[1]
    rest = sys.argv[2:]
    if command == "step1":
        step1()
    elif command == "confidence":
        configs, calib, report = rest[0].split(","), rest[1], rest[2].split(",")
        ece_table(configs, calib, report)
        print()
        for config in configs:
            threshold_table(config, calib, report)
            print()
    elif command == "releases":
        releases(rest[0], rest[1].split(","))
    elif command == "heldout":
        heldout_table(rest[0].split(","), rest[1].split(","))
    elif command == "pooled":
        pooled(rest[0].split(","), rest[1].split(","))
    elif command == "ensemble":
        ensemble(rest[0], rest[1].split(","))
    elif command == "folds":
        configs = rest[0].split(",")
        folds = rest[1].split(",")
        level = rest[2] if len(rest) > 2 else "files"
        set_name = "test" if folds == ["indist"] else "heldout"
        if len(rest) > 4:
            set_name = rest[4]
        subset = rest[3] if len(rest) > 3 and rest[3] != "-" else None
        folds_table(configs, folds, set_name, level, subset)


if __name__ == "__main__":
    main()
