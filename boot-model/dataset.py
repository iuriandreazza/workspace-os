"""Turns generated disks into arrays, and keeps the corpus honest about duplicates (B2)."""
import hashlib

import numpy as np


def arrays(disks):
    flat = [c for disk in disks for c in disk]
    x = np.frombuffer(b"".join(c.window for c in flat), dtype=np.uint8).reshape(len(flat), -1)
    y = np.array([c.indices() for c in flat], dtype=np.int64)
    return flat, x, y


def window_hashes(x):
    return [hashlib.sha256(row.tobytes()).hexdigest() for row in x]


def novel_mask(train_x, test_x):
    """True for the test windows that no training window equals byte for byte."""
    seen = set(window_hashes(train_x))
    return np.array([h not in seen for h in window_hashes(test_x)])


def content_only(x):
    """The window with its name zeroed: what the bytes alone carry."""
    out = x.copy()
    out[:, :64] = 0
    return out


def name_only(x):
    """The window with its content zeroed: what the path alone carries."""
    out = x.copy()
    out[:, 64:] = 0
    return out
