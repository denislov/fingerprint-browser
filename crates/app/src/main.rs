//! Fingerprint Browser — application bootstrap.
//!
//! Owns process-wide wiring only: storage, the runtime supervisor thread, the
//! services over them, and the single window. No child process is ever owned by
//! the UI; commands go through [`RuntimeService`] into the supervisor channel.

mod core_detect;
#[cfg(all(test, target_os = "linux"))]
mod real_browser;
mod state;
mod ui;

use application::{DefaultProfileService, ProfileService, RuntimeService};
use domain::BrowserCore;
use gpui_kit::component::Root;
use gpui_kit::*;
use runtime::{
    ChannelRuntimeFacade, RuntimeCommand, RuntimeFacade, RuntimeSupervisor,
    RuntimeSupervisorChannels, SupervisorComponents,
};
use state::AppState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use storage::{CoreRepository, ProfileRepository, ProxyRepository, SqliteStorage};
use ui::AppView;

/// Data root. Overridable so tests and portable installs can relocate it.
const DATA_DIR_ENV: &str = "FP_BROWSER_DATA_DIR";
const DEFAULT_DATA_DIR: &str = "data";
/// Explicit Xray executable for per-profile proxying.
const XRAY_BIN_ENV: &str = "FP_BROWSER_XRAY_BIN";
const EVENT_CAPACITY: usize = 256;
/// How long shutdown waits for the supervisor to reclaim children.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

fn main() {
    tracing_subscriber::fmt::init();

    let data_dir = std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));

    let storage = SqliteStorage::open(data_dir.join("app.db")).expect("open sqlite storage");
    let profile_repo: Arc<dyn ProfileRepository> = Arc::new(storage.profiles());
    let core_repo: Arc<dyn CoreRepository> = Arc::new(storage.cores());
    let proxy_repo: Arc<dyn ProxyRepository> = Arc::new(storage.proxies());

    let core_notice = ensure_core(core_repo.as_ref());

    let channels = RuntimeSupervisorChannels::new(EVENT_CAPACITY);
    let event_rx = channels.event_rx.clone();
    let snapshots = Arc::new(RwLock::new(HashMap::new()));

    let mut components = SupervisorComponents::default();
    if let Some(xray) = std::env::var_os(XRAY_BIN_ENV) {
        components.xray_executable = PathBuf::from(xray);
    }
    components.runtime_dir = data_dir.join("runtime");

    let supervisor = RuntimeSupervisor::with_components(
        channels.command_rx,
        channels.event_tx,
        Arc::clone(&snapshots),
        components,
    );
    let supervisor_thread = supervisor.spawn();

    let facade: Arc<dyn RuntimeFacade> = Arc::new(ChannelRuntimeFacade::new(
        channels.command_tx.clone(),
        Arc::clone(&snapshots),
    ));
    let runtime_service = Arc::new(RuntimeService::new(
        Arc::clone(&profile_repo),
        Arc::clone(&core_repo),
        Arc::clone(&proxy_repo),
        facade,
    ));
    let profile_service: Arc<dyn ProfileService> = Arc::new(DefaultProfileService::new(
        Arc::clone(&profile_repo),
        data_dir.clone(),
    ));

    let mut app_state = AppState::new(profile_service, runtime_service, core_repo, proxy_repo);
    let _ = app_state.load();
    if let Some((message, error)) = core_notice {
        app_state.push_notice(message, error);
    }

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        // The component library defaults to the light theme; the window paints its
        // own dark palette, so switch the components to match or their labels and
        // outlines become invisible against it.
        gpui_kit::component::theme::Theme::change(
            gpui_kit::component::theme::ThemeMode::Dark,
            None,
            cx,
        );

        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::new(px(100.0), px(100.0)),
                        size: Size {
                            width: px(1280.0),
                            height: px(860.0),
                        },
                    })),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Fingerprint Browser".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| {
                        let mut view = AppView::new(app_state, event_rx);
                        view.boot(cx);
                        view
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });

    shutdown(&channels.command_tx, supervisor_thread);
}

/// Resolve one browser core, or explain why the window cannot launch anything.
///
/// Returns the banner message and whether it is an error.
fn ensure_core(cores: &dyn CoreRepository) -> Option<(String, bool)> {
    match cores.list() {
        Ok(existing) if !existing.is_empty() => return None,
        Ok(_) => {}
        Err(error) => {
            return Some((format!("could not read browser cores: {error}"), true));
        }
    }

    let Some(executable) = core_detect::discover() else {
        return Some((
            format!(
                "no browser core found; set {} to a fingerprint-chromium executable and restart",
                core_detect::BIN_ENV
            ),
            true,
        ));
    };

    let detected = core_detect::detect(&executable);
    let notice = (detected.major == 0).then(|| {
        (
            format!(
                "{} did not report a usable version; set {} so fingerprint switches can be checked",
                detected.executable.display(),
                core_detect::MAJOR_ENV
            ),
            false,
        )
    });

    let core = BrowserCore {
        id: domain::CoreId::new(),
        name: detected.name,
        executable: detected.executable,
        version: detected.version,
        major: detected.major,
    };
    if let Err(error) = cores.save(&core) {
        return Some((format!("could not store browser core: {error}"), true));
    }

    tracing::info!(
        "registered browser core {} (major {})",
        core.name,
        core.major
    );
    notice
}

/// Ask the supervisor to reclaim every child, then give it a bounded moment.
fn shutdown(
    command_tx: &crossbeam_channel::Sender<RuntimeCommand>,
    supervisor_thread: std::thread::JoinHandle<()>,
) {
    let _ = command_tx.send(RuntimeCommand::ShutdownAll);

    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while !supervisor_thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    if supervisor_thread.is_finished() {
        let _ = supervisor_thread.join();
    } else {
        tracing::warn!("runtime supervisor did not finish shutdown within the grace period");
    }
}
