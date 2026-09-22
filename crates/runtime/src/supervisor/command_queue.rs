use super::{ChannelRuntimeFacade, HANDOVER_TIMEOUT};
use crate::error::RuntimeCommandError;
use crate::events::{RuntimeCommand, StartParams};
use crate::facade::{RuntimeFacade, RuntimeSnapshot};
use crossbeam_channel::{Receiver, Sender};
use domain::ProfileId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[test]
fn cancelling_a_queued_start_acknowledges_that_request() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let (events, _) = crossbeam_channel::unbounded();
    let snapshots = Arc::new(RwLock::new(HashMap::new()));
    let mut supervisor = super::RuntimeSupervisor::new(rx, events, Arc::clone(&snapshots));
    let params = crate::test_support::start_params(&std::env::temp_dir().join("cancel-ack"));
    let (id, request) = (params.profile_id(), params.request_id);
    supervisor
        .pending_commands
        .push_back(RuntimeCommand::Start(params));
    tx.send(RuntimeCommand::Stop(id)).unwrap();
    assert!(!supervisor.poll_start_commands(ProfileId::new()));
    assert!(supervisor.pending_commands.is_empty());
    let snapshot = snapshots.read().unwrap().get(&id).cloned().unwrap();
    assert_eq!(snapshot.acknowledged_start, request);
    assert_eq!(snapshot.state, domain::RuntimeState::Stopped);
}

fn facade(command_tx: Sender<RuntimeCommand>, handover_timeout: Duration) -> ChannelRuntimeFacade {
    ChannelRuntimeFacade::new(
        command_tx,
        Arc::new(RwLock::new(HashMap::<ProfileId, RuntimeSnapshot>::new())),
    )
    .with_handover_timeout(handover_timeout)
}

/// The command channel is bounded, and the window's own thread is what fills
/// it. A send that waits for room is a frozen window while the supervisor is
/// inside a readiness wait or a process-tree kill, so a full queue is refused
/// - immediately, and with an answer that says trying again is worth it.
#[test]
fn a_full_command_queue_is_refused_rather_than_waited_on() {
    let (command_tx, command_rx) = crossbeam_channel::bounded(1);
    let facade = facade(command_tx, HANDOVER_TIMEOUT);
    let params: StartParams =
        crate::test_support::start_params(&std::env::temp_dir().join("fp-runtime-queue"));

    // Nobody is draining: the second command has nowhere to go.
    facade
        .start(params.clone())
        .expect("room for the first command");
    let refused = facade
        .stop(params.profile.id)
        .expect_err("the queue is full and this must not wait for it");
    assert!(matches!(refused, RuntimeCommandError::Busy), "{refused}");
    assert!(
        matches!(facade.start(params.clone()), Err(RuntimeCommandError::Busy)),
        "and it is refused for every command, not just the one"
    );

    // A supervisor that is gone, on the other hand, is not a queue to retry:
    // the two answers have to stay different.
    drop(command_rx);
    assert!(matches!(
        facade.restart(params),
        Err(RuntimeCommandError::ChannelClosed)
    ));
}

/// Leaving the running sessions behind is the last thing a run does, so it is
/// the one command that waits for room - and it stops waiting, because an
/// exit that never finishes is worse than one that warns.
#[test]
fn the_handover_waits_for_room_but_not_forever() {
    let (command_tx, command_rx): (Sender<RuntimeCommand>, Receiver<RuntimeCommand>) =
        crossbeam_channel::bounded(1);
    let facade = facade(command_tx, Duration::from_millis(50));
    let params: StartParams =
        crate::test_support::start_params(&std::env::temp_dir().join("fp-runtime-queue"));
    facade.start(params).expect("room for the first command");

    let started = Instant::now();
    let refused = facade
        .release_all()
        .expect_err("a full queue it cannot wait out");
    let waited = started.elapsed();
    assert!(matches!(refused, RuntimeCommandError::Busy), "{refused}");
    assert!(
        waited >= Duration::from_millis(50) && waited < HANDOVER_TIMEOUT,
        "it waits for the timeout and then gives up: {waited:?}"
    );

    // With room it is taken, which is the ordinary exit.
    command_rx.recv().expect("the first command");
    facade
        .release_all()
        .expect("the command was taken once there was room");
    assert!(matches!(command_rx.recv(), Ok(RuntimeCommand::ReleaseAll)));
}
