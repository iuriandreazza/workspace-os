"""The window the boot model reads for one candidate: the contract B3 implements.

ADR 0027, section 2: 512 positions per candidate, the first 64 for the path or
partition name and the rest for the first bytes of the content. The ADR leaves
the exact layout to B2; this is it.

    bytes 0..64    the name, RIGHT-aligned: the last 64 bytes of the path (the
                   tail keeps the file name and its extension), zero bytes on
                   the left when the path is shorter
    bytes 64..512  the first 448 bytes of the content, zero bytes after the end
                   of a shorter file

Path: the way the firmware's file protocol reports it, with backslashes and a
leading backslash, in the case the volume stores (UCS-2, each unit narrowed to
one byte; a unit above 0x7f becomes `?`). Partition: the GPT partition name
narrowed the same way (at most 36 units), and the content is the 16 bytes of the
partition type GUID as stored on disk, then the first 432 bytes of the partition.
"""

WINDOW = 512
NAME_BYTES = 64
CONTENT_BYTES = WINDOW - NAME_BYTES
GUID_BYTES = 16
PARTITION_CONTENT_BYTES = CONTENT_BYTES - GUID_BYTES


def narrow(text):
    """One byte per UCS-2 unit; anything outside ASCII becomes `?`."""
    out = bytearray()
    for char in text:
        code = ord(char)
        if code > 0xFFFF:
            out += b"??"  # a surrogate pair is two UCS-2 units
        else:
            out.append(code if code < 0x80 else ord("?"))
    return bytes(out)


def name_slot(name):
    tail = narrow(name)[-NAME_BYTES:]
    return bytes(NAME_BYTES - len(tail)) + tail


def content_slot(content, size=CONTENT_BYTES):
    head = bytes(content[:size])
    return head + bytes(size - len(head))


def file_window(path, content):
    if not path.startswith("\\"):
        raise ValueError(f"firmware paths start with a backslash: {path!r}")
    return name_slot(path) + content_slot(content)


def partition_window(name, type_guid, content):
    if len(type_guid) != GUID_BYTES:
        raise ValueError("a partition type GUID is 16 bytes")
    return name_slot(narrow(name)[:36].decode("latin-1")) + bytes(type_guid) + content_slot(content, PARTITION_CONTENT_BYTES)
