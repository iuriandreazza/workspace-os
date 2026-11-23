#!/usr/bin/env python3
"""Evaluates a stored `\\WSOS.MDL` on the generated sets without training (B2).

    uv run --no-project --with numpy==2.5.5 python boot-model/evaluate_model.py target/boot-model/WSOS.MDL

Regenerates the disks of `run_b2.py` from the same seeds and prints, per set, the all-three accuracy,
the safety counts, the per-disk counts and, for the decoy sets, how many decoy files there were. The
decoy figures of the report come from here: they were recomputed with the stored weights after the
generator was fixed so that a decoy never shares a path with a real file.
"""
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import catalogue  # noqa: E402
import dataset  # noqa: E402
import evaluate  # noqa: E402
import layouts  # noqa: E402
import mdl  # noqa: E402
import synthetic  # noqa: E402
import training  # noqa: E402

SEED = 20261123
DECOY_PATHS = {"\\vmlinuz", "\\boot\\vmlinuz-6.1.0-18-arm64", "\\initrd.img", "\\KERNEL.ELF",
               "\\EFI\\BOOT\\BOOTAA64.EFI", "\\boot\\Image", "\\bsd", "\\grubaa64.efi"}
SETS = (("validation", 2, 11, 1500, "normal"), ("validation-decoy", 2, 13, 600, "decoy"),
        ("test", 3, 14, 1500, "normal"), ("test-decoy", 3, 16, 600, "decoy"))


def main(path):
    shape, tensors = mdl.read(Path(path).read_bytes())
    assets = catalogue.pinned_assets() + catalogue.qemu_dtb_assets()
    for name, junk_offset, disk_offset, count, mode in SETS:
        split = name.split("-")[0]
        disks = layouts.generate(assets, synthetic.junk_assets(400, SEED + junk_offset), split, count,
                                 SEED + disk_offset, mode)
        flat, x, y = dataset.arrays(disks)
        predicted = training.decode_all(training.int_forward(shape, tensors, x))
        truth = [tuple(map(int, row)) for row in y]
        guess = [tuple(map(int, row)) for row in predicted]
        summary = evaluate.summarise(truth, guess)
        start, truth_disks, guess_disks = 0, [], []
        for disk in disks:
            truth_disks.append(truth[start:start + len(disk)])
            guess_disks.append(guess[start:start + len(disk)])
            start += len(disk)
        decoys = sum(1 for c in flat if c.path in DECOY_PATHS and c.entry is None and c.role == "other"
                     and c.system in ("junk", "alpine-rootfs"))
        print(f"{name}: {len(flat)} candidates, all three {100 * summary['exact_decision']:.2f} %, "
              f"safety {summary['safety']}, disks (total, exact, safe) {evaluate.per_disk(truth_disks, guess_disks)}"
              + (f", decoy files {decoys}" if mode == "decoy" else ""))


if __name__ == "__main__":
    main(sys.argv[1])
