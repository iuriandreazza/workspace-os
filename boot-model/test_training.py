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

    def test_extra_inputs_after_the_window_are_left_alone_by_the_augmentations(self):
        import training
        rng = np.random.default_rng(3)
        x = rng.integers(1, 256, (50, 512 + 33), dtype=np.uint8)
        out = training.fresh_random_tails(x, rng, 1.0)
        self.assertTrue((out[:, 512:] == x[:, 512:]).all())
        self.assertTrue((out[:, training.HEADER_END:512] != x[:, training.HEADER_END:512]).any())
        dropped = training.modality_dropout(x, rng, 1.0, 0.0)
        self.assertTrue((dropped[:, 512:] == x[:, 512:]).all() and (dropped[:, 64:512] == 0).all())

    def test_a_longer_window_trains_and_quantises_to_a_shape_that_says_so(self):
        import training
        rng = np.random.default_rng(1)
        x = rng.integers(0, 256, (256, 513), dtype=np.uint8)
        y = np.stack([rng.integers(0, n, 256) for _, n in labels.SLICES], axis=1)
        params = training.train(x, y, 1, epochs=1, log=lambda m: None)
        self.assertEqual(params["W1"].shape[0], 513 * training.EMBEDDING)
        shape, tensors = training.quantise(params, x)
        self.assertEqual(shape.window, 513)
        self.assertEqual(len(tensors[1]), shape.hidden1 * 513 * training.EMBEDDING)

    def numeric_gradient(self, logits, y, outliers, weight):
        import training
        grad = np.zeros_like(logits)
        for index in np.ndindex(*logits.shape):
            up, down = logits.copy(), logits.copy()
            up[index] += 1e-5
            down[index] -= 1e-5
            grad[index] = (training.loss_and_grad_logits(up, y, (1, 1, 1, 0.5), outliers, weight)[0]
                           - training.loss_and_grad_logits(down, y, (1, 1, 1, 0.5), outliers, weight)[0]) / 2e-5
        return grad

    def test_the_gradient_of_the_loss_matches_finite_differences_with_and_without_outliers(self):
        import training
        rng = np.random.default_rng(2)
        logits = rng.normal(0, 1, (6, labels.HEADS))
        y = np.stack([rng.integers(0, n, 6) for _, n in labels.SLICES], axis=1)
        for outliers, weight in ((None, 0.0), (np.array([False, False, False, False, True, True]), 0.7)):
            _, grad = training.loss_and_grad_logits(logits, y, (1, 1, 1, 0.5), outliers, weight)
            self.assertLess(np.abs(grad - self.numeric_gradient(logits, y, outliers, weight)).max(), 1e-6)

    def test_an_outlier_row_ignores_its_labels_and_leaves_the_group_head_alone(self):
        import training
        rng = np.random.default_rng(2)
        logits = rng.normal(0, 1, (4, labels.HEADS))
        y = np.stack([rng.integers(0, n, 4) for _, n in labels.SLICES], axis=1)
        flag = np.array([False, False, True, True])
        _, first = training.loss_and_grad_logits(logits, y, (1, 1, 1, 1), flag, 1.0)
        y2 = y.copy()
        y2[2:] = 0
        _, second = training.loss_and_grad_logits(logits, y2, (1, 1, 1, 1), flag, 1.0)
        self.assertTrue((first == second).all())
        start, end = labels.slice_bounds()["group"]
        self.assertTrue((first[2:, start:end] == 0).all())

    def test_uniform_scores_are_the_minimum_of_the_outlier_loss(self):
        import training
        logits = np.zeros((2, labels.HEADS))
        flag = np.array([True, True])
        _, grad = training.loss_and_grad_logits(logits, np.zeros((2, 4), dtype=np.int64), (1, 1, 1, 1), flag, 1.0)
        self.assertTrue((grad == 0).all())

    def test_the_run_builds_windows_with_the_extra_inputs_from_generated_disks(self):
        import b2b
        import test_corpus
        disks = b2b.layouts.generate(test_corpus.entry_assets(), test_corpus.junk(), "train", 20, 1)
        flat, plain, y = b2b.disks_to_arrays(disks)
        _, both, _ = b2b.disks_to_arrays(disks, size=True, neighbours=True)
        self.assertEqual((plain.shape[1], both.shape[1]), (512, 512 + 1 + 512))
        self.assertTrue((both[:, :512] == plain).all())
        self.assertEqual(len(flat), len(y))

    def fold_assets(self):
        import test_corpus
        return test_corpus.entry_assets("train", "1.0") + test_corpus.entry_assets("validation", "2.0")

    def test_a_fold_never_trains_on_the_family_it_holds_out(self):
        import b2b
        for fold, held in (("el", {"fedora", "alma", "rocky"}), ("debian", {"debian"}), ("alpine", {"alpine"})):
            disks = b2b.train_disks(fold, {}, 0, self.fold_assets(), 80)
            seen = {c.system for disk in disks for c in disk}
            self.assertFalse(seen & held, (fold, seen & held))
            self.assertTrue(seen - held - {"junk", "partition"})

    def test_a_folds_evaluation_disks_hold_only_its_family_and_ignore_the_seed(self):
        import b2b
        assets = self.fold_assets()
        first = b2b.eval_disks("el", assets)["heldout"]
        again = b2b.eval_disks("el", assets)["heldout"]
        self.assertEqual([[c.window for c in d] for d in first], [[c.window for c in d] for d in again])
        systems = {c.system for disk in first for c in disk if c.role in ("kernel", "initrd", "entry")}
        self.assertTrue(systems <= {"fedora", "alma", "rocky"}, systems)
        self.assertTrue(systems)

    def test_the_gain_rule_wants_apart_ranges_and_a_point(self):
        import b2b_report
        verdict = b2b_report.verdict
        self.assertEqual(verdict([0.5, 0.52, 0.54], [0.6, 0.62, 0.64], True), "gain")
        self.assertEqual(verdict([0.5, 0.52, 0.54], [0.53, 0.62, 0.64], True), "within spread")  # ranges overlap
        self.assertEqual(verdict([0.5, 0.52, 0.54], [0.4, 0.42, 0.44], True), "loss")
        self.assertEqual(verdict([0.10, 0.11, 0.12], [0.02, 0.03, 0.04], False), "gain")  # lower is better
        self.assertEqual(verdict([0.0, 0.0, 0.0], [0.002, 0.003, 0.004], False), "within spread")  # under a point
        self.assertEqual(verdict([], [0.1], True), "n/a")


if __name__ == "__main__":
    unittest.main()
