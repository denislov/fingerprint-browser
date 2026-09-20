//! The paste dialog: a share link becomes a proxy.
//!
//! Four of the six protocols have no form of their own. A link carries what they
//! need - a public key, a short id, a uTLS fingerprint, a websocket path, an
//! alteration count - and typing those by hand is how mistakes are made, so a
//! link is the way in for them. It works for the other two as well, for anyone
//! who has a link rather than an address.
//!
//! Like the editor beside it, this never writes: it reads the link into the
//! model, and the view hands that to [`crate::state::AppState`], which refuses
//! it through the same rules a typed proxy goes through.

use crate::text::Text;
use crate::theme::palette;
use domain::{ProxyOutbound, parse_proxy_uri};
use gpui_kit::component::form::*;
use gpui_kit::component::input::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const LABEL_WIDTH: f32 = 150.0;

/// A link, and the reason the last one was refused.
pub struct ProxyImport {
    /// The table the dialog's words come from; see `ProfileEditor::text`.
    text: &'static Text,
    link: Entity<InputState>,
    error: Option<String>,
}

impl ProxyImport {
    pub fn new(text: &'static Text, window: &mut Window, cx: &mut App) -> Self {
        Self {
            text,
            link: cx.new(|cx| InputState::new(window, cx)),
            error: None,
        }
    }

    pub fn title(&self) -> &'static str {
        let t = self.text;
        t.import_link_title
    }

    /// The field itself, so a test can type a link into the dialog.
    #[cfg(test)]
    pub fn link_input(&self) -> Entity<InputState> {
        self.link.clone()
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    /// Why the last attempt was refused, as the dialog would show it.
    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// What the link says, or why it cannot be read.
    ///
    /// The name is the link's own remark, or the protocol and host when it
    /// carries none. The refusal is the parser's own sentence, which names the
    /// field it could not use - the dialog does not paraphrase it.
    pub fn read_link(&self, cx: &App) -> Result<(String, ProxyOutbound), String> {
        let link = self.link.read(cx).value().to_string();
        let parsed = parse_proxy_uri(&link).map_err(|error| error.to_string())?;
        Ok((parsed.suggested_name(), parsed.outbound))
    }
}

impl Render for ProxyImport {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.text;
        let p = palette(cx);
        let mut form = Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(LABEL_WIDTH))
            .child(
                Field::new()
                    .label(t.share_link)
                    .description("ss://, vmess://, vless:// or trojan://")
                    .child(
                        Input::new(&self.link)
                            .id("proxy-link")
                            .aria_label(t.share_link),
                    ),
            );

        if let Some(error) = self.error.clone() {
            form = form.footer(
                div()
                    .id("proxy-import-error")
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
