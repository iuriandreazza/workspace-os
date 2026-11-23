"""Unit tests for the corpus side of the boot model tooling (B2): labelling, the split,
the layouts and their groups, the generated files, the checksum parser and the cache
check. No network, no cache, no numpy: assets are built in memory."""
import random
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import catalogue  # noqa: E402
import fetch  # noqa: E402
import labels  # noqa: E402
import layouts  # noqa: E402
import synthetic  # noqa: E402
import window  # noqa: E402

HEX = "a" * 64


class Classify(unittest.TestCase):
    def check(self, system, name, expected):
        self.assertEqual(catalogue.classify(system, name), expected, f"{system}/{name}")

    def test_linux_kernels_and_initrds(self):
        self.check("alpine", "vmlinuz-virt", ("linux", "linux", "kernel"))
        self.check("alpine", "initramfs-lts", ("linux", "linux", "initrd"))
        self.check("debian", "linux", ("linux", "linux", "kernel"))
        self.check("debian", "initrd.gz", ("linux", "linux", "initrd"))
        self.check("fedora", "vmlinuz", ("linux", "linux", "kernel"))
        self.check("opensuse", "initrd", ("linux", "linux", "initrd"))

    def test_rebuilds_use_the_fedora_layout(self):
        self.check("rocky", "vmlinuz", ("linux", "linux", "kernel"))
        self.check("alma", "BOOTAA64.EFI", ("linux", "chainload", "loader"))

    def test_loaders_chain_load_and_other_efi_programs_are_not_entries(self):
        self.check("debian", "grubaa64.efi", ("linux", "chainload", "loader"))
        self.check("fedora", "grubaa64.efi", ("linux", "chainload", "loader"))
        self.check("fedora", "mmaa64.efi", ("none", "none", "other"))

    def test_bsd_kernel_is_a_kernel_the_loader_cannot_start(self):
        self.check("openbsd", "bsd.rd", ("bsd", "none", "kernel"))
        self.check("openbsd", "BOOTAA64.EFI", ("bsd", "chainload", "loader"))

    def test_device_trees_and_the_rest(self):
        self.check("alpine", "foo.dtb", ("linux", "linux", "dtb"))
        self.check("alpine", "config-virt", ("none", "none", "other"))
        self.check("alpine", "System.map-virt", ("none", "none", "other"))
        self.check("alpine", "modloop-virt", ("none", "none", "other"))
        self.check("debian", "grub.cfg", ("none", "none", "other"))
        # a name that only looks like a kernel in the wrong system is not one
        self.check("fedora", "linux", ("none", "none", "other"))

    def test_every_label_is_in_its_closed_set(self):
        for system in ("alpine", "debian", "fedora", "opensuse", "openbsd", "rocky", "alma", "unknown"):
            for name in ("vmlinuz", "linux", "initrd", "initrd.img", "bsd", "BOOTAA64.EFI", "x.dtb", "readme"):
                labels.encode(*catalogue.classify(system, name), 0)


class Refine(unittest.TestCase):
    def test_a_linux_kernel_that_is_not_a_pe_file_has_no_protocol(self):
        linux = ("linux", "linux", "kernel")
        self.assertEqual(catalogue.refine(linux, b"MZ\0\0zimg"), linux)
        self.assertEqual(catalogue.refine(linux, b"\x1f\x8b\x08\x00"), ("linux", "none", "kernel"))
        self.assertEqual(catalogue.refine(linux, b""), ("linux", "none", "kernel"))

    def test_only_linux_kernels_are_refined(self):
        for labels_ in (("linux", "linux", "initrd"), ("bsd", "none", "kernel"), ("ours", "ours", "kernel"),
                        ("none", "none", "other"), ("linux", "chainload", "loader")):
            self.assertEqual(catalogue.refine(labels_, b"\x1f\x8b"), labels_)

class Split(unittest.TestCase):
    def test_releases_fall_in_the_split_that_names_them(self):
        self.assertEqual(catalogue.split_of("alpine", "3.24.4"), "validation")
        self.assertEqual(catalogue.split_of("alpine", "3.18.12"), "test")
        self.assertEqual(catalogue.split_of("alpine", "3.22.8"), "train")
        self.assertEqual(catalogue.split_of("rocky", "9.4"), "validation")
        self.assertEqual(catalogue.split_of("rocky", "9.3"), "test")
        self.assertEqual(catalogue.split_of("alma", "9.4"), "train")
        self.assertEqual(catalogue.split_of("somethingnew", "1"), "train")

    def test_no_release_is_in_two_splits(self):
        for system, releases in catalogue.VALIDATION_RELEASES.items():
            self.assertFalse(releases & catalogue.TEST_RELEASES.get(system, set()), system)

    def test_every_held_out_release_resolves_to_its_split(self):
        for system, releases in catalogue.VALIDATION_RELEASES.items():
            for release in releases:
                self.assertEqual(catalogue.split_of(system, release), "validation")
        for system, releases in catalogue.TEST_RELEASES.items():
            for release in releases:
                self.assertEqual(catalogue.split_of(system, release), "test")

    def test_pinned_corpus_has_each_split_and_no_test_release_is_missing(self):
        import json
        pins = json.loads(fetch.PINS.read_text()) + json.loads((Path(__file__).parent / "corpus" / "ours-pins.json").read_text())
        seen = {(p["system"], p["release"]) for p in pins}
        for system, releases in {**catalogue.TEST_RELEASES}.items():
            for release in releases:
                self.assertIn((system, release), seen, "a release named as held-out has no pin")


def asset(system, release, name, content=b"", split="train", role=None):
    labels_ = catalogue.classify(system, name)
    if role:
        labels_ = (labels_[0], labels_[1], role)
    return catalogue.Asset(system, release, name, content, *labels_, split)


def tiny_assets(split="train", release="1.0"):
    return [
        asset("alpine", release, "vmlinuz-virt", b"KERN" * 200, split),
        asset("alpine", release, "initramfs-virt", b"INIT" * 200, split),
        asset("debian", release, "linux", b"DKERN" * 100, split),
        asset("debian", release, "initrd.gz", b"DINIT" * 100, split),
        asset("debian", release, "grubaa64.efi", b"MZgrub", split),
        catalogue.Asset("ours", release + "-kernel", "kernel", b"\x7fELF" + bytes(60), "ours", "ours", "kernel", split),
        catalogue.Asset("ours", release + "-bootloader", "bootloader.efi", b"MZours", "ours", "none", "other", split),
        asset("openbsd", release, "bsd", b"\x7fELFbsd", split),
        asset("openbsd", release, "BOOTAA64.EFI", b"MZbsd", split),
    ]


def junk(count=20):
    return synthetic.junk_assets(count, 7)


class Layouts(unittest.TestCase):
    def disks(self, count=60, split="train", seed=3, mode="normal", assets=None):
        return layouts.generate(assets or tiny_assets(split), junk(), split, count, seed, mode)

    def test_deterministic(self):
        a, b = self.disks(), self.disks()
        self.assertEqual([[c.window for c in d] for d in a], [[c.window for c in d] for d in b])
        self.assertNotEqual([[c.window for c in d] for d in a], [[c.window for c in d] for d in self.disks(seed=4)])

    def test_windows_follow_the_contract(self):
        for disk in self.disks():
            for c in disk:
                self.assertEqual(len(c.window), window.WINDOW)
                if c.system != "partition":
                    self.assertTrue(c.window[:64].rstrip(b"\0").endswith(c.name.encode()), c.name)
                    self.assertIn(b"\\", c.window[:64].rstrip(b"\0") if len(c.name) < 63 else b"\\")

    def test_content_comes_from_the_asset_and_labels_follow_it(self):
        seen = set()
        for disk in self.disks(200):
            for c in disk:
                if c.system == "alpine" and c.role == "kernel":
                    self.assertEqual(c.window[64:64 + 8], b"KERNKERN")
                    self.assertEqual((c.family, c.protocol), ("linux", "linux"))
                    seen.add("kernel")
                if c.system == "ours" and c.role == "kernel":
                    self.assertEqual(c.protocol, "ours")
                    seen.add("ours")
                if c.system == "openbsd" and c.role == "kernel":
                    self.assertEqual((c.family, c.protocol), ("bsd", "none"))
                    seen.add("bsd")
        self.assertEqual(seen, {"kernel", "ours", "bsd"})

    def test_groups_number_the_entries_and_everything_else_is_no_entry(self):
        for disk in self.disks(200):
            by_entry = {}
            for c in disk:
                if c.entry is None:
                    self.assertEqual(c.group, layouts.NO_ENTRY)
                else:
                    self.assertLess(c.group, layouts.NO_ENTRY)
                    by_entry.setdefault(c.entry, set()).add(c.group)
            self.assertTrue(all(len(g) == 1 for g in by_entry.values()))
            slots = [next(iter(g)) for g in by_entry.values()]
            self.assertEqual(len(slots), len(set(slots)), "two entries in one slot")

    def test_group_is_not_a_function_of_the_window(self):
        # the same file under the same name lands in different slots on different disks
        slots = {}
        for disk in self.disks(300):
            for c in disk:
                if c.entry is not None:
                    slots.setdefault(c.window, set()).add(c.group)
        self.assertTrue(any(len(v) > 1 for v in slots.values()))

    def test_a_split_only_uses_its_own_assets(self):
        assets = tiny_assets("train", "1.0") + tiny_assets("test", "2.0")
        for disk in layouts.generate(assets, junk(), "test", 50, 1):
            for c in disk:
                self.assertNotEqual(c.release, "1.0")
                self.assertNotEqual(c.release, "1.0-kernel")
        for disk in layouts.generate(assets, junk(), "train", 50, 1):
            for c in disk:
                self.assertNotEqual(c.release, "2.0")

    def test_renamed_mode_keeps_labels_and_drops_the_names(self):
        for disk in self.disks(80, mode="renamed"):
            for c in disk:
                if c.entry is not None:
                    self.assertNotIn(c.name.lower().split(".")[0], ("vmlinuz-virt", "kernel", "bsd", "initrd"))
                    self.assertEqual(len(c.name.split(".")[0]), 8)

    def test_decoys_are_labelled_none(self):
        decoys = [c for disk in self.disks(80, mode="decoy") for c in disk
                  if c.system in ("alpine-rootfs", "junk") and c.name in
                  ("vmlinuz", "initrd.img", "KERNEL.ELF", "BOOTAA64.EFI", "Image", "bsd", "grubaa64.efi", "vmlinuz-6.1.0-18-arm64")]
        self.assertTrue(decoys)
        for c in decoys:
            self.assertEqual((c.family, c.protocol, c.role), ("none", "none", "other"))

    def test_partitions_are_candidates_too(self):
        parts = [c for disk in self.disks(40) for c in disk if c.system == "partition"]
        self.assertTrue(parts)
        for c in parts:
            self.assertEqual(c.window[64:80], c.window[64:80])
            self.assertEqual(len(c.window), 512)
            self.assertEqual((c.family, c.protocol, c.role), ("none", "none", "other"))

    def test_a_disk_holds_one_file_per_path(self):
        for disk in self.disks(300):
            paths = [c.path.lower() for c in disk if c.system != "partition"]
            self.assertEqual(len(paths), len(set(paths)))

    def test_without_repeated_paths_keeps_the_first(self):
        a, b = asset("alpine", "1", "vmlinuz-virt"), asset("alpine", "1", "initramfs-virt")
        first = layouts.Entry("one").add("\\VMLINUZ", a).add("\\INITRD", b)
        second = layouts.Entry("two").add("\\vmlinuz", a).add("\\INITRD2", b)
        entries, extras = layouts.without_repeated_paths([first, second], [("\\INITRD2", b), ("\\X", b)])
        self.assertEqual([e.key for e in entries], ["one"])
        self.assertEqual([p for p, _ in extras], ["\\INITRD2", "\\X"])  # the second entry was dropped whole

    def test_no_more_entries_than_the_cap_are_numbered(self):
        saved = layouts.MAX_ENTRIES
        layouts.MAX_ENTRIES = 1
        try:
            for disk in self.disks(120):
                self.assertLessEqual(len({c.entry for c in disk if c.entry is not None}), 1)
        finally:
            layouts.MAX_ENTRIES = saved

    def test_the_entry_cap_is_the_group_space(self):
        self.assertEqual(layouts.MAX_ENTRIES, labels.GROUPS - 1)


class Synthetic(unittest.TestCase):
    def test_deterministic_and_typed(self):
        self.assertEqual(synthetic.junk_assets(30, 5), synthetic.junk_assets(30, 5))
        self.assertNotEqual(synthetic.junk_assets(30, 5), synthetic.junk_assets(30, 6))

    def test_magic_numbers(self):
        rng = random.Random(1)
        self.assertTrue(synthetic.png(rng).startswith(b"\x89PNG\r\n\x1a\n"))
        self.assertTrue(synthetic.gzip_file(rng).startswith(b"\x1f\x8b"))
        self.assertTrue(synthetic.xz_file(rng).startswith(b"\xfd7zXZ"))
        self.assertTrue(synthetic.zip_file(rng).startswith(b"PK"))
        self.assertTrue(synthetic.bmp(rng).startswith(b"BM"))
        self.assertTrue(synthetic.model_file(rng).startswith(b"WSMD"))

    def test_bls_entry_has_the_keys_a_loader_needs(self):
        text, machine, version = synthetic.bls_entry(random.Random(2), "fedora")
        lines = dict(line.split(" ", 1) for line in text.decode().splitlines())
        for key in ("title", "version", "linux", "initrd", "options", "architecture"):
            self.assertIn(key, lines)
        self.assertEqual(lines["linux"], f"/{machine}/{version}/linux")

    def test_partition_guids_are_sixteen_bytes_in_disk_order(self):
        for _, guid, _ in synthetic.PARTITIONS:
            self.assertEqual(len(guid), 16)
        esp = synthetic.PARTITIONS[0][1]
        self.assertEqual(esp.hex(), "28732ac11ff8d211ba4b00a0c93ec93b")  # C12A7328-F81F-11D2-BA4B-00A0C93EC93B

    def test_partition_starts_fit_the_window(self):
        rng = random.Random(3)
        for _, _, start in synthetic.PARTITIONS:
            self.assertGreaterEqual(len(start(rng)), 432)
        self.assertEqual(synthetic.fat32_boot_sector(rng)[:3], b"\xeb\x58\x90")


class Fetch(unittest.TestCase):
    def test_checksum_formats(self):
        text = (f"SHA256 (BOOTAA64.EFI) = {'1' * 64}\n"
                f"{'2' * 64}  ./netboot/netboot.tar.gz\n"
                f"{'3' * 64} *other\n"
                f"{'4' * 64}  alpine-minirootfs.tar.gz\n")
        self.assertEqual(fetch.parse_checksum(text, "BOOTAA64.EFI"), "1" * 64)
        self.assertEqual(fetch.parse_checksum(text, "netboot/netboot.tar.gz"), "2" * 64)
        self.assertEqual(fetch.parse_checksum(text, "other"), "3" * 64)
        self.assertEqual(fetch.parse_checksum(text, "alpine-minirootfs.tar.gz"), "4" * 64)
        self.assertIsNone(fetch.parse_checksum(text, "missing"))
        self.assertIsNone(fetch.parse_checksum(text, "BOOTAA64"))
        self.assertIsNone(fetch.parse_checksum(text, "netboot.tar.gz"))  # the directory is part of the name
        self.assertIsNone(fetch.parse_checksum(text, "tar.gz"))

    def test_cache_entry_is_checked_when_read(self):
        original = fetch.CACHE
        with tempfile.TemporaryDirectory() as scratch:
            fetch.CACHE = Path(scratch)
            try:
                digest = fetch.store(b"hello")
                pin = {"sha256": digest, "url": "x"}
                self.assertEqual(fetch.pinned_bytes(pin), b"hello")
                fetch.cache_path(digest).write_bytes(b"tampered")
                with self.assertRaisesRegex(ValueError, "corrupt"):
                    fetch.pinned_bytes(pin)
            finally:
                fetch.CACHE = original

    def test_the_pins_file_lists_only_verifiable_sources(self):
        import json
        pins = json.loads(fetch.PINS.read_text())
        self.assertTrue(pins)
        for pin in pins:
            self.assertRegex(pin["sha256"], r"^[0-9a-f]{64}$")
            self.assertIn(pin["mode"], ("prefix", "archive"))
            self.assertTrue(pin["url"].startswith("https://"))
            if pin["mode"] == "archive":
                self.assertNotIn("first fetch", pin["verified_against"])
            else:
                self.assertLessEqual(pin["bytes"], fetch.PREFIX_BYTES)


if __name__ == "__main__":
    unittest.main()
