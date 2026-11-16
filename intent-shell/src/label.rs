//! Friendly names for real locations (ADR 0014).
//!
//! The default view shows `"report.pdf" (Downloads)`, never a path. Two
//! different locations never get the same label: when they would, more of the
//! folders above them are added until they differ.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::action::{quoted, quoted_path};
use crate::ports::Host;
use crate::reference::Place;

/// A place and the folder it lives in, from the host.
struct Root {
    /// What the person sees: the place, or for a place that lives in several
    /// folders the name of the folder, so the two are never mistaken.
    title: String,
    path: PathBuf,
}

/// The label of each location that will be shown, and the real location
/// behind it for the details view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Labels(BTreeMap<PathBuf, String>);

impl Labels {
    pub fn build<'a>(host: &dyn Host, paths: impl IntoIterator<Item = &'a Path>) -> Self {
        let roots = roots(host);
        let paths: BTreeSet<PathBuf> = paths.into_iter().map(Path::to_path_buf).collect();
        let mut depth: BTreeMap<PathBuf, usize> = paths
            .iter()
            .map(|p| (p.clone(), first_depth(&roots, p)))
            .collect();
        // Each round adds one folder to every label that still collides; a
        // location has finitely many folders, so this ends.
        loop {
            let mut groups: BTreeMap<String, Vec<&PathBuf>> = BTreeMap::new();
            for path in &paths {
                groups
                    .entry(render(&roots, path, depth[path]))
                    .or_default()
                    .push(path);
            }
            let growable: Vec<PathBuf> = groups
                .values()
                .filter(|group| group.len() > 1)
                .flatten()
                .filter(|p| depth[**p] < folders(&roots, p).len())
                .map(|p| (*p).clone())
                .collect();
            if growable.is_empty() {
                break;
            }
            for path in growable {
                *depth.entry(path).or_default() += 1;
            }
        }
        Self(
            paths
                .iter()
                .map(|p| (p.clone(), render(&roots, p, depth[p])))
                .collect(),
        )
    }

    /// The label to show for `path`. A location nobody asked to label is shown
    /// as the unlabelled place it is, never as its path.
    pub fn of(&self, path: &Path) -> String {
        self.0
            .get(path)
            .cloned()
            .unwrap_or_else(|| format!("{} (elsewhere)", quoted_name(path)))
    }
}

fn roots(host: &dyn Host) -> Vec<Root> {
    Place::ALL
        .into_iter()
        .filter(|place| *place != Place::Everywhere)
        .flat_map(|place| {
            let folders = host.place_roots(place);
            let several = folders.len() > 1;
            folders.into_iter().map(move |path| Root {
                title: match path.file_name() {
                    Some(name) if several => name.to_string_lossy().into_owned(),
                    _ => place.title().to_owned(),
                },
                path,
            })
        })
        .collect()
}

/// The most specific place that holds `path`.
fn home_of<'a>(roots: &'a [Root], path: &Path) -> Option<&'a Root> {
    roots
        .iter()
        .filter(|r| path.starts_with(&r.path) && path != r.path)
        .max_by_key(|r| r.path.components().count())
}

/// The item's own name, byte-exact like `quoted_path`, so names that differ
/// only in bytes that are not UTF-8 stay different.
fn quoted_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| quoted("/"), |name| quoted_path(Path::new(name)))
}

/// The folders between the place (or the filesystem root) and the item.
fn folders(roots: &[Root], path: &Path) -> Vec<String> {
    let base = home_of(roots, path).map_or(Path::new("/"), |r| r.path.as_path());
    let parent = path.parent().unwrap_or(Path::new("/"));
    parent
        .strip_prefix(base)
        .unwrap_or(parent)
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(quoted_path(Path::new(name))),
            _ => None,
        })
        .collect()
}

/// A place gives a place name and one folder for free; elsewhere gives none.
fn first_depth(roots: &[Root], path: &Path) -> usize {
    usize::from(home_of(roots, path).is_some()).min(folders(roots, path).len())
}

fn render(roots: &[Root], path: &Path, depth: usize) -> String {
    if let Some(root) = roots.iter().find(|r| r.path == path) {
        return root.title.clone();
    }
    let all = folders(roots, path);
    let shown = &all[all.len() - depth.min(all.len())..];
    let home = home_of(roots, path);
    let place = home.map_or("elsewhere", |r| r.title.as_str());
    let mut container = vec![place.to_owned()];
    // Elsewhere hides every folder until two labels need telling apart.
    if shown.len() < all.len() && (home.is_some() || !shown.is_empty()) {
        container.push("…".to_owned());
    }
    container.extend(shown.iter().cloned());
    format!("{} ({})", quoted_name(path), container.join(" › "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;

    fn host() -> FakeHost {
        FakeHost::default()
            .place(Place::Downloads, "/home/ana/Downloads")
            .place(Place::Documents, "/home/ana/Documents")
            .place(Place::Home, "/home/ana")
    }

    fn label(paths: &[&str]) -> Vec<String> {
        let host = host();
        let labels = Labels::build(&host, paths.iter().map(|p| Path::new(*p)));
        paths.iter().map(|p| labels.of(Path::new(p))).collect()
    }

    #[test]
    fn a_label_names_the_item_and_its_place_and_no_path() {
        assert_eq!(
            label(&["/home/ana/Downloads/big.iso"]),
            [r#""big.iso" (Downloads)"#]
        );
        assert_eq!(
            label(&["/home/ana/Documents/taxes/a.pdf"]),
            [r#""a.pdf" (Documents › "taxes")"#]
        );
        assert_eq!(label(&["/etc/hosts"]), [r#""hosts" (elsewhere)"#]);
    }

    #[test]
    fn a_place_in_two_folders_does_not_mix_them_up() {
        let mut host = FakeHost::default();
        host.places.insert(
            Place::Videos,
            vec![PathBuf::from("/h/Videos"), PathBuf::from("/h/Movies")],
        );
        let labels = Labels::build(
            &host,
            [Path::new("/h/Videos/a/log"), Path::new("/h/Movies/a/log")],
        );
        assert_ne!(
            labels.of(Path::new("/h/Videos/a/log")),
            labels.of(Path::new("/h/Movies/a/log"))
        );
    }

    #[test]
    fn the_most_specific_place_wins() {
        assert_eq!(
            label(&["/home/ana/Documents/x"]),
            [r#""x" (Documents)"#],
            "not Home"
        );
        assert_eq!(label(&["/home/ana/notes"]), [r#""notes" (Home)"#]);
    }

    #[test]
    fn two_locations_never_share_a_label() {
        let shown = label(&[
            "/a/b/log",
            "/c/d/log",
            "/home/ana/Documents/x/y/log",
            "/home/ana/Documents/z/y/log",
        ]);
        let unique: BTreeSet<&String> = shown.iter().collect();
        assert_eq!(unique.len(), shown.len(), "{shown:?}");
        assert!(
            shown[0].contains(r#""b""#) && shown[1].contains(r#""d""#),
            "{shown:?}"
        );
    }

    #[test]
    fn a_name_that_looks_like_a_place_stays_inside_its_quotes() {
        let shown = label(&["/etc/report.pdf (Downloads)"]);
        assert_eq!(shown, [r#""report.pdf (Downloads)" (elsewhere)"#]);
        let real = label(&["/home/ana/Downloads/report.pdf"]);
        assert_ne!(shown, real);
    }

    #[test]
    fn a_non_utf8_name_does_not_collide_with_another() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let a = PathBuf::from(OsStr::from_bytes(b"/x/a\xff"));
        let b = PathBuf::from(OsStr::from_bytes(b"/x/a\xfe"));
        let host = host();
        let labels = Labels::build(&host, [a.as_path(), b.as_path()]);
        assert_ne!(labels.of(&a), labels.of(&b));
    }
}
