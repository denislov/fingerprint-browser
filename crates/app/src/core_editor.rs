//! The browser-core form: adding a binary, or renaming/re-pointing one.
//!
//! The version is never typed here. Adding a core means telling the service
//! where the binary is; the service runs `--version` and stores what came back,
//! because the major decides which fingerprint switches the core may claim.
//! A binary that does not answer is refused with the reason, and the form stays
//! open with it.
//!
//! Re-pointing an existing core at a different binary is the one edit that
//! re-reads the version, for the same reason.

use crate::text::Text;
use crate::theme::palette;
use domain::BrowserCore;
use gpui_kit::component::form::*;
use gpui_kit::component::input::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const LABEL_WIDTH: f32 = 150.0;

pub struct CoreEditor {
    /// The table the form's labels come from; see `ProfileEditor::text`.
    text: &'static Text,
    /// The core being edited, or `None` when adding one.
    base: Option<BrowserCore>,
    name: Entity<InputState>,
    executable: Entity<InputState>,
    /// Why the last save attempt was refused.
    error: Option<String>,
}

impl CoreEditor {
    /// A form for a new core: a path, and an optional name.
    pub fn new(text: &'static Text, window: &mut Window, cx: &mut App) -> Self {
        let field = |value: &str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
        };
        Self {
            text,
            base: None,
            name: field("", window, cx),
            executable: field("", window, cx),
            error: None,
        }
    }

    /// A form opened on a core that is already registered.
    pub fn for_core(
        core: &BrowserCore,
        text: &'static Text,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let field = |value: &str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
        };
        Self {
            text,
            base: Some(core.clone()),
            name: field(&core.name, window, cx),
            executable: field(&core.executable.to_string_lossy(), window, cx),
            error: None,
        }
    }

    pub fn title(&self) -> &'static str {
        let t = self.text;
        if self.base.is_some() {
            t.edit_core_title
        } else {
            t.add_core_title
        }
    }

    pub fn is_edit(&self) -> bool {
        self.base.is_some()
    }

    #[cfg(test)]
    pub fn core_id(&self) -> Option<domain::CoreId> {
        self.base.as_ref().map(|core| core.id)
    }

    /// The typed name, trimmed, or `None` when it was left blank.
    pub fn name(&self, cx: &App) -> Option<String> {
        let name = self.name.read(cx).value().trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    pub fn executable(&self, cx: &App) -> std::path::PathBuf {
        std::path::PathBuf::from(self.executable.read(cx).value().trim())
    }

    /// The core as it would be saved.
    ///
    /// For a new core there is nothing to build yet: the version and major come
    /// from the probe the service runs. The path is checked here so the obvious
    /// mistake is caught before a thread is spent on it.
    pub fn build_core(&self, cx: &App) -> Result<BrowserCore, String> {
        let t = self.text;
        let base = self
            .base
            .as_ref()
            .ok_or_else(|| t.add_core_use_add.to_string())?;
        let executable = self.executable(cx);
        if executable.as_os_str().is_empty() {
            return Err(t.executable_cannot_be_empty.to_string());
        }
        Ok(BrowserCore {
            name: self.name(cx).unwrap_or_else(|| base.name.clone()),
            executable,
            ..base.clone()
        })
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub fn name_input(&self) -> Entity<InputState> {
        self.name.clone()
    }

    #[cfg(test)]
    pub fn executable_input(&self) -> Entity<InputState> {
        self.executable.clone()
    }
}

impl Render for CoreEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.text;
        let p = palette(cx);
        let _ = cx;
        let known = self.base.as_ref().map(|core| {
            let generation = core.capabilities().map(|capabilities| {
                t.core_picker_label(
                    &capabilities.generation_label(),
                    capabilities.exclusion_label(),
                )
            });
            (core.version.clone(), generation)
        });

        let mut form = Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(LABEL_WIDTH))
            .child(
                Field::new()
                    .label(t.executable_field)
                    .description(t.executable_help)
                    .child(
                        Input::new(&self.executable)
                            .id("core-executable")
                            .aria_label(t.core_executable),
                    ),
            )
            .child(
                Field::new()
                    .label(t.name_field)
                    .description(t.core_name_help)
                    .child(
                        Input::new(&self.name)
                            .id("core-name")
                            .aria_label(t.core_name),
                    ),
            );

        if let Some((version, generation)) = known {
            form = form.child(
                Field::new()
                    .label(t.version_field)
                    .description(t.version_help)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_xs()
                            .child(div().text_color(rgb(p.text_soft)).child(version))
                            .children(generation.map(|generation| {
                                div().text_color(rgb(p.muted)).child(generation)
                            })),
                    ),
            );
        }

        if let Some(error) = self.error.clone() {
            form = form.footer(
                div()
                    .id("core-error")
                    .test_support()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(p.danger_bg))
                    .text_xs()
                    .text_color(rgb(p.danger))
                    // One line per clause: gpui does not wrap a single line, and
                    // a refusal that runs off the edge is a refusal half read.
                    .children(
                        error
                            .split("; ")
                            .map(|clause| div().child(clause.to_string())),
                    ),
            );
        }
        form
    }
}

#[cfg(test)]
mod tests {
    use crate::text::en;

    // Explicit imports: a glob here pulls the whole gpui surface into the test
    // macro's expansion and makes it recurse.
    use super::CoreEditor;
    use domain::{BrowserCore, CoreId};
    use gpui_kit::component::input::InputState;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};
    use std::path::PathBuf;

    fn core() -> BrowserCore {
        BrowserCore {
            id: CoreId::new(),
            name: "chrome 148".to_string(),
            executable: PathBuf::from("/opt/chrome"),
            version: "Chromium 148.0.7778.215".to_string(),
            major: 148,
        }
    }

    fn editor<'a>(
        cx: &'a mut TestAppContext,
        core: Option<&BrowserCore>,
    ) -> (Entity<CoreEditor>, &'a mut VisualTestContext) {
        let core = core.cloned();
        cx.add_window_view(move |window, cx| match &core {
            Some(core) => CoreEditor::for_core(core, en(), window, cx),
            None => CoreEditor::new(en(), window, cx),
        })
    }

    fn set(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        let input = input.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        });
    }

    #[gpui_kit::test]
    fn the_form_opens_on_a_stored_core(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = core();
        let (editor, cx) = editor(cx, Some(&original));

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_core(cx))
            .expect("an untouched form is valid");

        assert_eq!(rebuilt.id, original.id);
        assert_eq!(rebuilt.name, "chrome 148");
        assert_eq!(rebuilt.executable, original.executable);
        assert_eq!(
            rebuilt.version, original.version,
            "the version is carried, not typed"
        );
        assert_eq!(rebuilt.major, 148);
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.title()),
            "Edit browser core"
        );
        assert!(editor.read_with(cx, |editor, _| editor.is_edit()));
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.core_id()),
            Some(original.id)
        );
    }

    #[gpui_kit::test]
    fn a_renamed_core_keeps_its_version(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = core();
        let (editor, cx) = editor(cx, Some(&original));

        let name = editor.read_with(cx, |editor, _| editor.name_input());
        set(cx, &name, "  Work browser  ");

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_core(cx))
            .expect("valid");
        assert_eq!(rebuilt.name, "Work browser");
        assert_eq!(rebuilt.major, 148);
        assert_eq!(rebuilt.version, "Chromium 148.0.7778.215");
    }

    #[gpui_kit::test]
    fn a_blank_name_leaves_the_name_alone(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = core();
        let (editor, cx) = editor(cx, Some(&original));

        let name = editor.read_with(cx, |editor, _| editor.name_input());
        set(cx, &name, "   ");

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_core(cx))
            .expect("valid");
        assert_eq!(rebuilt.name, "chrome 148");
        assert!(editor.read_with(cx, |editor, cx| editor.name(cx)).is_none());
    }

    #[gpui_kit::test]
    fn re_pointing_a_core_takes_the_new_path(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = core();
        let (editor, cx) = editor(cx, Some(&original));

        let executable = editor.read_with(cx, |editor, _| editor.executable_input());
        set(cx, &executable, "/opt/other/chrome");

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_core(cx))
            .expect("valid");
        assert_eq!(rebuilt.executable, PathBuf::from("/opt/other/chrome"));
        assert_eq!(
            rebuilt.major, 148,
            "the probe decides the major, not this form"
        );
    }

    #[gpui_kit::test]
    fn a_new_form_has_no_core_to_build(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = editor(cx, None);

        assert_eq!(
            editor.read_with(cx, |editor, _| editor.title()),
            "Add browser core"
        );
        assert!(!editor.read_with(cx, |editor, _| editor.is_edit()));
        let error = editor
            .read_with(cx, |editor, cx| editor.build_core(cx))
            .expect_err("there is nothing to build yet");
        assert!(error.contains("picking a binary"), "{error}");
    }

    #[gpui_kit::test]
    fn an_empty_path_is_refused_by_the_form(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = core();
        let (editor, cx) = editor(cx, Some(&original));

        let executable = editor.read_with(cx, |editor, _| editor.executable_input());
        set(cx, &executable, "   ");

        let error = editor
            .read_with(cx, |editor, cx| editor.build_core(cx))
            .expect_err("no path");
        assert!(error.contains("cannot be empty"), "{error}");
    }

    #[gpui_kit::test]
    fn the_edited_path_is_what_a_new_core_would_use(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = editor(cx, None);
        let executable = editor.read_with(cx, |editor, _| editor.executable_input());
        let name = editor.read_with(cx, |editor, _| editor.name_input());
        set(cx, &executable, "  /opt/chrome  ");
        set(cx, &name, "  Mine  ");

        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.executable(cx)),
            PathBuf::from("/opt/chrome")
        );
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.name(cx)),
            Some("Mine".to_string())
        );
    }
}
