"""Confidence of the boot model's decisions (B2b, option D of the accuracy note): the margin between the two best
scores of a head, a temperature that turns scores into probabilities, the expected calibration error and a
risk-coverage curve. Standard library only, so the arithmetic is tested in the gate.

A score row is the list of integer scores of one head (`labels.slice_bounds`).
"""
import math


def margin(scores):
    """Best score minus second best: what the loader could compare against a threshold with integers only."""
    first, second = sorted(scores, reverse=True)[:2]
    return first - second


def softmax(scores, temperature=1.0):
    top = max(scores)
    exps = [math.exp((s - top) / temperature) for s in scores]
    total = sum(exps)
    return [e / total for e in exps]


def confidence(scores, temperature=1.0):
    return max(softmax(scores, temperature))


def negative_log_likelihood(rows, truth, temperature):
    return -sum(math.log(softmax(r, temperature)[t] + 1e-12) for r, t in zip(rows, truth)) / len(rows)


def fit_temperature(rows, truth, grid=None):
    """The temperature with the lowest negative log-likelihood on `rows` (a grid, so it is reproducible)."""
    grid = grid or [2 ** (k / 4) for k in range(-12, 81)]  # 0.125 to 1,048,576: integer scores can be large
    return min(grid, key=lambda t: negative_log_likelihood(rows, truth, t))


def expected_calibration_error(confidences, correct, bins=15):
    """Weighted mean gap between confidence and accuracy over equal-width bins."""
    total = len(confidences)
    if total == 0:
        raise ValueError("no examples")
    sums = [[0, 0.0, 0] for _ in range(bins)]
    for c, ok in zip(confidences, correct):
        cell = sums[min(bins - 1, int(c * bins))]
        cell[0] += 1
        cell[1] += c
        cell[2] += bool(ok)
    return sum(abs(conf / n - hits / n) * n for n, conf, hits in sums if n) / total


def risk_coverage(confidences, correct, thresholds):
    """`[(threshold, coverage, risk)]`: of the examples whose confidence reaches the threshold, the share kept and
    the share of those that are wrong (None where nothing is kept)."""
    out = []
    for t in thresholds:
        kept = [ok for c, ok in zip(confidences, correct) if c >= t]
        out.append((t, len(kept) / len(correct), None if not kept else 1 - sum(map(bool, kept)) / len(kept)))
    return out


def trade_off(margins, truth_is_kernel, predicted_kernel, thresholds):
    """For each threshold, the false kernels removed and the real kernels lost by calling every predicted kernel
    whose margin is below it "unverified" (a margin equal to the threshold stays).

    Returns `[(threshold, false_kernels_left, false_kernels_removed, real_kernels_kept, real_kernels_lost)]`.
    """
    false_total = sum(1 for k, p in zip(truth_is_kernel, predicted_kernel) if p and not k)
    real_total = sum(1 for k, p in zip(truth_is_kernel, predicted_kernel) if p and k)
    out = []
    for t in thresholds:
        false_left = sum(1 for m, k, p in zip(margins, truth_is_kernel, predicted_kernel) if p and not k and m >= t)
        real_kept = sum(1 for m, k, p in zip(margins, truth_is_kernel, predicted_kernel) if p and k and m >= t)
        out.append((t, false_left, false_total - false_left, real_kept, real_total - real_kept))
    return out
