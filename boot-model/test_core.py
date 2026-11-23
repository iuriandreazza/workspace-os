"""Unit tests for the deterministic parts of the boot model's host tooling (B2):
the window of a candidate, the labels, the `\\WSOS.MDL` file and its integer
forward pass, and the arithmetic of the evaluation harness. Standard library
only: the gate must not need numpy or the corpus."""
import sys
import unittest
import zlib
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import labels  # noqa: E402
import mdl  # noqa: E402
import evaluate  # noqa: E402
import metrics  # noqa: E402
import window  # noqa: E402


class Window(unittest.TestCase):
    def test_file_window_layout(self):
        w = window.file_window("\\boot\\vmlinuz", b"\x01\x02\x03")
        self.assertEqual(len(w), 512)
        self.assertEqual(w[:64], bytes(64 - 13) + b"\\boot\\vmlinuz")  # right-aligned
        self.assertEqual(w[64:67], b"\x01\x02\x03")
        self.assertEqual(w[67:], bytes(512 - 67))

    def test_long_path_keeps_the_tail(self):
        path = "\\" + "d" * 100 + "\\kernel.efi"
        name = window.file_window(path, b"")[:64]
        self.assertEqual(name, path.encode()[-64:])
        self.assertTrue(name.endswith(b"kernel.efi"))

    def test_content_is_cut_at_448_bytes(self):
        w = window.file_window("\\a", bytes(range(256)) * 4)
        self.assertEqual(w[64:], (bytes(range(256)) * 4)[:448])

    def test_path_must_start_with_a_backslash(self):
        with self.assertRaises(ValueError):
            window.file_window("boot\\vmlinuz", b"")

    def test_narrowing(self):
        self.assertEqual(window.narrow("aé中"), b"a??")
        self.assertEqual(window.narrow("\U0001F600"), b"??")  # two UCS-2 units

    def test_partition_window_layout(self):
        guid = bytes(range(16))
        w = window.partition_window("rootfs", guid, b"\xaa" * 1000)
        self.assertEqual(len(w), 512)
        self.assertEqual(w[:64], bytes(58) + b"rootfs")
        self.assertEqual(w[64:80], guid)
        self.assertEqual(w[80:], b"\xaa" * 432)

    def test_partition_name_is_at_most_36_units(self):
        w = window.partition_window("n" * 50, bytes(16), b"")
        self.assertEqual(w[:64], bytes(28) + b"n" * 36)

    def test_partition_name_limit_counts_ucs2_units_not_code_points(self):
        # an astral character is two UCS-2 units, so 36 units hold 18 of them
        w = window.partition_window("\U0001F600" * 36, bytes(16), b"")
        self.assertEqual(w[:64], bytes(28) + b"?" * 36)

    def test_partition_guid_length(self):
        with self.assertRaises(ValueError):
            window.partition_window("x", bytes(15), b"")


class Labels(unittest.TestCase):
    def test_head_layout(self):
        self.assertEqual(labels.HEADS, 8 + 4 + 6 + 16)
        self.assertEqual(labels.slice_bounds(),
                         {"family": (0, 8), "protocol": (8, 12), "role": (12, 18), "group": (18, 34)})

    def test_the_order_of_the_names_is_the_order_of_the_scores(self):
        # part of the \\WSOS.MDL contract with B3
        self.assertEqual(labels.FAMILIES[:4], ("none", "ours", "linux", "bsd"))
        self.assertEqual(labels.PROTOCOLS, ("none", "ours", "linux", "chainload"))
        self.assertEqual(labels.ROLES, ("other", "kernel", "initrd", "dtb", "loader", "entry"))
        self.assertEqual(labels.GROUPS, 16)
        self.assertEqual(labels.names_of("role"), labels.ROLES)

    def test_encode_checks_the_sets(self):
        self.assertEqual(labels.encode("linux", "chainload", "loader", 3), (2, 3, 4, 3))
        with self.assertRaises(ValueError):
            labels.encode("linux", "linux", "kernel", 16)
        with self.assertRaises(ValueError):
            labels.encode("windows", "linux", "kernel", 0)

    def test_decode_takes_each_argmax_and_the_lowest_index_on_a_tie(self):
        scores = [0] * 34
        scores[2] = 5            # family linux
        scores[8 + 1] = 7        # protocol ours
        scores[12 + 1] = 3       # role kernel
        scores[12 + 5] = 3       # tie with entry: the lowest index wins
        scores[18 + 15] = 1      # group 15
        self.assertEqual(labels.decode(scores), (2, 1, 1, 15))

    def test_decode_rejects_a_short_vector(self):
        with self.assertRaises(ValueError):
            labels.decode([0] * 33)


def tiny_shape():
    return mdl.Shape(window=2, embedding=2, hidden1=3, hidden2=2, heads=2, shift1=1, shift2=0)


def tiny_tensors():
    embedding = [0] * 512
    embedding[2 * 7:2 * 7 + 2] = [3, -2]     # byte 7
    embedding[2 * 9:2 * 9 + 2] = [4, 5]      # byte 9
    dense1 = [1, 1, 1, 1,   2, 0, 0, -1,   -1, -1, 0, 0]       # 3 x 4
    dense2 = [1, 1, 1,   0, 0, 2]                               # 2 x 3
    heads = [1, -1,   2, 3]                                     # 2 x 2
    return [embedding, dense1, dense2, heads]


class Mdl(unittest.TestCase):
    def test_round_trip(self):
        data = mdl.write(tiny_shape(), tiny_tensors())
        shape, tensors = mdl.read(data)
        self.assertEqual(shape, tiny_shape())
        self.assertEqual(tensors, tiny_tensors())

    def test_file_length_and_header(self):
        data = mdl.write(tiny_shape(), tiny_tensors())
        self.assertEqual(data[:4], b"WSMD")
        self.assertEqual(len(data), 40 + (512 + 12 + 6 + 4) + 4)
        self.assertEqual(int.from_bytes(data[-4:], "little"), zlib.crc32(data[:-4]))

    def test_negative_weights_survive(self):
        tensors = tiny_tensors()
        tensors[1][0] = -128
        tensors[1][1] = 127
        self.assertEqual(mdl.read(mdl.write(tiny_shape(), tensors))[1][1][:2], [-128, 127])

    def test_corruption_is_refused(self):
        data = bytearray(mdl.write(tiny_shape(), tiny_tensors()))
        data[60] ^= 1
        with self.assertRaisesRegex(ValueError, "CRC"):
            mdl.read(bytes(data))

    def test_truncation_bad_magic_and_version_are_refused(self):
        data = mdl.write(tiny_shape(), tiny_tensors())
        with self.assertRaises(ValueError):
            mdl.read(data[:-1])
        with self.assertRaises(ValueError):
            mdl.read(data + b"\0")
        with self.assertRaisesRegex(ValueError, "magic"):
            mdl.read(b"XXXX" + data[4:])
        bumped = bytearray(data)
        bumped[4] = 2
        with self.assertRaisesRegex(ValueError, "version"):
            mdl.read(bytes(bumped))

    def test_a_length_that_disagrees_with_the_shape_is_refused(self):
        data = bytearray(mdl.write(tiny_shape(), tiny_tensors()))
        data[36] += 1
        with self.assertRaisesRegex(ValueError, "payload length"):
            mdl.read(bytes(data))

    def test_a_header_that_disagrees_with_its_own_length_is_refused(self):
        # file length, payload length and CRC agree with each other; only the shape does not
        data = bytearray(mdl.write(tiny_shape(), tiny_tensors()))
        data[16] = 4  # hidden1 3 -> 4: the shape now needs more weights than the payload holds
        data[-4:] = zlib.crc32(bytes(data[:-4])).to_bytes(4, "little")
        with self.assertRaisesRegex(ValueError, "does not match the shape"):
            mdl.read(bytes(data))

    def test_write_refuses_a_weight_outside_int8_and_a_wrong_size(self):
        tensors = tiny_tensors()
        tensors[3][0] = 128
        with self.assertRaises(ValueError):
            mdl.write(tiny_shape(), tensors)
        tensors = tiny_tensors()
        tensors[2].append(0)
        with self.assertRaises(ValueError):
            mdl.write(tiny_shape(), tensors)

    def test_requantise_shifts_arithmetically_clamps_and_applies_relu(self):
        self.assertEqual(mdl.requantise([-5, -1, 0, 1, 3, 4, 1000, -1000], 1),
                         [0, 0, 0, 0, 1, 2, 127, 0])
        # an arithmetic shift rounds toward minus infinity: -1 >> 1 is -1, which the ReLU zeroes
        self.assertEqual(mdl.requantise([255, 256, 257], 1), [127, 127, 127])
        self.assertEqual(mdl.requantise([2 ** 20], 9), [127])
        self.assertEqual(mdl.requantise([1023, 1024], 9), [1, 2])

    def test_forward_by_hand(self):
        # window bytes 7 and 9 -> features [3, -2, 4, 5]
        # dense1 rows: 3-2+4+5 = 10; 6-5 = 1; -3+2 = -1   -> >>1 = 5, 0, -1 -> relu 5, 0, 0
        # dense2: 5+0+0 = 5; 0*5+0+0 = 0                  -> >>0 = 5, 0
        # heads: 5*1 + 0*-1 = 5; 5*2 + 0*3 = 10
        self.assertEqual(mdl.forward(tiny_shape(), tiny_tensors(), bytes([7, 9])), [5, 10])

    def test_scores_are_not_shifted_clamped_or_rectified(self):
        tensors = tiny_tensors()
        tensors[3] = [-1, 0,   100, 100]   # heads: -5 for the first, 100 * 5 = 500 for the second
        self.assertEqual(mdl.forward(tiny_shape(), tensors, bytes([7, 9])), [-5, 500])

    def test_forward_uses_the_byte_as_the_embedding_row(self):
        a = mdl.forward(tiny_shape(), tiny_tensors(), bytes([9, 7]))
        b = mdl.forward(tiny_shape(), tiny_tensors(), bytes([7, 9]))
        self.assertNotEqual(a, b)  # position matters: the layers are laid end to end

    def test_forward_checks_the_window_length(self):
        with self.assertRaises(ValueError):
            mdl.forward(tiny_shape(), tiny_tensors(), bytes(3))

    def test_the_reference_shape_is_the_one_the_probe_measured(self):
        shape = mdl.Shape(512, 32, 256, 128, 34, 9, 9)
        self.assertEqual(shape.weight_bytes(), 256 * 32 + 256 * 16384 + 128 * 256 + 34 * 128)
        self.assertEqual(shape.macs(), 256 * 16384 + 128 * 256 + 34 * 128)
        self.assertIsNone(mdl.within_budget(shape))

    def test_budget_limits_are_inclusive(self):
        shape = mdl.Shape(512, 32, 8, 8, 8, 9, 9)
        saved = mdl.MAX_WEIGHT_BYTES, mdl.MAX_MACS
        try:
            mdl.MAX_WEIGHT_BYTES, mdl.MAX_MACS = shape.weight_bytes(), shape.macs()
            self.assertIsNone(mdl.within_budget(shape))
            mdl.MAX_WEIGHT_BYTES = shape.weight_bytes() - 1
            self.assertIn("weight bytes", mdl.within_budget(shape))
            mdl.MAX_WEIGHT_BYTES, mdl.MAX_MACS = shape.weight_bytes(), shape.macs() - 1
            self.assertIn("multiply-accumulates", mdl.within_budget(shape))
        finally:
            mdl.MAX_WEIGHT_BYTES, mdl.MAX_MACS = saved

    def test_a_shape_whose_sums_can_overflow_32_bits_is_refused(self):
        big = mdl.Shape(512, 512, 8, 8, 34, 9, 9)  # inside both budgets, 262144 products in one sum
        self.assertLessEqual(big.macs(), mdl.MAX_MACS)
        self.assertIn("overflow", mdl.within_budget(big))
        self.assertIn("dense1", mdl.sum_overflow(big))
        edge = mdl.Shape(512, 8, 8, 8, 34, 9, 9)  # 4096 products
        self.assertIsNone(mdl.sum_overflow(edge))
        with self.assertRaisesRegex(ValueError, "overflow"):
            mdl.write(big, [[0] * n for n in big.sizes()])

    def test_the_overflow_limit_is_exactly_two_to_the_31(self):
        # 131071 products of 128 * 128 stay below 2**31, 131072 reach it
        below = mdl.Shape(1, 131071, 1, 1, 1, 0, 0)
        at = mdl.Shape(1, 131072, 1, 1, 1, 0, 0)
        self.assertIsNone(mdl.sum_overflow(below))
        self.assertIn("dense1", mdl.sum_overflow(at))

    def test_shifts_must_be_between_0_and_31(self):
        for bad in (-1, 32):
            shape = mdl.Shape(2, 2, 3, 2, 2, bad, 0)
            self.assertIn("shift", mdl.within_budget(shape))
            with self.assertRaises(ValueError):
                mdl.write(shape, [[0] * n for n in shape.sizes()])
        data = bytearray(mdl.write(tiny_shape(), tiny_tensors()))
        data[28] = 32  # shift1 in the header
        data[-4:] = zlib.crc32(bytes(data[:-4])).to_bytes(4, "little")
        with self.assertRaisesRegex(ValueError, "shift"):
            mdl.read(bytes(data))

    def test_budget(self):
        too_many_weights = mdl.Shape(512, 32, 1024, 128, 34, 9, 9)  # 16.8 million in dense1
        self.assertIn("weight bytes", mdl.within_budget(too_many_weights))
        too_many_macs = mdl.Shape(512, 32, 977, 1, 1, 9, 9)  # 977 * 16384 = 16.007 million MACs
        self.assertLessEqual(too_many_macs.weight_bytes(), mdl.MAX_WEIGHT_BYTES)
        self.assertIn("multiply-accumulates", mdl.within_budget(too_many_macs))
        edge = mdl.Shape(512, 32, 976, 1, 1, 9, 9)  # 976 * 16384 + 976 + 1 = 15.99 million
        self.assertIsNone(mdl.within_budget(edge))


class Metrics(unittest.TestCase):
    def test_accuracy(self):
        self.assertEqual(metrics.accuracy([1, 2, 3, 4], [1, 2, 0, 4]), 0.75)
        with self.assertRaises(ValueError):
            metrics.accuracy([], [])
        with self.assertRaises(ValueError):
            metrics.accuracy([1], [1, 2])

    def test_confusion_rows_are_the_truth(self):
        matrix = metrics.confusion([0, 0, 1, 2, 2], [0, 1, 1, 0, 2], 3)
        self.assertEqual(matrix, [[1, 1, 0], [0, 1, 0], [1, 0, 1]])

    def test_precision_recall(self):
        matrix = [[1, 1, 0], [0, 1, 0], [1, 0, 1], [0, 0, 0]]
        matrix = [row + [0] for row in matrix]
        pr = metrics.precision_recall(matrix)
        self.assertEqual(pr[0], (1 / 2, 1 / 2, 2))
        self.assertEqual(pr[1], (1 / 2, 1.0, 1))
        self.assertEqual(pr[2], (1.0, 1 / 2, 2))
        self.assertEqual(pr[3], (None, None, 0))

    def test_wilson_interval(self):
        low, high = metrics.wilson(50, 100)
        self.assertAlmostEqual(low, 0.4038, places=3)
        self.assertAlmostEqual(high, 0.5962, places=3)
        low, high = metrics.wilson(100, 100)
        self.assertAlmostEqual(high, 1.0, places=6)
        self.assertAlmostEqual(low, 0.9630, places=3)
        with self.assertRaises(ValueError):
            metrics.wilson(0, 0)

    def test_safety_counts(self):
        K, O = 1, 1  # role kernel, protocol ours
        truth_role = [0, 0, 1, 1, 1, 2]
        truth_protocol = [0, 0, 1, 1, 2, 2]
        pred_role = [1, 0, 1, 0, 1, 1]
        pred_protocol = [1, 0, 1, 1, 2, 2]
        # not kernels: indices 0, 1, 5; of them 0 and 5 are called kernel
        # ours: indices 2, 3 (role kernel, protocol ours); 2 is right, 3 has role 0
        self.assertEqual(metrics.safety_counts(truth_role, truth_protocol, pred_role, pred_protocol, K, O),
                         (2, 3, 1, 2))

    def test_only_a_kernel_with_the_ours_protocol_is_one_of_ours(self):
        # protocol "ours" on a file whose role is not kernel (a loader, say) is not our kernel
        self.assertEqual(metrics.safety_counts([0], [1], [0], [0], 1, 1), (0, 1, 0, 0))

    def test_an_ours_kernel_with_the_wrong_protocol_is_a_miss(self):
        self.assertEqual(metrics.safety_counts([1], [1], [1], [2], 1, 1), (0, 0, 1, 1))


def row(family=0, protocol=0, role=0, group=15):
    return (family, protocol, role, group)


KERNEL = labels.ROLES.index("kernel")
LOADER = labels.ROLES.index("loader")
INITRD = labels.ROLES.index("initrd")
OURS = labels.PROTOCOLS.index("ours")


class Evaluate(unittest.TestCase):
    def test_summarise_per_output(self):
        truth = [row(2, 2, KERNEL, 0), row(2, 2, INITRD, 0), row(0, 0, 0), row(0, 0, 0)]
        predicted = [row(2, 2, KERNEL, 0), row(2, 2, KERNEL, 1), row(0, 0, 0), row(2, 0, 0)]
        s = evaluate.summarise(truth, predicted)
        self.assertEqual(s["candidates"], 4)
        self.assertEqual(s["outputs"]["family"]["accuracy"], 0.75)
        self.assertEqual(s["outputs"]["protocol"]["accuracy"], 1.0)
        self.assertEqual(s["outputs"]["role"]["accuracy"], 0.75)
        self.assertEqual(s["outputs"]["group"]["accuracy"], 0.75)
        self.assertEqual(s["exact_decision"], 0.5)  # only candidates 0 and 2 match on all three
        self.assertEqual(s["outputs"]["role"]["confusion"][INITRD][KERNEL], 1)

    def test_safety_counts_in_summary(self):
        truth = [row(1, OURS, KERNEL), row(0, 0, 0), row(0, 0, 0)]
        predicted = [row(2, 2, KERNEL), row(0, 0, KERNEL), row(0, 0, 0)]
        s = evaluate.summarise(truth, predicted)["safety"]
        self.assertEqual(s, {"false_kernel": 1, "not_kernels": 2, "missed_ours": 1, "ours_kernels": 1})

    def test_disk_correct_needs_every_label_but_not_the_group(self):
        t = [row(2, 2, KERNEL, 0), row(0, 0, 0, 15)]
        self.assertTrue(evaluate.disk_correct(t, [row(2, 2, KERNEL, 7), row(0, 0, 0, 3)]))
        self.assertFalse(evaluate.disk_correct(t, [row(2, 2, KERNEL, 0), row(0, 0, 3, 15)]))

    def test_disk_safe(self):
        t = [row(0, 0, 0), row(1, OURS, KERNEL)]
        self.assertTrue(evaluate.disk_safe(t, [row(0, 0, 0), row(1, OURS, KERNEL)]))
        self.assertFalse(evaluate.disk_safe(t, [row(0, 0, KERNEL), row(1, OURS, KERNEL)]))   # a data file called a kernel
        self.assertFalse(evaluate.disk_safe(t, [row(0, 0, LOADER), row(1, OURS, KERNEL)]))   # a loader that is not one
        self.assertFalse(evaluate.disk_safe(t, [row(0, 0, 0), row(2, 2, KERNEL)]))           # ours, wrong protocol
        self.assertFalse(evaluate.disk_safe(t, [row(0, 0, 0), row(1, OURS, 0)]))             # ours missed
        # a kernel called an initrd is wrong, but it does not put a data file in the entry
        self.assertTrue(evaluate.disk_safe([row(2, 2, KERNEL)], [row(2, 2, 0)]))

    def test_disk_safe_refuses_an_invented_initrd(self):
        self.assertFalse(evaluate.disk_safe([row(0, 0, 0)], [row(2, 2, INITRD)]))

    def test_per_disk(self):
        good, bad = [row(0, 0, 0)], [row(0, 0, 0)]
        self.assertEqual(evaluate.per_disk([good, bad], [good, [row(0, 0, KERNEL)]]), (2, 1, 1))

    def test_tables(self):
        text = evaluate.confusion_table([[3, 1, 0], [0, 0, 0], [0, 0, 0]], ["a", "b", "c"])
        self.assertIn("| a | 3 | 1 |", text)
        self.assertNotIn("| c |", text)
        classes = evaluate.class_table([(0.5, 1.0, 2), (None, None, 0)], ["a", "b"])
        self.assertIn("| a | 2 | 50.0 % | 100.0 % |", classes)
        self.assertNotIn("| b |", classes)
        self.assertEqual(evaluate.format_percent(None), "n/a")


if __name__ == "__main__":
    unittest.main()
