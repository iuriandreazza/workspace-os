"""Rules that attach the companions of a kernel from a disk's labelled files (B2b, ADR 0034).

ADR 0034 (`accepted`) has the model say what each file *is* and code decide which files go together. The
rules here are the candidates for that code, measured on the host so the numbers exist before B3 builds one:

- `same_directory`: a companion belongs to the kernel in its own directory;
- `directory_suffix`: the same, and the version suffix of the names must agree (`vmlinuz-6.1.0-18-arm64` and
  `initrd.img-6.1.0-18-arm64`; `linux` and `initrd` have no suffix and match on the directory alone);
- `entry_text`: the `linux`, `initrd` and `devicetree` lines of a Boot Loader Specification entry name paths;
- `adr0034`: the entry text where a kernel is named by one, the directory and suffix rule for the others.

A *file* is `(path, role, text)`: the role as a name of `labels.ROLES` (given by the truth, or by the model), the
whole content of a file the model called `entry` (a window is cut at 448 bytes and a long line may not fit in it).
A rule returns the set of `(kernel index, companion index)` pairs it asserts, over the files whose role makes
them a kernel-like file (kernel, loader) and a companion (initrd, dtb, entry).

Standard library only; tested in the gate.
"""
import re

KERNEL_LIKE = ("kernel", "loader")
COMPANIONS = ("initrd", "dtb", "entry")


def normalise(path):
    """The way a volume compares paths: backslashes, a leading one, no case."""
    path = path.replace("/", "\\")
    return ("" if path.startswith("\\") else "\\") + path.lower()


def directory(path):
    return normalise(path).rsplit("\\", 1)[0]


def file_name(path):
    return normalise(path).rsplit("\\", 1)[1]


def version_suffix(path):
    """The part of the name after the first dash, without a trailing `.img`; empty for `linux`, `initrd`, `Image`."""
    name = file_name(path)
    if "-" not in name:
        return ""
    suffix = name.split("-", 1)[1]
    return suffix[:-4] if suffix.endswith(".img") else suffix


def parse_entry_text(content):
    """`{"linux": path or None, "initrd": [paths], "devicetree": [paths]}` of one entry's text.

    The paths are normalised. Lines that are not `key value`, comments and non-text bytes are ignored: the
    caller passes bytes of any file the model labelled `entry`, and a wrong label must not raise.
    """
    found = {"linux": None, "initrd": [], "devicetree": []}
    for line in content.decode("utf-8", "replace").splitlines():
        parts = re.split(r"\s+", line.strip(), maxsplit=1)
        if len(parts) != 2:
            continue
        key, value = parts[0], parts[1].strip()
        if not value.startswith(("/", "\\")):
            continue
        if key == "linux" and found["linux"] is None:
            found["linux"] = normalise(value)
        elif key in ("initrd", "devicetree"):
            found[key].append(normalise(value))
    return found


def _split(files):
    kernels = [i for i, (_, role, _) in enumerate(files) if role in KERNEL_LIKE]
    companions = [i for i, (_, role, _) in enumerate(files) if role in COMPANIONS]
    return kernels, companions


def same_directory(files):
    kernels, companions = _split(files)
    return {(k, c) for k in kernels for c in companions
            if files[k][0] and files[c][0] and directory(files[k][0]) == directory(files[c][0])}


def directory_suffix(files):
    return {(k, c) for k, c in same_directory(files)
            if files[c][1] != "entry" and version_suffix(files[k][0]) == version_suffix(files[c][0])}


def entry_text(files):
    """Pairs named by the text of an entry; returns them and the kernels some entry text names."""
    kernels, companions = _split(files)
    by_path = {normalise(files[i][0]): i for i in kernels + companions if files[i][0]}
    pairs, named = set(), set()
    for e in companions:
        if files[e][1] != "entry" or files[e][2] is None:
            continue
        parsed = parse_entry_text(files[e][2])
        k = by_path.get(parsed["linux"])
        if k is None or k not in kernels:
            continue
        named.add(k)
        pairs.add((k, e))
        for path in parsed["initrd"] + parsed["devicetree"]:
            c = by_path.get(path)
            if c is not None and c in companions:
                pairs.add((k, c))
    return pairs, named


def adr0034(files):
    """The rule ADR 0034 proposes: the entry text where a kernel is named by one, the fallback for the others."""
    named_pairs, named = entry_text(files)
    return named_pairs | {(k, c) for k, c in directory_suffix(files) if k not in named}


RULES = {
    "same directory": same_directory,
    "directory + version suffix": directory_suffix,
    "entry text lines": lambda files: entry_text(files)[0],
    "ADR 0034 (entry text, else directory + suffix)": adr0034,
}


def true_pairs(candidates):
    """The pairs of a disk that really belong to one entry, from the true roles and entries."""
    pairs = set()
    for k, a in enumerate(candidates):
        if a.role not in KERNEL_LIKE or a.entry is None:
            continue
        for c, b in enumerate(candidates):
            if b.role in COMPANIONS and b.entry == a.entry:
                pairs.add((k, c))
    return pairs


def files_of(candidates, roles=None):
    """`(path, role, text)` of each candidate, with the true role or the given ones (`roles[i]`, a name)."""
    return [(c.path, c.role if roles is None else roles[i], c.text) for i, c in enumerate(candidates)]


def score(disks, rule, predicted_roles=None):
    """Pair precision and recall of `rule` over `disks` (lists of candidates) and how many disks it gets entirely right.

    With `predicted_roles` (one list of role names per disk) the rule sees the model's roles while the truth stays
    the one of the true roles, so a mislabelled file costs a pair. `disks_all_right` counts the disks that have
    pairs and whose asserted set equals the true one; `spurious_on_pairless` the disks with no true pair on which
    the rule asserted one. `missed_by_role` and `wrong_by_role` count the pairs left out and the pairs wrongly
    asserted by the true role of the companion.
    """
    true_positive = asserted = actual = with_pairs = right = spurious = 0
    missed, wrong = {}, {}  # by the companion's true role
    for n, disk in enumerate(disks):
        truth = true_pairs(disk)
        got = rule(files_of(disk, None if predicted_roles is None else predicted_roles[n]))
        true_positive += len(truth & got)
        asserted += len(got)
        actual += len(truth)
        for _, c in truth - got:
            missed[disk[c].role] = missed.get(disk[c].role, 0) + 1
        for _, c in got - truth:
            wrong[disk[c].role] = wrong.get(disk[c].role, 0) + 1
        if truth:
            with_pairs += 1
            right += truth == got
        elif got:
            spurious += 1
    return {"precision": true_positive / asserted if asserted else None,
            "recall": true_positive / actual if actual else None,
            "pairs_asserted": asserted, "pairs_true": actual,
            "disks_with_pairs": with_pairs, "disks_all_right": right, "spurious_on_pairless": spurious,
            "missed_by_role": missed, "wrong_by_role": wrong}
