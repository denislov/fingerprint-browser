use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowProfile {
    pub width: u32,
    pub height: u32,
}

impl WindowProfile {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl Default for WindowProfile {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
        }
    }
}
