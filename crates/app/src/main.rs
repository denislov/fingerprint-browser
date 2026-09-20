//! Fingerprint Browser — application bootstrap.
//!
//! Owns process-wide wiring only: storage, the runtime supervisor thread, the
//! services over them, and the single window. No child process is ever owned by
//! the UI; commands go through [`RuntimeService`] into the supervisor channel.

mod core_detect;
mod core_editor;
mod editor;
mod log_file;
mod open_dir;
mod proxy_editor;
mod proxy_import;
#[cfg(all(test, target_os = "linux"))]
mod real_browser;
mod reclaim;
mod settings;
#[cfg(unix)]
mod signal;
mod state;
mod ui;
mod verifier;

use application::{
    CoreService, DefaultCoreService, DefaultProfileService, DefaultProxyService, ProfileService,
    ProxyService, RuntimeService,
};
use gpui_kit::component::Root;
use gpui_kit::*;
use runtime::{
    ChannelRuntimeFacade, RuntimeCommand, RuntimeFacade, RuntimeSupervisor,
    RuntimeSupervisorChannels, SupervisorComponents,
};
use state::AppState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use storage::{CoreRepository, ProfileRepository, ProxyRepository, SqliteStorage};
use ui::AppView;
use verifier::CdpFingerprintVerifier;

const EVENT_CAPACITY: usize = 256;
/// How long shutdown waits for the supervisor to reclaim children.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

fn main() {
    tracing_subscriber::fmt::init();

    // Configuration is resolved before anything opens: the data directory and
    // the Xray path both decide what this process does.
    let (settings, settings_notice) =
        settings::Settings::load(settings::Environment::from_process());
    let data_dir = settings.data_dir().to_path_buf();

    let storage = SqliteStorage::open(data_dir.join("app.db")).expect("open sqlite storage");
    let profile_repo: Arc<dyn ProfileRepository> = Arc::new(storage.profiles());
    let core_repo: Arc<dyn CoreRepository> = Arc::new(storage.cores());
    let proxy_repo: Arc<dyn ProxyRepository> = Arc::new(storage.proxies());

    let core_notice = core_detect::maintain(core_repo.as_ref());

    let channels = RuntimeSupervisorChannels::new(EVENT_CAPACITY);
    let event_rx = channels.event_rx.clone();
    let snapshots = Arc::new(RwLock::new(HashMap::new()));

    let components = SupervisorComponents {
        xray_executable: settings.xray_executable().to_path_buf(),
        runtime_dir: settings.runtime_dir(),
        ..SupervisorComponents::default()
    };

    let supervisor = RuntimeSupervisor::with_components(
        channels.command_rx,
        channels.event_tx,
        Arc::clone(&snapshots),
        components,
    );
    // Before the window opens and before any command can queue: a previous run
    // that was killed left browsers behind, and one of them may still hold the
    // profile and the debugging port this run is about to want.
    let reclaim = supervisor.reclaim_orphans();
    // The supervisor thread is the only owner of the child handles, so the
    // shutdown that a signal asks for has to be able to take it from here.
    let supervisor_thread = Arc::new(Mutex::new(Some(supervisor.spawn())));

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
    let proxy_service: Arc<dyn ProxyService> = Arc::new(DefaultProxyService::new(
        Arc::clone(&proxy_repo),
        Arc::clone(&profile_repo),
    ));
    let core_service: Arc<dyn CoreService> = Arc::new(DefaultCoreService::new(
        Arc::clone(&core_repo),
        Arc::clone(&profile_repo),
    ));

    let mut app_state = AppState::new(
        profile_service,
        runtime_service,
        core_service,
        proxy_service,
        settings,
    );
    let _ = app_state.load();
    if let Some((message, error)) = core_notice {
        app_state.push_notice(message, error);
    }
    if let Some((message, error)) = settings_notice {
        app_state.push_notice(message, error);
    }
    // Last, so that a problem found here is the one the banner shows: a settings
    // or core problem is still on its own page afterwards, while a browser that
    // was stopped at startup is reported nowhere else.
    if let Some((message, error)) = reclaim::reclaim_notice(&reclaim, |profile_id| {
        profile_repo
            .get(profile_id)
            .ok()
            .flatten()
            .map(|profile| profile.name)
    }) {
        // The banner is one line; the log is what a headless run has.
        if error {
            tracing::warn!("{message}");
        } else {
            tracing::info!("{message}");
        }
        app_state.push_notice(message, error);
    }

    #[cfg(unix)]
    let _signals = signal::on_shutdown_request({
        let command_tx = channels.command_tx.clone();
        let supervisor_thread = Arc::clone(&supervisor_thread);
        move |signal| {
            tracing::info!("received signal {signal}; reclaiming child processes before exiting");
            let thread = supervisor_thread
                .lock()
                .ok()
                .and_then(|mut slot| slot.take());
            shutdown(&command_tx, thread);
            std::process::exit(0);
        }
    });
    #[cfg(unix)]
    if let Err(error) = &_signals {
        // Not fatal: the window still closes through its own exit paths.
        tracing::warn!("signal handlers could not be installed: {error}");
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
                        let mut view = AppView::new(
                            app_state,
                            event_rx,
                            Arc::new(CdpFingerprintVerifier::default()),
                            Arc::new(open_dir::SystemDirectoryOpener),
                        );
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

    shutdown(
        &channels.command_tx,
        supervisor_thread
            .lock()
            .ok()
            .and_then(|mut slot| slot.take()),
    );
}

/// Ask the supervisor to reclaim every child, then give it a bounded moment.
/// The handle arrives in an `Option` because a signal may have taken it first.
fn shutdown(
    command_tx: &crossbeam_channel::Sender<RuntimeCommand>,
    supervisor_thread: Option<std::thread::JoinHandle<()>>,
) {
    let _ = command_tx.send(RuntimeCommand::ShutdownAll);

    let Some(supervisor_thread) = supervisor_thread else {
        // Another exit path is already reclaiming; do not wait twice.
        return;
    };

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
