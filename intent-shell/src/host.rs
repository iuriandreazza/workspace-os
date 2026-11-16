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
use std::path::{Path, PathBuf};

use crate::action::{Access, Action, quoted_path};
use crate::ports::{Host, Output};

const MAX_READ_BYTES: u64 = 64 * 1024;
const MAX_WALKED_ENTRIES: usize = 100_000;

pub struct UnixHost {
    home: Option<PathBuf>,
    cwd: PathBuf,
}

impl UnixHost {
    pub fn new(home: Option<PathBuf>, cwd: PathBuf) -> Self {
        Self { home, cwd }
    }

    pub fn from_environment() -> std::io::Result<Self> {
        Ok(Self::new(
            std::env::var_os("HOME").map(PathBuf::from),
            std::env::current_dir()?,
        ))
    }
}

fn permitted(path: &Path, mode: libc::c_int) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call,
    // and `access` only reads it.
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

/// Resolve `.` and `..` by name, so the path that is checked and shown is the
/// one that is used, and `..` cannot hide that a path is a directory's parent.
fn lexically_normal(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Above the root is the root.
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

impl Host for UnixHost {
    fn resolve(&self, path: &Path) -> PathBuf {
        let expanded = match (path.strip_prefix("~"), &self.home) {
            (Ok(rest), Some(home)) => home.join(rest),
            _ => path.to_path_buf(),
        };
        lexically_normal(&self.cwd.join(expanded))
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
            Action::List { path } => list(path),
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

fn io_error(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", quoted_path(path))
}

fn list(path: &Path) -> Result<Output, String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(path)
        .map_err(|e| io_error(path, e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    paths.sort();
    let text = paths
        .iter()
        .map(|p| quoted_path(p))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Output {
        text,
        paths,
        ..Output::default()
    })
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
        paths: Vec::new(),
        ..Output::default()
    })
}

fn read(path: &Path, limit: u64) -> Result<Output, String> {
    // Opening a named pipe or a device can block forever, so only regular files.
    if !fs::metadata(path).map_err(|e| io_error(path, e))?.is_file() {
        return Err(format!("{} is not a regular file", quoted_path(path)));
    }
    let mut bytes = Vec::new();
    // One byte more than the limit tells a file that fits from one that was cut.
    fs::File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
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
    let text = files
        .iter()
        .map(|(size, p)| format!("{size} {}", quoted_path(p)))
        .collect::<Vec<_>>()
        .join("\n");
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
        text,
        paths: files.into_iter().map(|(_, p)| p).collect(),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("intent-shell-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn host(cwd: &Path) -> UnixHost {
        UnixHost::new(Some(PathBuf::from("/home/ana")), cwd.to_path_buf())
    }

    #[test]
    fn finds_the_largest_files_below_a_directory_largest_first() {
        let dir = temp_dir("largest");
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("small"), vec![0; 10]).unwrap();
        fs::write(dir.join("sub/big"), vec![0; 300]).unwrap();
        fs::write(dir.join("medium"), vec![0; 100]).unwrap();
        let output = host(&dir)
            .execute(&Action::FindLargest {
                path: dir.clone(),
                count: 2,
            })
            .unwrap();
        assert_eq!(output.paths, vec![dir.join("sub/big"), dir.join("medium")]);
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
        let output = host(&dir)
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
        let output = host(&dir)
            .execute(&Action::FindLargest {
                path: dir.clone(),
                count: 5,
            })
            .unwrap();
        assert!(output.paths.is_empty());
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn reading_a_named_pipe_is_refused_instead_of_blocking() {
        let dir = temp_dir("fifo");
        let fifo = dir.join("pipe");
        let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(host(&dir).execute(&Action::Read { path: fifo }).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_an_error_not_an_empty_result() {
        let dir = temp_dir("missing");
        assert!(
            host(&dir)
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
        let result = host(&dir).execute(&Action::Delete {
            path: victim.clone(),
        });
        assert!(result.is_err());
        assert!(victim.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolves_the_home_shortcut_and_relative_paths() {
        let host = host(Path::new("/work"));
        assert_eq!(
            host.resolve(Path::new("~/Downloads")),
            PathBuf::from("/home/ana/Downloads")
        );
        assert_eq!(host.resolve(Path::new("src")), PathBuf::from("/work/src"));
        assert_eq!(host.resolve(Path::new("/etc")), PathBuf::from("/etc"));
    }

    #[test]
    fn dot_dot_is_resolved_and_the_root_can_never_be_removed() {
        let host = host(Path::new("/work/sub"));
        assert_eq!(host.resolve(Path::new("..")), PathBuf::from("/work"));
        assert_eq!(host.resolve(Path::new("../../..")), PathBuf::from("/"));
        assert_eq!(
            host.resolve(Path::new("a/./b/../c")),
            PathBuf::from("/work/sub/a/c")
        );
        assert!(!host.allows(Access::Delete, Path::new("/")));
        assert!(!host.allows(Access::Write, Path::new("/")));
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
            assert!(!host(&dir).allows(Access::Write, &file));
            assert!(host(&dir).allows(Access::Read, &file));
            assert!(!host(&dir).allows(Access::Read, &sub));
        }
        fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn permissions_follow_the_operating_system() {
        let dir = temp_dir("perm");
        let file = dir.join("f");
        fs::write(&file, "x").unwrap();
        let host = host(&dir);
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
            assert!(!host(&dir).allows(Access::Delete, &file));
            assert!(!host(&dir).allows(Access::Write, &dir.join("new")));
        }
        set_mode(&dir, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reading_a_directory_needs_the_execute_bit_too() {
        let dir = temp_dir("xbit");
        set_mode(&dir, 0o600);
        if !is_root() {
            assert!(!host(&dir).allows(Access::Read, &dir));
        }
        set_mode(&dir, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_root_is_never_a_target_even_for_root() {
        assert!(!host(Path::new("/work")).allows(Access::Write, Path::new("/")));
    }

    #[test]
    fn files_in_a_directory_that_can_be_listed_but_not_entered_are_reported() {
        let dir = temp_dir("listonly");
        let sub = dir.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("huge"), vec![0; 5000]).unwrap();
        fs::write(dir.join("small"), "x").unwrap();
        set_mode(&sub, 0o444);
        let output = host(&dir)
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
        assert_eq!(output.paths.len(), 2);
        assert!(output.paths.contains(&dir.join("small")));
    }

    #[test]
    fn stat_needs_to_reach_a_file_not_to_read_it() {
        let dir = temp_dir("statperm");
        let file = dir.join("f");
        fs::write(&file, "x").unwrap();
        set_mode(&file, 0o000);
        assert!(host(&dir).allows(Access::Stat, &file));
        assert!(host(&dir).allows(Access::Stat, &dir.join("missing")));
        assert!(host(&dir).allows(Access::Stat, Path::new("/")));
        set_mode(&dir, 0o600);
        if !is_root() {
            assert!(!host(&dir).allows(Access::Stat, &file));
        }
        set_mode(&dir, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }
}
