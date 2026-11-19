//! WoS Oracle integration: session credentials, gift-code discovery and player
//! enrichment.

pub mod client;
pub mod curl;

#[cfg(feature = "browser")]
pub mod browser;

pub use client::{enrichment_from_player, valid_code, OracleClient, OracleConfig, OracleError};
pub use curl::parse as parse_curl;
