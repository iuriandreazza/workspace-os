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

use crate::action::{Access, Action, quoted};
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
        self.cwd.join(expanded)
    }

    fn allows(&self, access: Access, path: &Path) -> bool {
        match access {
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
            Action::Read { path } => read(path),
            Action::FindLargest { path, count } => find_largest(path, *count as usize),
            other => Err(format!(
                "{} is not executable in this prototype",
                other.name()
            )),
        }
    }
}

fn io_error(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", quoted(&path.to_string_lossy()))
}

fn list(path: &Path) -> Result<Output, String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(path)
        .map_err(|e| io_error(path, e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    paths.sort();
    let text = paths
        .iter()
        .map(|p| quoted(&p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Output { text, paths })
}

fn stat(path: &Path) -> Result<Output, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    let kind = if meta.is_dir() {
        "directory"
    } else if meta.is_symlink() {
        "symlink"
    } else {
        "file"
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
    })
}

fn read(path: &Path) -> Result<Output, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_READ_BYTES).read_to_end(&mut bytes))
        .map_err(|e| io_error(path, e))?;
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
        paths: Vec::new(),
    })
}

fn find_largest(root: &Path, count: usize) -> Result<Output, String> {
    let mut files: Vec<(u64, PathBuf)> = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    let mut walked = 0usize;
    let mut truncated = false;
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if dir == root => return Err(io_error(&dir, e)),
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            walked += 1;
            if walked > MAX_WALKED_ENTRIES {
                truncated = true;
                break;
            }
            // `file_type` does not follow symlinks, so the walk stays inside `root`.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file()
                && let Ok(meta) = entry.metadata()
            {
                files.push((meta.len(), entry.path()));
            }
        }
        if truncated {
            break;
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files.truncate(count);
    let mut text = files
        .iter()
        .map(|(size, p)| format!("{size} {}", quoted(&p.to_string_lossy())))
        .collect::<Vec<_>>()
        .join("\n");
    if truncated {
        text.push_str(&format!(
            "\n(stopped after {MAX_WALKED_ENTRIES} entries: the result may miss larger files)"
        ));
    }
    Ok(Output {
        text,
        paths: files.into_iter().map(|(_, p)| p).collect(),
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
}
