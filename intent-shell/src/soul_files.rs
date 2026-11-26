//! The SOUL file and its history on a Unix host (ADR 0036).
//!
//! One writer at a time (a `flock` on `.lock` inside the history directory), and
//! an order of steps that leaves every history file describing a change that
//! happened and never a SOUL file half written: the entry is written complete as
//! `<seq>.pending`, the SOUL is replaced by a rename, and only then the entry
//! becomes `<seq>.json`. A process that dies between two steps leaves a
//! `.pending` that the next write settles.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::ports::{ApplyError, SoulStore};
use crate::soul::{self, Personality, Read as FileRead, Soul};
use crate::soul_change::{Approval, Change, ChangeKind, HistoryEntry, undo_target, validate};

/// An entry holds two SOULs and short strings: this only stops a runaway file.
const MAX_ENTRY_BYTES: u64 = 64 * 1024;

pub const NO_SOUL: &str = "there is no SOUL to change; a user with root authority builds one with: intent-shell --administrator --setup-soul";

pub struct FileSoulStore {
    /// The SOUL file the shell reads.
    soul: PathBuf,
    /// The personality file the shell would load for this person.
    personality: PathBuf,
}

impl FileSoulStore {
    pub fn new(soul: PathBuf, personality: PathBuf) -> Self {
        Self { soul, personality }
    }

    /// The file that is changed: the SOUL with links followed once, so the write,
    /// the history and the lock all name the same place.
    fn target(&self) -> Result<PathBuf, String> {
        fs::canonicalize(&self.soul).map_err(|e| {
            let at = soul::shown_path(&self.soul);
            if e.kind() != std::io::ErrorKind::NotFound {
                format!("cannot reach the SOUL {at}: {e}")
            } else if self.soul.symlink_metadata().is_ok() {
                format!("the SOUL {at} is a link to something that does not exist")
            } else {
                NO_SOUL.to_owned()
            }
        })
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    match soul::read(path, "SOUL", true)? {
        FileRead::Text(text) => Ok(text),
        FileRead::Problem(problem) => Err(problem),
        FileRead::Absent => Err(NO_SOUL.to_owned()),
    }
}

fn history_dir(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".history");
    target.with_file_name(name)
}

fn entry_name(seq: u32, extension: &str) -> String {
    format!("{seq:08}.{extension}")
}

/// The number in `00000007.json`, or `None` for any other name.
fn numbered(name: &str, extension: &str) -> Option<u32> {
    let stem = name.strip_suffix(&format!(".{extension}"))?;
    (stem.len() == 8 && stem.bytes().all(|b| b.is_ascii_digit()))
        .then(|| stem.parse().ok())
        .flatten()
}

fn read_entry(path: &Path) -> Result<HistoryEntry, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err("it is too large".to_owned());
    }
    let entry: HistoryEntry = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if entry.before.len() > soul::MAX_BYTES || entry.after.len() > soul::MAX_BYTES {
        return Err("it holds a SOUL larger than a SOUL can be".to_owned());
    }
    Ok(entry)
}

/// The committed entries, oldest first. They must be numbered 1, 2, 3... with
/// none missing, each file's number must be the entry's own, and every file
/// named `*.json` must be an entry.
fn read_history(dir: &Path) -> Result<Vec<HistoryEntry>, String> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(format!(
                "cannot read the history {}: {e}",
                soul::shown_path(dir)
            ));
        }
    };
    let mut numbers = Vec::new();
    for item in listing {
        let item = item.map_err(|e| format!("cannot read the history: {e}"))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") {
            let number = numbered(&name, "json").ok_or_else(|| {
                format!(
                    "the history is damaged: {} is not an entry name",
                    soul::shown_path(Path::new(&name))
                )
            })?;
            numbers.push(number);
        }
    }
    numbers.sort_unstable();
    let mut entries = Vec::new();
    for (index, number) in numbers.into_iter().enumerate() {
        let expected = index as u32 + 1;
        if number != expected {
            return Err(format!(
                "the history is damaged: entry {expected} is missing"
            ));
        }
        let name = entry_name(number, "json");
        let entry = read_entry(&dir.join(&name)).map_err(|why| {
            format!("the history is damaged: entry {number} cannot be used ({why})")
        })?;
        if entry.seq != number {
            return Err(format!(
                "the history is damaged: file {number} holds entry {}",
                entry.seq
            ));
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// Written whole and synced before it is named for what it is.
fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)?;
    // The mode is the machine's, whatever the umask: every user reads the SOUL.
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Best effort where a filesystem does not sync a directory.
fn sync_directory(dir: &Path) {
    let _ = File::open(dir).and_then(|d| d.sync_all());
}

/// The exclusive lock, held until it is dropped or the process ends.
struct Lock {
    _file: File,
}

impl Lock {
    fn acquire(path: &Path) -> Result<Self, ApplyError> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o644)
            .open(path)
            .map_err(|e| ApplyError::Failed(format!("cannot open the lock: {e}")))?;
        // SAFETY: the descriptor belongs to `file`, which is open for the whole call.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if locked == 0 {
            return Ok(Self { _file: file });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Err(ApplyError::Locked)
        } else {
            Err(ApplyError::Failed(format!("cannot lock the SOUL: {error}")))
        }
    }
}

/// What a process that died between two steps left (ADR 0036, section 6). Under
/// the lock, so nothing here belongs to a writer that is still running.
fn settle(dir: &Path, target: &Path, current: &str) -> Result<(), ApplyError> {
    if let Some(parent) = target.parent()
        && let Ok(listing) = fs::read_dir(parent)
    {
        for item in listing.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with(".soul-change-") && name.ends_with(".tmp") {
                let _ = fs::remove_file(item.path());
            }
        }
    }
    let mut next = read_history(dir).map_err(ApplyError::Damaged)?.len() as u32 + 1;
    let mut pending: Vec<u32> = fs::read_dir(dir)
        .map_err(|e| ApplyError::Failed(format!("cannot read the history: {e}")))?
        .flatten()
        .filter_map(|item| numbered(&item.file_name().to_string_lossy(), "pending"))
        .collect();
    pending.sort_unstable();
    for number in pending {
        let path = dir.join(entry_name(number, "pending"));
        // It counts only if it is whole, is the next number, and the SOUL is
        // what it says the SOUL became: then the process died after the swap.
        let applied = read_entry(&path)
            .is_ok_and(|e| e.seq == number && number == next && e.after == current);
        if applied && fs::rename(&path, dir.join(entry_name(number, "json"))).is_ok() {
            next += 1;
        } else {
            let _ = fs::remove_file(&path);
        }
    }
    sync_directory(dir);
    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl SoulStore for FileSoulStore {
    fn current(&self) -> Result<String, String> {
        read_text(&self.target()?)
    }

    fn history(&self) -> Result<Vec<HistoryEntry>, String> {
        read_history(&history_dir(&self.target()?))
    }

    fn personality_effect(&self, soul: &Soul) -> String {
        let at = soul::shown_path(&self.personality);
        match soul::read(&self.personality, "personality", false) {
            Ok(FileRead::Absent) => {
                "you have no personality file, so nothing of yours changes".to_owned()
            }
            Ok(FileRead::Problem(problem)) => {
                format!("your personality cannot be used now: {problem}")
            }
            Ok(FileRead::Text(text)) => match Personality::parse(&text).and_then(|p| soul.with(&p))
            {
                Ok(_) => format!("your personality {at} stays valid under this SOUL"),
                Err(why) => format!(
                    "your personality {at} would be dropped at the next start: {why}. The file is not changed"
                ),
            },
            Err(why) => format!("your personality cannot be used now: {why}"),
        }
    }

    fn apply(&self, change: &Change, approval: &Approval) -> Result<u32, ApplyError> {
        let target = self.target().map_err(ApplyError::Failed)?;
        let dir = history_dir(&target);
        fs::create_dir_all(&dir).map_err(|e| {
            ApplyError::Failed(format!("cannot create {}: {e}", soul::shown_path(&dir)))
        })?;
        let _lock = Lock::acquire(&dir.join(".lock"))?;
        let current = read_text(&target).map_err(ApplyError::Failed)?;
        settle(&dir, &target, &current)?;
        if current != change.before {
            return Err(ApplyError::Stale);
        }
        let entries = read_history(&dir).map_err(ApplyError::Damaged)?;
        if change.kind == ChangeKind::Undo {
            match undo_target(&entries) {
                Ok(Some(open))
                    if Some(open.seq) == change.reverts
                        && open.after == current
                        && open.before == change.after => {}
                Ok(_) => return Err(ApplyError::Stale),
                Err(why) => return Err(ApplyError::Damaged(why)),
            }
        }
        validate(&change.after).map_err(ApplyError::Failed)?;
        let seq = entries.last().map_or(0, |e| e.seq) + 1;
        let entry = HistoryEntry {
            seq,
            kind: change.kind,
            at: unix_seconds(),
            by: approval.by().to_owned(),
            rationale: change.rationale.clone(),
            reverts: change.reverts,
            before: change.before.clone(),
            after: change.after.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&entry)
            .map_err(|e| ApplyError::Failed(format!("cannot write the history: {e}")))?;
        let pending = dir.join(entry_name(seq, "pending"));
        write_synced(&pending, &bytes)
            .map_err(|e| ApplyError::Failed(format!("cannot write the history: {e}")))?;
        sync_directory(&dir);
        swap(&target, &change.after).map_err(|e| {
            let _ = fs::remove_file(&pending);
            ApplyError::Failed(format!("cannot write the SOUL: {e}; nothing was changed"))
        })?;
        fs::rename(&pending, dir.join(entry_name(seq, "json"))).map_err(|e| {
            ApplyError::Failed(format!(
                "the SOUL was changed but its history entry could not be committed ({e}); the next change settles it"
            ))
        })?;
        sync_directory(&dir);
        Ok(seq)
    }
}

/// The new text goes to a file beside the SOUL and is renamed over it, so a
/// reader sees the old file or the new one and a crash leaves one of them.
fn swap(target: &Path, text: &str) -> std::io::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let temporary =
        target.with_file_name(format!(".soul-change-{}-{nanos}.tmp", std::process::id()));
    let written =
        write_synced(&temporary, text.as_bytes()).and_then(|()| fs::rename(&temporary, target));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    } else if let Some(parent) = target.parent() {
        sync_directory(parent);
    }
    written
}
