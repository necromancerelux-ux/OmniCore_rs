//! R4/R5 Admin Toolkit: a Discord forum acting as the admin control surface.
//!
//! Every post in the forum is a panel over an existing Omnicore feature
//! (Welcome, Leveling, Tickets, WoS Voting, …). Opening a panel from a post
//! shows the relevant settings hub **ephemerally** to the clicker — the
//! shared forum post is never rewritten by a normal admin action.
//!
//! Elevation: members holding the configured R5 (or R4) Discord role may run
//! Toolkit actions, but only when the interaction originates from a Toolkit
//! post or from an ephemeral hub it opened. Everything else stays behind the
//! normal permission rules.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db;

/// In-memory TTL for ephemeral hub messages spawned from Toolkit posts.
const ELEVATION_TTL_MS: u64 = 30 * 60 * 1000;

fn tracked() -> &'static Mutex<HashMap<u64, u64>> {
    static TRACKED: OnceLock<Mutex<HashMap<u64, u64>>> = OnceLock::new();
    TRACKED.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn track_ephemeral(message_id: u64) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let expiry = now + ELEVATION_TTL_MS;
    let mut guard = tracked().lock().unwrap_or_else(|e| e.into_inner());
    // Prune expired entries while we're in here.
    guard.retain(|_, exp| *exp > now);
    guard.insert(message_id, expiry);
}

pub fn is_tracked(message_id: u64) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    tracked()
        .lock()
        .map(|guard| guard.get(&message_id).is_some_and(|exp| *exp > now))
        .unwrap_or(false)
}

/// Role ids the acting member holds.
pub fn member_role_ids(interaction: &poise::serenity_prelude::ComponentInteraction) -> Vec<u64> {
    interaction
        .member
        .as_ref()
        .map(|m| m.roles.iter().map(|id| id.get()).collect())
        .unwrap_or_default()
}

pub fn modal_role_ids(interaction: &poise::serenity_prelude::ModalInteraction) -> Vec<u64> {
    interaction
        .member
        .as_ref()
        .map(|m| m.roles.iter().map(|id| id.get()).collect())
        .unwrap_or_default()
}

/// Elevated (R4/R5) access for a raw component interaction.
pub async fn elevates_component(
    db: &db::Db,
    interaction: &poise::serenity_prelude::ComponentInteraction,
) -> bool {
    let Some(guild_id) = interaction.guild_id else {
        return false;
    };
    elevates(
        db,
        guild_id.get(),
        interaction.message.id.get(),
        &member_role_ids(interaction),
    )
    .await
}

/// Elevated (R4/R5) access for a raw modal interaction.
pub async fn elevates_modal(
    db: &db::Db,
    interaction: &poise::serenity_prelude::ModalInteraction,
) -> bool {
    let Some(guild_id) = interaction.guild_id else {
        return false;
    };
    let Some(message) = interaction.message.as_ref() else {
        return false;
    };
    elevates(
        db,
        guild_id.get(),
        message.id.get(),
        &modal_role_ids(interaction),
    )
    .await
}

async fn elevates(db: &db::Db, guild_id: u64, message_id: u64, role_ids: &[u64]) -> bool {
    if role_ids.is_empty() {
        return false;
    }
    let config = match db::admin_toolkit::ToolkitConfig::for_guild(db, guild_id).await {
        Ok(Some(config)) => config,
        _ => return false,
    };
    if !config.enabled {
        return false;
    }
    let level = config.matches_role(role_ids);
    if level == db::admin_toolkit::Level::None {
        return false;
    }
    // Elevate on toolkit posts, or on ephemeral hubs spawned from them.
    if is_tracked(message_id) {
        return true;
    }
    matches!(db::admin_toolkit::find_post(db, message_id).await, Ok(Some((post_guild, _))) if post_guild == guild_id)
}
