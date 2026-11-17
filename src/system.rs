//! Shell integration: notification-area icon, balloon alerts and sign-in startup.
use crate::gfx::Surface;
use std::{cell::Cell, mem::zeroed, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{CreateBitmap, DeleteObject},
    System::Registry::*,
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub const TRAY_MESSAGE: u32 = WM_APP + 7;
const TRAY_ID: u32 = 1;
thread_local! {
    static ICON: Cell<HICON> = const { Cell::new(null_mut()) };
    static ADDED: Cell<bool> = const { Cell::new(false) };
}

fn copy_into(dest: &mut [u16], text: &str) {
    let chars: Vec<u16> = text.encode_utf16().take(dest.len() - 1).collect();
    dest[..chars.len()].copy_from_slice(&chars);
    dest[chars.len()] = 0;
}
/// Converts a premultiplied render into an alpha icon.
fn icon_from(surface: &Surface) -> HICON {
    unsafe {
        let Some(color) = Surface::new(surface.w, surface.h) else {
            return null_mut();
        };
        let src = surface.pixels();
        let dst = std::slice::from_raw_parts_mut(color.bits, (surface.w * surface.h * 4) as usize);
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let a = s[3] as u32;
            for c in 0..3 {
                d[c] = if a == 0 {
                    0
                } else {
                    ((s[c] as u32 * 255 + a / 2) / a).min(255) as u8
                };
            }
            d[3] = s[3];
        }
        let mask_bits = vec![0u8; ((surface.w + 15) / 16 * 2 * surface.h) as usize];
        let mask = CreateBitmap(surface.w, surface.h, 1, 1, mask_bits.as_ptr().cast());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color.hbitmap_raw(),
        };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        icon
    }
}
/// Adds or updates the notification-area icon. Returns false if the shell refused it.
pub fn tray_set(hwnd: HWND, image: &Surface, tip: &str) -> bool {
    let icon = icon_from(image);
    if icon.is_null() {
        return false;
    }
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        data.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        data.hIcon = icon;
        copy_into(&mut data.szTip, tip);
        let ok = if ADDED.with(|a| a.get()) {
            Shell_NotifyIconW(NIM_MODIFY, &data) != 0
        } else {
            let added = Shell_NotifyIconW(NIM_ADD, &data) != 0;
            if added {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                Shell_NotifyIconW(NIM_SETVERSION, &data);
                ADDED.with(|a| a.set(true));
            }
            added
        };
        let old = ICON.with(|c| c.replace(icon));
        if !old.is_null() {
            DestroyIcon(old);
        }
        ok
    }
}
/// Explorer restarted: the icon must be added again.
pub fn tray_reset() {
    ADDED.with(|a| a.set(false));
}
/// Message id broadcast when the taskbar is recreated.
pub fn taskbar_created() -> u32 {
    let name: Vec<u16> = "TaskbarCreated\0".encode_utf16().collect();
    unsafe { RegisterWindowMessageW(name.as_ptr()) }
}
pub fn tray_remove(hwnd: HWND) {
    if !ADDED.with(|a| a.replace(false)) {
        return;
    }
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        Shell_NotifyIconW(NIM_DELETE, &data);
        let old = ICON.with(|c| c.replace(null_mut()));
        if !old.is_null() {
            DestroyIcon(old);
        }
    }
}
/// Shows a Windows notification from the tray icon, if present.
pub fn tray_balloon(hwnd: HWND, title: &str, body: &str) {
    if !ADDED.with(|a| a.get()) {
        return;
    }
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_into(&mut data.szInfoTitle, title);
        copy_into(&mut data.szInfo, body);
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0";
const RUN_VALUE: &str = "Muisland\0";
fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
/// True when the current executable is registered to start at sign-in.
pub fn launch_at_login() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w(RUN_KEY).as_ptr(),
            w(RUN_VALUE).as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return false;
    }
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..len]);
    value
        .trim_matches('"')
        .eq_ignore_ascii_case(&exe.to_string_lossy())
}
pub fn set_launch_at_login(enabled: bool) -> std::io::Result<()> {
    unsafe {
        let mut key: HKEY = null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w(RUN_KEY).as_ptr(),
            0,
            null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null_mut(),
            &mut key,
            null_mut(),
        );
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }
        let result = if enabled {
            let exe = std::env::current_exe()?;
            let value = w(&format!("\"{}\"\0", exe.to_string_lossy()));
            RegSetValueExW(
                key,
                w(RUN_VALUE).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } else {
            let status = RegDeleteValueW(key, w(RUN_VALUE).as_ptr());
            if status == 2 {
                0
            } else {
                status
            }
        };
        RegCloseKey(key);
        if result != 0 {
            return Err(std::io::Error::from_raw_os_error(result as i32));
        }
    }
    Ok(())
}
