//! The development machine as a `Host`: the user's own Unix permissions, and
//! a small set of real, read-only actions.
//!
//! Actions that change anything are validated, shown and confirmed like the
//! others, but this prototype does not run them: nothing here writes, deletes,
//! starts a program or touches accounts (S3 scope).

use std::ffi::CString;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::action::{Access, Action, is_hidden, quoted_path};
use crate::ports::Entry;
use crate::ports::{EntryKind, Found, Host, Output, Search, TREE_DEPTH, TreeEntry};
use crate::reference::Place;

const MAX_READ_BYTES: u64 = 64 * 1024;
const MAX_WALKED_ENTRIES: usize = 100_000;
/// Entries a listing gathers for the tree view, so a huge folder cannot stall it.
const MAX_TREE_ENTRIES: usize = 5_000;

pub struct UnixHost {
    home: Option<PathBuf>,
}

impl UnixHost {
    pub fn new(home: Option<PathBuf>) -> Self {
        Self { home }
    }

    fn is_protected(&self, path: &Path) -> bool {
        path.parent().is_none()
            || Place::ALL
                .into_iter()
                .flat_map(|place| self.place_roots(place))
                .any(|root| root == path)
    }

    fn tree_entry(&self, path: PathBuf, meta: &fs::Metadata) -> TreeEntry {
        let file_type = meta.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Link(fs::read_link(&path).unwrap_or_default())
        } else if file_type.is_dir() {
            EntryKind::Dir
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        let readable = if file_type.is_dir() {
            permitted(&path, libc::R_OK | libc::X_OK)
        } else {
            permitted(&path, libc::R_OK)
        };
        TreeEntry {
            size: file_type.is_file().then_some(meta.len()),
            protected: self.is_protected(&path),
            owner: owner_name(meta.uid()),
            created: meta
                .created()
                .or_else(|_| meta.modified())
                .ok()
                .and_then(local_time),
            kind,
            readable,
            path,
        }
    }

    /// The direct children, which `from_step` may use, and what lies below
    /// them down to the tree's depth, which only the display may use.
    fn list(&self, path: &Path) -> Result<Output, String> {
        self.list_capped(path, MAX_TREE_ENTRIES)
    }

    fn list_capped(&self, path: &Path, cap: usize) -> Result<Output, String> {
        refuse_hidden_link(path)?;
        // Not the whole folder: a huge one must not stall or fill the memory.
        let mut children: Vec<PathBuf> = fs::read_dir(path)
            .map_err(|e| io_error(path, e))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .take(cap + 1)
            .collect();
        let listing_cut = children.len() > cap;
        children.truncate(cap);
        children.sort();
        let mut tree = Vec::new();
        // Level by level, so a huge first folder cannot crowd out its siblings.
        let mut pending: std::collections::VecDeque<(PathBuf, usize)> =
            children.iter().map(|c| (c.clone(), 1)).collect();
        let mut capped = false;
        while let Some((child, level)) = pending.pop_front() {
            if tree.len() >= cap {
                capped = true;
                break;
            }
            let Ok(meta) = fs::symlink_metadata(&child) else {
                continue;
            };
            let entry = self.tree_entry(child.clone(), &meta);
            // A hidden folder is shown, not opened, and a link is never followed.
            let opens = level < TREE_DEPTH
                && entry.kind == EntryKind::Dir
                && entry.readable
                && !is_hidden(Path::new(child.file_name().unwrap_or_default()));
            tree.push(entry);
            if opens && let Ok(below) = fs::read_dir(&child) {
                // At most what the cap can still take, so a huge folder is not read whole.
                let room = cap.saturating_sub(tree.len() + pending.len()).max(1);
                let mut names: Vec<PathBuf> = below
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .take(room + 1)
                    .collect();
                if names.len() > room {
                    names.truncate(room);
                    capped = true;
                }
                names.sort();
                pending.extend(names.into_iter().map(|n| (n, level + 1)));
            }
        }
        let mut notes = Vec::new();
        if listing_cut {
            notes.push(format!(
                "the listing stopped after {cap} entries, the first the system returned: more exist"
            ));
        } else if capped {
            notes.push(format!("the tree stopped after {cap} entries: more exist"));
        }
        Ok(Output {
            found: children
                .into_iter()
                .map(|path| Found { path, size: None })
                .collect(),
            tree,
            notes,
            ..Output::default()
        })
    }

    pub fn from_environment() -> Self {
        Self::new(std::env::var_os("HOME").map(PathBuf::from))
    }
}

/// The name of the user with this id, when the system knows it.
fn owner_name(uid: u32) -> Option<String> {
    // Longer entries ask for a bigger buffer by failing with ERANGE.
    for size in [1024, 16 * 1024, 256 * 1024] {
        let mut buffer = vec![0 as libc::c_char; size];
        // SAFETY: an all-zero `passwd` is a valid value for the C call to fill in.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: `entry`, `buffer` and `found` are live for the call and
        // `buffer.len()` is its true size; `getpwuid_r` writes only inside them.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut found,
            )
        };
        if status == libc::ERANGE {
            continue;
        }
        if status != 0 || found.is_null() {
            break;
        }
        // SAFETY: on success `pw_name` points to a NUL-terminated string inside `buffer`.
        let name = unsafe { std::ffi::CStr::from_ptr(entry.pw_name) };
        return Some(name.to_string_lossy().into_owned());
    }
    Some(uid.to_string())
}

/// A moment as the person's clock shows it, `2026-11-17 14:30`.
fn local_time(moment: std::time::SystemTime) -> Option<String> {
    let seconds = moment.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let seconds = libc::time_t::try_from(seconds).ok()?;
    // SAFETY: an all-zero `tm` is a valid value for the C call to fill in.
    let mut broken: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are live for the call; `localtime_r` is thread-safe.
    if unsafe { libc::localtime_r(&seconds, &mut broken) }.is_null() {
        return None;
    }
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        broken.tm_year + 1900,
        broken.tm_mon + 1,
        broken.tm_mday,
        broken.tm_hour,
        broken.tm_min
    ))
}

fn permitted(path: &Path, mode: libc::c_int) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call,
    // and `access` only reads it.
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

impl Host for UnixHost {
    fn place_roots(&self, place: Place) -> Vec<PathBuf> {
        let Some(home) = self.home() else {
            return Vec::new();
        };
        let folders: Vec<PathBuf> = match place {
            Place::Home => vec![home.clone()],
            Place::Downloads => vec![home.join("Downloads")],
            Place::Documents => vec![home.join("Documents")],
            Place::Desktop => vec![home.join("Desktop")],
            Place::Pictures => vec![home.join("Pictures")],
            Place::Music => vec![home.join("Music")],
            // macOS calls it Movies. Both count, so neither can be removed whole.
            Place::Videos => vec![home.join("Videos"), home.join("Movies")],
        };
        // The real folders, so a place that is a link is judged and shown where it leads.
        folders
            .into_iter()
            .filter_map(|p| fs::canonicalize(p).ok())
            .filter(|p| p.is_dir())
            .collect()
    }

    fn home(&self) -> Option<PathBuf> {
        // A relative home would make `~` depend on the working directory.
        let home = self.home.as_ref().filter(|h| h.is_absolute())?;
        Some(fs::canonicalize(home).unwrap_or_else(|_| home.clone()))
    }

    fn canonicalize(&self, path: &Path, follow_last: bool) -> PathBuf {
        if follow_last {
            if let Ok(real) = fs::canonicalize(path) {
                return real;
            }
            // A link whose target does not exist yet still leads somewhere:
            // writing to it creates the target, so that is the location.
            if let Some(target) = dangling_target(path) {
                return self.canonicalize(&target, true);
            }
        }
        // Resolve the longest part that exists and can be reached; the rest
        // stays as written, and whether the user may touch it is the broker's.
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        let mut head = path.to_path_buf();
        while let Some(name) = head.file_name().map(ToOwned::to_owned) {
            tail.push(name);
            head.pop();
            if let Ok(real) = fs::canonicalize(&head) {
                head = real;
                break;
            }
        }
        head.extend(tail.iter().rev());
        head
    }

    fn is_dir(&self, path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|m| m.is_dir())
    }

    fn is_symlink(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }

    fn same(&self, a: &Path, b: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(x), Ok(y)) => (x.dev(), x.ino()) == (y.dev(), y.ino()),
            // Neither exists (yet): a case-insensitive volume would fold these,
            // so differing only in case counts as the same.
            _ => a == b || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase(),
        }
    }

    fn exists(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    fn search(&self, roots: &[PathBuf], needle: &str) -> Result<Search, String> {
        search(roots, needle, MAX_WALKED_ENTRIES)
    }

    fn allows(&self, access: Access, path: &Path) -> bool {
        match access {
            // The root has no directory above it to remove it from, whoever asks.
            Access::Write | Access::Delete if path.parent().is_none() => false,
            // Reaching an entry needs search permission on the directory that holds it.
            Access::Stat => path.parent().is_none() || permitted(parent_of(path), libc::X_OK),
            Access::Read if path.is_dir() => permitted(path, libc::R_OK | libc::X_OK),
            Access::Read => permitted(path, libc::R_OK),
            Access::Write if path.exists() => permitted(path, libc::W_OK),
            Access::Write | Access::Delete => permitted(parent_of(path), libc::W_OK | libc::X_OK),
        }
    }

    fn execute(&self, action: &Action) -> Result<Output, String> {
        match action {
            Action::List { path } => self.list(path),
            Action::Stat { path } => stat(path),
            Action::Read { path } => read(path, MAX_READ_BYTES),
            Action::FindLargest { path, count } => {
                find_largest(path, *count as usize, MAX_WALKED_ENTRIES)
            }
            other => Err(format!(
                "{} is not executable in this prototype",
                other.name()
            )),
        }
    }
}

/// The name only: errors reach the default view, which shows no location. The
/// exact action, with its location, is in the log.
fn io_error(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", quoted_name(path))
}

fn quoted_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "\"/\"".to_owned(), |name| quoted_path(Path::new(name)))
}

fn stat(path: &Path) -> Result<Output, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    use std::os::unix::fs::FileTypeExt;
    let file_type = meta.file_type();
    let kind = if file_type.is_dir() {
        "directory"
    } else if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_file() {
        "file"
    } else if file_type.is_fifo() {
        "fifo"
    } else if file_type.is_socket() {
        "socket"
    } else {
        "device"
    };
    let text = format!(
        "{kind} size={} mode={:o} uid={} gid={}",
        meta.len(),
        meta.mode() & 0o7777,
        meta.uid(),
        meta.gid()
    );
    Ok(Output {
        text,
        ..Output::default()
    })
}

/// Where a link that leads nowhere yet points, as an absolute path, following
/// a chain of such links a few steps.
fn dangling_target(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..8 {
        let target = fs::read_link(&current).ok()?;
        let absolute = if target.is_absolute() {
            target
        } else {
            current.parent()?.join(target)
        };
        if fs::symlink_metadata(&absolute).is_err() || fs::canonicalize(&absolute).is_ok() {
            return Some(absolute);
        }
        current = absolute;
    }
    None
}

/// A plain name can be a link to a hidden place (`notes.txt` to `.ssh/id_rsa`),
/// and looking at a plain name is not asked about (ADR 0016). A path that came
/// from a listing or a search was never followed, so the host refuses what the
/// broker could not see.
fn refuse_hidden_link(path: &Path) -> Result<(), String> {
    if !is_hidden(path)
        && let Ok(real) = fs::canonicalize(path)
        && is_hidden(&real)
    {
        return Err(format!(
            "{} leads to a hidden location; ask for it by its own name",
            quoted_name(path)
        ));
    }
    Ok(())
}

fn read(path: &Path, limit: u64) -> Result<Output, String> {
    use std::os::unix::fs::OpenOptionsExt;
    refuse_hidden_link(path)?;
    // Opening a named pipe or a device can block forever, so the open does not
    // wait, and what was opened (not what the name was a moment ago) must be a
    // regular file.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| io_error(path, e))?;
    if !file.metadata().map_err(|e| io_error(path, e))?.is_file() {
        return Err(format!("{} is not a regular file", quoted_name(path)));
    }
    let mut bytes = Vec::new();
    // One byte more than the limit tells a file that fits from one that was cut.
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(path, e))?;
    let mut notes = Vec::new();
    if bytes.len() as u64 > limit {
        bytes.truncate(limit as usize);
        notes.push(format!("only the first {limit} bytes were read"));
    }
    // File content is data for the terminal, not for the terminal to obey.
    let text = String::from_utf8_lossy(&bytes)
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '?'
            } else {
                c
            }
        })
        .collect();
    Ok(Output {
        text,
        notes,
        ..Output::default()
    })
}

fn find_largest(root: &Path, count: usize, walk_limit: usize) -> Result<Output, String> {
    refuse_hidden_link(root)?;
    let root_is_hidden = is_hidden(root);
    use std::os::unix::fs::MetadataExt;
    let mut files: Vec<(u64, PathBuf)> = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    let mut walked = 0usize;
    let mut truncated = false;
    let mut unreadable = 0usize;
    // Hard links name one file more than once; it counts once.
    let mut linked: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if dir == root => return Err(io_error(&dir, e)),
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            walked += 1;
            if walked > walk_limit {
                truncated = true;
                break;
            }
            // Hidden entries are not looked into unless the root is hidden itself:
            // a ranking of a plain folder asks no question (ADR 0016).
            if !root_is_hidden && entry.file_name().as_encoded_bytes().first() == Some(&b'.') {
                continue;
            }
            // `file_type` does not follow symlinks, so the walk stays inside `root`.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                match entry.metadata() {
                    Ok(meta) if meta.nlink() > 1 && !linked.insert((meta.dev(), meta.ino())) => {}
                    Ok(meta) => files.push((meta.len(), entry.path())),
                    Err(_) => unreadable += 1,
                }
            }
        }
        if truncated {
            break;
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files.truncate(count);
    let mut notes = Vec::new();
    if truncated {
        notes.push(format!(
            "stopped after {walk_limit} entries: larger files may exist"
        ));
    }
    if unreadable > 0 {
        notes.push(format!(
            "{unreadable} entries could not be read: larger files may exist"
        ));
    }
    Ok(Output {
        found: files
            .into_iter()
            .map(|(size, path)| Found {
                path,
                size: Some(size),
            })
            .collect(),
        notes,
        ..Output::default()
    })
}

/// Names that contain `needle` below each root, looking only into directories
/// the user may list. Says when it could not look at everything.
fn search(roots: &[PathBuf], needle: &str, walk_limit: usize) -> Result<Search, String> {
    let needle = needle.to_lowercase();
    let mut entries = Vec::new();
    // Breadth first, so a deep tree cannot use up the budget before the shallow places are seen.
    let mut pending: std::collections::VecDeque<PathBuf> = roots.iter().cloned().collect();
    let (mut walked, mut unreadable, mut truncated) = (0usize, 0usize, false);
    while let Some(dir) = pending.pop_front() {
        let Ok(listing) = fs::read_dir(&dir) else {
            unreadable += 1;
            continue;
        };
        for entry in listing.flatten() {
            walked += 1;
            if walked > walk_limit {
                truncated = true;
                break;
            }
            // `file_type` does not follow symlinks, so the walk stays inside the roots.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if name.contains(&needle) {
                entries.push(Entry {
                    path: path.clone(),
                    // A link to a folder is a folder to enter, though never walked.
                    is_dir: kind.is_dir()
                        || (kind.is_symlink() && fs::metadata(&path).is_ok_and(|m| m.is_dir())),
                });
            }
            if kind.is_dir() {
                pending.push_back(path);
            }
        }
        if truncated {
            break;
        }
    }
    let mut notes = Vec::new();
    if truncated {
        notes.push(format!(
            "stopped after {walk_limit} entries: more matches may exist"
        ));
    }
    if unreadable > 0 {
        notes.push(format!(
            "{unreadable} folders could not be searched: more matches may exist"
        ));
    }
    Ok(Search { entries, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("intent-shell-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Where a link such as `/var` leads, as `canonicalize` will report it.
        fs::canonicalize(dir).unwrap()
    }

    fn host() -> UnixHost {
        UnixHost::new(Some(PathBuf::from("/home/ana")))
    }

    #[test]
    fn finds_the_largest_files_below_a_directory_largest_first() {
        let dir = temp_dir("largest");
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("small"), vec![0; 10]).unwrap();
        fs::write(dir.join("sub/big"), vec![0; 300]).unwrap();
        fs::write(dir.join("medium"), vec![0; 100]).unwrap();
        let output = host()
            .execute(&Action::FindLargest {
                path: dir.clone(),
                count: 2,
            })
            .unwrap();
        assert_eq!(
            output.paths().cloned().collect::<Vec<_>>(),
            vec![dir.join("sub/big"), dir.join("medium")]
        );
        assert_eq!(output.found[0].size, Some(300));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_subdirectory_is_reported_not_silently_skipped() {
        let dir = temp_dir("locked");
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("big"), vec![0; 500]).unwrap();
        fs::write(dir.join("small"), "x").unwrap();
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o0)).unwrap();
        let output = host()
            .execute(&Action::FindLargest {
                path: dir.clone(),
                count: 1,
            })
            .unwrap();
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(output.notes.len(), 1, "{output:?}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_largest_does_not_follow_a_symlink_out_of_the_directory() {
        let dir = temp_dir("symlink");
        let outside = temp_dir("symlink-outside");
        fs::write(outside.join("huge"), vec![0; 1000]).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        let output = host()
            .execute(&Action::FindLargest {
                path: dir.clone(),
                count: 5,
            })
            .unwrap();
        assert!(output.found.is_empty());
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn a_plain_name_that_links_to_a_hidden_file_is_not_read() {
        let dir = temp_dir("hidden-link");
        fs::create_dir(dir.join(".ssh")).unwrap();
        fs::write(dir.join(".ssh/id_rsa"), "secret").unwrap();
        std::os::unix::fs::symlink(dir.join(".ssh/id_rsa"), dir.join("notes.txt")).unwrap();
        std::os::unix::fs::symlink(dir.join(".ssh"), dir.join("docs")).unwrap();
        for path in [dir.join("notes.txt"), dir.join("docs/id_rsa")] {
            let error = host().execute(&Action::Read { path }).unwrap_err();
            assert!(error.contains("hidden location"), "{error}");
        }
        let direct = host()
            .execute(&Action::Read {
                path: dir.join(".ssh/id_rsa"),
            })
            .unwrap();
        assert_eq!(direct.text, "secret");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reading_a_named_pipe_is_refused_instead_of_blocking() {
        let dir = temp_dir("fifo");
        let fifo = dir.join("pipe");
        let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(host().execute(&Action::Read { path: fifo }).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_an_error_not_an_empty_result() {
        let dir = temp_dir("missing");
        assert!(
            host()
                .execute(&Action::List {
                    path: dir.join("nope")
                })
                .is_err()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn actions_that_change_things_are_not_executable_here() {
        let dir = temp_dir("readonly");
        let victim = dir.join("keep");
        fs::write(&victim, "x").unwrap();
        let result = host().execute(&Action::Delete {
            path: victim.clone(),
        });
        assert!(result.is_err());
        assert!(victim.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn permissions_of_an_existing_file_and_of_a_directory_follow_its_own_mode() {
        let dir = temp_dir("modes");
        let file = dir.join("f");
        fs::write(&file, "x").unwrap();
        fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o400)).unwrap();
        let sub = dir.join("d");
        fs::create_dir(&sub).unwrap();
        fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        if unsafe { libc::geteuid() } != 0 {
            assert!(!host().allows(Access::Write, &file));
            assert!(host().allows(Access::Read, &file));
            assert!(!host().allows(Access::Read, &sub));
        }
        fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn permissions_follow_the_operating_system() {
        let dir = temp_dir("perm");
        let file = dir.join("f");
        fs::write(&file, "x").unwrap();
        let host = host();
        assert!(host.allows(Access::Read, &file));
        assert!(host.allows(Access::Write, &file));
        assert!(host.allows(Access::Delete, &file));
        fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o500)).unwrap();
        // Root ignores directory permissions, so the denial checks mean nothing for it.
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        let is_root = unsafe { libc::geteuid() } == 0;
        if !is_root {
            assert!(!host.allows(Access::Delete, &file));
            assert!(!host.allows(Access::Write, &dir.join("new")));
        }
        fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode)).unwrap();
    }

    fn is_root() -> bool {
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn a_search_stops_at_its_entry_limit_and_says_so() {
        let dir = temp_dir("walklimit");
        for i in 0..5 {
            fs::write(dir.join(format!("f{i}")), "x").unwrap();
        }
        let output = find_largest(&dir, 1, 3).unwrap();
        assert_eq!(output.notes.len(), 1, "{output:?}");
        assert!(find_largest(&dir, 1, 5).unwrap().notes.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_read_stops_at_its_byte_limit_and_says_so() {
        let dir = temp_dir("readlimit");
        fs::write(dir.join("f"), "x".repeat(100)).unwrap();
        let output = read(&dir.join("f"), 10).unwrap();
        assert_eq!(output.text.len(), 10);
        assert_eq!(output.notes.len(), 1);
        assert!(read(&dir.join("f"), 100).unwrap().notes.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn removing_a_file_needs_the_execute_bit_of_its_directory() {
        let dir = temp_dir("xparent");
        let file = dir.join("f");
        fs::write(&file, "x").unwrap();
        set_mode(&dir, 0o600);
        if !is_root() {
            assert!(!host().allows(Access::Delete, &file));
            assert!(!host().allows(Access::Write, &dir.join("new")));
        }
        set_mode(&dir, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reading_a_directory_needs_the_execute_bit_too() {
        let dir = temp_dir("xbit");
        set_mode(&dir, 0o600);
        if !is_root() {
            assert!(!host().allows(Access::Read, &dir));
        }
        set_mode(&dir, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_root_is_never_a_target_even_for_root() {
        assert!(!host().allows(Access::Write, Path::new("/")));
    }

    #[test]
    fn files_in_a_directory_that_can_be_listed_but_not_entered_are_reported() {
        let dir = temp_dir("listonly");
        let sub = dir.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("huge"), vec![0; 5000]).unwrap();
        fs::write(dir.join("small"), "x").unwrap();
        set_mode(&sub, 0o444);
        let output = host()
            .execute(&Action::FindLargest {
                path: dir.clone(),
                count: 1,
            })
            .unwrap();
        set_mode(&sub, 0o700);
        if !is_root() {
            assert_eq!(output.notes.len(), 1, "{output:?}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hard_linked_file_counts_once_in_a_search() {
        let dir = temp_dir("hardlink");
        fs::write(dir.join("big"), vec![0; 500]).unwrap();
        fs::hard_link(dir.join("big"), dir.join("big2")).unwrap();
        fs::write(dir.join("small"), "x").unwrap();
        let output = find_largest(&dir, 2, 100).unwrap();
        assert_eq!(output.found.len(), 2);
        assert!(output.paths().any(|p| *p == dir.join("small")));
    }

    #[test]
    fn stat_needs_to_reach_a_file_not_to_read_it() {
        let dir = temp_dir("statperm");
        let file = dir.join("f");
        fs::write(&file, "x").unwrap();
        set_mode(&file, 0o000);
        assert!(host().allows(Access::Stat, &file));
        assert!(host().allows(Access::Stat, &dir.join("missing")));
        assert!(host().allows(Access::Stat, Path::new("/")));
        set_mode(&dir, 0o600);
        if !is_root() {
            assert!(!host().allows(Access::Stat, &file));
        }
        set_mode(&dir, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn places_live_under_the_home_directory_and_only_when_they_exist() {
        let home = temp_dir("places");
        fs::create_dir(home.join("Downloads")).unwrap();
        let host = UnixHost::new(Some(home.clone()));
        assert_eq!(
            host.place_roots(Place::Downloads),
            vec![home.join("Downloads")]
        );
        assert_eq!(host.place_roots(Place::Home), vec![home.clone()]);
        assert!(host.place_roots(Place::Music).is_empty());
        assert!(UnixHost::new(None).place_roots(Place::Home).is_empty());
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_name_search_finds_names_that_contain_the_text_ignoring_case() {
        let dir = temp_dir("search");
        fs::create_dir(dir.join("Reports")).unwrap();
        fs::write(dir.join("Reports/Budget-2024.PDF"), "x").unwrap();
        fs::write(dir.join("other.txt"), "x").unwrap();
        let found = search(std::slice::from_ref(&dir), "budget", 100).unwrap();
        assert_eq!(found.entries.len(), 1);
        assert_eq!(found.entries[0].path, dir.join("Reports/Budget-2024.PDF"));
        assert!(!found.entries[0].is_dir);
        assert!(
            search(std::slice::from_ref(&dir), "reports", 100)
                .unwrap()
                .entries[0]
                .is_dir
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_search_does_not_follow_a_symlink_and_reports_what_it_could_not_open() {
        let dir = temp_dir("searchlimits");
        let outside = temp_dir("searchlimits-outside");
        fs::write(outside.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("secret2.txt"), "x").unwrap();
        set_mode(&locked, 0o000);
        let found = search(std::slice::from_ref(&dir), "secret", 100).unwrap();
        set_mode(&locked, 0o700);
        if !is_root() {
            assert!(found.entries.is_empty(), "{found:?}");
            assert_eq!(found.notes.len(), 1, "{found:?}");
        }
        assert!(
            search(std::slice::from_ref(&dir), "link", 100)
                .unwrap()
                .entries
                .len()
                == 1
        );
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn a_name_search_stops_at_its_entry_limit_and_says_so() {
        let dir = temp_dir("searchwalk");
        for i in 0..5 {
            fs::write(dir.join(format!("f{i}")), "x").unwrap();
        }
        assert_eq!(
            search(std::slice::from_ref(&dir), "f", 3)
                .unwrap()
                .notes
                .len(),
            1
        );
        assert!(
            search(std::slice::from_ref(&dir), "f", 5)
                .unwrap()
                .notes
                .is_empty()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_error_names_the_item_and_not_where_it_is() {
        let dir = temp_dir("errname");
        let message = host()
            .execute(&Action::List {
                path: dir.join("secret-name"),
            })
            .unwrap_err();
        assert!(message.contains("secret-name"));
        assert!(
            !message.contains(&dir.to_string_lossy().into_owned()),
            "{message}"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_host_itself_refuses_to_write_or_remove_the_filesystem_root() {
        assert!(!host().allows(Access::Write, Path::new("/")));
        assert!(!host().allows(Access::Delete, Path::new("/")));
    }

    #[test]
    fn a_relative_or_empty_home_gives_no_places() {
        // `src` exists next to the tests, so without the guard it would be a place.
        for home in ["", "src", "relative/home"] {
            let host = UnixHost::new(Some(PathBuf::from(home)));
            for place in Place::ALL {
                assert!(host.place_roots(place).is_empty(), "{home:?} {place:?}");
            }
        }
    }

    #[test]
    fn two_names_for_one_folder_are_the_same_location() {
        let dir = temp_dir("same");
        fs::create_dir(dir.join("real")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
        fs::create_dir(dir.join("other")).unwrap();
        assert!(host().same(&dir.join("real"), &dir.join("alias")));
        assert!(!host().same(&dir.join("real"), &dir.join("other")));
        assert!(host().same(&dir.join("missing"), &dir.join("missing")));
        assert!(host().same(&dir.join("Notes.txt"), &dir.join("notes.txt")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shallow_match_is_found_before_a_deep_tree_uses_up_the_budget() {
        let dir = temp_dir("breadth");
        let deep = dir.join("a/b/c");
        fs::create_dir_all(&deep).unwrap();
        for i in 0..50 {
            fs::write(deep.join(format!("f{i}")), "x").unwrap();
        }
        fs::create_dir(dir.join("docs")).unwrap();
        fs::write(dir.join("docs/report.pdf"), "x").unwrap();
        let found = search(std::slice::from_ref(&dir), "report", 20).unwrap();
        assert_eq!(found.entries.len(), 1, "{found:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_search_ignores_case_in_the_needle_too() {
        let dir = temp_dir("needlecase");
        fs::write(dir.join("report.pdf"), "x").unwrap();
        assert_eq!(
            search(std::slice::from_ref(&dir), "REPORT.PDF", 10)
                .unwrap()
                .entries
                .len(),
            1
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dangling_symlink_counts_as_taken() {
        let dir = temp_dir("dangling");
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("link")).unwrap();
        assert!(host().exists(&dir.join("link")));
        assert!(!host().exists(&dir.join("free")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_largest_on_an_unreadable_root_is_an_error() {
        let dir = temp_dir("unreadable-root");
        set_mode(&dir, 0o000);
        let result = find_largest(&dir, 1, 10);
        set_mode(&dir, 0o700);
        if !is_root() {
            assert!(result.is_err());
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn movies_stand_in_for_videos() {
        let home = temp_dir("movies");
        fs::create_dir(home.join("Movies")).unwrap();
        assert_eq!(
            UnixHost::new(Some(home.clone())).place_roots(Place::Videos),
            vec![home.join("Movies")]
        );
        fs::create_dir(home.join("Videos")).unwrap();
        assert_eq!(
            UnixHost::new(Some(home.clone())).place_roots(Place::Videos),
            vec![home.join("Videos"), home.join("Movies")]
        );
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_path_is_resolved_through_links_except_a_last_link_that_is_to_be_removed() {
        let dir = temp_dir("canonical");
        fs::create_dir(dir.join("real")).unwrap();
        fs::write(dir.join("real/file"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let host = host();
        // A link in the middle always leads where it leads.
        assert_eq!(
            host.canonicalize(&dir.join("link/file"), false),
            dir.join("real/file")
        );
        // A last link is the thing named, unless it is a directory to enter.
        assert_eq!(
            host.canonicalize(&dir.join("link"), false),
            dir.join("link")
        );
        assert_eq!(host.canonicalize(&dir.join("link"), true), dir.join("real"));
        // What does not exist stays as written under what does.
        assert_eq!(
            host.canonicalize(&dir.join("link/new/er"), false),
            dir.join("real/new/er")
        );
        assert_eq!(host.canonicalize(Path::new("/"), true), Path::new("/"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_cannot_be_reached_stays_as_written_for_the_broker_to_refuse() {
        let dir = temp_dir("unreachable");
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("secret"), "x").unwrap();
        set_mode(&locked, 0o000);
        let resolved = host().canonicalize(&locked.join("secret"), true);
        let allowed = host().allows(Access::Read, &locked.join("secret"));
        set_mode(&locked, 0o700);
        assert_eq!(resolved, locked.join("secret"));
        if !is_root() {
            assert!(!allowed);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_home_is_the_real_location_and_only_an_absolute_one() {
        let dir = temp_dir("home");
        std::os::unix::fs::symlink(&dir, dir.with_extension("alias")).unwrap();
        let host = UnixHost::new(Some(dir.with_extension("alias")));
        assert_eq!(host.home(), Some(dir.clone()));
        assert_eq!(UnixHost::new(Some(PathBuf::from("rel"))).home(), None);
        assert_eq!(UnixHost::new(None).home(), None);
        fs::remove_file(dir.with_extension("alias")).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_listing_carries_the_owner_and_the_creation_time_of_each_entry() {
        let dir = temp_dir("owner");
        fs::write(dir.join("notes.txt"), "hello").unwrap();
        let output = host().execute(&Action::List { path: dir.clone() }).unwrap();
        let entry = &output.tree[0];
        let owner = entry.owner.as_deref().unwrap();
        // Taken from `id`, not from `owner_name`, which is what is being checked.
        let expected = std::process::Command::new("id")
            .arg("-un")
            .output()
            .unwrap();
        let expected = String::from_utf8_lossy(&expected.stdout);
        // A uid with no name (a container) is shown as its number.
        let expected = match expected.trim() {
            "" => fs::metadata(&dir).unwrap().uid().to_string(),
            name => name.to_owned(),
        };
        assert_eq!(owner, expected);
        let created = entry.created.as_deref().unwrap();
        // `2026-11-17 14:30`: digits and separators, where only the time is known.
        let shape: String = created
            .chars()
            .map(|c| if c.is_ascii_digit() { '9' } else { c })
            .collect();
        assert_eq!(shape, "9999-99-99 99:99", "{created}");
    }

    #[test]
    fn a_listing_gathers_two_levels_for_the_tree_and_only_the_children_for_references() {
        let dir = temp_dir("tree");
        fs::create_dir_all(dir.join("films/deep/deeper")).unwrap();
        fs::write(dir.join("films/old.mkv"), vec![0; 40]).unwrap();
        fs::write(dir.join("films/deep/x"), "x").unwrap();
        fs::create_dir_all(dir.join(".cache")).unwrap();
        fs::write(dir.join(".cache/hidden-child"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("films"), dir.join("alias")).unwrap();
        fs::write(dir.join("notes.txt"), "hello").unwrap();
        let output = host().execute(&Action::List { path: dir.clone() }).unwrap();
        let children: Vec<_> = output.paths().cloned().collect();
        assert_eq!(
            children,
            [".cache", "alias", "films", "notes.txt"].map(|n| dir.join(n))
        );
        let below: Vec<PathBuf> = output.tree.iter().map(|e| e.path.clone()).collect();
        assert!(below.contains(&dir.join("films/old.mkv")), "{below:?}");
        assert!(below.contains(&dir.join("films/deep")), "{below:?}");
        // Two levels: what is under `deep` is not gathered. A hidden folder is
        // shown but not opened, and a link is never followed.
        assert!(!below.contains(&dir.join("films/deep/x")), "{below:?}");
        assert!(
            !below.contains(&dir.join(".cache/hidden-child")),
            "{below:?}"
        );
        assert!(!below.contains(&dir.join("alias/old.mkv")), "{below:?}");
        let by = |name: &str| {
            output
                .tree
                .iter()
                .find(|e| e.path == dir.join(name))
                .unwrap()
        };
        assert_eq!(by("alias").kind, EntryKind::Link(dir.join("films")));
        assert_eq!(by("films").kind, EntryKind::Dir);
        assert_eq!(by("notes.txt").size, Some(5));
        assert!(by("notes.txt").readable && !by("notes.txt").protected);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_place_and_the_root_are_protected_in_a_listing() {
        let home = temp_dir("protected");
        fs::create_dir(home.join("Downloads")).unwrap();
        let host = UnixHost::new(Some(home.clone()));
        let output = host.execute(&Action::List { path: home.clone() }).unwrap();
        assert!(
            output
                .tree
                .iter()
                .find(|e| e.path == home.join("Downloads"))
                .unwrap()
                .protected
        );
        assert!(host.is_protected(Path::new("/")));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_huge_first_folder_does_not_crowd_the_other_top_level_entries_out_of_the_tree() {
        let dir = temp_dir("bfs");
        fs::create_dir(dir.join("a-big")).unwrap();
        for n in 0..30 {
            fs::write(dir.join(format!("a-big/f{n}")), "").unwrap();
        }
        fs::create_dir(dir.join("z-last")).unwrap();
        fs::write(dir.join("z-last/file"), "x").unwrap();
        let output = host().list_capped(&dir, 10).unwrap();
        let has = |p: PathBuf| output.tree.iter().any(|e| e.path == p);
        assert!(has(dir.join("a-big")) && has(dir.join("z-last")));
        assert_eq!(output.notes.len(), 1, "{:?}", output.notes);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_to_a_hidden_folder_is_not_listed_or_searched_by_its_plain_name() {
        let dir = temp_dir("hidden-link-list");
        fs::create_dir_all(dir.join(".ssh")).unwrap();
        fs::write(dir.join(".ssh/id_rsa"), "secret").unwrap();
        std::os::unix::fs::symlink(dir.join(".ssh"), dir.join("lnk")).unwrap();
        for action in [
            Action::List {
                path: dir.join("lnk"),
            },
            Action::FindLargest {
                path: dir.join("lnk"),
                count: 3,
            },
        ] {
            let error = host().execute(&action).unwrap_err();
            assert!(error.contains("hidden location"), "{error}");
        }
        assert!(
            host()
                .execute(&Action::List {
                    path: dir.join(".ssh")
                })
                .is_ok()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_link_that_leads_nowhere_yet_is_followed_to_where_a_write_would_create_it() {
        let dir = temp_dir("dangling-write");
        fs::create_dir(dir.join(".ssh")).unwrap();
        std::os::unix::fs::symlink(dir.join(".ssh/new_keys"), dir.join("lnk")).unwrap();
        std::os::unix::fs::symlink("lnk", dir.join("chain")).unwrap();
        assert_eq!(
            host().canonicalize(&dir.join("lnk"), true),
            dir.join(".ssh/new_keys")
        );
        assert_eq!(
            host().canonicalize(&dir.join("chain"), true),
            dir.join(".ssh/new_keys")
        );
        // Naming it to remove it still names the link.
        assert_eq!(
            host().canonicalize(&dir.join("lnk"), false),
            dir.join("lnk")
        );
        assert!(host().is_symlink(&dir.join("lnk")));
        assert!(!host().is_symlink(&dir.join(".ssh")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_listing_of_a_huge_folder_stops_at_the_cap_and_says_so() {
        let dir = temp_dir("listcap");
        for n in 0..12 {
            fs::write(dir.join(format!("f{n:02}")), "").unwrap();
        }
        let output = host().list_capped(&dir, 5).unwrap();
        assert_eq!(output.found.len(), 5);
        assert!(
            output.notes[0].contains("listing stopped after 5"),
            "{:?}",
            output.notes
        );
        let all = host().list_capped(&dir, 50).unwrap();
        assert_eq!(all.found.len(), 12);
        assert!(all.notes.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_place_that_is_a_link_is_the_real_folder_it_leads_to() {
        let home = temp_dir("place-link");
        fs::create_dir(home.join(".priv")).unwrap();
        std::os::unix::fs::symlink(home.join(".priv"), home.join("Documents")).unwrap();
        let host = UnixHost::new(Some(home.clone()));
        assert_eq!(host.place_roots(Place::Documents), vec![home.join(".priv")]);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_link_to_a_folder_is_found_as_a_folder_but_not_walked() {
        let dir = temp_dir("search-link");
        fs::create_dir_all(dir.join("real/inside")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("films")).unwrap();
        let found = search(std::slice::from_ref(&dir), "films", 100).unwrap();
        assert_eq!(found.entries.len(), 1);
        assert!(found.entries[0].is_dir);
        let inside = search(std::slice::from_ref(&dir), "inside", 100).unwrap();
        assert_eq!(
            inside.entries.len(),
            1,
            "found once, under real/ and not again under the link"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_ranking_of_a_plain_folder_does_not_look_into_hidden_entries() {
        let dir = temp_dir("rank-hidden");
        fs::create_dir(dir.join(".ssh")).unwrap();
        fs::write(dir.join(".ssh/id_rsa"), vec![0; 900]).unwrap();
        fs::write(dir.join(".hidden-file"), vec![0; 800]).unwrap();
        fs::write(dir.join("plain"), vec![0; 10]).unwrap();
        let rank = |path: PathBuf| {
            host()
                .execute(&Action::FindLargest { path, count: 5 })
                .unwrap()
                .paths()
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(rank(dir.clone()), [dir.join("plain")]);
        // Asking for the hidden folder itself is asked about by the broker, and shows it.
        assert_eq!(rank(dir.join(".ssh")), [dir.join(".ssh/id_rsa")]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_huge_child_is_not_read_whole_for_the_tree() {
        let dir = temp_dir("huge-child");
        fs::create_dir(dir.join("big")).unwrap();
        for n in 0..60 {
            fs::write(dir.join(format!("big/f{n:02}")), "").unwrap();
        }
        let output = host().list_capped(&dir, 10).unwrap();
        assert!(output.tree.len() <= 10, "{}", output.tree.len());
        assert!(!output.notes.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
