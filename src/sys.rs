//! Small Windows helpers: single instance, focusing windows, processes, start with Windows,
//! and the keyboard hook used by reactive effects.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateMutexW, OpenMutexW, OpenProcess, TerminateProcess, PROCESS_TERMINATE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, FindWindowW, SetForegroundWindow, SetWindowsHookExW, ShowWindow, UnhookWindowsHookEx, HHOOK,
    KBDLLHOOKSTRUCT, SW_RESTORE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

pub const APP_TITLE: &str = "Keyboard Custom Software";
pub const DAEMON_MUTEX: &str = "Local\\KeyboardCustomSoftwareService";
pub const GUI_MUTEX: &str = "Local\\KeyboardCustomSoftwareWindow";
pub const VENDOR_EXE: &str = "SURMENV400.exe";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Holds a named mutex for the life of the process. None = another instance owns it.
pub struct InstanceLock(HANDLE);

impl InstanceLock {
    pub fn acquire(name: &str) -> Option<Self> {
        let n = wide(name);
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, n.as_ptr());
            if h.is_null() {
                return None;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(h);
                return None;
            }
            Some(InstanceLock(h))
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Is a process holding this named mutex?
pub fn instance_running(name: &str) -> bool {
    let n = wide(name);
    unsafe {
        let h = OpenMutexW(0x0010_0000, 0, n.as_ptr()); // SYNCHRONIZE
        if h.is_null() {
            false
        } else {
            CloseHandle(h);
            true
        }
    }
}

/// Bring our settings window to the front if it's open.
pub fn focus_window(title: &str) -> bool {
    let t = wide(title);
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), t.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
        true
    }
}

pub fn spawn_self(args: &[&str]) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(args).spawn();
    }
}

fn find_processes(exe: &str) -> Vec<u32> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..len]);
                if name.eq_ignore_ascii_case(exe) {
                    out.push(e.th32ProcessID);
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

pub fn process_running(exe: &str) -> bool {
    !find_processes(exe).is_empty()
}

pub fn kill_process(exe: &str) {
    for pid in find_processes(exe) {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, 0);
                CloseHandle(h);
            }
        }
    }
}

// ---------------------------------------------------------------- start with Windows

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_NAME: &str = "KeyboardCustomSoftware";

pub fn autostart_enabled() -> bool {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(RUN_NAME))
        .is_ok()
}

pub fn set_autostart(on: bool) {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    if let Ok((key, _)) = hkcu.create_subkey(RUN_KEY) {
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let _ = key.set_value(RUN_NAME, &format!("\"{}\" --background", exe.display()));
            }
        } else {
            let _ = key.delete_value(RUN_NAME);
        }
    }
}

// ---------------------------------------------------------------- keyboard hook
// Only installed while a reactive effect is active. It only forwards which key
// went down to the lighting engine; nothing is stored or logged.

static KEY_TX: OnceLock<Sender<u16>> = OnceLock::new();
static DOWN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

unsafe extern "system" fn hook_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        let vk = (kb.vkCode & 0xFF) as usize;
        match wparam as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // ignore auto-repeat while held
                if !DOWN[vk].swap(true, Ordering::Relaxed) {
                    if let Some(tx) = KEY_TX.get() {
                        let _ = tx.send(vk as u16);
                    }
                }
            }
            WM_KEYUP | WM_SYSKEYUP => DOWN[vk].store(false, Ordering::Relaxed),
            _ => {}
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

pub struct KeyHook(HHOOK);

impl KeyHook {
    /// Must be called on a thread that runs a message loop.
    pub fn install(tx: &Sender<u16>) -> Option<Self> {
        let _ = KEY_TX.set(tx.clone());
        unsafe {
            let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(std::ptr::null()), 0);
            if h.is_null() {
                None
            } else {
                Some(KeyHook(h))
            }
        }
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        unsafe {
            UnhookWindowsHookEx(self.0);
        }
    }
}

/// Local time of day as 0..1 (0 = midnight, 0.5 = noon).
pub fn local_day_fraction() -> f32 {
    let mut st = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st) };
    let secs = st.wHour as f32 * 3600.0 + st.wMinute as f32 * 60.0 + st.wSecond as f32 + st.wMilliseconds as f32 / 1000.0;
    secs / 86400.0
}
