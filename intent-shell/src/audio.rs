//! The two audio ports of the host: the speaker and the microphone (ADR 0031,
//! section 4).
//!
//! Playback hands a temporary WAV file to the machine's own player. Capture on
//! macOS is the AudioQueue API of the system's AudioToolbox, declared here by
//! hand, so no crate is added: `cpal`, `hound` and `rodio` are Apache-2.0 only
//! (ADR 0005), and `coreaudio-sys` would bring a C binding generator for forty
//! declarations. Other systems have neither adapter yet: the message says so.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::ports::{AudioCapture, AudioPlayback};

/// Whisper reads 16 kHz.
pub const SAMPLE_RATE: u32 = 16_000;
/// An utterance is a request: this stops a forgotten recording from filling memory.
pub const MAX_SECONDS: u32 = 30;
const BYTES_PER_SECOND: usize = SAMPLE_RATE as usize * 2;
/// Less than this is a key pressed twice, not a request.
const MIN_BYTES: usize = BYTES_PER_SECOND / 4;
/// A reply is a few sentences; a player that runs longer than this is stuck.
const PLAYBACK_LIMIT: Duration = Duration::from_secs(180);

/// A mono 16-bit little-endian PCM file of `samples`, with its header.
pub fn wav_of(samples: &[u8]) -> Vec<u8> {
    let data = u32::try_from(samples.len()).unwrap_or(u32::MAX);
    let mut wav = Vec::with_capacity(44 + samples.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&data.saturating_add(36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    wav.extend_from_slice(samples);
    wav
}

/// What a finished recording is checked for before it is sent to anything.
pub fn checked_recording(samples: &[u8]) -> Result<Vec<u8>, String> {
    if samples.len() < MIN_BYTES {
        return Err("nothing was recorded: speak after the recording starts".to_owned());
    }
    // A microphone the terminal may not use gives zeros, not an error.
    if samples.iter().all(|&byte| byte == 0) {
        return Err("the microphone gave only silence: the terminal may lack permission to use it (System Settings, Privacy & Security, Microphone)".to_owned());
    }
    Ok(wav_of(samples))
}

/// The directory only this user can open, where audio is written for the player.
fn private_directory() -> Result<PathBuf, String> {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let dir = std::env::temp_dir().join(format!("workspace-os-speech-{uid}"));
    let made = DirBuilder::new().mode(0o700).create(&dir);
    if let Err(e) = made
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(format!("cannot make {}: {e}", dir.display()));
    }
    // Whoever made it first may have been someone else, or it may be a link.
    let meta = fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} is not a private directory of yours; remove it and try again",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Plays through macOS's `afplay`, on a temporary file that is removed after.
pub struct AfplayPlayback;

static FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

impl AudioPlayback for AfplayPlayback {
    fn play(&self, wav: &[u8]) -> Result<(), String> {
        let path = private_directory()?.join(format!(
            "reply-{}-{}.wav",
            std::process::id(),
            FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| file.write_all(wav))
            .map_err(|e| format!("cannot write the audio: {e}"))?;
        let played = run_player(&path);
        let _ = fs::remove_file(&path);
        played
    }
}

fn run_player(path: &std::path::Path) -> Result<(), String> {
    let mut child = Command::new("/usr/bin/afplay")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run afplay: {e}"))?;
    let deadline = Instant::now() + PLAYBACK_LIMIT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("afplay stopped with {status}")),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("afplay did not finish".to_owned());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The player of this system, or why there is none.
pub fn platform_playback() -> Result<Box<dyn AudioPlayback + Send>, String> {
    if cfg!(target_os = "macos") {
        Ok(Box::new(AfplayPlayback))
    } else {
        Err("playing audio is only implemented for macOS (afplay) so far".to_owned())
    }
}

/// The microphone of this system, or why there is none.
pub fn platform_capture() -> Result<Box<dyn AudioCapture + Send>, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(mac::AudioQueueCapture::default()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("recording audio is only implemented for macOS (AudioQueue) so far".to_owned())
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::{Arc, Mutex};

    use super::{BYTES_PER_SECOND, MAX_SECONDS, SAMPLE_RATE, checked_recording};
    use crate::ports::AudioCapture;

    #[repr(C)]
    struct StreamDescription {
        sample_rate: f64,
        format_id: u32,
        format_flags: u32,
        bytes_per_packet: u32,
        frames_per_packet: u32,
        bytes_per_frame: u32,
        channels_per_frame: u32,
        bits_per_channel: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct QueueBuffer {
        capacity: u32,
        data: *mut c_void,
        size: u32,
        user_data: *mut c_void,
        description_capacity: u32,
        descriptions: *mut c_void,
        description_count: u32,
    }

    type Queue = *mut c_void;
    type InputCallback = extern "C" fn(
        user: *mut c_void,
        queue: Queue,
        buffer: *mut QueueBuffer,
        start_time: *const c_void,
        packets: u32,
        descriptions: *const c_void,
    );

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        fn AudioQueueNewInput(
            format: *const StreamDescription,
            callback: InputCallback,
            user: *mut c_void,
            run_loop: *const c_void,
            run_loop_mode: *const c_void,
            flags: u32,
            queue: *mut Queue,
        ) -> i32;
        fn AudioQueueAllocateBuffer(queue: Queue, bytes: u32, buffer: *mut *mut QueueBuffer)
        -> i32;
        fn AudioQueueEnqueueBuffer(
            queue: Queue,
            buffer: *mut QueueBuffer,
            descriptions: u32,
            description_array: *const c_void,
        ) -> i32;
        fn AudioQueueStart(queue: Queue, start_time: *const c_void) -> i32;
        fn AudioQueueStop(queue: Queue, immediately: u8) -> i32;
        fn AudioQueueDispose(queue: Queue, immediately: u8) -> i32;
    }

    const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
    const SIGNED_INTEGER_PACKED: u32 = 0x4 | 0x8;
    const BUFFERS: usize = 3;
    /// A tenth of a second of audio per buffer.
    const BUFFER_BYTES: u32 = (SAMPLE_RATE / 10) * 2;

    /// What the audio thread writes and the caller reads.
    struct Recording {
        samples: Mutex<Vec<u8>>,
    }

    extern "C" fn on_audio(
        user: *mut c_void,
        queue: Queue,
        buffer: *mut QueueBuffer,
        _start_time: *const c_void,
        _packets: u32,
        _descriptions: *const c_void,
    ) {
        // SAFETY: `user` is the `Arc<Recording>` pointer given to
        // `AudioQueueNewInput`; `Recorder` keeps that reference alive until the
        // queue is disposed, after which this callback is never called again.
        let recording = unsafe { &*user.cast::<Recording>() };
        // SAFETY: the queue passes a buffer it allocated, whose `data` holds
        // `size` valid bytes for the duration of this call.
        let chunk = unsafe {
            let buffer = &*buffer;
            std::slice::from_raw_parts(buffer.data.cast::<u8>(), buffer.size as usize)
        };
        if let Ok(mut samples) = recording.samples.lock() {
            let room = (MAX_SECONDS as usize * BYTES_PER_SECOND).saturating_sub(samples.len());
            samples.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        // SAFETY: the buffer belongs to this queue and is handed back to it.
        unsafe { AudioQueueEnqueueBuffer(queue, buffer, 0, ptr::null()) };
    }

    /// One open queue: the handle and the recording its callback writes to.
    struct Running {
        queue: Queue,
        recording: *const Recording,
    }

    // SAFETY: the queue handle and the pointer are only used by the thread that
    // owns the `AudioQueueCapture`, one call at a time; the audio thread only
    // touches the `Recording`, which is `Sync`.
    unsafe impl Send for Running {}

    #[derive(Default)]
    pub struct AudioQueueCapture {
        running: Option<Running>,
    }

    impl AudioQueueCapture {
        fn stop(&mut self) -> Option<Arc<Recording>> {
            let running = self.running.take()?;
            // SAFETY: `queue` was created by `AudioQueueNewInput` and is
            // disposed once. Stopping and disposing immediately means the
            // callback has returned and will not run again.
            unsafe {
                AudioQueueStop(running.queue, 1);
                AudioQueueDispose(running.queue, 1);
            }
            // SAFETY: the pointer came from `Arc::into_raw` in `start` and the
            // callback that borrowed it is finished.
            Some(unsafe { Arc::from_raw(running.recording) })
        }
    }

    impl AudioCapture for AudioQueueCapture {
        fn start(&mut self) -> Result<(), String> {
            if self.running.is_some() {
                return Err("the microphone is already recording".to_owned());
            }
            let format = StreamDescription {
                sample_rate: f64::from(SAMPLE_RATE),
                format_id: LINEAR_PCM,
                format_flags: SIGNED_INTEGER_PACKED,
                bytes_per_packet: 2,
                frames_per_packet: 1,
                bytes_per_frame: 2,
                channels_per_frame: 1,
                bits_per_channel: 16,
                reserved: 0,
            };
            let recording = Arc::new(Recording {
                samples: Mutex::new(Vec::new()),
            });
            let user = Arc::into_raw(Arc::clone(&recording));
            let mut queue: Queue = ptr::null_mut();
            // SAFETY: `format` and `queue` are live for the call; `user` stays
            // valid until `stop` reclaims it; a null run loop makes the queue
            // call back on its own thread.
            let made = unsafe {
                AudioQueueNewInput(
                    &raw const format,
                    on_audio,
                    user.cast_mut().cast(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    &raw mut queue,
                )
            };
            let reclaim = || {
                // SAFETY: `user` came from `Arc::into_raw` above and the queue
                // that would have used it was not created or is gone.
                drop(unsafe { Arc::from_raw(user) });
            };
            if made != 0 || queue.is_null() {
                reclaim();
                return Err(format!("the microphone cannot be opened (error {made})"));
            }
            for _ in 0..BUFFERS {
                let mut buffer: *mut QueueBuffer = ptr::null_mut();
                // SAFETY: `queue` is the live queue made above; `buffer` is
                // written by the call and used only if it succeeded.
                let ready = unsafe {
                    AudioQueueAllocateBuffer(queue, BUFFER_BYTES, &raw mut buffer) == 0
                        && AudioQueueEnqueueBuffer(queue, buffer, 0, ptr::null()) == 0
                };
                if !ready {
                    // SAFETY: the queue is live and disposed once, here.
                    unsafe { AudioQueueDispose(queue, 1) };
                    reclaim();
                    return Err("the microphone buffers cannot be set up".to_owned());
                }
            }
            // SAFETY: `queue` is live and has buffers enqueued.
            let started = unsafe { AudioQueueStart(queue, ptr::null()) };
            if started != 0 {
                // SAFETY: the queue is live and disposed once, here.
                unsafe { AudioQueueDispose(queue, 1) };
                reclaim();
                return Err(format!(
                    "the microphone cannot be started (error {started})"
                ));
            }
            self.running = Some(Running {
                queue,
                recording: user,
            });
            Ok(())
        }

        fn finish(&mut self) -> Result<Vec<u8>, String> {
            let recording = self.stop().ok_or("the microphone was not recording")?;
            let samples = recording
                .samples
                .lock()
                .map_err(|_| "the recording was lost".to_owned())?;
            checked_recording(&samples)
        }

        fn cancel(&mut self) {
            drop(self.stop());
        }
    }

    impl Drop for AudioQueueCapture {
        fn drop(&mut self) {
            self.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wav_header_describes_the_samples_it_carries() {
        let wav = wav_of(&[1, 0, 2, 0]);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            SAMPLE_RATE
        );
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
    }

    #[test]
    fn silence_and_a_blip_are_refused_with_a_reason_and_speech_is_wrapped() {
        let blip = vec![1u8; MIN_BYTES - 1];
        assert!(
            checked_recording(&blip)
                .unwrap_err()
                .contains("nothing was recorded")
        );
        let zeros = vec![0u8; MIN_BYTES * 2];
        assert!(
            checked_recording(&zeros)
                .unwrap_err()
                .contains("permission")
        );
        let speech = vec![3u8; MIN_BYTES * 2];
        assert_eq!(checked_recording(&speech).unwrap().len(), 44 + speech.len());
    }
}
