use crate::error::PortError;
use std::net::TcpListener;

pub trait PortAllocator: Send + Sync {
    fn reserve_loopback(&self) -> Result<PortReservation, PortError>;

    /// Only discovers a port. Launchers must use reserve_loopback instead.
    fn allocate_loopback(&self) -> Result<u16, PortError> {
        Ok(self.reserve_loopback()?.port())
    }
}

/// Keeps a TCP port bound until dropped, including on startup rollback.
#[derive(Debug)]
pub struct PortReservation {
    _listener: TcpListener,
    port: u16,
}

impl PortReservation {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Binds one explicit loopback port instead of asking the OS for an
    /// ephemeral one. Tests use this to observe release semantics on a port the
    /// ephemeral pool cannot hand to a concurrently running test.
    #[cfg(test)]
    fn bind_loopback(port: u16) -> Result<Self, PortError> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        Ok(Self {
            _listener: listener,
            port,
        })
    }
}

#[derive(Debug, Default)]
pub struct TcpPortAllocator;

impl TcpPortAllocator {
    pub fn new() -> Self {
        Self
    }
}

impl PortAllocator for TcpPortAllocator {
    fn reserve_loopback(&self) -> Result<PortReservation, PortError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        Ok(PortReservation {
            _listener: listener,
            port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Picks a reservation on a port below the OS ephemeral range
    /// (`/proc/sys/net/ipv4/ip_local_port_range`, 32768-60999 by default).
    /// `TcpPortAllocator` binds `127.0.0.1:0`, so it can never receive a port
    /// from this band; that is what keeps the release assertion below
    /// deterministic while the rest of the test binary reserves ports in
    /// parallel.
    fn reservation_outside_ephemeral_range() -> PortReservation {
        (20_000..=29_999)
            .find_map(|port| PortReservation::bind_loopback(port).ok())
            .expect("no free non-ephemeral loopback port")
    }

    #[test]
    fn reservations_are_distinct_and_hold_their_port() {
        let first = TcpPortAllocator.reserve_loopback().unwrap();
        let second = TcpPortAllocator.reserve_loopback().unwrap();

        assert_ne!(first.port(), second.port());
        assert!(TcpListener::bind(("127.0.0.1", first.port())).is_err());

        // Releasing one reservation must not release its sibling.
        drop(first);
        assert!(TcpListener::bind(("127.0.0.1", second.port())).is_err());
    }

    #[test]
    fn dropping_a_reservation_releases_its_port() {
        let reservation = reservation_outside_ephemeral_range();
        let port = reservation.port();

        assert!(TcpListener::bind(("127.0.0.1", port)).is_err());

        drop(reservation);
        assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    }
}
