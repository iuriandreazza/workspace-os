"""`\\WSOS.MDL`: the file that holds the boot model's weights (ADR 0027, section 7),
and the integer arithmetic that runs them.

The file is little-endian and has no padding:

    offset  size  field
    0       4     magic "WSMD"
    4       4     version, 1
    8       4     window (positions per candidate)
    12      4     embedding (numbers per position)
    16      4     hidden1
    20      4     hidden2
    24      4     heads (scores per candidate)
    28      4     shift1 (right shift after the first dense layer)
    32      4     shift2 (right shift after the second dense layer)
    36      4     payload length in bytes
    40      n     payload: int8 weights, row-major, in this order
                    embedding  256 x embedding
                    dense1     hidden1 x (window * embedding)
                    dense2     hidden2 x hidden1
                    heads      heads x hidden2
    40+n    4     CRC-32 (zlib) of every byte before it

The forward pass is the one `bootloader/probe/inferprobe.rs` measured: the
embedding rows of the window's bytes are laid end to end, a dense layer of int8
by int8 with 32-bit sums, an arithmetic right shift, a clamp to int8, a ReLU,
twice, and a last dense layer whose 32-bit scores are not shifted. No bias.
A CRC guards against corruption, not tampering (ADR 0027, section 7; B4).
"""
import struct
import zlib

MAGIC = b"WSMD"
VERSION = 1
HEADER = struct.Struct("<4s9I")
CRC = struct.Struct("<I")
VOCABULARY = 256
# What the bootloader's budget allows (ADR 0027, section 1).
MAX_WEIGHT_BYTES = 16 * 1024 * 1024
MAX_MACS = 16_000_000


class Shape:
    def __init__(self, window, embedding, hidden1, hidden2, heads, shift1, shift2):
        self.window, self.embedding = window, embedding
        self.hidden1, self.hidden2, self.heads = hidden1, hidden2, heads
        self.shift1, self.shift2 = shift1, shift2

    def inputs(self):
        return self.window * self.embedding

    def sizes(self):
        """Weights of each tensor, in file order."""
        return (
            VOCABULARY * self.embedding,
            self.hidden1 * self.inputs(),
            self.hidden2 * self.hidden1,
            self.heads * self.hidden2,
        )

    def weight_bytes(self):
        return sum(self.sizes())

    def macs(self):
        """Multiply-accumulates per candidate; the embedding lookup has none."""
        _, d1, d2, d3 = self.sizes()
        return d1 + d2 + d3

    def tuple(self):
        return (self.window, self.embedding, self.hidden1, self.hidden2, self.heads,
                self.shift1, self.shift2)

    def __eq__(self, other):
        return isinstance(other, Shape) and self.tuple() == other.tuple()

    def __repr__(self):
        return f"Shape{self.tuple()}"


INT32 = 2 ** 31


def _shift_problem(shape):
    for shift in (shape.shift1, shape.shift2):
        if not 0 <= shift < 32:
            return f"shift {shift} outside 0..31"
    return None


def sum_overflow(shape):
    """Why a dense layer's 32-bit sum could overflow in the bootloader, or None.

    Python integers do not wrap, a 32-bit sum does: a shape in which the largest possible sum
    (every weight and input at -128) reaches 2**31 would run differently in `mdl.forward`
    and in the bootloader.
    """
    for name, count in (("dense1", shape.inputs()), ("dense2", shape.hidden1), ("heads", shape.hidden2)):
        if count * 128 * 128 >= INT32:
            return f"{name} sums {count} products of int8 values, which can overflow 32 bits"
    return None


def within_budget(shape):
    """Why a shape is over the budget of ADR 0027, or None."""
    reason = sum_overflow(shape) or _shift_problem(shape)
    if reason:
        return reason
    if shape.weight_bytes() > MAX_WEIGHT_BYTES:
        return f"{shape.weight_bytes()} weight bytes, limit {MAX_WEIGHT_BYTES}"
    if shape.macs() > MAX_MACS:
        return f"{shape.macs()} multiply-accumulates, limit {MAX_MACS}"
    return None


def _to_bytes(tensor):
    return bytes(value & 0xFF for value in tensor)


def write(shape, tensors):
    """The file for `shape` and its four tensors (flat lists of ints in -128..127)."""
    if len(tensors) != 4:
        raise ValueError("four tensors: embedding, dense1, dense2, heads")
    refused = sum_overflow(shape) or _shift_problem(shape)
    if refused:
        raise ValueError(refused)
    for tensor, size in zip(tensors, shape.sizes()):
        if len(tensor) != size:
            raise ValueError(f"tensor has {len(tensor)} weights, the shape needs {size}")
        if tensor and not (-128 <= min(tensor) and max(tensor) <= 127):
            raise ValueError("a weight does not fit in int8")
    payload = b"".join(_to_bytes(t) for t in tensors)
    body = HEADER.pack(MAGIC, VERSION, *shape.tuple()[:5], shape.shift1, shape.shift2,
                       len(payload)) + payload
    return body + CRC.pack(zlib.crc32(body))


def read(data):
    """`(shape, tensors)` of a file, or ValueError saying what is wrong with it."""
    if len(data) < HEADER.size + CRC.size:
        raise ValueError("file too short")
    magic, version, *fields = HEADER.unpack_from(data)
    if magic != MAGIC:
        raise ValueError("bad magic")
    if version != VERSION:
        raise ValueError(f"unsupported version {version}")
    window, embedding, hidden1, hidden2, heads, shift1, shift2, length = fields
    shape = Shape(window, embedding, hidden1, hidden2, heads, shift1, shift2)
    refused = _shift_problem(shape) or sum_overflow(shape)
    if refused:
        raise ValueError(refused)
    if length != shape.weight_bytes():
        raise ValueError("payload length does not match the shape")
    if len(data) != HEADER.size + length + CRC.size:
        raise ValueError("file length does not match the payload length")
    (crc,) = CRC.unpack_from(data, HEADER.size + length)
    if crc != zlib.crc32(data[:HEADER.size + length]):
        raise ValueError("CRC mismatch")
    tensors, offset = [], HEADER.size
    for size in shape.sizes():
        raw = data[offset:offset + size]
        tensors.append([b - 256 if b > 127 else b for b in raw])
        offset += size
    return shape, tensors


def requantise(sums, shift):
    """Arithmetic shift, clamp to int8, ReLU: what the bootloader does between layers."""
    return [max(0, min(127, s >> shift)) for s in sums]


def dense(weights, cols, vector):
    """`y = W x` with exact integer sums."""
    return [
        sum(w * x for w, x in zip(weights[row:row + cols], vector))
        for row in range(0, len(weights), cols)
    ]


def forward(shape, tensors, window_bytes):
    """The 32-bit scores of one candidate's window, in plain Python integers."""
    if len(window_bytes) != shape.window:
        raise ValueError(f"window must be {shape.window} bytes")
    embedding, dense1, dense2, heads = tensors
    e = shape.embedding
    features = []
    for byte in window_bytes:
        features.extend(embedding[byte * e:(byte + 1) * e])
    hidden1 = requantise(dense(dense1, shape.inputs(), features), shape.shift1)
    hidden2 = requantise(dense(dense2, shape.hidden1, hidden1), shape.shift2)
    return dense(heads, shape.hidden2, hidden2)
