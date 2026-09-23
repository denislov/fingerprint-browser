//! Fingerprint Browser — application bootstrap.
//!
//! Owns process-wide wiring only: storage, the runtime supervisor thread, the
//! services over them, and the single window. No child process is ever owned by
//! the UI; commands go through [`RuntimeService`] into the supervisor channel.

mod browser_data;
mod cli;
mod core_detect;
mod core_editor;
mod diagnostics;
mod editor;
mod exit;
mod instance;
mod log_file;
mod maintenance;
mod open_dir;
mod paths;
mod proxy_editor;
mod proxy_import;
mod proxy_tester;
#[cfg(all(test, target_os = "linux"))]
mod real_browser;
mod reclaim;
mod settings;
#[cfg(unix)]
mod signal;
mod state;
mod task;
mod text;
mod theme;
mod tray;
mod ui;
mod verifier;
mod version;
mod window_visibility;

use application::{
    CoreService, DefaultCoreService, DefaultProfileService, DefaultProxyService, ProfileService,
    ProxyService, RuntimeService,
};
use gpui_kit::component::Root;
use gpui_kit::*;
use proxy_tester::{ProxyTester, XrayProxyTester};
use runtime::{
    ChannelRuntimeFacade, RuntimeCommand, RuntimeFacade, RuntimeSupervisor,
    RuntimeSupervisorChannels, SupervisorComponents,
};
use state::{AppState, Services};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};
use storage::{
    ConfigurationRepository, CoreRepository, ProfileRepository, ProxyRepository, SqliteStorage,
};
use ui::AppView;
use verifier::CdpFingerprintVerifier;

const EVENT_CAPACITY: usize = 256;
/// How long shutdown waits for the supervisor to reclaim children.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let invocation = cli::parse(&arguments);

    // The one question that has to be answerable by a script, on a machine whose
    // window will not open, and without changing anything about the answer:
    // what is installed here. Answered before a single file is read.
    if invocation == Ok(cli::Command::Version) {
        println!("{}", version::line());
        return;
    }

    tracing_subscriber::fmt::init();

    // `--help` and a refused argument both end here, and both are written in the
    // language the config file names - read the cheap way, so answering "how do
    // I use this" cannot move a file (`settings::language_hint`).
    match &invocation {
        Ok(cli::Command::Help) => {
            let t = text::text(settings::language_hint(
                &settings::Environment::from_process(),
            ));
            println!("{}", t.cli_usage);
            return;
        }
        Err(refusal) => {
            let t = text::text(settings::language_hint(
                &settings::Environment::from_process(),
            ));
            eprintln!("{}", cli::refusal_message(refusal, t));
            eprintln!("{}", t.cli_usage_hint);
            std::process::exit(2);
        }
        _ => {}
    }

    // Configuration is resolved before anything opens: the data directory and
    // the Xray path both decide what this process does.
    let (settings, settings_notice) =
        settings::Settings::load(settings::Environment::from_process());
    let data_dir = settings.data_dir().to_path_buf();
    // The table is 'static and the language is already resolved, so the notices
    // below are built in the language the config file asked for - including the
    // ones produced before the window exists.
    let t = settings.text();

    // A report is about the installation rather than about the window, so it is
    // written and the process ends - here, before storage opens, because asking
    // for a report must not be the thing that creates a database.
    if let Ok(cli::Command::Diagnostics { destination }) = &invocation {
        // What a real start would have complained about is worth saying out
        // loud: it is usually the reason a report was asked for.
        if let Some((message, _)) = &settings_notice {
            eprintln!("{message}");
        }
        let destination = destination
            .clone()
            .unwrap_or_else(|| paths::default_diagnostics_file(&data_dir, SystemTime::now()));
        match diagnostics::write_for(&settings, &destination, t) {
            Ok(()) => println!("{}", t.diag_written(&destination.display().to_string())),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }

    // One window per data directory. Taken here - after the questions that are
    // answered without touching the installation, and before the database opens -
    // because the two things below are what a second copy must not do: open the
    // same database, and read the first copy's session records as orphans and
    // stop the browsers it is running. The binding lives until `main` returns, so
    // the lock is held for as long as the window is; the kernel releases it on
    // every path out, including the ones that never return here.
    let _instance = match instance::InstanceLock::acquire(&data_dir) {
        Ok(lock) => lock,
        Err(instance::Busy::Held(holder)) => {
            eprintln!(
                "{}",
                t.instance_busy(
                    holder
                        .as_ref()
                        .map(|holder| (holder.pid, holder.build.as_str()))
                )
            );
            std::process::exit(3);
        }
        Err(instance::Busy::Unavailable(error)) => {
            eprintln!("{}", t.instance_unavailable(&error));
            std::process::exit(1);
        }
    };

    // What this run is and where it keeps its files, in the one log that outlives
    // the window and in the one a headless run has.
    let started = t.run_started(
        &version::line(),
        &version::platform(),
        &data_dir.display().to_string(),
    );
    tracing::info!("{started}");

    // Checked before storage opens, because opening it is what creates the
    // database in the new place and ends the question this asks.
    let moved_notice = paths::moved_data_dir_notice(
        &data_dir,
        std::path::Path::new(paths::FALLBACK_DATA_DIR),
        settings.text(),
    );

    let storage = SqliteStorage::open(data_dir.join("app.db")).expect("open sqlite storage");
    let profile_repo: Arc<dyn ProfileRepository> = Arc::new(storage.profiles());
    let core_repo: Arc<dyn CoreRepository> = Arc::new(storage.cores());
    let proxy_repo: Arc<dyn ProxyRepository> = Arc::new(storage.proxies());
    // The same connection the three repositories write through, addressed as one
    // configuration: a restore replaces all of it or none of it.
    let configuration: Arc<dyn ConfigurationRepository> = Arc::new(storage.clone());

    let core_notice = core_detect::maintain(core_repo.as_ref(), t);

    let channels = RuntimeSupervisorChannels::new(EVENT_CAPACITY);
    let event_rx = channels.event_rx.clone();
    let snapshots = Arc::new(RwLock::new(HashMap::new()));

    let components = SupervisorComponents {
        xray_executable: settings.xray_executable().to_path_buf(),
        runtime_dir: settings.runtime_dir(),
        ..SupervisorComponents::default()
    };

    let mut supervisor = RuntimeSupervisor::with_components(
        channels.command_rx,
        channels.event_tx,
        Arc::clone(&snapshots),
        components,
    );
    // Before the window opens and before any command can queue: a previous run
    // that was killed left browsers behind, and one of them may still hold the
    // profile and the debugging port this run is about to want.
    // Deliberately `mut`: recovering is not only a cleanup any more. A previous
    // run that was told to leave its browsers running left records marked to say
    // so, and those sessions are adopted - they become running profiles again
    // rather than being stopped by the start that follows them.
    let reclaim = supervisor.recover_orphans();
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

    // A proxy test starts its own engine rather than going through the
    // supervisor: it has to be able to run before anything is launched, and it
    // must never stop an engine a profile is using. Its temporary config lands
    // in the same runtime directory, so a killed run leaves nothing new behind.
    let proxy_tester: Arc<dyn ProxyTester> = Arc::new(XrayProxyTester::new(
        settings.xray_executable().to_path_buf(),
        settings.runtime_dir(),
    ));

    let mut app_state = AppState::new(
        Services {
            profiles: profile_service,
            runtime: runtime_service,
            cores: core_service,
            proxies: proxy_service,
            configuration,
        },
        settings,
    );
    let _ = app_state.load();
    // The activity log's first line, before anything a start has to report: what
    // build this is, on what platform, and where it keeps its files is what
    // every problem report is asked for first, and the log is what outlives the
    // window.
    app_state.note_startup(started);
    // A restore killed between the two renames of its swap left the profile with
    // no data directory and the only copy of its data under a `.old` name beside
    // it. This is the half of that recovery the program can find by itself - a
    // backup directory is chosen for one operation and not recorded - and it runs
    // before the window so the sentence is the first thing the log and the user
    // read. It goes in before the notices below, because a real problem is worth
    // more than a repair that worked.
    let put_back = application::recover_user_data_dirs(&profile_repo.list().unwrap_or_default());
    if let Some((message, error)) = browser_data::recovery_notice(&put_back, t) {
        tracing::info!("{message}");
        app_state.push_notice(message, error);
    }
    if let Some((message, error)) = core_notice {
        app_state.push_notice(message, error);
    }
    if let Some((message, error)) = settings_notice {
        app_state.push_notice(message, error);
    }
    if let Some((message, error)) = moved_notice {
        // The banner is for problems, and this is not one; the log keeps it for
        // a run that has no window.
        tracing::info!("{message}");
        app_state.push_notice(message, error);
    }
    // Last, so that a problem found here is the one the banner shows: a settings
    // or core problem is still on its own page afterwards, while a browser that
    // was stopped at startup is reported nowhere else.
    if let Some((message, error)) = reclaim::reclaim_notice(
        &reclaim,
        |profile_id| {
            profile_repo
                .get(profile_id)
                .ok()
                .flatten()
                .map(|profile| profile.name)
        },
        t,
    ) {
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

    // The window's icons and brand mark, registered before the first frame: an
    // asset source is read while painting, and a frame drawn before it is set
    // would draw every icon as nothing.
    gpui_kit::application()
        .with_assets(ui::icons::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            // The appearance the user last chose, applied before the first frame so
            // the window is never painted in the other palette and then corrected.
            // The component library's own default is light; this one call moves both
            // layers - the mode and the accent family - which is why it is
            // `theme::apply` rather than the library's `Theme::change`.
            let appearance = app_state.theme();
            theme::apply(appearance, cx);

            let window_bounds = WindowBounds::centered(
                Size {
                    width: px(1200.0),
                    height: px(680.0),
                },
                cx,
            );

            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(window_bounds),
                        window_min_size: Some(Size {
                            width: px(960.0),
                            height: px(560.0),
                        }),
                        titlebar: Some(TitlebarOptions {
                            // The version is in the title so a screenshot answers the
                            // question a report would otherwise have to ask.
                            title: Some(version::window_title().into()),
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
                                proxy_tester,
                                Arc::new(open_dir::SystemDirectoryOpener),
                                Arc::new(browser_data::DiskBrowserDataCopier),
                                // The desktop's own tray, started the first time
                                // "keep running" puts the window away.
                                tray::Tray::start,
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
