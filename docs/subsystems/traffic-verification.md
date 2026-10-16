# Traffic verification

**Status: implemented.** Two related proofs about where a profile's traffic
leaves from. The **pre-flight** - a proxy test - sends one real request through
a proxy's engine - starting a temporary one when nothing is already running -
and reports the address the far end saw, or a fault classified by where the
request stopped. The **read-back** asks a running browser to fetch an address
endpoint in a tab of its own and reads the address back out of the page, so
that "the browser was pointed at the proxy" is shown rather than assumed.
Neither is a fingerprint check: a profile can reproduce every fingerprint claim
and still be leaking, because a fingerprint is what the page is told while the
exit address is what the far end sees.

The pre-flight is `crates/runtime/src/diagnostic.rs` (with
`diagnostic/transport.rs` and `diagnostic/engine.rs`), driven from the app by
`crates/app/src/proxy_tester.rs`. The read-back is `crates/runtime/src/egress.rs`,
driven by the verifier in `crates/app/src/verifier.rs`, which also runs the
fingerprint reading owned by [Fingerprints](fingerprints.md).

## Responsibilities and boundaries

- **Pre-flight** - **Test** on the Proxies page, or the start of a profile with a proxy: does one real request leave through this proxy's engine, and from what address?
- **Read-back** - **Verify** on a running profile with a proxy: does the running browser's own network path reach the address endpoint, and from what address?

The pre-flight is about a *proxy*; the read-back is about a *browser*. They come
apart in exactly the cases that matter - a launch flag that was ignored, a
profile with no proxy, a second proxy between the browser and the engine - and
the symptom is the one this project exists to prevent: a fingerprint that claims
one machine while the traffic leaves from another. The pre-flight cannot prove
the browser was pointed at the engine; the read-back reads one document, in one
tab, once.

Not owned here:

- Fingerprint claims, their comparison and the report's vocabulary - [Fingerprints](fingerprints.md); the switch measurements themselves stay in [Fingerprint matrix](../fingerprint-matrix.md).
- Starting, stopping, crashing and reclaiming the browser and the profile's engine - [Runtime lifecycle](../lifecycle.md) - and the profile, proxy and fingerprint types - [Domain model](../domain-model.md).
- The injected-port contract (`ProxyTester`, `FingerprintVerifier`) that lets the window run both proofs on worker threads - [Service contracts](../service-contracts.md).
- The activity log and the diagnostics report - [Diagnostics](../diagnostics.md) - and which settings are exported (the address endpoint is not) - [Backup and restore](../backup-and-restore.md).

## Source of truth

| Concern | File | Named symbols |
| --- | --- | --- |
| Classification, reading, orchestration and the temporary engine | `crates/runtime/src/diagnostic.rs`, `crates/runtime/src/diagnostic/engine.rs` | `FaultClass`, `Fault`, `Diagnosis`, `EchoClient`, `SocksEchoClient`, `extract_exit_ip`, `classify_engine_log`, `Engine`, `DiagnosticRequest`, `diagnose` (re-exported as `diagnose_proxy`), `TemporaryEngine`, `Drop for TemporaryEngine`; `DEFAULT_ECHO_URL`, `DIAGNOSTIC_CONFIG_FILE`, `DIAGNOSTIC_LOG_FILE` |
| The hand-written SOCKS5 and HTTP conversation | `crates/runtime/src/diagnostic/transport.rs` | `check_echo_url`, `ECHO_PREFIX`, `HttpTarget`, `Bounded`, `handshake`, `request`, `class_for_reply`, `reply_label` |
| The read-back, over CDP | `crates/runtime/src/egress.rs`, `crates/runtime/src/cdp.rs` | `EgressProbe::read`, `EgressOutcome`, `EgressReading`, `verify_egress`, `READING_LIMIT`, `EXCERPT_LIMIT`, `POLL`; `DocumentState`, `HttpCdpProbe::create_page`/`close_page`/`wait_ready`, `CdpSession::document_state`/`evaluate` |
| App-side workers | `crates/app/src/proxy_tester.rs`, `crates/app/src/verifier.rs` | `ProxyTestJob`, `XrayProxyTester`, `PROBE_TIMEOUT`/`ENGINE_READY_TIMEOUT`; `VerificationJob`, `EgressJob`, `VerificationReport`, `CdpFingerprintVerifier`, `EGRESS_TIMEOUT` |
| Window state and the endpoint setting | `crates/app/src/state/proxies.rs`, `crates/app/src/state/profiles.rs`, `crates/app/src/state/rows.rs`, `crates/app/src/settings.rs` | `begin_proxy_test`, `finish_proxy_test`, `forget_proxy_test`, `live_port_for`; `begin_opening`, `egress_job`, `begin_verification`; `ProxyTest`, `ProxyReading`; `Settings::echo_url`, `SettingKey::EchoUrl`, `ECHO_URL_ENV` |

## Main runtime paths

### A proxy test (the pre-flight)

1. `AppState::begin_proxy_test` claims the test slot (a second test of the same
   proxy is refused) and reads the endpoint from the settings. `live_port_for`
   picks the engine: the SOCKS port of a profile running on this proxy means the
   test asks about the live path; `None` means it starts an engine of its own.
2. The window runs `ProxyTester::test` on a worker thread and collects the answer
   a tick later, through the channel the view drains.
3. `XrayProxyTester::test` builds `Engine::Running { socks_port }` or
   `Engine::Temporary { executable, directory, ready_timeout }`, where the
   directory is `runtime_dir/<proxy id>-<process id>-<task id>` so a retest
   after an edit cannot collide with a worker still running. The config builder
   carries the same system-proxy hop a launch uses, so a test that passes is a
   test of what a start would do.
4. `diagnose_proxy` sends one request. `SocksEchoClient` shakes hands with the
   engine's loopback SOCKS5 inbound offering no authentication, puts the
   endpoint's *name* in the CONNECT request for the engine to resolve (what
   Chromium's `--proxy-server=socks5://` does; resolving it here would test this
   machine's resolvers), then issues one plain-HTTP `GET` with
   `Connection: close` and `User-Agent: fp-browser-diagnostic`.
5. Success is a `Diagnosis { exit_ip, elapsed }`, where `extract_exit_ip` takes
   the first token of the answer that parses as an IP address - a bare address,
   a line of prose and a JSON field are all readings. Failure is a
   `Fault { class, detail }`, `detail` being whatever produced it, bounded.
6. `finish_proxy_test` records `ProxyTest::Passed(ProxyReading { exit_ip,
   elapsed, live })` or `ProxyTest::Failed(fault)` for the row, a toast and the
   activity log; `live` says whether the engine probed was already carrying a
   profile's traffic or was started for the test.

**Who starts and stops the engine.** A temporary engine is started by
`TemporaryEngine::start`: it reserves a loopback port, writes
`xray-diagnostic.json` through the same `XrayConfigBuilder` the launch uses (so a
diagnostic cannot pass a proxy the launcher would refuse, or refuse one it would
accept), starts it through `process::spawn_managed` with the plan's own
arguments, and waits for the inbound with `xray::wait_ready`. `Drop for
TemporaryEngine` stops it at the end of the call on every path, including a
failure and a panic: the process tree is terminated (Unix process group; on
Windows the kill-on-close job) and reaped, and both the config and the engine's
log are removed - the config holds the upstream credentials, so it must not
outlive the diagnostic. An `Engine::Running` is only probed and otherwise left
alone: whoever started it stops it, and a diagnostic that killed a running
profile's engine would be a worse bug than the one it is looking for.

### The proxy-gated start

A start or restart of a profile that has a proxy asks the proxy first.
`AppState::begin_opening` takes the profile's lease and then either queues the
command (`StartGate::Queued`, a profile with no proxy has nothing to ask), joins
a test already in flight for the same proxy (`StartGate::Awaiting`, the start is
registered in `pending_starts`), or begins a test of its own
(`StartGate::Checking`, the job is handed to the window). A pass queues the
command; a fault refuses the start, names the profile, the proxy and the fault,
and gives the lease back so Start can be pressed again once the proxy is fixed.
The check is one operation with the start - the profile is busy while it runs -
and it is called off rather than left pending when Stop is pressed, when the
proxy is edited or deleted, or when no answer has come within `CHECK_LEASE`
(60 s).

### The read-back

1. On **Verify**, `begin_verification` assembles a `VerificationJob`.
   `egress_job` is `Some` only for a profile with a proxy: `echo_url` is the
   endpoint the settings show, and `expected` is the reading of the proxy's
   most recent test (`ProxyTest::exit_ip`) - nothing when the proxy was never
   tested in this session, when that test failed, or while it is still running.
2. The worker runs `CdpFingerprintVerifier::verify`. The fingerprint question is
   settled first (`FingerprintProbe::read_fresh`); a browser that cannot be read
   at all fails there, never as a path problem.
3. `EgressProbe::read` puts the address question to the browser:
   - `check_echo_url` refuses an endpoint that is not plain http before anything
     is dialled; `HttpCdpProbe::wait_ready` and `create_page` bound the CDP
     side, and a browser that cannot be asked - no CDP endpoint, or it refused -
     is `NotAsked`.
   - The probe opens a page of its own at the endpoint and connects a session to
     it, then polls `DocumentState` every `POLL` (25 ms). When the page holds a
     finished `http:`/`https:` document it reads the text, bounded to
     `READING_LIMIT` (4 KiB), and takes the address from it.
   - The wait ends as soon as the page has settled - an answer was read, or the
     page committed elsewhere and finished loading - rather than at the deadline,
     and the tab is closed again on every path, best effort.
4. `verify_egress(expected, &outcome)` turns the outcome into discrepancies. The
   report carries the address (`exit_ip`) or the reason it was not read
   (`exit_unreadable`), shown by `VerificationReport::exit_label`.

## State and transitions

**A proxy test's result.** No entry, then `ProxyTest::Running`, then
`ProxyTest::Passed(ProxyReading)` or `ProxyTest::Failed(Fault)`. Editing or
deleting the proxy clears the result (`forget_proxy_test`) - the old reading is
a claim about the old upstream - and a failed test that a waiting start depended
on frees that profile's lease.

**The engine a test chooses.** A profile running on the proxy means the test
probes that profile's engine and records `live: true`; nothing running means a
temporary engine is started and `live: false`. One is what is happening to a
profile's traffic, the other a rehearsal, and the row's detail says which.

**The expectation.** `expected` for a read-back is the reading of the proxy's
most recent test. It is absent when the proxy has not been tested in this
session, when that test failed, or while it is still running; a pass sets it to
the address the test saw. Editing or deleting the proxy clears it, and so does
a later test that ends in a fault - a fault replaces the earlier pass rather
than leaving the old reading in place.

**Where the results live.** The proxy results (`proxy_tests`) and the
verifications (`verifications`) are plain in-memory maps on `AppState`: nothing
writes them to storage, so a restart leaves none behind. A configuration
restore clears both outright (`AppState::finish_restore`), so the expected
address and every past reading disappear that way too, not only through an edit
or a delete.

**Pre-flight outcomes.** Every way the pre-flight can end is either a reading
(`Diagnosis`) or a `Fault` of one of: `Engine` (the engine could not be started
or never came up, or no loopback SOCKS endpoint accepted the connection),
`Config` (the configuration, the request or the endpoint is unusable), `Auth`
(the far end refused the credentials), `Dns` (the name could not be resolved),
`Unreachable` (nothing carried the connection), `Timeout`, `Tls`, `Http(u16)`
(the endpoint answered with a status that carries no reading, except 401/407
which are `Auth`), `Reading` (the endpoint answered without an address), and
`Other`, which carries the engine's own words instead of guessing a class.

**Read-back outcomes.** `EgressOutcome` has six variants, kept apart because
they are fixed in different places:

| Variant | Meaning |
| --- | --- |
| `Read(reading)` | The endpoint named an address; `reading.exit_ip` is it and `reading.url` is the endpoint it was read from |
| `NotReached { url, state }` | The page never committed to the endpoint - nothing left by this route |
| `Timeout { url, state }` | The page was at the endpoint and had not finished loading - a slow path, not a path that carries nothing |
| `Unreadable { url, excerpt }` | The endpoint answered and the answer held no address |
| `Unusable(reason)` | The endpoint is not one that can be asked (not plain http) |
| `NotAsked(reason)` | The browser could not be asked at all |

`NotReached` and `Timeout` are read from the page's state rather than inferred
from having waited: a page that committed somewhere other than the endpoint
(for example the browser's own error page) has finished, so it is reported as
not reached immediately and waiting longer cannot change it. The state carried
in both is the probe's own `readyState|protocol|document?` answer. A
verification result is cleared when the profile stops, restarts, is edited or
deleted: a new browser has a new fingerprint and a new path.

## Constraints and failure behavior

**The address endpoint must be plain http.** `check_echo_url` refuses
`https://` rather than downgrading it, with a `FaultClass::Config` fault saying
why: a TLS request would let a certificate problem be reported as a path
problem, which is the most expensive wrong answer to give someone debugging a
proxy. The pre-flight and the read-back both call the same check, so both give
the same answer to the same setting. The endpoint is a setting the user owns,
read for every test, because it sees the exit address; `DEFAULT_ECHO_URL` is
`http://api.ipify.org`. A plain-http endpoint may redirect, and the read-back
accepts a document that arrived over `https:` as an arrival - the endpoint's own
scheme is what is constrained, not where the browser ended up.

**Readiness is not carrying.** `xray::wait_ready` only proves the engine bound
its loopback inbound (the definition and its bounds are owned by
[Proxies and Xray](proxies-xray.md)). A port that accepts a connection and then says nothing
looks exactly like a working proxy, and the difference is whether the profile
leaks - which is why the pre-flight sends a real request instead.

**Every wait is bounded.** `PROBE_TIMEOUT` (10 s) bounds one request and
`ENGINE_READY_TIMEOUT` (5 s, the launch path's own budget) bounds a temporary
engine's readiness. Every read and write shares one absolute deadline
(`Bounded`): partial progress does not renew it. Headers are bounded to 8 KiB
and the reading to 4 KiB, and a declared `Content-Length` is preferred over
waiting for the close, because an endpoint that keeps the connection open would
turn a good address into a timeout. The SOCKS5 conversation is written out
rather than delegated because no timeout in a general-purpose client survived a
peer that accepts a connection and goes silent, and because the protocol's
reply codes are an exact account where a library's error strings would be a
guess.

**The engine's log names the cause the transport cannot.** An upstream
rejection does not fail the handshake to the local inbound, so the socket
failure alone cannot say whether the credential was refused or the name did not
resolve. After a transport failure - never instead of it - the diagnostic reads
the temporary engine's log and classifies its lines: the first marker that
matches wins (`Config`, `Auth`, `Dns`, `Unreachable`, `Timeout`, `Tls`), and a
line nobody has seen before is carried unclassified rather than mapped onto a
class it may not belong to. Only a temporary engine has a log in the
diagnostic's directory; a live engine's failure is reported from the socket
alone. `Dns` can only come from the log, because the SOCKS protocol has no
reply code for it; the reply codes map from the protocol itself (0x02-0x05
`Unreachable`, 0x06 `Timeout`, 0x07/0x08 `Config`, anything else `Other`), with
the protocol's own label in the detail.

**What a successful pre-flight does not prove.** That the browser was pointed at
the engine, that any page the user opens takes the same path, or that the
address is the one the profile was configured for - a profile is configured
with a proxy, not with an address. The tested address is the only expectation
the product holds for where a profile's traffic should leave from, and it is
what the read-back is compared against. And the read-back proves little more:
it reads one document, in one tab, once, and nothing about the profile's
configuration beyond the address the proxy was tested at.

**The read-back disturbs nothing.** Like the fingerprint probe, it opens a tab
of its own instead of navigating the page the user is looking at, and closes it
afterwards; only loopback CDP debugger URLs are dialled, because CDP hands out
code execution, and the expression that reads the answer asks the page for
nothing new.

**When there is a finding and when there is not.** `verify_egress` judges only
against a tested proxy: with `expected` absent, a reading is information rather
than a finding, because there is no tested address the reading could
contradict. A reading that matches the tested address exactly is not a
discrepancy; the comparison is made before the expectation is worded, because
the wording ("... where the proxy was tested") is part of the message. Anything
else - a different address, or an outcome that is not a reading at all - is one
`Discrepancy` on the claim `exit address`, and a browser whose traffic did not
come out of the tested proxy at all is a finding at least as strong as the
wrong address: the tested proxy carried a request and this browser's traffic
did not come out of it.

## Verification

The read-back's orchestration is covered without a browser: the stub in
`crates/runtime/src/egress.rs` speaks both protocols on one port, as a browser
does, so readiness, page creation, polling, reading and closing all run under
test; only a real browser's behaviour remains for the opt-in suites.

- `cargo test -p runtime egress` - the read-back against the stub:
  `a_read_back_opens_a_page_of_its_own_at_the_endpoint_and_closes_it`,
  `a_page_that_committed_elsewhere_ends_the_wait_without_a_deadline`,
  `an_endpoint_that_is_not_plain_http_is_refused_before_anything_is_asked`,
  `a_browser_that_cannot_be_asked_is_not_a_path_that_carried_nothing`, and the
  comparison rules: `a_different_address_is_reported_against_the_tested_one`,
  `nothing_is_claimed_about_a_profile_whose_proxy_was_never_tested` and
  `not_reaching_the_endpoint_is_a_finding_against_a_tested_proxy`.
- `cargo test -p runtime diagnostic` - the transport and the classification:
  `an_open_port_that_carries_nothing_is_a_fault_and_not_a_pass`,
  `an_endpoint_that_never_answers_is_a_timeout_and_not_a_hang`,
  `partial_reads_share_one_deadline` and
  `a_proxy_that_demands_a_credential_is_not_reported_as_a_dead_network`,
  `a_running_engine_is_probed_and_left_alone`,
  `the_engine_log_names_the_cause_a_transport_failure_cannot`, and - on Unix,
  against a stub engine process -
  `engine_lifetime::an_engine_that_binds_but_carries_nothing_is_bounded_and_leaves_nothing`.
- `cargo test -p app proxy_tester`, `cargo test -p app verifier` - the workers'
  failure paths without a proxy or a browser:
  `a_proxy_that_leads_nowhere_reports_a_fault_not_a_reading`,
  `a_browser_that_cannot_be_read_is_not_reported_as_a_path_problem`.
- `cargo test -p app state::tests::proxies` - the expectation and the gate:
  `a_proxied_profile_is_asked_about_the_address_its_proxy_was_tested_at`,
  `an_untested_proxy_leaves_nothing_to_disagree_with`,
  `a_profile_without_a_proxy_is_not_asked_about_an_address` and
  `opening_a_proxied_profile_is_refused_when_its_proxy_has_no_traffic`.
- `cargo test -p app state::tests::verification` -
  `a_confirmed_reading_is_logged_with_the_address_it_left_from`.
- `cargo test -p app ui` - the headless window tests, including
  `ui::tests::verification::the_address_a_verified_profile_left_from_is_shown_on_the_profile`.

Opt-in suites, each needing a binary and a network:

- `XRAY_BIN=... DIAG_SOCKS5='host:port[:user:password]' DIAG_ECHO_URL=http://... cargo test -p runtime --test proxy_diag_real -- --ignored`
  (`PROXY_URI` takes a share link) - a real engine answers the hand-written
  conversation: `a_real_engine_reports_where_a_real_request_left_from` and
  `an_upstream_that_cannot_reach_the_target_is_a_fault_not_a_reading`.
- `CHROMIUM_BIN=... ECHO_URL=http://api.ipify.org cargo test -p app real_chromium_reports_the_address -- --ignored`
  - on Linux only, since `crates/app/src/real_browser.rs` is compiled under
  `#[cfg(all(test, target_os = "linux"))]` - a real browser's own path is read
  back without disturbing it:
  `real_chromium_reports_the_address_its_own_traffic_leaves_from`.
