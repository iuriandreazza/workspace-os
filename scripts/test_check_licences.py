"""Unit tests for the decision logic of check-licences.py (no network, no cargo)."""
import importlib.util
import sys
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # set before the import, so the gate leaves no .pyc beside the script

spec = importlib.util.spec_from_file_location(
    "check_licences", Path(__file__).with_name("check-licences.py")
)
lic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lic)

MPL = {("ucs2", "0.3.3")}


def verdict(licence, name="x", version="1.0.0"):
    return lic.verdict(name, version, licence, MPL)


class Allowed(unittest.TestCase):
    def test_accepted(self):
        for licence in [
            "MIT", "MIT OR Apache-2.0", "Apache-2.0 OR MIT", "MIT/Apache-2.0",
            "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT", "Unlicense OR MIT",
            "Apache-2.0 OR BSL-1.0", "Zlib", "ISC", "BSD-3-Clause", "GPL-2.0-only",
            "(MIT OR Apache-2.0) AND Unicode-3.0", "GPL-3.0-only OR MIT",
            "MPL-2.0 OR MIT",
        ]:
            self.assertIsNone(verdict(licence), licence)

    def test_listed_mpl_crate_and_version(self):
        self.assertIsNone(verdict("MPL-2.0", "ucs2", "0.3.3"))


class Refused(unittest.TestCase):
    def test_refused(self):
        for licence in [
            "Apache-2.0", "GPL-3.0-only", "GPL-3.0-or-later", "AGPL-3.0-only",
            "Apache-2.0 WITH LLVM-exception", "Apache-2.0 AND MIT", "(MIT OR Apache-2.0) AND GPL-3.0-only",
            "LGPL-2.1-only", "Foo-1.0", "MPL-1.1", "MIT AND",
            "MIT OR", "(MIT", "MIT)", "MIT WITH", "OR MIT", "mit", "", None,
        ]:
            self.assertIsNotNone(verdict(licence), licence)

    def test_mpl_unlisted_crate_names_the_adr_and_the_exhibit_b_check(self):
        message = verdict("MPL-2.0", "newcrate", "1.2.3")
        for part in ["newcrate 1.2.3", "MPL-2.0", "ADR 0017", "Exhibit B", "table"]:
            self.assertIn(part, message)

    def test_mpl_new_version_of_listed_crate(self):
        self.assertIn("ADR 0017", verdict("MPL-2.0", "ucs2", "0.3.4"))

    def test_mpl_inside_and_is_not_rescued_by_other_side(self):
        self.assertIn("ADR 0017", verdict("MPL-2.0 AND MIT", "ucs2", "0.4.0"))

    def test_mpl_older_licence_is_not_the_mpl_message(self):
        self.assertIn("ADR 0005", verdict("MPL-1.1"))

    def test_other_messages_name_crate_version_licence_and_adr(self):
        message = verdict("Apache-2.0", "approx", "0.5.1")
        for part in ["approx 0.5.1", "Apache-2.0", "ADR 0005"]:
            self.assertIn(part, message)
        self.assertIn("ADR 0005", verdict(None))


class Table(unittest.TestCase):
    ADR = "text\n\nMPL-2.0 dependencies accepted so far:\n\n| Crate | Version |\n|---|---|\n| `ucs2` | 0.3.3 | `bootloader` | `uefi` 0.41.0 | absent |\n"

    def test_reads_rows(self):
        self.assertEqual(lic.mpl_table(self.ADR), {("ucs2", "0.3.3")})

    def test_rows_outside_the_table_or_with_exhibit_b_are_not_accepted(self):
        adr = self.ADR + "| `present` | 1.0.0 | `bootloader` | `x` | present |\n\nlater\n\n| `evil` | 1.0.0 | `a` | `b` | absent |\n"
        self.assertEqual(lic.mpl_table(adr), {("ucs2", "0.3.3")})

    def test_missing_marker_or_rows_is_an_error(self):
        with self.assertRaises(ValueError):
            lic.mpl_table("nothing")
        with self.assertRaises(ValueError):
            lic.mpl_table("MPL-2.0 dependencies accepted so far:\n\n| Crate | Version |\n")

    def test_the_real_adr_lists_ucs2(self):
        self.assertIn(("ucs2", "0.3.3"), lic.mpl_table(lic.MPL_ADR.read_text()))


class TreeAgainstMetadata(unittest.TestCase):
    def test_tree_entries_unknown_to_metadata_are_reported(self):
        packages = [{"name": "a", "version": "1.0.0"}]
        self.assertEqual(lic.unknown_to_metadata(packages, {("a", "1.0.0")}), [])
        self.assertEqual(lic.unknown_to_metadata(packages, {("b", "2.0.0")}), [("b", "2.0.0")])


class Tree(unittest.TestCase):
    def test_parses_crates_and_ignores_feature_and_duplicate_markers(self):
        text = (
            "kernel v0.1.0 (/work/kernel)\n"
            "serde_derive v1.0.229 (proc-macro)\n"
            "wasi v0.11.1+wasi-snapshot-preview1\n"
            "foo v0.1.0 (https://github.com/x/foo?rev=1#abc)\n"
            "serde v1.0.229 (*)\n"
            "serde feature \"derive\"\n"
            "\n"
        )
        self.assertEqual(
            lic.parse_tree(text),
            {
                ("kernel", "0.1.0"), ("serde_derive", "1.0.229"),
                ("wasi", "0.11.1+wasi-snapshot-preview1"), ("foo", "0.1.0"), ("serde", "1.0.229"),
            },
        )

    def test_unexpected_output_gives_an_empty_set(self):
        self.assertEqual(lic.parse_tree("garbage\n"), set())


class Violations(unittest.TestCase):
    def test_unbuilt_crates_are_skipped_members_and_vendored_crates_are_not(self):
        def package(name, source, licence):
            return {"id": name, "name": name, "version": "1.0.0", "source": source, "license": licence}

        packages = [
            package("own", None, "GPL-2.0-only"),
            package("unbuilt", "registry", "Apache-2.0"),
            package("bad", "registry", "GPL-3.0-only"),
            package("vendored", None, "AGPL-3.0-only"),  # a path dependency inside the workspace is a member
        ]
        built = {(name, "1.0.0") for name in ["own", "bad", "vendored"]}
        checked = lic.built_only(packages, built)
        self.assertEqual([p["name"] for p in checked], ["bad", "own", "vendored"])
        self.assertEqual(len(lic.violations(checked, MPL)), 2)


if __name__ == "__main__":
    unittest.main()
