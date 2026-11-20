//! The HTTP side of lookup.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::Value;

use super::types::{AllianceProfile, PlayerProfile, StateProfile};

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    #[error("not found")]
    NotFound,
    #[error("upstream is rate limiting us — try again in a moment")]
    RateLimited,
    #[error("upstream returned HTTP {0}")]
    Http(u16),
    #[error("upstream returned malformed data")]
    Malformed,
    #[error("network error: {0}")]
    Network(String),
}

impl From<reqwest::Error> for LookupError {
    fn from(value: reqwest::Error) -> Self {
        if value.is_timeout() {
            Self::Network("timed out".to_string())
        } else {
            Self::Network(value.to_string())
        }
    }
}

/// Cached, cheap-to-clone lookup client.
#[derive(Clone)]
pub struct LookupClient {
    http: reqwest::Client,
    site: String,
    api: String,
    api_key: Arc<RwLock<Option<String>>>,
}

impl LookupClient {
    pub fn new(site: String, api: String, api_key: Option<String>) -> anyhow::Result<Self> {
        let http = crate::net::builder()
            .user_agent(BROWSER_UA)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .gzip(true)
            .brotli(true)
            .build()?;
        Ok(Self {
            http,
            site: site.trim_end_matches('/').to_string(),
            api: api.trim_end_matches('/').to_string(),
            api_key: Arc::new(RwLock::new(api_key)),
        })
    }

    fn api_key_value(&self) -> Option<String> {
        self.api_key.read().ok().and_then(|guard| guard.clone())
    }

    /// Replace (or clear) the runtime API key — used after the owner stores
    /// or removes one from the settings page.
    pub fn set_api_key(&self, key: Option<String>) {
        if let Ok(mut guard) = self.api_key.write() {
            *guard = key;
        }
    }

    fn site_url(&self, path: &str) -> String {
        format!("{}/api{}", self.site, path)
    }

    fn official_url(&self, path: &str) -> String {
        format!("{}{}", self.api, path)
    }

    /// Site first (no key needed), then the keyed API.
    async fn get(&self, path: &str) -> Result<Value, LookupError> {
        if let Ok(value) = self.get_site(path).await {
            return Ok(value);
        }
        if self.api_key_value().is_some() {
            return self.get_official(path).await;
        }
        self.get_site(path).await
    }

    async fn get_site(&self, path: &str) -> Result<Value, LookupError> {
        let response = self
            .http
            .get(self.site_url(path))
            .header("Accept", "application/json")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", format!("{}/map", self.site))
            .send()
            .await?;

        Self::decode(response).await
    }

    async fn get_official(&self, path: &str) -> Result<Value, LookupError> {
        let key = self
            .api_key_value()
            .ok_or_else(|| LookupError::Network("no API key configured".to_string()))?;
        let response = self
            .http
            .get(self.official_url(path))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {key}"))
            .header("X-Api-Key", key)
            .send()
            .await?;

        Self::decode(response).await
    }

    async fn decode(response: reqwest::Response) -> Result<Value, LookupError> {
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(LookupError::RateLimited);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(LookupError::NotFound);
        }
        if !status.is_success() {
            return Err(LookupError::Http(status.as_u16()));
        }
        let text = response.text().await?;
        serde_json::from_str(&text).map_err(|_| LookupError::Malformed)
    }

    // ------------------------------------------------------------- players

    /// Resolve a player by FID/UID/name.
    pub async fn player(&self, query: &str) -> Result<PlayerProfile, LookupError> {
        let encoded = urlencoding::encode(query.trim());

        // Direct hit first — the site accepts both UID and FID.
        if let Ok(value) = self.get_site(&format!("/players/{encoded}")).await {
            let profile = PlayerProfile::from_value(&value);
            if !profile.is_empty() {
                return Ok(profile);
            }
        }

        // Otherwise search and take the best match.
        let results = self.search_players(query).await?;
        let best = results
            .iter()
            .find(|value| {
                value
                    .get("fid")
                    .and_then(Value::as_str)
                    .is_some_and(|fid| fid == query.trim())
            })
            .or_else(|| results.first())
            .ok_or(LookupError::NotFound)?
            .clone();

        let profile = PlayerProfile::from_value(&best);
        if profile.is_empty() {
            return Err(LookupError::NotFound);
        }
        Ok(profile)
    }

    /// Raw `/search?q=` results.
    pub async fn search_players(&self, query: &str) -> Result<Vec<Value>, LookupError> {
        let encoded = urlencoding::encode(query.trim());
        let value = self.get(&format!("/search?q={encoded}")).await?;
        let rows = value
            .get("results")
            .and_then(Value::as_array)
            .or_else(|| value.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(rows)
    }

    // ------------------------------------------------------------ alliances

    pub async fn alliance(
        &self,
        abbr: &str,
        state: Option<i64>,
    ) -> Result<AllianceProfile, LookupError> {
        let encoded = urlencoding::encode(abbr.trim());
        let kid = state.unwrap_or(0);
        let value = self
            .get(&format!("/alliances/lookup?slug={encoded}&kid={kid}"))
            .await?;
        Ok(AllianceProfile::from_value(abbr, &value))
    }

    pub async fn search_alliances(
        &self,
        state: i64,
        query: &str,
    ) -> Result<Vec<Value>, LookupError> {
        let encoded = urlencoding::encode(if query.trim().is_empty() {
            "a"
        } else {
            query.trim()
        });
        let value = self
            .get(&format!("/search/alliances?q={encoded}&kid={state}"))
            .await?;
        Ok(value
            .get("results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    // ---------------------------------------------------------------- states

    pub async fn state(&self, state: i64) -> Result<StateProfile, LookupError> {
        let value = self
            .get(&format!("/kingdoms/{state}?alliances=100&players=100"))
            .await?;
        Ok(StateProfile::from_value(state, &value))
    }

    /// Probe connectivity. Returns a short human-readable status line.
    pub async fn health(&self) -> Result<String, LookupError> {
        let value = self.get_site("/leaderboards/home").await?;
        if value.get("players").is_some() || value.get("ok").is_some() {
            Ok("public site reachable (no key needed)".to_string())
        } else {
            Ok("public site answered with an unexpected body".to_string())
        }
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key_value().is_some()
    }

    /// Connection probe, like the legacy `testWosConnection`: public site
    /// first, then the official API with the stored key.
    pub async fn test_connection(&self) -> (bool, String) {
        if self.get_site("/leaderboards/home").await.is_ok() {
            return (true, "Connected (public site — no key needed)".to_string());
        }

        let Some(key) = self.api_key_value() else {
            return (
                false,
                "Public site unreachable and no API key stored.".to_string(),
            );
        };

        let response = self
            .http
            .get(format!("{}/usage", self.api))
            .header("Accept", "application/json")
            .header("Authorization", format!("Bearer {key}"))
            .header("X-Api-Key", &key)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                (true, "Connected (API key accepted)".to_string())
            }
            Ok(response) => (
                false,
                format!("API key rejected: HTTP {}", response.status().as_u16()),
            ),
            Err(err) => (false, format!("API unreachable: {err}")),
        }
    }
}
