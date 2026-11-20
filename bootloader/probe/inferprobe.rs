//! Measures how long a boot model takes to run inside a UEFI application (B1-2,
//! ADR 0027). It is not the bootloader and is never loaded by it: it is the
//! only program on its own disk image (`scripts/measure-boot-model.py`).
//!
//! No trained weights exist yet (B2), so the weights are bytes from a fixed
//! pseudo-random generator, laid out in the shapes of the candidates. Time does
//! not depend on the values of the weights, only on their shapes and on how
//! much memory they take; the checksums below say the work was done and is the
//! same on every machine.
//!
//! Three workloads, all single core and with no operating system:
//!
//! - `classifier`: the candidate of ADR 0027, one candidate file scored by an
//!   int8 network (512 header bytes embedded, two dense layers, heads), times
//!   the number of candidates of one disk.
//! - `slm-int8`: one decoding step of a language model with the shapes of
//!   SmolLM2-135M, int8 weights, without attention and normalisation (they add
//!   little at this size, ADR 0027 says how much).
//! - `slm-fp32-layer`: one transformer layer of the same shapes in fp32.
//!
//! `\INFER.CFG` on the partition may hold `reps N`, `slm 0` or `slm 1` and
//! `warmup_ms N`.

#![no_std]
#![no_main]

use core::arch::asm;
use core::hint::black_box;
use core::slice;

use uefi::boot::{self, AllocateType, MemoryType};
use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileMode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{self, ResetType};
use uefi::{cstr16, println};

const PAGE: usize = 4096;
const DEFAULT_REPS: usize = 11;
const MAX_REPS: usize = 64;
const DEFAULT_WARMUP_MS: u64 = 1000;
/// Candidate files scored for one disk: a kernel, an initrd, a loader and a
/// handful of other files per system on the disk.
const CANDIDATES: usize = 16;

// The classifier's shape. 512 bytes of a file header, each embedded in 32
// numbers, one dense layer of 256, one of 128 and the heads: about 32
// scores for the labels of ADR 0027 (system, protocol, role, group).
const WINDOW: usize = 512;
const EMBEDDING: usize = 32;
const INPUTS: usize = WINDOW * EMBEDDING;
const HIDDEN1: usize = 256;
const HIDDEN2: usize = 128;
const HEADS: usize = 32;

// SmolLM2-135M (Hugging Face card): 30 layers, hidden 576, intermediate 1536,
// 9 attention heads of 64 and 3 key-value heads, vocabulary 49152.
const LAYERS: usize = 30;
const HIDDEN: usize = 576;
const KV: usize = 192;
const INTERMEDIATE: usize = 1536;
const VOCABULARY: usize = 49152;
/// Rows and columns of the matrices of one layer, in the order they run.
const LAYER_MATRICES: [(usize, usize); 7] = [
    (HIDDEN, HIDDEN),
    (KV, HIDDEN),
    (KV, HIDDEN),
    (HIDDEN, HIDDEN),
    (INTERMEDIATE, HIDDEN),
    (INTERMEDIATE, HIDDEN),
    (HIDDEN, INTERMEDIATE),
];

fn layer_weights() -> usize {
    LAYER_MATRICES.iter().map(|(rows, cols)| rows * cols).sum()
}

fn slm_weights() -> usize {
    LAYERS * layer_weights() + VOCABULARY * HIDDEN
}

/// The counter that runs at a fixed frequency. The `isb` keeps the read from
/// being executed ahead of the work before it.
fn ticks() -> u64 {
    let value: u64;
    // SAFETY: `CNTVCT_EL0` is readable at EL1 and EL2, which is where UEFI
    // applications run, and the read has no side effect.
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) value, options(nostack)) };
    value
}

fn frequency() -> u64 {
    let value: u64;
    // SAFETY: `CNTFRQ_EL0` is readable at every exception level and the read
    // has no side effect.
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn current_el() -> u64 {
    let value: u64;
    // SAFETY: `CurrentEL` is readable at every exception level.
    unsafe { asm!("mrs {}, CurrentEL", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value >> 2
}

fn fpcr() -> u64 {
    let value: u64;
    // SAFETY: `FPCR` is readable when FP is enabled, and the program is only
    // here after the compiler already used the FP registers (UEFI, section
    // 2.3.6, leaves them enabled); if it were not enabled this traps, which is
    // the finding.
    unsafe { asm!("mrs {}, fpcr", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn cpacr_el1() -> u64 {
    let value: u64;
    // SAFETY: only called at EL1, where `CPACR_EL1` is readable.
    unsafe { asm!("mrs {}, cpacr_el1", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: fixed seed, so every machine gets the same weights.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

fn fill_bytes(bytes: &mut [i8], rng: &mut Rng) {
    let (words, rest) = bytes.as_chunks_mut::<8>();
    for chunk in words {
        let word = rng.next().to_le_bytes();
        for (slot, byte) in chunk.iter_mut().zip(word) {
            *slot = byte as i8;
        }
    }
    let word = rng.next().to_le_bytes();
    for (slot, byte) in rest.iter_mut().zip(word) {
        *slot = byte as i8;
    }
}

fn fill_floats(values: &mut [f32], rng: &mut Rng) {
    for value in values {
        // In [-1/64, 1/64): finite, and sums of a thousand of them stay normal.
        *value = ((rng.next() >> 40) as i32 - (1 << 23)) as f32 / (1u32 << 29) as f32;
    }
}

/// Types for which all-zero bytes are a valid value, the only ones `allocate`
/// may hand out.
trait Zeroed {}
impl Zeroed for i8 {}
impl Zeroed for i32 {}
impl Zeroed for f32 {}

/// Pages from the firmware, as the bootloader takes them for the kernel.
fn allocate<T: Zeroed>(count: usize) -> Option<&'static mut [T]> {
    let bytes = count * size_of::<T>();
    let pages = bytes.div_ceil(PAGE).max(1);
    let pointer =
        boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages).ok()?;
    // SAFETY: the firmware gave `pages` whole pages, aligned to 4096, that
    // nothing else uses and that stay allocated until the application ends.
    // `T` is `i8`, `i32` or `f32`, for which every bit pattern is valid, and the
    // memory is zeroed first.
    unsafe {
        core::ptr::write_bytes(pointer.as_ptr(), 0, pages * PAGE);
        Some(slice::from_raw_parts_mut(
            pointer.as_ptr().cast::<T>(),
            count,
        ))
    }
}

/// `y = W x`, int8 weights and activations, 32-bit sums. Sixteen independent
/// lanes, so the compiler can use the vector unit without reordering sums
/// (integer sums do not depend on the order).
fn matvec_i8(weights: &[i8], cols: usize, input: &[i8], output: &mut [i32]) {
    debug_assert!(cols.is_multiple_of(16), "the lanes cover whole rows");
    let (input, _) = input.as_chunks::<16>();
    for (row, slot) in weights.chunks_exact(cols).zip(output.iter_mut()) {
        let mut lanes = [0i32; 16];
        let (row, _) = row.as_chunks::<16>();
        for (w, x) in row.iter().zip(input) {
            for lane in 0..16 {
                lanes[lane] += i32::from(w[lane]) * i32::from(x[lane]);
            }
        }
        *slot = lanes.iter().sum::<i32>();
    }
}

/// `y = W x` in fp32 with eight lanes written out, so the order of the sums is
/// fixed by the source and the result is the same on every machine.
fn matvec_f32(weights: &[f32], cols: usize, input: &[f32], output: &mut [f32]) {
    debug_assert!(cols.is_multiple_of(8), "the lanes cover whole rows");
    let (input, _) = input.as_chunks::<8>();
    for (row, slot) in weights.chunks_exact(cols).zip(output.iter_mut()) {
        let mut lanes = [0f32; 8];
        let (row, _) = row.as_chunks::<8>();
        for (w, x) in row.iter().zip(input) {
            for lane in 0..8 {
                lanes[lane] += w[lane] * x[lane];
            }
        }
        let mut total = 0f32;
        for lane in lanes {
            total += lane;
        }
        *slot = total;
    }
}

/// Back to int8 for the next layer: a shift stands in for the scale, and the
/// clamp for the saturation a real int8 network does.
fn requantise(from: &[i32], to: &mut [i8], relu: bool) {
    for (source, target) in from.iter().zip(to.iter_mut()) {
        let value = (*source >> 9).clamp(-128, 127) as i8;
        *target = if relu { value.max(0) } else { value };
    }
}

fn fnv(mut hash: u64, value: u64) -> u64 {
    hash ^= value;
    hash.wrapping_mul(0x0000_0100_0000_01b3)
}

const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

struct Classifier {
    embedding: &'static mut [i8],
    dense1: &'static mut [i8],
    dense2: &'static mut [i8],
    heads: &'static mut [i8],
    features: &'static mut [i8],
    hidden: &'static mut [i32],
    activations: &'static mut [i8],
    scores: &'static mut [i32],
}

impl Classifier {
    fn new(rng: &mut Rng) -> Option<Self> {
        let classifier = Self {
            embedding: allocate(256 * EMBEDDING)?,
            dense1: allocate(HIDDEN1 * INPUTS)?,
            dense2: allocate(HIDDEN2 * HIDDEN1)?,
            heads: allocate(HEADS * HIDDEN2)?,
            features: allocate(INPUTS)?,
            hidden: allocate(HIDDEN1)?,
            activations: allocate(HIDDEN1)?,
            scores: allocate(HEADS)?,
        };
        fill_bytes(classifier.embedding, rng);
        fill_bytes(classifier.dense1, rng);
        fill_bytes(classifier.dense2, rng);
        fill_bytes(classifier.heads, rng);
        Some(classifier)
    }

    /// Scores one candidate: `header` is its first `WINDOW` bytes.
    fn score(&mut self, header: &[u8; WINDOW]) -> u64 {
        for (byte, slot) in header
            .iter()
            .zip(self.features.as_chunks_mut::<EMBEDDING>().0.iter_mut())
        {
            let start = usize::from(*byte) * EMBEDDING;
            slot.copy_from_slice(&self.embedding[start..start + EMBEDDING]);
        }
        matvec_i8(self.dense1, INPUTS, self.features, self.hidden);
        requantise(self.hidden, self.activations, true);
        matvec_i8(self.dense2, HIDDEN1, self.activations, self.hidden);
        requantise(
            &self.hidden[..HIDDEN2],
            &mut self.activations[..HIDDEN2],
            true,
        );
        matvec_i8(
            self.heads,
            HIDDEN2,
            &self.activations[..HIDDEN2],
            self.scores,
        );
        self.scores
            .iter()
            .fold(FNV_START, |hash, score| fnv(hash, *score as u64))
    }

    fn decide(&mut self, headers: &[[u8; WINDOW]; CANDIDATES]) -> u64 {
        let mut hash = FNV_START;
        for header in headers {
            hash = fnv(hash, self.score(black_box(header)));
        }
        hash
    }
}

struct Slm {
    weights: &'static mut [i8],
    /// What every step starts from, so repetitions do the same work.
    seed: &'static mut [i8],
    input: &'static mut [i8],
    output: &'static mut [i32],
}

impl Slm {
    fn new(rng: &mut Rng) -> Option<Self> {
        let slm = Self {
            weights: allocate(slm_weights())?,
            seed: allocate(INTERMEDIATE)?,
            input: allocate(INTERMEDIATE)?,
            output: allocate(VOCABULARY)?,
        };
        fill_bytes(slm.weights, rng);
        fill_bytes(slm.seed, rng);
        Some(slm)
    }

    /// One token through every layer and the output projection.
    fn step(&mut self) -> u64 {
        self.input.copy_from_slice(self.seed);
        let mut offset = 0;
        for _ in 0..LAYERS {
            for (rows, cols) in LAYER_MATRICES {
                let matrix = &self.weights[offset..offset + rows * cols];
                offset += rows * cols;
                matvec_i8(matrix, cols, &self.input[..cols], &mut self.output[..rows]);
                let kept = rows.min(INTERMEDIATE);
                requantise(&self.output[..kept], &mut self.input[..kept], false);
            }
        }
        let head = &self.weights[offset..offset + VOCABULARY * HIDDEN];
        matvec_i8(head, HIDDEN, &self.input[..HIDDEN], self.output);
        self.output
            .iter()
            .step_by(97)
            .fold(FNV_START, |hash, value| fnv(hash, *value as u64))
    }
}

struct Layer {
    weights: &'static mut [f32],
    seed: &'static mut [f32],
    input: &'static mut [f32],
    output: &'static mut [f32],
}

impl Layer {
    fn new(rng: &mut Rng) -> Option<Self> {
        let layer = Self {
            weights: allocate(layer_weights())?,
            seed: allocate(INTERMEDIATE)?,
            input: allocate(INTERMEDIATE)?,
            output: allocate(INTERMEDIATE)?,
        };
        fill_floats(layer.weights, rng);
        fill_floats(layer.seed, rng);
        Some(layer)
    }

    fn run(&mut self) -> u64 {
        self.input.copy_from_slice(self.seed);
        let mut offset = 0;
        for (rows, cols) in LAYER_MATRICES {
            let matrix = &self.weights[offset..offset + rows * cols];
            offset += rows * cols;
            matvec_f32(matrix, cols, &self.input[..cols], &mut self.output[..rows]);
            self.input[..rows.min(INTERMEDIATE)]
                .copy_from_slice(&self.output[..rows.min(INTERMEDIATE)]);
        }
        self.input.iter().fold(FNV_START, |hash, value| {
            fnv(hash, u64::from(value.to_bits()))
        })
    }
}

fn microseconds(ticks: u64, frequency: u64) -> u64 {
    (u128::from(ticks) * 1_000_000 / u128::from(frequency)) as u64
}

/// Runs `work` for `warmup_ms`, then `reps` times, prints the spread and returns the checksum, which
/// must be the same on every repetition.
fn measure(
    name: &str,
    reps: usize,
    warmup_ms: u64,
    frequency: u64,
    mut work: impl FnMut() -> u64,
) -> u64 {
    // A core that was idle runs slow for the first stretch of work, and the
    // firmware has been mostly waiting: run the work unmeasured first.
    let warmup_end = ticks() + frequency * warmup_ms / 1000;
    while ticks() < warmup_end {
        black_box(work());
    }
    let mut samples = [0u64; MAX_REPS];
    let mut checksum = 0;
    for (index, sample) in samples.iter_mut().take(reps).enumerate() {
        let start = ticks();
        let result = black_box(work());
        let end = ticks();
        *sample = end - start;
        if index == 0 {
            checksum = result;
        } else if result != checksum {
            println!("  inferprobe: FAIL {name} checksum changed between repetitions");
        }
    }
    let samples = &mut samples[..reps];
    let raw: [u64; MAX_REPS] = {
        let mut copy = [0; MAX_REPS];
        copy[..reps].copy_from_slice(samples);
        copy
    };
    samples.sort_unstable();
    let at = |numerator: usize| microseconds(samples[(reps - 1) * numerator / 4], frequency);
    println!(
        "  inferprobe: {name} reps {reps} median_us {} min_us {} q1_us {} q3_us {} max_us {} checksum {checksum:#018x}",
        at(2),
        microseconds(samples[0], frequency),
        at(1),
        at(3),
        microseconds(samples[reps - 1], frequency),
    );
    uefi::print!("  inferprobe: {name} raw_us");
    for sample in &raw[..reps] {
        uefi::print!(" {}", microseconds(*sample, frequency));
    }
    println!();
    checksum
}

struct Settings {
    reps: usize,
    slm: bool,
    warmup_ms: u64,
}

/// `reps N`, `slm 0|1` and `warmup_ms N`, one per line, from `\INFER.CFG` if there is one.
fn settings() -> Settings {
    let mut settings = Settings {
        reps: DEFAULT_REPS,
        slm: true,
        warmup_ms: DEFAULT_WARMUP_MS,
    };
    let mut text = [0u8; 256];
    let read = (|| {
        let image = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).ok()?;
        let mut fs = boot::open_protocol_exclusive::<SimpleFileSystem>(image.device()?).ok()?;
        let mut root = fs.open_volume().ok()?;
        let handle = root
            .open(
                cstr16!("\\INFER.CFG"),
                FileMode::Read,
                FileAttribute::empty(),
            )
            .ok()?;
        handle.into_regular_file()?.read(&mut text).ok()
    })()
    .unwrap_or(0);
    for line in core::str::from_utf8(&text[..read]).unwrap_or("").lines() {
        let mut words = line.split_whitespace();
        match (
            words.next(),
            words.next().and_then(|word| word.parse::<usize>().ok()),
        ) {
            (Some("reps"), Some(reps)) => settings.reps = reps.clamp(1, MAX_REPS),
            (Some("slm"), Some(flag)) => settings.slm = flag != 0,
            (Some("warmup_ms"), Some(ms)) => settings.warmup_ms = ms as u64,
            _ => {}
        }
    }
    settings
}

#[entry]
fn main() -> Status {
    let settings = settings();
    // The firmware arms a 5 minute watchdog before it starts a boot option
    // (UEFI 2.10, 7.5.1) and resets the machine when it fires. A long
    // series must not be reset in the middle, so the application disables it.
    if let Err(error) = boot::set_watchdog_timer(0, 0x1_0000, None) {
        println!(
            "  inferprobe: could not disable the watchdog: {:?}",
            error.status()
        );
    }
    let frequency = frequency();
    if frequency == 0 {
        println!("  inferprobe: FAIL the counter frequency is not set");
        return Status::UNSUPPORTED;
    }
    let level = current_el();
    println!();
    println!("  inferprobe: start");
    println!(
        "  inferprobe: exception level {level} counter {frequency} Hz fpcr {:#x}",
        fpcr()
    );
    if level == 1 {
        println!("  inferprobe: cpacr_el1 {:#x}", cpacr_el1());
    }
    println!(
        "  inferprobe: profile {} reps {} slm {} warmup_ms {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        settings.reps,
        settings.slm as u8,
        settings.warmup_ms
    );

    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let Some(mut classifier) = Classifier::new(&mut rng) else {
        println!("  inferprobe: FAIL no memory for the classifier");
        return Status::OUT_OF_RESOURCES;
    };
    println!(
        "  inferprobe: classifier {} int8 weights, {} candidates per disk, {} MACs per candidate",
        classifier.embedding.len()
            + classifier.dense1.len()
            + classifier.dense2.len()
            + classifier.heads.len(),
        CANDIDATES,
        HIDDEN1 * INPUTS + HIDDEN2 * HIDDEN1 + HEADS * HIDDEN2
    );
    let mut headers = [[0u8; WINDOW]; CANDIDATES];
    for header in &mut headers {
        for chunk in header.as_chunks_mut::<8>().0 {
            chunk.copy_from_slice(&rng.next().to_le_bytes());
        }
    }
    let one = measure(
        "classifier-candidate",
        settings.reps,
        settings.warmup_ms,
        frequency,
        || classifier.score(black_box(&headers[0])),
    );
    let disk = measure(
        "classifier-disk",
        settings.reps,
        settings.warmup_ms,
        frequency,
        || classifier.decide(&headers),
    );
    println!("  inferprobe: classifier checksums {one:#018x} {disk:#018x}");

    if settings.slm {
        let Some(mut layer) = Layer::new(&mut rng) else {
            println!("  inferprobe: FAIL no memory for the fp32 layer");
            return Status::OUT_OF_RESOURCES;
        };
        println!(
            "  inferprobe: slm-fp32-layer {} weights, {} MACs",
            layer_weights(),
            layer_weights()
        );
        measure(
            "slm-fp32-layer",
            settings.reps,
            settings.warmup_ms,
            frequency,
            || layer.run(),
        );
        let Some(mut slm) = Slm::new(&mut rng) else {
            println!(
                "  inferprobe: FAIL no memory for {} MiB of int8 weights",
                slm_weights() >> 20
            );
            return Status::OUT_OF_RESOURCES;
        };
        println!(
            "  inferprobe: slm-int8 {} weights ({} MiB), one token",
            slm_weights(),
            slm_weights() >> 20
        );
        measure(
            "slm-int8-token",
            settings.reps,
            settings.warmup_ms,
            frequency,
            || slm.step(),
        );
    }
    println!("  inferprobe: done");
    runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None)
}
