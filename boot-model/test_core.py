"""Unit tests for the deterministic parts of the boot model's host tooling (B2):
the window of a candidate, the labels, the `\\WSOS.MDL` file and its integer
forward pass, and the arithmetic of the evaluation harness. Standard library
only: the gate must not need numpy or the corpus."""
import sys
import unittest
import zlib
from types import SimpleNamespace
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import assembly  # noqa: E402
import calibration  # noqa: E402
import contexts  # noqa: E402
import measure  # noqa: E402
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

    def test_read_refuses_a_header_whose_sums_can_overflow(self):
        # a small payload with a shape that makes dense1 sum 131072 products: the file would be untrusted input
        shape = mdl.Shape(131072, 1, 1, 1, 1, 0, 0)
        payload = bytes(sum(shape.sizes()))
        body = mdl.HEADER.pack(mdl.MAGIC, mdl.VERSION, 131072, 1, 1, 1, 1, 0, 0, len(payload)) + payload
        data = body + zlib.crc32(body).to_bytes(4, "little")
        with self.assertRaisesRegex(ValueError, "overflow"):
            mdl.read(data)

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


def cand(path, role, entry=None, text=None):
    return SimpleNamespace(path=path, role=role, entry=entry, text=text)


ENTRY_TEXT = (b"title Fedora\nversion 6.1.0\nlinux /abc/6.1.0/linux\ninitrd /abc/6.1.0/initrd\n"
              b"devicetree /dtb/board.dtb\noptions root=/dev/vda2 ro\n")


class Assembly(unittest.TestCase):
    def test_paths_compare_as_a_volume_does(self):
        self.assertEqual(assembly.normalise("/Boot/VMLINUZ"), "\\boot\\vmlinuz")
        self.assertEqual(assembly.normalise("\\boot\\x"), "\\boot\\x")
        self.assertEqual(assembly.directory("\\boot\\x"), "\\boot")
        self.assertEqual(assembly.directory("\\x"), "")

    def test_version_suffix(self):
        self.assertEqual(assembly.version_suffix("\\boot\\vmlinuz-6.1.0-18-arm64"), "6.1.0-18-arm64")
        self.assertEqual(assembly.version_suffix("\\boot\\initrd.img-6.1.0-18-arm64"), "6.1.0-18-arm64")
        self.assertEqual(assembly.version_suffix("\\boot\\initramfs-6.1.0-1.el9.aarch64.img"), "6.1.0-1.el9.aarch64")
        self.assertEqual(assembly.version_suffix("\\boot\\initramfs-virt"), "virt")
        self.assertEqual(assembly.version_suffix("\\x\\linux"), "")
        self.assertEqual(assembly.version_suffix("\\x\\initrd.img"), "")

    def test_entry_text_lines_are_parsed_and_junk_is_ignored(self):
        parsed = assembly.parse_entry_text(ENTRY_TEXT)
        self.assertEqual(parsed["linux"], "\\abc\\6.1.0\\linux")
        self.assertEqual(parsed["initrd"], ["\\abc\\6.1.0\\initrd"])
        self.assertEqual(parsed["devicetree"], ["\\dtb\\board.dtb"])
        self.assertEqual(assembly.parse_entry_text(b"\xff\xfe\x00 not an entry"), {"linux": None, "initrd": [], "devicetree": []})
        self.assertIsNone(assembly.parse_entry_text(b"# linux /a\nlinux relative\n")["linux"])

    def test_the_first_linux_line_wins_and_initrd_may_repeat(self):
        parsed = assembly.parse_entry_text(b"linux /a\nlinux /b\ninitrd /i1\ninitrd /i2\n")
        self.assertEqual((parsed["linux"], parsed["initrd"]), ("\\a", ["\\i1", "\\i2"]))

    def disk(self):
        return [cand("\\abc\\6.1.0\\linux", "kernel", "e1"), cand("\\abc\\6.1.0\\initrd", "initrd", "e1"),
                cand("\\dtb\\board.dtb", "dtb", "e1"),
                cand("\\loader\\entries\\abc-6.1.0.conf", "entry", "e1", ENTRY_TEXT),
                cand("\\boot\\vmlinuz-5.0", "kernel", "e2"), cand("\\boot\\initrd.img-5.0", "initrd", "e2"),
                cand("\\boot\\vmlinuz-4.0", "kernel", "e3"), cand("\\boot\\initrd.img-4.0", "initrd", "e3"),
                cand("\\docs\\readme.txt", "other")]

    def test_true_pairs_follow_the_entries(self):
        self.assertEqual(assembly.true_pairs(self.disk()), {(0, 1), (0, 2), (0, 3), (4, 5), (6, 7)})

    def test_same_directory_joins_two_kernels_of_one_directory(self):
        self.assertEqual(assembly.same_directory(assembly.files_of(self.disk())),
                         {(0, 1), (4, 5), (4, 7), (6, 5), (6, 7)})

    def test_the_suffix_separates_two_kernels_of_one_directory(self):
        self.assertEqual(assembly.directory_suffix(assembly.files_of(self.disk())), {(0, 1), (4, 5), (6, 7)})

    def test_entry_text_names_the_files_wherever_they_are(self):
        pairs, named = assembly.entry_text(assembly.files_of(self.disk()))
        self.assertEqual((pairs, named), ({(0, 1), (0, 2), (0, 3)}, {0}))

    def test_adr0034_uses_the_text_where_it_exists_and_the_fallback_elsewhere(self):
        disk = self.disk()
        self.assertEqual(assembly.adr0034(assembly.files_of(disk)), assembly.true_pairs(disk))

    def test_a_kernel_named_by_a_text_is_not_also_paired_by_directory(self):
        disk = self.disk()
        disk[1] = cand("\\abc\\6.1.0\\initrd", "initrd", "e1")
        disk.append(cand("\\abc\\6.1.0\\other.img", "initrd", "e9"))
        self.assertNotIn((0, len(disk) - 1), assembly.adr0034(assembly.files_of(disk)))

    def test_an_entry_text_that_names_a_missing_file_asserts_nothing(self):
        disk = [cand("\\x\\linux", "kernel", "e"), cand("\\y.conf", "entry", "e", b"linux /nope\n")]
        self.assertEqual(assembly.entry_text(assembly.files_of(disk))[0], set())

    def test_the_text_is_read_in_full_not_cut_at_the_window(self):
        long_line = b"title " + b"x" * 600 + b"\nlinux /k\ninitrd /i\n"
        disk = [cand("\\k", "kernel", "e"), cand("\\i", "initrd", "e"), cand("\\e.conf", "entry", "e", long_line)]
        self.assertEqual(assembly.entry_text(assembly.files_of(disk))[0], {(0, 1), (0, 2)})

    def test_predicted_roles_decide_what_the_rule_sees(self):
        disk = self.disk()
        wrong = [c.role for c in disk]
        wrong[1] = "other"
        got = assembly.adr0034(assembly.files_of(disk, wrong))
        self.assertNotIn((0, 1), got)
        self.assertIn((0, 2), got)

    def test_score_counts_pairs_and_disks(self):
        disks = [self.disk(), [cand("\\a", "kernel", "e"), cand("\\b", "initrd", "e")], [cand("\\a", "other")]]
        perfect = assembly.score(disks, assembly.adr0034)
        self.assertEqual((perfect["precision"], perfect["recall"]), (1.0, 1.0))
        self.assertEqual((perfect["disks_with_pairs"], perfect["disks_all_right"]), (2, 2))
        loose = assembly.score(disks, assembly.same_directory)
        self.assertEqual(loose["pairs_asserted"], 6)
        self.assertLess(loose["precision"], 1.0)
        self.assertEqual(loose["disks_all_right"], 1)

    def test_a_disk_with_an_extra_pair_is_not_right(self):
        disk = [cand("\\boot\\k", "kernel", "e1"), cand("\\boot\\i", "initrd", "e1"), cand("\\boot\\j", "initrd", "e2")]
        result = assembly.score([disk], assembly.same_directory)
        self.assertEqual((result["pairs_true"], result["pairs_asserted"], result["disks_all_right"]), (1, 2, 0))

    def test_missed_and_wrong_pairs_are_counted_by_the_role_of_the_companion(self):
        result = assembly.score([self.disk()], assembly.directory_suffix)
        self.assertEqual(result["missed_by_role"], {"dtb": 1, "entry": 1})
        self.assertEqual(result["wrong_by_role"], {})
        loose = assembly.score([self.disk()], assembly.same_directory)
        self.assertEqual(loose["wrong_by_role"], {"initrd": 2})

    def test_a_pair_asserted_on_a_disk_without_pairs_is_counted(self):
        disk = [cand("\\boot\\k", "kernel", None), cand("\\boot\\i", "initrd", None)]
        result = assembly.score([disk], assembly.same_directory)
        self.assertEqual((result["disks_with_pairs"], result["spurious_on_pairless"], result["recall"]), (0, 1, None))


class Contexts(unittest.TestCase):
    def disk(self):
        mk = lambda path, size=0: SimpleNamespace(path=path, window=window.file_window(path, b"x") if path else bytes(512), size=size)
        return [mk("\\boot\\vmlinuz-6.1", 1 << 23), mk("\\boot\\initrd.img-6.1", 1 << 24), mk("\\boot\\config-6.1"),
                mk("\\EFI\\BOOT\\BOOTAA64.EFI"), mk("")]

    def test_size_bucket_is_the_bit_length_capped(self):
        self.assertEqual([contexts.size_bucket(s) for s in (0, 1, 2, 255, 256, 1 << 23, 1 << 40, -5)], [0, 1, 2, 8, 9, 24, 31, 0])

    def test_the_siblings_are_the_other_files_of_the_directory_sorted(self):
        slots = contexts.sibling_slots(self.disk())
        self.assertEqual(len(slots[0]), contexts.SIBLING_SLOT)
        names = [slots[0][i:i + 32].lstrip(b"\0") for i in range(0, 64, 32)]
        self.assertEqual(names, [b"config-6.1", b"initrd.img-6.1"])
        self.assertEqual(slots[0][64:], bytes(contexts.SIBLING_SLOT - 64))

    def test_a_file_alone_in_its_directory_and_a_partition_have_no_siblings(self):
        slots = contexts.sibling_slots(self.disk())
        self.assertEqual(slots[3], bytes(contexts.SIBLING_SLOT))
        self.assertEqual(slots[4], bytes(contexts.SIBLING_SLOT))

    def test_directories_compare_without_case_and_names_are_right_aligned(self):
        mk = lambda path: SimpleNamespace(path=path, window=bytes(512), size=0)
        slots = contexts.sibling_slots([mk("\\Boot\\A"), mk("\\BOOT\\b")])
        self.assertEqual(slots[0][:32], bytes(31) + b"b")

    def test_more_than_sixteen_siblings_are_cut(self):
        mk = lambda n: SimpleNamespace(path=f"\\d\\f{n:02d}", window=bytes(512), size=0)
        slots = contexts.sibling_slots([mk(n) for n in range(20)])
        self.assertEqual(len(slots[19]), contexts.SIBLING_SLOT)
        self.assertEqual(slots[19][-32:].lstrip(b"\0"), b"f15")

    def test_extended_windows_append_in_the_order_size_then_neighbours(self):
        disk = self.disk()
        plain = contexts.extended_windows(disk)
        self.assertEqual(plain, [c.window for c in disk])
        both = contexts.extended_windows(disk, size=True, neighbours=True)
        self.assertEqual(len(both[0]), 512 + 1 + contexts.SIBLING_SLOT)
        self.assertEqual(both[0][512], contexts.size_bucket(1 << 23))
        self.assertEqual(both[0][513:], contexts.sibling_slots(disk)[0])
        self.assertEqual(len(contexts.extended_windows(disk, size=True)[0]), 513)


class Calibration(unittest.TestCase):
    def test_margin_is_the_gap_between_the_two_best(self):
        self.assertEqual(calibration.margin([3, 10, 7, -2]), 3)
        self.assertEqual(calibration.margin([5, 5, 1]), 0)

    def test_softmax_sums_to_one_and_a_high_temperature_flattens_it(self):
        sharp, flat = calibration.softmax([10, 0, 0], 1.0), calibration.softmax([10, 0, 0], 100.0)
        self.assertAlmostEqual(sum(sharp), 1.0)
        self.assertGreater(max(sharp), max(flat))
        self.assertAlmostEqual(calibration.confidence([1, 1]), 0.5)

    def test_the_fitted_temperature_softens_an_overconfident_head(self):
        rows = [[20, 0], [20, 0], [20, 0], [20, 0]]
        truth = [0, 0, 0, 1]  # right 3 times in 4, but the scores claim certainty
        self.assertGreater(calibration.fit_temperature(rows, truth), 4)
        self.assertLess(calibration.fit_temperature([[2, 0]] * 4, [0] * 4), 1)

    def test_expected_calibration_error(self):
        self.assertAlmostEqual(calibration.expected_calibration_error([0.9] * 10, [1] * 9 + [0]), 0.0)
        self.assertAlmostEqual(calibration.expected_calibration_error([1.0] * 10, [1] * 5 + [0] * 5), 0.5)
        with self.assertRaises(ValueError):
            calibration.expected_calibration_error([], [])

    def test_risk_coverage(self):
        curve = calibration.risk_coverage([0.9, 0.8, 0.6, 0.5], [1, 1, 0, 0], [0.0, 0.7, 0.95])
        self.assertEqual(curve, [(0.0, 1.0, 0.5), (0.7, 0.5, 0.0), (0.95, 0.0, None)])
        # a confidence equal to the threshold is kept
        self.assertEqual(calibration.risk_coverage([0.5, 0.5], [1, 0], [0.5]), [(0.5, 1.0, 0.5)])

    def test_trade_off_counts_what_each_threshold_removes_and_loses(self):
        margins = [10, 2, 9, 1, 5]
        truth_kernel = [True, True, False, False, False]
        predicted = [True, True, True, True, False]
        rows = calibration.trade_off(margins, truth_kernel, predicted, [0, 3, 10])
        self.assertEqual(rows, [(0, 2, 0, 2, 0), (3, 1, 1, 1, 1), (10, 0, 2, 1, 1)])
        # a margin equal to the threshold stays, for a false kernel as for a real one
        self.assertEqual(calibration.trade_off([9, 9], [False, True], [True, True], [9]), [(9, 1, 0, 1, 0)])


class Measure(unittest.TestCase):
    def test_a_file_counts_once_however_many_disks_it_is_on(self):
        truth = [(2, 2, 1, 0)] * 4 + [(0, 0, 0, 15)]
        predicted = [(2, 2, 1, 0)] * 3 + [(2, 2, 0, 0)] + [(0, 0, 0, 15)]
        groups = [("debian", "1")] * 5
        out = measure.summarise(truth, predicted, ["k"] * 4 + ["o"], groups)
        self.assertEqual(out["candidates"]["kernel_recall"], 0.75)
        self.assertEqual(out["files"]["units"], 2)
        self.assertEqual(out["files"]["kernel_recall"], 0.75)
        self.assertEqual(out["files"]["all_three"], (0.75 + 1) / 2)

    def test_the_rare_file_weighs_as_much_as_the_common_one(self):
        truth = [(2, 2, 1, 0)] * 9 + [(2, 2, 1, 0)]
        predicted = [(2, 2, 1, 0)] * 9 + [(2, 2, 0, 0)]
        out = measure.summarise(truth, predicted, ["a"] * 9 + ["b"], [("s", "r")] * 10)
        self.assertAlmostEqual(out["candidates"]["kernel_recall"], 0.9)
        self.assertAlmostEqual(out["files"]["kernel_recall"], 0.5)

    def test_false_kernels_are_counted_on_files_that_are_not_kernels(self):
        truth = [(0, 0, 0, 15), (0, 0, 0, 15), (0, 0, 0, 15), (2, 2, 1, 0)]
        predicted = [(2, 2, 1, 0), (0, 0, 0, 15), (0, 0, 0, 15), (2, 2, 1, 0)]
        out = measure.summarise(truth, predicted, ["a", "b", "c", "k"], [("junk", "x")] * 4)
        self.assertEqual((out["files"]["false_kernel"], out["files"]["not_kernel_units"]), (1 / 3, 3))

    def test_a_class_with_no_file_has_no_recall(self):
        out = measure.summarise([(0, 0, 0, 15)], [(0, 0, 0, 15)], ["a"], [("s", "r")])
        self.assertIsNone(out["files"]["initrd_recall"])
        self.assertEqual(out["files"]["initrd_units"], 0)

    def test_releases_are_reported_apart(self):
        truth = [(2, 2, 1, 0), (2, 2, 1, 0)]
        predicted = [(2, 2, 1, 0), (2, 2, 0, 0)]
        out = measure.summarise(truth, predicted, ["a", "b"], [("debian", "1"), ("debian", "2")])
        self.assertEqual((out["releases"]["debian/1"]["kernel_recall"], out["releases"]["debian/2"]["kernel_recall"]), (1.0, 0.0))


if __name__ == "__main__":
    unittest.main()
