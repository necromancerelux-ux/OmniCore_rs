//! Oracle status panel — shared by the settings hub (no slash commands).

use crate::db;
use crate::state::Data;
use crate::ui::panels;
use poise::serenity_prelude as serenity;

/// Build the status view shared by the panel buttons.
pub async fn status_view(data: &Data) -> panels::OracleStatus {
    let creds = db::oracle::credentials(&data.db).await.ok().flatten();
    let state = db::oracle::state(&data.db).await.unwrap_or_default();
    let configured = data.oracle.is_configured().await;

    let preview = creds.as_ref().map(|row| {
        let raw = row.js_auth.trim().trim_start_matches("js_auth=");
        let tail = if raw.len() > 8 {
            &raw[raw.len() - 6..]
        } else {
            raw
        };
        format!("…{tail}")
    });

    panels::OracleStatus {
        has_credentials: configured,
        js_auth_preview: preview,
        updated: creds.as_ref().map(|row| row.updated_at),
        updated_by: creds.as_ref().and_then(|row| row.updated_by.clone()),
        last_test: creds.as_ref().and_then(|row| row.last_test_at),
        last_test_ok: creds.as_ref().and_then(|row| row.last_test_ok()),
        last_test_message: creds.as_ref().and_then(|row| row.last_test_message.clone()),
        codes_sync: state.last_codes_sync,
        lookup_sync: state.last_lookup_sync,
        total_codes: state.total_codes,
        browser_enabled: data.cfg.browser_enabled,
    }
}

pub async fn status_panel(data: &Data) -> (serenity::CreateEmbed, Vec<serenity::CreateActionRow>) {
    panels::oracle_panel(&status_view(data).await)
}
