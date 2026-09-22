use crate::text::en;

use super::*;
use crate::paths::FALLBACK_DATA_DIR;

/// A config file in its own directory, removed when the test ends.
struct TempConfig {
    dir: PathBuf,
    path: PathBuf,
}

impl TempConfig {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fp-settings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.json");
        Self { dir, path }
    }

    fn write(&self, text: &str) {
        std::fs::write(&self.path, text).expect("write config");
    }

    fn read(&self) -> String {
        std::fs::read_to_string(&self.path).expect("read config")
    }
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env(config: &TempConfig) -> Environment {
    Environment {
        config: Some(config.path.to_string_lossy().to_string()),
        ..Environment::default()
    }
}

/// The default is the platform's own place for application data; the
/// relative `data` is only what is left for a host with no home directory.
#[test]
fn the_default_data_directory_follows_the_platform() {
    let config = TempConfig::new("platform");
    let with_home = |os: &'static str, home: &str| paths::Host {
        os,
        home: Some(PathBuf::from(home)),
        ..paths::Host::default()
    };
    let windows_base = PathBuf::from(r"C:\Users\me\AppData\Local");
    let cases = [
        (
            paths::Host {
                local_app_data: Some(windows_base.clone()),
                app_data: Some(PathBuf::from(r"C:\Users\me\AppData\Roaming")),
                ..with_home("windows", r"C:\Users\me")
            },
            // Built with the same join rather than written out: `Path::join`
            // uses the separator of the host the test runs on.
            windows_base.join(paths::APP_DIR),
        ),
        (
            with_home("linux", "/home/me"),
            PathBuf::from("/home/me/.local/share/FpBrowser"),
        ),
        (
            with_home("macos", "/Users/me"),
            PathBuf::from("/Users/me/Library/Application Support/FpBrowser"),
        ),
        // No home directory to put it under: the fallback, unchanged from
        // what every host used to get.
        (
            paths::Host {
                os: "linux",
                ..paths::Host::default()
            },
            PathBuf::from(FALLBACK_DATA_DIR),
        ),
    ];

    for (host, expected) in cases {
        let (settings, _) = Settings::load(Environment {
            config: Some(config.path.to_string_lossy().to_string()),
            host,
            ..Environment::default()
        });
        assert_eq!(settings.data_dir(), expected);
    }
}

#[test]
fn with_nothing_set_every_value_is_the_default() {
    let config = TempConfig::new("defaults");
    let (settings, notice) = Settings::load(env(&config));

    assert!(notice.is_none(), "a missing config file is not an error");
    assert_eq!(settings.data_dir(), Path::new(FALLBACK_DATA_DIR));
    assert_eq!(settings.xray_executable(), default_xray());
    let rows = settings.rows(en());
    assert_eq!(rows.len(), SettingKey::ALL.len());
    for key in [
        SettingKey::DataDir,
        SettingKey::XrayExecutable,
        SettingKey::EchoUrl,
        SettingKey::ChromiumBin,
        SettingKey::ChromiumMajor,
    ] {
        let row = rows.iter().find(|row| row.key == key).expect("the row");
        assert_ne!(
            row.source,
            Source::Environment,
            "{key:?} came from the environment"
        );
    }
    assert_eq!(
        rows.iter()
            .find(|row| row.key == SettingKey::ConfigFile)
            .expect("the row")
            .source,
        Source::Environment,
        "the test points the config path at a temp file through the environment"
    );
    assert_eq!(rows[0].source_label(en()), "from the default");
}

#[test]
fn a_stored_value_is_used_and_says_where_it_came_from() {
    let config = TempConfig::new("stored");
    config.write(r#"{"xray_executable": "/opt/xray"}"#);
    let (settings, notice) = Settings::load(env(&config));

    assert!(notice.is_none());
    assert_eq!(settings.xray_executable(), Path::new("/opt/xray"));
    let row = settings.rows(en())[1].clone();
    assert_eq!(row.key, SettingKey::XrayExecutable);
    assert_eq!(row.source, Source::ConfigFile);
    assert_eq!(row.source_label(en()), "from the config file");
    assert_eq!(row.value, "/opt/xray");
}

/// The config file lives in the data directory, so everything a run needs is
/// under one path - and a file left in the old location is folded in once,
/// then removed, rather than being read for ever from two places.
#[test]
fn a_config_file_in_the_old_location_is_moved_into_the_data_directory() {
    let home = std::env::temp_dir().join(format!("fp-settings-home-move-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let host = paths::Host {
        home: Some(home.clone()),
        ..paths::Host::default()
    };
    // Taken from the platform rule rather than spelled out, so a test cannot
    // pass while the directory the program uses has moved somewhere else.
    let data = paths::data_dir(&host).expect("a data directory");
    let legacy = home.join(".config/fp-browser/config.json");
    std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy dir");
    std::fs::write(
        &legacy,
        r#"{"xray_executable": "/opt/xray", "theme": "light", "lang": "zh"}"#,
    )
    .expect("legacy config");

    let environment = Environment {
        host: host.clone(),
        ..Environment::default()
    };
    let (settings, notice) = Settings::load(environment);

    assert!(notice.is_none(), "a plain move needs no explanation");
    assert_eq!(settings.data_dir(), data, "the platform's own directory");
    assert_eq!(settings.xray_executable(), Path::new("/opt/xray"));
    assert_eq!(settings.theme(), ThemeChoice::Light);
    assert_eq!(settings.language(), Lang::Zh);
    let moved = data.join(paths::CONFIG_FILE);
    assert!(moved.exists(), "the file is where the next start looks");
    assert!(!legacy.exists(), "and it is not in two places");
    let written = std::fs::read_to_string(&moved).expect("read moved file");
    assert!(!written.contains("data_dir"), "{written}");
    let _ = std::fs::remove_dir_all(&home);
}

/// A move that cannot be written says so, and keeps reading the old file:
/// the values are in force either way, and the alternative is a settings
/// page that looks like it saved.
#[test]
fn a_move_that_cannot_be_written_keeps_reading_the_old_file() {
    let home =
        std::env::temp_dir().join(format!("fp-settings-home-blocked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("temp dir");
    let legacy = home.join(".config/fp-browser/config.json");
    std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy dir");
    std::fs::write(&legacy, r#"{"theme": "light"}"#).expect("legacy config");
    // A file where the data directory would have to go: creating it fails,
    // which is what a full disk or a read-only home looks like from here.
    let blocked = home.join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("a file in the way");

    let environment = Environment {
        data_dir: Some(blocked.join("data").to_string_lossy().to_string()),
        host: paths::Host {
            home: Some(home.clone()),
            ..paths::Host::default()
        },
        ..Environment::default()
    };
    let (settings, notice) = Settings::load(environment);

    let (message, is_error) = notice.expect("a notice");
    assert!(is_error, "{message}");
    assert!(message.contains("could not be moved"), "{message}");
    assert!(
        legacy.exists(),
        "the settings are still only here, so the file has to stay"
    );
    assert_eq!(
        settings.theme(),
        ThemeChoice::Light,
        "and they are in force"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// The one case the move cannot decide: an old file that named a data
/// directory of its own. It is left alone - it is the only record of where
/// that directory is - and the banner says how to get back to it.
#[test]
fn a_config_file_naming_another_data_directory_is_left_where_it_is() {
    let home = std::env::temp_dir().join(format!("fp-settings-home-kept-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let host = paths::Host {
        home: Some(home.clone()),
        ..paths::Host::default()
    };
    let data = paths::data_dir(&host).expect("a data directory");
    let elsewhere = home.join("elsewhere");
    let legacy = home.join(".config/fp-browser/config.json");
    std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy dir");
    // The file an older version would have written, built by the JSON writer
    // rather than a `format!`: a Windows path is full of backslashes and half
    // of them are not valid escapes, so the hand-written version produced a
    // config file this program could not read - on the one platform this test
    // is never run on locally.
    let body = serde_json::json!({
        "data_dir": elsewhere.display().to_string(),
        "theme": "light",
    });
    std::fs::write(&legacy, serde_json::to_vec(&body).expect("json")).expect("legacy config");

    let environment = Environment {
        host,
        ..Environment::default()
    };
    let (settings, notice) = Settings::load(environment);

    let (message, is_error) = notice.expect("a notice");
    assert!(is_error, "the profile list is about to look empty");
    assert!(
        message.contains(&elsewhere.display().to_string()),
        "{message}"
    );
    assert!(message.contains(DATA_DIR_ENV), "{message}");
    assert!(
        legacy.exists(),
        "the file that knows where that directory is was not moved"
    );
    assert_eq!(
        settings.data_dir(),
        data,
        "this run uses the directory it resolved, not the one in the file"
    );
    assert_eq!(
        settings.rows(en())[0].shadowed_label(en()).as_deref(),
        Some(
            format!(
                "the config file holds {}, which this overrides",
                elsewhere.display()
            )
            .as_str()
        ),
        "the row says what was ignored"
    );
    assert_eq!(
        settings.theme(),
        ThemeChoice::Light,
        "the rest still applies"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// A stored setting the environment overrides must be visible, not silently
/// ignored: this is the "I set it and nothing happened" case.
#[test]
fn an_environment_override_shows_the_value_it_shadows() {
    let config = TempConfig::new("shadowed");
    config.write(r#"{"data_dir": "/srv/fp"}"#);
    let mut environment = env(&config);
    environment.data_dir = Some("/tmp/other".to_string());
    let (settings, _) = Settings::load(environment);

    assert_eq!(settings.data_dir(), Path::new("/tmp/other"));
    let row = &settings.rows(en())[0];
    assert_eq!(row.source, Source::Environment);
    assert_eq!(row.source_label(en()), "set by FP_BROWSER_DATA_DIR");
    assert_eq!(row.value, "/tmp/other");
    assert_eq!(
        row.shadowed_label(en()).as_deref(),
        Some("the config file holds /srv/fp, which this overrides")
    );
}

/// The endpoint the proxy test asks is a standing choice about this machine,
/// so it is stored and overridable rather than hard-coded.
#[test]
fn the_proxy_test_endpoint_is_stored_and_takes_effect_immediately() {
    let config = TempConfig::new("echo");
    let (mut settings, _) = Settings::load(env(&config));

    assert_eq!(settings.echo_url(), runtime::DEFAULT_ECHO_URL);
    assert!(
        settings.echo_url().starts_with("http://"),
        "the default must be plain http, so a certificate problem cannot be \
         read as a proxy problem: {}",
        settings.echo_url()
    );

    settings
        .set(SettingKey::EchoUrl, "  http://echo.example/ip  ")
        .expect("save the endpoint");

    assert_eq!(settings.echo_url(), "http://echo.example/ip", "trimmed");
    assert!(
        config.read().contains("http://echo.example/ip"),
        "it is stored, not just held in memory: {}",
        config.read()
    );

    let row = settings
        .rows(en())
        .into_iter()
        .find(|row| row.key == SettingKey::EchoUrl)
        .expect("a row for the endpoint");
    assert_eq!(row.source, Source::ConfigFile);
    assert_eq!(
        row.key.effect().label(en()),
        "now",
        "the next test asks this endpoint, not the next start"
    );
    assert_eq!(row.value, "http://echo.example/ip");
    assert_eq!(
        settings.pending(SettingKey::EchoUrl),
        None,
        "nothing is waiting on a restart: the endpoint is already in use"
    );
}

/// The row has to say that the endpoint learns the address it is asked from,
/// because that is the one thing about this setting a user cannot infer.
#[test]
fn the_endpoint_row_says_what_the_endpoint_learns() {
    let config = TempConfig::new("echo-note");
    let (settings, _) = Settings::load(env(&config));
    let row = settings
        .rows(en())
        .into_iter()
        .find(|row| row.key == SettingKey::EchoUrl)
        .expect("a row for the endpoint");
    let note = row.note.expect("a note about what it sees");
    assert!(note.contains("sees that address"), "{note}");
    assert!(
        note.contains("only when you run a proxy test"),
        "it is asked on demand, not in the background: {note}"
    );
    assert!(row.key.editable(), "the endpoint is the user's to choose");
}

#[test]
fn an_environment_endpoint_shadows_the_stored_one() {
    let config = TempConfig::new("echo-env");
    config.write(r#"{"echo_url": "http://stored.example/ip"}"#);
    let mut environment = env(&config);
    environment.echo_url = Some("http://env.example/ip".to_string());
    let (settings, _) = Settings::load(environment);

    assert_eq!(settings.echo_url(), "http://env.example/ip");
    let row = settings
        .rows(en())
        .into_iter()
        .find(|row| row.key == SettingKey::EchoUrl)
        .expect("a row for the endpoint");
    assert_eq!(row.source, Source::Environment);
    assert_eq!(row.source_label(en()), "set by FP_BROWSER_ECHO_URL");
    assert_eq!(
        row.value, "http://env.example/ip",
        "the page shows the endpoint a test would really ask"
    );
    assert_eq!(
        row.shadowed_label(en()).as_deref(),
        Some("the config file holds http://stored.example/ip, which this overrides"),
        "a stored endpoint the environment wins must be shown, not hidden"
    );
}

#[test]
fn a_broken_config_file_is_reported_and_does_not_lose_settings() {
    let config = TempConfig::new("broken");
    config.write("{ this is not json");
    let (mut settings, notice) = Settings::load(env(&config));

    let (message, is_error) = notice.expect("the problem is reported");
    assert!(is_error);
    assert!(message.contains("config.json"), "{message}");
    assert_eq!(
        settings.data_dir(),
        Path::new(FALLBACK_DATA_DIR),
        "the defaults are used so the app still starts"
    );

    let refusal = settings
        .set(SettingKey::DataDir, "/srv/fp")
        .expect_err("saving is refused while the file is broken");
    assert!(refusal.contains("unreadable"), "{refusal}");
    assert_eq!(
        config.read(),
        "{ this is not json",
        "the existing file was not overwritten"
    );
}

#[test]
fn saving_keeps_the_other_setting() {
    let config = TempConfig::new("keep");
    let (mut settings, _) = Settings::load(env(&config));

    settings
        .set(SettingKey::XrayExecutable, "/opt/xray")
        .expect("save xray");
    settings
        .set(SettingKey::EchoUrl, "http://echo.example/ip")
        .expect("save the endpoint");

    let text = config.read();
    assert!(text.contains("/opt/xray"), "{text}");
    assert!(
        text.contains("http://echo.example/ip"),
        "saving one setting rewrites the file, so every other value has to \
         survive the rewrite: {text}"
    );

    // And a fresh load agrees.
    let (reloaded, _) = Settings::load(env(&config));
    assert_eq!(reloaded.xray_executable(), Path::new("/opt/xray"));
    assert_eq!(reloaded.echo_url(), "http://echo.example/ip");
}

/// The appearance survives a restart, and a config file written before the
/// switch existed still starts - on the palette the program has always
/// painted, not on the component library's default.
#[test]
fn the_appearance_is_stored_and_an_absent_one_means_dark() {
    let config = TempConfig::new("theme");
    config.write(r#"{"xray_executable": "/srv/xray"}"#);
    let (mut settings, _) = Settings::load(env(&config));
    assert_eq!(settings.theme(), ThemeChoice::Dark, "absent means dark");

    settings.set_theme(ThemeChoice::Light).expect("save");
    assert_eq!(settings.theme(), ThemeChoice::Light);
    assert_eq!(
        settings.xray_executable(),
        Path::new("/srv/xray"),
        "kept across the rewrite"
    );

    let (reloaded, _) = Settings::load(env(&config));
    assert_eq!(reloaded.theme(), ThemeChoice::Light);
}

/// What closing the window does survives a restart, and a config file that
/// does not name it asks - which is also what a name from a later build
/// means, because the other three answer a question the user was never asked.
#[test]
fn the_exit_mode_is_stored_and_an_unknown_one_means_asking() {
    let config = TempConfig::new("exit-mode");
    config.write(r#"{"xray_executable": "/srv/xray"}"#);
    let (mut settings, _) = Settings::load(env(&config));
    assert_eq!(settings.exit_mode(), ExitMode::Ask, "absent means ask");

    settings.set_exit_mode(ExitMode::KeepRunning).expect("save");
    assert_eq!(settings.exit_mode(), ExitMode::KeepRunning);
    assert_eq!(
        settings.xray_executable(),
        Path::new("/srv/xray"),
        "kept across the rewrite"
    );

    let (reloaded, _) = Settings::load(env(&config));
    assert_eq!(reloaded.exit_mode(), ExitMode::KeepRunning);

    // A name this build does not know is not a reason to refuse to start,
    // and it is not a guess at what the user meant either.
    config.write(r#"{"exit_mode": "minimize-to-bathtub"}"#);
    let (unknown, _) = Settings::load(env(&config));
    assert_eq!(unknown.exit_mode(), ExitMode::Ask);
}

/// The language survives a restart, a region is not a different language,
/// and a name this build does not know starts in English rather than
/// refusing to start at all.
#[test]
fn the_language_is_stored_and_an_unknown_one_means_english() {
    let config = TempConfig::new("language");
    config.write(r#"{"xray_executable": "/srv/xray"}"#);
    let (mut settings, _) = Settings::load(env(&config));
    assert_eq!(settings.language(), Lang::En, "absent means English");

    settings.set_language(Lang::Zh).expect("save");
    assert_eq!(settings.language(), Lang::Zh);
    assert_eq!(
        settings.xray_executable(),
        Path::new("/srv/xray"),
        "kept across the rewrite"
    );

    let (reloaded, _) = Settings::load(env(&config));
    assert_eq!(reloaded.language(), Lang::Zh, "the choice outlives the run");

    config.write(r#"{"lang": "zh-Hans"}"#);
    let (regional, _) = Settings::load(env(&config));
    assert_eq!(
        regional.language(),
        Lang::Zh,
        "a region is the same language"
    );

    config.write(r#"{"lang": "kl"}"#);
    let (unknown, _) = Settings::load(env(&config));
    assert_eq!(unknown.language(), Lang::En, "not knowing is not a refusal");
    assert!(
        !unknown.rows(en()).is_empty(),
        "and the window still renders"
    );
}

/// `--help` is answered before the window exists and must not be the thing
/// that moves a config file: the hint reads the language and stops there.
#[test]
fn the_language_hint_reads_the_config_file_without_moving_it() {
    let home = std::env::temp_dir().join(format!("fp-settings-home-hint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let legacy = home.join(".config/fp-browser/config.json");
    std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy dir");
    // A field this build does not know, because a hint that refused a file
    // over one unknown key would cost the reader their own language.
    std::fs::write(&legacy, r#"{"lang": "zh", "future_field": 1}"#).expect("legacy config");

    let environment = Environment {
        host: paths::Host {
            home: Some(home.clone()),
            ..paths::Host::default()
        },
        ..Environment::default()
    };

    assert_eq!(language_hint(&environment), Lang::Zh);
    assert!(
        legacy.exists(),
        "answering --help does not move the file it read"
    );
    let data = paths::data_dir(&environment.host).expect("a data directory");
    assert!(
        !data.join(paths::CONFIG_FILE).exists(),
        "and it writes nothing"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_language_hint_that_cannot_be_read_is_english() {
    // No file anywhere: the same answer `Settings::load` would give.
    assert_eq!(language_hint(&Environment::default()), Lang::En);

    let config = TempConfig::new("hint-broken");
    config.write("{ not json");
    assert_eq!(language_hint(&env(&config)), Lang::En);

    // A name this build does not know, and a `lang` that is not a string.
    config.write(r#"{"lang": "kl"}"#);
    assert_eq!(language_hint(&env(&config)), Lang::En);
    config.write(r#"{"lang": 3}"#);
    assert_eq!(language_hint(&env(&config)), Lang::En);
}

/// A config file this build cannot write refuses the change rather than
/// accepting one that would be gone by the next start.
#[test]
fn the_appearance_is_refused_while_the_config_file_is_unreadable() {
    let config = TempConfig::new("theme-broken");
    config.write("{ not json");
    let (mut settings, _) = Settings::load(env(&config));

    let error = settings
        .set_theme(ThemeChoice::Light)
        .expect_err("a broken file cannot be rewritten");

    assert!(error.contains("cannot be written"), "{error}");
    assert_eq!(settings.theme(), ThemeChoice::Dark, "nothing changed");
}

#[test]
fn a_saved_value_is_the_pending_one_even_while_the_process_uses_the_old() {
    let config = TempConfig::new("pending");
    let (mut settings, _) = Settings::load(env(&config));
    assert_eq!(settings.pending(SettingKey::XrayExecutable), None);

    settings
        .set(SettingKey::XrayExecutable, "/opt/xray")
        .expect("save");

    assert!(
        settings
            .rows(en())
            .iter()
            .any(|row| row.key == SettingKey::XrayExecutable && row.value == "/opt/xray"),
        "the page shows what the next start will use"
    );
    assert_eq!(
        settings.pending(SettingKey::XrayExecutable).as_deref(),
        Some("/opt/xray")
    );
}

#[test]
fn an_environment_override_has_no_pending_value() {
    let config = TempConfig::new("pending-env");
    let mut environment = env(&config);
    environment.xray_executable = Some("/tmp/other".to_string());
    let (mut settings, _) = Settings::load(environment);

    // Stored, but overridden: it is not what the next start will use.
    settings
        .set(SettingKey::XrayExecutable, "/opt/xray")
        .expect("save");
    assert_eq!(settings.pending(SettingKey::XrayExecutable), None);
    assert_eq!(settings.xray_executable(), Path::new("/tmp/other"));
}

#[test]
fn an_empty_value_is_refused() {
    let config = TempConfig::new("empty");
    let (mut settings, _) = Settings::load(env(&config));
    let error = settings
        .set(SettingKey::DataDir, "   ")
        .expect_err("empty is refused");
    assert!(error.contains("cannot be empty"), "{error}");
}

#[test]
fn the_read_only_settings_cannot_be_saved() {
    let config = TempConfig::new("readonly");
    let (mut settings, _) = Settings::load(env(&config));
    for key in [
        SettingKey::ChromiumBin,
        SettingKey::ChromiumMajor,
        SettingKey::ConfigFile,
        SettingKey::RuntimeDir,
    ] {
        assert!(!key.editable(), "{key:?} is not editable");
        assert!(settings.set(key, "/tmp/x").is_err(), "{key:?}");
    }
}

#[test]
fn the_runtime_directory_follows_the_data_directory() {
    let config = TempConfig::new("runtime");
    let (settings, _) = Settings::load(env(&config));
    assert_eq!(settings.runtime_dir(), Path::new("data/runtime"));
    let row = settings
        .rows(en())
        .into_iter()
        .find(|row| row.key == SettingKey::RuntimeDir)
        .expect("a runtime row");
    assert_eq!(
        row.key.effect().label(en()),
        "derived from the data directory"
    );
    assert_eq!(row.source, Source::Derived);
    assert_eq!(
        row.source_label(en()),
        "derived from the data directory",
        "it is computed, not chosen anywhere"
    );
}

#[test]
fn a_relative_data_directory_is_shown_against_the_working_directory() {
    let config = TempConfig::new("relative");
    let (settings, _) = Settings::load(env(&config));
    let value = &settings.rows(en())[0].value;
    assert!(value.starts_with(FALLBACK_DATA_DIR), "{value}");
    assert!(
        value.contains("relative to"),
        "a relative path is shown with what it is relative to: {value}"
    );
}

#[test]
fn the_settings_page_names_every_source() {
    let config = TempConfig::new("sources");
    config.write(r#"{"xray_executable": "/opt/xray"}"#);
    let mut environment = env(&config);
    environment.chromium_bin = Some("/opt/chrome".to_string());
    let (settings, _) = Settings::load(environment);

    let rows = settings.rows(en());
    let by_key = |key: SettingKey| {
        rows.iter()
            .find(|row| row.key == key)
            .expect("the row exists")
            .clone()
    };
    assert_eq!(
        by_key(SettingKey::DataDir).source,
        Source::Default,
        "nothing but the environment can move the data directory now"
    );
    assert_eq!(
        by_key(SettingKey::XrayExecutable).source,
        Source::ConfigFile
    );
    assert_eq!(by_key(SettingKey::ChromiumBin).source, Source::Environment);
    assert_eq!(
        by_key(SettingKey::ChromiumMajor).value,
        "not set; read from each binary"
    );
    assert_eq!(
        by_key(SettingKey::ConfigFile).value,
        config.path.to_string_lossy()
    );
}
