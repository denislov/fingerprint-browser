use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", content = "url", rename_all = "snake_case")]
pub enum StartTarget {
    #[default]
    Blank,
    Url(String),
}
