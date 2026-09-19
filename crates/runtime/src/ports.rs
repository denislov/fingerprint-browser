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

    #[test]
    fn reservations_are_distinct_and_released_on_drop() {
        let first = TcpPortAllocator.reserve_loopback().unwrap();
        let second = TcpPortAllocator.reserve_loopback().unwrap();
        assert_ne!(first.port(), second.port());
        let address = ("127.0.0.1", first.port());
        assert!(TcpListener::bind(address).is_err());
        drop(first);
        assert!(TcpListener::bind(address).is_ok());
        assert!(TcpListener::bind(("127.0.0.1", second.port())).is_err());
    }
}
