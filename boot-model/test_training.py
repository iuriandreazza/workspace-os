"""Tests of the numpy side of the boot model tooling (B2): the integer forward pass must equal
the pure Python reference in `mdl.py` bit for bit, and quantisation must keep the file valid.
Skipped where numpy is not installed (the gate has none); run with
`uv run --no-project --with numpy==2.5.5 python boot-model/test_training.py`."""
import random
import sys
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

try:
    import numpy as np
except ImportError:  # the gate runs with the standard library only
    np = None

import labels  # noqa: E402
import mdl  # noqa: E402


@unittest.skipIf(np is None, "numpy not installed")
class IntegerPath(unittest.TestCase):
    def tiny(self, seed):
        import training
        rng = random.Random(seed)
        shape = mdl.Shape(8, 4, 6, 5, labels.HEADS, 3, 2)
        # small weights keep the activations below the clamp, so floor against round shows
        spans = (4, 4, 8, 128)
        tensors = [[rng.randrange(-span, span) for _ in range(size)] for span, size in zip(spans, shape.sizes())]
        return training, shape, tensors, rng

    def test_numpy_integer_path_equals_the_pure_python_reference(self):
        for seed in range(5):
            training, shape, tensors, rng = self.tiny(seed)
            windows = [bytes(rng.randrange(256) for _ in range(shape.window)) for _ in range(6)]
            expected = [mdl.forward(shape, tensors, w) for w in windows]
            got = training.int_forward(shape, tensors, np.frombuffer(b"".join(windows), dtype=np.uint8).reshape(6, -1))
            self.assertEqual([list(map(int, row)) for row in got], expected)

    def test_decode_all_matches_labels_decode_including_ties(self):
        training, _, _, rng = self.tiny(1)
        scores = [[rng.choice((0, 0, 1, 5)) for _ in range(labels.HEADS)] for _ in range(50)]
        got = training.decode_all(np.array(scores))
        self.assertEqual([tuple(map(int, row)) for row in got], [labels.decode(s) for s in scores])

    def test_quantise_writes_a_valid_file_whose_shape_fits_the_budget(self):
        import training
        rng = np.random.default_rng(0)
        params = training.initial_params(rng)
        x = rng.integers(0, 256, (64, training.WINDOW), dtype=np.uint8)
        shape, tensors = training.quantise(params, x)
        self.assertIsNone(mdl.within_budget(shape))
        shape2, tensors2 = mdl.read(mdl.write(shape, tensors))
        self.assertEqual((shape2, tensors2), (shape, tensors))
        self.assertTrue(all(-128 <= v <= 127 for t in tensors for v in t[:1000]))
        # the integer scores follow the float scores: a transposed or mis-scaled tensor would not
        _, _, _, logits = training.forward(params, x)
        scores = training.int_forward(shape, tensors, x)
        corr = np.corrcoef(logits.reshape(-1), (scores / max(1, np.abs(scores).max())).reshape(-1))[0, 1]
        self.assertGreater(corr, 0.5)
        # file layout: one row per output, so row r of dense1 holds column r of the float matrix
        inputs = shape.inputs()
        flat = np.array(tensors[1]).reshape(shape.hidden1, inputs)
        self.assertTrue((np.sign(flat[:8, :200]) * np.sign(params["W1"][:200, :8].T) >= 0).all())

    def test_modality_dropout_and_fresh_tails_touch_only_what_they_say(self):
        import training
        rng = np.random.default_rng(3)
        x = rng.integers(1, 256, (200, 512), dtype=np.uint8)
        out = training.fresh_random_tails(x, rng, 1.0)
        self.assertTrue((out[:, :training.HEADER_END] == x[:, :training.HEADER_END]).all())
        self.assertTrue((out[:, training.HEADER_END:] != x[:, training.HEADER_END:]).any())
        low = np.zeros_like(x)
        self.assertTrue((training.fresh_random_tails(low, rng, 1.0) == low).all())  # low entropy is left alone
        dropped = training.modality_dropout(x, rng, 1.0, 0.0)
        self.assertTrue((dropped[:, 64:] == 0).all() and (dropped[:, :64] == x[:, :64]).all())


if __name__ == "__main__":
    unittest.main()
