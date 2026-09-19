pub mod error;
pub mod profile_service;

pub use error::AppError;
pub use profile_service::{DefaultProfileService, DeleteMode, NewProfile, ProfileService};

#[cfg(test)]
mod tests {
    use super::*;
    use domain::CoreId;
    use std::path::PathBuf;
    use std::sync::Arc;
    use storage::MemProfileRepository;

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
}
