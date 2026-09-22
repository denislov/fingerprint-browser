//! What a profile is busy with.
//!
//! Two things write a profile's browser data and must never overlap: the browser
//! the runtime starts, and a browser-data copy. The runtime's own answer - whether
//! the snapshot says the profile is active - cannot be the guard, because it lags
//! its commands: a start returns as soon as the command is queued, and the state
//! that command produces is published a tick later. That window is long enough to
//! start a copy of a profile whose browser is already coming up, which is the
//! shape that produces a backup of an unspecified moment - and two copies of one
//! profile is worse, because each of them clears a destination the other is
//! writing.
//!
//! So an operation takes a *lease* on the profiles it is about to use, where the
//! decision to run it is made, and gives the lease back where the operation ends.
//! The window between the two belongs to the lease, and the answer to "may I do
//! this now?" is this registry rather than a snapshot that has not caught up.

use domain::ProfileId;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What a profile is being used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// A start or restart that has been asked for and not yet answered by the
    /// runtime's snapshot.
    Starting,
    /// A browser-data copy: out to a backup, or back in.
    Copying,
}

impl Operation {
    /// What it is called in a log line.
    pub fn code(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Copying => "a browser-data copy",
        }
    }
}

/// Why a lease could not be taken: who has the profile, and for what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy {
    pub profile: ProfileId,
    pub operation: Operation,
}

/// The leases this installation holds, by profile.
#[derive(Debug, Default)]
pub struct Operations {
    held: Mutex<BTreeMap<ProfileId, Lease>>,
}

#[derive(Debug, Clone, Copy)]
struct Lease {
    operation: Operation,
    taken: Instant,
}

impl Operations {
    /// Takes the profile for one operation, or says what it is already doing.
    pub fn take(&self, profile: ProfileId, operation: Operation) -> Result<(), Busy> {
        let mut held = self.lock();
        if let Some(current) = held.get(&profile) {
            return Err(Busy {
                profile,
                operation: current.operation,
            });
        }
        held.insert(
            profile,
            Lease {
                operation,
                taken: Instant::now(),
            },
        );
        Ok(())
    }

    /// Gives back a lease this operation took.
    ///
    /// Matched by operation, so a late release cannot give away a lease somebody
    /// else took in the meantime.
    pub fn free(&self, profile: ProfileId, operation: Operation) -> bool {
        let mut held = self.lock();
        if held
            .get(&profile)
            .is_some_and(|lease| lease.operation == operation)
        {
            held.remove(&profile);
            return true;
        }
        false
    }

    /// Takes every one of these profiles for one operation, or none of them.
    ///
    /// All or nothing: a copy that held half of what it needed would be refused by
    /// the other half anyway, and the half it held would be held for a copy that
    /// never runs. The answer comes back as a [`Held`], which gives all of them
    /// back when it is dropped - including when the worker holding it panics.
    pub fn lease(
        self: &Arc<Self>,
        profiles: &[ProfileId],
        operation: Operation,
    ) -> Result<Held, Busy> {
        let mut taken = Vec::with_capacity(profiles.len());
        for profile in profiles {
            match self.take(*profile, operation) {
                Ok(()) => taken.push(*profile),
                Err(busy) => {
                    for held in &taken {
                        self.free(*held, operation);
                    }
                    return Err(busy);
                }
            }
        }
        Ok(Held {
            operations: Arc::clone(self),
            profiles: taken,
            operation,
            installation: None,
        })
    }

    /// What the profile is being used for, if anything.
    pub fn held(&self, profile: ProfileId) -> Option<Operation> {
        self.lock().get(&profile).map(|lease| lease.operation)
    }

    /// The first profile anything is being done with, for an operation that needs
    /// the whole installation to itself.
    pub fn any(&self) -> Option<(ProfileId, Operation)> {
        self.lock()
            .iter()
            .next()
            .map(|(profile, lease)| (*profile, lease.operation))
    }

    /// Reports overdue starts without releasing them. A timeout is not proof
    /// that a queued command has been cancelled; only its acknowledgement is.
    pub fn overdue(&self, age: Duration) -> Vec<ProfileId> {
        let now = Instant::now();
        let held = self.lock();
        held.iter()
            .filter(|(_, lease)| {
                lease.operation == Operation::Starting && now.duration_since(lease.taken) >= age
            })
            .map(|(profile, _)| *profile)
            .collect()
    }

    /// The map, whatever a panic while holding it did to the lock.
    ///
    /// A poisoned lock would mean a panic in code that only inserts and removes
    /// entries here; the map is a plain value and refusing to read it would turn
    /// one panic into a window that can never start anything again.
    fn lock(&self) -> MutexGuard<'_, BTreeMap<ProfileId, Lease>> {
        self.held.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// Leases taken together, given back together when this is dropped.
///
/// Owned by the worker that holds it, so a copy that fails, returns or panics
/// gives its profiles back either way: a lease that outlived its worker would
/// refuse every later start of those profiles for as long as the window is open.
#[derive(Debug)]
#[must_use = "a lease is given back as soon as it is dropped"]
pub struct Held {
    operations: Arc<Operations>,
    profiles: Vec<ProfileId>,
    operation: Operation,
    installation: Option<crate::coordination::InstallationLease>,
}

impl Held {
    pub fn with_installation(mut self, lease: crate::coordination::InstallationLease) -> Self {
        self.installation = Some(lease);
        self
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        for profile in &self.profiles {
            self.operations.free(*profile, self.operation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operations() -> Arc<Operations> {
        Arc::new(Operations::default())
    }

    #[test]
    fn a_profile_can_be_taken_and_given_back() {
        let operations = operations();
        let profile = ProfileId::new();

        assert_eq!(operations.held(profile), None);
        operations
            .take(profile, Operation::Starting)
            .expect("the first lease");
        assert_eq!(operations.held(profile), Some(Operation::Starting));

        assert!(operations.free(profile, Operation::Starting));
        assert_eq!(operations.held(profile), None);
    }

    /// The conflict, and what it says: the operation that is already there, not
    /// the one that was asked for.
    #[test]
    fn a_second_operation_on_one_profile_is_refused_by_name() {
        let operations = operations();
        let profile = ProfileId::new();
        operations
            .take(profile, Operation::Copying)
            .expect("the first lease");

        let busy = operations
            .take(profile, Operation::Starting)
            .expect_err("a copy holds the profile");
        assert_eq!(
            busy,
            Busy {
                profile,
                operation: Operation::Copying
            }
        );
    }

    /// A release is matched against the operation that took the lease: a worker
    /// that finishes late must not give away a lease taken since.
    #[test]
    fn a_release_that_does_not_match_gives_nothing_away() {
        let operations = operations();
        let profile = ProfileId::new();
        operations
            .take(profile, Operation::Starting)
            .expect("the first lease");

        assert!(!operations.free(profile, Operation::Copying));
        assert_eq!(operations.held(profile), Some(Operation::Starting));
    }

    /// All or nothing: a lease that could not cover every profile must not keep
    /// the ones it managed to take, or a refused copy would lock its profiles out
    /// of everything else for good.
    #[test]
    fn a_lease_is_all_or_nothing() {
        let operations = operations();
        let first = ProfileId::new();
        let second = ProfileId::new();
        operations
            .take(second, Operation::Starting)
            .expect("something else holds the second profile");

        let busy = operations
            .lease(&[first, second], Operation::Copying)
            .expect_err("the second profile is taken");
        assert_eq!(busy.profile, second);
        assert_eq!(
            operations.held(first),
            None,
            "the first profile was given back"
        );
    }

    #[test]
    fn a_lease_gives_every_profile_back_when_it_is_dropped() {
        let operations = operations();
        let profiles = [ProfileId::new(), ProfileId::new()];

        let held = operations
            .lease(&profiles, Operation::Copying)
            .expect("the profiles are free");
        assert_eq!(operations.any().map(|(_, op)| op), Some(Operation::Copying));

        drop(held);
        assert_eq!(operations.any(), None);
    }

    /// Age is diagnostic only. Neither a queued start nor a copy can be
    /// released just because its worker has taken longer than expected.
    #[test]
    fn overdue_starts_remain_held_until_acknowledged() {
        let operations = operations();
        let starting = ProfileId::new();
        let copying = ProfileId::new();
        operations
            .take(starting, Operation::Starting)
            .expect("the start");
        operations
            .take(copying, Operation::Copying)
            .expect("the copy");

        assert!(operations.overdue(Duration::from_secs(3600)).is_empty());
        assert_eq!(operations.overdue(Duration::ZERO), vec![starting]);
        assert_eq!(operations.held(starting), Some(Operation::Starting));
        assert_eq!(
            operations.held(copying),
            Some(Operation::Copying),
            "a copy is not expired out from under its worker"
        );
    }
}
