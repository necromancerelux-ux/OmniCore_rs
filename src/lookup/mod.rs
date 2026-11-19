//! Whiteout Stats lookup client.
//!
//! Talks to the public site API (`https://www.whiteoutstats.com/api`) first and
//! falls back to the keyed API (`https://api.whiteoutstats.com/v1`) when a key
//! is configured. The site sits behind Cloudflare, so we send a browser-like
//! `User-Agent`/`Referer` pair.

mod client;
mod types;

pub use client::LookupClient;
pub use types::PlayerProfile;
