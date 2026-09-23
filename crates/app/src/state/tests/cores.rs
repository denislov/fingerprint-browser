use super::*;

#[test]
fn an_added_core_carries_the_version_its_binary_reported() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("add", Some("Chromium 148.0.7778.215"));

    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].core.id, id);
    assert_eq!(rows[0].core.version, "Chromium 148.0.7778.215");
    assert_eq!(rows[0].core.major, 148);
    assert!(rows[0].present, "the binary is on disk");
    assert_eq!(rows[0].usage_label(en()), "not used");
    assert_eq!(
        rows[0].capability_parts(),
        Some(("Chrome 144+".to_string(), true)),
        "the generation and the exclusion switch, as two values"
    );
}

/// The pivot made visible: a core below it is labelled as not offering the
/// switches this project only verified from 144.
#[test]
fn a_legacy_core_says_the_noise_switches_are_not_offered() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("legacy", Some("Chromium 128.0.0.0"));
    fixture
        .state
        .add_core(Some("Old".to_string()), binary.path_buf())
        .expect("add");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows[0].core.name, "Old");
    assert_eq!(rows[0].core.major, 128);
    assert_eq!(
        rows[0].capability_parts(),
        Some(("Chrome 143 and older".to_string(), false))
    );
}

#[test]
fn a_binary_without_a_usable_version_is_refused_and_reported() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("silent", None);

    let error = fixture.state.add_core(None, binary.path_buf()).unwrap_err();

    assert!(error.to_string().contains("--version"), "{error}");
    assert!(fixture.state.core_rows().expect("rows").is_empty());
    assert!(fixture.state.notice().is_some_and(|notice| notice.error));
}

#[test]
fn re_detecting_a_replaced_binary_updates_the_major_and_the_label() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("redetect", Some("Chromium 148.0.7778.215"));
    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");

    // The same path now answers with another build.
    binary.replace(Some("Chromium 128.0.0.0"));
    fixture.state.redetect_core(id).expect("redetect");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows[0].core.major, 128);
    assert_eq!(
        rows[0].capability_parts(),
        Some(("Chrome 143 and older".to_string(), false))
    );
    assert!(
        fixture
            .state
            .toasts()
            .iter()
            .any(|toast| toast.message.contains("128")),
        "the toast says what it found"
    );
}

#[test]
fn renaming_a_core_keeps_its_version() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("rename", Some("Chromium 148.0.7778.215"));
    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");

    let mut core = fixture.state.core(id).expect("stored");
    core.name = "Work browser".to_string();
    fixture.state.update_core(core).expect("update");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows[0].core.name, "Work browser");
    assert_eq!(rows[0].core.major, 148);
}

#[test]
fn a_core_a_profile_launches_with_cannot_be_deleted() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let core_id = fixture.state.core_rows().expect("rows")[0].core.id;
    fixture.state.create_profile("Uses it").expect("create");

    let error = fixture.state.delete_core(core_id).unwrap_err();
    assert!(error.to_string().contains("Uses it"), "{error}");
    assert_eq!(fixture.state.core_rows().expect("rows").len(), 1);
}

#[test]
fn an_unused_core_is_deleted() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("unused", Some("Chromium 148.0.7778.215"));
    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");
    fixture.state.delete_core(id).expect("delete");
    assert!(fixture.state.core_rows().expect("rows").is_empty());
    assert!(!fixture.state.has_core());
}

#[test]
fn create_without_a_core_records_an_error_notice() {
    let mut fixture = fixture();
    fixture.state.load().expect("initial load");

    let error = fixture
        .state
        .create_profile("Primary")
        .expect_err("must fail");

    assert!(matches!(error, AppError::Conflict(_)));
    assert!(fixture.state.rows().is_empty());
    let notice = fixture.state.notice().expect("notice");
    assert!(notice.error);
    assert!(notice.message.contains("no browser core"));
}
