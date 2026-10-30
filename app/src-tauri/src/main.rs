// Release builds are GUI apps: no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod browser;
mod commands;
mod manager;
mod native_host;
mod settings;

use manager::DownloadManager;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if native_host::is_native_host_launch(&args) {
        native_host::run();
        return;
    }

    tauri::Builder::default()
        // Must be first: a second launch (e.g. from the extension) hands its
        // args to this window and exits.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(inc) = native_host::parse_add_arg(&args) {
                app.state::<Arc<DownloadManager>>().incoming(inc);
            } else {
                show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            let downloads = app.path().download_dir().unwrap_or_else(|_| data_dir.join("Downloads"));
            let mgr = DownloadManager::new(app.handle().clone(), data_dir.clone(), downloads)?;
            if let Err(e) = browser::register(&data_dir) {
                eprintln!("browser integration not registered: {e:#}");
            }
            mgr.start_ticker();
            if let Some(inc) = native_host::parse_add_arg(&args) {
                mgr.incoming(inc);
            }
            app.manage(mgr);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::probe_url,
            commands::default_dir,
            commands::add_download,
            commands::pause,
            commands::resume,
            commands::remove,
            commands::open_file,
            commands::open_folder,
            commands::set_settings,
            commands::open_url,
            commands::extension_dir,
            commands::open_extension_folder,
            commands::pick_folder,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                window.app_handle().state::<Arc<DownloadManager>>().save();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running SwiftDL");
}
