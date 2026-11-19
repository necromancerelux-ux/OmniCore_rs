//! `oracle_credentials` + `oracle_state` tables.

use sqlx::FromRow;

use super::{now, Db};

#[derive(Debug, Clone, FromRow)]
pub struct OracleCredentials {
    pub js_auth: String,
    pub api_token: String,
    pub extra_cookie: Option<String>,
    pub updated_at: i64,
    pub updated_by: Option<String>,
    pub last_test_at: Option<i64>,
    pub last_test_ok: Option<i64>,
    pub last_test_message: Option<String>,
}

impl OracleCredentials {
    /// Build the `Cookie` header value from the stored pieces.
    pub fn cookie_header(&self) -> String {
        let mut parts = Vec::new();
        let js_auth = self.js_auth.trim();
        if !js_auth.is_empty() {
            if js_auth.to_ascii_lowercase().starts_with("js_auth=") {
                parts.push(js_auth.to_string());
            } else {
                parts.push(format!("js_auth={js_auth}"));
            }
        }
        if let Some(extra) = self.extra_cookie.as_deref().map(str::trim) {
            if !extra.is_empty() {
                parts.push(extra.trim_end_matches(';').to_string());
            }
        }
        parts.join("; ")
    }

    pub fn api_token(&self) -> &str {
        self.api_token.trim()
    }

    pub fn last_test_ok(&self) -> Option<bool> {
        self.last_test_ok.map(|v| v != 0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct OracleSyncState {
    pub last_codes_sync: Option<i64>,
    pub last_lookup_sync: Option<i64>,
    pub last_error: Option<String>,
    pub total_codes: i64,
    pub total_players: i64,
}

pub async fn credentials(db: &Db) -> sqlx::Result<Option<OracleCredentials>> {
    let row = sqlx::query_as::<_, OracleCredentials>(
        "SELECT js_auth, api_token, extra_cookie, updated_at, updated_by,
                last_test_at, last_test_ok, last_test_message
         FROM oracle_credentials WHERE id = 1",
    )
    .fetch_optional(&db.pool)
    .await?;
    // Secrets are encrypted at rest (see `crate::secrets`); an undecryptable row reads as "not configured".
    Ok(row.and_then(|mut creds| {
        creds.js_auth = crate::secrets::open(&creds.js_auth)?;
        creds.api_token = crate::secrets::open(&creds.api_token)?;
        creds.extra_cookie = crate::secrets::open_opt(creds.extra_cookie);
        Some(creds)
    }))
}

pub async fn save_credentials(
    db: &Db,
    js_auth: &str,
    api_token: &str,
    extra_cookie: Option<&str>,
    updated_by: Option<u64>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO oracle_credentials (id, js_auth, api_token, extra_cookie, updated_at, updated_by)
         VALUES (1, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             js_auth = excluded.js_auth,
             api_token = excluded.api_token,
             extra_cookie = excluded.extra_cookie,
             updated_at = excluded.updated_at,
             updated_by = excluded.updated_by,
             last_test_at = NULL,
             last_test_ok = NULL,
             last_test_message = NULL",
    )
    .bind(crate::secrets::seal(js_auth.trim()))
    .bind(crate::secrets::seal(api_token.trim()))
    .bind(crate::secrets::seal_opt(
        extra_cookie.map(str::trim).filter(|v| !v.is_empty()),
    ))
    .bind(now())
    .bind(updated_by.map(|id| id.to_string()))
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn delete_credentials(db: &Db) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM oracle_credentials WHERE id = 1")
        .execute(&db.pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Internal row shape for the single-row `oracle_state` table. Kept private so
/// callers only ever see the public `OracleSyncState`.
#[derive(sqlx::FromRow)]
struct OracleStateRow {
    last_codes_sync: Option<i64>,
    last_lookup_sync: Option<i64>,
    last_error: Option<String>,
    total_codes: i64,
    total_players: i64,
}

pub async fn record_test(db: &Db, ok: bool, message: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE oracle_credentials
         SET last_test_at = ?, last_test_ok = ?, last_test_message = ?
         WHERE id = 1",
    )
    .bind(now())
    .bind(ok as i64)
    .bind(message)
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn state(db: &Db) -> sqlx::Result<OracleSyncState> {
    let row: Option<OracleStateRow> = sqlx::query_as(
        "SELECT last_codes_sync, last_lookup_sync, last_error, total_codes, total_players
         FROM oracle_state WHERE id = 1",
    )
    .fetch_optional(&db.pool)
    .await?;

    Ok(row
        .map(|row| OracleSyncState {
            last_codes_sync: row.last_codes_sync,
            last_lookup_sync: row.last_lookup_sync,
            last_error: row.last_error,
            total_codes: row.total_codes,
            total_players: row.total_players,
        })
        .unwrap_or_default())
}

pub async fn touch_codes_sync(db: &Db, total: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO oracle_state (id, last_codes_sync, total_codes) VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             last_codes_sync = excluded.last_codes_sync,
             total_codes = excluded.total_codes,
             last_error = NULL",
    )
    .bind(now())
    .bind(total)
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn touch_lookup_sync(db: &Db, total: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO oracle_state (id, last_lookup_sync, total_players) VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             last_lookup_sync = excluded.last_lookup_sync,
             total_players = excluded.total_players,
             last_error = NULL",
    )
    .bind(now())
    .bind(total)
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn record_error(db: &Db, message: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO oracle_state (id, last_error) VALUES (1, ?)
         ON CONFLICT(id) DO UPDATE SET last_error = excluded.last_error",
    )
    .bind(message)
    .execute(&db.pool)
    .await?;
    Ok(())
}
