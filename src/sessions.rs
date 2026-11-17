//! Shares Claude Desktop's Code-tab session list between accounts.
//!
//! Claude keeps one small file per Code session under
//! `claude-code-sessions/<account>/<org>/local_<id>.json`, so each account only
//! lists its own sessions. The files hold the title, folder and a CLI session
//! id; the conversation itself lives in `~/.claude/projects`, which every
//! account already shares. Copying the session files between account folders
//! therefore lets every account list and resume every session.
//!
//! Runs only while Claude is closed (during a switch). It never deletes a
//! session: missing files are added, older copies are replaced by the newest
//! one, and each replaced copy is kept in Muisland's folder first.
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Session files are a few dozen KB; anything far larger is not one.
const MAX_SESSION_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Default, PartialEq)]
pub struct Shared {
    pub added: usize,
    pub updated: usize,
}

fn uuid_like(name: &str) -> bool {
    name.len() == 36
        && name.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}
fn session_file(name: &str) -> bool {
    name.starts_with("local_")
        && name.ends_with(".json")
        && !name.contains(['/', '\\'])
        && name.len() < 128
}
/// Plain directories and files only: never follow links or junctions.
fn plain(path: &Path) -> Option<fs::Metadata> {
    use std::os::windows::fs::MetadataExt;
    fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_attributes() & 0x400 == 0)
}
fn subdirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(uuid_like)
                && plain(p).is_some_and(|m| m.is_dir())
        })
        .collect();
    dirs.sort();
    dirs
}
/// Every `<account>/<org>` folder Claude has created.
fn folders(root: &Path) -> Vec<PathBuf> {
    subdirs(root).iter().flat_map(|a| subdirs(a)).collect()
}
fn modified(meta: &fs::Metadata) -> SystemTime {
    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)
}
/// Copies with Windows' CopyFile (which keeps the modified time, so "newest"
/// stays stable across runs), then renames into place.
fn place(source: &Path, target: &Path) -> io::Result<()> {
    let temp = target.with_extension("json.muisland-tmp");
    fs::copy(source, &temp)?;
    fs::rename(&temp, target).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// `claude` is Claude's data folder; `keep` receives replaced copies.
pub fn share(claude: &Path, keep: &Path) -> io::Result<Shared> {
    let root = claude.join("claude-code-sessions");
    if !plain(&root).is_some_and(|m| m.is_dir()) {
        return Ok(Shared::default());
    }
    let folders = folders(&root);
    if folders.len() < 2 {
        return Ok(Shared::default());
    }
    // Newest copy of every session across all folders.
    let mut newest: HashMap<String, (PathBuf, SystemTime)> = HashMap::new();
    for folder in &folders {
        for entry in fs::read_dir(folder)?.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !session_file(&name) {
                continue;
            }
            let path = entry.path();
            let Some(meta) = plain(&path).filter(|m| m.is_file() && m.len() <= MAX_SESSION_BYTES)
            else {
                continue;
            };
            let when = modified(&meta);
            match newest.get(&name) {
                Some((_, best)) if *best >= when => {}
                _ => {
                    newest.insert(name, (path, when));
                }
            }
        }
    }
    let mut shared = Shared::default();
    let stamp = crate::accounts::now();
    for folder in &folders {
        for (name, (source, when)) in &newest {
            let target = folder.join(name);
            if &target == source {
                continue;
            }
            match plain(&target) {
                None => {
                    place(source, &target)?;
                    shared.added += 1;
                }
                Some(meta) if meta.is_file() && modified(&meta) < *when => {
                    if fs::read(&target)? == fs::read(source)? {
                        continue;
                    }
                    let rel = target.strip_prefix(&root).map_err(io::Error::other)?;
                    let kept = keep.join(stamp.to_string()).join(rel);
                    fs::create_dir_all(kept.parent().unwrap())?;
                    fs::copy(&target, &kept)?;
                    place(source, &target)?;
                    shared.updated += 1;
                }
                _ => {}
            }
        }
    }
    prune_kept(keep, 5);
    Ok(shared)
}
/// Replaced copies are a safety net; keep the last few runs.
fn prune_kept(keep: &Path, runs: usize) {
    let mut stamps: Vec<(u64, PathBuf)> = fs::read_dir(keep)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let n = e.file_name().to_str()?.parse::<u64>().ok()?;
            Some((n, e.path()))
        })
        .collect();
    stamps.sort();
    for (_, old) in &stamps[..stamps.len().saturating_sub(runs)] {
        let _ = fs::remove_dir_all(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    const A: &str = "aaaaaaaa-0000-0000-0000-000000000001";
    const B: &str = "bbbbbbbb-0000-0000-0000-000000000002";
    const ORG: &str = "cccccccc-0000-0000-0000-000000000003";
    const TEAM: &str = "dddddddd-0000-0000-0000-000000000004";

    fn set_time(path: &Path, secs: u64) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
    }

    #[test]
    fn every_account_gets_every_session_and_the_newest_copy_wins() {
        let root = std::env::temp_dir().join(format!(
            "muisland-sessions-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let claude = root.join("claude");
        let sessions = claude.join("claude-code-sessions");
        let a = sessions.join(A).join(ORG);
        let b = sessions.join(B).join(ORG);
        let b_team = sessions.join(B).join(TEAM);
        for dir in [&a, &b, &b_team] {
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join("scheduled-tasks.json"), b"[]").unwrap();
        }
        fs::write(a.join("local_one.json"), b"one").unwrap();
        fs::write(b.join("local_two.json"), b"two").unwrap();
        // Both have "shared"; B's copy is newer.
        fs::write(a.join("local_shared.json"), b"old").unwrap();
        fs::write(b.join("local_shared.json"), b"new").unwrap();
        set_time(&a.join("local_shared.json"), 1_000);
        set_time(&b.join("local_shared.json"), 2_000);
        fs::write(a.join("notes.txt"), b"not a session").unwrap();

        let keep = root.join("keep");
        let result = share(&claude, &keep).unwrap();
        for dir in [&a, &b, &b_team] {
            assert_eq!(fs::read(dir.join("local_one.json")).unwrap(), b"one");
            assert_eq!(fs::read(dir.join("local_two.json")).unwrap(), b"two");
            assert_eq!(fs::read(dir.join("local_shared.json")).unwrap(), b"new");
            assert_eq!(fs::read(dir.join("scheduled-tasks.json")).unwrap(), b"[]");
        }
        assert!(!b.join("notes.txt").exists(), "only session files are shared");
        // one → B, B-team; two → A, B-team; shared → B-team (A's copy is updated).
        assert_eq!(result.updated, 1);
        assert_eq!(result.added, 5);
        // The replaced copy was kept.
        let kept: Vec<_> = walk(&keep);
        assert!(kept.iter().any(|p| p.ends_with("local_shared.json")));
        assert_eq!(fs::read(&kept[0]).unwrap(), b"old");
        // Running again changes nothing.
        assert_eq!(share(&claude, &keep).unwrap(), Shared::default());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nothing_happens_without_two_folders() {
        let root = std::env::temp_dir().join(format!(
            "muisland-sessions-one-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let only = root.join("claude/claude-code-sessions").join(A).join(ORG);
        fs::create_dir_all(&only).unwrap();
        fs::write(only.join("local_x.json"), b"x").unwrap();
        assert_eq!(share(&root.join("claude"), &root.join("keep")).unwrap(), Shared::default());
        assert_eq!(share(&root.join("missing"), &root.join("keep")).unwrap(), Shared::default());
        fs::remove_dir_all(root).unwrap();
    }

    /// Dry run on a copy: `MUISLAND_SESSIONS_COPY=<folder containing claude-code-sessions>`.
    #[test]
    #[ignore]
    fn share_on_a_copy() {
        let Some(dir) = std::env::var_os("MUISLAND_SESSIONS_COPY") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let result = share(&dir, &dir.join("kept")).unwrap();
        eprintln!("added {} · updated {}", result.added, result.updated);
        for folder in folders(&dir.join("claude-code-sessions")) {
            let n = fs::read_dir(&folder)
                .unwrap()
                .filter(|e| session_file(&e.as_ref().unwrap().file_name().to_string_lossy()))
                .count();
            eprintln!("{} → {n} sessions", folder.strip_prefix(&dir).unwrap().display());
        }
        eprintln!("second run: {:?}", share(&dir, &dir.join("kept")).unwrap());
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
