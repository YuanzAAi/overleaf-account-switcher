use super::show_main_window;
use base64::Engine;
use overleaf_browser::detect_chrome_executable;
use std::{fs, path::PathBuf, process::Command, thread, time::Duration};
use tauri::AppHandle;

#[cfg(windows)]
use std::{path::Path, time::Instant};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, LPARAM};
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    VK_CONTROL, VK_L, VK_RETURN,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

const MAX_TEXT_WRITE_BYTES: usize = 16 * 1024 * 1024;

#[tauri::command]
pub(super) fn desktop_write_text_file(path: String, text: String) -> Result<(), String> {
    if text.len() > MAX_TEXT_WRITE_BYTES {
        return Err(format!(
            "text file is too large: {} bytes exceeds {} bytes",
            text.len(),
            MAX_TEXT_WRITE_BYTES
        ));
    }

    write_user_file(path, text.as_bytes())
}

#[tauri::command]
pub(super) fn desktop_write_binary_file(path: String, data: String) -> Result<(), String> {
    if data.len() > 180 * 1024 * 1024 {
        return Err("file exceeds the download size limit".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "invalid file data".to_string())?;
    write_user_file(path, &bytes)
}

fn write_user_file(path: String, bytes: &[u8]) -> Result<(), String> {
    let path = normalize_user_file_path(path)?;
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            return Err(format!(
                "parent directory does not exist: {}",
                parent.display()
            ));
        }
    }
    if path.is_dir() {
        return Err(format!("path points to a directory: {}", path.display()));
    }

    fs::write(&path, bytes).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

#[tauri::command]
pub(super) fn desktop_write_clipboard_text(text: String) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..3 {
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.clone())) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error.to_string());
                if attempt < 2 {
                    thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }
    Err(format!(
        "failed to write clipboard: {}",
        last_error.unwrap_or_else(|| "unknown clipboard error".to_string())
    ))
}

#[tauri::command]
pub(super) fn desktop_focus_main_window(app: AppHandle) {
    show_main_window(&app);
}

#[tauri::command]
pub(super) fn desktop_open_chrome_extensions(profile_name: Option<String>) -> Result<(), String> {
    let chrome_executable =
        detect_chrome_executable().ok_or_else(|| "Chrome executable was not found".to_string())?;
    let profile_name = normalize_chrome_profile_name(profile_name)?;

    #[cfg(windows)]
    {
        open_chrome_extensions_windows(&chrome_executable, profile_name.as_deref())
    }

    #[cfg(not(windows))]
    {
        let mut command = Command::new(&chrome_executable);
        if let Some(profile_name) = profile_name {
            command.arg(format!("--profile-directory={profile_name}"));
        }
        command.arg("chrome://extensions/");
        command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("failed to open Chrome extensions page: {error}"))
    }
}

#[cfg(windows)]
fn open_chrome_extensions_windows(
    chrome_executable: &Path,
    profile_name: Option<&str>,
) -> Result<(), String> {
    let existing_windows = chrome_window_handles();
    let mut command = Command::new(chrome_executable);
    if let Some(profile_name) = profile_name {
        command.arg(format!("--profile-directory={profile_name}"));
    }
    command.args(["--new-window", "about:blank"]);
    command
        .spawn()
        .map_err(|error| format!("failed to open Chrome window: {error}"))?;

    let deadline = Instant::now() + Duration::from_secs(6);
    let window = loop {
        if let Some(window) = chrome_window_handles()
            .into_iter()
            .find(|window| !existing_windows.contains(window))
        {
            break window;
        }
        if Instant::now() >= deadline {
            return Err("Chrome window opened, but its window handle was not detected".to_string());
        }
        thread::sleep(Duration::from_millis(100));
    };

    navigate_chrome_window(window, "chrome://extensions/")
}

#[cfg(windows)]
fn chrome_window_handles() -> Vec<isize> {
    unsafe extern "system" fn collect_chrome_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut class_name = [0_u16; 64];
        let length =
            unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
        if length <= 0 {
            return 1;
        }
        if String::from_utf16_lossy(&class_name[..length as usize]) == "Chrome_WidgetWin_1" {
            let windows = unsafe { &mut *(lparam as *mut Vec<isize>) };
            windows.push(hwnd as isize);
        }
        1
    }

    let mut windows = Vec::new();
    unsafe {
        EnumWindows(
            Some(collect_chrome_window),
            (&mut windows as *mut Vec<isize>) as LPARAM,
        );
    }
    windows
}

#[cfg(windows)]
fn navigate_chrome_window(window: isize, url: &str) -> Result<(), String> {
    let hwnd = window as HWND;
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        if SetForegroundWindow(hwnd) == 0 {
            return Err("Chrome window could not be focused".to_string());
        }
    }
    thread::sleep(Duration::from_millis(180));

    let mut inputs = Vec::with_capacity(6 + url.encode_utf16().count() * 2);
    inputs.push(virtual_key_input(VK_CONTROL, false));
    inputs.push(virtual_key_input(VK_L, false));
    inputs.push(virtual_key_input(VK_L, true));
    inputs.push(virtual_key_input(VK_CONTROL, true));
    for unit in url.encode_utf16() {
        inputs.push(unicode_input(unit, false));
        inputs.push(unicode_input(unit, true));
    }
    inputs.push(virtual_key_input(VK_RETURN, false));
    inputs.push(virtual_key_input(VK_RETURN, true));

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        return Err(format!(
            "Chrome navigation input was incomplete: sent {sent} of {} events",
            inputs.len()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn virtual_key_input(key: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(windows)]
fn unicode_input(unit: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn normalize_chrome_profile_name(profile_name: Option<String>) -> Result<Option<String>, String> {
    let Some(profile_name) = profile_name else {
        return Ok(None);
    };
    let profile_name = profile_name.trim();
    if profile_name.is_empty() {
        return Ok(None);
    }
    if profile_name == "." || profile_name == ".." || profile_name.contains(['/', '\\', ':']) {
        return Err("invalid Chrome profile name".to_string());
    }
    Ok(Some(profile_name.to_string()))
}

fn normalize_user_file_path(path: String) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("path is empty".to_string());
    }

    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {}", path.display()));
    }
    Ok(path)
}
