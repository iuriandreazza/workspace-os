"""The arithmetic of the evaluation harness: plain counting, no third-party code."""


def accuracy(truth, predicted):
    if len(truth) != len(predicted):
        raise ValueError("truth and prediction differ in length")
    if not truth:
        raise ValueError("no examples")
    return sum(t == p for t, p in zip(truth, predicted)) / len(truth)


def confusion(truth, predicted, classes):
    """`matrix[true][predicted]` as counts, `classes` being the number of classes."""
    matrix = [[0] * classes for _ in range(classes)]
    for t, p in zip(truth, predicted):
        matrix[t][p] += 1
    return matrix


def precision_recall(matrix):
    """`[(precision, recall, support)]` per class; None where the denominator is zero."""
    out = []
    for c in range(len(matrix)):
        true_positive = matrix[c][c]
        predicted = sum(row[c] for row in matrix)
        actual = sum(matrix[c])
        out.append((
            true_positive / predicted if predicted else None,
            true_positive / actual if actual else None,
            actual,
        ))
    return out


def wilson(successes, total, z=1.96):
    """The 95 % Wilson interval of a proportion; a held-out set is a sample, not the truth."""
    if total == 0:
        raise ValueError("no examples")
    p = successes / total
    centre = p + z * z / (2 * total)
    spread = z * ((p * (1 - p) / total + z * z / (4 * total * total)) ** 0.5)
    denominator = 1 + z * z / total
    return (centre - spread) / denominator, (centre + spread) / denominator


def safety_counts(truth_role, truth_protocol, predicted_role, predicted_protocol,
                  kernel, ours_protocol):
    """The two cases that matter most for safety.

    `false_kernel`: a candidate that is not a kernel image, labelled as one.
    `missed_ours`: a candidate of our own kernel not labelled as our kernel.
    `kernel` is the index of the role "kernel"; `ours_protocol` the index of the
    protocol "ours". Returns `(false_kernel, not_kernels, missed_ours, ours)`.
    """
    false_kernel = not_kernels = missed_ours = ours = 0
    for tr, tp, pr, pp in zip(truth_role, truth_protocol, predicted_role, predicted_protocol):
        if tr != kernel:
            not_kernels += 1
            false_kernel += pr == kernel
        if tp == ours_protocol and tr == kernel:
            ours += 1
            missed_ours += not (pr == kernel and pp == ours_protocol)
    return false_kernel, not_kernels, missed_ours, ours
