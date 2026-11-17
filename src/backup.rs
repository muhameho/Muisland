//! Encrypted, chunked local-data checkpoints. Recovery writes a separate folder.
//!
//! Format 2 packs every file into one stream (`data.bin`) sealed with
//! AES-256-GCM in 4 MB chunks. The random per-backup key is stored protected
//! by DPAPI, so only this Windows user can open it, as before. Format 1 called
//! DPAPI on every 1 MB of every file (about 11 MB/s) and wrote one archive file
//! per source file; it still recovers.
use crate::profiles;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
const MAGIC: &[u8] = b"MUISLAND-LOCAL-1\n";
const PACK_MAGIC: &[u8] = b"MUISLAND-PACK-2\n";
const CHUNK: usize = 1024 * 1024;
const PACK_CHUNK: usize = 4 * 1024 * 1024;
/// Files at least this large (the Cowork VM data disk) get their own sealed
/// blob, which the next backup hard-links instead of re-encrypting when the
/// file has not changed.
const BLOB_MIN: u64 = 16 * 1024 * 1024;
/// Completed checkpoints kept; older ones are deleted after each new backup.
pub const KEEP: usize = 3;
/// Folders Claude downloads or rebuilds by itself. Copying them made every
/// backup gigabytes (VM images, bundled CLIs) and tens of thousands of cache
/// files, while restoring them brings nothing back.
pub const SKIPPED: &[&str] = &[
    "claude-code",
    "claude-code-vm",
    "Cache",
    "Code Cache",
    "GPUCache",
    "GrShaderCache",
    "GraphiteDawnCache",
    "ShaderCache",
    "DawnWebGPUCache",
    "DawnGraphiteCache",
    "GPUPersistentCache",
    "Crashpad",
    "sentry",
    "logs",
    "Service Worker/CacheStorage",
    "Service Worker/ScriptCache",
];
/// The Cowork VM folder holds the downloadable base image (kernel, initrd,
/// rootfs, several GB) next to `sessiondata.vhdx`, the VM's own data disk,
/// which changes as Cowork works. Only the data disk is backed up.
const VM_FOLDER: &str = "vm_bundles";
const VM_KEPT: &[&str] = &["sessiondata.vhdx"];
fn skipped(rel: &Path, is_dir: bool) -> bool {
    let rel = rel.to_string_lossy().replace('\\', "/");
    let under = |folder: &str| {
        rel.eq_ignore_ascii_case(folder)
            || (rel.len() > folder.len()
                && rel.as_bytes()[folder.len()] == b'/'
                && rel[..folder.len()].eq_ignore_ascii_case(folder))
    };
    if under(VM_FOLDER) {
        let name = rel.rsplit('/').next().unwrap_or("");
        return !is_dir && !VM_KEPT.iter().any(|k| name.eq_ignore_ascii_case(k));
    }
    SKIPPED.iter().any(|skip| {
        rel.eq_ignore_ascii_case(skip)
            || (rel.len() > skip.len()
                && rel.as_bytes()[skip.len()] == b'/'
                && rel[..skip.len()].eq_ignore_ascii_case(skip))
    })
}
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

/// AES-256-GCM through Windows CNG. Chunk `i` uses nonce `0u32 || i as u64`;
/// keys are random per backup, so nonces never repeat under one key.
struct Gcm {
    alg: *mut core::ffi::c_void,
    key: *mut core::ffi::c_void,
}
const TAG: usize = 16;
fn cng(status: i32, what: &str) -> io::Result<()> {
    if status < 0 {
        Err(io::Error::other(format!("{what} failed ({status:#x})")))
    } else {
        Ok(())
    }
}
impl Gcm {
    fn new(secret: &[u8; 32]) -> io::Result<Self> {
        use windows_sys::Win32::Security::Cryptography::*;
        let mut gcm = Gcm {
            alg: std::ptr::null_mut(),
            key: std::ptr::null_mut(),
        };
        unsafe {
            cng(
                BCryptOpenAlgorithmProvider(&mut gcm.alg, BCRYPT_AES_ALGORITHM, std::ptr::null(), 0),
                "Opening AES",
            )?;
            let mode: Vec<u16> = "ChainingModeGCM\0".encode_utf16().collect();
            cng(
                BCryptSetProperty(
                    gcm.alg,
                    BCRYPT_CHAINING_MODE,
                    mode.as_ptr().cast(),
                    (mode.len() * 2) as u32,
                    0,
                ),
                "Selecting GCM",
            )?;
            cng(
                BCryptGenerateSymmetricKey(
                    gcm.alg,
                    &mut gcm.key,
                    std::ptr::null_mut(),
                    0,
                    secret.as_ptr(),
                    secret.len() as u32,
                    0,
                ),
                "Creating the backup key",
            )?;
        }
        Ok(gcm)
    }
    fn nonce(index: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&index.to_le_bytes());
        nonce
    }
    fn run(&self, index: u64, input: &[u8], tag: &mut [u8; TAG], seal: bool) -> io::Result<Vec<u8>> {
        use windows_sys::Win32::Security::Cryptography::*;
        let mut nonce = Self::nonce(index);
        let mut info: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32;
        info.dwInfoVersion = 1;
        info.pbNonce = nonce.as_mut_ptr();
        info.cbNonce = nonce.len() as u32;
        info.pbTag = tag.as_mut_ptr();
        info.cbTag = TAG as u32;
        let mut out = vec![0u8; input.len()];
        let mut written = 0u32;
        let status = unsafe {
            if seal {
                BCryptEncrypt(
                    self.key,
                    input.as_ptr(),
                    input.len() as u32,
                    (&info as *const BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO).cast(),
                    std::ptr::null_mut(),
                    0,
                    out.as_mut_ptr(),
                    out.len() as u32,
                    &mut written,
                    0,
                )
            } else {
                BCryptDecrypt(
                    self.key,
                    input.as_ptr(),
                    input.len() as u32,
                    (&info as *const BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO).cast(),
                    std::ptr::null_mut(),
                    0,
                    out.as_mut_ptr(),
                    out.len() as u32,
                    &mut written,
                    0,
                )
            }
        };
        if status < 0 {
            out.fill(0);
            return Err(io::Error::other(if seal {
                "Encrypting the backup failed"
            } else {
                "Backup is damaged or was modified"
            }));
        }
        out.truncate(written as usize);
        Ok(out)
    }
    /// Returns ciphertext followed by the 16-byte tag.
    fn seal(&self, index: u64, plain: &[u8]) -> io::Result<Vec<u8>> {
        let mut tag = [0u8; TAG];
        let mut out = self.run(index, plain, &mut tag, true)?;
        out.extend_from_slice(&tag);
        Ok(out)
    }
    fn open(&self, index: u64, sealed: &[u8]) -> io::Result<Vec<u8>> {
        if sealed.len() < TAG {
            return Err(io::Error::other("Damaged backup chunk"));
        }
        let (cipher, tag) = sealed.split_at(sealed.len() - TAG);
        let mut tag: [u8; TAG] = tag.try_into().unwrap();
        self.run(index, cipher, &mut tag, false)
    }
}
impl Drop for Gcm {
    fn drop(&mut self) {
        use windows_sys::Win32::Security::Cryptography::*;
        unsafe {
            if !self.key.is_null() {
                BCryptDestroyKey(self.key);
            }
            if !self.alg.is_null() {
                BCryptCloseAlgorithmProvider(self.alg, 0);
            }
        }
    }
}
fn random_key() -> io::Result<[u8; 32]> {
    use windows_sys::Win32::Security::Cryptography::*;
    let mut key = [0u8; 32];
    cng(
        unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                key.as_mut_ptr(),
                key.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        },
        "Generating the backup key",
    )?;
    Ok(key)
}
/// `magic, [u32 len][DPAPI(key)], ([u32 len][AES-GCM chunk])*, 0u32`.
struct PackWriter {
    file: io::BufWriter<File>,
    buffer: Vec<u8>,
    gcm: Gcm,
    index: u64,
}
impl PackWriter {
    fn create(path: &Path) -> io::Result<Self> {
        let mut file = io::BufWriter::new(File::create(path)?);
        file.write_all(PACK_MAGIC)?;
        let mut key = random_key()?;
        let gcm = Gcm::new(&key);
        let protected = profiles::dpapi(&key, true);
        key.fill(0);
        let (gcm, protected) = (gcm?, protected?);
        file.write_all(&(protected.len() as u32).to_le_bytes())?;
        file.write_all(&protected)?;
        Ok(Self {
            file,
            buffer: Vec::with_capacity(PACK_CHUNK),
            gcm,
            index: 0,
        })
    }
    fn flush_chunk(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let cipher = self.gcm.seal(self.index, &self.buffer)?;
        self.index += 1;
        self.buffer.fill(0);
        self.buffer.clear();
        self.file.write_all(&(cipher.len() as u32).to_le_bytes())?;
        self.file.write_all(&cipher)
    }
    fn write(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let take = (PACK_CHUNK - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == PACK_CHUNK {
                self.flush_chunk()?;
            }
        }
        Ok(())
    }
    fn finish(mut self) -> io::Result<()> {
        self.flush_chunk()?;
        self.file.write_all(&0u32.to_le_bytes())?;
        let file = self.file.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()
    }
}
/// Decrypts a packed stream back into consecutive files.
struct PackReader {
    file: io::BufReader<File>,
    plain: Vec<u8>,
    pos: usize,
    done: bool,
    gcm: Gcm,
    index: u64,
}
impl PackReader {
    fn open(path: &Path) -> io::Result<Self> {
        ordinary(path)?;
        let mut file = io::BufReader::new(File::open(path)?);
        let mut magic = vec![0; PACK_MAGIC.len()];
        file.read_exact(&mut magic)?;
        if magic != PACK_MAGIC {
            return Err(io::Error::other("Unsupported backup format"));
        }
        let mut len = [0; 4];
        file.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        if len > 4096 {
            return Err(io::Error::other("Damaged backup key"));
        }
        let mut protected = vec![0; len];
        file.read_exact(&mut protected)?;
        let mut key = profiles::dpapi(&protected, false)?;
        let secret: Result<[u8; 32], _> = key.as_slice().try_into();
        key.fill(0);
        let mut secret = secret.map_err(|_| io::Error::other("Damaged backup key"))?;
        let gcm = Gcm::new(&secret);
        secret.fill(0);
        Ok(Self {
            file,
            plain: Vec::new(),
            pos: 0,
            done: false,
            gcm: gcm?,
            index: 0,
        })
    }
    fn refill(&mut self) -> io::Result<bool> {
        if self.done {
            return Ok(false);
        }
        let mut len = [0; 4];
        self.file.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        if len == 0 {
            self.done = true;
            return Ok(false);
        }
        if len > PACK_CHUNK + TAG {
            return Err(io::Error::other("Damaged backup chunk"));
        }
        let mut encrypted = vec![0; len];
        self.file.read_exact(&mut encrypted)?;
        self.plain.fill(0);
        self.plain = self.gcm.open(self.index, &encrypted)?;
        self.index += 1;
        self.pos = 0;
        Ok(true)
    }
    /// Copies exactly `n` bytes of the stream into `dest`.
    fn copy_to(&mut self, dest: &mut impl Write, mut n: u64) -> io::Result<()> {
        while n > 0 {
            if self.pos == self.plain.len() && !self.refill()? {
                return Err(io::Error::other("Incomplete or damaged backup"));
            }
            let take = ((self.plain.len() - self.pos) as u64).min(n) as usize;
            dest.write_all(&self.plain[self.pos..self.pos + take])?;
            self.pos += take;
            n -= take as u64;
        }
        Ok(())
    }
    /// The stream must end exactly where the manifest says.
    fn finish(mut self) -> io::Result<()> {
        let rest = self.plain.len() - self.pos;
        self.plain.fill(0);
        if rest != 0 || self.refill()? {
            return Err(io::Error::other("Incomplete or damaged backup"));
        }
        let mut trailing = [0];
        if self.file.read(&mut trailing)? != 0 {
            return Err(io::Error::other("Incomplete or damaged backup"));
        }
        Ok(())
    }
}
/// Large files of the previous backup: path → (bytes, modified, blob path).
type Reuse = std::collections::HashMap<String, (u64, u64, PathBuf)>;
fn modified(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
fn blob_name(name: &str) -> bool {
    name.strip_prefix("blob-")
        .and_then(|n| n.strip_suffix(".bin"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
struct Walk<'a> {
    pack: &'a mut PackWriter,
    dest: &'a Path,
    reuse: &'a Reuse,
    blobs: usize,
}
fn copy_into(file: &mut File, buffer: &mut [u8], out: &mut PackWriter) -> io::Result<u64> {
    let mut size = 0u64;
    loop {
        let n = file.read(buffer)?;
        if n == 0 {
            return Ok(size);
        }
        out.write(&buffer[..n])?;
        size += n as u64;
    }
}
fn walk(source: &Path, w: &mut Walk, root: &Path, entries: &mut Vec<Value>) -> io::Result<()> {
    let mut buffer = vec![0; 256 * 1024];
    let mut stack = vec![source.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut children = fs::read_dir(&dir)?
            .map(|e| e.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        children.sort();
        // Directories are recorded before their contents; visit them after files.
        let mut subdirs = Vec::new();
        for p in children {
            let meta = ordinary(&p)?;
            let rel = p.strip_prefix(root).map_err(io::Error::other)?;
            if !relative(rel) {
                return Err(io::Error::other("Invalid backup path"));
            }
            if skipped(rel, meta.is_dir()) {
                continue;
            }
            if meta.is_dir() {
                entries.push(json!({"path":rel.to_string_lossy(),"directory":true}));
                subdirs.push(p);
            } else if meta.is_file() {
                let key = rel.to_string_lossy().into_owned();
                let changed = || {
                    io::Error::other("Claude data changed during backup; switch cancelled")
                };
                if meta.len() >= BLOB_MIN {
                    let stamp = modified(&meta);
                    let name = format!("blob-{}.bin", w.blobs);
                    w.blobs += 1;
                    let target = w.dest.join(&name);
                    let reused = match w.reuse.get(&key) {
                        Some((bytes, when, old)) if *bytes == meta.len() && *when == stamp => {
                            fs::hard_link(old, &target).or_else(|_| fs::copy(old, &target).map(|_| ())).is_ok()
                        }
                        _ => false,
                    };
                    if !reused {
                        let mut blob = PackWriter::create(&target)?;
                        let size = copy_into(&mut File::open(&p)?, &mut buffer, &mut blob)?;
                        blob.finish()?;
                        if size != meta.len() {
                            return Err(changed());
                        }
                    }
                    entries.push(json!({"path":key,"bytes":meta.len(),"modified":stamp,"blob":name}));
                } else {
                    let size = copy_into(&mut File::open(&p)?, &mut buffer, w.pack)?;
                    if size != meta.len() {
                        return Err(changed());
                    }
                    entries.push(json!({"path":key,"bytes":size}));
                }
            } else {
                return Err(io::Error::other("Unsupported local data item"));
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
    buffer.fill(0);
    Ok(())
}
/// Blobs of the newest completed backup, so unchanged large files are linked.
fn reusable(previous: Option<&Path>) -> Reuse {
    let mut reuse = Reuse::new();
    let Some(previous) = previous else {
        return reuse;
    };
    let Some(value) = fs::read(previous.join("manifest.bin"))
        .ok()
        .and_then(|b| profiles::dpapi(&b, false).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return reuse;
    };
    for e in value["entries"].as_array().into_iter().flatten() {
        if let (Some(path), Some(bytes), Some(when), Some(blob)) = (
            e["path"].as_str(),
            e["bytes"].as_u64(),
            e["modified"].as_u64(),
            e["blob"].as_str().filter(|b| blob_name(b)),
        ) {
            reuse.insert(path.to_string(), (bytes, when, previous.join(blob)));
        }
    }
    reuse
}
#[cfg(test)]
fn snapshot(source: &Path, dest: &Path) -> io::Result<()> {
    snapshot_from(source, dest, None)
}
fn snapshot_from(source: &Path, dest: &Path, previous: Option<&Path>) -> io::Result<()> {
    ordinary(source)?;
    fs::create_dir(dest)?;
    let mut pack = PackWriter::create(&dest.join("data.bin"))?;
    let reuse = reusable(previous);
    let mut entries = Vec::new();
    walk(
        source,
        &mut Walk {
            pack: &mut pack,
            dest,
            reuse: &reuse,
            blobs: 0,
        },
        source,
        &mut entries,
    )?;
    pack.finish()?;
    let manifest =
        serde_json::to_vec(&json!({"format":2,"created":crate::accounts::now(),"entries":entries}))
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
    // An earlier backup that never finished must not keep its space.
    remove_partials(&root);
    let stage = root.join(format!("{stamp}.partial"));
    let previous = completed(&root).pop();
    if let Err(e) = snapshot_from(source, &stage, previous.as_deref()) {
        let _ = fs::remove_dir_all(&stage);
        return Err(e);
    }
    let dest = root.join(stamp.to_string());
    fs::rename(stage, &dest)?;
    prune(&root, KEEP);
    Ok(dest)
}
fn completed(root: &Path) -> Vec<PathBuf> {
    let mut archives = fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                        && p.join("manifest.bin").is_file()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // Names are nanosecond stamps; compare numerically, not as text.
    archives.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse::<u128>().ok())
            .unwrap_or(0)
    });
    archives
}
/// Deletes all but the newest `keep` checkpoints, and any interrupted ones.
fn prune(root: &Path, keep: usize) {
    let archives = completed(root);
    for old in &archives[..archives.len().saturating_sub(keep)] {
        let _ = fs::remove_dir_all(old);
    }
    remove_partials(root);
}
fn remove_partials(root: &Path) {
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "partial") {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
}
/// Number of kept checkpoints, their total size on disk, and when the newest was taken.
pub struct Summary {
    pub count: usize,
    pub bytes: u64,
    pub latest: Option<u64>,
}
/// Disk use of the kept backups; a blob hard-linked into several backups counts once.
fn kept_size(archives: &[PathBuf]) -> u64 {
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    for archive in archives {
        for entry in fs::read_dir(archive).into_iter().flatten().filter_map(Result::ok) {
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if blob_name(&name) && !seen.insert((meta.len(), modified(&meta))) {
                continue;
            }
            total += if meta.is_dir() { size_of(&entry.path()) } else { meta.len() };
        }
    }
    total
}
fn size_of(path: &Path) -> u64 {
    fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => size_of(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}
pub fn summary() -> Summary {
    let Ok(root) = profiles::base().map(|b| b.join("local-backups")) else {
        return Summary { count: 0, bytes: 0, latest: None };
    };
    let archives = completed(&root);
    Summary {
        count: archives.len(),
        bytes: kept_size(&archives),
        latest: archives
            .last()
            .and_then(|a| a.file_name()?.to_str()?.parse::<u128>().ok())
            .map(|ns| (ns / 1_000_000_000) as u64),
    }
}
pub fn folder() -> io::Result<PathBuf> {
    let root = profiles::base()?.join("local-backups");
    fs::create_dir_all(&root)?;
    Ok(root)
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
    let format = value["format"].as_u64();
    if !matches!(format, Some(1) | Some(2)) {
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
    if format == Some(2) {
        let mut pack = PackReader::open(&archive.join("data.bin"))?;
        for entry in entries {
            let target = dest.join(Path::new(entry["path"].as_str().unwrap()));
            if entry["directory"].as_bool() == Some(true) {
                fs::create_dir(&target)?;
            } else {
                let bytes = entry["bytes"]
                    .as_u64()
                    .ok_or_else(|| io::Error::other("Invalid backup size"))?;
                let mut out = io::BufWriter::new(
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&target)?,
                );
                match entry["blob"].as_str() {
                    Some(blob) => {
                        if !blob_name(blob) {
                            return Err(io::Error::other("Unsafe backup path"));
                        }
                        let mut reader = PackReader::open(&archive.join(blob))?;
                        reader.copy_to(&mut out, bytes)?;
                        reader.finish()?;
                    }
                    None => pack.copy_to(&mut out, bytes)?,
                }
                out.flush()?;
            }
        }
        return pack.finish();
    }
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
    let archives = completed(&root);
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
        let packed = fs::read(archive.join("data.bin")).unwrap();
        assert!(!packed.windows(64).any(|w| w == &payload[..64]), "data is encrypted");
        let dest = root.join("recovered");
        recover(&archive, &dest).unwrap();
        assert_eq!(fs::read(dest.join("شغل/task.bin")).unwrap(), payload);
        assert!(dest.join("empty").is_dir());
        assert_eq!(fs::read(source.join("شغل/task.bin")).unwrap(), payload);
        assert!(recover(&archive, &dest).is_err());
        let file = archive.join("data.bin");
        let mut bytes = fs::read(&file).unwrap();
        let last = bytes.len() - 10;
        bytes[last] ^= 1;
        fs::write(&file, &bytes).unwrap();
        assert!(recover(&archive, &root.join("tampered")).is_err());
        let f = fs::OpenOptions::new().write(true).open(file).unwrap();
        f.set_len(400).unwrap();
        assert!(recover(&archive, &root.join("damaged")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn caches_and_downloads_are_skipped_and_old_checkpoints_pruned() {
        let root = std::env::temp_dir().join(format!(
            "muisland-skip-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let source = root.join("source");
        for dir in ["vm_bundles/claudevm.bundle", "Code Cache/js", "Service Worker/CacheStorage", "Service Worker/Database", "local-agent-mode-sessions"] {
            fs::create_dir_all(source.join(dir)).unwrap();
        }
        fs::write(source.join("vm_bundles/claudevm.bundle/rootfs.vhdx"), [1; 64]).unwrap();
        fs::write(source.join("vm_bundles/claudevm.bundle/sessiondata.vhdx"), [3; 64]).unwrap();
        fs::write(source.join("Code Cache/js/a"), [1; 64]).unwrap();
        fs::write(source.join("Service Worker/CacheStorage/x"), [1; 64]).unwrap();
        fs::write(source.join("Service Worker/Database/keep"), [2; 8]).unwrap();
        fs::write(source.join("local-agent-mode-sessions/s.json"), b"{}").unwrap();
        fs::write(source.join("config.json"), b"{}").unwrap();
        let archive = root.join("archive");
        snapshot(&source, &archive).unwrap();
        let out = root.join("out");
        recover(&archive, &out).unwrap();
        assert!(out.join("config.json").is_file());
        assert!(out.join("local-agent-mode-sessions/s.json").is_file());
        assert!(out.join("Service Worker/Database/keep").is_file());
        assert!(out.join("vm_bundles/claudevm.bundle/sessiondata.vhdx").is_file());
        assert!(!out.join("vm_bundles/claudevm.bundle/rootfs.vhdx").exists());
        assert!(!out.join("Code Cache").exists());
        assert!(!out.join("Service Worker/CacheStorage").exists());
        assert!(!skipped(Path::new("Cachet"), true));
        assert!(skipped(Path::new("code cache\\js"), true));
        assert!(!skipped(Path::new("vm_bundles\\claudevm.bundle"), true));
        assert!(skipped(Path::new("vm_bundles\\claudevm.bundle\\initrd"), false));

        let backups = root.join("backups");
        for stamp in ["9", "10", "100", "1000", "5.partial"] {
            fs::create_dir_all(backups.join(stamp)).unwrap();
            if !stamp.contains('.') {
                fs::write(backups.join(stamp).join("manifest.bin"), b"x").unwrap();
            }
        }
        prune(&backups, KEEP);
        let left: Vec<_> = completed(&backups)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["10", "100", "1000"]);
        assert!(!backups.join("5.partial").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unchanged_large_files_are_linked_from_the_previous_backup() {
        let root = std::env::temp_dir().join(format!(
            "muisland-blob-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let source = root.join("source");
        fs::create_dir_all(source.join("vm_bundles/claudevm.bundle")).unwrap();
        let disk: Vec<u8> = (0..BLOB_MIN as usize + 4321).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(source.join("vm_bundles/claudevm.bundle/sessiondata.vhdx"), &disk).unwrap();
        fs::write(source.join("config.json"), b"{}").unwrap();
        let first = root.join("1");
        snapshot_from(&source, &first, None).unwrap();
        let second = root.join("2");
        snapshot_from(&source, &second, Some(&first)).unwrap();
        // Same blob on disk: modifying one would show in the other.
        let a = fs::metadata(first.join("blob-0.bin")).unwrap();
        let b = fs::metadata(second.join("blob-0.bin")).unwrap();
        assert_eq!(a.len(), b.len());
        assert_eq!(
            kept_size(&[first.clone(), second.clone()]),
            size_of(&first) + size_of(&second) - b.len(),
            "the shared blob counts once"
        );
        let out = root.join("out");
        recover(&second, &out).unwrap();
        assert_eq!(fs::read(out.join("vm_bundles/claudevm.bundle/sessiondata.vhdx")).unwrap(), disk);
        // A changed file is sealed again rather than linked.
        fs::write(source.join("vm_bundles/claudevm.bundle/sessiondata.vhdx"), &disk[..disk.len() - 1]).unwrap();
        let third = root.join("3");
        snapshot_from(&source, &third, Some(&second)).unwrap();
        recover(&third, &root.join("out3")).unwrap();
        assert_eq!(
            fs::read(root.join("out3/vm_bundles/claudevm.bundle/sessiondata.vhdx")).unwrap().len(),
            disk.len() - 1
        );
        // Removing the older backup leaves the linked blob readable.
        fs::remove_dir_all(&first).unwrap();
        recover(&second, &root.join("out2")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    /// `MUISLAND_BENCH_DIR=<copy of Claude's data> cargo test --release backup_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn backup_speed() {
        let Some(source) = std::env::var_os("MUISLAND_BENCH_DIR") else {
            return;
        };
        let dest = std::env::temp_dir().join(format!(
            "muisland-bench-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let start = std::time::Instant::now();
        let mut bytes = 0u64;
        let mut stack = vec![PathBuf::from(&source)];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                let rel = p.strip_prefix(&source).unwrap();
                if skipped(rel, p.is_dir()) {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else {
                    bytes += fs::read(&p).unwrap().len() as u64;
                }
            }
        }
        eprintln!("read only: {:.0} ms ({:.1} MB)", start.elapsed().as_secs_f64() * 1000., bytes as f64 / 1e6);
        let start = std::time::Instant::now();
        snapshot(Path::new(&source), &dest).unwrap();
        let took = start.elapsed();
        eprintln!("backup: {:.0} ms, {:.1} MB on disk", took.as_secs_f64() * 1000., size_of(&dest) as f64 / 1e6);
        let start = std::time::Instant::now();
        recover(&dest, &dest.with_extension("restored")).unwrap();
        eprintln!("restore: {:.0} ms", start.elapsed().as_secs_f64() * 1000.);
        let again = dest.with_extension("again");
        let start = std::time::Instant::now();
        snapshot_from(Path::new(&source), &again, Some(&dest)).unwrap();
        eprintln!("next backup (unchanged large files linked): {:.0} ms", start.elapsed().as_secs_f64() * 1000.);
        let _ = fs::remove_dir_all(&again);
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(dest.with_extension("restored"));
    }
    #[test]
    fn recovery_rejects_escaping_paths() {
        assert!(!relative(Path::new("../Claude/config.json")));
        assert!(!relative(Path::new("C:\\Claude")));
        assert!(relative(Path::new("tasks\\one.json")));
    }
}
