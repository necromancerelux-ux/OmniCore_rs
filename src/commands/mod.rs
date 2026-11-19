//! Slash commands and the component/modal router.
//!
//! `/settings` opens the button-panel hub; `/play`, `/volume`, `/rank` and
//! `/leaderboard` are the four everyday commands people type.
pub mod extras;
pub mod giveaways;
pub mod interactions;
pub mod lookup;
pub mod oracle;
pub mod settings;

use crate::state::Data;

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// Every registered slash command.
pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![
        settings::settings(),
        extras::play(),
        extras::volume(),
        extras::rank(),
        extras::leaderboard(),
    ]
}
