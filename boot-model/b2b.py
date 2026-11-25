#!/usr/bin/env python3
"""B2b: the measurements on the boot model (host only, nothing is shipped or released).

One *run* is one training of the B2 network on a *fold* with a *configuration* and a *seed*; it writes its measures
and its integer scores into the cache (outside the repository: `~/.cache/workspace-os/boot-model/b2b`).

    export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1
    uv run --no-project --with numpy==2.5.5 --with scikit-learn==1.9.1 --with zstandard --with lz4 \\
        python boot-model/b2b.py queue --jobs 6 --folds el debian --configs base A --seeds 0 1 2

Folds leave one family of distributions out of training whole (`el` is Fedora, AlmaLinux and Rocky together: they
share a layout); `b2four` is the fold of the B2 report; `indist` is B2's split by release. The held-out family is
evaluated on all its releases. The evaluation disks do not depend on the seed or the configuration, so runs are
paired; the seed changes the training disks and junk, the initial weights and the order of the batches.
"""
import argparse
import copy
import json
import os
import pickle
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import catalogue  # noqa: E402
import contexts  # noqa: E402
import dataset  # noqa: E402
import evaluate  # noqa: E402
import fetch  # noqa: E402
import formats  # noqa: E402
import labels  # noqa: E402
import layouts  # noqa: E402
import measure  # noqa: E402
import synthetic  # noqa: E402
import training  # noqa: E402
import window  # noqa: E402

BASE_SEED = 20261123
CACHE = Path(os.environ.get("WSOS_CORPUS_CACHE", "~/.cache/workspace-os/boot-model")).expanduser() / "b2b"
RUNS = CACHE / "runs"

FOLDS = {
    "alpine": {"alpine"}, "debian": {"debian"}, "el": {"fedora", "alma", "rocky"}, "opensuse": {"opensuse"},
    "openbsd": {"openbsd"}, "b2four": {"fedora", "alma", "rocky", "opensuse"}, "indist": set(),
}
CONFIGS = {
    "base": {},
    "A": {"augment": True},
    "C": {"mixed": 0.25},
    "AC": {"augment": True, "mixed": 0.25},
    "name": {"mask": "name"},
    "bytes": {"mask": "content"},
    "OE": {"outliers": True},
    "E": {"size": True},
    "F": {"neighbours": True},
}
EVAL_DISKS = 800


def log(message):
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


def write_atomically(path, data):
    """A cache file appears whole or not at all: workers read these while others may be writing them."""
    partial = path.with_name(path.name + f".{os.getpid()}.partial")
    partial.write_bytes(data)
    partial.replace(path)


def load_assets():
    path = CACHE / "assets.pkl"
    if path.is_file():
        return pickle.loads(path.read_bytes())
    assets = catalogue.pinned_assets() + catalogue.qemu_dtb_assets()
    CACHE.mkdir(parents=True, exist_ok=True)
    write_atomically(path, pickle.dumps(assets))
    return assets


def augmented(assets, seed):
    """Re-encoded copies of every training-split release (`formats.augment_assets`), cached per seed."""
    path = CACHE / f"augment-s{seed}.pkl"
    if path.is_file():
        return pickle.loads(path.read_bytes())
    pins = json.loads(fetch.PINS.read_text())
    tarballs = [fetch.pinned_bytes(p) for p in pins
                if p["system"] == "alpine-rootfs" and catalogue.split_of(p["system"], p["release"]) == "train"]
    extra = formats.augment_assets([a for a in assets if a.split == "train"], tarballs, BASE_SEED + seed)
    write_atomically(path, pickle.dumps(extra))
    return extra


def disks_to_arrays(disks, size=False, neighbours=False):
    flat = [c for disk in disks for c in disk]
    if size or neighbours:
        rows = [w for disk in disks for w in contexts.extended_windows(disk, size, neighbours)]
    else:
        rows = [c.window for c in flat]
    x = np.frombuffer(b"".join(rows), dtype=np.uint8).reshape(len(flat), -1)
    y = np.array([c.indices() for c in flat], dtype=np.int64)
    return flat, x, y


def eval_disks(fold, assets):
    """The evaluation sets of a fold, the same for every seed and configuration."""
    junk = synthetic.junk_assets(400, BASE_SEED + 3)
    if fold == "indist":
        return {
            "test": layouts.generate(assets, junk, "test", 1500, BASE_SEED + 14),
            "test-renamed": layouts.generate(assets, junk, "test", 600, BASE_SEED + 15, "renamed"),
            "test-decoy": layouts.generate(assets, junk, "test", 600, BASE_SEED + 16, "decoy"),
        }
    held = FOLDS[fold]
    pool = []
    for a in assets:
        if a.system in held:
            a = copy.copy(a)
            a.split = "test"
            pool.append(a)
        elif a.split == "test" and a.system in ("alpine-rootfs", "qemu"):
            pool.append(a)
    return {"heldout": layouts.generate(pool, junk, "test", EVAL_DISKS, BASE_SEED + 21, systems=held)}


def train_disks(fold, config, seed, assets, count):
    held = FOLDS[fold]
    pool = [a for a in assets if a.split == "train" and a.system not in held]
    if config.get("augment"):
        pool += [a for a in augmented(assets, seed) if a.system not in held]
    junk = synthetic.junk_assets(400, BASE_SEED + 1 + 100 * seed)
    mixed = int(count * config.get("mixed", 0))
    disks = layouts.generate(pool, junk, "train", count - mixed, BASE_SEED + 10 + 1000 * seed)
    if mixed:
        disks += layouts.generate(pool, junk, "train", mixed, BASE_SEED + 30 + 1000 * seed, "mixed")
    return disks


def outlier_windows(width, seed, count=3000):
    out = np.zeros((count, width), dtype=np.uint8)
    for i, (path, content) in enumerate(synthetic.outlier_files(count, seed)):
        out[i, :window.WINDOW] = np.frombuffer(window.file_window(path, content), dtype=np.uint8)
    return out


def masked(x, how):
    return {"name": dataset.name_only, "content": dataset.content_only}[how](x) if how else x


def holdout_b2(flat, held):
    """The candidates the B2 report's fold kept: held-out releases of the test split, and the rest."""
    return [i for i, c in enumerate(flat) if c.system not in held or c.release in catalogue.TEST_RELEASES.get(c.system, ())]


def tuples(array):
    return [tuple(int(v) for v in row) for row in array]


def measures(flat, y, predicted, subset=None):
    idx = list(range(len(flat))) if subset is None else subset
    truth, guess = tuples(y[idx]), tuples(predicted[idx])
    summary = measure.summarise(truth, guess, [flat[i].file_id for i in idx], [(flat[i].system, flat[i].release) for i in idx])
    summary["safety"] = evaluate.summarise(truth, guess)["safety"]
    summary["exact_decision"] = evaluate.summarise(truth, guess)["exact_decision"]
    return summary


def run(fold, config_name, seed, train_count=4000, epochs=12, force=False):
    RUNS.mkdir(parents=True, exist_ok=True)
    stem = RUNS / f"{fold}__{config_name}__s{seed}"
    asked = {"train_disks": train_count, "epochs": epochs}
    if stem.with_suffix(".json").is_file() and not force:
        cached = json.loads(stem.with_suffix(".json").read_text())
        if {k: cached.get(k) for k in asked} == asked:
            return cached
        log(f"{stem.name}: cached with other settings ({cached.get('train_disks')} disks, {cached.get('epochs')} epochs); running again")
    config = CONFIGS[config_name]
    assets = load_assets()
    start = time.time()
    size, neigh = config.get("size", False), config.get("neighbours", False)
    train = train_disks(fold, config, seed, assets, train_count)
    flat_t, x_train, y_train = disks_to_arrays(train, size, neigh)
    x_train = masked(x_train, config.get("mask"))
    outliers = outlier_windows(x_train.shape[1], BASE_SEED + 40 + seed) if config.get("outliers") else None
    log(f"{fold}/{config_name}/s{seed}: {len(train)} disks, {len(flat_t)} candidates, window {x_train.shape[1]}")
    params = training.train(x_train, y_train, BASE_SEED + seed, epochs=epochs, log=lambda m: None, outliers=outliers)
    shape, tensors = training.quantise(params, x_train[:4000])
    result = {"fold": fold, "config": config_name, "seed": seed, "train_disks": train_count, "epochs": epochs,
              "train_candidates": len(flat_t),
              "train_distinct_files": len({c.file_id for c in flat_t}), "window": int(x_train.shape[1]), "sets": {}}
    saved = {}
    for name, disks in eval_disks(fold, assets).items():
        flat, x, y = disks_to_arrays(disks, size, neigh)
        scores = training.int_forward(shape, tensors, masked(x, config.get("mask")))
        predicted = training.decode_all(scores)
        entry = measures(flat, y, predicted)
        if fold != "indist":
            entry["b2_subset"] = measures(flat, y, predicted, holdout_b2(flat, FOLDS[fold]))
        sizes, start_i, per_disk_t, per_disk_p = [], 0, [], []
        for disk in disks:
            per_disk_t.append(tuples(y[start_i:start_i + len(disk)]))
            per_disk_p.append(tuples(predicted[start_i:start_i + len(disk)]))
            start_i += len(disk)
        entry["disks"] = evaluate.per_disk(per_disk_t, per_disk_p)
        result["sets"][name] = entry
        saved[f"{name}__scores"] = scores.astype(np.int32)
    result["seconds"] = time.time() - start
    np.savez_compressed(stem.with_suffix(".npz"), **saved)
    if fold == "indist" and config_name == "base":
        np.savez(stem.with_name(stem.name + "__params.npz"), **params)
    write_atomically(stem.with_suffix(".json"), json.dumps(result, indent=1).encode())
    files = result["sets"][next(iter(result["sets"]))]["files"]
    log(f"{fold}/{config_name}/s{seed} done in {result['seconds']:.0f}s: files all-three {files['all_three']:.3f}, "
        f"kernel {files['kernel_recall']}, initrd {files['initrd_recall']}, false kernel {files['false_kernel']}")
    return result


def queue(folds, configs, seeds, jobs):
    assets = load_assets()
    for seed in seeds:
        if any(CONFIGS[c].get("augment") for c in configs):
            augmented(assets, seed)
    todo = [(f, c, s) for c in configs for s in seeds for f in folds if not (RUNS / f"{f}__{c}__s{s}.json").is_file()]
    log(f"{len(todo)} runs to do with {jobs} workers")
    env = {**os.environ, "OPENBLAS_NUM_THREADS": "1", "OMP_NUM_THREADS": "1", "VECLIB_MAXIMUM_THREADS": "1"}
    running = []
    while todo or running:
        while todo and len(running) < jobs:
            fold, config, seed = todo.pop(0)
            logfile = open(CACHE / f"log-{fold}__{config}__s{seed}.txt", "w")
            running.append((subprocess.Popen([sys.executable, str(HERE / "b2b.py"), "run", "--fold", fold, "--config", config,
                                              "--seed", str(seed)], env=env, stdout=logfile, stderr=subprocess.STDOUT), (fold, config, seed)))
        time.sleep(5)
        for item in list(running):
            code = item[0].poll()
            if code is not None:
                running.remove(item)
                log(f"{item[1]} exited with {code}")


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    one = sub.add_parser("run")
    one.add_argument("--fold", required=True, choices=sorted(FOLDS))
    one.add_argument("--config", required=True, choices=sorted(CONFIGS))
    one.add_argument("--seed", type=int, default=0)
    one.add_argument("--train-disks", type=int, default=4000)
    one.add_argument("--force", action="store_true")
    many = sub.add_parser("queue")
    many.add_argument("--folds", nargs="+", default=sorted(FOLDS), choices=sorted(FOLDS))
    many.add_argument("--configs", nargs="+", default=["base"], choices=sorted(CONFIGS))
    many.add_argument("--seeds", nargs="+", type=int, default=[0, 1, 2])
    many.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    CACHE.mkdir(parents=True, exist_ok=True)
    if args.command == "run":
        run(args.fold, args.config, args.seed, args.train_disks, force=args.force)
    else:
        queue(args.folds, args.configs, args.seeds, args.jobs)


if __name__ == "__main__":
    main()
