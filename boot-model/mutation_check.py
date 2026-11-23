#!/usr/bin/env python3
"""Confirms by mutation that the tests of the boot model tooling notice a change (B2).

    python3 -I boot-model/mutation_check.py

Each mutation below changes one thing in a copy of this directory and the unit tests
must then fail; a mutation the tests survive is reported and the script exits with 1.
It is run on demand, not in the gate: it runs the tests once per mutation.
"""
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent

MUTATIONS = [
    # window.py: the contract with B3
    ("window.py", "narrow(name)[-NAME_BYTES:]", "narrow(name)[:NAME_BYTES]", "name keeps the head, not the tail"),
    ("window.py", "bytes(NAME_BYTES - len(tail)) + tail", "tail + bytes(NAME_BYTES - len(tail))", "name left-aligned"),
    ("window.py", "code if code < 0x80 else ord(\"?\")", "code & 0xFF", "non-ASCII not narrowed to ?"),
    ("window.py", "out += b\"??\"", "out += b\"?\"", "a surrogate pair is one unit"),
    ("window.py", "name[:36]", "name[:40]", "partition name limit"),
    ("window.py", "if not path.startswith(\"\\\\\"):", "if False:", "path without a backslash accepted"),
    ("window.py", "CONTENT_BYTES = WINDOW - NAME_BYTES", "CONTENT_BYTES = WINDOW - NAME_BYTES - 1", "content window one byte short"),
    ("window.py", "PARTITION_CONTENT_BYTES = CONTENT_BYTES - GUID_BYTES", "PARTITION_CONTENT_BYTES = CONTENT_BYTES", "partition content ignores the GUID"),
    # labels.py
    ("labels.py", "key=lambda i: (part[i], -i)", "key=lambda i: (part[i], i)", "a tie goes to the highest index"),
    ("labels.py", "if not 0 <= group < GROUPS:", "if not 0 <= group <= GROUPS:", "group 16 accepted"),
    ("labels.py", "FAMILIES = (\"none\", \"ours\",", "FAMILIES = (\"ours\", \"none\",", "family order changed"),
    ("labels.py", "ROLES = (\"other\", \"kernel\",", "ROLES = (\"kernel\", \"other\",", "role order changed"),
    ("labels.py", "(\"group\", GROUPS),", "(\"group\", GROUPS - 1),", "group slice one short"),
    # mdl.py
    ("mdl.py", "return [max(0, min(127, s >> shift)) for s in sums]", "return [min(127, s >> shift) for s in sums]", "no ReLU"),
    ("mdl.py", "min(127, s >> shift)", "min(127, s >> (shift + 1))", "shift off by one"),
    ("mdl.py", "max(0, min(127, s >> shift))", "max(0, min(128, s >> shift))", "clamp at 128"),
    ("mdl.py", "return body + CRC.pack(zlib.crc32(body))", "return body + CRC.pack(zlib.crc32(payload))", "CRC over the payload only"),
    ("mdl.py", "if magic != MAGIC:\n        raise ValueError(\"bad magic\")", "if False:\n        raise ValueError(\"bad magic\")", "magic not checked"),
    ("mdl.py", "if version != VERSION:", "if False:", "version not checked"),
    ("mdl.py", "if len(data) != HEADER.size + length + CRC.size:", "if False:", "trailing bytes accepted"),
    ("mdl.py", "if length != shape.weight_bytes():", "if False:", "payload length not checked"),
    ("mdl.py", "embedding[byte * e:(byte + 1) * e]", "embedding[byte * e + 1:(byte + 1) * e + 1]", "embedding row off by one"),
    ("mdl.py", "return dense(heads, shape.hidden2, hidden2)", "return requantise(dense(heads, shape.hidden2, hidden2), 0)", "scores clamped and rectified"),
    ("mdl.py", "if shape.weight_bytes() > MAX_WEIGHT_BYTES:", "if shape.weight_bytes() >= MAX_WEIGHT_BYTES:", "weight limit exclusive"),
    ("mdl.py", "if shape.macs() > MAX_MACS:", "if shape.macs() >= MAX_MACS:", "MAC limit exclusive"),
    ("mdl.py", "return d1 + d2 + d3", "return d1 + d2", "heads not counted in the MACs"),
    ("mdl.py", "if tensor and not (-128 <= min(tensor) and max(tensor) <= 127):", "if False:", "int8 range not checked"),
    # metrics.py
    ("metrics.py", "matrix[t][p] += 1", "matrix[p][t] += 1", "confusion transposed"),
    ("metrics.py", "true_positive / predicted if predicted else None", "true_positive / actual if actual else None", "precision uses the recall's denominator"),
    ("metrics.py", "centre + spread", "centre - spread + 2 * spread * 0.9", "interval upper bound"),
    ("metrics.py", "false_kernel += pr == kernel", "false_kernel += pr != kernel", "false kernel counted wrong"),
    ("metrics.py", "if tp == ours_protocol and tr == kernel:", "if tp == ours_protocol:", "ours counted without the kernel role"),
    ("metrics.py", "missed_ours += not (pr == kernel and pp == ours_protocol)", "missed_ours += not (pr == kernel)", "ours with the wrong protocol not a miss"),
    ("metrics.py", "raise ValueError(\"truth and prediction differ in length\")", "pass", "length mismatch ignored"),
    # evaluate.py
    ("evaluate.py", "if p[2] in (kernel, loader, initrd) and t[2] != p[2]:", "if p[2] in (kernel, loader) and t[2] != p[2]:", "an invented initrd is safe"),
    ("evaluate.py", "if t[1] == ours and t[2] == kernel and (p[2] != kernel or p[1] != ours):", "if t[1] == ours and t[2] == kernel and p[2] != kernel:", "ours with the wrong protocol is safe"),
    ("evaluate.py", "return all(t[:3] == p[:3] for t, p in zip(truth, predicted))", "return all(t == p for t, p in zip(truth, predicted))", "the group counts in a disk's exactness"),
    ("evaluate.py", "metrics.accuracy([row[:3] for row in truth], [row[:3] for row in predicted])", "metrics.accuracy([row[:2] for row in truth], [row[:2] for row in predicted])", "exact decision ignores the role"),
    # catalogue.py
    ("catalogue.py", "if low.startswith(\"bsd\"):\n            return (\"bsd\", \"none\", \"kernel\")", "if low.startswith(\"bsd\"):\n            return (\"bsd\", \"linux\", \"kernel\")", "a BSD kernel starts through the Linux stub"),
    ("catalogue.py", "return (\"linux\", \"chainload\", \"loader\")\n    return other", "return (\"linux\", \"linux\", \"loader\")\n    return other", "a loader uses the Linux protocol"),
    ("catalogue.py", "system = SAME_LAYOUT.get(system, system)", "system = system", "rebuilds not mapped to the Fedora layout"),
    ("catalogue.py", "if release in TEST_RELEASES.get(system, ()):\n        return TEST", "if False:\n        return TEST", "no final test split"),
    ("catalogue.py", "if release in VALIDATION_RELEASES.get(system, ()):\n        return VALIDATION", "if False:\n        return VALIDATION", "no validation split"),
    ("catalogue.py", "if low.endswith(\".dtb\"):", "if low.endswith(\".dtbo\"):", "device trees not recognised"),
    ("catalogue.py", "not content.startswith(b\"MZ\")", "content.startswith(b\"MZ\")", "refine inverted"),
    ("catalogue.py", "return family, \"none\", role\n    return labels", "return family, \"linux\", role\n    return labels", "gzip kernel keeps the stub protocol"),
    ("catalogue.py", "if role == \"kernel\" and protocol == \"linux\" and", "if role == \"kernel\" and", "every kernel refined"),
    # layouts.py
    ("layouts.py", "candidates.append(_candidate(path, asset, NO_ENTRY, None, disk_id, split))\n    if mode == \"decoy\"", "candidates.append(_candidate(path, asset, 0, None, disk_id, split))\n    if mode == \"decoy\"", "files outside an entry get slot 0"),
    ("layouts.py", "for slot, entry in zip(order, entries):", "for slot, entry in zip([0] * len(order), entries):", "all entries share a slot"),
    ("layouts.py", "entries = entries[:MAX_ENTRIES]", "entries = entries[:MAX_ENTRIES + 1]", "more entries than slots"),
    ("layouts.py", "corpus = Corpus([a for a in assets if a.split == split], junk)", "corpus = Corpus(list(assets), junk)", "a split uses the other split's files"),
    ("layouts.py", "renamed.add(f\"\\\\{hexname(rng)}{rng.choice(('.bin', '.dat', '', '.img'))}\", asset)", "renamed.add(path, asset)", "renamed mode keeps the names"),
    ("layouts.py", "if len(set(paths)) == len(paths) and not used & set(paths):", "if True:", "repeated paths kept"),
    ("layouts.py", "paths = [path.lower() for path, _ in entry.files]", "paths = [path for path, _ in entry.files]", "paths compared case-sensitively"),
    ("layouts.py", "if path.lower() not in used:\n            used.add(path.lower())", "if True:\n            used.add(path.lower())", "a file may reuse an entry's path"),
    # fetch.py
    ("fetch.py", "if gnu and gnu.group(2) == name:", "if gnu and name in gnu.group(2):", "checksum name matched by substring"),
    ("fetch.py", "if sha256(data) != pin_[\"sha256\"]:\n        raise ValueError", "if False:\n        raise ValueError", "cache entry not re-checked"),
    # synthetic.py
    ("synthetic.py", "return b\"WSMD\" + struct.pack(\"<I\", 1)", "return b\"WSMX\" + struct.pack(\"<I\", 1)", "model stand-in without the magic"),
    ("synthetic.py", "_guid(\"C12A7328-F81F-11D2-BA4B-00A0C93EC93B\")", "_guid(\"C12A7328-F81F-11D2-BA4B-00A0C93EC93C\")", "ESP type GUID changed"),
]


def run_tests(directory):
    for test in ("test_core.py", "test_corpus.py"):
        result = subprocess.run([sys.executable, "-I", str(directory / test)], capture_output=True)
        if result.returncode != 0:
            return False
    return True


def main():
    survivors, skipped = [], []
    with tempfile.TemporaryDirectory(prefix="wsos-mutation-") as scratch:
        base = Path(scratch) / "boot-model"
        shutil.copytree(HERE, base, ignore=shutil.ignore_patterns("__pycache__"))
        assert run_tests(base), "the unmutated tests must pass"
        for name, old, new, why in MUTATIONS:
            path = base / name
            original = path.read_text()
            if original.count(old) != 1:
                skipped.append((name, why, f"{original.count(old)} matches"))
                continue
            path.write_text(original.replace(old, new))
            try:
                killed = not run_tests(base)
            finally:
                path.write_text(original)
            print(f"{'killed  ' if killed else 'SURVIVED'} {name}: {why}")
            if not killed:
                survivors.append((name, why))
    for item in skipped:
        print("not applied:", *item)
    if survivors or skipped:
        sys.exit(1)
    print(f"all {len(MUTATIONS)} mutations were noticed")


if __name__ == "__main__":
    main()
