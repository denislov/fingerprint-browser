//! Opt-in: the identity page as a real browser renders it.
//!
//! The service is otherwise proved over a socket, which says nothing about
//! whether a browser will render what it serves: the JavaScript, the content
//! security policy and the escaping of a profile's name are only exercised by
//! something that runs them.
//!
//! ```sh
//! CHROMIUM_BIN=/path/to/chrome cargo test -p runtime --test status_page_real -- --ignored
//! ```
//!
//! The browser is started behind a proxy that nothing listens on, which is the
//! question this file exists for as much as the rendering is: **does a profile
//! with a proxy reach its own page?** Chromium bypasses the proxy for loopback
//! addresses unless `--proxy-bypass-list=<-loopback>` takes that rule away.
//!
//! Measured, rather than inferred from the token existing: on fingerprint-chromium
//! 148 the same page behind the same dead proxy loads by default and does **not**
//! load with `<-loopback>` added. If that default ever changed, a proxied profile
//! would be sent to its identity page through an engine that cannot reach it, and
//! every profile with a proxy would open on an error page. The repo's own
//! `chromium_real` planner is one of the callers that removes the rule, which is
//! why that suite's browsers do not reach a page - a property of that harness,
//! not of a launch, which never sets the list.

#![cfg(any(target_os = "linux", windows))]

use runtime::status::{StatusReport, StatusService, StatusSession};
use runtime::{PortAllocator, TcpPortAllocator};
use std::io::Read as _;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a browser gets to render one page and dump it.
const RENDER_DEADLINE: Duration = Duration::from_secs(60);

#[test]
#[ignore = "requires CHROMIUM_BIN pointing at a real Chromium build"]
fn a_real_browser_renders_the_page_behind_a_proxy_that_cannot_carry_it() {
    let browser = std::env::var("CHROMIUM_BIN").expect("set CHROMIUM_BIN");

    // Port 1 on loopback: nothing listens, so the reading is a fault this test
    // can expect quickly, and no request leaves the machine.
    let session = Arc::new(StatusSession {
        report: StatusReport {
            profile: "shop-1".to_string(),
            profile_id: "8f1c".to_string(),
            core: "148.0.7778.215".to_string(),
            core_major: Some(148),
            proxy: Some("tokyo".to_string()),
            cdp_port: 9222,
            socks_port: Some(1),
            started_at: 1_700_000_000,
            warning: Some("--disable-spoofing is not honoured by this core".to_string()),
            echo_url: "http://127.0.0.1:1/".to_string(),
        },
        socks_port: Some(1),
        lang: "en",
    });
    let service = StatusService::start(
        TcpPortAllocator
            .reserve_loopback()
            .expect("a loopback port"),
        session,
    )
    .expect("the page starts");

    let profile_dir = std::env::temp_dir().join(format!("fp-status-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile_dir);

    let dom = render(
        &browser,
        &[
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--disable-sync",
            "--no-default-browser-check",
            // A proxy that carries nothing. If loopback were not bypassed, the
            // page itself could not load and this test would fail on an error
            // page rather than on the assertions below.
            "--proxy-server=socks5://127.0.0.1:1",
            // Long enough for the page's two reads, then dump.
            "--virtual-time-budget=10000",
            "--dump-dom",
            &format!("--user-data-dir={}", profile_dir.display()),
            &service.url(),
        ],
    );

    let _ = std::fs::remove_dir_all(&profile_dir);

    // The page rendered at all: a browser that could not reach it reports the
    // error in the DOM rather than in an exit status, and an empty dump is a
    // browser that never answered.
    assert!(
        !dom.contains("ERR_") && !dom.is_empty(),
        "the browser did not render the page: {dom}"
    );

    // Every field is read from the element JavaScript wrote it into, rather than
    // from the document as a whole: the page's own script is part of the DOM, so
    // a message that only ever lives in the source would match a search over all
    // of it whether or not anything rendered.
    assert_eq!(field(&dom, "profile"), "shop-1 (8f1c)");
    assert_eq!(field(&dom, "core"), "148.0.7778.215 · 148");
    assert_eq!(field(&dom, "proxy"), "tokyo");
    assert_eq!(field(&dom, "socks"), "1");
    assert!(
        field(&dom, "started") != "…",
        "the start time never landed: {dom}"
    );
    assert!(
        field(&dom, "warning").contains("not honoured"),
        "the compatibility warning was swallowed: {dom}"
    );
    // The row the warning lives in is hidden until something fills it.
    assert!(
        !dom.contains(r#"id="warning-row" hidden"#),
        "the notice stayed folded away: {dom}"
    );

    // The reading came back - a fault, because nothing is listening on port 1 -
    // and it replaced the placeholder rather than leaving it there.
    let egress = field(&dom, "egress");
    assert_ne!(egress, "…", "the reading never landed: {dom}");
    assert!(egress.contains("127.0.0.1:1"), "{egress}");
    assert_eq!(field(&dom, "endpoint"), "http://127.0.0.1:1/");

    // Nothing of the proxy's own configuration reaches the page.
    assert!(!dom.contains("socks5://"), "{dom}");
}

/// The rendered content of one `<dd id="…">`, or the whole document when there
/// is no such element - so a missing element fails the assertion that reads it
/// rather than passing an empty string.
fn field(document: &str, id: &str) -> String {
    let opener = format!("<dd id=\"{id}\"");
    let Some(start) = document.find(&opener) else {
        return document.to_string();
    };
    let rest = &document[start..];
    let Some(open_end) = rest.find('>') else {
        return document.to_string();
    };
    let content = &rest[open_end + 1..];
    match content.find("</dd>") {
        Some(end) => content[..end].trim().to_string(),
        None => document.to_string(),
    }
}

/// Renders one URL and returns the document, or what was there when the
/// deadline passed - a browser that hangs is a failure, not a wait forever.
fn render(browser: &str, args: &[&str]) -> String {
    let mut child = Command::new(browser)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the browser starts");

    let mut stdout = child.stdout.take().expect("a stdout pipe");
    let reader = std::thread::spawn(move || {
        let mut document = String::new();
        let _ = stdout.read_to_string(&mut document);
        document
    });

    let until = Instant::now() + RENDER_DEADLINE;
    loop {
        match child.try_wait().expect("the browser is waitable") {
            Some(_) => break,
            None if Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    // Killing the browser closes the pipe, so this returns either way.
    reader.join().unwrap_or_default()
}
