"""Training and integer evaluation of the boot model with numpy (B2). Host only; never shipped.

Not part of the gate: it needs numpy (BSD-3-Clause) and is run with `uv` (see the report).
The deterministic parts (windows, labels, the file, the arithmetic of metrics) are
tested without it in `test_core.py`.

Network: the embedding of each byte of the window (256 x 32), laid end to end
(512 x 32 = 16,384 numbers), a dense layer of 256 with ReLU, one of 128 with ReLU, and
34 scores: the four heads of `labels.py`. No bias, as in the program that measured the time.
Each head is trained with its own softmax cross-entropy and the four losses add.
"""
import numpy as np

import labels
import mdl

WINDOW, EMBEDDING, HIDDEN1, HIDDEN2 = 512, 32, 256, 128
HEAD_NAMES = [name for name, _ in labels.SLICES]


def initial_params(rng, window=WINDOW):
    return {
        "E": rng.normal(0, 0.5, (256, EMBEDDING)).astype(np.float32),
        "W1": (rng.normal(0, 1, (window * EMBEDDING, HIDDEN1)) * np.sqrt(2 / (window * EMBEDDING))).astype(np.float32),
        "W2": (rng.normal(0, 1, (HIDDEN1, HIDDEN2)) * np.sqrt(2 / HIDDEN1)).astype(np.float32),
        "W3": (rng.normal(0, 1, (HIDDEN2, labels.HEADS)) * np.sqrt(1 / HIDDEN2)).astype(np.float32),
    }


def forward(params, x):
    b = x.shape[0]
    feats = params["E"][x].reshape(b, -1)
    h1 = np.maximum(feats @ params["W1"], 0)
    h2 = np.maximum(h1 @ params["W2"], 0)
    return feats, h1, h2, h2 @ params["W3"]


def loss_and_grad_logits(logits, y, head_weights, outliers=None, outlier_weight=0.0):
    """Sum of the cross-entropies of the heads, and its gradient with respect to the logits.

    `outliers` is a boolean mask of rows that belong to no class (outlier exposure, B2b): for them the family,
    protocol and role heads are pulled to the uniform distribution (cross-entropy to it, weighted by
    `outlier_weight`) and the group head is left alone; the labels of those rows are ignored by the ordinary loss.
    """
    total, grad = 0.0, np.zeros_like(logits)
    rows = np.arange(logits.shape[0])
    ordinary = np.ones(len(rows), dtype=bool) if outliers is None else ~outliers
    count, extra = max(1, int(ordinary.sum())), 0 if outliers is None else int(outliers.sum())
    for head, (_, (start, end)) in enumerate(labels.slice_bounds().items()):
        part = logits[:, start:end]
        part = part - part.max(axis=1, keepdims=True)
        prob = np.exp(part)
        prob /= prob.sum(axis=1, keepdims=True)
        hit = np.where(ordinary, y[:, head], 0)
        total += head_weights[head] * -np.log(prob[rows, hit] + 1e-9)[ordinary].sum() / count
        step = prob.copy()
        step[rows, hit] -= 1
        step[~ordinary] = 0
        grad[:, start:end] = head_weights[head] * step / count
        if extra and head < 3:
            uniform = 1.0 / (end - start)
            total += outlier_weight * -np.log(prob[outliers] + 1e-9).mean(axis=1).sum() / extra
            grad[outliers, start:end] = outlier_weight * (prob[outliers] - uniform) / extra
    return total, grad


def train(x, y, seed, epochs=12, batch=128, lr=1e-3, head_weights=(1.0, 1.0, 1.0, 0.05), log=print, params=None,
          drop_content=0.25, drop_name=0.15, fresh_tails=0.8, outliers=None, outlier_share=0.1, outlier_weight=0.5):
    """`outliers`: windows that belong to no class (B2b); a share of each batch is drawn from them.

    The window may be longer than 512 (extra inputs after the content, B2b): the augmentations touch only
    positions below `WINDOW`.
    """
    rng = np.random.default_rng(seed)
    params = params or initial_params(rng, x.shape[1])
    moment = {k: np.zeros_like(v) for k, v in params.items()}
    velocity = {k: np.zeros_like(v) for k, v in params.items()}
    step = 0
    total_steps = epochs * ((len(x) + batch - 1) // batch)
    for epoch in range(epochs):
        order = rng.permutation(len(x))
        running = 0.0
        for start in range(0, len(x), batch):
            idx = order[start:start + batch]
            xb = fresh_random_tails(x[idx], rng, fresh_tails)
            xb, yb = modality_dropout(xb, rng, drop_content, drop_name), y[idx]
            flagged = None
            if outliers is not None:
                extra = outliers[rng.integers(0, len(outliers), max(1, int(len(idx) * outlier_share)))]
                xb = np.concatenate([xb, extra])
                yb = np.concatenate([yb, np.zeros((len(extra), yb.shape[1]), dtype=yb.dtype)])
                flagged = np.arange(len(xb)) >= len(idx)
            feats, h1, h2, logits = forward(params, xb)
            value, d_logits = loss_and_grad_logits(logits, yb, head_weights, flagged, outlier_weight)
            d_h2 = (d_logits @ params["W3"].T) * (h2 > 0)
            d_h1 = (d_h2 @ params["W2"].T) * (h1 > 0)
            d_feats = d_h1 @ params["W1"].T
            grads = {"W3": h2.T @ d_logits, "W2": h1.T @ d_h2, "W1": feats.T @ d_h1,
                     "E": embedding_grad(xb, d_feats)}
            step += 1
            rate = lr * 0.5 * (1 + np.cos(np.pi * step / total_steps)) + 1e-6  # cosine decay: no late blow-up
            for key, g in grads.items():
                moment[key] = 0.9 * moment[key] + 0.1 * g
                velocity[key] = 0.999 * velocity[key] + 0.001 * g * g
                m_hat = moment[key] / (1 - 0.9 ** step)
                v_hat = velocity[key] / (1 - 0.999 ** step)
                params[key] -= rate * m_hat / (np.sqrt(v_hat) + 1e-8)
            running += value
        log(f"epoch {epoch + 1}/{epochs}  loss {running / max(1, len(x) // batch):.4f}")
    return params


HEADER_END = 128  # the name and the first 64 bytes of content: magic numbers and format headers live here
HIGH_ENTROPY = 150  # distinct byte values in the tail of a 320-byte tail; random bytes give about 180


def fresh_random_tails(xb, rng, probability):
    """Replaces the tail of compressed-looking windows by new random bytes.

    A compressed stream (an initrd, a gzip file, a kernel's payload) is the same noise to the
    model whatever it holds, but with a few hundred distinct windows it can memorise each one.
    Fresh bytes every time keep it from doing that, and keep the headers it should learn from.
    """
    xb = xb.copy()
    tail = np.sort(xb[:, HEADER_END:WINDOW], axis=1)
    distinct = 1 + (np.diff(tail.astype(np.int16), axis=1) != 0).sum(axis=1)
    rows = np.flatnonzero((distinct > HIGH_ENTROPY) & (rng.random(len(xb)) < probability))
    xb[rows, HEADER_END:WINDOW] = rng.integers(0, 256, (len(rows), WINDOW - HEADER_END), dtype=np.uint8)
    return xb


def modality_dropout(xb, rng, drop_content, drop_name):
    """Zeroes the content (or the name) of some windows. Bytes of a compressed file are noise that a
    network this size memorises; hiding them at random makes it learn from the name as well, and the
    other way round, so neither modality alone decides."""
    xb = xb.copy()
    roll = rng.random(len(xb))
    xb[roll < drop_content, 64:WINDOW] = 0
    xb[(roll >= drop_content) & (roll < drop_content + drop_name), :64] = 0
    return xb


def embedding_grad(x, d_feats):
    flat = x.reshape(-1)
    d = d_feats.reshape(-1, EMBEDDING)
    out = np.zeros((256, EMBEDDING), dtype=np.float32)
    for column in range(EMBEDDING):
        out[:, column] = np.bincount(flat, weights=d[:, column], minlength=256)
    return out


def float_predict(params, x, batch=2048):
    out = []
    for start in range(0, len(x), batch):
        out.append(forward(params, x[start:start + batch])[3])
    return np.concatenate(out)


def percentile_scale(values, pct=99.9):
    return float(np.percentile(values, pct)) or 1.0


def quantise(params, calibration, clip_pct=99.99):
    """Float weights to the int8 tensors and the two shifts of the file.

    Scales are chosen so the shift between layers is a power of two (the bootloader only
    shifts): `shift = floor(log2(scale_weights * scale_in / scale_out))`, then the
    weights take the largest scale that still fits int8 at that shift.
    """
    e = params["E"]
    s_e = 127.0 / np.abs(e).max()
    e_q = np.rint(e * s_e).astype(np.int64)
    feats, h1, h2, _ = forward(params, calibration)
    a1, a2 = 127.0 / percentile_scale(h1), 127.0 / percentile_scale(h2)

    def layer(weights, s_in, s_out):
        limit = 127.0 / np.percentile(np.abs(weights), clip_pct)
        shift = int(np.floor(np.log2(limit * s_in / s_out)))
        scale = s_out * 2.0 ** shift / s_in
        return np.clip(np.rint(weights * scale), -128, 127).astype(np.int64), shift

    w1_q, shift1 = layer(params["W1"], s_e, a1)
    w2_q, shift2 = layer(params["W2"], a1, a2)
    w3 = params["W3"]
    w3_q = np.clip(np.rint(w3 * (127.0 / np.abs(w3).max())), -128, 127).astype(np.int64)
    shape = mdl.Shape(calibration.shape[1], EMBEDDING, HIDDEN1, HIDDEN2, labels.HEADS, shift1, shift2)
    # file order is row-major with one row per output: the transpose of the float layout
    tensors = [e_q.reshape(-1), w1_q.T.reshape(-1), w2_q.T.reshape(-1), w3_q.T.reshape(-1)]
    return shape, [t.tolist() for t in tensors]


def int_forward(shape, tensors, x, batch=1024):
    """Exact integer scores of many windows. float64 holds every integer sum exactly (< 2**53)."""
    e = np.array(tensors[0], dtype=np.float64).reshape(256, shape.embedding)
    w1 = np.array(tensors[1], dtype=np.float64).reshape(shape.hidden1, shape.inputs()).T
    w2 = np.array(tensors[2], dtype=np.float64).reshape(shape.hidden2, shape.hidden1).T
    w3 = np.array(tensors[3], dtype=np.float64).reshape(shape.heads, shape.hidden2).T
    out = []
    for start in range(0, len(x), batch):
        xb = x[start:start + batch]
        feats = e[xb].reshape(len(xb), -1)
        h1 = np.clip(np.floor(feats @ w1 / 2.0 ** shape.shift1), 0, 127)
        h2 = np.clip(np.floor(h1 @ w2 / 2.0 ** shape.shift2), 0, 127)
        out.append((h2 @ w3).astype(np.int64))
    return np.concatenate(out)


def decode_all(scores):
    """Argmax of every head for many score rows; the lowest index wins a tie, as `labels.decode`."""
    cols = []
    for _, (start, end) in labels.slice_bounds().items():
        cols.append(np.argmax(scores[:, start:end], axis=1))
    return np.stack(cols, axis=1)
