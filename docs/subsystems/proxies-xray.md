# Proxies and Xray

A proxy is a stored upstream endpoint the user configures once and assigns to
profiles. Xray is the engine that carries a profile's traffic to it. A browser
never speaks Shadowsocks, VMess, VLESS or Trojan itself, so for every start of
a profile that has a proxy, the runtime writes a temporary Xray config with a
loopback SOCKS inbound, starts one Xray process with it, and points the browser
at that inbound with `--proxy-server=socks5://127.0.0.1:<port>`. This page
covers the outbound model's rules, the share links that produce it, config
generation, the engine's one-per-profile lifetime and readiness, and the
machine's own proxy. Design rationale: [architecture](../architecture.md)
section 10; builder contract: [service contracts](../service-contracts.md)
section 5.

## Responsibilities and boundaries

The subsystem spans `crates/domain` (the record, `validate_proxy`, the
share-link parsers and the credential-stripping rule), `crates/runtime` (config
generation, reading the machine's own proxy, and the engine's start, readiness,
stop, rollback and reclaim), `crates/app` (the form for address-and-credentials
protocols and the paste dialog for links), and `crates/application` plus the
schema (the rules that keep an assigned proxy from being deleted).

Not here: the model and field meanings are owned by
[domain-model](../domain-model.md) section 8, the start/stop/restart state
machine by [lifecycle](../lifecycle.md), and backup, import and restore of
proxy records by [backup and restore](../backup-and-restore.md). Whether
traffic *actually leaves* through the proxy — the diagnostic and the
browser-side read-back — is a separate subsystem, [traffic
verification](traffic-verification.md): this page covers what is generated and
run, not how its output is judged. Where the engine binary comes from is
[Browser cores](browser-cores.md); the diagnostics report and activity log are
[diagnostics](../diagnostics.md).

## Source of truth

| Concern | Source |
| --- | --- |
| Model, kinds, stream settings, credential stripping | `crates/domain/src/proxy.rs` (`ProxyProfile`, `ProxyOutbound`, `StreamSettings`) |
| Validation rules | `crates/domain/src/validation.rs` (`validate_proxy`, `validate_stream`) |
| Share links | `crates/domain/src/uri.rs` and `crates/domain/src/uri/` (`parse_proxy_uri`, `ParsedProxy`, `UriError`, `Transport`) |
| Config generation | `crates/runtime/src/xray.rs` (`XrayConfigBuilder`, `DefaultXrayConfigBuilder`, `SYSTEM_PROXY_TAG`, `XRAY_CONFIG_FILE`, `wait_ready`, `stream_json`) |
| The machine's own proxy | `crates/runtime/src/system_proxy.rs` (`detect`, `Reading`, `SystemProxy`, `Protocol`) |
| Planning and engine lifetime | `crates/domain/src/plan.rs` (`XrayLaunchPlan::args`), `crates/runtime/src/planner.rs`, `crates/runtime/src/supervisor.rs`, `crates/runtime/src/supervisor/start.rs`, `crates/runtime/src/supervisor/ownership.rs`, `crates/runtime/src/journal.rs` |
| Application rules | `crates/application/src/proxy_service.rs` (`ProxyService`, `DefaultProxyService`), `crates/application/src/runtime_service.rs` (`launch_params`) |
| Window paths | `crates/app/src/proxy_editor.rs` (`ProxyEditor`, `ProxyKind`), `crates/app/src/proxy_import.rs` (`ProxyImport`), `crates/app/src/state/proxies.rs` (`AppState::create_proxy`), `crates/app/src/ui/proxies.rs` |
| Deletion protection in the schema | `crates/storage/src/migrations.rs`, migration `002_proxy_reference_is_restrictive` |

## Main runtime paths

### A proxy record is created

The form `ProxyEditor` fills in the two kinds that are an address and a pair of
credentials: `ProxyKind::ALL` is SOCKS5 (port default 1080) and HTTP (port
default 8080), and `ProxyEditor::build_outbound` runs `validate_proxy` before
it returns, so the form refuses a save for the same reasons the service would.
The paste dialog `ProxyImport` fills in the other four:
`ProxyImport::read_link` calls `parse_proxy_uri` and returns the link's own
remark (or `kind host`) as the name; the view hands the result to
`AppState::create_proxy`, which calls `ProxyService::create`. A refusal is the
parser's or validator's own sentence, and the dialog stays open.

`ProxyService` applies the same `validate_proxy` on `create`, `update` and
`insert`; `update` refuses an identifier that is not stored
(`AppError::NotFound`) and `insert` is an upsert for import, because the
identifier is what a profile's assignment travels by and re-importing must be
able to finish. `delete` refuses while any profile still points at the proxy,
naming them: silently clearing the assignment would send that profile's traffic
direct. `list` sorts by lowercased name; `usage` maps each proxy to the sorted
names of its profiles.

### A share link becomes an outbound

`parse_proxy_uri` recognises exactly four schemes, matched case-insensitively:
`ss://`, `vmess://`, `vless://` and `trojan://`. Anything else — including
`socks5://` and `http://` — is refused with `UriError::UnsupportedScheme`,
whose message names the four schemes. A plain `socks5://host:port` is not a
share link here: it carries no field the form cannot already ask for, and the
two address-and-credentials kinds have a form, while the four link-only
protocols carry a public key, short id, uTLS fingerprint or websocket path
(`ProxyKind::BY_LINK` names them on the form). The same `socks5://` spelling
*is* read, by a different parser, when it names the machine's own proxy — see
below.

Each scheme has its own shape. `ss://` arrives as base64 credentials before the
`@`, as the whole `method:password@host:port` base64-encoded, or with the
credentials in the clear (`uri::shadowsocks::parse_ss`). `vmess://` is base64
of a JSON object (`uri::vmess::parse_vmess`) whose remark is `ps`, whose cipher
comes from `scy` (or `security`) and whose `aid` is carried because the engine
reads it. `vless://` and `trojan://` are URL-shaped
(`uri::url_link::parse_url_link`), `scheme://secret@host:port?params#remark`;
VLESS's `encryption` defaults to `none`, and Trojan defaults to TLS because
that is what deployments use, but a link that says `security=none` is honoured.

`uri::transport::Transport` maps query parameters and vmess field names onto
`StreamSettings`: `type`/`network` is tcp/ws/grpc, `security` is
none/tls/reality, `sni`/`fp`/`alpn` fill `TlsSettings`, `pbk`/`sid`/`spx` fill
`RealitySettings`, `path`/`host` fill `WsSettings`, and `serviceName` fills
`GrpcSettings`. Two rules keep the parser faithful rather than permissive. A
field it recognises but the model cannot carry is refused **by name**
(`Transport::refuse_what_this_program_cannot_carry`, `Transport::stream`):
`plugin`, a `headerType` other than none, a `mode` other than `gun`, a network
or security it does not model, a `tls` field under `security=none`, REALITY key
material without `security=reality`, a websocket `path` under tcp. A query
parameter it has never seen (`utm_source` and the like) is passed over instead,
because refusing working links over another client's parameter is worse. After
parsing, the stream is run through the same `validate_stream` the editor uses,
so a link whose stream parses carries stream settings the editor would also
accept (`UriError::WouldNotLaunch` reports the refusal). The rest of the
outbound is checked later: `validate_proxy` runs when the import is stored, and
it can still refuse a link the parser accepted — a whitespace-only Shadowsocks
or Trojan password passes the parser (which only refuses an empty credential)
but is refused by the validator.

### A start turns the proxy into a config and an engine

`RuntimeService::launch_params` resolves the profile, its core and its proxy
record (a missing proxy refuses the command before the runtime hears about it)
and sends `StartParams` through `RuntimeFacade`. In the supervisor:

1. `begin_attempt` reserves a CDP port, and a second loopback port for SOCKS
   when the profile has a proxy (`PortAllocator::reserve_loopback`).
2. `DefaultLaunchPlanner::build` refuses an inconsistent plan (proxy present
   without a socks port or vice versa, a mismatched `proxy_id`, port zero, the
   two ports equal) and adds `--proxy-server=socks5://127.0.0.1:<socks_port>`
   to the browser arguments; `XrayLaunchPlan` carries the engine executable,
   the config path and the socks port, and `XrayLaunchPlan::args` is
   `run -config <path>` — one source for the spawn and for the session record
   that must recognise the process later.
3. `XrayConfigBuilder::build` writes the config (next paragraph); the SOCKS
   reservation is released just before the child is spawned so Xray can bind
   the port.
4. Xray is spawned with stdio nulled, then `wait_ready` polls; only after it
   succeeds is the CDP reservation released and Chromium spawned, so the
   browser is not started against a SOCKS endpoint nothing is listening on.

One engine per profile: the config path is
`<runtime dir>/<profile id>/xray.json` (`XRAY_CONFIG_FILE`), the socks port is
a per-start reservation (never stored in the profile), and a start of a profile
that already has an `ActiveSession` is refused with a warning rather than
starting a second engine.

### The generated config

`DefaultXrayConfigBuilder::build` re-runs `validate_proxy` first, so what
reaches the engine is what was checked, and refuses a local port of zero. The
document has three sections: `log` at `warning`, one inbound, and the
outbounds. The inbound is explicit loopback — `listen: 127.0.0.1`, the reserved
port, `protocol: socks`, `auth: noauth`, `udp: true`. The profile's outbound is
written under the engine's protocol name for that kind; credentials go where
that protocol's engine code looks for them — `servers` for SOCKS5, HTTP,
Shadowsocks and Trojan, `vnext[].users` for VMess and VLESS. The VMess
`alterId` is omitted when zero (the engine's own default), and a flowless
VLESS user gets an empty `flow` string rather than a missing field. The
per-protocol field mapping is the [service contracts](../service-contracts.md)
section 5.

`stream_json` writes `streamSettings` only when the stream is not plain
(`StreamSettings::is_plain`), renaming the model's fields to the ones the
engine reads: `network`/`security` plus a block for each selected concern —
`tlsSettings` (`serverName`, `fingerprint`, `alpn`), `realitySettings`
(`serverName`, `publicKey`, `shortId`, `fingerprint`, `spiderX`), `wsSettings`
(`path`, `headers.Host`) and `grpcSettings` (`serviceName`) — written
independently, so a tls-over-websocket stream carries both `tlsSettings` and
`wsSettings`. There is no field
that could produce `allowInsecure`: that switch was removed from the engine
this program targets, so "do not check the certificate" cannot be expressed.
The config carries no routing rules at all — log, inbound, outbounds is the
whole document. On Unix the file is created with mode `0600`, because it holds
the upstream credentials; the directory layout around it is owned by
[lifecycle](../lifecycle.md) section 9.

### The machine's own proxy as a first hop

When the "use system proxy" setting is on, the profile's outbound is dialled
*through* the machine's proxy instead of directly. `with_system_proxy` switches
`DefaultXrayConfigBuilder` to read the machine when the config is written
(`Source::Host`), so a machine whose proxy changed after the program started is
still read as it is now; the reading is `system_proxy::detect`, called for each
config the builder writes and once when the switch is flipped so the window can
report what was found. `Reading::Found` appends a second outbound tagged
`system-proxy` (`SYSTEM_PROXY_TAG`; nothing else in a generated config is
tagged) and adds `streamSettings.sockopt.dialerProxy: "system-proxy"` to the
profile's outbound — creating a `streamSettings` object even when the stream is
otherwise plain. `Reading::Automatic` (a script chooses the proxy) and
`Reading::None` write no hop at all: the engine dials directly, and the
difference between "nothing there" and "something this cannot read" is the
window's to report, not the config's to guess at.

`detect` reads the most specific source that answers: the environment first
(`all_proxy`, then `https_proxy`, then `http_proxy`, both cases, the first
non-empty one that parses), then the platform store — GNOME's `gsettings`
(`mode: auto` is `Automatic`; manual reads SOCKS before HTTP), macOS
`scutil --proxy` (HTTPS, then HTTP, then SOCKS, each gated on its enable key),
or Windows `reg query` (the plain `ProxyServer` entry preferred over the legacy
`socks=`; `AutoConfigURL` as `Automatic`). Only HTTP and SOCKS5 proxies are
read; other schemes are passed over and `https://` is read as a plain HTTP
proxy on that port. Credentials are read only where the machine keeps them in
the clear (URL userinfo, GNOME's own keys). Each source is a local process with
a 2-second ceiling; one that never answers is killed and read as "nothing",
which means dialling directly. The switch itself is a stored setting with
`Effect::NextStart`, baked into the supervisor's builder when the program
starts.

### Stop, crash and reclaim

The supervisor is the only owner of the child handles. On stop it asks the
browser to close over CDP, waits, then kills the browser tree and the Xray
process; if both die, the temporary config and the session record are removed
and the profile is published `Stopped`. If either kill fails, the session and
its record stay — a start cannot then put a second engine on a port or a second
browser on a data directory this one still holds. If Xray exits unexpectedly
while the profile runs, the profile is reported `Crashed` with the Xray
component and the browser is reclaimed too: a browser left running while its
tunnel died would be egressing by a path the user did not choose. A browser
exit with status zero is a normal exit and is reclaimed without an alarm.

At program start, before the window opens, `RuntimeSupervisor::recover_orphans`
runs `journal::recover` over the previous run's records. A session whose
processes are gone has its leftover `xray.json` removed (it holds the upstream
credentials) along with the record; a record marked `left_running` is adopted
instead and becomes a running profile again, and `install_adopted` remembers
its config file as the thing an ordinary stop must remove.

## State and transitions

- Proxy records are create / update / delete; editing a proxy a profile is
  running on does not disturb the started engine (the config was written at
  start, and the record is read again at the next start). An engine belongs to
  one profile and one start: there is at most one `ActiveSession` per profile,
  and the socks port and config path are session state, never stored in the
  profile. The profile's own state machine is owned by
  [lifecycle](../lifecycle.md).
- The config file's lifetime is the session's: written before the engine is
  spawned, removed by `StartAttempt::drop` on any failed or cancelled start, by
  the stop path, and by startup reclaim; kept while the session is adopted, and
  removed when that session is stopped.
- The system-proxy switch takes effect at the next program start
  (`Effect::NextStart`), while the machine's proxy itself is re-read at each
  config write, so `Found` / `Automatic` / `None` can change between two starts
  without a restart.

## Constraints and failure behavior

- `validate_proxy` requires a non-empty name; a non-empty host with no
  whitespace and no `://`; a port above zero; a username and password that are
  both present or both absent; and, per kind, the mandatory secrets — the
  Shadowsocks password and method, the VMess uuid and security, the VLESS uuid
  and encryption, the Trojan password. `validate_stream` refuses blocks nobody
  would read (`tls` or `reality` under `security=none`, `ws`/`grpc` under the
  wrong network), a REALITY stream without a public key, and REALITY over
  anything but TCP — the engine refuses that one at startup, while it accepts
  the others in silence, which is why they are checked here.
- A stripped proxy (`ProxyOutbound::without_credentials`) still validates for
  SOCKS5 and HTTP — where authentication is optional, that is a different but
  complete proxy — and is refused for the other four, whose credentials are
  mandatory. A stripped REALITY stream is refused for the missing key rather
  than merely degraded.
- The builder refuses before writing anything: an invalid proxy, a zero port
  and incomplete credentials all return an error and leave no file behind.
  Readiness then fails in three named ways: the child exited before the port
  answered, the timeout passed (default 5 s, `xray_ready_timeout`), or the
  start was cancelled by a Stop/Shutdown. Every one of them rolls the whole
  attempt back — children ended, config and record removed — and the profile is
  not reported `Running`.
- `wait_ready` proves only that something answers on `127.0.0.1:<socks_port>`:
  a live child and a listening TCP endpoint. It cannot say that a request would
  travel through the proxy; that is the diagnostic's question.
- Deleting a proxy still assigned to any profile is refused twice: the service
  checks and names the profiles, and the schema keeps the reference
  `ON DELETE RESTRICT`, so a database written or edited outside the program
  cannot leave an assignment pointing at a proxy that is gone.

## Verification

Unit tests live with the code:

- Share links: `cargo test -p domain uri` — e.g.
  `shadowsocks_links_arrive_in_three_shapes`,
  `a_link_this_program_cannot_carry_is_refused_by_name`,
  `everything_that_parses_would_also_validate`.
- Model and validation: `cargo test -p domain proxy` and
  `cargo test -p domain validation` — e.g.
  `an_outbound_carries_credentials_exactly_when_stripping_changes_it`,
  `a_stream_setting_nobody_would_read_is_refused`.
- Config generation: `cargo test -p runtime --lib xray` — e.g.
  `each_protocol_writes_its_secret_where_the_engine_reads_it`,
  `the_machine_proxy_is_an_outbound_the_profile_dials_through`,
  `nothing_is_written_when_the_machine_has_no_proxy_or_chooses_one_by_script`.
- The machine's proxy: `cargo test -p runtime --lib system_proxy` — e.g.
  `a_variable_is_read_as_whatever_kind_of_proxy_it_names`.
- Engine lifetime: the supervisor's tests are Unix-only (`#[cfg(all(test,
  unix))]` in `crates/runtime/src/supervisor.rs`); on Linux,
  `cargo test -p runtime --lib supervisor` — e.g.
  `xray_crash_terminates_browser_and_clears_session`,
  `startup_failures_rollback_xray_and_config`.
- Application rules: `cargo test -p application proxy` — e.g.
  `a_proxy_still_assigned_to_a_profile_cannot_be_deleted`; and
  `cargo test -p application both_commands_carry_the_same_resolved_records`.
- Window paths: `cargo test -p app proxy` — e.g.
  `a_pasted_link_becomes_a_proxy`,
  `saving_a_proxy_keeps_a_running_profile_on_what_it_started_with`; and
  `cargo test -p app the_system_proxy_switch_is_stored_and_an_unknown_name_means_off`.

Opt-in suites, `#[ignore]`d and needing a real engine (never part of
`scripts/check.sh` / `check.ps1`):

- `XRAY_BIN=/path/to/xray cargo test -p runtime --test xray_real -- --ignored`
  — `links_from_the_wild_become_configs_the_engine_accepts`,
  `every_shape_the_builder_makes_is_accepted_by_the_engine`.
- `XRAY_BIN=/path/to/xray cargo test -p runtime --lib a_hop -- --ignored` —
  `a_hop_config_is_one_the_real_engine_accepts`, the system-proxy hop held
  against the real engine's `xray run -test`.
- With `XRAY_BIN` and a reachable upstream, `cargo test -p runtime --test
  proxy_diag_real -- --ignored` also runs a temporary engine whose config comes
  from this builder; that suite is the diagnostic path, and the acceptance
  suites are listed in [code standards](../code-standards.md).
