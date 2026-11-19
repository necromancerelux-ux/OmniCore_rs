//! Bear Trap / custom notification storage.
//!
//! Shares the legacy tables (`bear_notifications`, `notification_history`)
//! with the Node bot so both runtimes see the same rows. Timestamps stay in
//! ISO-8601 UTC format, matching luxon's `toISO()` output.

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::FromRow;

use super::Db;

/// Minutes before the event time at which each notification type fires.
pub fn slots(notification_type: i64) -> &'static [i64] {
    match notification_type {
        1 => &[30, 10, 5, 0],
        2 => &[10, 5, 0],
        3 => &[5, 0],
        4 => &[5],
        _ => &[0],
    }
}

pub fn parse_iso(raw: &str) -> Option<DateTime<Utc>> {
    // luxon emits milliseconds + Z; chrono's parser accepts both.
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .map(|n| n.and_utc())
                .ok()
        })
}

pub fn to_iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, FromRow)]
pub struct Notification {
    pub id: i64,
    pub guild_id: String,
    pub channel_id: String,
    pub hour: i64,
    pub minute: i64,
    pub timezone: String,
    pub description: String,
    pub notification_type: i64,
    pub mention_type: String,
    pub repeat_enabled: i64,
    pub repeat_minutes: Option<i64>,
    pub is_enabled: i64,
    pub last_notification: Option<String>,
    pub next_notification: Option<String>,
    pub event_type: Option<String>,
    pub custom_delete_delay_minutes: Option<i64>,
}

impl Notification {
    pub fn enabled(&self) -> bool {
        self.is_enabled != 0
    }

    pub fn channel(&self) -> Option<u64> {
        self.channel_id.parse().ok()
    }

    pub fn tz(&self) -> chrono_tz::Tz {
        self.timezone
            .parse::<chrono_tz::Tz>()
            .unwrap_or(chrono_tz::Tz::UTC)
    }

    pub fn mention_prefix(&self) -> &'static str {
        match self.mention_type.as_str() {
            "here" => "@here ",
            "everyone" => "@everyone ",
            _ => "",
        }
    }
}

pub async fn list_for_guild(db: &Db, guild_id: u64) -> sqlx::Result<Vec<Notification>> {
    sqlx::query_as::<_, Notification>(
        "SELECT id, guild_id, channel_id, hour, minute, timezone, description,
                notification_type, mention_type, repeat_enabled, repeat_minutes, is_enabled,
                last_notification, next_notification, event_type, custom_delete_delay_minutes
         FROM bear_notifications WHERE guild_id = ? ORDER BY id",
    )
    .bind(guild_id.to_string())
    .fetch_all(&db.pool)
    .await
}

pub struct NewNotification<'a> {
    pub channel_id: u64,
    pub hour: i64,
    pub minute: i64,
    pub timezone: &'a str,
    pub description: &'a str,
    pub notification_type: i64,
    pub mention_type: &'a str,
    pub repeat_enabled: bool,
    pub repeat_minutes: i64,
    pub created_by: u64,
    pub custom_delete_delay_minutes: Option<i64>,
}

pub async fn create(db: &Db, guild_id: u64, input: NewNotification<'_>) -> sqlx::Result<i64> {
    let tz = input
        .timezone
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::Tz::UTC);
    let next = next_slot(
        slots(input.notification_type),
        input.hour,
        input.minute,
        tz,
        Utc::now(),
    );
    let row = sqlx::query(
        "INSERT INTO bear_notifications
         (guild_id, channel_id, hour, minute, timezone, description, notification_type,
          mention_type, repeat_enabled, repeat_minutes, is_enabled, created_by, next_notification,
          custom_delete_delay_minutes)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)
         RETURNING id",
    )
    .bind(guild_id.to_string())
    .bind(input.channel_id.to_string())
    .bind(input.hour)
    .bind(input.minute)
    .bind(tz.name())
    .bind(input.description)
    .bind(input.notification_type)
    .bind(input.mention_type)
    .bind(if input.repeat_enabled { 1 } else { 0 })
    .bind(input.repeat_minutes)
    .bind(input.created_by.to_string())
    .bind(next.map(to_iso))
    .bind(input.custom_delete_delay_minutes)
    .fetch_one(&db.pool)
    .await?;
    use sqlx::Row;
    Ok(row.get(0))
}

/// Next send instant at or after `from`, given the event's wall-clock time
/// (`hour:minute` in `tz`) and the set of minute-before slots.
pub fn next_slot(
    slot_minutes: &[i64],
    hour: i64,
    minute: i64,
    tz: chrono_tz::Tz,
    from: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    use chrono::{Duration, TimeZone};

    // Look at today and tomorrow's occurrence — a slot for "today" might have
    // already passed.
    for day_offset in 0..=1 {
        let today = from.with_timezone(&tz).date_naive() + Duration::days(day_offset);
        let Some(local) = today.and_hms_opt(hour as u32, minute as u32, 0) else {
            continue;
        };
        let event_tz = match tz.from_local_datetime(&local) {
            chrono::LocalResult::Single(dt) => dt,
            chrono::LocalResult::Ambiguous(_, second) => second,
            chrono::LocalResult::None => continue, // nonexistent local time (DST gap) — skip today
        };
        let event_utc = event_tz.with_timezone(&Utc);

        for slot in slot_minutes {
            let send_at = event_utc - Duration::minutes(*slot);
            if send_at >= from {
                return Some(send_at);
            }
        }
    }
    None
}

pub async fn count(db: &Db, guild_id: u64) -> sqlx::Result<i64> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM bear_notifications WHERE guild_id = ?")
        .bind(guild_id.to_string())
        .fetch_one(&db.pool)
        .await?;
    Ok(row.0)
}

pub async fn set_enabled(db: &Db, id: i64, enabled: bool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE bear_notifications SET is_enabled = ?, auto_disabled_at = NULL WHERE id = ?",
    )
    .bind(if enabled { 1 } else { 0 })
    .bind(id)
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn auto_disable(db: &Db, id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE bear_notifications SET is_enabled = 0, auto_disabled_at = datetime('now') WHERE id = ?",
    )
    .bind(id)
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn delete(db: &Db, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM bear_notifications WHERE id = ?")
        .bind(id)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Rows whose next send time has arrived (or whose `next_notification` was
/// never computed).
pub async fn due(db: &Db, now: DateTime<Utc>) -> sqlx::Result<Vec<Notification>> {
    sqlx::query_as::<_, Notification>(
        "SELECT id, guild_id, channel_id, hour, minute, timezone, description,
                notification_type, mention_type, repeat_enabled, repeat_minutes, is_enabled,
                last_notification, next_notification, event_type, custom_delete_delay_minutes
         FROM bear_notifications
         WHERE is_enabled = 1 AND (next_notification IS NULL OR next_notification <= ?)",
    )
    .bind(to_iso(now))
    .fetch_all(&db.pool)
    .await
}

/// Advance `next_notification` after a successful send of `slot`
/// (minutes-before-event, 0 = final slot).
pub async fn advance(
    db: &Db,
    notification: &Notification,
    slot: i64,
    just_sent: DateTime<Utc>,
) -> sqlx::Result<()> {
    let tz = notification.tz();
    let now = Utc::now();
    let slots = slots(notification.notification_type);

    // If repeating on an interval, just schedule the next interval tick.
    let repeat_minutes = notification.repeat_minutes.unwrap_or(0);
    let new_next = if notification.repeat_enabled == 1 && repeat_minutes > 0 {
        Some(now + chrono::Duration::minutes(repeat_minutes))
    } else {
        next_slot(slots, notification.hour, notification.minute, tz, now)
    };
    // One-shot: after the final (0-minute) slot fires, disable.
    let disable = notification.repeat_enabled != 1 && slot == 0;

    if disable {
        sqlx::query(
            "UPDATE bear_notifications SET is_enabled = 0, last_notification = ? WHERE id = ?",
        )
        .bind(to_iso(just_sent))
        .bind(notification.id)
        .execute(&db.pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE bear_notifications SET last_notification = ?, next_notification = ? WHERE id = ?",
        )
        .bind(to_iso(just_sent))
        .bind(new_next.map(to_iso))
        .bind(notification.id)
        .execute(&db.pool)
        .await?;
    }
    Ok(())
}

pub async fn record_history(
    db: &Db,
    notification_id: i64,
    scheduled_at: DateTime<Utc>,
    message_id: Option<u64>,
    channel_id: Option<u64>,
    delete_delay_minutes: Option<i64>,
) -> sqlx::Result<()> {
    let scheduled =
        delete_delay_minutes.map(|mins| to_iso(scheduled_at + chrono::Duration::minutes(mins)));
    sqlx::query(
        "INSERT INTO notification_history
         (notification_id, notification_time, sent_at, message_id, channel_id, scheduled_delete_at)
         VALUES (?, ?, datetime('now'), ?, ?, ?)",
    )
    .bind(notification_id)
    .bind(scheduled_at.timestamp())
    .bind(message_id.map(|id| id.to_string()))
    .bind(channel_id.map(|id| id.to_string()))
    .bind(scheduled)
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Messages due for deletion (scheduled_delete_at passed, not yet deleted).
pub async fn due_deletions(
    db: &Db,
    now: DateTime<Utc>,
) -> sqlx::Result<Vec<(i64, String, String)>> {
    // (history_id, channel_id, message_id)
    let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, channel_id, message_id FROM notification_history
         WHERE scheduled_delete_at IS NOT NULL AND deleted_at IS NULL AND scheduled_delete_at <= ?",
    )
    .bind(to_iso(now))
    .fetch_all(&db.pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, channel_id, message_id)| Some((id, channel_id?, message_id?)))
        .collect())
}

pub async fn mark_deleted(db: &Db, history_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE notification_history SET deleted_at = datetime('now') WHERE id = ?")
        .bind(history_id)
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn slots_types() {
        assert_eq!(slots(1), &[30, 10, 5, 0]);
        assert_eq!(slots(2), &[10, 5, 0]);
        assert_eq!(slots(5), &[0]);
    }

    #[test]
    fn next_slot_picks_earliest_future_slot() {
        let tz = chrono_tz::Tz::UTC;
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 11, 50, 0).unwrap();
        let next = next_slot(&[10, 5, 0], 12, 0, tz, now).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 11, 19, 11, 50, 0).unwrap());
    }

    #[test]
    fn next_slot_rolls_to_tomorrow_when_all_slots_passed() {
        let tz = chrono_tz::Tz::UTC;
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 12, 30, 0).unwrap();
        let next = next_slot(&[10, 5, 0], 12, 0, tz, now).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 11, 20, 11, 50, 0).unwrap());
    }

    #[test]
    fn iso_roundtrip() {
        let now = Utc::now();
        let s = to_iso(now);
        assert!(s.ends_with('Z'));
        assert!(parse_iso(&s).is_some());
    }
}
