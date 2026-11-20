//! "Safe to clean" suggestions. Only reports; never deletes anything.

use crate::tree::{Dir, ItemKind};
use serde::Serialize;
use std::{collections::HashMap, path::Path};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Regenerated automatically by the owning tool or app.
    Safe,
    /// Worth a look; may contain something the user wants to keep.
    Review,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// Stable id; the UI maps it to a translated title and explanation.
    pub rule: &'static str,
    pub level: Level,
    pub path: String,
    pub name: String,
    pub kind: ItemKind,
    pub size: u64,
}

enum Match {
    /// Exactly this directory (relative to home).
    Exact(&'static str),
    /// Each entry of this directory is a separate suggestion.
    ChildrenOf(&'static str),
    /// Any directory with this name.
    DirName(&'static str),
    /// Big files with one of these extensions.
    FileExt(&'static [&'static str]),
}

struct Rule {
    id: &'static str,
    level: Level,
    matcher: Match,
}

const RULES: &[Rule] = &[
    Rule {
        id: "appCache",
        level: Level::Safe,
        matcher: Match::ChildrenOf("Library/Caches"),
    },
    Rule {
        id: "logs",
        level: Level::Safe,
        matcher: Match::Exact("Library/Logs"),
    },
    Rule {
        id: "xcodeDerivedData",
        level: Level::Safe,
        matcher: Match::Exact("Library/Developer/Xcode/DerivedData"),
    },
    Rule {
        id: "xcodeDeviceSupport",
        level: Level::Safe,
        matcher: Match::Exact("Library/Developer/Xcode/iOS DeviceSupport"),
    },
    Rule {
        id: "simulatorCache",
        level: Level::Safe,
        matcher: Match::Exact("Library/Developer/CoreSimulator/Caches"),
    },
    Rule {
        id: "xcodeArchives",
        level: Level::Review,
        matcher: Match::Exact("Library/Developer/Xcode/Archives"),
    },
    Rule {
        id: "simulatorDevices",
        level: Level::Review,
        matcher: Match::Exact("Library/Developer/CoreSimulator/Devices"),
    },
    Rule {
        id: "androidEmulators",
        level: Level::Review,
        matcher: Match::Exact(".android/avd"),
    },
    Rule {
        id: "androidSystemImages",
        level: Level::Review,
        matcher: Match::Exact("Library/Android/sdk/system-images"),
    },
    Rule {
        id: "packageCache",
        level: Level::Safe,
        matcher: Match::Exact(".gradle/caches"),
    },
    Rule {
        id: "packageCache",
        level: Level::Safe,
        matcher: Match::Exact(".npm/_cacache"),
    },
    Rule {
        id: "packageCache",
        level: Level::Safe,
        matcher: Match::Exact(".cargo/registry"),
    },
    Rule {
        id: "packageCache",
        level: Level::Safe,
        matcher: Match::Exact("go/pkg/mod"),
    },
    Rule {
        id: "toolCache",
        level: Level::Review,
        matcher: Match::Exact(".cache"),
    },
    Rule {
        id: "spotifyCache",
        level: Level::Safe,
        matcher: Match::Exact("Library/Application Support/Spotify/PersistentCache"),
    },
    Rule {
        id: "deviceBackups",
        level: Level::Review,
        matcher: Match::Exact("Library/Application Support/MobileSync/Backup"),
    },
    Rule {
        id: "docker",
        level: Level::Review,
        matcher: Match::Exact("Library/Containers/com.docker.docker"),
    },
    Rule {
        id: "trash",
        level: Level::Review,
        matcher: Match::Exact(".Trash"),
    },
    Rule {
        id: "downloads",
        level: Level::Review,
        matcher: Match::Exact("Downloads"),
    },
    Rule {
        id: "nodeModules",
        level: Level::Safe,
        matcher: Match::DirName("node_modules"),
    },
    Rule {
        id: "installer",
        level: Level::Review,
        matcher: Match::FileExt(&["dmg", "pkg", "iso", "xip", "ipa"]),
    },
];

/// Suggestions at or above this size are reported.
pub const MIN_SUGGESTION_SIZE: u64 = 50 * 1024 * 1024;

pub fn compute(root: &Dir, root_path: &Path, home: &Path) -> Vec<Suggestion> {
    let mut exact: HashMap<std::path::PathBuf, &Rule> = HashMap::new();
    let mut children_of: HashMap<std::path::PathBuf, &Rule> = HashMap::new();
    let mut names: HashMap<&str, &Rule> = HashMap::new();
    let mut exts: HashMap<&str, &Rule> = HashMap::new();
    for rule in RULES {
        match rule.matcher {
            Match::Exact(rel) => {
                exact.insert(home.join(rel), rule);
            }
            Match::ChildrenOf(rel) => {
                children_of.insert(home.join(rel), rule);
            }
            Match::DirName(name) => {
                names.insert(name, rule);
            }
            Match::FileExt(list) => list.iter().for_each(|e| {
                exts.insert(e, rule);
            }),
        }
    }

    let mut out = Vec::new();
    let mut push = |rule: &Rule, path: &Path, name: &str, kind: ItemKind, size: u64| {
        if size >= MIN_SUGGESTION_SIZE {
            out.push(Suggestion {
                rule: rule.id,
                level: rule.level,
                path: path.to_string_lossy().into_owned(),
                name: name.to_owned(),
                kind,
                size,
            });
        }
    };

    #[allow(clippy::too_many_arguments)]
    fn walk(
        dir: &Dir,
        path: &Path,
        is_root: bool,
        exact: &HashMap<std::path::PathBuf, &Rule>,
        children_of: &HashMap<std::path::PathBuf, &Rule>,
        names: &HashMap<&str, &Rule>,
        exts: &HashMap<&str, &Rule>,
        push: &mut impl FnMut(&Rule, &Path, &str, ItemKind, u64),
    ) {
        if dir.size < MIN_SUGGESTION_SIZE {
            return;
        }
        let by_name = (!is_root).then(|| names.get(&*dir.name)).flatten();
        if let Some(rule) = exact.get(path).or(by_name) {
            push(rule, path, &dir.name, ItemKind::Dir, dir.size);
            return;
        }
        if let Some(rule) = children_of.get(path) {
            for child in &dir.children {
                push(
                    rule,
                    &path.join(&*child.name),
                    &child.name,
                    ItemKind::Dir,
                    child.size,
                );
            }
            for file in &dir.files {
                push(
                    rule,
                    &path.join(&*file.name),
                    &file.name,
                    ItemKind::File,
                    file.size,
                );
            }
            return;
        }
        for file in &dir.files {
            let ext = Path::new(&*file.name)
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase());
            if let Some(rule) = ext.as_deref().and_then(|e| exts.get(e)) {
                push(
                    rule,
                    &path.join(&*file.name),
                    &file.name,
                    ItemKind::File,
                    file.size,
                );
            }
        }
        for child in &dir.children {
            walk(
                child,
                &path.join(&*child.name),
                false,
                exact,
                children_of,
                names,
                exts,
                push,
            );
        }
    }

    walk(
        root,
        root_path,
        true,
        &exact,
        &children_of,
        &names,
        &exts,
        &mut push,
    );
    out.sort_by(|a, b| a.level.cmp(&b.level).then(b.size.cmp(&a.size)));
    out
}
