"""What is reported about a model's predictions on a held-out fold (B2b): rates per distinct file and per release.

B2 counted candidates. One kernel file appears under many paths on many disks, and the test set held 24 distinct
kernels, so a rate per candidate repeats a few files many times. Here a *file* is one distinct asset (same system,
release and bytes); each file's rate is the share of its appearances that were right, and the headline is the mean of
the file rates. Standard library only; tested in the gate.
"""
import labels

KERNEL = labels.ROLES.index("kernel")
ROLE_NAMES = labels.ROLES


def _mean(values):
    values = list(values)
    return sum(values) / len(values) if values else None


def _rates(units, truth_of, predicted_of):
    """`units`: lists of row indices. Mean over units of the share of appearances right, for each measure."""
    def share(unit, test):
        return sum(1 for i in unit if test(i)) / len(unit)

    out = {"units": len(units)}
    out["all_three"] = _mean(share(u, lambda i: truth_of[i][:3] == predicted_of[i][:3]) for u in units)
    for index, (name, _) in enumerate(labels.SLICES[:3]):
        out[name] = _mean(share(u, lambda i, k=index: truth_of[i][k] == predicted_of[i][k]) for u in units)
    for role in ("kernel", "initrd", "dtb", "entry", "loader"):
        r = ROLE_NAMES.index(role)
        mine = [u for u in units if truth_of[u[0]][2] == r]
        out[f"{role}_recall"] = _mean(share(u, lambda i: predicted_of[i][2] == r) for u in mine)
        out[f"{role}_units"] = len(mine)
    others = [u for u in units if truth_of[u[0]][2] != KERNEL]
    out["false_kernel"] = _mean(share(u, lambda i: predicted_of[i][2] == KERNEL) for u in others)
    out["not_kernel_units"] = len(others)
    return out


def summarise(truth, predicted, file_ids, groups):
    """Candidate-level and file-level rates, and the file-level rates of each group.

    `truth` and `predicted` are `(family, protocol, role, group)` tuples, `file_ids` the identity of each
    candidate's file and `groups` the `(system, release)` it comes from. Within one file the role is the same on
    every disk, so a file's truth is read from its first appearance.
    """
    n = len(truth)
    candidates = _rates([[i] for i in range(n)], truth, predicted)
    by_file = {}
    for i, f in enumerate(file_ids):
        by_file.setdefault(f, []).append(i)
    files = _rates(list(by_file.values()), truth, predicted)
    per_group = {}
    for unit in by_file.values():
        per_group.setdefault(groups[unit[0]], []).append(unit)
    releases = {f"{system}/{release}": _rates(units, truth, predicted) for (system, release), units in sorted(per_group.items())}
    return {"candidates": candidates, "files": files, "releases": releases}
