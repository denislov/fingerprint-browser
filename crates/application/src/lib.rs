pub mod browser_data;
pub mod config_backup;
pub mod core_service;
pub mod error;
pub mod export;
pub mod import;
pub mod operations;
pub mod profile_service;
pub mod proxy_service;
pub mod restore;
pub mod runtime_service;

pub use browser_data::{
    BrowserDataError, BrowserDataReport, Direction, copy_browser_data, recover_user_data_dirs,
    restore_browser_data,
};
pub use config_backup::{BackupError, ConfigBackup, ConfigSnapshot, Credentials, ExportOrigin};
pub use core_service::{CoreService, DefaultCoreService, VersionProbe};
pub use error::AppError;
pub use export::{ExportError, ExportReport, read_configuration, write_config_backup};
pub use import::{
    Counts, ImportError, ImportNotes, ImportPlan, ImportReport, Repointed, apply_import,
    plan_import, read_config_backup,
};
pub use operations::{Busy, Held, Operation, Operations};
pub use profile_service::{
    DefaultProfileService, DeleteMode, NewProfile, ProfileService, default_user_data_dir,
};
pub use proxy_service::{DefaultProxyService, NewProxy, ProxyService};
pub use restore::{
    RestoreError, RestoreMode, RestorePlan, RestoreReport, apply_restore, plan_restore,
};
pub use runtime_service::RuntimeService;

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{CoreId, RuntimeState};
    use runtime::{
        ChannelRuntimeFacade, RuntimeCommand, RuntimeEvent, RuntimeSupervisor,
        RuntimeSupervisorChannels,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;
    use storage::{CoreRepository, MemProfileRepository};

    #[test]
    fn test_profile_service_create_and_duplicate() {
        let repo = Arc::new(MemProfileRepository::new());
        let base_dir = PathBuf::from("data");
        let service = DefaultProfileService::new(repo, base_dir);

        let core_id = CoreId::new();
        let draft = NewProfile {
            name: "Primary Profile".to_string(),
            core_id,
            user_data_dir: None,
            fingerprint: None,
            proxy_id: None,
            window: None,
            start_target: None,
        };

        let profile = service.create(draft).expect("create profile");
        assert_eq!(profile.name, "Primary Profile");
        assert_eq!(profile.core_id, core_id);

        // Duplicate
        let dup = service
            .duplicate(profile.id, "Primary Profile Copy".to_string())
            .expect("duplicate profile");
        assert_ne!(dup.id, profile.id);
        assert_eq!(dup.name, "Primary Profile Copy");
        assert_ne!(dup.user_data_dir, profile.user_data_dir);
        assert_ne!(dup.fingerprint.seed, profile.fingerprint.seed);

        // List
        let all = service.list().expect("list profiles");
        assert_eq!(all.len(), 2);

        // Delete
        service
            .delete(profile.id, DeleteMode::KeepUserData)
            .expect("delete");
        let remaining = service.list().expect("list profiles");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, dup.id);
    }

    #[test]
    fn a_created_profile_lives_where_an_imported_one_would() {
        // Two places compute this path - `create` for a new profile, and the
        // import for one whose recorded directory is not on this machine - and
        // they have to agree, or one identifier would name two directories and
        // the browser data would be split between them.
        let repo = Arc::new(MemProfileRepository::new());
        let base = PathBuf::from("data");
        let service = DefaultProfileService::new(repo, base.clone());

        let profile = service
            .create(NewProfile {
                name: "Primary Profile".to_string(),
                core_id: CoreId::new(),
                user_data_dir: None,
                fingerprint: None,
                proxy_id: None,
                window: None,
                start_target: None,
            })
            .expect("create profile");

        assert_eq!(
            profile.user_data_dir,
            default_user_data_dir(&base, profile.id)
        );
    }

    #[test]
    fn test_profile_service_with_sqlite_storage() {
        let storage = storage::SqliteStorage::in_memory().expect("sqlite in memory");
        let repo = Arc::new(storage.profiles());
        let base_dir = PathBuf::from("data");
        let service = DefaultProfileService::new(repo, base_dir);

        let core_id = CoreId::new();
        let core = domain::BrowserCore {
            id: core_id,
            name: "Core 128".to_string(),
            executable: PathBuf::from("chrome.exe"),
            version: "128.0".to_string(),
            major: 128,
        };
        storage.cores().save(&core).expect("save core");

        let draft = NewProfile {
            name: "SQLite Profile".to_string(),
            core_id,
            user_data_dir: None,
            fingerprint: None,
            proxy_id: None,
            window: None,
            start_target: None,
        };

        let profile = service.create(draft).expect("create profile in sqlite");
        assert_eq!(profile.name, "SQLite Profile");

        let dup = service
            .duplicate(profile.id, "SQLite Profile Copy".to_string())
            .expect("duplicate in sqlite");
        assert_eq!(dup.name, "SQLite Profile Copy");

        let all = service.list().expect("list from sqlite");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_runtime_service_orchestration() {
        let storage = storage::SqliteStorage::in_memory().expect("sqlite in memory");
        let profile_repo = Arc::new(storage.profiles());
        let core_repo = Arc::new(storage.cores());
        let proxy_repo = Arc::new(storage.proxies());

        let channels = RuntimeSupervisorChannels::new(16);
        let snapshots = Arc::new(RwLock::new(HashMap::new()));
        let supervisor = RuntimeSupervisor::new(
            channels.command_rx,
            channels.event_tx,
            Arc::clone(&snapshots),
        );
        let facade = Arc::new(ChannelRuntimeFacade::new(
            channels.command_tx.clone(),
            Arc::clone(&snapshots),
        ));
        let handle = supervisor.spawn();

        let runtime_service = RuntimeService::new(
            Arc::clone(&profile_repo) as Arc<dyn storage::ProfileRepository>,
            Arc::clone(&core_repo) as Arc<dyn storage::CoreRepository>,
            Arc::clone(&proxy_repo) as Arc<dyn storage::ProxyRepository>,
            facade,
        );

        let core_id = CoreId::new();
        let core = domain::BrowserCore {
            id: core_id,
            name: "Dummy Core".to_string(),
            executable: PathBuf::from("dummy_browser_missing.exe"),
            version: "128.0".to_string(),
            major: 128,
        };
        core_repo.save(&core).expect("save core");

        let profile_service = DefaultProfileService::new(profile_repo, PathBuf::from("data"));
        let profile = profile_service
            .create(NewProfile {
                name: "Orchestrated Profile".to_string(),
                core_id,
                user_data_dir: None,
                fingerprint: None,
                proxy_id: None,
                window: None,
                start_target: None,
            })
            .expect("create profile");

        // Start through runtime service
        runtime_service
            .start(profile.id)
            .expect("start via runtime service");

        // Verify supervisor received and processed start
        let event = channels
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("received event");
        assert!(matches!(
            event,
            RuntimeEvent::StateChanged {
                state: RuntimeState::Starting,
                ..
            }
        ));

        // Stop through runtime service
        runtime_service
            .stop(profile.id)
            .expect("stop via runtime service");

        // Shutdown
        channels
            .command_tx
            .send(RuntimeCommand::ShutdownAll)
            .expect("shutdown");
        handle.join().expect("join supervisor thread");
    }
}
