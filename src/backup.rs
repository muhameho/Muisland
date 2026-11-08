//! Encrypted, chunked local-data checkpoints. Recovery writes a separate folder.
use crate::profiles;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
const MAGIC: &[u8] = b"MUISLAND-LOCAL-1\n";
const CHUNK: usize = 1024 * 1024;
fn relative(p: &Path) -> bool {
    !p.as_os_str().is_empty()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}
fn ordinary(path: &Path) -> io::Result<fs::Metadata> {
    use std::os::windows::fs::MetadataExt;
    let m = fs::symlink_metadata(path)?;
    if m.file_attributes() & 0x400 != 0 {
        return Err(io::Error::other(
            "Local backup cannot follow links or junctions",
        ));
    }
    Ok(m)
}
fn write_file(source: &Path, dest: &Path) -> io::Result<u64> {
    let mut source = File::open(source)?;
    let mut dest = File::create(dest)?;
    dest.write_all(MAGIC)?;
    let mut buffer = vec![0; CHUNK];
    let mut total = 0;
    loop {
        let n = source.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let cipher = profiles::dpapi(&buffer[..n], true)?;
        dest.write_all(&(cipher.len() as u32).to_le_bytes())?;
        dest.write_all(&cipher)?;
        total += n as u64;
    }
    buffer.fill(0);
    dest.write_all(&0u32.to_le_bytes())?;
    dest.sync_all()?;
    Ok(total)
}
fn walk(source: &Path, data: &Path, root: &Path, entries: &mut Vec<Value>) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let p = entry.path();
        let meta = ordinary(&p)?;
        let rel = p.strip_prefix(root).map_err(io::Error::other)?;
        if !relative(rel) {
            return Err(io::Error::other("Invalid backup path"));
        }
        let dest = data.join(rel);
        if meta.is_dir() {
            fs::create_dir(&dest)?;
            entries.push(json!({"path":rel.to_string_lossy(),"directory":true}));
            walk(&p, data, root, entries)?;
        } else if meta.is_file() {
            let size = write_file(&p, &dest)?;
            if size != meta.len() {
                return Err(io::Error::other(
                    "Claude data changed during backup; switch cancelled",
                ));
            }
            entries.push(json!({"path":rel.to_string_lossy(),"bytes":size}));
        } else {
            return Err(io::Error::other("Unsupported local data item"));
        }
    }
    Ok(())
}
fn snapshot(source: &Path, dest: &Path) -> io::Result<()> {
    ordinary(source)?;
    fs::create_dir(dest)?;
    let data = dest.join("data");
    fs::create_dir(&data)?;
    let mut entries = Vec::new();
    walk(source, &data, source, &mut entries)?;
    let manifest =
        serde_json::to_vec(&json!({"format":1,"created":crate::accounts::now(),"entries":entries}))
            .map_err(io::Error::other)?;
    profiles::write_atomic(
        &dest.join("manifest.bin"),
        &profiles::dpapi(&manifest, true)?,
    )
}
pub fn checkpoint(source: &Path) -> io::Result<PathBuf> {
    let root = profiles::base()?.join("local-backups");
    fs::create_dir_all(&root)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stage = root.join(format!("{stamp}.partial"));
    snapshot(source, &stage)?;
    let dest = root.join(stamp.to_string());
    fs::rename(stage, &dest)?;
    Ok(dest)
}
fn restore_file(source: &Path, dest: &Path, expected: u64) -> io::Result<()> {
    ordinary(source)?;
    let mut source = File::open(source)?;
    let mut magic = vec![0; MAGIC.len()];
    source.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(io::Error::other("Unsupported backup format"));
    }
    let mut dest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    let mut total = 0u64;
    loop {
        let mut len = [0; 4];
        source.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        if len == 0 {
            break;
        }
        if len > CHUNK + 65536 {
            return Err(io::Error::other("Damaged backup chunk"));
        }
        let mut encrypted = vec![0; len];
        source.read_exact(&mut encrypted)?;
        let mut plain = profiles::dpapi(&encrypted, false)?;
        total = total
            .checked_add(plain.len() as u64)
            .ok_or_else(|| io::Error::other("Invalid backup size"))?;
        if total > expected {
            return Err(io::Error::other("Backup size mismatch"));
        }
        dest.write_all(&plain)?;
        plain.fill(0);
    }
    let mut trailing = [0];
    if total != expected || source.read(&mut trailing)? != 0 {
        return Err(io::Error::other("Incomplete or damaged backup"));
    }
    dest.sync_all()
}
fn recover(archive: &Path, dest: &Path) -> io::Result<()> {
    ordinary(archive)?;
    ordinary(&archive.join("manifest.bin"))?;
    let value: Value = serde_json::from_slice(&profiles::dpapi(
        &fs::read(archive.join("manifest.bin"))?,
        false,
    )?)
    .map_err(io::Error::other)?;
    if value["format"].as_u64() != Some(1) {
        return Err(io::Error::other("Unsupported backup version"));
    }
    let entries = value["entries"]
        .as_array()
        .ok_or_else(|| io::Error::other("Invalid backup manifest"))?;
    // Validate every path before writing anything; never overwrite Claude's live data.
    for entry in entries {
        if !relative(Path::new(
            entry["path"]
                .as_str()
                .ok_or_else(|| io::Error::other("Invalid backup path"))?,
        )) {
            return Err(io::Error::other("Unsafe backup path"));
        }
    }
    fs::create_dir(dest)?;
    let data = archive.join("data");
    ordinary(&data)?;
    for entry in entries {
        let rel = Path::new(entry["path"].as_str().unwrap());
        let target = dest.join(rel);
        let source = data.join(rel);
        // Check each archive ancestor as well as each leaf for reparse points.
        let mut current = data.clone();
        for component in rel.components() {
            current.push(component);
            ordinary(&current)?;
        }
        if entry["directory"].as_bool() == Some(true) {
            fs::create_dir(&target)?;
        } else {
            restore_file(
                &source,
                &target,
                entry["bytes"]
                    .as_u64()
                    .ok_or_else(|| io::Error::other("Invalid backup size"))?,
            )?;
        }
    }
    Ok(())
}
pub fn recover_latest() -> io::Result<PathBuf> {
    let root = profiles::base()?.join("local-backups");
    let mut archives = fs::read_dir(&root)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
                && p.join("manifest.bin").is_file()
        })
        .collect::<Vec<_>>();
    archives.sort();
    let latest = archives
        .last()
        .ok_or_else(|| io::Error::other("No completed local backup exists"))?;
    let recovered = profiles::base()?.join("recovered");
    fs::create_dir_all(&recovered)?;
    let stage = recovered.join(format!("{}.partial", crate::accounts::now()));
    recover(latest, &stage)?;
    let dest = stage.with_extension("");
    fs::rename(&stage, &dest)?;
    Ok(dest)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_backup_roundtrip_is_encrypted_and_does_not_overwrite_source() {
        let root = std::env::temp_dir().join(format!(
            "muisland-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        fs::create_dir(source.join("شغل")).unwrap();
        let payload = vec![42u8; CHUNK + 123];
        fs::write(source.join("شغل/task.bin"), &payload).unwrap();
        fs::write(source.join("empty-file"), []).unwrap();
        let archive = root.join("archive");
        snapshot(&source, &archive).unwrap();
        assert_ne!(
            fs::read(archive.join("data/شغل/task.bin")).unwrap(),
            payload
        );
        let dest = root.join("recovered");
        recover(&archive, &dest).unwrap();
        assert_eq!(fs::read(dest.join("شغل/task.bin")).unwrap(), payload);
        assert!(dest.join("empty").is_dir());
        assert_eq!(fs::read(source.join("شغل/task.bin")).unwrap(), payload);
        assert!(recover(&archive, &dest).is_err());
        let file = archive.join("data/شغل/task.bin");
        let f = fs::OpenOptions::new().write(true).open(file).unwrap();
        f.set_len(32).unwrap();
        assert!(recover(&archive, &root.join("damaged")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn recovery_rejects_escaping_paths() {
        assert!(!relative(Path::new("../Claude/config.json")));
        assert!(!relative(Path::new("C:\\Claude")));
        assert!(relative(Path::new("tasks\\one.json")));
    }
}
