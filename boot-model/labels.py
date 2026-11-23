"""The closed labels of the boot model's typed decision (ADR 0027, section 3).

The model outputs one score per label, in four consecutive slices of its head:
system family, protocol, role and group. The order of the names below is the
order of the scores, so it is part of the `\\WSOS.MDL` contract with B3.
"""

# Families the corpus really contains (docs/b2-boot-model-corpus-and-training.md).
# Eight slots are reserved by ADR 0027: the last four have no example in the
# corpus, and the model is not trained to say them.
FAMILIES = ("none", "ours", "linux", "bsd", "unused4", "unused5", "unused6", "unused7")
PROTOCOLS = ("none", "ours", "linux", "chainload")
ROLES = ("other", "kernel", "initrd", "dtb", "loader", "entry")
GROUPS = 16

SLICES = (
    ("family", len(FAMILIES)),
    ("protocol", len(PROTOCOLS)),
    ("role", len(ROLES)),
    ("group", GROUPS),
)
HEADS = sum(size for _, size in SLICES)


def slice_bounds():
    """`{name: (start, end)}` of each output's scores in the head."""
    bounds, start = {}, 0
    for name, size in SLICES:
        bounds[name] = (start, start + size)
        start += size
    return bounds


def names_of(output):
    return {"family": FAMILIES, "protocol": PROTOCOLS, "role": ROLES}[output]


def encode(family, protocol, role, group):
    """The four class indices of one candidate, each checked against its set."""
    return (
        FAMILIES.index(family),
        PROTOCOLS.index(protocol),
        ROLES.index(role),
        _group(group),
    )


def _group(group):
    if not 0 <= group < GROUPS:
        raise ValueError(f"group {group} outside 0..{GROUPS - 1}")
    return group


def decode(scores):
    """Argmax of each slice of a head's scores: `(family, protocol, role, group)` indices.

    A tie goes to the lowest index, as the loader's argmax must (the same rule
    on both sides, or a tie decides differently in the bootloader and here).
    """
    if len(scores) != HEADS:
        raise ValueError(f"expected {HEADS} scores, got {len(scores)}")
    picked = []
    for _, (start, end) in slice_bounds().items():
        part = scores[start:end]
        picked.append(max(range(len(part)), key=lambda i: (part[i], -i)))
    return tuple(picked)
