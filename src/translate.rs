//! Shared "Translate" button for public bot messages.
//!
//! Any public message the bot posts can carry a **Translate** button; clicking
//! it replies with an ephemeral language picker; picking a language replies
//! with a private translated copy of the message. The original message is
//! never edited.
//!
//! Translation uses Google's free `translate_a/single?client=gtx` endpoint —
//! the same unofficial endpoint the legacy bot used via
//! `google-translate-api-x`. No API key is required.

use anyhow::{anyhow, Result};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use serenity::{
    CreateActionRow, CreateEmbed, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
    ReactionType,
};

use crate::ui::components;

pub const LANGUAGES: &[(&str, &str, &str)] = &[
    ("English", "🇬🇧", "en"),
    ("Deutsch", "🇩🇪", "de"),
    ("Français", "🇫🇷", "fr"),
    ("Español", "🇪🇸", "es"),
    ("Italiano", "🇮🇹", "it"),
    ("Русский", "🇷🇺", "ru"),
    ("Українська", "🇺🇦", "uk"),
    ("Português", "🇵🇹", "pt"),
    ("Türkçe", "🇹🇷", "tr"),
    ("العربية", "🇸🇦", "ar"),
    ("日本語", "🇯🇵", "ja"),
    ("한국어", "🇰🇷", "ko"),
    ("中文", "🇨🇳", "zh-CN"),
    ("हिन्दी", "🇮🇳", "hi"),
    ("नेपाली", "🇳🇵", "ne"),
];

/// Delimiter unlikely to appear in normal copy; used to translate a batch of
/// short strings in a single API call.
const BATCH_DELIMITER: &str = "\n￼￼￼\n";

/// Embed payload persisted for later translation. Kept intentionally minimal:
/// title, description, field names/values, and inline flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredEmbed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub fields: Vec<StoredField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredField {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

impl StoredEmbed {
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    pub fn to_create_embed(&self) -> CreateEmbed {
        let mut embed = CreateEmbed::new();
        if let Some(title) = &self.title {
            embed = embed.title(title);
        }
        if let Some(description) = &self.description {
            let safe: String = description.chars().take(4000).collect();
            embed = embed.description(safe);
        }
        for field in &self.fields {
            embed = embed.field(&field.name, &field.value, field.inline);
        }
        if let Some(footer) = &self.footer {
            embed = embed.footer(serenity::CreateEmbedFooter::new(footer.clone()));
        }
        if let Some(color) = self.color {
            embed = embed.color(color);
        }
        embed
    }
}

pub fn translate_row(message_id: u64) -> CreateActionRow {
    CreateActionRow::Buttons(vec![components::success(
        format!("translate:{message_id}"),
        "Translate",
    )])
}

pub fn language_select_row(message_id: u64) -> CreateActionRow {
    let menu = CreateSelectMenu::new(
        format!("translate_lang:{message_id}"),
        CreateSelectMenuKind::String {
            options: LANGUAGES
                .iter()
                .map(|(label, emoji, code)| {
                    CreateSelectMenuOption::new(*label, *code)
                        .emoji(ReactionType::Unicode((*emoji).to_string()))
                })
                .collect(),
        },
    )
    .placeholder("Choose your language");
    CreateActionRow::SelectMenu(menu)
}

/// Register a message as translatable (upsert).
pub async fn register(
    db: &crate::db::Db,
    message_id: u64,
    input: crate::db::translate::NewTranslatable<'_>,
) -> anyhow::Result<()> {
    crate::db::translate::register(db, message_id, input).await?;
    Ok(())
}

async fn translate_one(client: &reqwest::Client, text: &str, target: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }
    let url = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t&q={}",
        target,
        urlencoding(text)
    );
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| anyhow!("translate request: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("translate HTTP {}", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("translate json: {e}"))?;
    // Response shape: [[["translated","original",...],...],...]
    let segments = body[0]
        .as_array()
        .ok_or_else(|| anyhow!("unexpected translate response"))?;
    let mut out = String::new();
    for segment in segments {
        if let Some(text) = segment[0].as_str() {
            out.push_str(text);
        }
    }
    Ok(out)
}

fn urlencoding(text: &str) -> String {
    text.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

/// Translate a batch of strings in one call, falling back to per-string calls
/// if the delimiter does not survive translation.
pub async fn translate_batch(
    client: &reqwest::Client,
    texts: &[String],
    target: &str,
) -> Vec<String> {
    if target == "en" || texts.is_empty() {
        return texts.to_vec();
    }
    let joined = texts.join(BATCH_DELIMITER);
    if let Ok(translated) = translate_one(client, &joined, target).await {
        let parts: Vec<String> = translated
            .split('￼')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() == texts.len() {
            return parts;
        }
    }
    // Fallback: translate one by one.
    let mut out = Vec::with_capacity(texts.len());
    for text in texts {
        match translate_one(client, text, target).await {
            Ok(t) => out.push(t),
            Err(_) => out.push(text.clone()),
        }
    }
    out
}

/// Translate the stored embed/plain text to `target`, returning a new embed.
pub async fn translate_stored(
    client: &reqwest::Client,
    record: &crate::db::translate::TranslatableMessage,
    target: &str,
) -> Option<CreateEmbed> {
    if let Some(json) = &record.embed_json {
        let stored = StoredEmbed::from_json(json)?;
        let mut strings = Vec::new();
        strings.push(stored.title.clone().unwrap_or_default());
        strings.push(stored.description.clone().unwrap_or_default());
        for field in &stored.fields {
            strings.push(field.name.clone());
            strings.push(field.value.clone());
        }
        let translated = translate_batch(client, &strings, target).await;
        let mut iter = translated.into_iter();
        let title = iter.next().unwrap_or_default();
        let description = iter.next().unwrap_or_default();
        let mut result = stored.clone();
        result.title = if title.is_empty() { None } else { Some(title) };
        result.description = if description.is_empty() {
            None
        } else {
            Some(description)
        };
        result.fields = result
            .fields
            .iter()
            .map(|field| StoredField {
                name: iter.next().unwrap_or_else(|| field.name.clone()),
                value: iter.next().unwrap_or_else(|| field.value.clone()),
                inline: field.inline,
            })
            .collect();
        result.footer = Some("Private translation — only you can see this.".to_string());
        Some(result.to_create_embed())
    } else {
        None
    }
}

pub fn translation_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::net::builder()
            .user_agent("Mozilla/5.0 (compatible; OmniCore/2.0)")
            .build()
            .expect("translate client")
    })
}
