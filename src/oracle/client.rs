//! Oracle HTTP client with automatic session renewal.
//!
//! Credentials are resolved fresh on every request (DB first, then `.env`) so
//! that a renewed session is picked up by background jobs immediately.

use std::time::Duration;

use serde_json::Value;

use crate::db::{self, oracle::OracleCredentials, Db};

use super::curl::ParsedCurl;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// Where the credentials came from, for log lines and the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Database,
    Environment,
}

impl CredentialSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Database => "db",
            Self::Environment => "env",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedCredentials {
    pub js_auth: String,
    pub api_token: String,
    pub extra_cookie: Option<String>,
    pub source: CredentialSource,
}

impl ResolvedCredentials {
    pub fn cookie_header(&self) -> String {
        let mut parts = vec![format!("js_auth={}", self.js_auth.trim())];
        if let Some(extra) = self.extra_cookie.as_deref().map(str::trim) {
            if !extra.is_empty() {
                parts.push(extra.trim_end_matches(';').to_string());
            }
        }
        parts.join("; ")
    }

    fn from_row(row: &OracleCredentials) -> Self {
        Self {
            js_auth: row.js_auth.clone(),
            api_token: row.api_token.clone(),
            extra_cookie: row.extra_cookie.clone(),
            source: CredentialSource::Database,
        }
    }

    fn from_env() -> Option<Self> {
        let js_auth = std::env::var("ORACLE_JS_AUTH").ok()?.trim().to_string();
        let api_token = std::env::var("ORACLE_API_TOKEN").ok()?.trim().to_string();
        if js_auth.is_empty() || api_token.is_empty() {
            return None;
        }
        Some(Self {
            js_auth,
            api_token,
            extra_cookie: std::env::var("ORACLE_EXTRA_COOKIE")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            source: CredentialSource::Environment,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    #[error("Oracle is not connected — paste a cURL snippet or run the browser capture")]
    NotConfigured,
    #[error("Oracle session expired (HTTP {0})")]
    Expired(u16),
    #[error("Oracle returned HTTP {0}")]
    Http(u16),
    #[error("Oracle returned an unexpected body")]
    Malformed,
    #[error("network error: {0}")]
    Network(String),
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for OracleError {
    fn from(value: reqwest::Error) -> Self {
        if value.is_timeout() {
            Self::Network("request timed out".to_string())
        } else {
            Self::Network(value.to_string())
        }
    }
}

/// One row from `/api/codes`.
#[derive(Debug, Clone)]
pub struct CodeRow {
    pub code: String,
    pub date: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct OracleConfig {
    pub site: String,
    pub codes_url: String,
    pub browser_enabled: bool,
    pub browser_headless: bool,
    pub browser_profile: std::path::PathBuf,
    pub chromium_path: Option<String>,
}

/// Cheap-to-clone Oracle access layer.
#[derive(Clone)]
pub struct OracleClient {
    http: reqwest::Client,
    pub db: Db,
    pub cfg: OracleConfig,
    /// Serialises session renewals so two jobs cannot launch two browsers.
    refresh_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl OracleClient {
    pub fn new(db: Db, cfg: OracleConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .gzip(true)
            .brotli(true)
            .build()?;
        Ok(Self {
            http,
            db,
            cfg,
            refresh_lock: Default::default(),
        })
    }

    // ------------------------------------------------------------ credentials

    /// DB credentials, then `.env`. Never cached.
    pub async fn resolve(&self) -> Option<ResolvedCredentials> {
        match db::oracle::credentials(&self.db).await {
            Ok(Some(row)) if !row.js_auth.trim().is_empty() && !row.api_token.trim().is_empty() => {
                Some(ResolvedCredentials::from_row(&row))
            }
            Ok(_) => ResolvedCredentials::from_env(),
            Err(err) => {
                tracing::warn!(error = %err, "reading oracle credentials failed; falling back to env");
                ResolvedCredentials::from_env()
            }
        }
    }

    pub async fn is_configured(&self) -> bool {
        self.resolve().await.is_some()
    }

    pub async fn save(&self, parsed: &ParsedCurl, updated_by: Option<u64>) -> anyhow::Result<()> {
        db::oracle::save_credentials(
            &self.db,
            &parsed.js_auth,
            &parsed.api_token,
            parsed.extra_cookie_ref(),
            updated_by,
        )
        .await?;
        Ok(())
    }

    pub async fn forget(&self) -> anyhow::Result<bool> {
        Ok(db::oracle::delete_credentials(&self.db).await?)
    }

    // -------------------------------------------------------------- transport

    fn codes_url(&self) -> &str {
        &self.cfg.codes_url
    }

    fn api_url(&self, path: &str) -> String {
        let base = self.cfg.site.trim_end_matches('/');
        if path.starts_with('/') {
            format!("{base}{path}")
        } else {
            format!("{base}/{path}")
        }
    }

    /// One authenticated GET. Returns the raw body text so we can log shape
    /// errors accurately.
    async fn once(
        &self,
        url: &str,
        creds: &ResolvedCredentials,
    ) -> Result<(u16, String), OracleError> {
        let response = self
            .http
            .get(url)
            .header("accept", "*/*")
            .header("accept-language", "en-US,en;q=0.8")
            .header(
                "referer",
                format!("{}/giftcodes", self.cfg.site.trim_end_matches('/')),
            )
            .header("cookie", creds.cookie_header())
            .header("x-api-token", creds.api_token.trim())
            .send()
            .await?;

        let status = response.status().as_u16();
        let text = response.text().await?;
        Ok((status, text))
    }

    /// GET with one session renewal on 401/403.
    async fn get(&self, url: &str) -> Result<Value, OracleError> {
        let creds = self.resolve().await.ok_or(OracleError::NotConfigured)?;
        let (status, text) = self.once(url, &creds).await?;

        if status == 401 || status == 403 {
            tracing::warn!(status, "oracle session rejected; attempting renewal");
            if self.refresh_session().await.is_ok() {
                let creds = self.resolve().await.ok_or(OracleError::NotConfigured)?;
                let (status, text) = self.once(url, &creds).await?;
                if status == 401 || status == 403 {
                    return Err(OracleError::Expired(status));
                }
                return decode(status, text);
            }
            return Err(OracleError::Expired(status));
        }

        decode(status, text)
    }

    // ---------------------------------------------------------------- payloads

    /// Active gift codes from `wosoracle.com/api/codes`.
    pub async fn fetch_codes(&self) -> Result<Vec<CodeRow>, OracleError> {
        let value = self.get(self.codes_url()).await?;
        let rows = value
            .as_array()
            .cloned()
            .or_else(|| {
                ["codes", "data", "items", "results"]
                    .iter()
                    .find_map(|key| value.get(*key).and_then(Value::as_array).cloned())
            })
            .ok_or(OracleError::Malformed)?;

        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let raw = row
                .get("Code")
                .or_else(|| row.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if !valid_code(&raw) || !seen.insert(raw.clone()) {
                continue;
            }
            let active = row
                .get("IsActive")
                .or_else(|| row.get("isActive"))
                .or_else(|| row.get("is_active"))
                .or_else(|| row.get("active"))
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    let status = row
                        .get("Status")
                        .or_else(|| row.get("status"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    !matches!(status.as_str(), "expired" | "inactive" | "invalid")
                });
            let date = row
                .get("DiscoveredAt")
                .or_else(|| row.get("discoveredAt"))
                .or_else(|| row.get("discovered_at"))
                .and_then(Value::as_str)
                .filter(|d| d.len() >= 10)
                .map(|d| d[..10].to_string());
            out.push(CodeRow {
                code: raw,
                date,
                active,
            });
            if out.len() >= 200 {
                break;
            }
        }
        Ok(out)
    }

    /// Single player profile: `/api/players/{fid}`.
    pub async fn fetch_player(&self, fid: &str) -> Result<Value, OracleError> {
        let path = format!("/api/players/{}", urlencoding::encode(fid));
        self.get(&self.api_url(&path)).await
    }

    /// Player loadout (gear, heroes): `/api/players/{fid}/loadout`.
    pub async fn fetch_loadout(&self, fid: &str) -> Result<Value, OracleError> {
        let path = format!("/api/players/{}/loadout?cached=1", urlencoding::encode(fid));
        self.get(&self.api_url(&path)).await
    }

    /// State overview: `/api/kingdoms/{kid}`.
    pub async fn fetch_state(&self, kid: i64) -> Result<Value, OracleError> {
        self.get(&self.api_url(&format!("/api/kingdoms/{kid}")))
            .await
    }

    /// Lightweight auth probe.
    pub async fn test(&self) -> Result<String, OracleError> {
        if self.resolve().await.is_none() {
            return Err(OracleError::NotConfigured);
        }
        let url = self.api_url("/api/config");
        let creds = self.resolve().await.ok_or(OracleError::NotConfigured)?;
        let (status, _) = self.once(&url, &creds).await?;
        match status {
            200 => Ok("session accepted".to_string()),
            401 | 403 => Err(OracleError::Expired(status)),
            other => Err(OracleError::Http(other)),
        }
    }

    /// Renew the session. Uses the browser when compiled in and enabled,
    /// otherwise returns a clear error telling the owner to paste a cURL.
    pub async fn refresh_session(&self) -> Result<(), OracleError> {
        let _guard = self.refresh_lock.lock().await;

        if !self.cfg.browser_enabled {
            return Err(OracleError::Other(
                "browser capture is unavailable on this build — paste a cURL snippet instead"
                    .to_string(),
            ));
        }

        #[cfg(feature = "browser")]
        {
            match super::browser::capture(self).await {
                Ok(parsed) => {
                    // Only overwrite stored credentials, never env-only sessions.
                    if matches!(
                        self.resolve().await.map(|c| c.source),
                        Some(CredentialSource::Database)
                    ) {
                        if let Err(err) = self.save(&parsed, None).await {
                            tracing::error!(error = %err, "saving refreshed oracle session failed");
                        }
                    }
                    tracing::info!("oracle session renewed via browser");
                    Ok(())
                }
                Err(err) => Err(OracleError::Other(format!("browser capture failed: {err}"))),
            }
        }

        #[cfg(not(feature = "browser"))]
        {
            Err(OracleError::Other(
                "this build has no browser support — paste a cURL snippet instead".to_string(),
            ))
        }
    }

    /// Test and persist the result for the Oracle panel.
    pub async fn test_and_record(&self) -> Result<String, OracleError> {
        let outcome = self.test().await;
        let (ok, message) = match &outcome {
            Ok(msg) => (true, msg.clone()),
            Err(err) => (false, err.to_string()),
        };
        if let Err(err) = db::oracle::record_test(&self.db, ok, &message).await {
            tracing::warn!(error = %err, "recording oracle test result failed");
        }
        outcome
    }
}

fn decode(status: u16, text: String) -> Result<Value, OracleError> {
    if status == 401 || status == 403 {
        return Err(OracleError::Expired(status));
    }
    if !(200..300).contains(&status) {
        return Err(OracleError::Http(status));
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") {
        // The SPA serves 200 HTML for unknown routes.
        return Err(OracleError::Malformed);
    }
    serde_json::from_str(&text).map_err(|_| OracleError::Malformed)
}

/// Gift codes are alphanumeric, 4–32 chars.
pub fn valid_code(code: &str) -> bool {
    let len = code.len();
    (4..=32).contains(&len) && code.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Parse a JSON body into the enrichment struct used by `players`.
pub fn enrichment_from_player(value: &Value) -> db::players::Enrichment {
    use serde_json::Value as V;

    fn locate<'a>(value: &'a V, keys: &[&str]) -> Option<&'a V> {
        let obj = value.as_object()?;
        keys.iter()
            .find_map(|k| obj.get(*k))
            .filter(|v| !v.is_null())
    }

    fn text(value: &V, keys: &[&str]) -> Option<String> {
        let found = locate(value, keys)?;
        match found {
            V::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            V::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    fn number(value: &V, keys: &[&str]) -> Option<i64> {
        let found = locate(value, keys)?;
        match found {
            V::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            V::String(s) => s
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '-')
                .collect::<String>()
                .parse()
                .ok(),
            _ => None,
        }
    }

    let root = locate(value, &["player", "data", "profile"]).unwrap_or(value);
    let alliance = locate(root, &["alliance", "alliance_info"]);

    let abbr = text(root, &["alliance_abbr", "allianceAbbr", "alliance_tag"])
        .or_else(|| alliance.and_then(|a| text(a, &["abbr", "tag", "short_name"])));
    let alliance_name = text(root, &["alliance_name", "allianceName"])
        .or_else(|| alliance.and_then(|a| text(a, &["name", "full_name"])));
    let alliance_id = text(root, &["alliance_id", "allianceId"])
        .or_else(|| alliance.and_then(|a| text(a, &["id", "aid"])));
    let alliance_rank = number(root, &["alliance_rank", "allianceRank", "rank"])
        .or_else(|| alliance.and_then(|a| number(a, &["rank"])));

    db::players::Enrichment {
        game_name: text(root, &["game_name", "name", "nickname", "ign"]),
        alliance_abbr: abbr,
        alliance_id,
        alliance_name,
        alliance_rank,
        power: number(root, &["power", "total_power", "totalPower"]),
        kills: number(root, &["kills", "total_kills", "totalKills"]),
        town_hall_level: number(
            root,
            &[
                "town_hall_level",
                "townHallLevel",
                "furnace_level",
                "furnaceLevel",
                "tc_level",
            ],
        ),
        life_tree_level: number(root, &["life_tree_level", "lifeTreeLevel"]),
        avatar_url: text(root, &["avatar_url", "avatar", "avatarUrl", "photo"]),
        oracle_uid: text(root, &["uid", "uuid", "oracle_uid"]),
        state: number(root, &["state", "kid", "kingdom"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_enrichment() {
        let value = json!({
            "player": {
                "fid": "1",
                "name": "Bob",
                "power": "1,234",
                "alliance": { "abbr": "ABC", "name": "Alpha", "rank": 2 },
                "furnaceLevel": 30
            }
        });
        let e = enrichment_from_player(&value);
        assert_eq!(e.game_name.as_deref(), Some("Bob"));
        assert_eq!(e.alliance_abbr.as_deref(), Some("ABC"));
        assert_eq!(e.power, Some(1234));
        assert_eq!(e.town_hall_level, Some(30));
    }

    #[test]
    fn code_validation() {
        assert!(valid_code("ABCD1234"));
        assert!(!valid_code("AB"));
        assert!(!valid_code("AB CD"));
    }
}
