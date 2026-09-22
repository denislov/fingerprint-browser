use super::*;

/// A switch reaches the words the window *writes* afterwards, not only the
/// ones it looks up while rendering: a notice and a log line are built the
/// moment the event happens, so they have to be built in the language in
/// force then - and the file has to agree with the window, because the log
/// outlives it.
#[test]
fn switching_the_language_reaches_what_the_window_writes_next() {
    let dir = std::env::temp_dir().join(format!("fp-app-language-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    // The log file is handed in rather than opened by the fixture, so the
    // test can read the bytes the window wrote.
    let log = crate::log_file::LogFile::open(&dir.join("logs")).expect("log file");
    let mut fixture = fixture_full(&dir.join("config.json"), None, Some(log), None);

    fixture
        .state
        .set_language(Lang::Zh)
        .expect("the choice is stored");
    assert_eq!(fixture.state.language(), Lang::Zh);

    fixture
        .state
        .set_theme(ThemeChoice::Light)
        .expect("the window repaints");
    let toast = fixture
        .state
        .toasts()
        .last()
        .expect("a toast")
        .message
        .clone();
    assert!(toast.contains("主题"), "{toast}");

    fixture
        .state
        .append_log(LogLevel::Warning, None, "a warning".to_string());
    let line = std::fs::read_to_string(dir.join("logs").join("activity.log"))
        .expect("the line reached the file");
    assert!(line.contains("警告"), "{line}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_settings_rows_say_where_each_value_came_from() {
    let fixture = fixture();
    let rows = fixture.state.setting_rows();

    assert_eq!(
        rows.len(),
        SettingKey::ALL.len(),
        "every setting is listed, whatever the list grows to"
    );
    let data_dir = rows
        .iter()
        .find(|row| row.key == SettingKey::DataDir)
        .expect("the data directory row");
    assert_eq!(data_dir.source, crate::settings::Source::Default);
    assert_eq!(data_dir.source_label(en()), "from the default");
    assert!(
        !data_dir.key.editable(),
        "the config file lives in the data directory, so the directory is \
             chosen by the environment or the platform, not from inside it"
    );
    assert_eq!(data_dir.key.effect().label(en()), "next start");
    assert!(
        data_dir.note.is_some(),
        "the row says how to move it, because the row cannot do it"
    );

    let chromium = rows
        .iter()
        .find(|row| row.key == SettingKey::ChromiumBin)
        .expect("the chromium row");
    assert!(
        !chromium.key.editable(),
        "the binary is chosen by the environment and shown on the cores page"
    );

    let endpoint = rows
        .iter()
        .find(|row| row.key == SettingKey::EchoUrl)
        .expect("the proxy test endpoint row");
    assert!(
        endpoint.key.editable(),
        "the endpoint a proxy test asks is the user's to choose"
    );
    assert_eq!(
        endpoint.key.effect().label(en()),
        "now",
        "a test asks whatever the endpoint is at the time, not what it was at startup"
    );
    assert!(
        endpoint.source == crate::settings::Source::Default,
        "the fixture sets no endpoint, so the built-in one is shown"
    );
}

#[test]
fn saving_a_setting_stores_it_and_says_when_it_applies() {
    let dir = std::env::temp_dir().join(format!("fp-app-settings-{}", CoreId::new()));
    let config = dir.join("config.json");
    let mut fixture = fixture_with_config(&config);

    fixture
        .state
        .update_setting(SettingKey::XrayExecutable, "/opt/xray")
        .expect("save");

    let stored = std::fs::read_to_string(&config).expect("the config file was written");
    assert!(stored.contains("/opt/xray"), "{stored}");
    let toast = fixture
        .state
        .toasts()
        .last()
        .expect("a toast saying what happened");
    assert!(
        toast.message.contains("next start"),
        "the toast says when it applies: {}",
        toast.message
    );
    assert!(
        fixture.state.notice().is_none(),
        "saving a setting is not a problem, so the banner stays clear"
    );
    assert_eq!(
        fixture
            .state
            .setting_rows()
            .iter()
            .find(|row| row.key == SettingKey::XrayExecutable)
            .expect("the row")
            .value,
        "/opt/xray",
        "the page shows what the next start will use"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The appearance is stored and reported. It is read back out of the state
/// rather than only out of the file, because the state is what the window
/// paints from: a change that reached the file alone would leave the window
/// showing one thing while the file said another.
#[test]
fn choosing_an_appearance_stores_it_and_says_so() {
    let dir = std::env::temp_dir().join(format!("fp-app-theme-{}", CoreId::new()));
    let config = dir.join("config.json");
    let mut fixture = fixture_with_config(&config);
    assert_eq!(fixture.state.theme(), ThemeChoice::Dark, "the default");

    fixture.state.set_theme(ThemeChoice::Light).expect("save");

    assert_eq!(fixture.state.theme(), ThemeChoice::Light);
    let stored = std::fs::read_to_string(&config).expect("the config file was written");
    assert!(stored.contains("\"light\""), "{stored}");
    let toast = fixture.state.toasts().last().expect("a toast saying so");
    assert!(toast.message.contains("light theme"), "{}", toast.message);
    assert!(
        fixture.state.notice().is_none(),
        "choosing an appearance is not a problem, so the banner stays clear"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_setting_is_reported_and_changes_nothing() {
    let mut fixture = fixture();
    fixture
        .state
        .update_setting(SettingKey::DataDir, "   ")
        .expect_err("an empty value is refused");
    assert!(fixture.state.notice().is_some_and(|notice| notice.error));
    assert_eq!(
        fixture
            .state
            .setting_rows()
            .iter()
            .find(|row| row.key == SettingKey::DataDir)
            .expect("the row")
            .source,
        crate::settings::Source::Default,
        "nothing was stored"
    );
}

#[test]
fn the_page_can_be_switched() {
    let mut fixture = fixture();
    assert_eq!(fixture.state.page(), Page::Profiles);
    fixture.state.set_page(Page::Proxies);
    assert_eq!(fixture.state.page(), Page::Proxies);
    assert!(Page::Proxies.is_ready());
    assert!(
        Page::Settings.is_ready(),
        "every page in the sidebar is built now"
    );
}
