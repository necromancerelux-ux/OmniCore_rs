//! `gift_codes` + `redemptions` tables.

use sqlx::FromRow;

use super::{now, Db};

#[derive(Debug, Clone, FromRow)]
pub struct GiftCode {
    pub code: String,
    pub status: String,
    pub added_by: String,
    pub source_guild_id: String,
    pub added_at: i64,
    pub date: Option<String>,
    pub source: Option<String>,
    pub api_pushed: i64,
    pub is_vip: i64,
    pub last_validated: Option<String>,
    pub announced: i64,
}

impl GiftCode {
    pub fn is_active(&self) -> bool {
        self.status.eq_ignore_ascii_case("active")
    }

    pub fn is_vip(&self) -> bool {
        self.is_vip != 0
    }

    pub fn source_label(&self) -> &str {
        self.source.as_deref().unwrap_or("manual")
    }
}

#[derive(Debug, Clone, Default)]
pub struct GiftCodeStats {
    pub total: i64,
    pub active: i64,
    pub vip: i64,
    pub today: i64,
}

/// Insert a code. Returns `true` when it was new.
pub async fn insert(
    db: &Db,
    code: &str,
    added_by: u64,
    guild_id: u64,
    source: &str,
    date: Option<&str>,
    vip: bool,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "INSERT INTO gift_codes (code, status, added_by, source_guild_id, added_at, date, source, is_vip)
         VALUES (?, 'active', ?, ?, ?, ?, ?, ?)
         ON CONFLICT(code) DO NOTHING",
    )
    .bind(code)
    .bind(added_by.to_string())
    .bind(guild_id.to_string())
    .bind(now())
    .bind(date)
    .bind(source)
    .bind(vip as i64)
    .execute(&db.pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Bulk-insert codes discovered by the Oracle sync, returning the ones that
/// were actually new so they can be announced.
pub async fn insert_many(
    db: &Db,
    codes: &[String],
    guild_id: u64,
    source: &str,
) -> sqlx::Result<Vec<String>> {
    let mut fresh = Vec::new();
    let mut tx = db.pool.begin().await?;
    for code in codes {
        let result = sqlx::query(
            "INSERT INTO gift_codes (code, status, added_by, source_guild_id, added_at, source)
             VALUES (?, 'active', 'oracle', ?, ?, ?)
             ON CONFLICT(code) DO NOTHING",
        )
        .bind(code)
        .bind(guild_id.to_string())
        .bind(now())
        .bind(source)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() > 0 {
            fresh.push(code.clone());
        }
    }
    tx.commit().await?;
    Ok(fresh)
}

pub async fn get(db: &Db, code: &str) -> sqlx::Result<Option<GiftCode>> {
    sqlx::query_as::<_, GiftCode>("SELECT * FROM gift_codes WHERE code = ?")
        .bind(code)
        .fetch_optional(&db.pool)
        .await
}

pub async fn list_active(db: &Db, limit: i64) -> sqlx::Result<Vec<GiftCode>> {
    sqlx::query_as::<_, GiftCode>(
        "SELECT * FROM gift_codes WHERE status = 'active' ORDER BY added_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&db.pool)
    .await
}

pub async fn list_recent(db: &Db, limit: i64) -> sqlx::Result<Vec<GiftCode>> {
    sqlx::query_as::<_, GiftCode>("SELECT * FROM gift_codes ORDER BY added_at DESC LIMIT ?")
        .bind(limit)
        .fetch_all(&db.pool)
        .await
}

pub async fn search(db: &Db, needle: &str, limit: i64) -> sqlx::Result<Vec<GiftCode>> {
    let escaped = needle
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    sqlx::query_as::<_, GiftCode>(
        "SELECT * FROM gift_codes WHERE code LIKE ? ESCAPE '\\' ORDER BY added_at DESC LIMIT ?",
    )
    .bind(pattern)
    .bind(limit)
    .fetch_all(&db.pool)
    .await
}

pub async fn set_status(db: &Db, code: &str, status: &str) -> sqlx::Result<bool> {
    let result = sqlx::query("UPDATE gift_codes SET status = ?, last_validated = ? WHERE code = ?")
        .bind(status)
        .bind(now().to_string())
        .bind(code)
        .execute(&db.pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete(db: &Db, code: &str) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM gift_codes WHERE code = ?")
        .bind(code)
        .execute(&db.pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_announced(db: &Db, code: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE gift_codes SET announced = 1 WHERE code = ?")
        .bind(code)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Codes added since the last announcement, oldest first.
pub async fn unannounced(db: &Db, limit: i64) -> sqlx::Result<Vec<GiftCode>> {
    sqlx::query_as::<_, GiftCode>(
        "SELECT * FROM gift_codes WHERE announced = 0 ORDER BY added_at ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&db.pool)
    .await
}

pub async fn stats(db: &Db) -> sqlx::Result<GiftCodeStats> {
    let row: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            COUNT(*),
            COALESCE(SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(is_vip), 0),
            COALESCE(SUM(CASE WHEN added_at >= strftime('%s','now') - 86400 THEN 1 ELSE 0 END), 0)
         FROM gift_codes",
    )
    .fetch_one(&db.pool)
    .await?;

    Ok(GiftCodeStats {
        total: row.0,
        active: row.1,
        vip: row.2,
        today: row.3,
    })
}

/// Record a redemption attempt for a tracked player.
pub async fn record_redemption(db: &Db, fid: &str, code: &str, status: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO redemptions (fid, code, status, redeemed_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(fid, code) DO UPDATE SET status = excluded.status, redeemed_at = excluded.redeemed_at",
    )
    .bind(fid)
    .bind(code)
    .bind(status)
    .bind(now())
    .execute(&db.pool)
    .await?;
    Ok(())
}
