//! Gift-code synchronisation shared by commands, buttons and the background job.

use poise::serenity_prelude as serenity;

use crate::db::{self, settings::keys};
use crate::state::Data;
use crate::ui::panels;

#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    /// How many active codes Oracle returned.
    pub total: usize,
    /// Codes that were not in the database before this run.
    pub fresh: Vec<String>,
    /// How many channels the new codes were posted to.
    pub announcements: usize,
}

/// Pull the active code list from Oracle, persist it, expire codes that
/// disappeared, and announce anything new.
pub async fn sync(data: &Data) -> anyhow::Result<SyncOutcome> {
    let rows = data
        .oracle
        .fetch_codes()
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    let mut active = Vec::new();
    for row in &rows {
        if row.active {
            active.push(row.code.clone());
        } else if let Err(err) = db::giftcodes::set_status(&data.db, &row.code, "expired").await {
            tracing::debug!(error = %err, code = %row.code, "marking gift code expired failed");
        }
    }

    let total = active.len();
    let fresh = db::giftcodes::insert_many(&data.db, &active, 0, "oracle").await?;
    db::oracle::touch_codes_sync(&data.db, total as i64).await?;

    // Announce everything still unannounced (not just this run's new codes), so a code whose
    // first announcement reached no channel is retried on the next sync. The 7-day window
    // keeps an old backlog from flooding a channel that was only just configured.
    let cutoff = db::now() - 7 * 24 * 3600;
    let pending: Vec<String> = db::giftcodes::unannounced(&data.db, 50)
        .await?
        .into_iter()
        .filter(|row| row.is_active() && row.added_at >= cutoff)
        .map(|row| row.code)
        .collect();
    let announcements = if pending.is_empty() {
        0
    } else {
        match announce(data, &pending).await {
            Ok(delivered) => delivered,
            Err(err) => {
                tracing::warn!(error = %err, "gift-code announcement failed; will retry next sync");
                0
            }
        }
    };

    Ok(SyncOutcome {
        total,
        fresh,
        announcements,
    })
}

/// Post `codes` into every guild that configured an announcement channel.
pub async fn announce(data: &Data, codes: &[String]) -> anyhow::Result<usize> {
    if codes.is_empty() {
        return Ok(0);
    }

    let channels = db::settings::guilds_with_key(&data.db, keys::GIFT_ANNOUNCE_CHANNEL).await?;
    // Owner-set gift priority: lower number first; unconfigured guilds last.
    let mut channels = channels;
    let guild_ids: Vec<u64> = channels.iter().map(|(guild, _)| *guild).collect();
    let guild_ids = db::subscriptions::order_guilds_by_gift_priority(&data.db, guild_ids).await;
    let priority: std::collections::HashMap<u64, usize> = guild_ids
        .iter()
        .enumerate()
        .map(|(index, guild)| (*guild, index))
        .collect();
    channels.sort_by_key(|(guild, _)| priority.get(guild).copied().unwrap_or(usize::MAX));
    let embed = panels::giftcode_announce(codes);
    let mut delivered = 0usize;

    for (_guild, raw) in &channels {
        let Ok(channel_id) = raw.parse::<u64>() else {
            continue;
        };
        match serenity::ChannelId::new(channel_id)
            .send_message(
                &data.discord_http,
                serenity::CreateMessage::new().embed(embed.clone()),
            )
            .await
        {
            Ok(_) => delivered += 1,
            Err(err) => {
                tracing::warn!(error = %err, channel = channel_id, "gift-code announcement failed")
            }
        }
    }

    if delivered > 0 {
        for code in codes {
            let _ = db::giftcodes::mark_announced(&data.db, code).await;
        }
    }

    Ok(delivered)
}

/// Post every code that has never been announced (used by the Gift Codes panel's Announce button).
pub async fn announce_pending(data: &Data, limit: i64) -> anyhow::Result<usize> {
    let pending = db::giftcodes::unannounced(&data.db, limit).await?;
    let codes = pending.into_iter().map(|row| row.code).collect::<Vec<_>>();
    announce(data, &codes).await
}
