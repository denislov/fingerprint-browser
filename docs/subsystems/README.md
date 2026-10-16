# Subsystem design references

These pages describe current responsibilities, runtime paths, state, and contracts. Start with the subsystem relevant to your task, then follow its source links to verify the implementation you change.

| Subsystem | Reference |
| --- | --- |
| Runtime lifecycle: profile launch, start/stop/restart, crash handling, reclaim and adoption | [Runtime lifecycle](../lifecycle.md) |
| Domain model: profile, fingerprint, proxy, core, launch-plan and session types | [Domain model](../domain-model.md) |
| Fingerprints: persona switches, capability table and browser read-back | [Fingerprints](fingerprints.md) |
| Proxies and Xray: outbound model, share-link import, per-profile engine and the machine's proxy | [Proxies and Xray](proxies-xray.md) |
| Proxy tests and the exit address a browser's traffic leaves from | [Traffic verification](traffic-verification.md) |
| The local page a session can serve about itself, and what each number on it proves | [Identity page](identity-page.md) |
| Browser cores: discovery, version probe and published-build downloads | [Browser cores](browser-cores.md) |
| Storage: schema, migrations, repositories and the configuration transaction | [Storage](storage.md) |
| Configuration backup, import, restore and browser-data copy | [Backup and restore](../backup-and-restore.md) |
| The window: pages, dialogs, view state and appearance | [UI shell](ui-shell.md) |
| Settings, files and effective configuration | [Settings](settings.md) |
| Interface text and the catalog's translation rules | [Interface text](../i18n.md) |
| Closing the window, the tray and one window per data directory | [Exit modes](../exit-modes.md) |
| The diagnostics report and the activity log | [Diagnostics](../diagnostics.md) |
| Packaging and releases | [Releases](../release.md) |

A subsystem can span several packages or repositories. Its reference describes the behavior those parts provide together; it does not need to list every function.

## Maintain a reference

Update the owning reference in the same PR when a change alters a documented responsibility, interface, state transition, runtime path, compatibility rule, or failure behavior. An internal refactor that preserves these contracts does not require a prose change.

Keep each fact in one reference and link to it from agent instructions or other pages. Link to source files and named symbols rather than copying implementation details that change frequently. Explain field meanings and constraints that signatures alone do not express.

For a new reference, include the responsibilities and boundaries, source of truth, main runtime paths, state transitions, important constraints, and existing verification commands. Include failure and compatibility behavior where relevant. Document current behavior separately from proposed changes; the proposal belongs in its issue or PR.

## Describe a PR's semantic changes

For each affected subsystem, explain the observable change with its trigger and before/after behavior. Include changes to interfaces, state, compatibility, and failure behavior when relevant. Link the affected reference, implementation, and verification evidence. State when a refactor preserves the documented behavior.

For example, a change to session adoption at startup should explain which journal record and `left_running` marker make a browser adopted rather than reclaimed, which code path runs before and after, and which test exercises that transition.

This is an author-provided explanation that a reviewer verifies against the diff and surrounding code. It is not an automatically proven compatibility verdict.

## Review a change

1. Identify the affected subsystem and the actual runtime path, including which layer owns it - the window and app state, the application services, the runtime supervisor, or storage.
2. Compare the claimed before/after behavior with the base and head implementation. Trace relevant callers and state transitions beyond the changed lines.
3. Check that changed contracts appear in the owning reference and that source links still resolve.
4. Check that the cited tests or runtime evidence exercise the claimed behavior. Report checks that were not run.

A documentation update alone does not prove the implementation. Existing tests and runtime evidence remain necessary for behavior changes.
