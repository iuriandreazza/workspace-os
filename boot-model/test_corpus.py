"""Unit tests for the corpus side of the boot model tooling (B2): labelling, the split,
the layouts and their groups, the generated files, the checksum parser and the cache
check. No network, no cache, no numpy: assets are built in memory."""
import bz2
import gzip
import io
import lzma
import random
import sys
import tarfile
import zlib
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))

import assembly  # noqa: E402
import catalogue  # noqa: E402
import fetch  # noqa: E402
import formats  # noqa: E402
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

    def test_a_decoy_never_shares_a_path_with_a_real_file(self):
        for disk in self.disks(300, mode="decoy"):
            paths = [c.path.lower() for c in disk if c.system != "partition"]
            self.assertEqual(len(paths), len(set(paths)))

    def test_partitions_are_candidates_too(self):
        parts = [c for disk in self.disks(40) for c in disk if c.system == "partition"]
        self.assertTrue(parts)
        for c in parts:
            self.assertEqual(len(c.window), 512)
            self.assertIn(c.window[64:80], [guid for _, guid, _ in synthetic.PARTITIONS])  # the GUID sits at 64..80
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


def entry_assets(split="train", release="1.0"):
    return tiny_assets(split, release) + [
        asset("fedora", release, "vmlinuz", b"MZfedora" * 40, split),
        asset("fedora", release, "initrd.img", b"FINIT" * 80, split),
        asset("opensuse", release, "linux", b"MZsuse" * 40, split),
        asset("opensuse", release, "initrd", b"SINIT" * 80, split),
    ]


class EntryText(unittest.TestCase):
    """The text of a generated entry names the files of its own disk (ADR 0034 asked for this)."""

    def disks(self, count=400):
        return layouts.generate(entry_assets(), junk(), "train", count, 5)

    def texts(self):
        for disk in self.disks():
            for index, c in enumerate(disk):
                if c.role == "entry":
                    yield disk, index, c

    def test_the_generator_writes_entry_text_on_some_disks(self):
        self.assertGreater(len(list(self.texts())), 20)

    def test_the_kernel_line_names_a_kernel_of_the_same_entry_on_the_same_disk(self):
        for disk, _, c in self.texts():
            parsed = assembly.parse_entry_text(c.text)
            named = [d for d in disk if d.path and assembly.normalise(d.path) == parsed["linux"]]
            self.assertEqual(len(named), 1, c.text)
            self.assertEqual((named[0].role, named[0].entry), ("kernel", c.entry))

    def test_the_initrd_line_names_the_initrd_of_the_same_entry(self):
        for disk, _, c in self.texts():
            for path in assembly.parse_entry_text(c.text)["initrd"]:
                named = [d for d in disk if d.path and assembly.normalise(d.path) == path]
                self.assertEqual([(d.role, d.entry) for d in named], [("initrd", c.entry)])

    def test_the_entry_text_rule_has_no_false_pair_and_finds_every_pair_of_an_entry_with_text(self):
        for disk in self.disks():
            truth = assembly.true_pairs(disk)
            got = assembly.entry_text(assembly.files_of(disk))[0]
            self.assertLessEqual(got, truth)
            with_text = {c.entry for c in disk if c.role == "entry"}
            self.assertEqual({p for p in truth if disk[p[0]].entry in with_text}, got)

    def test_enterprise_linux_disks_carry_text_with_both_layouts(self):
        folders = set()
        for disk in self.disks():
            for c in disk:
                if c.role == "kernel" and c.system == "fedora" and any(d.role == "entry" and d.entry == c.entry for d in disk):
                    folders.add(assembly.directory(c.path).count("\\"))
        self.assertEqual(folders, {1, 2})  # `\boot` and `\<machine>\<version>`

    def test_the_text_is_far_from_its_kernel(self):
        for _, _, c in self.texts():
            self.assertTrue(c.path.lower().startswith("\\loader\\entries\\"))

    def test_two_kernels_share_a_directory_on_some_disks(self):
        shared = 0
        for disk in self.disks():
            kernels = [(c.entry[:2], assembly.directory(c.path)) for c in disk if c.role == "kernel" and c.system != "openbsd"]
            shared += len(kernels) != len(set(kernels))
        self.assertGreater(shared, 10)

    def test_the_suffix_rule_beats_the_directory_rule_on_such_disks(self):
        disks = self.disks()
        directory = assembly.score(disks, assembly.same_directory)
        suffix = assembly.score(disks, assembly.directory_suffix)
        self.assertGreater(suffix["precision"], directory["precision"])

    def test_every_candidate_carries_its_file_so_a_wrongly_labelled_one_is_parsed_as_what_it_is(self):
        for disk in self.disks(60):
            for c in disk:
                if c.system != "partition":
                    self.assertIsNotNone(c.text)
            roles = ["entry" if c.role == "other" else c.role for c in disk]  # every junk file called an entry
            truth = assembly.true_pairs(disk)
            got = assembly.entry_text(assembly.files_of(disk, roles))[0]
            self.assertEqual(got & truth, got)  # junk text names nothing on the disk: it adds no pair

    def test_generated_file_sizes_do_not_tell_the_class(self):
        sizes = [layouts.junk_size(kind, name, data) for kind, name, data in synthetic.junk_assets(300, 5)]
        self.assertGreater(max(sizes), 1 << 24)
        self.assertLess(min(sizes), 1 << 12)
        self.assertEqual(layouts.junk_size("a", "b", b"c"), layouts.junk_size("a", "b", b"c"))

    def test_generated_junk_does_not_depend_on_the_clock(self):
        # the local header of a zip holds a modification time; it must be the fixed one, not the current time
        archive = synthetic.zip_file(random.Random(1))
        self.assertEqual(archive[10:14], bytes.fromhex("0000" "215c"))  # 00:00:00 on 2026-01-01, MS-DOS format
        self.assertEqual(synthetic.junk_assets(100, 9), synthetic.junk_assets(100, 9))


def noise(size, seed=1):
    rng = random.Random(seed)
    return bytes(rng.randrange(256) for _ in range(size))


def tiny_rootfs():
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        rng = random.Random(4)
        for index in range(12):
            body = (b"\x7fELF" if index % 2 == 0 else b"#!/bin/sh\n") + bytes(rng.randrange(256) for _ in range(6000))
            info = tarfile.TarInfo(f"bin/tool{index}")
            info.size, info.mode = len(body), 0o755
            archive.addfile(info, io.BytesIO(body))
    return buffer.getvalue()


class Formats(unittest.TestCase):
    def members(self):
        return formats.rootfs_members([tiny_rootfs()])

    def test_cpio_newc_is_what_the_kernel_unpacks(self):
        archive = formats.cpio_newc([(".", 0o040755, b""), ("bin/a", 0o100755, b"abcde")])
        self.assertTrue(archive.startswith(b"070701"))
        self.assertEqual(len(archive) % 4, 0)
        self.assertIn(b"TRAILER!!!", archive)
        second = archive.index(b"070701", 6)
        self.assertEqual(int(archive[second + 6 + 8 * 6:second + 6 + 8 * 7], 16), 5)  # c_filesize
        self.assertEqual(archive[second + 110:second + 116], b"bin/a\0")
        self.assertEqual(archive[second + 116:second + 124], b"abcde" + bytes(3))  # data, padded to four

    def test_rootfs_members_keep_the_elf_flag(self):
        members = self.members()
        self.assertEqual(len(members), 12)
        self.assertEqual(sum(m[3] for m in members), 6)

    def test_payloads_are_deterministic_and_vary_with_the_seed(self):
        members = self.members()
        self.assertEqual(formats.initrd_payload(members, 1), formats.initrd_payload(members, 1))
        self.assertNotEqual(formats.initrd_payload(members, 1), formats.initrd_payload(members, 2))
        self.assertTrue(formats.code_payload(members, 1).startswith(b"\x7fELF"))

    def test_standard_library_formats_round_trip_and_start_with_their_magic(self):
        data = noise(50000) + b"abc" * 5000
        for variant in range(4):
            self.assertEqual(gzip.decompress(formats.encode("gzip", data, variant)), data)
            self.assertEqual(lzma.decompress(formats.encode("xz", data, variant)), data)
            self.assertEqual(bz2.decompress(formats.encode("bzip2", data, variant)), data)
        self.assertTrue(formats.encode("gzip", data).startswith(b"\x1f\x8b\x08"))
        self.assertTrue(formats.encode("xz", data).startswith(b"\xfd7zXZ\x00"))
        self.assertTrue(formats.encode("bzip2", data).startswith(b"BZh"))

    def test_xz_variants_use_both_integrity_checks(self):
        data = b"x" * 5000
        checks = {formats.encode("xz", data, v)[7] & 0x0F for v in range(2)}  # stream flags, second byte
        self.assertEqual(checks, {1, 4})  # CRC32 and CRC64

    def test_a_stream_prefix_is_the_size_the_corpus_keeps(self):
        self.assertEqual(len(formats.stream_prefix("xz", noise(20000))), 4096)

    def test_unknown_format_is_refused(self):
        with self.assertRaises(ValueError):
            formats.encode("rar", b"x")

    def test_optional_formats_have_their_magic_when_the_packages_are_there(self):
        data = b"hello world " * 4000
        try:
            import lz4.frame  # noqa: F401
            import zstandard  # noqa: F401
        except ImportError:
            self.skipTest("zstandard or lz4 not installed")
        self.assertTrue(formats.encode("zstd", data).startswith(b"\x28\xb5\x2f\xfd"))
        self.assertTrue(formats.encode("lz4", data, 0).startswith(b"\x02\x21\x4c\x18"))
        self.assertTrue(formats.encode("lz4", data, 1).startswith(b"\x04\x22\x4d\x18"))

    def augmented(self, assets, seed=1, copies=2):
        saved = formats.KERNEL_CHOICES, formats.INITRD_CHOICES
        formats.KERNEL_CHOICES = formats.INITRD_CHOICES = (None, "gzip", "xz", "bzip2")
        try:
            return formats.augment_assets(assets, [tiny_rootfs()], seed, copies)
        finally:
            formats.KERNEL_CHOICES, formats.INITRD_CHOICES = saved

    def test_a_recompressed_kernel_cannot_be_started_and_an_initrd_keeps_its_labels(self):
        assets = [asset("debian", "1", "linux", b"MZ" + bytes(62) + b"ARM\x64" + bytes(3000)),
                  asset("debian", "1", "initrd.gz", b"\x1f\x8b" + bytes(100)),
                  asset("debian", "1", "grubaa64.efi", b"MZgrub")]
        extra = self.augmented(assets, copies=30)
        kernels = [a for a in extra if a.role == "kernel"]
        self.assertTrue(any(not a.content.startswith(b"MZ") for a in kernels))
        for a in kernels:
            self.assertEqual(a.protocol, "linux" if a.content.startswith(b"MZ") else "none")
            self.assertEqual((a.family, a.system), ("linux", "debian"))
        initrds = [a for a in extra if a.role == "initrd"]
        self.assertTrue(all(a.protocol == "linux" and a.family == "linux" for a in initrds))
        self.assertEqual(len({a.content[:3] for a in initrds}), 4)  # real, gzip, xz, bzip2 all drawn

    def test_a_kernel_stream_starts_from_the_real_header_and_the_name_follows_the_format(self):
        assets = [asset("debian", "1", "linux", b"MZ" + bytes(62) + b"ARM\x64" + bytes(3000)),
                  asset("debian", "1", "initrd.gz", b"\x1f\x8b" + bytes(100))]
        extra = self.augmented(assets, copies=8)
        for a in extra:
            if a.role == "initrd" and a.content.startswith(b"\xfd7zXZ"):
                self.assertEqual(a.alias, "initrd.xz")
            if a.role == "initrd" and a.content.startswith(b"BZh"):
                self.assertEqual(a.alias, "initrd.bz2")
        sample = [a for a in extra if a.role == "kernel" and a.content.startswith(b"\x1f\x8b")]
        self.assertTrue(sample)
        head = zlib.decompressobj(31).decompress(sample[0].content)  # the stream is cut at 4 KiB: decode what is there
        self.assertEqual(head[:70], assets[0].content[:70])

    def test_releases_are_tagged_copies_and_the_originals_are_untouched(self):
        assets = [asset("fedora", "40", "vmlinuz", b"MZk" * 100), asset("fedora", "40", "initrd.img", b"i" * 300),
                  asset("fedora", "40", "grubaa64.efi", b"MZg"), asset("openbsd", "7", "bsd", b"\x7fELF")]
        before = [(a.release, a.content) for a in assets]
        extra = self.augmented(assets, copies=2)
        self.assertEqual(before, [(a.release, a.content) for a in assets])
        self.assertTrue(all(a.release.startswith("40~") for a in extra))
        self.assertEqual({a.name for a in extra}, {"vmlinuz", "initrd.img", "grubaa64.efi"})  # the lookup keys do not change
        self.assertEqual(len({a.release for a in extra}) * 3, len(extra))  # a copy is the whole release


    def test_a_debian_disk_names_a_re_encoded_initrd_by_its_format_and_still_builds(self):
        base = [asset("debian", "1", "linux", b"MZ" + bytes(3000)), asset("debian", "1", "initrd.gz", b"\x1f\x8b" + bytes(100))]
        extra = self.augmented(base, copies=30)
        names = set()
        for disk in layouts.generate(base + extra, junk(), "train", 300, 2):
            for c in disk:
                if c.role == "initrd" and c.entry and "installer" in c.path:
                    names.add(c.name)
        self.assertTrue({"initrd.gz", "initrd.xz", "initrd.bz2"} <= names, names)

    def test_other_systems_kernels_are_not_reencoded(self):
        assets = [asset("openbsd", "7", "bsd", b"\x7fELF" + bytes(100)), asset("openbsd", "7", "BOOTAA64.EFI", b"MZ")]
        self.assertEqual(self.augmented(assets), [])

    def test_two_releases_of_one_system_are_encoded_differently(self):
        assets = [asset("debian", r, n, c) for r in ("1", "2", "3") for n, c in
                  (("linux", b"MZ" + bytes(3000)), ("initrd.gz", b"\x1f\x8b"))]
        extra = self.augmented(assets, copies=2)
        tags = {}
        for a in extra:
            tags.setdefault(a.release.split("~")[0], set()).add(a.release.split("~")[1])
        self.assertEqual(len({frozenset(v) for v in tags.values()}), 3)

    def test_a_release_is_encoded_the_same_whatever_else_is_in_the_set(self):
        one = [asset("debian", "1", "linux", b"MZ" + bytes(3000)), asset("debian", "1", "initrd.gz", b"\x1f\x8b")]
        two = one + [asset("alpine", "3", "vmlinuz-virt", b"MZ" + bytes(3000)),
                     asset("alpine", "3", "initramfs-virt", b"\x1f\x8b")]
        pick = lambda extra: [(a.release, a.name, a.content) for a in extra if a.system == "debian"]
        self.assertEqual(pick(self.augmented(one, copies=8)), pick(self.augmented(two, copies=8)))


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

    def test_bls_entry_names_the_paths_it_is_given(self):
        text = synthetic.bls_entry(random.Random(2), "fedora", "/m/6.1/linux", "/m/6.1/initrd", "/dtb/a.dtb", "6.1")
        lines = dict(line.split(" ", 1) for line in text.decode().splitlines() if not line.startswith("sort-key"))
        for key in ("title", "version", "linux", "initrd", "devicetree", "options", "architecture"):
            self.assertIn(key, lines)
        self.assertEqual((lines["linux"], lines["initrd"], lines["devicetree"]), ("/m/6.1/linux", "/m/6.1/initrd", "/dtb/a.dtb"))
        self.assertNotIn("initrd", synthetic.bls_entry(random.Random(2), "fedora", "/k").decode())

    def test_outlier_files_use_formats_no_negative_uses(self):
        files = synthetic.outlier_files(200, 3)
        self.assertEqual(files, synthetic.outlier_files(200, 3))
        magics = {magic for magic, _ in synthetic.OUTLIER_KINDS.values()}
        self.assertTrue(all(any(content.startswith(m) for m in magics) for _, content in files))
        junk_kinds = {kind for kind, _, _ in synthetic.junk_assets(300, 1)}
        self.assertFalse(junk_kinds & set(synthetic.OUTLIER_KINDS))
        self.assertTrue(all(len(content) <= 4096 and path.startswith("\\") for path, content in files))
        junk_names = {name for _, names in synthetic.JUNK_KINDS.values() for name in names}
        self.assertFalse({path.rsplit("\\", 1)[1] for path, _ in files} & junk_names)

    def test_mixed_mode_names_come_from_other_templates_than_the_evaluation_sets(self):
        self.assertFalse(set(layouts.MIXED_DECOY_PATHS) & set(layouts.DECOY_PATHS))
        seen = set()
        for disk in layouts.generate(entry_assets(), junk(), "train", 200, 3, "mixed"):
            for c in disk:
                if c.entry is not None and c.role in ("kernel", "initrd"):
                    seen.add(c.name.rsplit(".", 1)[-1])
        self.assertTrue({"rom", "fw", "pak", "sys"} & seen)
        self.assertFalse({"dat", "bin"} & seen)

    def test_mixed_mode_keeps_the_labels_of_the_files_under_their_new_names(self):
        for disk in layouts.generate(entry_assets(), junk(), "train", 100, 3, "mixed"):
            for c in disk:
                if c.path in layouts.MIXED_DECOY_PATHS:
                    self.assertEqual((c.role, c.entry), ("other", None))

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
