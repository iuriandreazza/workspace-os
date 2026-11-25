"""Extra inputs a window could carry, built for the B2b measurements (options E and F of the accuracy note).

They are measurement code, not the contract: `window.py` and the `\\WSOS.MDL` format are unchanged, and the report
says what B3 would have to settle if either input is wanted.

- E, the file size: one more position after the 512, holding the size bucket (`bit_length` of the size, capped to 31).
  The size is in the directory entry the enumeration already reads, so no read is added. It filters nothing: it is an
  input to the model, and ADR 0027's enumeration "applies no filter by name, size, extension or content".
- F, neighbour names: 16 slots of 32 bytes after the rest, one for each other file of the same directory, sorted,
  each name right-aligned like the path of the window.

Standard library only; tested in the gate.
"""
import assembly
import window

SIZE_BUCKETS = 32
SIBLINGS = 16
SIBLING_BYTES = 32
SIBLING_SLOT = SIBLINGS * SIBLING_BYTES


def size_bucket(size):
    return min(SIZE_BUCKETS - 1, max(0, size).bit_length())


def _sibling_name(path):
    tail = window.narrow(path.rsplit("\\", 1)[1])[-SIBLING_BYTES:]
    return bytes(SIBLING_BYTES - len(tail)) + tail


def sibling_slots(disk):
    """The sibling bytes of every candidate of a disk (`SIBLING_SLOT` bytes each, zeros where there are none)."""
    by_directory = {}
    for index, c in enumerate(disk):
        if c.path:
            by_directory.setdefault(assembly.directory(c.path), []).append(index)
    out = []
    for index, c in enumerate(disk):
        names = []
        if c.path:
            names = sorted(_sibling_name(disk[i].path) for i in by_directory[assembly.directory(c.path)] if i != index)
        slot = b"".join(names[:SIBLINGS])
        out.append(slot + bytes(SIBLING_SLOT - len(slot)))
    return out


def extended_windows(disk, size=False, neighbours=False):
    """Each candidate's window followed by the extra inputs asked for."""
    siblings = sibling_slots(disk) if neighbours else None
    out = []
    for index, c in enumerate(disk):
        extra = bytes([size_bucket(c.size)]) if size else b""
        out.append(c.window + extra + (siblings[index] if neighbours else b""))
    return out
