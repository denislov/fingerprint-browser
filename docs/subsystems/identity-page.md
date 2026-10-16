# Identity page

**Status: implemented.** A profile whose launch asks for it opens a local page
first, served by this program on a loopback port, which says what the session is
and where one request leaves from. It is the standing version of the question
the window's **Verify** action answers once: *which profile am I, right now, and
from what address does my traffic leave?*

The page belongs to a session, not to the window. It is served for as long as
the profile runs and ends with it, so it survives the window being closed while
browsers keep running, and it is not re-opened for a session this run adopted
from a previous one.

## Responsibilities and boundaries

What this subsystem owns:

- The listening socket and the HTTP conversation: one request per connection,
  `GET` only, three paths, and the answer's headers - `crates/runtime/src/status.rs`
  (`StatusService`, `serve`, `Request`, `respond`).
- The three things a request may **not** do, checked before anything is served:
  it may not name another `Host`, it may not carry a query string, and it may not
  be a method other than `GET`. See [The three refusals](#the-three-refusals).
- The protocol of what the page shows: `StatusReport` (the session, as JSON),
  `StatusEgress` (one reading, as JSON), and `StatusSession` (what the service is
  answering about).
- The page itself, as HTML with its own small catalog of messages in the two
  languages (`PAGE`, `STRINGS`, `render_page`).
- The port: reserved through `PortAllocator` and **taken** rather than released
  (`PortReservation::into_listener`), so the address the browser is sent to is one
  already being answered on.

It does not own:

- The reading's meaning, its fault vocabulary, or the difference between the
  engine's exit address and the browser's - [Traffic verification](traffic-verification.md).
  The page commissions one reading; it does not classify it.
- What the profile, core and proxy are - [Domain model](../domain-model.md).
- Starting, stopping and crashing the session the page belongs to, and what
  reclaim and adoption do to it - [Runtime lifecycle](../lifecycle.md).
- The `StartParams` field the launch carries and the planner argument it becomes
  - [Service contracts](../service-contracts.md).
- The window's switch and its row - [Settings](settings.md), [UI shell](ui-shell.md).

## Source of truth

| Concern | File | Named symbols |
| --- | --- | --- |
| The service, its HTTP framing and its refusals | `crates/runtime/src/status.rs` | `StatusService`, `start`, `start_with`, `serve`, `read_head`, `Request`, `Request::host_names`, `respond`, `Status`, `Body` |
| What the page shows | `crates/runtime/src/status.rs` | `StatusReport`, `StatusEgress`, `StatusSession`, `StatusSession::egress`, `LANGS` |
| The page's text and markup | `crates/runtime/src/status.rs` | `PAGE`, `STRINGS`, `render_page`, `escape`, `CSP` |
| The deadlines | `crates/runtime/src/status.rs` | `REQUEST_DEADLINE`, `EGRESS_TIMEOUT`, `MAX_REQUEST_HEAD`, `POLL` |
| Where the launch starts it | `crates/runtime/src/supervisor/start.rs` | `begin_attempt` (step 3c) |
| Who owns it while it runs | `crates/runtime/src/supervisor/ownership.rs` | `StartAttempt::hold_status`/`take_status`, `ActiveSession::_status` |
| The argument it becomes | `crates/runtime/src/planner.rs` | `LaunchContext::status_url` |
| Taking the reserved port | `crates/runtime/src/ports.rs` | `PortReservation::into_listener` |
| What a launch asks for | `crates/runtime/src/events.rs` | `StartParams::status`, `StartParams::with_status`, `StatusLaunch` |
| Where a launch is built | `crates/application/src/runtime_service.rs` | `RuntimeService::start`, `RuntimeService::restart`, `launch_params` |
| The switch | `crates/app/src/settings.rs` | `SettingKey::StatusPage`, `Settings::status_page`, `Settings::status_launch`, `Settings::set_status_page`, `STATUS_PAGE_OFF` |
| The switch, resolved | `crates/app/src/settings/resolution.rs` | `resolve_status_page` |
| Where Start passes it | `crates/app/src/state/profiles.rs`, `crates/app/src/state/mod.rs` | `queue_opening`, `AppState::status_launch` |
| The row and its click | `crates/app/src/ui/settings.rs`, `crates/app/src/ui/settings/actions.rs` | `switch_control`, `AppView::on_set_switch` |

## Main runtime paths

### A start that asks for the page

1. `AppState::queue_opening` reads `Settings::status_launch()` at the moment
   Start is pressed. The value carries the configured endpoint and the language
   in force, both from the same read - a page that answered in the language of the
   previous start would be wrong in a way nobody would check.
2. `RuntimeService::launch_params` puts it on `StartParams`. Nothing about it is
   stored against the profile.
3. `begin_attempt` reserves a loopback port, builds the `StatusReport` from what
   the launch already knows - the profile's name and identifier, the core's
   version and major, the **proxy's name**, the two ports, the start instant, and
   the compatibility warning when there is one - and starts the service, which
   holds the socket the reservation was holding.
4. The service's address becomes `LaunchContext::status_url`. The browser is sent
   to it as its first argument, ahead of the profile's own start page.
5. The service lives in the `StartAttempt` until the start succeeds and moves into
   the `ActiveSession`; on any path out of a failed start it is dropped, which
   stops its thread and frees the port.
6. `GET /` renders the page in the session's language. `GET /api/status` answers
   the report. `GET /api/egress` sends one request through the profile's engine
   and answers with the address, or the fault class where it stopped, or that
   nothing was asked because the profile has no proxy.

### The page's own two requests

The page is one HTML document that reads the two JSON paths on load, and it has a
button that re-reads the second. It asks for nothing else, and it cannot: the
content security policy it is served under is `default-src 'none'` with
`connect-src 'self'`, so a later edit to the page cannot quietly start asking the
network a question the reader did not ask for. That rule exists because the
endpoint is the reader's setting: whether it sends CORS headers is not something
this program can promise, so a page that fetched it directly would show a reading
that disappears exactly when it is being looked for.

### A stop, a crash and a reclaim

All three drop the `ActiveSession`, and that drop is the whole of it: the service
stops its thread and closes the socket. Nothing is written down about the page
anywhere, so there is no state to reconcile on the next start.

An **adopted** session has no page: the previous run's service died with that
run, and the tab it left in the browser is a page that now answers nothing.
Opening one again would mean navigating a browser this run did not start.

## The three refusals

| Refusal | Answer | Why |
| --- | --- | --- |
| The `Host` header names something else | `403` | A page on any origin can make the reader's browser send a request to `127.0.0.1`; what it cannot do is make the browser lie about the name it is asking for. Checked before routing, so such a request learns nothing - not even which paths exist. |
| The target carries a query string | `400` | The endpoint is fixed by the launch. A page that could name one here would have turned the reader's own machine into a proxy carrying their profile's credentials. |
| Anything but `GET` | `405` | There is no endpoint that changes anything, so there is nothing for another method to do. |

The query-string refusal is the one where the guarantee does not rest on the
check itself: routing is an exact match, so `GET /api/egress?url=...` reaches no
handler and is a `404` even with the check removed - measured, by removing it.
The check is kept and a test says which answer a caller gets, because what makes
the accident safe is the shape of a `match` three lines down, and a later change
from an exact match to a prefix would take the guarantee with it.

Two more things are true of every answer, including the refusals: nothing the
request carried is echoed back, and the proxy appears only as its **name** - never
its URI, never its credentials. This is a page a reader may leave open on a
second monitor.

## What each number proves

| The page says | What it is | What it is not |
| --- | --- | --- |
| Where one request leaves from | One request through the port the profile's engine listens on, to the configured endpoint - the same reading the Proxies page's **Test** takes | Not a proof that the browser was pointed at the engine |
| The session's identity | What this launch was started as: the profile's name, the core's version, the proxy's name | Not a re-read of the database, so it cannot disagree with the window about a profile edited while its browser runs |

The second question - whether the browser's *own* traffic takes that path - is
what the window's **Verify** reads back out of the browser, and it is not on this
page. A profile with no proxy is told that nothing was asked rather than given a
number it did not earn, which is the rule the rest of the program applies.

## Constraints and failure behavior

- **Loopback only.** The listener is a `127.0.0.1` socket from `PortAllocator`;
  the service never binds anything else.
- **One connection at a time.** The work behind a request is a page render and,
  at most, one outbound request with its own deadline. A thread per connection
  would be an unbounded number of them, handed to whoever can open sockets
  fastest; serial handling means a slow reader delays the next one instead.
- **Every read and write carries what is left of one absolute deadline**
  (`REQUEST_DEADLINE`), never a fresh timeout - so a peer that drips a byte at a
  time cannot hold the service open by making progress. A client that connects
  and says nothing is timed out and does not hold the port.
- **A request head longer than 8 KiB is a `431`**, and the service keeps serving.
- **The accept loop does not end on a transient error.** A failed `accept` is
  logged and retried: a listener this program owns cannot be closed underneath
  it, and ending the loop would take the page away silently.
- **The reading is bounded by `EGRESS_TIMEOUT`**, and a profile with no proxy is
  answered without dialling anything.
- **The page works offline.** Every number comes over loopback, so a machine with
  no route to the endpoint shows the endpoint's fault rather than a blank page.
- **A proxied profile still reaches its own page, because Chromium bypasses the
  proxy for loopback by default.** This is load-bearing rather than incidental: a
  launch that added `--proxy-bypass-list=<-loopback>` - the flag that takes that
  default away, which `chromium_real`'s test planner does - would send the browser
  to its identity page through an engine that cannot reach it, and every proxied
  profile would open on an error page. Measured on fingerprint-chromium 148: the
  same page behind the same dead proxy loads by default and does not load with
  `<-loopback>` added (`crates/runtime/tests/status_page_real.rs`).

## Verification

The gate covers the service without a browser or a network:

```sh
cargo test -p runtime --lib status::        # the service, its refusals and the page
cargo test -p runtime --lib a_session_serves # served for the session, gone with it
cargo test -p runtime --lib the_identity_page # the argument order
cargo test -p app --bin fp-browser settings::tests::the_identity_page
cargo test -p app --bin fp-browser a_launch_asks_for_the_identity_page
cargo test -p app --bin fp-browser the_identity_page_row
```

The checks named for a guarantee were proved red by hand before being trusted:
with the `Host` check removed, a request naming `evil.example` is answered with
the whole report; with the service left out of the `ActiveSession`, the address
the browser was sent to has nothing listening; with the switch's click stored
without looking at its row, the identity-page row keeps the state it had while
the other one moves.

The page itself is rendered by a real browser in an opt-in suite, because a
socket read cannot say whether a browser will run the script, honour the policy
or escape a profile's name:

```sh
CHROMIUM_BIN=/path/to/chrome cargo test -p runtime --test status_page_real -- --ignored
```

Run against fingerprint-chromium 148 on Linux: the page loaded behind a dead
proxy, every field was filled in by the page's own script - including the
compatibility warning and the fault the reading came back as - and the same page
did not load once `--proxy-bypass-list=<-loopback>` was added, which is how the
loopback-bypass constraint above was measured rather than assumed. That run also
found a real defect: the two separator characters were written into the script
with a doubled backslash, so the page showed `·` where the middle dot
belongs; the escapes are the characters now, and the test reads the element
rather than the document, which is what let it see the difference.

Not run here: the `chromium_real`, `fingerprint_real` and `proxy_diag_real`
suites, and no real window has opened the page - the browser was driven headless.
