pub mod error;
pub mod memory;
pub mod traits;

pub use error::StorageError;
pub use memory::{MemCoreRepository, MemProfileRepository, MemProxyRepository};
pub use traits::{CoreRepository, ProfileRepository, ProxyRepository};

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        BrowserBrand, BrowserProfile, CoreId, FingerprintProfile, Platform, ProfileId, StartTarget,
        WebRtcPolicy, WindowProfile,
    };
    use std::path::PathBuf;

    #[test]
    fn test_mem_profile_repository_crud() {
        let repo = MemProfileRepository::new();
        let id = ProfileId::new();
        let profile = BrowserProfile {
            id,
            name: "Test Profile".to_string(),
            core_id: CoreId::new(),
            user_data_dir: PathBuf::from("data/test"),
            fingerprint: FingerprintProfile {
                seed: 100,
                brand: BrowserBrand::Chrome,
                brand_version: None,
                platform: Platform::Windows,
                platform_version: None,
                language: "en-US".to_string(),
                accept_language: "en-US,en;q=0.9".to_string(),
                timezone: "UTC".to_string(),
                hardware_concurrency: Some(4),
                webrtc_policy: WebRtcPolicy::DisableNonProxiedUdp,
                disabled_spoofing: vec![],
            },
            proxy_id: None,
            window: WindowProfile::new(1280, 800),
            start_target: StartTarget::Blank,
        };

        // Insert
        repo.insert(&profile).expect("insert should succeed");

        // Duplicate insert should fail
        assert!(repo.insert(&profile).is_err());

        // Get
        let fetched = repo.get(id).expect("get should succeed");
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "Test Profile");

        // List
        let list = repo.list().expect("list should succeed");
        assert_eq!(list.len(), 1);

        // Update
        let mut updated_profile = profile.clone();
        updated_profile.name = "Updated Profile".to_string();
        repo.update(&updated_profile)
            .expect("update should succeed");
        let fetched_updated = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_updated.name, "Updated Profile");

        // Delete
        repo.delete(id).expect("delete should succeed");
        assert!(repo.get(id).unwrap().is_none());
        assert_eq!(repo.list().unwrap().len(), 0);
    }
}
