use crate::error::PortError;
use std::net::TcpListener;

pub trait PortAllocator: Send + Sync {
    fn allocate_loopback(&self) -> Result<u16, PortError>;
}

#[derive(Debug, Default)]
pub struct TcpPortAllocator;

impl TcpPortAllocator {
    pub fn new() -> Self {
        Self
    }
}

impl PortAllocator for TcpPortAllocator {
    fn allocate_loopback(&self) -> Result<u16, PortError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        Ok(port)
    }
}
