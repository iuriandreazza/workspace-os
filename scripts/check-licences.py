#!/usr/bin/env python3
"""Fails when a third-party dependency has a licence ADR 0005 and ADR 0017 do
not allow. Called by scripts/check.sh; run it alone to see the verdict.

It reads the licence each crate declares in its manifest (`cargo metadata`),
for the crates cargo can build in this workspace (`cargo tree`, all targets,
all kinds, all features of the workspace crates). A crate that sits in
Cargo.lock only through an optional feature nobody enables is not built, so it
is not checked; enabling that feature changes the tree and so the verdict.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MPL_ADR = ROOT / "docs/adr/0017-mpl2-dependencies.md"

# Permissive, or GPL-2.0 itself. Each is listed by the FSF as compatible with
# the GPL, versions 2 and 3 alike (checked against gnu.org/licenses/license-list
# on 2026-11-16), except the two GPL-2.0 ids, which need no check.
ACCEPTED = frozenset(
    {
        "MIT",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "ISC",
        "Zlib",
        "Unicode-3.0",
        "Unlicense",
        "BSL-1.0",
        "GPL-2.0-only",
        "GPL-2.0-or-later",
    }
)
MPL = "MPL-2.0"


def parse_expression(text):
    """SPDX expression to a tree: ("id", s), ("and", [...]) or ("or", [...]).

    The old `/` separator means OR. A `WITH` exception stays part of the id, so
    `Apache-2.0 WITH LLVM-exception` is a different licence from `Apache-2.0`.
    Anything malformed raises ValueError and is refused.
    """
    tokens = re.findall(r"\(|\)|[^\s()]+", text.replace("/", " OR "))
    pos = 0

    def peek():
        return tokens[pos] if pos < len(tokens) else None

    def take():
        nonlocal pos
        pos += 1
        return tokens[pos - 1]

    def parse_or():
        items = [parse_and()]
        while peek() == "OR":
            take()
            items.append(parse_and())
        return items[0] if len(items) == 1 else ("or", items)

    def parse_and():
        items = [parse_atom()]
        while peek() == "AND":
            take()
            items.append(parse_atom())
        return items[0] if len(items) == 1 else ("and", items)

    def parse_atom():
        if peek() is None:
            raise ValueError("unexpected end")
        token = take()
        if token == "(":
            node = parse_or()
            if peek() != ")":
                raise ValueError("missing )")
            take()
            return node
        if token in (")", "OR", "AND", "WITH"):
            raise ValueError(f"unexpected {token}")
        if peek() == "WITH":
            take()
            if peek() is None or peek() in ("(", ")", "OR", "AND", "WITH"):
                raise ValueError("WITH without an exception")
            token = f"{token} WITH {take()}"
        return ("id", token)

    tree = parse_or()
    if pos != len(tokens):
        raise ValueError(f"unexpected {tokens[pos]}")
    return tree


def allowed(node, name, version, mpl_accepted):
    kind, value = node
    if kind == "id":
        if value == MPL:
            return (name, version) in mpl_accepted
        return value in ACCEPTED
    results = (allowed(child, name, version, mpl_accepted) for child in value)
    return any(results) if kind == "or" else all(results)


def verdict(name, version, licence, mpl_accepted):
    """None if the crate is allowed, otherwise the message that says why."""
    who = f"{name} {version}"
    if not licence:
        return (
            f"{who}: no licence declared in its manifest. Refused by ADR 0005 "
            "(third-party code must be GPL-2.0 compatible, and that cannot be shown)."
        )
    try:
        tree = parse_expression(licence)
    except ValueError as error:
        return f"{who}: licence `{licence}` is not a valid SPDX expression ({error}). Refused by ADR 0005."
    if allowed(tree, name, version, mpl_accepted):
        return None
    if re.search(r"(?<![\w.-])MPL-2\.0(?![\w.-])", licence):
        return (
            f"{who}: licence `{licence}` is MPL-2.0 and this crate and version are not in "
            "the table of docs/adr/0017-mpl2-dependencies.md. Repeat the Exhibit B check "
            "on the sources of this exact version (ADR 0017, condition 2), then add or "
            "update its row in the table."
        )
    return (
        f"{who}: licence `{licence}` is not allowed by ADR 0005 "
        "(allowed: permissive licences compatible with GPL-2.0, GPL-2.0 itself, and "
        "an OR expression with one allowed side; Apache-2.0 alone, GPL-3.0 and "
        "AGPL-3.0 cannot be linked in)."
    )


def mpl_table(adr_text):
    """(crate, version) pairs from the ADR 0017 table of accepted dependencies."""
    marker = "MPL-2.0 dependencies accepted so far:"
    if marker not in adr_text:
        raise ValueError(f"ADR 0017 has no line `{marker}`")
    rows = re.findall(
        r"^\|\s*`([^`]+)`\s*\|\s*([^|\s]+)\s*\|",
        adr_text.split(marker, 1)[1],
        re.MULTILINE,
    )
    if not rows:
        raise ValueError("the table of accepted MPL-2.0 dependencies in ADR 0017 has no rows")
    return set(rows)


def third_party_built(packages, members, built):
    """Built packages that are not workspace members. A path dependency outside
    the workspace has no registry source but is still third-party code."""
    return sorted(
        (p for p in packages if p["id"] not in members and (p["name"], p["version"]) in built),
        key=lambda p: (p["name"], p["version"]),
    )


def violations(packages, mpl_accepted):
    found = []
    for package in packages:
        message = verdict(package["name"], package["version"], package["license"], mpl_accepted)
        if message:
            found.append(message)
    return found


def run(*command):
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"licences: `{' '.join(command)}` failed:\n{result.stderr}")
    return result.stdout


def built_packages():
    tree = run(
        "cargo", "tree", "--locked", "--workspace", "--all-features", "--target", "all",
        "-e", "all", "--prefix", "none", "-f", "{p}",
    )
    return {m.groups() for m in re.finditer(r"^(\S+) v(\S+)", tree, re.MULTILINE)}


def main():
    try:
        mpl_accepted = mpl_table(MPL_ADR.read_text())
    except ValueError as error:
        sys.exit(f"licences: {error}")
    metadata = json.loads(run("cargo", "metadata", "--locked", "--format-version", "1"))
    built = built_packages()
    packages = third_party_built(metadata["packages"], set(metadata["workspace_members"]), built)
    found = violations(packages, mpl_accepted)
    for message in found:
        print(f"licences: {message}", file=sys.stderr)
    if found:
        return 1
    print(f"licences: {len(packages)} third-party crates, all allowed by ADR 0005 and ADR 0017")
    return 0


if __name__ == "__main__":
    sys.exit(main())
