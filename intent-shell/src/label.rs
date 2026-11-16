//! How a location is written for the person (ADR 0018, decision 2).
//!
//! The default views show a location relative to the current directory when it
//! is inside it, as `~/…` when it is elsewhere in the home, and absolute
//! otherwise. The exact absolute location stays in the details view and in the
//! root panel. Two different locations never read the same, because the three
//! forms are different spellings of one absolute path.

use std::path::{Path, PathBuf};

use crate::action::quoted_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Labels {
    cwd: PathBuf,
    home: Option<PathBuf>,
}

impl Default for Labels {
    /// No directory and no home: every location is shown absolute.
    fn default() -> Self {
        Self::new(Path::new("/"), None)
    }
}

impl Labels {
    pub fn new(cwd: &Path, home: Option<PathBuf>) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            home,
        }
    }

    /// The location as text for a screen, relative where that is shorter and
    /// still unambiguous. Not quoted: a tree prints names bare.
    pub fn display(&self, path: &Path) -> PathBuf {
        if path == self.cwd {
            return PathBuf::from(".");
        }
        if let Ok(inside) = path.strip_prefix(&self.cwd) {
            return inside.to_path_buf();
        }
        match self.home.as_deref().map(|home| path.strip_prefix(home)) {
            Some(Ok(inside)) if inside.as_os_str().is_empty() => PathBuf::from("~"),
            Some(Ok(inside)) => Path::new("~").join(inside),
            _ => path.to_path_buf(),
        }
    }

    /// The location quoted like any string from the machine, for a table.
    pub fn of(&self, path: &Path) -> String {
        quoted_path(&self.display(path))
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The current directory itself: `~` or `~/Downloads`, never `.`.
    pub fn cwd_text(&self) -> PathBuf {
        match &self.home {
            Some(home) if &self.cwd == home => PathBuf::from("~"),
            Some(home) => self
                .cwd
                .strip_prefix(home)
                .map_or_else(|_| self.cwd.clone(), |inside| Path::new("~").join(inside)),
            None => self.cwd.clone(),
        }
    }

    /// Outside the person's home: the system's space, which the tree marks.
    pub fn is_system(&self, path: &Path) -> bool {
        !self
            .home
            .as_deref()
            .is_some_and(|home| path.starts_with(home))
    }

    /// A folder above the home, such as `/`: it holds both spaces, so it is
    /// not marked as the system's.
    pub fn holds_home(&self, path: &Path) -> bool {
        self.home
            .as_deref()
            .is_some_and(|home| home.starts_with(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Labels {
        Labels::new(
            Path::new("/home/ana/Downloads"),
            Some(PathBuf::from("/home/ana")),
        )
    }

    #[test]
    fn inside_the_current_directory_a_location_is_relative_to_it() {
        let labels = labels();
        assert_eq!(
            labels.of(Path::new("/home/ana/Downloads/films/a.mkv")),
            r#""films/a.mkv""#
        );
        assert_eq!(labels.of(Path::new("/home/ana/Downloads")), r#"".""#);
    }

    #[test]
    fn elsewhere_in_the_home_it_starts_with_a_tilde() {
        let labels = labels();
        assert_eq!(
            labels.of(Path::new("/home/ana/Documents/r.pdf")),
            r#""~/Documents/r.pdf""#
        );
        assert_eq!(labels.of(Path::new("/home/ana")), r#""~""#);
    }

    #[test]
    fn outside_the_home_it_is_absolute_and_a_system_location() {
        let labels = labels();
        assert_eq!(labels.of(Path::new("/etc/hosts")), r#""/etc/hosts""#);
        assert!(labels.is_system(Path::new("/etc/hosts")));
        assert!(!labels.is_system(Path::new("/home/ana/x")));
        assert!(!labels.is_system(Path::new("/home/ana")));
    }

    #[test]
    fn a_sibling_that_only_shares_a_prefix_is_not_inside() {
        let labels = labels();
        assert_eq!(
            labels.of(Path::new("/home/ana/Downloads2/x")),
            r#""~/Downloads2/x""#
        );
        assert_eq!(
            labels.of(Path::new("/home/anabel/x")),
            r#""/home/anabel/x""#
        );
    }

    #[test]
    fn the_current_directory_is_written_without_a_dot() {
        assert_eq!(labels().cwd_text(), PathBuf::from("~/Downloads"));
        let at_home = Labels::new(Path::new("/home/ana"), Some(PathBuf::from("/home/ana")));
        assert_eq!(at_home.cwd_text(), PathBuf::from("~"));
        assert_eq!(Labels::default().cwd_text(), PathBuf::from("/"));
    }

    #[test]
    fn names_that_are_not_text_stay_different() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let a = PathBuf::from(OsStr::from_bytes(b"/x/\xff"));
        let b = PathBuf::from(OsStr::from_bytes(b"/x/\xfe"));
        assert_ne!(Labels::default().of(&a), Labels::default().of(&b));
    }
}
