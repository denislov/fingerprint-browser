//! The proxy editor: the form behind "New proxy" and "Edit proxy".
//!
//! This form is for the two protocols that are an address and a pair of
//! credentials. The other four carry a public key, a short id, a uTLS
//! fingerprint or a websocket path, which is not something to type: they arrive
//! through the paste dialog beside it ([`crate::proxy_import::ProxyImport`]), and
//! they are named here so the list does not look like an oversight.
//!
//! Like the profile editor, this never writes: the view turns the form into a
//! [`ProxyProfile`] through [`ProxyEditor::build_outbound`], hands it to
//! [`crate::state::AppState`], and shows the refusal without closing.

use crate::text::Text;
use crate::theme::{Palette, palette};
use domain::{HttpOutbound, ProxyOutbound, ProxyProfile, Socks5Outbound, validate_proxy};
use gpui_kit::component::form::*;
use gpui_kit::component::input::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const LABEL_WIDTH: f32 = 150.0;

/// The protocols this form can build, in the order they are offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyKind {
    Socks5,
    Http,
}

impl ProxyKind {
    pub const ALL: [(Self, &'static str); 2] = [(Self::Socks5, "SOCKS5"), (Self::Http, "HTTP")];

    /// Protocols with no form here: they are created by pasting a share link.
    pub const BY_LINK: [&'static str; 4] = ["Shadowsocks", "VMess", "VLESS", "Trojan"];

    fn of(outbound: &ProxyOutbound) -> Self {
        match outbound {
            ProxyOutbound::Http(_) => Self::Http,
            _ => Self::Socks5,
        }
    }
}

/// An editable copy of one proxy.
pub struct ProxyEditor {
    /// The table the form's labels come from; see `ProfileEditor::text`.
    text: &'static Text,
    /// The proxy this form started from, for the id and as the save target.
    base: Option<ProxyProfile>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    kind: ProxyKind,
    /// Why the last save attempt was refused.
    error: Option<String>,
}

impl ProxyEditor {
    /// A form for a new proxy: SOCKS5 on the conventional port, nothing else.
    pub fn new(text: &'static Text, window: &mut Window, cx: &mut App) -> Self {
        let field = |value: &str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
        };
        Self {
            text,
            base: None,
            name: field("", window, cx),
            host: field("", window, cx),
            port: field("1080", window, cx),
            username: field("", window, cx),
            password: field("", window, cx),
            kind: ProxyKind::Socks5,
            error: None,
        }
    }

    /// A form opened on a proxy that is already stored.
    pub fn for_proxy(
        proxy: &ProxyProfile,
        text: &'static Text,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let field = |value: &str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
        };
        let (username, password) = match &proxy.outbound {
            ProxyOutbound::Socks5(o) => (o.username.clone(), o.password.clone()),
            ProxyOutbound::Http(o) => (o.username.clone(), o.password.clone()),
            _ => (None, None),
        };
        Self {
            text,
            base: Some(proxy.clone()),
            name: field(&proxy.name, window, cx),
            host: field(proxy.outbound.host(), window, cx),
            port: field(&proxy.outbound.port().to_string(), window, cx),
            username: field(username.as_deref().unwrap_or(""), window, cx),
            password: field(password.as_deref().unwrap_or(""), window, cx),
            kind: ProxyKind::of(&proxy.outbound),
            error: None,
        }
    }

    pub fn title(&self) -> &'static str {
        let t = self.text;
        if self.base.is_some() {
            t.edit_proxy_title
        } else {
            t.new_proxy_title
        }
    }

    pub fn is_edit(&self) -> bool {
        self.base.is_some()
    }

    /// The whole proxy, with the id it already had when editing.
    pub fn build_proxy(&self, cx: &App) -> Result<ProxyProfile, String> {
        let outbound = self.build_outbound(cx)?;
        Ok(ProxyProfile {
            id: self.base.as_ref().map(|proxy| proxy.id).unwrap_or_default(),
            name: self.text(&self.name, cx),
            outbound,
        })
    }

    /// Only the part the protocol decides, which is all a new proxy draft needs.
    pub fn build_outbound(&self, cx: &App) -> Result<ProxyOutbound, String> {
        let t = self.text;
        let host = self.text(&self.host, cx);
        let port: u16 = self
            .text(&self.port, cx)
            .parse()
            .map_err(|_| t.port_whole_number.to_string())?;
        let username = self.optional(&self.username, cx);
        let password = self.optional(&self.password, cx);

        let outbound = match self.kind {
            ProxyKind::Socks5 => ProxyOutbound::Socks5(Socks5Outbound {
                host,
                port,
                username,
                password,
            }),
            ProxyKind::Http => ProxyOutbound::Http(HttpOutbound {
                host,
                port,
                username,
                password,
            }),
        };

        // The same rules the proxy service applies before it stores anything.
        let candidate = ProxyProfile {
            id: self.base.as_ref().map(|proxy| proxy.id).unwrap_or_default(),
            name: self.text(&self.name, cx),
            outbound: outbound.clone(),
        };
        validate_proxy(&candidate).map_err(|error| error.to_string())?;
        Ok(outbound)
    }

    fn text(&self, input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    fn optional(&self, input: &Entity<InputState>, cx: &App) -> Option<String> {
        let value = self.text(input, cx);
        (!value.is_empty()).then_some(value)
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Switches protocol, taking the conventional port with it.
    ///
    /// Only while the port still holds the previous protocol's default: a port
    /// the user typed is theirs and survives the switch.
    fn set_kind(&mut self, kind: ProxyKind, window: &mut Window, cx: &mut App) {
        let current = self.port.read(cx).value().trim().to_string();
        let untouched = current.is_empty() || current == self.kind.default_port().to_string();
        if untouched {
            let next = kind.default_port().to_string();
            self.port
                .update(cx, |state, cx| state.set_value(next, window, cx));
        }
        self.kind = kind;
    }

    /// For callers that drive the form directly.
    #[cfg(test)]
    pub fn name_input(&self) -> Entity<InputState> {
        self.name.clone()
    }

    #[cfg(test)]
    pub fn host_input(&self) -> Entity<InputState> {
        self.host.clone()
    }

    #[cfg(test)]
    pub fn port_input(&self) -> Entity<InputState> {
        self.port.clone()
    }

    #[cfg(test)]
    pub fn username_input(&self) -> Entity<InputState> {
        self.username.clone()
    }
}

impl ProxyKind {
    fn default_port(self) -> u16 {
        match self {
            Self::Socks5 => 1080,
            Self::Http => 8080,
        }
    }
}

/// One of the two protocols this form fills in, as a chip.
fn kind_row(editor: Entity<ProxyEditor>, selected: ProxyKind, p: Palette) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children(ProxyKind::ALL.iter().map(|(kind, label)| {
            let kind = *kind;
            let active = kind == selected;
            let editor = editor.clone();
            div()
                .id(format!("proxy-kind-{label}"))
                .test_support()
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(rgb(if active { p.dim } else { p.border }))
                .text_xs()
                .when(active, |this| {
                    this.bg(rgb(p.border))
                        .text_color(rgb(p.text))
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!active, |this| this.text_color(rgb(p.muted)))
                .child(*label)
                .on_click(move |_, window, cx| {
                    editor.update(cx, |editor, cx| {
                        editor.set_kind(kind, window, cx);
                        cx.notify();
                    });
                })
        }))
}

/// The protocols that are not filled in here, named rather than hidden.
///
/// Split into short lines on purpose: the dialog is 640px wide, and a longer
/// sentence is clipped at the edge instead of wrapping.
fn unbuildable_note(p: Palette, t: &Text) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .text_xs()
        .text_color(rgb(p.muted))
        .child(t.link_only_protocols(&ProxyKind::BY_LINK.join(", ")))
        .child(t.link_only_note)
        .child(t.link_only_hint)
}

impl Render for ProxyEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.text;
        let p = palette(cx);
        let editor = cx.entity();

        let mut form = Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(LABEL_WIDTH))
            .child(
                Field::new().label(t.name_field).child(
                    Input::new(&self.name)
                        .id("proxy-name")
                        .aria_label(t.proxy_name),
                ),
            )
            .child(
                Field::new().label(t.protocol_field).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(kind_row(editor.clone(), self.kind, p))
                        .child(unbuildable_note(p, t)),
                ),
            )
            .child(
                Field::new()
                    .label(t.host_field)
                    .description(t.host_help)
                    .child(
                        Input::new(&self.host)
                            .id("proxy-host")
                            .aria_label(t.proxy_host),
                    ),
            )
            .child(
                Field::new().label(t.port_field).child(
                    Input::new(&self.port)
                        .id("proxy-port")
                        .aria_label(t.proxy_port),
                ),
            )
            .child(
                Field::new()
                    .label(t.username_field)
                    .description(t.username_help)
                    .child(
                        Input::new(&self.username)
                            .id("proxy-username")
                            .aria_label(t.proxy_username),
                    ),
            )
            .child(
                Field::new().label(t.password_field).child(
                    Input::new(&self.password)
                        .id("proxy-password")
                        .aria_label(t.proxy_password),
                ),
            );

        if let Some(error) = self.error.clone() {
            form = form.footer(
                div()
                    .id("proxy-error")
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
    use super::{ProxyEditor, ProxyKind};
    use domain::{
        HttpOutbound, ProxyId, ProxyOutbound, ProxyProfile, Socks5Outbound, ValidationError,
    };
    use gpui_kit::component::input::InputState;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    fn stored(kind: ProxyKind) -> ProxyProfile {
        let outbound = match kind {
            ProxyKind::Socks5 => ProxyOutbound::Socks5(Socks5Outbound {
                host: "10.0.0.1".to_string(),
                port: 1080,
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
            }),
            ProxyKind::Http => ProxyOutbound::Http(HttpOutbound {
                host: "proxy.example".to_string(),
                port: 3128,
                username: None,
                password: None,
            }),
        };
        ProxyProfile {
            id: ProxyId::new(),
            name: "Office".to_string(),
            outbound,
        }
    }

    /// The editor inside a window, so its inputs can be driven.
    fn editor<'a>(
        cx: &'a mut TestAppContext,
        proxy: Option<&ProxyProfile>,
    ) -> (Entity<ProxyEditor>, &'a mut VisualTestContext) {
        let proxy = proxy.cloned();
        cx.add_window_view(move |window, cx| match &proxy {
            Some(proxy) => ProxyEditor::for_proxy(proxy, en(), window, cx),
            None => ProxyEditor::new(en(), window, cx),
        })
    }

    fn port_value(cx: &mut VisualTestContext, input: &Entity<InputState>) -> String {
        input.read_with(cx, |state, _| state.value().to_string())
    }

    fn set(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        let input = input.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        });
    }

    #[gpui_kit::test]
    fn a_new_form_starts_on_socks5_and_needs_a_name_and_host(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = editor(cx, None);

        let error = editor
            .read_with(cx, |editor, cx| editor.build_proxy(cx))
            .expect_err("an empty form is not valid");
        assert!(error.contains("name"), "{error}");

        let (name, host, port) = editor.read_with(cx, |editor, _| {
            (
                editor.name_input(),
                editor.host_input(),
                editor.port_input(),
            )
        });
        set(cx, &name, "Office");
        let error = editor
            .read_with(cx, |editor, cx| editor.build_proxy(cx))
            .expect_err("a proxy without a host is not valid");
        assert!(error.contains("host"), "{error}");

        set(cx, &host, "10.0.0.1");
        assert_eq!(port_value(cx, &port), "1080");
        let proxy = editor
            .read_with(cx, |editor, cx| editor.build_proxy(cx))
            .expect("a named proxy with a host is valid");
        assert_eq!(proxy.name, "Office");
        assert_eq!(proxy.outbound.kind(), "socks5");
        assert_eq!(proxy.endpoint(), "socks5://10.0.0.1:1080");
    }

    #[gpui_kit::test]
    fn the_form_opens_on_a_stored_proxy(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = stored(ProxyKind::Http);
        let (editor, cx) = editor(cx, Some(&original));

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_proxy(cx))
            .expect("an untouched form is valid");

        assert_eq!(rebuilt.id, original.id, "the id it already had is kept");
        assert_eq!(rebuilt.name, "Office");
        assert_eq!(rebuilt.outbound, original.outbound);
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.title()),
            "Edit proxy"
        );
        assert!(editor.read_with(cx, |editor, _| editor.is_edit()));
    }

    #[gpui_kit::test]
    fn a_port_the_user_typed_survives_a_protocol_switch(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = editor(cx, None);
        let (name, host, port) = editor.read_with(cx, |editor, _| {
            (
                editor.name_input(),
                editor.host_input(),
                editor.port_input(),
            )
        });
        set(cx, &name, "Office");
        set(cx, &host, "10.0.0.1");
        set(cx, &port, "9050");

        switch_to(cx, &editor, ProxyKind::Http);

        assert_eq!(port_value(cx, &port), "9050", "a typed port is kept");
        let proxy = editor
            .read_with(cx, |editor, cx| editor.build_proxy(cx))
            .expect("the form is still valid");
        assert_eq!(proxy.endpoint(), "http://10.0.0.1:9050");
    }

    #[gpui_kit::test]
    fn an_untouched_port_follows_the_protocol(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = editor(cx, None);
        let (name, host, port) = editor.read_with(cx, |editor, _| {
            (
                editor.name_input(),
                editor.host_input(),
                editor.port_input(),
            )
        });
        set(cx, &name, "Office");
        set(cx, &host, "10.0.0.1");

        switch_to(cx, &editor, ProxyKind::Http);

        assert_eq!(port_value(cx, &port), "8080");
    }

    #[gpui_kit::test]
    fn half_a_credential_is_refused_by_the_form(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = editor(cx, None);
        let (name, host, username) = editor.read_with(cx, |editor, _| {
            (
                editor.name_input(),
                editor.host_input(),
                editor.username_input(),
            )
        });
        set(cx, &name, "Office");
        set(cx, &host, "10.0.0.1");
        set(cx, &username, "user");

        let error = editor
            .read_with(cx, |editor, cx| editor.build_proxy(cx))
            .expect_err("a user name without a password is not valid");
        assert_eq!(
            error,
            ValidationError::IncompleteProxyCredentials.to_string()
        );
    }

    #[gpui_kit::test]
    fn a_port_that_is_not_a_number_is_refused_by_the_form(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = editor(cx, None);
        let (name, host, port) = editor.read_with(cx, |editor, _| {
            (
                editor.name_input(),
                editor.host_input(),
                editor.port_input(),
            )
        });
        set(cx, &name, "Office");
        set(cx, &host, "10.0.0.1");
        set(cx, &port, "ten eighty");

        let error = editor
            .read_with(cx, |editor, cx| editor.build_proxy(cx))
            .expect_err("a port has to be a number");
        assert!(error.contains("port must be"), "{error}");
    }

    /// Every one of the model's six protocols is reachable: two through this
    /// form, four through a share link. Nothing is storable-but-uncreatable.
    #[test]
    fn every_protocol_without_a_form_arrives_by_link() {
        let links = [
            "ss://YWVzLTI1Ni1nY206c2VjcmV0@node.example:8388",
            "trojan://secret@node.example:443",
            "vless://b831381d-6324-4d53-ad4f-8cda48b30811@node.example:443?encryption=none",
            "vmess://eyJhZGQiOiJub2RlLmV4YW1wbGUiLCJwb3J0IjoxMDA4NiwiaWQiOiJiODMxMzgxZC02MzI0LTRkNTMtYWQ0Zi04Y2RhNDhiMzA4MTEifQ==",
        ];
        let mut kinds: Vec<&str> = links
            .iter()
            .map(|link| {
                domain::parse_proxy_uri(link)
                    .expect("a real link")
                    .outbound
                    .kind()
            })
            .collect();
        kinds.sort_unstable();
        assert_eq!(kinds, ["shadowsocks", "trojan", "vless", "vmess"]);
        assert_eq!(ProxyKind::BY_LINK.len(), kinds.len());
        assert_eq!(ProxyKind::ALL.len(), 2);
        assert_eq!(
            ProxyKind::ALL.len() + ProxyKind::BY_LINK.len(),
            6,
            "six protocols to reach, and every one of them has a way in"
        );
    }

    /// Clicks the protocol chip the way the window does.
    fn switch_to(cx: &mut VisualTestContext, editor: &Entity<ProxyEditor>, kind: ProxyKind) {
        let label = ProxyKind::ALL
            .iter()
            .find(|(candidate, _)| *candidate == kind)
            .map(|(_, label)| *label)
            .expect("the kind is offered");
        cx.update(|window, cx| {
            window.click(format!("proxy-kind-{label}"), cx);
        });
        let _ = editor;
    }
}
