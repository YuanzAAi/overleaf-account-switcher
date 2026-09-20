#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, LPARAM};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
    ShowWindow, SW_RESTORE,
};

pub fn focus_browser_process_window(process_id: Option<u32>) -> bool {
    let Some(process_id) = process_id else {
        return false;
    };
    focus_browser_process_window_impl(process_id)
}

#[cfg(windows)]
fn focus_browser_process_window_impl(process_id: u32) -> bool {
    struct WindowSearch {
        process_id: u32,
        window: isize,
    }

    unsafe extern "system" fn find_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }

        let search = unsafe { &mut *(lparam as *mut WindowSearch) };
        let mut owner_process_id = 0_u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut owner_process_id);
        }
        if owner_process_id != search.process_id {
            return 1;
        }

        let mut class_name = [0_u16; 64];
        let length =
            unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
        if length <= 0
            || String::from_utf16_lossy(&class_name[..length as usize]) != "Chrome_WidgetWin_1"
        {
            return 1;
        }

        search.window = hwnd as isize;
        0
    }

    let mut search = WindowSearch {
        process_id,
        window: 0,
    };
    unsafe {
        EnumWindows(
            Some(find_window),
            (&mut search as *mut WindowSearch) as LPARAM,
        );
    }
    if search.window == 0 {
        return false;
    }

    let hwnd = search.window as HWND;
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd) != 0
    }
}

#[cfg(not(windows))]
fn focus_browser_process_window_impl(_process_id: u32) -> bool {
    false
}
