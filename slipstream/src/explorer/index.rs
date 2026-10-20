//! The home folder's files and folders by name, for the explorer's search. Walking the folders
//! can take as long as a slow disk or a hung mount, so it is done on a thread of its own, and a
//! search reads whatever list that thread last finished.

use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
};

/// The deepest a file or folder is looked for, in folders below the home folder:
/// `~/Documents/Taxes/2026/May/receipt.pdf` is four down.
pub(super) const DEEPEST: u8 = 4;
/// The most files and folders kept, nearest the home folder first.
pub(super) const KEPT: usize = 20_000;
/// The most folders opened in one walk, so a home of many small folders can't keep it going.
const FOLDERS: usize = 5_000;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Entry {
    pub(super) path: PathBuf,
    /// Its name in lower case, which is what a query is matched against.
    name: Box<str>,
    /// Folders between it and the home folder.
    depth: u8,
    folder: bool,
}

/// A file or folder the explorer shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Found {
    pub(super) path: PathBuf,
    pub(super) folder: bool,
    /// How well its name matches the query, as `name_score` has it.
    pub(super) score: u8,
}

/// Every file and folder under `home` worth finding, breadth first so the caps keep the nearest.
/// Hidden folders and files, symbolic links, `node_modules` and cache folders (tagged with
/// `CACHEDIR.TAG`, as build tools tag their output) are left out.
pub(super) fn walk(home: &Path) -> Vec<Entry> {
    let mut found = Vec::new();
    let mut folders = VecDeque::from([(home.to_path_buf(), 0u8)]);
    let mut opened = 0;
    while let Some((dir, depth)) = folders.pop_front() {
        if opened == FOLDERS {
            break;
        }
        opened += 1;
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<fs::DirEntry> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let folder = kind.is_dir();
            if folder {
                if name == "node_modules" || path.join("CACHEDIR.TAG").exists() {
                    continue;
                }
                if depth < DEEPEST {
                    folders.push_back((path.clone(), depth + 1));
                }
            } else if !kind.is_file() {
                continue;
            }
            found.push(Entry {
                path,
                name: name.to_lowercase().into(),
                depth,
                folder,
            });
            if found.len() == KEPT {
                return found;
            }
        }
    }
    found
}

/// How well a lower-case name matches a lower-case query: 0 when it starts with it, 1 when a
/// word in it does, 3 when it's anywhere in it. The numbers sit on the apps' scale, where 2 is a
/// keyword.
pub(super) fn name_score(name: &str, query: &str) -> Option<u8> {
    if name.starts_with(query) {
        Some(0)
    } else if name
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        Some(1)
    } else if name.contains(query) {
        Some(3)
    } else {
        None
    }
}

/// The files and folders whose names match `query` (lower case), at most `most` of them, best
/// first: the better match, then a recent file ahead of the rest, then the one nearer the home
/// folder, then the shorter name.
pub(super) fn search(recent: &[PathBuf], index: &[Entry], query: &str, most: usize) -> Vec<Found> {
    let lower = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let mut hits: Vec<((u8, u8, u8, usize), Found)> = Vec::new();
    for path in recent {
        let name = lower(path);
        if let Some(score) = name_score(&name, query) {
            let found = Found {
                path: path.clone(),
                folder: false,
                score,
            };
            hits.push(((score, 0, 0, name.len()), found));
        }
    }
    for entry in index {
        if let Some(score) = name_score(&entry.name, query)
            && !recent.contains(&entry.path)
        {
            let found = Found {
                path: entry.path.clone(),
                folder: entry.folder,
                score,
            };
            hits.push(((score, 1, entry.depth, entry.name.len()), found));
        }
    }
    hits.sort_by_key(|(key, _)| *key);
    hits.into_iter()
        .take(most)
        .map(|(_, found)| found)
        .collect()
}

/// A made-up file or folder, as a walk would have found it.
#[cfg(test)]
pub(super) fn entry(path: &str, depth: u8, folder: bool) -> Entry {
    let path = PathBuf::from(path);
    let name = path.file_name().unwrap().to_string_lossy().to_lowercase();
    Entry {
        path,
        name: name.into(),
        depth,
        folder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_skips_hidden_cache_and_deep_folders() {
        let home = crate::files::test_scratch("explorer-index");
        let deep = home.join("a/b/c/d/e");
        fs::create_dir_all(&deep).unwrap();
        for file in [
            "notes.txt",
            "a/b/c/d/four-down.txt",
            "a/b/c/d/e/five-down.txt",
            ".hidden/secret.txt",
            ".profile",
            "Projects/tool/target/CACHEDIR.TAG",
            "Projects/tool/target/built.o",
            "Projects/site/node_modules/left-pad.js",
            "Projects/tool/main.rs",
        ] {
            let path = home.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        std::os::unix::fs::symlink(&home, home.join("loop")).unwrap();
        let names: Vec<String> = walk(&home)
            .iter()
            .map(|entry| {
                entry
                    .path
                    .strip_prefix(&home)
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        for wanted in [
            "notes.txt",
            "a/b/c/d/four-down.txt",
            "Projects/tool/main.rs",
        ] {
            assert!(names.iter().any(|name| name == wanted), "{wanted} is found");
        }
        assert!(names.iter().any(|name| name == "Projects"), "folders too");
        for unwanted in [
            "five-down",
            "secret",
            "profile",
            "built.o",
            "left-pad",
            "loop",
        ] {
            assert!(
                !names.iter().any(|name| name.contains(unwanted)),
                "{unwanted} is left out: {names:?}"
            );
        }
        // Nearest first.
        let depth = |name: &str| name.matches('/').count();
        assert!(
            names
                .windows(2)
                .all(|pair| depth(&pair[0]) <= depth(&pair[1]))
        );
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_search_ranks_the_name_then_recent_use_then_nearness() {
        let index = [
            entry("/home/sam/old/a/my budget notes.txt", 2, false),
            entry("/home/sam/Budget", 0, true),
            entry("/home/sam/old/budget.ods", 1, false),
            entry("/home/sam/Documents/budget.ods", 1, false),
            entry("/home/sam/rebudgeted.txt", 0, false),
            entry("/home/sam/holiday.jpg", 0, false),
        ];
        let recent = [PathBuf::from("/home/sam/Documents/budget.ods")];
        let found = search(&recent, &index, "budget", 10);
        let paths: Vec<&str> = found.iter().map(|f| f.path.to_str().unwrap()).collect();
        assert_eq!(
            paths,
            [
                "/home/sam/Documents/budget.ods",
                "/home/sam/Budget",
                "/home/sam/old/budget.ods",
                "/home/sam/old/a/my budget notes.txt",
                "/home/sam/rebudgeted.txt",
            ]
        );
        assert!(found[1].folder);
        assert_eq!(
            found.iter().map(|f| f.score).collect::<Vec<_>>(),
            [0, 0, 0, 1, 3]
        );
        assert_eq!(search(&recent, &index, "budget", 2).len(), 2, "capped");
        assert!(search(&recent, &index, "zebra", 10).is_empty());
    }
}
