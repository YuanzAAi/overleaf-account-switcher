#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, Url, WindowEvent,
};

mod commands;
mod service;
mod updates;
use service::{ensure_local_service, ManagedService, SERVICE_HOST, SERVICE_PORT};

const DEFAULT_DESKTOP_UI_URL: &str = "http://127.0.0.1:8765/?v=20260821-runtime158";
const DESKTOP_URL_OVERRIDE_ENV: &str = "OVERLEAF_SWITCHER_DESKTOP_URL";
const MAIN_WINDOW_LABEL: &str = "main";
const TRAY_ID: &str = "overleaf-account-switcher";
const TRAY_OPEN_MENU_ID: &str = "tray-open";
const TRAY_EXIT_MENU_ID: &str = "tray-exit";
const DESKTOP_BRIDGE_SCRIPT: &str = r#"
(() => {
  const tauriInvoke =
    (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) ||
    (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke);
  if (typeof tauriInvoke !== "function") {
    return;
  }
  const existing = window.__OVERLEAF_DESKTOP__ || {};
  window.__OVERLEAF_DESKTOP__ = {
    ...existing,
    openDialog: (options = {}) => tauriInvoke("plugin:dialog|open", { options }),
    saveDialog: (options = {}) => tauriInvoke("plugin:dialog|save", { options }),
    writeTextFile: (path, text) => tauriInvoke("desktop_write_text_file", { path, text }),
    writeBinaryFile: (path, data) => tauriInvoke("desktop_write_binary_file", { path, data }),
    writeClipboardText: (text) => tauriInvoke("desktop_write_clipboard_text", { text }),
    focusMainWindow: () => tauriInvoke("desktop_focus_main_window"),
    installUpdate: (tag) => tauriInvoke("desktop_install_update", { tag }),
    openChromeExtensions: (profileName) =>
      tauriInvoke("desktop_open_chrome_extensions", { profileName }),
  };
})();
"#;

#[derive(Default)]
struct DesktopLifecycle {
    explicit_exit: AtomicBool,
}

impl DesktopLifecycle {
    fn should_hide_on_close(&self) -> bool {
        !self.explicit_exit.load(Ordering::SeqCst)
    }

    fn request_exit(&self) {
        self.explicit_exit.store(true, Ordering::SeqCst);
    }
}

fn main() {
    if env::args().nth(1).as_deref() == Some("--service") {
        if let Err(error) = overleaf_service::runtime::server::run() {
            eprintln!("service failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    let override_url = desktop_url_override();
    let managed_service = ManagedService::default();
    let service_startup = override_url.is_none().then(|| {
        let service = managed_service.clone();
        let (sender, receiver) = mpsc::channel::<AppHandle>();
        thread::spawn(move || {
            ensure_local_service(&service);
            let Ok(app_handle) = receiver.recv() else {
                service.stop();
                return;
            };
            let dispatcher = app_handle.clone();
            let _ = dispatcher.run_on_main_thread(move || {
                if app_handle
                    .state::<DesktopLifecycle>()
                    .explicit_exit
                    .load(Ordering::SeqCst)
                {
                    return;
                }
                if let Some(window) = app_handle.get_webview_window(MAIN_WINDOW_LABEL) {
                    if let Err(error) = window.navigate(default_desktop_ui_url()) {
                        eprintln!("failed to navigate to local service: {error}");
                    }
                }
            });
        });
        sender
    });
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(managed_service)
        .manage(DesktopLifecycle::default())
        .setup(move |app| {
            setup_tray(app)?;
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                show_main_window(app.handle());
                if let Some(url) = override_url {
                    window.navigate(url.clone())?;
                    schedule_desktop_url_override(app.handle().clone(), url);
                }
            } else {
                eprintln!("main desktop window was not found for UI navigation");
            }
            if let Some(sender) = service_startup {
                let _ = sender.send(app.handle().clone());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW_LABEL {
                return;
            }
            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            let lifecycle = window.state::<DesktopLifecycle>();
            if !lifecycle.should_hide_on_close() {
                return;
            }
            match window.hide() {
                Ok(()) => {
                    api.prevent_close();
                }
                Err(error) => eprintln!("failed to hide main window to tray: {error}"),
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::desktop_write_text_file,
            commands::desktop_write_binary_file,
            commands::desktop_write_clipboard_text,
            commands::desktop_focus_main_window,
            commands::desktop_open_chrome_extensions,
            updates::desktop_install_update
        ])
        .on_page_load(|webview, _| {
            if let Err(error) = webview.eval(DESKTOP_BRIDGE_SCRIPT) {
                eprintln!("failed to inject desktop bridge: {error}");
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build Overleaf Account Switcher desktop shell")
        .run(|app, event| match event {
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_main_window(app),
            tauri::RunEvent::Exit => app.state::<ManagedService>().stop(),
            _ => {}
        });
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, TRAY_OPEN_MENU_ID, "打开", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, TRAY_EXIT_MENU_ID, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &exit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("Overleaf 账号控制台")
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_OPEN_MENU_ID => show_main_window(app),
            TRAY_EXIT_MENU_ID => {
                app.state::<DesktopLifecycle>().request_exit();
                app.exit(0);
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

fn schedule_desktop_url_override(app_handle: AppHandle, url: Url) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(350));
        let dispatcher = app_handle.clone();
        let lookup_handle = app_handle.clone();
        let _ = dispatcher.run_on_main_thread(move || {
            if let Some(window) = lookup_handle.get_webview_window(MAIN_WINDOW_LABEL) {
                if let Err(error) = window.navigate(url) {
                    eprintln!("failed to apply delayed desktop URL override: {error}");
                }
            }
        });
    });
}

fn default_desktop_ui_url() -> Url {
    Url::parse(DEFAULT_DESKTOP_UI_URL).expect("default desktop UI URL must remain valid")
}

fn is_local_service_url(url: &Url) -> bool {
    url.host_str() == Some(SERVICE_HOST) && url.port_or_known_default() == Some(SERVICE_PORT)
}

fn desktop_url_override() -> Option<Url> {
    let raw = env::var(DESKTOP_URL_OVERRIDE_ENV).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    match Url::parse(trimmed) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => Some(url),
        Ok(url) => {
            eprintln!(
                "{DESKTOP_URL_OVERRIDE_ENV} ignored: unsupported URL scheme '{}'",
                url.scheme()
            );
            None
        }
        Err(error) => {
            eprintln!("{DESKTOP_URL_OVERRIDE_ENV} ignored: invalid URL: {error}");
            None
        }
    }
}
