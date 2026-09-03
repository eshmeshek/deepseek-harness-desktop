//! DSH Desktop - an independent desktop wrapper for DeepSeek Harness.
//!
//! It contributes the three things upstream deliberately leaves out, without
//! patching a line of it:
//!
//! * a background host that outlives the window, so a running task is not tied
//!   to whether the UI is open;
//! * a tray icon whose Quit is the one full stop;
//! * an update prompt, because upstream ships no updater at all.
//!
//! This wrapper contributes **no interface of its own**: the only page ever
//! shown is the harness's own web UI, loaded from the local host process.
//! Everything this app has to say - progress, failures, update offers - it says
//! through native OS dialogs. That keeps the surface small and leaves the
//! product's look entirely upstream's.
//!
//! Upstream is consumed exactly as published on npm, which is what keeps an
//! update a matter of fetching a newer upstream rather than rebasing local
//! changes.

#[macro_use]
mod log;
mod node;
mod orphan;
mod pnpm;
mod proc;
mod resources;
mod runtime;
mod supervisor;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use runtime::Channel;

const APP_TITLE: &str = "DSH Desktop";

pub struct Shared {
    host: Mutex<Option<supervisor::Host>>,
    /// URL of the running host, token included. `None` until startup finishes.
    url: Mutex<Option<String>>,
    runtimes: PathBuf,
    logs: PathBuf,
    data: PathBuf,
    workspace: PathBuf,
}

/// Bring up the harness UI, reusing the window if it already exists.
///
/// The window loads the harness's own page directly; this app renders nothing.
fn show_main_window(app: &AppHandle, url: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let Ok(parsed) = url.parse() else { return };
    let built = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(parsed))
        .title("DeepSeek Harness")
        .inner_size(1400.0, 900.0)
        .min_inner_size(800.0, 600.0)
        .build();

    if let Ok(window) = built {
        let handle = window.clone();
        window.on_window_event(move |event| {
            // Closing the window must not stop the host: that is the entire
            // point of this wrapper. Hide instead, and leave Quit to the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = handle.hide();
            }
        });
    }
}

/// Short status shown on hover; the tray tooltip is this app's only ambient UI.
fn set_tooltip(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("tray") {
        let _ = tray.set_tooltip(Some(text));
    }
}

fn error_dialog(app: &AppHandle, message: &str) {
    app.dialog()
        .message(message)
        .title(APP_TITLE)
        .kind(MessageDialogKind::Error)
        .blocking_show();
}

/// Install the runtime if needed, offer an update, start the host, open the UI.
///
/// Runs off the main thread for two reasons: a first run downloads a whole npm
/// tree, and the blocking dialogs below must never block the event loop.
fn boot(app: AppHandle, shared: Arc<Shared>) {
    std::thread::spawn(move || {
        if let Err(error) = boot_inner(&app, &shared) {
            log_line!("startup failed: {error:#}");
            set_tooltip(&app, "DSH Desktop — not running");
            error_dialog(
                &app,
                &format!("Could not start DeepSeek Harness.\n\n{error:#}"),
            );
        }
    });
}

fn boot_inner(app: &AppHandle, shared: &Arc<Shared>) -> anyhow::Result<()> {
    set_tooltip(app, "DSH Desktop — starting…");
    log_line!("--- startup ---");
    // Before anything touches the runtime directory: a host left over from a
    // hard-killed previous run still holds it open.
    if let Some(what) = orphan::reap(&shared.data) {
        log_line!("{what}");
    }
    let (node, origin) = node::locate(app)?;
    log_line!("node: {} ({origin:?})", node.display());
    let pnpm = pnpm::locate(app)?;
    log_line!("pnpm: {}", pnpm.display());

    std::fs::create_dir_all(&shared.runtimes)?;

    // A release ships a harness inside the installer, so there is nothing to
    // download here and no network needed. Only a development build, which
    // stages no resources, has to fetch one.
    let mut current = runtime::newest(app, &shared.runtimes);
    if current.is_none() {
        set_tooltip(app, "DSH Desktop — installing DeepSeek Harness…");
        let version = runtime::published_version(Channel::Latest)?;
        log_line!("no bundled harness (development build); installing {version}");
        runtime::install(&node, &pnpm, &shared.runtimes, &version)?;
        current = runtime::newest(app, &shared.runtimes);
    }
    let current =
        current.ok_or_else(|| anyhow::anyhow!("no DeepSeek Harness version is available"))?;

    offer_update(app, shared, &node, &pnpm, &current.version);

    // Re-resolve: an accepted update just added a newer one.
    let chosen = runtime::newest(app, &shared.runtimes)
        .ok_or_else(|| anyhow::anyhow!("no DeepSeek Harness version is available"))?;
    let version = chosen.version.clone();
    log_line!(
        "runtime {version} ({})",
        if chosen.bundled {
            "bundled"
        } else {
            "downloaded"
        }
    );

    set_tooltip(app, &format!("DSH Desktop — starting {version}…"));
    let bin = chosen.bin;
    log_line!("starting host {version}");
    let host = supervisor::start(&node, &bin, &shared.workspace, &shared.logs)?;
    let host_pid = host.pid();
    log_line!("host up (pid {host_pid})");
    let url = host.url.clone();

    orphan::record(&shared.data, host_pid);
    *shared.url.lock().unwrap() = Some(url.clone());
    *shared.host.lock().unwrap() = Some(host);
    set_tooltip(app, &format!("DeepSeek Harness {version} is running"));

    watch_host(app.clone(), shared.clone());
    show_main_window(app, &url);
    Ok(())
}

/// Notice when the host dies on its own.
///
/// Without this the tray would keep advertising a service that is gone, and the
/// window would sit on a dead page - the failure mode that looks most like
/// "it works" while nothing does.
fn watch_host(app: AppHandle, shared: Arc<Shared>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(2));

        // Take the exit code and release the lock before touching any UI:
        // a blocking dialog must never be shown while holding it.
        let exit = {
            let mut guard = shared.host.lock().unwrap();
            match guard.as_mut() {
                Some(host) => match host.exited() {
                    Ok(Some(code)) => {
                        *guard = None;
                        Some(code)
                    }
                    _ => None,
                },
                // Quit already cleared it; nothing left to watch.
                None => return,
            }
        };

        let Some(code) = exit else { continue };
        orphan::clear(&shared.data);
        *shared.url.lock().unwrap() = None;
        set_tooltip(&app, "DSH Desktop — backend stopped");
        if let Some(window) = app.get_webview_window("main") {
            // destroy(), not close(): close() re-enters the CloseRequested
            // guard above, which exists precisely to refuse closing.
            let _ = window.destroy();
        }
        error_dialog(
            &app,
            &format!(
                "The DeepSeek Harness backend stopped (exit code {code}).\n\nDetails are in the logs: \"Show logs\" in the tray menu."
            ),
        );
        return;
    });
}

/// Ask npm whether a newer version exists and, if the user agrees, install it.
///
/// Deliberately a question, never automatic: upstream is a developer preview
/// that announces compatibility-breaking changes, so the person using it decides
/// when to take one. Being offline is not an error - it just means no offer.
fn offer_update(app: &AppHandle, shared: &Arc<Shared>, node: &Path, pnpm: &Path, current: &str) {
    let Ok(published) = runtime::published_version(Channel::Latest) else {
        return;
    };
    // Only ever forwards: a dist-tag can move backwards, and being offered a
    // downgrade as an "update" would be worse than being offered nothing.
    if runtime::compare(&published, current) != std::cmp::Ordering::Greater {
        return;
    }

    let accepted = app
        .dialog()
        .message(format!(
            "A newer DeepSeek Harness is available: {published}.\nInstalled: {current}.\n\nUpdate now?\n\nThe current version stays on disk, so you can go back to it."
        ))
        .title(APP_TITLE)
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::YesNo)
        .blocking_show();

    if !accepted {
        return;
    }

    set_tooltip(app, &format!("DSH Desktop — updating to {published}…"));
    if let Err(error) = runtime::install(node, pnpm, &shared.runtimes, &published) {
        // A failed update is not a failed start: the previous version is intact
        // and untouched, so say so and carry on with it.
        error_dialog(
            app,
            &format!("Updating to {published} failed; staying on {current}.\n\n{error:#}"),
        );
        return;
    }
    // Keep one previous version as a rollback target.
    let _ = runtime::prune(&shared.runtimes, 2);
}

/// Re-run the update check on demand, from the tray.
fn check_for_updates(app: &AppHandle, shared: &Arc<Shared>) {
    let app = app.clone();
    let shared = shared.clone();
    std::thread::spawn(move || {
        let (node, pnpm) = match (node::locate(&app), pnpm::locate(&app)) {
            (Ok((node, _)), Ok(pnpm)) => (node, pnpm),
            (Err(error), _) | (_, Err(error)) => return error_dialog(&app, &format!("{error:#}")),
        };
        let current = runtime::newest(&app, &shared.runtimes)
            .map(|r| r.version)
            .unwrap_or_default();
        match runtime::published_version(Channel::Latest) {
            Ok(published)
                if runtime::compare(&published, &current) == std::cmp::Ordering::Greater =>
            {
                offer_update(&app, &shared, &node, &pnpm, &current);
                app.dialog()
                    .message("The update is installed. It takes effect after a restart: choose Quit in the tray, then start the app again.")
                    .title(APP_TITLE)
                    .blocking_show();
            }
            Ok(published) => {
                app.dialog()
                    .message(format!("You are on the latest version: {published}."))
                    .title(APP_TITLE)
                    .blocking_show();
            }
            Err(error) => error_dialog(&app, &format!("Could not check for updates.\n\n{error:#}")),
        }
    });
}

fn build_tray(app: &AppHandle, shared: Arc<Shared>) -> tauri::Result<TrayIcon> {
    let open = MenuItem::with_id(app, "open", "Open DeepSeek Harness", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "update", "Check for updates…", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "Show logs", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start at login",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit (stops the backend)", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &update,
            &autostart,
            &logs,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id("tray")
        .icon(tauri::include_image!("icons/32x32.png"))
        .tooltip(APP_TITLE)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => {
                let url = shared.url.lock().unwrap().clone();
                match url {
                    Some(url) => show_main_window(app, &url),
                    None => {
                        app.dialog()
                            .message("DeepSeek Harness is still starting.")
                            .title(APP_TITLE)
                            .show(|_| {});
                    }
                }
            }
            "update" => check_for_updates(app, &shared),
            "logs" => open_in_file_manager(&shared.logs),
            "autostart" => {
                let manager = app.autolaunch();
                let enabled = manager.is_enabled().unwrap_or(false);
                let _ = if enabled {
                    manager.disable()
                } else {
                    manager.enable()
                };
            }
            "quit" => {
                shutdown(app, &shared);
            }
            _ => {}
        })
        .build(app)
}

/// Stop the backend and quit.
///
/// Shared by the tray's Quit and by the signal handler below, because a stop is
/// a stop however it was asked for. Clearing the host slot matters as much as
/// killing the process: it is the watchdog's signal to stand down, so it cannot
/// pop a dialog in the middle of shutting down.
fn shutdown(app: &AppHandle, shared: &Arc<Shared>) {
    let mut guard = shared.host.lock().unwrap();
    if let Some(host) = guard.as_mut() {
        host.stop();
    }
    *guard = None;
    drop(guard);
    orphan::clear(&shared.data);
    app.exit(0);
}

/// Treat SIGTERM and SIGINT as a request to quit.
///
/// On Windows the exit event runs on its own; on Unix nothing translates a
/// signal into one, so the process would die with its shutdown handler unrun and
/// the backend left orphaned. That is not an exotic path: it is what a logout,
/// a shutdown, `systemctl --user stop`, or `pkill` all do. The next launch would
/// reap the orphan, but until then a stray host keeps running.
#[cfg(unix)]
fn handle_signals(app: AppHandle, shared: Arc<Shared>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    std::thread::spawn(move || {
        let Ok(mut signals) = Signals::new([SIGTERM, SIGINT]) else {
            log_line!("cannot install signal handlers; a logout will orphan the host");
            return;
        };
        if let Some(signal) = signals.forever().next() {
            log_line!("signal {signal} received, stopping the backend");
            shutdown(&app, &shared);
        }
    });
}

#[cfg(not(unix))]
fn handle_signals(_app: AppHandle, _shared: Arc<Shared>) {}

fn open_in_file_manager(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(path).spawn();
}

pub fn run() {
    tauri::Builder::default()
        // Registered first, as the plugin requires. Without it every launch of
        // the shortcut starts another app: they race each other installing the
        // runtime into the same staging directory and corrupt it, each starts
        // its own host, and each overwrites the single host record - stranding
        // the previous host with nothing able to reap it.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            log_line!("second launch: showing the existing window");
            let url = app
                .try_state::<Arc<Shared>>()
                .and_then(|shared| shared.url.lock().unwrap().clone());
            match url {
                Some(url) => show_main_window(app, &url),
                None => {
                    app.dialog()
                        .message("DeepSeek Harness is still starting.")
                        .title(APP_TITLE)
                        .show(|_| {});
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let handle = app.handle().clone();
            let data = handle.path().app_local_data_dir()?;
            let shared = Arc::new(Shared {
                host: Mutex::new(None),
                url: Mutex::new(None),
                runtimes: data.join("runtimes"),
                logs: data.join("logs"),
                data: data.clone(),
                // The harness treats its working directory as the default
                // filesystem location; home is the least surprising choice for
                // an app launched from a desktop icon.
                workspace: handle.path().home_dir().unwrap_or_else(|_| data.clone()),
            });

            log::init(&shared.logs);
            app.manage(shared.clone());
            build_tray(&handle, shared.clone())?;
            handle_signals(handle.clone(), shared.clone());
            boot(handle.clone(), shared);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("cannot start DSH Desktop")
        .run(|app, event| match event {
            // Closing the last window must leave the service running in the
            // tray. Only an explicit exit (which carries a code) may pass.
            tauri::RunEvent::ExitRequested { api, code, .. } if code.is_none() => {
                api.prevent_exit();
            }
            tauri::RunEvent::Exit => {
                if let Some(shared) = app.try_state::<Arc<Shared>>() {
                    if let Some(host) = shared.host.lock().unwrap().as_mut() {
                        host.stop();
                    }
                }
            }
            _ => {}
        });
}
