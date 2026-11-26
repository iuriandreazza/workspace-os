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

/// An entry, or in a few words of ours why it cannot be used: nothing the file
/// says is repeated, because what a message repeats the file's author chose.
fn read_entry(path: &Path) -> Result<HistoryEntry, &'static str> {
    let mut bytes = Vec::new();
    // Non-blocking and regular files only: a named pipe planted here must not
    // hold the lock for ever (the same care as `soul::read`).
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| {
            if file.metadata()?.is_file() {
                file.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut bytes)
            } else {
                Err(std::io::Error::other("it is not a regular file"))
            }
        })
        .map_err(|_| "it is not a regular file that can be read")?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err("it is too large");
    }
    let entry: HistoryEntry = serde_json::from_slice(&bytes).map_err(|e| match e.classify() {
        serde_json::error::Category::Syntax => "it is not valid JSON",
        serde_json::error::Category::Eof => "it ends too soon",
        serde_json::error::Category::Data => "its fields are not those of an entry",
        serde_json::error::Category::Io => "it cannot be read",
    })?;
    if entry.before.len() > soul::MAX_BYTES || entry.after.len() > soul::MAX_BYTES {
        return Err("it holds a SOUL larger than a SOUL can be");
    }
    Ok(entry)
}

/// The committed entries, oldest first, up to the first thing wrong with the
/// history, which is said. They must be numbered 1, 2, 3... with none missing,
/// each file's number must be the entry's own, and every file named `*.json`
/// must be an entry.
fn read_history_partial(dir: &Path) -> Result<(Vec<HistoryEntry>, Option<String>), String> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), None)),
        Err(e) => {
            return Err(format!(
                "cannot read the history {}: {e}",
                soul::shown_path(dir)
            ));
        }
    };
    let mut numbers = Vec::new();
    let mut problem = None;
    for item in listing {
        let item = item.map_err(|e| format!("cannot read the history: {e}"))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") {
            match numbered(&name, "json") {
                Some(number) => numbers.push(number),
                None => {
                    problem.get_or_insert_with(|| {
                        "a file ending in .json is not named like an entry".to_owned()
                    });
                }
            }
        }
    }
    numbers.sort_unstable();
    let mut entries = Vec::new();
    for (index, number) in numbers.into_iter().enumerate() {
        let expected = index as u32 + 1;
        if number != expected {
            problem.get_or_insert_with(|| format!("entry {expected} is missing"));
            break;
        }
        let entry = match read_entry(&dir.join(entry_name(number, "json"))) {
            Ok(entry) if entry.seq == number => entry,
            Ok(entry) => {
                problem.get_or_insert_with(|| format!("file {number} holds entry {}", entry.seq));
                break;
            }
            Err(why) => {
                problem.get_or_insert_with(|| format!("entry {number} cannot be used: {why}"));
                break;
            }
        };
        entries.push(entry);
    }
    Ok((entries, problem))
}

fn read_history(dir: &Path) -> Result<Vec<HistoryEntry>, String> {
    match read_history_partial(dir)? {
        (entries, None) => Ok(entries),
        (_, Some(problem)) => Err(problem),
    }
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
        // Non-blocking, and only a regular file: a named pipe planted as the
        // lock must not freeze every operation.
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o644)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| ApplyError::Failed(format!("cannot open the lock: {e}")))?;
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            return Err(ApplyError::Failed(
                "the lock is not a regular file".to_owned(),
            ));
        }
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
            if is_temporary_of(&name, target) {
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
        // A SOUL that merely has the same text, written by someone else, is not it.
        let applied = read_entry(&path).is_ok_and(|e| {
            e.seq == number
                && number == next
                && e.after == current
                && e.file_id.is_some()
                && e.file_id == fs::metadata(target).ok().map(|m| file_id(&m))
        });
        if !applied {
            let _ = fs::remove_file(&path);
        } else if fs::rename(&path, dir.join(entry_name(number, "json"))).is_ok() {
            next += 1;
        } else {
            // The change happened: its entry stays where it is for the next try.
            return Err(ApplyError::Failed(
                "a change that was applied has an entry that could not be committed; nothing was changed now"
                    .to_owned(),
            ));
        }
    }
    sync_directory(dir);
    Ok(())
}

/// Named after the SOUL it belongs to, so settling one SOUL never deletes the
/// temporary file of another SOUL in the same directory.
fn temporary_name(target: &Path, nanos: u32) -> String {
    format!(
        ".soul-change.{}.{}.{nanos}.tmp",
        target.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    )
}

/// Whether `name` is a temporary file of `target`: its prefix, then two numbers.
fn is_temporary_of(name: &str, target: &Path) -> bool {
    let prefix = format!(
        ".soul-change.{}.",
        target.file_name().unwrap_or_default().to_string_lossy()
    );
    name.strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .and_then(|numbers| numbers.split_once('.'))
        .is_some_and(|(pid, nanos)| {
            [pid, nanos]
                .iter()
                .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Whether the history directory is there: a link is refused, so nothing is
/// ever written or changed in a place the link leads to.
fn real_directory(dir: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => Err(format!(
            "the history {} is not a plain directory (a link or a file); refused",
            soul::shown_path(dir)
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!(
            "cannot look at the history {}: {e}",
            soul::shown_path(dir)
        )),
    }
}

#[cfg(test)]
thread_local! {
    /// Makes the commit of history entries fail, on this thread.
    static FAIL_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn fail_commits(on: bool) {
    FAIL_COMMIT.with(|f| f.set(on));
}

/// Makes the entry part of the history. The test build can be told to fail here.
fn commit(pending: &Path, entry: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_COMMIT.with(std::cell::Cell::get) {
        return Err(std::io::Error::other("injected"));
    }
    fs::rename(pending, entry)
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
        match self.history_listing()? {
            (entries, None) => Ok(entries),
            (_, Some(problem)) => Err(problem),
        }
    }

    fn history_listing(&self) -> Result<(Vec<HistoryEntry>, Option<String>), String> {
        let target = self.target()?;
        let dir = history_dir(&target);
        // A writer that died between the swap and the commit left a change that
        // already happened: if nobody is writing now, settle it before reading, so
        // it can be seen and undone. Who may not write here just reads what is there.
        if real_directory(&dir)?
            && let Ok(_lock) = Lock::acquire(&dir.join(".lock"))
            && let Ok(current) = read_text(&target)
        {
            let _ = settle(&dir, &target, &current);
        }
        let (entries, problem) = read_history_partial(&dir)?;
        // An entry still waiting after the attempt to settle it: a change may have
        // happened that this list cannot show yet, and undo must not guess.
        let waiting = fs::read_dir(&dir).is_ok_and(|listing| {
            listing
                .flatten()
                .any(|e| numbered(&e.file_name().to_string_lossy(), "pending").is_some())
        });
        let problem = match (problem, waiting) {
            (None, true) => Some(
                "an entry is waiting to be settled; a change may have been applied that this list does not show yet"
                    .to_owned(),
            ),
            (problem, _) => problem,
        };
        Ok((entries, problem))
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
        real_directory(&dir).map_err(ApplyError::Failed)?;
        fs::create_dir_all(&dir).map_err(|e| {
            ApplyError::Failed(format!("cannot create {}: {e}", soul::shown_path(&dir)))
        })?;
        // The directory is the machine's, read by everyone, whatever the umask.
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
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
            file_id: None,
        };
        // The new SOUL is written first, beside the old one, so the entry can say
        // which file it is; the entry is complete before the file takes the SOUL's place.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let temporary = target.with_file_name(temporary_name(&target, nanos));
        let identified = write_synced(&temporary, change.after.as_bytes())
            .and_then(|()| fs::metadata(&temporary))
            .map_err(|e| {
                let _ = fs::remove_file(&temporary);
                ApplyError::Failed(format!("cannot write the SOUL: {e}; nothing was changed"))
            })?;
        let entry = HistoryEntry {
            file_id: Some(file_id(&identified)),
            ..entry
        };
        let bytes = serde_json::to_vec_pretty(&entry)
            .map_err(|e| ApplyError::Failed(format!("cannot write the history: {e}")))?;
        if bytes.len() as u64 > MAX_ENTRY_BYTES {
            let _ = fs::remove_file(&temporary);
            return Err(ApplyError::Failed(
                "the history entry would be larger than the history accepts; nothing was changed"
                    .to_owned(),
            ));
        }
        let pending = dir.join(entry_name(seq, "pending"));
        write_synced(&pending, &bytes).map_err(|e| {
            let _ = fs::remove_file(&temporary);
            ApplyError::Failed(format!("cannot write the history: {e}"))
        })?;
        sync_directory(&dir);
        // A reader sees the old file or the new one, and a crash leaves one of them.
        fs::rename(&temporary, &target).map_err(|e| {
            let _ = fs::remove_file(&temporary);
            let _ = fs::remove_file(&pending);
            ApplyError::Failed(format!("cannot write the SOUL: {e}; nothing was changed"))
        })?;
        if let Some(parent) = target.parent() {
            sync_directory(parent);
        }
        commit(&pending, &dir.join(entry_name(seq, "json"))).map_err(|e| {
            ApplyError::AppliedUncommitted(format!(
                "its history entry {seq} is not committed yet ({e}); the next operation on the SOUL settles it"
            ))
        })?;
        sync_directory(&dir);
        Ok(seq)
    }
}

/// Which file this is, by inode: a device number may change between boots, an
/// inode of a file that stays does not.
fn file_id(meta: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    meta.ino().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soul_change::ChangeKind;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn an_entry_that_could_not_be_committed_is_kept_and_not_deleted() {
        // A directory that cannot be written is no obstacle to the superuser.
        // SAFETY: `geteuid` reads the process's user id and touches no memory.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = std::env::temp_dir().join(format!("intent-shell-settle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let history = dir.join("soul.history");
        fs::create_dir_all(&history).unwrap();
        let soul = dir.join("soul");
        fs::write(&soul, "format: 1\npreset: warm-didactic\nlanguages: any\n").unwrap();
        let current = fs::read_to_string(&soul).unwrap();
        let entry = HistoryEntry {
            seq: 1,
            kind: ChangeKind::Change,
            at: 1,
            by: "ana".into(),
            rationale: None,
            reverts: None,
            before: "format: 1\npreset: direct-technical\nlanguages: any\n".into(),
            after: current.clone(),
            file_id: Some(fs::metadata(&soul).unwrap().ino().to_string()),
        };
        let pending = history.join("00000001.pending");
        fs::write(&pending, serde_json::to_vec(&entry).unwrap()).unwrap();
        fs::set_permissions(&history, fs::Permissions::from_mode(0o555)).unwrap();
        let result = settle(&history, &soul, &current);
        fs::set_permissions(&history, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(ApplyError::Failed(_))), "{result:?}");
        assert!(
            pending.exists(),
            "the entry of a change that happened is kept"
        );
        assert_eq!(settle(&history, &soul, &current), Ok(()));
        assert!(history.join("00000001.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
