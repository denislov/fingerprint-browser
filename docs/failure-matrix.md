# Failure matrix

What this program does when something goes wrong, subsystem by subsystem, and
where that behavior is checked. A row is a failure the program is built to
survive; "proved by" names the test that would fail if the behavior were lost.
The opt-in suites at the end re-check paths that need a real browser, engine or
network.

This page is a map, not a second specification. The owning reference and the
test are the sources of truth; if this page and the code disagree, the code
wins and this page is the bug. `path::test` is a test in
`crates/<path>/src`, unless the path names a `tests/` file.

## Launch and process lifecycle

The runtime supervisor's job is to leave nothing behind that it cannot describe,
and to keep describing anything it could not stop.

| Failure | What must happen | Proved by |
| --- | --- | --- |
| The browser executable is missing or cannot be spawned | The start fails with a message; no record, no Xray config, no held port | `runtime/src/lib.rs::test_supervisor_start_nonexistent_executable_fails` |
| The browser starts but its debugging endpoint never answers | The start rolls back: no record, no config, nothing for the next run to reclaim | `runtime/src/supervisor/tests.rs::a_start_that_never_reaches_readiness_leaves_no_record` |
| Xray exits before readiness, or never binds | The four startup-failure cases all fail closed; no `Started`, no record, no process left | `runtime/src/supervisor/tests.rs::startup_failures_rollback_xray_and_config` |
| A stranger's listener holds the released SOCKS port | Readiness waits for the SOCKS5 greeting the inbound answers; a bare connect is not readiness | `runtime/src/xray.rs::a_listener_that_is_not_the_engine_is_not_ready` (unix) |
| Planning fails after the ports were reserved | The start fails and every reserved port is released | `runtime/src/supervisor/tests.rs::startup_holds_distinct_ports_and_releases_them_on_planning_failure` |
| Stop, restart or shutdown arrives while a start is waiting for readiness | The start is cancelled or re-run deliberately; no `Started`/`Failed` for a cancelled request, and nothing is left running | `runtime/src/supervisor/tests.rs::stop_during_cdp_readiness_rolls_back_without_started_or_failed`, `::stop_during_xray_wait_reaps_process_before_timeout`, `::restart_during_readiness_cancels_then_starts_again_without_recursion`, `::shutdown_during_start_exits_run_loop_and_discards_deferred_start` |
| The browser or Xray crashes while the session is running | The other child is reaped, the session is cleared, a `Crashed` event names the component, and the profile ends `Stopped`; a following stop is idempotent | `runtime/src/supervisor/tests.rs::browser_crash_reclaims_xray_and_stop_is_idempotent`, `::xray_crash_terminates_browser_and_clears_session` |
| A stop cannot reach or kill a process (adopted or unresponsive tree) | The session and its record are kept, the state stays `Running`, and the window is told why | `runtime/src/supervisor/tests.rs::a_stop_that_cannot_reach_an_adopted_process_keeps_the_session_and_its_record`, `::a_stop_a_process_tree_refuses_keeps_the_session_and_its_record` |
| The Xray process is already gone when a stop runs | The stop still succeeds; a missing engine is not a reason to keep the session | `runtime/src/supervisor/tests.rs::normal_stop_reclaims_both_children_and_missing_xray_fails_closed` |
| A killed run leaves a record and processes behind | Reclaim stops only processes it can identify, removes the record it acted on, and reports the rest | `runtime/src/supervisor/tests.rs::reclaiming_orphans_stops_what_a_killed_run_left_running`; `runtime/src/journal.rs::a_session_left_by_a_crash_is_stopped_and_its_record_removed` |
| A recorded PID was recycled, or now runs something else | It is not signalled; the record is left unresolved rather than acted on | `runtime/src/journal.rs::a_recorded_pid_that_was_recycled_is_not_killed`, `::a_live_pid_running_something_else_is_not_killed` |
| A record cannot be read or checked | It is reported and left in place; nothing it describes is signalled | `runtime/src/journal.rs::an_unreadable_record_is_reported_and_left_in_place`, `::a_record_nobody_can_check_is_left_alone` |
| A session was deliberately left running (release-all) | The next run adopts it instead of reclaiming it | `runtime/src/journal.rs::a_session_marked_as_left_running_is_adopted_and_not_stopped` |
| The event queue is full or its receiver is gone | Stop, crash and shutdown still make progress; failure diagnostics are preserved | `runtime/src/supervisor/tests.rs::full_event_queue_does_not_block_stop_crash_or_shutdown`, `::disconnected_event_receiver_preserves_failure_diagnostics` |
| The command queue is full or disconnected | A start is refused or cancelled; queued starts are acknowledged by request id, not by timeout | `runtime/src/supervisor/command_queue.rs::a_full_command_queue_is_refused_rather_than_waited_on`, `::cancelling_a_queued_start_acknowledges_that_request`; `runtime/src/supervisor/tests.rs::disconnected_command_channel_cancels_start` |
| A profile's start page is a switch or an unsupported scheme | It is refused before it can become a Chromium argument | `domain/src/start_target.rs::a_switch_spelled_as_a_start_page_is_refused`, `::a_scheme_this_program_does_not_open_is_refused_by_name`; `runtime/src/lib.rs::a_start_page_that_is_a_switch_never_becomes_an_argument` |
| The core's major was never detected (major 0) | Capability resolution refuses it rather than assuming a switch set | `runtime/src/capability.rs::refuses_an_undetected_major` |
| The core is newer than the newest measured major | The launch is allowed, but the compatibility report says the rule was not read back on that build | `runtime/src/compat.rs::a_core_newer_than_the_newest_measured_major_is_reported`; the boundary is in [fingerprint-matrix.md](fingerprint-matrix.md) |

## Data and configuration

A replace writes beside its target and swaps; a destructive write validates the
whole input before its first remove.

| Failure | What must happen | Proved by |
| --- | --- | --- |
| A settings write fails part-way, or its rename fails | The previous file survives and no temporary file is left behind | `storage/src/atomic_file.rs::a_partial_write_failure_preserves_the_previous_file`, `::a_failed_rename_does_not_remove_the_target_or_leak_a_tempfile` |
| The configuration file is not readable or is broken | The problem is reported, defaults are used so the program still starts, and every write is refused without overwriting the file | `app/src/settings/tests.rs::a_broken_config_file_is_reported_and_does_not_lose_settings`, `::the_read_only_settings_cannot_be_saved` |
| A configuration replacement fails after some records were written | The previous configuration is still the installed one | `storage/src/lib.rs::a_replacement_that_fails_part_way_leaves_the_old_configuration` |
| The database was written by a newer build | Opening it is refused with the schema versions, not attempted | `storage/src/lib.rs::a_database_from_a_newer_build_is_refused` |
| The database is from the previous schema | It upgrades without losing rows | `storage/src/lib.rs::a_database_from_the_previous_schema_upgrades_without_losing_rows` |
| A repository write names a missing core or proxy | The refusal is classified as dangling and the original row survives | `storage/src/lib.rs::the_database_keeps_the_repository_contract`, `::the_memory_backend_keeps_the_repository_contract`; `storage/src/sqlite.rs::every_constraint_sqlite_reports_is_classified_by_its_extended_code` |
| A restore file contains a record this program refuses, a missing core or proxy, a duplicate id, or no credentials | Nothing changes: the whole file is validated before the first remove | `application/src/restore.rs::a_file_with_a_record_this_program_refuses_changes_nothing`, `::a_file_whose_profile_names_a_core_it_does_not_hold_changes_nothing`, `::a_file_whose_profile_names_a_proxy_it_does_not_hold_changes_nothing`, `::a_file_that_holds_the_same_identifier_twice_changes_nothing`, `::a_file_exported_without_credentials_changes_nothing` |
| A restore or import targets a populated installation without confirmation | It is refused and changes nothing | `application/src/restore.rs::a_restore_onto_a_populated_installation_is_refused_and_changes_nothing`; `app/src/state/tests/maintenance.rs::a_restore_without_confirmation_refuses_a_populated_installation` |
| An import file has no path, is not this program's document, or names a missing core | The file is refused by name; a profile whose core is missing is skipped and named; the rest of the file still imports | `application/src/import.rs::a_path_with_nothing_at_it_is_refused_by_name`, `::a_file_that_is_not_one_of_ours_is_refused_as_a_document`, `::a_profile_whose_core_is_missing_is_skipped_and_named`, `::an_item_the_database_refuses_does_not_stop_the_rest` |
| A browser-data copy/restore source and destination overlap, nest, or are the same directory | The operation is refused before anything is copied; a symlinked source is not followed out of its tree | `application/src/browser_data/tests.rs::a_backup_directory_that_is_the_profile_directory_is_refused`, `::a_backup_directory_inside_the_profile_directory_is_refused`, `::a_restore_whose_destination_contains_its_source_is_refused`, `::two_profiles_whose_directories_overlap_are_refused`, `::a_destination_inside_a_symlinked_source_is_refused` |
| A browser-data copy runs while the profile is running | It is refused by name before anything is copied | `application/src/browser_data/tests.rs::a_running_profile_is_refused_by_name_before_anything_is_copied` |
| A copy fails part-way | The directory that was there survives | `application/src/browser_data/tests.rs::a_copy_that_fails_leaves_the_directory_that_was_there` |
| A run is killed in the middle of a copy or restore | The next start puts the set-aside directory back and names it; another profile's directory is never touched | `application/src/browser_data/tests.rs::a_restore_left_aside_is_put_back_at_startup_and_named`, `::recovery_does_not_delete_another_profiles_directory`, `::unowned_siblings_are_preserved_during_copy_and_startup` |
| Two data-changing operations overlap | They are excluded by an exclusive lease that names the conflicting work and is released on unwind | `application/src/coordination.rs::leases_exclude_conflicting_work_and_release_on_unwind`; `application/src/operations.rs::a_second_operation_on_one_profile_is_refused_by_name`, `::a_lease_is_all_or_nothing`, `::a_lease_gives_every_profile_back_when_it_is_dropped` |
| A launch names a profile whose record is missing | The command is refused before the runtime hears about it | `application/src/runtime_service.rs::a_missing_record_refuses_the_command_before_the_runtime_hears_about_it` |
| The activity log is full, or its directory cannot be created | The file is rotated and the new line survives; a directory that cannot be created is an error | `app/src/log_file.rs::a_full_file_is_rotated_and_the_new_line_survives`, `::a_directory_that_cannot_be_created_is_an_error` |
| A second instance starts on the same data directory | It is refused and told who holds the directory; the lock is not readable by everyone | `app/src/instance.rs::a_second_run_is_refused_and_told_who_holds_the_directory`, `::the_lock_file_is_not_readable_by_everyone` |

## Inputs from outside the program

Everything here arrives from a user, a proxy provider, a web page, the network
or another program, and is checked where it turns into an argument, a path, a
socket or a stored record.

| Failure | What must happen | Proved by |
| --- | --- | --- |
| A proxy share link cannot be carried, is empty, or carries an unknown parameter | The link is refused by name (or the unknown parameter is passed over, never guessed at); everything that parses also validates | `domain/src/uri/tests.rs::a_link_this_program_cannot_carry_is_refused_by_name`, `::a_link_with_nothing_in_it_says_so`, `::a_parameter_this_program_has_never_seen_is_passed_over`, `::everything_that_parses_would_also_validate` |
| A proxy is nameless, has no host or port, has half a credential, or is missing its protocol's secret | The record is refused with the field named | `domain/src/validation.rs::a_nameless_proxy_is_refused`, `::a_proxy_without_a_host_or_port_is_refused`, `::a_pasted_url_is_refused_as_a_host`, `::half_a_credential_is_refused`, `::the_per_protocol_secrets_are_required` |
| A debugger URL names another host, a target id is not id-shaped, or a created URL is off loopback | It is never dialled or put in a path; the reply is not treated as a page | `runtime/src/cdp.rs::a_debugger_url_off_loopback_is_refused`, `::a_debugger_url_off_loopback_is_not_dialled_after_creation`, `::a_target_id_that_is_not_id_shaped_is_never_put_in_a_path` |
| A CDP peer is silent, never completes the handshake, or answers the wrong shape | The deadline wins; empty or wrong-shaped JSON is not readiness; a page that never committed is not the document asked for | `runtime/src/cdp.rs::a_silent_peer_cannot_hold_the_evaluation_past_its_deadline`, `::a_peer_that_never_completes_the_handshake_fails_the_dial`, `::unresponsive_http_server_cannot_exceed_readiness_deadline`, `::empty_json_is_not_ready`, `::a_page_that_did_not_arrive_is_not_the_document_asked_for` |
| The identity page receives a request naming another `Host`, no `Host`, two `Host`s, a query string, another method, or an unknown path | `403`, `403`, `400`, `400`, `405` and `404` respectively - and a refused request learns nothing about the paths | `runtime/src/status.rs::a_request_that_names_another_host_is_refused`, `::a_request_without_a_host_is_refused`, `::a_request_with_two_hosts_is_refused`, `::a_query_string_is_refused_rather_than_ignored`, `::an_unknown_path_is_not_found_and_another_method_is_refused` |
| An identity-page request head is oversized, or its client asks and stops | A head over 8 KiB gets `431` and the service survives; a silent client is timed out and does not hold the port; dropping the service frees it | `runtime/src/status.rs::a_head_that_is_too_long_is_refused_and_the_service_survives`, `::a_client_that_never_asks_does_not_hold_the_service`, `::dropping_the_service_frees_its_port` |
| The identity page is asked for a report | It carries the proxy's **name** only, echoes nothing from the request, and escapes a profile name with markup in it | `runtime/src/status.rs::the_report_is_json_and_carries_no_proxy_credentials`, `::nothing_the_request_carried_is_echoed_back`, `::a_name_with_markup_in_it_is_escaped` |
| A proxy diagnostic meets TLS, a timeout, an open port that carries nothing, a credential demand, a refused forward, or an unrecognised engine line | Each is its own fault class, bounded before it reaches the window; none becomes a reading | `runtime/src/diagnostic/tests.rs::a_tls_endpoint_is_refused_rather_than_downgraded`, `::a_socket_that_ran_out_of_time_is_a_timeout_and_not_a_dead_network`, `::an_open_port_that_carries_nothing_is_a_fault_and_not_a_pass`, `::a_proxy_that_demands_a_credential_is_not_reported_as_a_dead_network`, `::a_refused_forward_is_not_reported_as_a_refused_credential`, `::an_unrecognised_engine_line_is_carried_rather_than_guessed_at`, `::a_detail_is_bounded_before_it_reaches_a_window` |
| A diagnostic peer drips bytes or never answers | One absolute deadline covers every read and write, so partial progress cannot renew it | `runtime/src/diagnostic/tests.rs::partial_reads_share_one_deadline`, `::an_endpoint_that_never_answers_is_a_timeout_and_not_a_hang` |
| An egress probe endpoint is not plain HTTP, the page never commits or commits elsewhere, or the answer is prose | The endpoint is refused before anything is asked; a page that never arrived is not a reading; an answer without an address is unreadable, not an address | `runtime/src/egress.rs::an_endpoint_that_is_not_plain_http_is_refused_before_anything_is_asked`, `::a_page_that_never_committed_did_not_arrive`, `::a_page_that_committed_elsewhere_did_not_arrive`, `::an_endpoint_that_answers_with_prose_is_unreadable_rather_than_an_address`, `::an_answer_with_no_address_in_it_is_bounded_before_it_reaches_a_window` |
| A fingerprint reading is missing, a WebRTC gathering never finished, or a font surface is unreadable | A missing reading is a discrepancy, not a pass; unfinished gathering is not "no leak"; missing CJK or emoji is reported | `runtime/src/verify.rs::a_missing_reading_is_a_discrepancy_not_a_pass`, `::gathering_that_never_finished_is_not_a_pass`, `::a_relaxed_policy_makes_no_leak_claim`, `::missing_cjk_glyphs_are_reported`, `::an_emoji_that_collapses_into_the_tofu_box_is_reported` |
| Verification is asked of a core with no version, or a stale reading is still in flight | The action is refused rather than asserted; stopping/restarting drops the stale reading; a second verification is refused | `app/src/state/tests/verification.rs::verifying_through_a_core_without_a_version_is_refused_not_asserted`, `::stopping_or_restarting_a_profile_drops_a_stale_reading`, `::a_second_verification_of_the_same_profile_is_refused` |
| The published-core list cannot be read, a release has no usable asset, or a download fails | The dialog and banner say why; a failed download registers nothing; an asset name cannot describe a path; a partial file is named beside its destination | `app/src/state/tests/releases.rs::a_failed_fetch_is_a_reason_in_the_dialog_and_in_the_banner`, `::one_download_at_a_time_and_the_refusal_names_the_one_running`, `::a_failed_download_is_reported_and_registers_nothing`, `::a_release_directory_cannot_escape_the_cores_directory`; `app/src/releases.rs::a_partial_download_is_named_beside_its_file`, `::an_asset_name_cannot_describe_a_path`; `app/src/core_releases.rs::a_release_with_no_usable_asset_is_not_listed`, `::a_reply_that_is_not_the_expected_document_is_an_error` |
| A diagnostics report cannot be written | The report says which path failed; it never carries what is in the database | `app/src/diagnostics.rs::a_report_that_cannot_be_written_says_which_path`, `::a_report_never_carries_what_is_in_the_database` |
| A directory to open does not exist | The opener is not asked; the path is refused and named | `app/src/open_dir.rs::a_directory_that_does_not_exist_is_refused_with_its_path` |

## Acceptance runs that need the real thing

These are `#[ignore]`d and never part of the gate; they need a binary, an
upstream, the network or a machine. Their requirements are in the
[code standards](code-standards.md#acceptance-criteria) and each suite's
`#[ignore]` reason.

| Path | What the run re-checks | Suite |
| --- | --- | --- |
| A real browser, cancelled during readiness | Nothing is left running or recorded | `runtime/tests/chromium_real.rs::a_start_cancelled_while_waiting_for_readiness_leaves_nothing_running` |
| A real browser left by a killed run | The next run reclaims it | `runtime/tests/chromium_real.rs::a_browser_left_by_a_killed_run_is_reclaimed` |
| A real browser and a failing engine | Cookies persist and a Xray failure closes the browser | `runtime/tests/chromium_real.rs::profiles_persist_cookies_and_xray_failure_closes_browser` |
| A browser left running on purpose | The next run adopts it | `runtime/tests/chromium_real.rs::a_browser_left_running_by_one_run_is_adopted_by_the_next` |
| The fingerprint switch set | Every profile claim is read back from the page; the capability table agrees with the binary's major | `runtime/tests/fingerprint_real.rs` (the 150 attempt is recorded in [fingerprint-matrix.md](fingerprint-matrix.md)) |
| A real Xray | Authenticated SOCKS/HTTP upstreams carry a payload; links from the wild and every builder shape are accepted | `runtime/tests/xray_real.rs` |
| A real engine and upstream | The diagnostic reports the address a request left from; an unreachable upstream is a fault | `runtime/tests/proxy_diag_real.rs` |
| A real browser and the identity page | The page renders behind a proxy that cannot carry it | `runtime/tests/status_page_real.rs` |
| The Windows lifecycle | Cookies persist, a failure closes the browser, and a profile carries traffic through a remote upstream | `runtime/tests/windows_real.rs` |
| The window driven through a real Chromium | Start/stop, fingerprint verification, and the address the browser's traffic leaves from | `app/src/real_browser.rs` |
