//! Notification engine: scheduled channel posts (Bear Trap / custom).
//!
//! Every notification is a row in `bear_notifications`: a wall-clock time in
//! an IANA timezone, a set of minute-before slots for delivery, and an
//! optional repeat. A 30-second sweep posts due notifications and advances
//! `next_notification`; a 60-second sweep deletes messages whose retention
//! window has expired.

use chrono::{DateTime, Duration, Utc};
use poise::serenity_prelude as serenity;

use crate::state::Data;

pub async fn check(ctx: &serenity::Context, data: &Data) -> anyhow::Result<usize> {
    let now = Utc::now();
    let due = crate::db::notifications::due(&data.db, now).await?;
    let mut sent = 0usize;

    for notification in due {
        let Some(channel_id) = notification.channel() else {
            // Corrupt row — auto-disable so we don't spam the logs forever.
            let _ = crate::db::notifications::auto_disable(&data.db, notification.id).await;
            continue;
        };
        // The slot that this send represents.
        let slot = slot_for(
            now,
            notification.hour,
            notification.minute,
            notification.tz(),
            notification.notification_type,
        );

        // Expand placeholders: %t = human "in N minutes", %e = 🔔, %n = description.
        let minutes = slot.max(0);
        let time_phrase = match minutes {
            0 => "now".to_string(),
            1 => "in 1 minute".to_string(),
            n => format!("in {n} minutes"),
        };
        let body = notification
            .description
            .replace("%t", &time_phrase)
            .replace("%e", "🔔")
            .replace("%n", "");

        let mention = notification.mention_prefix().to_string();
        let content = if mention.is_empty() {
            None
        } else {
            Some(format!("{mention}{time_phrase}"))
        };
        let embed = serenity::CreateEmbed::new()
            .title("🔔 Reminder")
            .description(body)
            .color(crate::ui::theme::colors::BRAND)
            .footer(serenity::CreateEmbedFooter::new(crate::ui::theme::footer()));

        let mut builder = serenity::CreateMessage::new().embed(embed);
        if let Some(content) = content {
            builder = builder.content(content);
        }
        builder = builder.allowed_mentions(serenity::CreateAllowedMentions::new().everyone(
            matches!(notification.mention_type.as_str(), "here" | "everyone"),
        ));

        match serenity::ChannelId::new(channel_id)
            .send_message(&ctx.http, builder)
            .await
        {
            Ok(message) => {
                sent += 1;
                let scheduled_trigger = notification
                    .next_notification
                    .as_deref()
                    .and_then(crate::db::notifications::parse_iso)
                    .unwrap_or(now);
                let _ = crate::db::notifications::record_history(
                    &data.db,
                    notification.id,
                    scheduled_trigger,
                    Some(message.id.get()),
                    Some(channel_id),
                    notification.custom_delete_delay_minutes,
                )
                .await;
                let _ = crate::db::notifications::advance(&data.db, &notification, slot, now).await;
            }
            Err(err) => {
                tracing::warn!(error = %err, channel = channel_id, "notification send failed");
                // After repeated failures, quarantine the row so it stops
                // retrying forever (equals legacy's 3-strike rule).
                let failures = failure_marker(&data.db, notification.id).await;
                if failures >= 3 {
                    let _ = crate::db::notifications::auto_disable(&data.db, notification.id).await;
                    tracing::error!(
                        id = notification.id,
                        channel = channel_id,
                        "notification auto-disabled after repeated failures"
                    );
                }
            }
        }
    }
    Ok(sent)
}

/// Delete messages whose retention window expired.
pub async fn sweep_deletions(ctx: &serenity::Context, data: &Data) -> anyhow::Result<usize> {
    let now = Utc::now();
    let due = crate::db::notifications::due_deletions(&data.db, now).await?;
    let mut deleted = 0usize;

    for (history_id, channel_id, message_id) in due {
        let Ok(channel) = channel_id.parse::<u64>() else {
            let _ = crate::db::notifications::mark_deleted(&data.db, history_id).await;
            continue;
        };
        let Ok(message) = message_id.parse::<u64>() else {
            let _ = crate::db::notifications::mark_deleted(&data.db, history_id).await;
            continue;
        };
        match serenity::ChannelId::new(channel)
            .delete_message(&ctx.http, serenity::MessageId::new(message))
            .await
        {
            Ok(_) | Err(_) => {
                // Even a 404 counts as deleted — the row would fail forever otherwise.
                let _ = crate::db::notifications::mark_deleted(&data.db, history_id).await;
                deleted += 1;
            }
        }
    }
    Ok(deleted)
}

/// In-memory strike counter for send failures (per notification, per process).
async fn failure_marker(_db: &crate::db::Db, id: i64) -> i64 {
    use std::sync::OnceLock;
    static STRIKES: OnceLock<tokio::sync::Mutex<std::collections::HashMap<i64, i64>>> =
        OnceLock::new();
    let strikes = STRIKES.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = strikes.lock().await;
    let entry = guard.entry(id).or_insert(0);
    *entry += 1;
    *entry
}

/// Which slot fired, given the trigger time (used for one-shot disabling).
fn slot_for(
    trigger: DateTime<Utc>,
    hour: i64,
    minute: i64,
    tz: chrono_tz::Tz,
    notification_type: i64,
) -> i64 {
    use chrono::Timelike;
    for slot in crate::db::notifications::slots(notification_type) {
        let event = trigger + Duration::minutes(*slot);
        let local = event.with_timezone(&tz);
        if local.hour() as i64 == hour && local.minute() as i64 == minute {
            return *slot;
        }
    }
    0
}

/// Human-readable summary for the settings panel.
pub fn describe(notification: &crate::db::notifications::Notification) -> String {
    let tz = notification.timezone.replace("_", " ");
    let slots = crate::db::notifications::slots(notification.notification_type);
    let slot_text: Vec<String> = slots
        .iter()
        .map(|s| {
            if *s == 0 {
                "at event".to_string()
            } else {
                format!("{s}m before")
            }
        })
        .collect();
    format!(
        "`{}` · **{:02}:{:02}** `{tz}` · {} · mention `{}` · {} · {}",
        notification.id,
        notification.hour,
        notification.minute,
        slot_text.join(", "),
        notification.mention_type,
        if notification.repeat_enabled == 1 {
            "repeats"
        } else {
            "one-shot"
        },
        if notification.is_enabled == 1 {
            "✅"
        } else {
            "⏸️"
        },
    )
}
