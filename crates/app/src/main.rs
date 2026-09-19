use gpui_kit::component::Root;
use gpui_kit::*;

struct AppView {
    title: String,
}

impl AppView {
    pub fn new() -> Self {
        Self {
            title: "Fingerprint Browser v1".to_string(),
        }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xf4f4f5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(rgb(0x27272a))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xa1a1aa))
                            .child("Rust + GPUI Kit + Fingerprint-Chromium"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .child(
                        // Sidebar
                        div()
                            .w(px(240.0))
                            .border_r_1()
                            .border_color(rgb(0x27272a))
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(0x27272a))
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Profiles"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_color(rgb(0xa1a1aa))
                                    .text_sm()
                                    .child("Proxies"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_color(rgb(0xa1a1aa))
                                    .text_sm()
                                    .child("Browser Cores"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .text_color(rgb(0xa1a1aa))
                                    .text_sm()
                                    .child("Settings"),
                            ),
                    )
                    .child(
                        // Main content
                        div()
                            .flex_1()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Browser Profiles"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x71717a))
                                    .child("Personal lightweight fingerprint browser manager."),
                            ),
                    ),
            )
    }
}

fn main() {
    tracing_subscriber::fmt::init();

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::new(px(100.0), px(100.0)),
                        size: Size {
                            width: px(1200.0),
                            height: px(800.0),
                        },
                    })),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Fingerprint Browser".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|_| AppView::new());
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });
}
