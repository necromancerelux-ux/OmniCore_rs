//! Server Index: auto-updating public directory of channels and forum posts.
//!
//! Mirrors src/serverIndex/{builder,publisher,events}.js. The index lives in
//! a configured channel; it is rebuilt automatically whenever channels or
//! forum threads change (debounced), or manually via the settings panel.

use std::collections::HashMap;

use poise::serenity_prelude as serenity;
use serenity::{ChannelId, ChannelType, CreateEmbed};

use crate::state::Data;
use crate::ui::theme::colors;

const MAX_EMBEDS: usize = 6;
const FIELD_VALUE_MAX: usize = 900;

fn section_emoji(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let has = |pats: &[&str]| pats.iter().any(|p| n.contains(p));
    if has(&["get-start", "getstart", "welcome", "intro", "start"]) {
        "👋"
    } else if has(&["important", "announce", "news", "notice", "alert"]) {
        "📢"
    } else if has(&[
        "guide", "help", "info", "resource", "wiki", "calc", "hero", "event",
    ]) {
        "📖"
    } else if has(&["nap", "leader", "officer", "staff", "admin"]) {
        "🛡️"
    } else if has(&["voice", "vc", "talk", "speak", "stage"]) {
        "🎙️"
    } else if has(&["chat", "general", "social", "lounge"]) {
        "💬"
    } else if has(&["gift", "code", "giveaway", "redeem"]) {
        "🎁"
    } else if has(&["rule", "policy"]) {
        "📜"
    } else if has(&["vote", "poll"]) {
        "🗳️"
    } else {
        "📁"
    }
}

fn channel_icon(kind: ChannelType, name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    match kind {
        ChannelType::Forum => "🗣️",
        ChannelType::Stage => "🎙️",
        ChannelType::Voice => "🔊",
        ChannelType::News => "📢",
        ChannelType::Text => {
            if n.contains("rule") {
                "📜"
            } else if n.contains("verify") {
                "✅"
            } else if n.contains("intro") || n.contains("meet") {
                "👋"
            } else if n.contains("announce") || n.contains("news") {
                "📢"
            } else if n.contains("gift") || n.contains("code") || n.contains("giveaway") {
                "🎁"
            } else if n.contains("event") || n.contains("reminder") {
                "🕒"
            } else if n.contains("vote") || n.contains("poll") {
                "🗳️"
            } else if n.contains("help") {
                "❓"
            } else if n.contains("calc") {
                "🧮"
            } else if n.contains("nap") {
                "🐾"
            } else if n.contains("role") {
                "🏷️"
            } else {
                "💬"
            }
        }
        _ => "💬",
    }
}

fn topic_snippet(topic: Option<&str>) -> String {
    let Some(topic) = topic else {
        return String::new();
    };
    let first = topic
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let collapsed: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() > 90 {
        format!("{}…", collapsed.chars().take(89).collect::<String>())
    } else {
        collapsed
    }
}

struct IndexChannel {
    id: u64,
    name: String,
    kind: ChannelType,
    topic: Option<String>,
    position: u16,
    parent_id: Option<u64>,
}

fn collect_channels(
    ctx: &serenity::Context,
    guild_id: u64,
    include_voice: bool,
) -> Vec<IndexChannel> {
    let Some(guild) = ctx.cache.guild(guild_id) else {
        return Vec::new();
    };
    guild
        .channels
        .values()
        .filter(|ch| match ch.kind {
            ChannelType::Text | ChannelType::News | ChannelType::Forum => true,
            ChannelType::Voice | ChannelType::Stage => include_voice,
            _ => false,
        })
        .map(|ch| IndexChannel {
            id: ch.id.get(),
            name: ch.name.clone(),
            kind: ch.kind,
            topic: ch.topic.clone(),
            position: ch.position,
            parent_id: ch.parent_id.map(|id| id.get()),
        })
        .collect()
}

fn collect_categories(ctx: &serenity::Context, guild_id: u64) -> Vec<(u64, String, u16)> {
    let Some(guild) = ctx.cache.guild(guild_id) else {
        return Vec::new();
    };
    let mut cats: Vec<(u64, String, u16)> = guild
        .channels
        .values()
        .filter(|ch| ch.kind == ChannelType::Category)
        .map(|ch| (ch.id.get(), ch.name.clone(), ch.position))
        .collect();
    cats.sort_by_key(|(_, _, position)| *position);
    cats
}

/// Build the index embeds (title + one field per category).
pub fn build_embeds(
    ctx: &serenity::Context,
    guild_id: u64,
    include_voice: bool,
    server_name: &str,
) -> Vec<CreateEmbed> {
    let channels = collect_channels(ctx, guild_id, include_voice);
    let categories = collect_categories(ctx, guild_id);

    let mut by_cat: HashMap<u64, Vec<&IndexChannel>> = HashMap::new();
    let mut root: Vec<&IndexChannel> = Vec::new();
    for ch in &channels {
        match ch.parent_id {
            Some(parent) => by_cat.entry(parent).or_default().push(ch),
            None => root.push(ch),
        }
    }
    for list in by_cat.values_mut() {
        list.sort_by(|a, b| {
            a.position
                .cmp(&b.position)
                .then_with(|| a.name.cmp(&b.name))
        });
    }
    root.sort_by(|a, b| {
        a.position
            .cmp(&b.position)
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut embeds = Vec::new();
    let mut current = CreateEmbed::new()
        .title(format!("📑 {} — Channel Index", server_name))
        .color(colors::BRAND)
        .description("Auto-generated map of this server. Jump straight to any channel below.")
        .footer(serenity::CreateEmbedFooter::new(crate::ui::theme::footer()));
    let mut fields = 0usize;

    let push_field =
        |embed: &mut CreateEmbed, name: String, lines: Vec<String>, fields: &mut usize| {
            let mut text = String::new();
            for line in lines {
                if text.len() + line.len() + 1 > FIELD_VALUE_MAX {
                    break; // Discord's 1024-char field value cap; keep it simple.
                }
                text.push_str(&line);
                text.push('\n');
            }
            let value = if text.is_empty() {
                "_empty_".to_string()
            } else {
                text.trim_end().to_string()
            };
            let taken = std::mem::replace(embed, CreateEmbed::new());
            *embed = taken.field(name, value, false);
            *fields += 1;
        };

    // Categories in order, then orphan channels.
    for (cat_id, cat_name, _) in &categories {
        let Some(list) = by_cat.remove(cat_id) else {
            continue;
        };
        if list.is_empty() {
            continue;
        }
        let lines: Vec<String> = list.iter().map(|ch| channel_line(ch)).collect();
        let name = format!("{} {}", section_emoji(cat_name), cat_name);
        if fields >= 20 {
            embeds.push(std::mem::replace(
                &mut current,
                CreateEmbed::new().color(colors::BRAND),
            ));
            fields = 0;
        }
        push_field(&mut current, name, lines, &mut fields);
        if embeds.len() >= MAX_EMBEDS - 1 {
            // Flush remaining sections as raw fields into the current embed.
            for (cat_id2, cat_name2, _) in &categories {
                let Some(list2) = by_cat.remove(cat_id2) else {
                    continue;
                };
                if list2.is_empty() {
                    continue;
                }
                let lines2: Vec<String> = list2.iter().map(|ch| channel_line(ch)).collect();
                let name2 = format!("{} {}", section_emoji(cat_name2), cat_name2);
                push_field(&mut current, name2, lines2, &mut fields);
            }
            break;
        }
    }
    if !root.is_empty() {
        if fields >= 20 {
            embeds.push(std::mem::replace(
                &mut current,
                CreateEmbed::new().color(colors::BRAND),
            ));
        }
        let lines: Vec<String> = root.iter().map(|ch| channel_line(ch)).collect();
        push_field(&mut current, "📁 Other".to_string(), lines, &mut fields);
    }

    embeds.push(current);
    embeds
}

fn channel_line(ch: &IndexChannel) -> String {
    let icon = channel_icon(ch.kind, &ch.name);
    let snippet = topic_snippet(ch.topic.as_deref());
    if snippet.is_empty() {
        format!("{icon} <#{}>", ch.id)
    } else {
        format!("{icon} <#{}> — {}", ch.id, snippet)
    }
}

/// Publish (or refresh) the index in the configured channel.
/// `force` bypasses the "auto-index disabled" gate for manual Rebuild.
pub async fn publish(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: u64,
    force: bool,
) -> Result<Vec<u64>, String> {
    let settings = crate::db::server_index::get(&data.db, guild_id)
        .await
        .map_err(|e| e.to_string())?;
    let Some(channel_id) = settings.channel() else {
        return Err("no channel configured".to_string());
    };
    if !force && !settings.enabled() {
        return Err("auto-index disabled".to_string());
    }

    let server_name = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| "Server".to_string());
    let embeds = build_embeds(ctx, guild_id, settings.include_voice(), &server_name);
    let old_ids = settings.message_ids();
    let target = ChannelId::new(channel_id);

    let mut new_ids: Vec<u64> = Vec::with_capacity(embeds.len());
    for (index, embed) in embeds.into_iter().enumerate() {
        let message_id = old_ids.get(index).copied();
        let stored = crate::translate::StoredEmbed {
            title: Some(format!("📑 {server_name} — Channel Index")),
            description: None,
            fields: Vec::new(),
            footer: None,
            color: Some(colors::BRAND),
        };

        match message_id {
            Some(id) => {
                let id = serenity::MessageId::new(id);
                match target
                    .edit_message(
                        &ctx.http,
                        id,
                        serenity::EditMessage::new()
                            .embed(embed.clone())
                            .components(vec![]),
                    )
                    .await
                {
                    Ok(_) => {
                        let _ = crate::translate::register(
                            &data.db,
                            id.get(),
                            crate::db::translate::NewTranslatable {
                                guild_id: Some(guild_id),
                                channel_id: Some(channel_id),
                                kind: "server_index",
                                embed_json: serde_json::to_string(&stored).ok(),
                                content: None,
                                owner_id: None,
                            },
                        )
                        .await;
                        new_ids.push(id.get());
                    }
                    Err(_) => {
                        let sent = target
                            .send_message(&ctx.http, serenity::CreateMessage::new().embed(embed))
                            .await
                            .map_err(|e| e.to_string())?;
                        new_ids.push(sent.id.get());
                    }
                }
            }
            None => {
                let sent = target
                    .send_message(&ctx.http, serenity::CreateMessage::new().embed(embed))
                    .await
                    .map_err(|e| e.to_string())?;
                if index == 0 {
                    let _ = crate::translate::register(
                        &data.db,
                        sent.id.get(),
                        crate::db::translate::NewTranslatable {
                            guild_id: Some(guild_id),
                            channel_id: Some(channel_id),
                            kind: "server_index",
                            embed_json: serde_json::to_string(&stored).ok(),
                            content: None,
                            owner_id: None,
                        },
                    )
                    .await;
                }
                new_ids.push(sent.id.get());
            }
        }

        // Only the first message carries a Translate button.
        if index == 0 {
            let _ = target
                .edit_message(
                    &ctx.http,
                    serenity::MessageId::new(*new_ids.last().unwrap()),
                    serenity::EditMessage::new().components(vec![crate::translate::translate_row(
                        *new_ids.last().unwrap(),
                    )]),
                )
                .await;
        }
    }

    // Delete leftover messages from previous versions.
    for old in old_ids.iter().skip(new_ids.len()) {
        let _ = target
            .delete_message(&ctx.http, serenity::MessageId::new(*old))
            .await;
    }

    crate::db::server_index::set_message_ids(&data.db, guild_id, &new_ids)
        .await
        .map_err(|e| e.to_string())?;
    Ok(new_ids)
}

/// Debounced rebuild — called from channel/thread events.
pub async fn schedule_refresh(data: Data, ctx: serenity::Context, guild_id: u64) {
    use std::sync::{Mutex, OnceLock};
    static IN_FLIGHT: OnceLock<Mutex<std::collections::HashSet<u64>>> = OnceLock::new();
    let in_flight = IN_FLIGHT.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    {
        let mut guard = in_flight.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.insert(guild_id) {
            return; // already pending for this guild
        }
    }
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&guild_id);
        let enabled = crate::db::server_index::get(&data.db, guild_id)
            .await
            .map(|s| s.enabled() && s.channel().is_some())
            .unwrap_or(false);
        if !enabled {
            return;
        }
        if let Err(err) = publish(&ctx, &data, guild_id, false).await {
            tracing::warn!(error = %err, guild = guild_id, "server index refresh failed");
        }
    });
}
