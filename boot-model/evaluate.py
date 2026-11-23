"""The evaluation harness: what is reported about a model's predictions on a held-out
set (B2). Standard library only, so its arithmetic is tested in the gate.

`truth` and `predicted` are lists of `(family, protocol, role, group)` index tuples.
"""
import labels
import metrics


def column(rows, index):
    return [row[index] for row in rows]


def summarise(truth, predicted):
    """Per-output accuracy with its 95 % interval, the confusion matrix and per-class precision and recall."""
    out = {"candidates": len(truth), "outputs": {}}
    for index, (name, size) in enumerate(labels.SLICES):
        t, p = column(truth, index), column(predicted, index)
        hits = sum(a == b for a, b in zip(t, p))
        matrix = metrics.confusion(t, p, size)
        out["outputs"][name] = {
            "accuracy": hits / len(t),
            "interval": metrics.wilson(hits, len(t)),
            "confusion": matrix,
            "per_class": metrics.precision_recall(matrix),
        }
    out["exact_decision"] = metrics.accuracy([row[:3] for row in truth], [row[:3] for row in predicted])
    kernel, ours = labels.ROLES.index("kernel"), labels.PROTOCOLS.index("ours")
    f, n, m, o = metrics.safety_counts(column(truth, 2), column(truth, 1), column(predicted, 2),
                                       column(predicted, 1), kernel, ours)
    out["safety"] = {"false_kernel": f, "not_kernels": n, "missed_ours": m, "ours_kernels": o}
    return out


def disk_correct(truth, predicted):
    """Whether one disk's decision is right: every candidate's family, protocol and role is exact.

    A disk with one wrong label can still boot (a wrong label on a file that is ignored), but this is the strict
    reading: the loader would assemble the right entries only if nothing was wrong.
    """
    return all(t[:3] == p[:3] for t, p in zip(truth, predicted))


def disk_safe(truth, predicted):
    """No candidate that is not a kernel was labelled a kernel, no loader or initrd invented, none of ours missed."""
    kernel, loader, initrd = (labels.ROLES.index(r) for r in ("kernel", "loader", "initrd"))
    ours = labels.PROTOCOLS.index("ours")
    for t, p in zip(truth, predicted):
        if p[2] in (kernel, loader, initrd) and t[2] != p[2]:
            return False
        if t[1] == ours and t[2] == kernel and (p[2] != kernel or p[1] != ours):
            return False
    return True


def per_disk(disks_truth, disks_predicted):
    """`(disks, all labels exact, safe)`."""
    exact = sum(disk_correct(t, p) for t, p in zip(disks_truth, disks_predicted))
    safe = sum(disk_safe(t, p) for t, p in zip(disks_truth, disks_predicted))
    return len(disks_truth), exact, safe


def format_percent(value, digits=1):
    return "n/a" if value is None else f"{100 * value:.{digits}f} %"


def confusion_table(matrix, names):
    """A Markdown table: rows are the truth, columns the prediction; empty classes are left out."""
    used = [i for i in range(len(matrix)) if sum(matrix[i]) or sum(row[i] for row in matrix)]
    head = "| truth \\ predicted | " + " | ".join(names[i] for i in used) + " |"
    rule = "|---|" + "---:|" * len(used)
    rows = ["| " + names[i] + " | " + " | ".join(str(matrix[i][j]) for j in used) + " |" for i in used]
    return "\n".join([head, rule] + rows)


def class_table(per_class, names):
    rows = ["| class | support | precision | recall |", "|---|---:|---:|---:|"]
    for name, (precision, recall, support) in zip(names, per_class):
        if support or precision is not None:
            rows.append(f"| {name} | {support} | {format_percent(precision)} | {format_percent(recall)} |")
    return "\n".join(rows)
