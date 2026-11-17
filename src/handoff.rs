//! Finds the folders of Cowork tasks being handed off between accounts.
//!
//! Claude keeps no local record of which folder each Cowork session works in,
//! but the handoff request makes Cowork save `HANDOFF.md` at the top of that
//! folder. Watching the fixed drives for that file therefore gives the real
//! Windows path of every task, one per project, without guessing. (Cowork runs
//! in a VM and sees different paths, so a path written *inside* the file would
//! not open on Windows.)
use std::{
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    time::{Duration, SystemTime},
};

pub const FILE_NAME: &str = "HANDOFF.md";

/// The project folder for a changed path, if it is a `HANDOFF.md` worth using.
pub fn project_folder(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    if !name.eq_ignore_ascii_case(FILE_NAME) {
        return None;
    }
    let text = path.to_string_lossy().to_ascii_lowercase();
    // Deleted files, Muisland's own data and temporary copies are not projects.
    if ["\\$recycle.bin\\", "\\appdata\\local\\temp\\", "\\claudeisland\\"]
        .iter()
        .any(|skip| text.contains(skip))
    {
        return None;
    }
    path.parent().map(Path::to_path_buf)
}
/// Whether `path` is a regular `HANDOFF.md` written at or after `since`.
pub fn fresh(path: &Path, since: SystemTime) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| {
        m.is_file()
            && m.file_attributes() & 0x400 == 0
            && m.modified().is_ok_and(|t| t + Duration::from_secs(2) >= since)
    })
}
/// Local fixed drives (C:\, D:\ …): where Cowork project folders live.
pub fn fixed_drives() -> Vec<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|root| {
            let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            unsafe { GetDriveTypeW(wide.as_ptr()) == 3 } // DRIVE_FIXED
        })
        .map(PathBuf::from)
        .collect()
}
/// Watches `root` and everything under it, sending each changed `HANDOFF.md`
/// path to `found` and calling `notify` after each one. Runs until the
/// receiver is gone; meant for its own thread.
pub fn watch(root: PathBuf, found: Sender<PathBuf>, notify: impl Fn()) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_NOTIFY_INFORMATION,
    };
    let wide: Vec<u16> = root
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0x0001,             // FILE_LIST_DIRECTORY
            0x1 | 0x2 | 0x4,    // share read, write, delete
            std::ptr::null(),
            3,                  // OPEN_EXISTING
            0x0200_0000,        // FILE_FLAG_BACKUP_SEMANTICS (needed for folders)
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return;
    }
    // DWORD-aligned buffer, as the API requires.
    let mut buffer = vec![0u32; 16 * 1024];
    loop {
        let mut bytes = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                handle,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 4) as u32,
                1,               // whole tree
                0x1 | 0x10,      // file names and last writes
                &mut bytes,
                std::ptr::null_mut(),
                None,
            )
        };
        if ok == 0 {
            break;
        }
        // bytes == 0 means the buffer overflowed: some events were lost, and
        // the folder-less fallback (Choose folder) still covers that case.
        let base = buffer.as_ptr() as *const u8;
        let mut offset = 0usize;
        while bytes > 0 && offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() <= bytes as usize {
            let info = unsafe { &*(base.add(offset) as *const FILE_NOTIFY_INFORMATION) };
            let len = info.FileNameLength as usize / 2;
            let name = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), len) };
            // Added, modified, or renamed into place.
            if matches!(info.Action, 1 | 3 | 5) {
                let rel = String::from_utf16_lossy(name);
                if rel.len() >= FILE_NAME.len()
                    && rel[rel.len() - FILE_NAME.len()..].eq_ignore_ascii_case(FILE_NAME)
                {
                    if found.send(root.join(&rel)).is_err() {
                        unsafe { CloseHandle(handle) };
                        return;
                    }
                    notify();
                }
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            offset += info.NextEntryOffset as usize;
        }
    }
    unsafe { CloseHandle(handle) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_project_handoff_files_count() {
        assert_eq!(
            project_folder(Path::new("D:\\muhammad\\projects\\crm\\HANDOFF.md")),
            Some(PathBuf::from("D:\\muhammad\\projects\\crm"))
        );
        assert_eq!(
            project_folder(Path::new("D:\\work\\handoff.MD")),
            Some(PathBuf::from("D:\\work"))
        );
        assert!(project_folder(Path::new("D:\\work\\HANDOFF.md.tmp")).is_none());
        assert!(project_folder(Path::new("D:\\work\\NOT-HANDOFF.md")).is_none());
        assert!(project_folder(Path::new("C:\\$Recycle.Bin\\S-1\\HANDOFF.md")).is_none());
        assert!(project_folder(Path::new(
            "C:\\Users\\u\\AppData\\Local\\Temp\\x\\HANDOFF.md"
        ))
        .is_none());
    }

    #[test]
    fn watching_a_folder_reports_a_new_handoff_file() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "handoff-watch-{}",
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let project = root.join("projects").join("crm");
        std::fs::create_dir_all(&project).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let watched = root.clone();
        std::thread::spawn(move || watch(watched, tx, || {}));
        std::thread::sleep(Duration::from_millis(300));
        let since = SystemTime::now();
        std::fs::write(project.join("notes.md"), b"x").unwrap();
        std::fs::write(project.join("HANDOFF.md"), b"# Handoff").unwrap();
        let got = rx.recv_timeout(Duration::from_secs(5)).expect("no event");
        assert_eq!(project_folder(&got), Some(project.clone()));
        assert!(fresh(&got, since));
        drop(rx);
        let _ = std::fs::remove_dir_all(root);
    }
}
