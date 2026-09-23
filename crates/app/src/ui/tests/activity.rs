use super::*;

/// The three answers are three of the same thing, and the last one is whole.
///
/// Both halves were wrong before this was a test: the last row came out a text
/// line shorter than the two above it and its own note painted over where its
/// bottom border was. The text, the order and the line height were each ruled
/// out by measuring, which is why the height is now given rather than derived.
#[gpui_kit::test]
fn the_exit_dialog_shows_three_equal_answers_inside_itself(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-exit-equal-{}", std::process::id()));
    let (view, _) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);

    let _ = cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
    settle(cx);

    let bounds = |id: &str, cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|window, _| window.try_find(id.to_string()).expect(id).bounds())
    };
    let surface = cx.debug_bounds("dialog-0").expect("the dialog's surface");
    let rows = [
        "exit-choice-background",
        "exit-choice-keep-running",
        "exit-choice-exit-all",
    ]
    .map(|id| bounds(id, cx));

    assert_eq!(rows[0].size.height, rows[1].size.height, "{rows:?}");
    assert_eq!(rows[1].size.height, rows[2].size.height, "{rows:?}");
    for (index, row) in rows.iter().enumerate() {
        assert!(
            row.bottom() <= surface.bottom(),
            "answer {index} is outside the dialog: {row:?} in {surface:?}"
        );
        if let Some(next) = rows.get(index + 1) {
            assert!(
                row.bottom() <= next.origin.y,
                "answer {index} overlaps the next one: {rows:?}"
            );
        }
    }
}

#[gpui_kit::test]
fn an_action_is_acknowledged_with_a_toast(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    flush_toasts(cx, &view);

    assert_eq!(
        cx.update(|window, cx| window.notifications(cx).len()),
        1,
        "creating a profile is acknowledged"
    );

    // A toast is shown once. Draining what was already shown must not
    // duplicate it on the next tick.
    flush_toasts(cx, &view);
    assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 1);
}

#[gpui_kit::test]
fn a_problem_keeps_the_banner_and_still_gets_a_toast(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    // A value the settings file refuses is a problem and not a success: it
    // stays in the banner and is toasted while it happens.
    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    open_settings_group(cx, crate::settings::SettingGroup::Runtime);
    cx.update(|window, cx| window.click("edit-setting-echo-url", cx));
    settle(cx);
    let field = view
        .read_with(cx, |view, _| view.setting_editor())
        .expect("the settings field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| state.set_value("   ", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("dismiss-notice").is_some()),
        "a problem stays in the banner until it is dismissed"
    );
    flush_toasts(cx, &view);
    assert!(
        cx.update(|window, cx| window.notifications(cx).len()) >= 1,
        "the problem is also toasted while it happens"
    );
}

#[gpui_kit::test]
fn the_sidebar_switches_to_the_log_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    assert!(
        cx.update(|window, _| window.try_find("logs-scroll").is_none()),
        "the log page is not shown first"
    );

    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("logs-scroll").is_some()),
        "the log page is shown"
    );
    assert!(
        cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
        "only one page is rendered at a time"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Log
    );
}

#[gpui_kit::test]
fn the_log_page_lists_what_happened_and_can_be_cleared(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    let label = cx
        .update(|window, _| window.find("log-0").label().map(|label| label.to_string()))
        .expect("the newest line is rendered");
    assert!(
        label.contains("Created Profile 1"),
        "the line says what happened: {label}"
    );
    assert!(label.contains("[info]"), "and at what level: {label}");

    cx.update(|window, cx| window.click("clear-log", cx));
    settle(cx);
    assert!(
        view.read_with(cx, |view, _| view.state().log_rows().is_empty()),
        "clearing empties the history"
    );
    assert!(
        cx.update(|window, _| window.try_find("logs-scroll").is_some()),
        "the page stays put with an empty history"
    );
}

#[gpui_kit::test]
fn the_log_page_filter_can_be_narrowed(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("log-0").is_some()),
        "the created profile is an info line"
    );

    cx.update(|window, cx| window.click("log-filter-errors", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("log-0").is_none()),
        "an info line is hidden by the error filter"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().log_filter()),
        crate::state::LogFilter::Errors
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().log_len()),
        1,
        "the line is hidden, not dropped"
    );

    cx.update(|window, cx| window.click("log-filter-all", cx));
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("log-0").is_some()));
}

/// The page has to say where the log is being kept, and the file has to
/// really hold what the page shows.
#[gpui_kit::test]
fn the_log_page_names_the_file_it_writes_to(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let log = crate::log_file::LogFile::open(&dir).expect("open the log file");
    let path = log.path().to_path_buf();
    let (view, _runtime, _opener) = view_with_log(
        cx,
        Arc::new(FakeVerifier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        Some(log),
    );
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    let label = cx
        .update(|window, _| {
            window
                .find("log-file-status")
                .label()
                .map(|label| label.to_string())
        })
        .expect("the page says where the log is");
    assert!(label.contains(&path.display().to_string()), "{label}");

    let text = std::fs::read_to_string(&path).expect("the file was written");
    assert!(text.contains("Created Profile 1"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
fn a_refused_edit_keeps_the_dialog_open_and_shows_why(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);
    let name_before = view.read_with(cx, |view, _| view.state().rows()[0].profile.name.clone());

    profile_menu(cx, id, ProfileMenu::Edit);

    let editor = view
        .read_with(cx, |view, _| view.editor())
        .expect("an editor");
    let seed = editor.read_with(cx, |editor, _| editor.seed_input());
    cx.update(|window, cx| {
        seed.update(cx, |state, cx| state.set_value("not a number", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("editor-name").is_some()),
        "a refused save leaves the form open"
    );
    assert!(
        cx.update(|window, _| window.try_find("editor-error").is_some()),
        "the form says what is wrong"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows()[0].profile.name.clone()),
        name_before,
        "nothing was written"
    );
}

/// A line longer than a row is clamped, and the rest of it is one press away.
///
/// The identity of an open line is when it was written, not where it currently
/// sits: the list is newest-first, so a new line moves every index below it, and
/// a line opened by index would open somebody else's line the moment one arrived.
#[gpui_kit::test]
fn a_long_log_line_is_clamped_until_it_is_asked_for(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    let id = seed_profile(cx, &view);

    let tail = "and this is the end of it, which is the part nobody can see";
    let long = format!("{}{tail}", "an engine that says too much ".repeat(12));
    let at = view.update(cx, |view, _| {
        view.state_mut().record_event(&RuntimeEvent::Warning {
            profile_id: id,
            message: long.clone(),
        });
        view.state().log_rows()[0].at
    });

    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    let toggle = |cx: &mut gpui_kit::VisualTestContext, index: usize| {
        cx.update(|window, _| {
            window
                .find(format!("log-toggle-{index}"))
                .label()
                .map(|label| label.to_string())
        })
        .unwrap_or_else(|| panic!("log line {index} has a control of its own"))
    };
    let height = |cx: &mut gpui_kit::VisualTestContext, index: usize| {
        cx.update(|window, _| {
            window
                .find(format!("log-{index}"))
                .bounds()
                .size
                .height
                .as_f32()
        })
    };

    // Clamped: the row offers the rest and says how much of it there is. The
    // visible text itself is what the real window shows; what a test can hold is
    // that a line this long is not laid out whole.
    let clamped = toggle(cx, 0);
    assert!(
        clamped.contains(&(long.chars().count() - 150).to_string()),
        "the control says how much is hidden: {clamped}"
    );
    let collapsed_height = height(cx, 0);

    cx.update(|window, cx| window.click("log-toggle-0", cx));
    settle(cx);
    assert!(
        toggle(cx, 0).contains("Collapse"),
        "the control now closes it again"
    );
    assert!(
        height(cx, 0) > collapsed_height,
        "the whole line takes more room than the clamped one: {} vs {collapsed_height}",
        height(cx, 0)
    );
    assert!(
        view.read_with(cx, |view, _| view.state().log_expanded(at)),
        "the line is remembered as open"
    );

    // A newer line arrives above it. The open one stays open, because it is
    // keyed by when it was written rather than by where it now sits - and the
    // new line, which is short, offers no control at all.
    view.update(cx, |view, _| {
        view.state_mut().record_event(&RuntimeEvent::Warning {
            profile_id: id,
            message: "a newer warning".to_string(),
        });
    });
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("log-toggle-1").is_some()),
        "the open line kept its control at its new place in the list"
    );
    assert!(toggle(cx, 1).contains("Collapse"), "and is still open");
    assert!(
        cx.update(|window, _| window.try_find("log-toggle-0").is_none()),
        "while the short new line has nothing to open"
    );
}

/// Clearing is named for what it clears, and the file keeps what it clears.
#[gpui_kit::test]
fn clearing_the_list_says_what_it_does_not_touch(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-log-clear-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let log = crate::log_file::LogFile::open(&dir).expect("open the log file");
    let path = log.path().to_path_buf();
    let (view, _runtime, _opener) = view_with_log(
        cx,
        Arc::new(FakeVerifier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        Some(log),
    );
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    let scope = cx
        .update(|window, _| {
            window
                .find("log-clear-scope")
                .label()
                .map(|label| label.to_string())
        })
        .expect("the scope of a clear is on the page");
    assert!(scope.contains("log file"), "{scope}");

    cx.update(|window, cx| window.click("clear-log", cx));
    settle(cx);
    assert!(
        view.read_with(cx, |view, _| view.state().log_len()) == 0,
        "the list is empty"
    );
    let text = std::fs::read_to_string(&path).expect("the file is still there");
    assert!(
        text.contains("Created Profile 1"),
        "and the file still holds what was cleared from the list: {text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The log's four columns line up under the headings that name them.
#[gpui_kit::test]
fn the_log_rows_line_up_under_the_column_headings(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_aligned(
            window.find("column-time").bounds().left(),
            window.find("log-time-0").bounds().left(),
            "the time column",
        );
        assert_aligned(
            window.find("column-level").bounds().left(),
            window.find("log-level-0").bounds().left(),
            "the level column",
        );
        assert_aligned(
            window.find("column-log-who").bounds().left(),
            window.find("log-who-0").bounds().left(),
            "the profile column",
        );
        assert_aligned(
            window.find("column-message").bounds().left(),
            window.find("log-body-0").bounds().left(),
            "the message column",
        );
    });
}
