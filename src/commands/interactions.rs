//! Component and modal routing for the button-panel UI.
//!
//! Every settings hub page and every public panel flows through here.
//! Handlers render panels from `ui::panels`, mutate the shared SQLite database
//! and answer with an updated ephemeral message.

use super::Error;
use crate::db;
use crate::permissions;
use crate::state::Data;
use crate::ui::theme::{colors, icons};
use crate::ui::{components, embeds, panels, theme};
use base64::Engine as _;
use poise::serenity_prelude as serenity;
use serenity::{
    ComponentInteraction, CreateActionRow, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, EditInteractionResponse, ModalInteraction,
};
use sqlx;

// ------------------------------------------------------------------ helpers

async fn send_modal(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    id: &str,
    title: &str,
    inputs: Vec<CreateActionRow>,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, modal(id, title, inputs))
        .await?;
    Ok(())
}

fn message(
    embed: CreateEmbed,
    components: Vec<CreateActionRow>,
    ephemeral: bool,
) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .embed(embed)
            .components(components)
            .ephemeral(ephemeral),
    )
}

fn update(embed: CreateEmbed, rows: Vec<CreateActionRow>) -> CreateInteractionResponse {
    CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new()
            .embed(embed)
            .components(rows),
    )
}

fn modal(id: &str, title: &str, inputs: Vec<CreateActionRow>) -> CreateInteractionResponse {
    CreateInteractionResponse::Modal(serenity::CreateModal::new(id, title).components(inputs))
}

async fn reply(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    embed: CreateEmbed,
    rows: Vec<CreateActionRow>,
    ephemeral: bool,
) -> anyhow::Result<()> {
    interaction
        .create_response(&ctx.http, message(embed, rows, ephemeral))
        .await?;
    Ok(())
}

async fn update_panel(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    embed: CreateEmbed,
    rows: Vec<CreateActionRow>,
) -> anyhow::Result<()> {
    let is_post = db::admin_toolkit::find_post(&data.db, interaction.message.id.get())
        .await
        .ok()
        .flatten()
        .is_some();
    if is_post {
        interaction
            .create_response(&ctx.http, message(embed, rows, true))
            .await?;
        if let Ok(response) = interaction.get_response(&ctx.http).await {
            crate::admintoolkit::track_ephemeral(response.id.get());
        }
        return Ok(());
    }
    interaction
        .create_response(&ctx.http, update(embed, rows))
        .await?;
    Ok(())
}

fn char_field<F>(interaction: &ModalInteraction, id: &str, f: F) -> Option<String>
where
    F: FnOnce(&str) -> Option<String>,
{
    interaction
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) if input.custom_id == id => {
                input.value.as_deref().map(str::to_string)
            }
            _ => None,
        })
        .and_then(|value| f(&value))
}

fn trimmed(interaction: &ModalInteraction, id: &str) -> Option<String> {
    char_field(interaction, id, |value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn guild_of(interaction: &ModalInteraction) -> Result<u64, Error> {
    interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("run this inside a server"))
}

fn opt_parse_channel(raw: &str) -> Result<Option<u64>, anyhow::Error> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("none") || raw.is_empty() {
        return Ok(None);
    }
    let id = raw
        .trim_end_matches('>')
        .trim_start_matches('<')
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("not a channel id or `none`"))?;
    Ok(Some(id))
}

fn parse_number_input(raw: &str) -> Option<i64> {
    let cleaned: String = raw
        .chars()
        .filter(|c| {
            c.is_ascii_digit() || *c == '.' || *c == 'K' || *c == 'M' || *c == 'k' || *c == 'm'
        })
        .collect();
    let value = crate::calculator::game_numbers::parse_game_amount(&cleaned);
    (value > 0 || raw.trim().chars().any(|c| c.is_ascii_digit())).then_some(value)
}

pub(crate) async fn defer_modal(
    ctx: &serenity::Context,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

pub(crate) async fn finish_modal(
    ctx: &serenity::Context,
    interaction: &ModalInteraction,
    embed: CreateEmbed,
) -> anyhow::Result<()> {
    interaction
        .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Component router
// ---------------------------------------------------------------------------

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
) {
    let custom_id = interaction.data.custom_id.clone();
    if let Err(err) = route_component(ctx, data, interaction, &custom_id).await {
        tracing::warn!(error = %crate::error::display_anyhow(&err), id = %custom_id, "component failed");
        let embed = embeds::error(
            "That did not work",
            format!("`{}`", crate::error::public(&err)),
        );
        let _ = interaction
            .create_response(&ctx.http, message(embed, Vec::new(), true))
            .await;
    }
}

async fn route_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    custom_id: &str,
) -> anyhow::Result<()> {
    let (namespace, action) = custom_id
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("unknown component `{custom_id}`"))?;

    match namespace {
        "settings" => settings_component(ctx, data, interaction, action).await,
        "music" => music_component(ctx, data, interaction, action).await,
        "lookup" => lookup_component(ctx, data, interaction, action).await,
        "oracle" => oracle_component(ctx, data, interaction, action).await,
        "gift" => gift_component(ctx, data, interaction, action).await,
        "ballot" => {
            let kind = action.split(':').next().unwrap_or("");
            match kind {
                "addq" | "togglepublic" | "togglechange" | "removeq" | "setrole" | "post"
                | "discard" => ballot_draft_component(ctx, data, interaction, action).await,
                _ => ballot_component(ctx, data, interaction, action).await,
            }
        }
        "giveaway" => giveaway_component(ctx, data, interaction, action).await,
        "translate" | "translate_lang" => {
            translate_component(ctx, data, interaction, namespace, action).await
        }
        "deploy" => deploy_component(ctx, data, interaction, action).await,
        "svs" => svs_component(ctx, data, interaction, action).await,
        "svscalc" => svscalc_component(ctx, data, interaction, action).await,
        "buildingcalc" => buildingcalc_component(ctx, data, interaction, action).await,
        "chiefgearcalc" => plan_component(ctx, data, interaction, action, 0).await,
        "charmcalc" => plan_component(ctx, data, interaction, action, 1).await,
        "petcalc" => pets_component(ctx, data, interaction, action).await,
        "waracademycalc" => war_academy_component(ctx, data, interaction, action).await,
        "role" => role_component(ctx, data, interaction, action).await,
        "roleassign" => roleassign_component(ctx, data, interaction, action).await,
        "perm" => perm_component(ctx, data, interaction, action).await,
        "ministry" => ministry_component(ctx, data, interaction, action).await,
        "tvc" => tempvoice_component(ctx, data, interaction, action).await,
        "owner" => crate::owner::owner_component(ctx, data, interaction, action).await,
        "branding" => branding_component(ctx, data, interaction, action).await,
        "giveawayrules" => giveawayrules_component(ctx, data, interaction, action).await,
        "help" => {
            if action == "home" {
                let (embed, rows) = panels::help_panel("");
                update_panel(ctx, data, interaction, embed, rows).await
            } else {
                help_component(ctx, interaction, action).await
            }
        }
        other => Err(anyhow::anyhow!("unknown component namespace `{other}`")),
    }
}

// ---------------------------------------------------------------- settings

async fn settings_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("settings only work inside a server"))?;

    let is_admin = permissions::component_is_admin(ctx, data, interaction).await;
    if !is_admin && !permissions::component_can_manage_guild(ctx, data, interaction, guild_id).await
    {
        return reply(
            ctx,
            interaction,
            embeds::denied("Manage Server"),
            Vec::new(),
            true,
        )
        .await;
    }

    // Per-server claim: first authorised user becomes Server Bot Admin when
    // the guild has none yet (mirrors the legacy claim flow).
    let _ =
        crate::db::subscriptions::claim_server_admin(&data.db, guild_id, interaction.user.id.get())
            .await;

    if let Some(action) = action.strip_prefix("lookup_api") {
        // Whiteout Stats API key management — owner-only, global to the bot.
        if data.cfg.owner_id != Some(interaction.user.id.get()) {
            return reply(
                ctx,
                interaction,
                embeds::denied("Bot Owner"),
                Vec::new(),
                true,
            )
            .await;
        }
        match action {
            "key" => {
                let current = db::wos_credentials::get(&data.db).await.unwrap_or_default();
                let inputs = vec![CreateActionRow::InputText(
                    components::paragraph("api_key", "Whiteout Stats API key")
                        .placeholder("wos_…  or  Authorization: Bearer wos_…")
                        .value(current.api_key.unwrap_or_default())
                        .max_length(500),
                )];
                interaction
                    .create_response(
                        &ctx.http,
                        modal("lookupmodal:apikey", "Whiteout Stats API key", inputs),
                    )
                    .await?;
                return Ok(());
            }
            "test" => {
                interaction
                    .create_response(
                        &ctx.http,
                        CreateInteractionResponse::Defer(
                            CreateInteractionResponseMessage::new().ephemeral(true),
                        ),
                    )
                    .await?;
                let (ok, message) = data.lookup.test_connection().await;
                if db::wos_credentials::is_configured(&data.db).await {
                    let _ = db::wos_credentials::record_test(&data.db, ok, &message).await;
                }
                let embed = if ok {
                    embeds::success("Test passed", message)
                } else {
                    embeds::warn("Test failed", message)
                };
                interaction
                    .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                    .await?;
                return Ok(());
            }
            "disconnect" => {
                interaction
                    .create_response(
                        &ctx.http,
                        CreateInteractionResponse::Defer(
                            CreateInteractionResponseMessage::new().ephemeral(true),
                        ),
                    )
                    .await?;
                db::wos_credentials::clear(&data.db).await?;
                data.lookup.set_api_key(None);
                interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .embed(embeds::ok("Whiteout Stats API key disconnected.")),
                    )
                    .await?;
                return Ok(());
            }
            _ => {}
        }
    }

    if action == "close" {
        return update_panel(
            ctx,
            data,
            interaction,
            embeds::ok("Settings closed. Use `/settings` to reopen."),
            Vec::new(),
        )
        .await;
    }

    // Feature panels all live behind their own handlers (render + mutate).
    let ns = action.split(':').next().unwrap_or("");

    // Owner feature toggles — a force-off panel renders a locked embed instead.
    if ns == "owner" {
        return crate::owner::owner_component(ctx, data, interaction, "hub").await;
    }
    let gate_keys: &[&str] = match ns {
        "leveling" => &["leveling"],
        "welcome" => &["welcome"],
        "logging" => &["logging"],
        "tempvoice" => &["tempvoice"],
        "voting" | "voting_create" => &["polls"],
        "birthday" => &["birthday"],
        "tickets" => &["tickets"],
        "giveaways" => &["giveaways"],
        "messages" => &["messages_guides"],
        "sneakpeek" => &["sneak_peek"],
        "wos" => &["wos_announcements"],
        "index" => &["server_index"],
        "roles" => &["autorole"],
        "notifications" => &["custom_notifications"],
        "calculator" => &["building_calculator", "all_calculators"],
        "ministry" => &["ministry"],
        "music" => &["music"],
        "gift" | "lookup" | "oracle" => &["gift_redemption"],
        _ => &[],
    };
    if settings_flag_locked(ctx, data, interaction, guild_id, gate_keys).await? {
        return Ok(());
    }

    match ns {
        "voting_create" => {
            return ballot_create_start(ctx, data, interaction, guild_id).await;
        }
        "leveling" => {
            return leveling_component(
                ctx,
                data,
                interaction,
                action[ns.len()..].trim_start_matches(':'),
            )
            .await
        }
        "welcome" => {
            return welcome_component(
                ctx,
                data,
                interaction,
                action[ns.len()..].trim_start_matches(':'),
            )
            .await
        }
        "logging" | "tempvoice" | "voting" | "birthday" | "tickets" | "giveaways" | "messages"
        | "cleanup" | "sneakpeek" | "wos" | "index" | "roles" | "notifications" | "calculator"
        | "admin" | "ministry" | "branding" => {
            return settings_feature_component(ctx, data, interaction, action, ns).await;
        }
        _ => {}
    }

    let (embed, rows) = if action == "home" {
        settings_home(ctx, data, guild_id, interaction.user.id.get()).await?
    } else {
        settings_section(ctx, data, guild_id, action, interaction.user.id.get()).await?
    };

    update_panel(ctx, data, interaction, embed, rows).await?;

    Ok(())
}

pub(crate) async fn settings_home(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: u64,
    owner_id: u64,
) -> anyhow::Result<(CreateEmbed, Vec<CreateActionRow>)> {
    let guild_name = ctx
        .cache
        .guild(serenity::GuildId::new(guild_id))
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| "this server".to_string());
    let players = db::players::stats(&data.db, guild_id).await?;
    let music = db::music::get(&data.db, guild_id).await?;
    let lookup = db::lookup::get(&data.db, guild_id).await?;
    let oracle = db::oracle::state(&data.db).await.unwrap_or_default();

    let view = panels::SettingsContext {
        guild_name,
        player_ids: players.tracked,
        player_limit: db::subscriptions::player_id_limit(&data.db, guild_id)
            .await
            .ok()
            .flatten(),
        music_configured: music
            .as_ref()
            .is_some_and(|row| row.channel_id.is_some() && row.panel_message_id.is_some()),
        lookup_configured: lookup.channel_id.is_some(),
        gift_sync: oracle.last_codes_sync,
        oracle_ready: data.oracle.is_configured().await,
        uptime: data.uptime(),
        latency_ms: 0,
        is_owner: data.cfg.owner_id == Some(owner_id),
    };
    Ok(panels::settings_hub(&view))
}

async fn settings_section(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: u64,
    section: &str,
    user_id: u64,
) -> anyhow::Result<(CreateEmbed, Vec<CreateActionRow>)> {
    let back = components::secondary("settings:home", format!("{} Back", icons::BACK));
    let footer = serenity::CreateEmbedFooter::new(theme::footer());

    let (mut embed, buttons) = match section {
        "players" => {
            let stats = db::players::stats(&data.db, guild_id).await?;
            let recent = db::players::page_for_guild(&data.db, guild_id, 5, 0).await?;
            let list = if recent.is_empty() {
                "_No players tracked yet._".to_string()
            } else {
                recent
                    .iter()
                    .map(|row| {
                        format!(
                            "`{}` **{}**{} · `{}`",
                            row.fid,
                            crate::error::truncate(row.label(), 24),
                            row.alliance_abbr
                                .as_deref()
                                .map(|abbr| format!(" [{abbr}]"))
                                .unwrap_or_default(),
                            row.power
                                .map(theme::compact_number)
                                .unwrap_or_else(|| "—".into())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let embed = embeds::brand(format!("{} Players", icons::PLAYER))
                .color(colors::INFO)
                .description("Track WoS player IDs and keep their Oracle profile fresh.")
                .field(
                    "Database",
                    embeds::rows(&[
                        ("Tracked".into(), format!("`{}`", stats.tracked)),
                        ("Enriched".into(), format!("`{}`", stats.enriched)),
                        ("Alliances".into(), format!("`{}`", stats.alliances)),
                        (
                            "Total power".into(),
                            format!("`{}`", theme::compact_number(stats.total_power)),
                        ),
                    ]),
                    true,
                )
                .field("Recently added", list, false)
                .footer(footer.clone());
            (embed, vec![back])
        }
        "music" => {
            let row = db::music::get(&data.db, guild_id).await?;
            let embed = embeds::brand(format!("{} Music", icons::MUSIC))
                .color(colors::BRAND_DEEP)
                .description("Use **Deploy panel here** to post the control panel, then tap the buttons on it.")
                .field(
                    "Configuration",
                    embeds::rows(&[
                        (
                            "Panel channel".into(),
                            row.as_ref()
                                .and_then(|row| row.channel_id())
                                .map(|id| format!("<#{id}>"))
                                .unwrap_or_else(|| "`not deployed`".into()),
                        ),
                        (
                            "Panel message".into(),
                            row.as_ref()
                                .and_then(|row| row.panel_message_id())
                                .map(|id| format!("`{id}`"))
                                .unwrap_or_else(|| "`missing`".into()),
                        ),
                        (
                            "Volume".into(),
                            format!("`{}%`", row.as_ref().map(|row| row.volume()).unwrap_or(80)),
                        ),
                        (
                            "Loop".into(),
                            format!(
                                "`{}`",
                                row.as_ref().map(|row| row.loop_mode().label()).unwrap_or("Off")
                            ),
                        ),
                    ]),
                    false,
                )
                .field(
                    "Lavalink",
                    if data.cfg.music_enabled() {
                        format!(
                            "`{}`",
                            data.music.describe_host().unwrap_or_else(|| "configured".into())
                        )
                    } else {
                        "`not configured` — set `LAVALINK_HOST` and `LAVALINK_PASSWORD`".into()
                    },
                    false,
                )
                .footer(footer.clone());
            (
                embed,
                vec![
                    components::primary(
                        "deploy:music",
                        format!("{} Deploy panel here", icons::ADD),
                    ),
                    components::secondary("settings:music", format!("{} Refresh", icons::REFRESH)),
                    back,
                ],
            )
        }
        "lookup" => {
            let row = db::lookup::get(&data.db, guild_id).await?;
            let creds = db::wos_credentials::get(&data.db).await.unwrap_or_default();
            let masked = creds.api_key.as_deref().map(|key| {
                if key.len() > 10 {
                    format!("`{}…{}`", &key[..6], &key[key.len() - 4..])
                } else {
                    "`••••••`".to_string()
                }
            });
            let test_line = match (creds.last_test_ok, creds.last_test_at) {
                (Some(true), _) => "🟢 last test passed".to_string(),
                (Some(false), _) => format!(
                    "🔴 last test failed{}",
                    creds
                        .last_test_message
                        .as_deref()
                        .map(|message| format!(" — `{}`", crate::error::truncate(message, 80)))
                        .unwrap_or_default()
                ),
                _ => "⚪ not tested yet".to_string(),
            };
            let key_block = match &masked {
                Some(masked) => format!("API key: {masked}\n{test_line}"),
                None => "API key: `none` — public site only\n_Optional: connect a key for the official API._".to_string(),
            };
            let embed = embeds::brand(format!("{} Lookup", icons::LOOKUP))
                .color(colors::TEAL)
                .description(
                    "Player, alliance and state lookups powered by Whiteout Stats and Oracle data.",
                )
                .field(
                    "Public panel",
                    embeds::rows(&[
                        (
                            "Channel".into(),
                            row.channel()
                                .map(|id| format!("<#{id}>"))
                                .unwrap_or_else(|| "`not deployed`".into()),
                        ),
                        (
                            "Player message".into(),
                            row.player_panel()
                                .map(|id| format!("`{id}`"))
                                .unwrap_or_else(|| "`missing`".into()),
                        ),
                    ]),
                    false,
                )
                .field(
                    "Source",
                    format!("`{}`\n{key_block}", data.cfg.wos_site),
                    false,
                )
                .footer(footer.clone());

            let mut buttons = vec![
                components::primary("deploy:lookup", format!("{} Deploy panel here", icons::ADD)),
                components::secondary("settings:lookup", format!("{} Refresh", icons::REFRESH)),
            ];
            // Owner-only: Whiteout Stats API key management (global to the bot).
            if data.cfg.owner_id == Some(user_id) {
                buttons.push(components::success(
                    "settings:lookup_apikey",
                    if creds.api_key.is_some() {
                        "🔑 Update API key"
                    } else {
                        "🔑 Connect API key"
                    },
                ));
                buttons.push(components::secondary("settings:lookup_apitest", "🧪 Test"));
                if creds.api_key.is_some() {
                    buttons.push(components::danger(
                        "settings:lookup_apidisconnect",
                        "🔌 Disconnect",
                    ));
                }
            }
            buttons.push(back);
            (embed, buttons)
        }
        "gift" => {
            let stats = db::giftcodes::stats(&data.db).await?;
            let oracle = db::oracle::state(&data.db).await.unwrap_or_default();
            let channel = db::settings::get_u64(
                &data.db,
                guild_id,
                db::settings::keys::GIFT_ANNOUNCE_CHANNEL,
            )
            .await?;
            let embed = embeds::brand(format!("{} Gift codes", icons::GIFT))
                .color(colors::GOLD)
                .description("Sync codes from Oracle and announce new ones automatically.")
                .field(
                    "Codes",
                    embeds::rows(&[
                        ("Total".into(), format!("`{}`", stats.total)),
                        ("Active".into(), format!("`{}`", stats.active)),
                        ("VIP".into(), format!("`{}`", stats.vip)),
                        ("Last 24h".into(), format!("`{}`", stats.today)),
                    ]),
                    true,
                )
                .field(
                    "Sync",
                    embeds::rows(&[
                        (
                            "Announce channel".into(),
                            channel
                                .map(|id| format!("<#{id}>"))
                                .unwrap_or_else(|| "`not set` — tap **Announce channel**".into()),
                        ),
                        (
                            "Last sync".into(),
                            oracle
                                .last_codes_sync
                                .map(embeds::db_relative)
                                .unwrap_or_else(|| "`never`".into()),
                        ),
                    ]),
                    true,
                )
                .footer(footer.clone());
            (
                embed,
                vec![
                    components::primary(
                        "deploy:gift_channel",
                        format!("{} Announce channel", icons::EDIT),
                    ),
                    components::secondary("gift:sync", format!("{} Sync now", icons::REFRESH)),
                    back,
                ],
            )
        }
        "oracle" => {
            let (mut embed, mut rows) = crate::commands::oracle::status_panel(data).await;
            rows.push(components::row(vec![components::secondary(
                "settings:home",
                format!("{} Back", icons::BACK),
            )]));
            embed = embed.footer(footer);
            return Ok((embed, rows));
        }
        "admin" => return admin_panel(ctx, data, guild_id).await,
        other => {
            let embed = embeds::warn("Unknown settings page", format!("`{other}`"));
            (embed, vec![back])
        }
    };

    embed = embed.footer(footer);
    Ok((embed, components::rows(buttons)))
}

// ------------------------------------------------------------- leveling

fn leveling_view(
    settings: db::leveling::LevelSettings,
    xp: i64,
    ranked: i64,
) -> panels::LevelingView {
    panels::LevelingView {
        enabled: settings.enabled(),
        xp_min: settings.xp_min,
        xp_max: settings.xp_max,
        cooldown_sec: settings.cooldown_sec,
        voice_xp: settings.voice_xp,
        announce_mode: settings.announce_mode.clone(),
        stack_rewards: settings.stack_rewards(),
        level: crate::leveling::level_of(xp),
        total_ranked: ranked,
    }
}

async fn render_leveling(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::leveling::settings(&data.db, guild_id).await?;
    let user_id = interaction.user.id.get();
    let xp = db::leveling::get_user(&data.db, guild_id, user_id)
        .await
        .map(|row| row.xp)
        .unwrap_or(0);
    let ranked = db::leveling::count_ranked(&data.db, guild_id)
        .await
        .unwrap_or(0);

    let view = leveling_view(settings, xp, ranked);
    let (embed, rows) = panels::leveling_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn leveling_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("leveling settings only work inside a server"))?;

    if action.is_empty() || action == "refresh" {
        return render_leveling(ctx, data, interaction, guild_id).await;
    }

    let sub = action.trim_start_matches(':');
    match sub {
        "toggle" => {
            let enabled = db::leveling::settings(&data.db, guild_id).await?.enabled();
            db::leveling::set_enabled(&data.db, guild_id, !enabled).await?;
            render_leveling(ctx, data, interaction, guild_id).await
        }
        "xp" => {
            let settings = db::leveling::settings(&data.db, guild_id).await?;
            let inputs = vec![
                CreateActionRow::InputText(
                    components::short("xp_min", "Minimum XP per message")
                        .value(settings.xp_min.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("xp_max", "Maximum XP per message")
                        .value(settings.xp_max.to_string()),
                ),
            ];
            interaction
                .create_response(
                    &ctx.http,
                    modal("settings:leveling_xp_modal", "XP range", inputs),
                )
                .await?;
            Ok(())
        }
        "cooldown" => {
            let settings = db::leveling::settings(&data.db, guild_id).await?;
            let inputs = vec![CreateActionRow::InputText(
                components::short("cooldown", "Cooldown seconds per message")
                    .value(settings.cooldown_sec.to_string()),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal("settings:leveling_cooldown_modal", "XP cooldown", inputs),
                )
                .await?;
            Ok(())
        }
        "voice" => {
            let settings = db::leveling::settings(&data.db, guild_id).await?;
            let inputs = vec![CreateActionRow::InputText(
                components::short("voice_xp", "Voice XP per minute")
                    .value(settings.voice_xp.to_string()),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal("settings:leveling_voice_modal", "Voice XP", inputs),
                )
                .await?;
            Ok(())
        }
        "announce" => {
            let settings = db::leveling::settings(&data.db, guild_id).await?;
            let next = match settings.announce_mode.as_str() {
                "same" => "channel",
                "channel" => "off",
                _ => "same",
            };
            db::leveling::set_announce_mode(&data.db, guild_id, next).await?;
            render_leveling(ctx, data, interaction, guild_id).await
        }
        "stack" => {
            let settings = db::leveling::settings(&data.db, guild_id).await?;
            db::leveling::set_stack_rewards(&data.db, guild_id, !settings.stack_rewards()).await?;
            render_leveling(ctx, data, interaction, guild_id).await
        }
        "message" => {
            let settings = db::leveling::settings(&data.db, guild_id).await?;
            let inputs = vec![CreateActionRow::InputText(
                components::paragraph(
                    "announce_message",
                    "Level-up message ({user}, {level}, {xp})",
                )
                .value(settings.announce_message.clone()),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal(
                        "settings:leveling_message_modal",
                        "Announcement message",
                        inputs,
                    ),
                )
                .await?;
            Ok(())
        }
        "rewards" => {
            let rewards = db::leveling::list_rewards(&data.db, guild_id).await?;
            let body = if rewards.is_empty() {
                "_No level rewards yet — tap **Rewards** to add one._".to_string()
            } else {
                rewards
                    .iter()
                    .map(|reward| format!("**Level {}** → <@&{}>", reward.level, reward.role_id))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let embed = embeds::brand(format!("{} Level rewards", icons::CROWN))
                .color(colors::GOLD)
                .description(format!("{}\n{}", theme::DIVIDER, body))
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        "reset" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "confirm",
                "Type RESET to wipe all XP for this server",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal("settings:leveling_reset_modal", "Reset XP", inputs),
                )
                .await?;
            Ok(())
        }
        other => Err(anyhow::anyhow!("unknown leveling action `{other}`")),
    }
}

// ------------------------------------------------------------- welcome

async fn render_welcome(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::welcome::welcome(&data.db, guild_id).await?;
    let autorole = db::welcome::autorole(&data.db, guild_id).await?;
    let joindm = db::welcome::join_dm(&data.db, guild_id).await?;
    let member_count = ctx
        .cache
        .guild(serenity::GuildId::new(guild_id))
        .map(|guild| guild.members.len() as u64)
        .unwrap_or(0);

    let view = panels::WelcomeView {
        channel: settings.channel(),
        avatar_mode: settings.avatar_mode.clone(),
        autorole_enabled: autorole.enabled(),
        member_roles: autorole.member_roles().len(),
        join_dm: joindm.join_dm_enabled(),
        reminder_interval: joindm.interval_days(),
        member_count,
    };
    let (embed, rows) = panels::welcome_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn welcome_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("welcome settings only work inside a server"))?;

    if action.is_empty() || action == "refresh" {
        return render_welcome(ctx, data, interaction, guild_id).await;
    }

    let sub = action.trim_start_matches(':');
    match sub {
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id or `none` to disable",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal("settings:welcome_channel_modal", "Welcome channel", inputs),
                )
                .await?;
            Ok(())
        }
        "title" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "title",
                "Title ({mention}, {server}, {membercount})",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal("settings:welcome_title_modal", "Welcome title", inputs),
                )
                .await?;
            Ok(())
        }
        "description" => {
            let inputs = vec![CreateActionRow::InputText(components::paragraph(
                "description",
                "Description ({mention}, {server}, {membercount})",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal(
                        "settings:welcome_description_modal",
                        "Welcome description",
                        inputs,
                    ),
                )
                .await?;
            Ok(())
        }
        "avatar" => {
            let current = db::welcome::welcome(&data.db, guild_id)
                .await
                .map(|row| row.avatar_mode)
                .unwrap_or_else(|_| "off".to_string());
            let next = match current.as_str() {
                "off" => "member",
                "member" => "server",
                _ => "off",
            };
            db::welcome::set_welcome_field(
                &data.db,
                guild_id,
                db::welcome::WelcomeField::AvatarMode,
                next,
            )
            .await?;
            render_welcome(ctx, data, interaction, guild_id).await
        }
        "autorole" => {
            let enabled = db::welcome::autorole(&data.db, guild_id)
                .await
                .map(|row| row.enabled())
                .unwrap_or(true);
            db::welcome::set_autorole_enabled(&data.db, guild_id, !enabled).await?;
            render_welcome(ctx, data, interaction, guild_id).await
        }
        "joindm" => {
            let enabled = db::welcome::join_dm(&data.db, guild_id)
                .await
                .map(|row| row.join_dm_enabled())
                .unwrap_or(true);
            db::welcome::set_join_dm(&data.db, guild_id, !enabled).await?;
            render_welcome(ctx, data, interaction, guild_id).await
        }
        "preview" => {
            let settings = db::welcome::welcome(&data.db, guild_id).await?;
            let member_count = ctx
                .cache
                .guild(serenity::GuildId::new(guild_id))
                .map(|guild| guild.members.len() as u64)
                .unwrap_or(0);
            let server = ctx
                .cache
                .guild(serenity::GuildId::new(guild_id))
                .map(|guild| guild.name.clone())
                .unwrap_or_else(|| "this server".to_string());
            let rendered = settings.render("<@you>", &server, member_count);
            let embed = serenity::CreateEmbed::new()
                .title(rendered.title)
                .description(rendered.description)
                .color(rendered.color as u32)
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        other => Err(anyhow::anyhow!("unknown welcome action `{other}`")),
    }
}

// ---------------------------------------------------- generic settings panels

/// Locked-feature gate — returns true when any `keys` is force-disabled on
/// this server (Bot Owner override via server_feature_flags).
async fn settings_flag_locked(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
    keys: &[&str],
) -> anyhow::Result<bool> {
    for key in keys {
        if !db::subscriptions::is_feature_enabled(&data.db, guild_id, key).await {
            let embed = embeds::warn(
                "Feature disabled",
                format!(
                    "**{}** is disabled on this server.\n\nThe Bot Owner can re-enable it from **Owner Panel → Features**.",
                    key.replace('_', " ")
                ),
            );
            reply(ctx, interaction, embed, Vec::new(), true).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

async fn settings_feature_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
    ns: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("settings only work inside a server"))?;

    let sub = action[ns.len()..].trim_start_matches(':');
    match ns {
        "logging" => logging_panel_action(ctx, data, interaction, sub, guild_id).await,
        "tempvoice" => tempvoice_panel_action(ctx, data, interaction, sub, guild_id).await,
        "voting" => voting_panel_action(ctx, data, interaction, sub, guild_id).await,
        "birthday" => birthday_panel_action(ctx, data, interaction, sub, guild_id).await,
        "tickets" => tickets_panel_action(ctx, data, interaction, sub, guild_id).await,
        "giveaways" => giveaways_panel_action(ctx, data, interaction, sub, guild_id).await,
        "messages" => messages_panel_action(ctx, data, interaction, sub, guild_id).await,
        "cleanup" => cleanup_panel_action(ctx, data, interaction, sub, guild_id).await,
        "sneakpeek" => sneakpeek_panel_action(ctx, data, interaction, sub, guild_id).await,
        "wos" => wos_panel_action(ctx, data, interaction, sub, guild_id).await,
        "index" => index_panel_action(ctx, data, interaction, sub, guild_id).await,
        "roles" => roles_panel_action(ctx, data, interaction, sub, guild_id).await,
        "notifications" => notifications_panel_action(ctx, data, interaction, sub, guild_id).await,
        "calculator" => calculator_panel_action(ctx, data, interaction, sub, guild_id).await,
        "ministry" => return ministry_settings_page(ctx, data, interaction, guild_id).await,
        "admin" => return admin_panel(ctx, data, guild_id).await.map(|_| ()),
        "branding" => return branding_settings_page(ctx, data, interaction, guild_id).await,
        other => Err(anyhow::anyhow!("unknown settings panel `{other}`")),
    }
}

// ------------------------------------------------------------- logging

async fn render_logging(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let channel = db::logging::channel_id(&data.db, guild_id).await?;
    let disabled = db::logging::disabled(&data.db, guild_id).await?;
    let view = panels::LoggingView { channel, disabled };
    let (embed, rows) = panels::logging_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn logging_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_logging(ctx, data, interaction, guild_id).await,
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id or `none` to clear",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:logging_channel_modal",
                "Log channel",
                inputs,
            )
            .await
        }
        "categories" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short(
                    "category",
                    "giftcode players members messages voice channels roles server",
                )),
                CreateActionRow::InputText(components::short("state", "on or off")),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:logging_categories_modal",
                "Toggle a category",
                inputs,
            )
            .await
        }
        "test" => {
            let channel = db::logging::channel_id(&data.db, guild_id).await?;
            let Some(channel_id) = channel else {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("No log channel", "Set one first with **Channel**."),
                    Vec::new(),
                    true,
                )
                .await;
            };
            let embed = crate::logging::entry(
                "🧪 Test log entry",
                &format!(
                    "Logging is working. Triggered by <@{}>.",
                    interaction.user.id.get()
                ),
                colors::SUCCESS,
            );
            let _ = serenity::ChannelId::new(channel_id)
                .send_message(&ctx.http, serenity::CreateMessage::new().embed(embed))
                .await;
            reply(
                ctx,
                interaction,
                embeds::ok("Test entry posted."),
                Vec::new(),
                true,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown logging action `{other}`")),
    }
}

// ------------------------------------------------------------ tempvoice

async fn render_tempvoice(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::tempvoice::settings(&data.db, guild_id).await?;
    let active = db::tempvoice::count_for_guild(&data.db, guild_id)
        .await
        .unwrap_or(0);
    let view = panels::TempVoiceView {
        enabled: settings.enabled(),
        lobby: settings.lobby_channel(),
        category: settings.category(),
        template: settings.name_template().to_string(),
        user_limit: settings.user_limit(),
        active,
    };
    let (embed, rows) = panels::tempvoice_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn tempvoice_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_tempvoice(ctx, data, interaction, guild_id).await,
        "toggle" => {
            let enabled = db::tempvoice::settings(&data.db, guild_id).await?.enabled();
            db::tempvoice::set_enabled(&data.db, guild_id, !enabled).await?;
            render_tempvoice(ctx, data, interaction, guild_id).await
        }
        "lobby" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Voice channel id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:tempvoice_lobby_modal",
                "Lobby channel",
                inputs,
            )
            .await
        }
        "category" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Category id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:tempvoice_category_modal",
                "Temp channel category",
                inputs,
            )
            .await
        }
        "template" => {
            let settings = db::tempvoice::settings(&data.db, guild_id).await?;
            let inputs = vec![CreateActionRow::InputText(
                components::short("template", "Name template ({user}, {count})")
                    .value(settings.name_template().to_string()),
            )];
            send_modal(
                ctx,
                interaction,
                "settings:tempvoice_template_modal",
                "Channel name template",
                inputs,
            )
            .await
        }
        "limit" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "limit",
                "Max users (0 = unlimited)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:tempvoice_limit_modal",
                "User limit",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown tempvoice action `{other}`")),
    }
}

// ------------------------------------------------------------- voting

async fn render_voting(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::voting::settings(&data.db, guild_id).await?;
    let active = db::ballots::count_active(&data.db, guild_id)
        .await
        .unwrap_or(0);
    let view = panels::VotingPanel {
        voting_channel: settings.channel_id,
        results_channel: settings.results_channel_id,
        active,
    };
    let (embed, rows) = panels::voting_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn voting_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_voting(ctx, data, interaction, guild_id).await,
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Vote channel id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:voting_channel_modal",
                "Vote channel",
                inputs,
            )
            .await
        }
        "results" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Results channel id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:voting_results_modal",
                "Auto-post results channel",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown voting action `{other}`")),
    }
}

// ------------------------------------------------------------- birthday

async fn render_birthday(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::birthdays::settings(&data.db, guild_id).await?;
    let count = db::birthdays::list(&data.db, guild_id)
        .await
        .map(|rows| rows.len() as i64)
        .unwrap_or(0);
    let view = panels::BirthdayView {
        channel: settings.channel(),
        count,
    };
    let (embed, rows) = panels::birthday_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn birthday_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_birthday(ctx, data, interaction, guild_id).await,
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:birthday_channel_modal",
                "Birthday announcement channel",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown birthday action `{other}`")),
    }
}

// ------------------------------------------------------------ ticketspanel

async fn render_tickets(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::tickets::settings(&data.db, guild_id).await?;
    let view = panels::TicketsView {
        panel_channel: settings.panel_channel(),
        category: settings.category(),
        log_channel: settings.log_channel(),
        staff_roles: settings.staff_roles().len(),
        max_open: settings.max_open_per_user,
        next_number: settings.next_ticket_number,
    };
    let (embed, rows) = panels::tickets_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn tickets_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_tickets(ctx, data, interaction, guild_id).await,
        "panel" => {
            let channel_id = interaction.channel_id.get();
            let settings = db::tickets::settings(&data.db, guild_id).await?;
            let (embed, rows) = panels::ticket_panel(&settings);
            serenity::ChannelId::new(channel_id)
                .send_message(
                    &ctx.http,
                    serenity::CreateMessage::new().embed(embed).components(rows),
                )
                .await?;
            reply(
                ctx,
                interaction,
                embeds::ok("Ticket panel posted."),
                Vec::new(),
                true,
            )
            .await
        }
        "category" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Category id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:tickets_category_modal",
                "Ticket category",
                inputs,
            )
            .await
        }
        "staff" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "roles",
                "Comma-separated staff role ids (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:tickets_staff_modal",
                "Ticket staff roles",
                inputs,
            )
            .await
        }
        "maxopen" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "maxopen",
                "Max open tickets per member",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:tickets_maxopen_modal",
                "Max open tickets",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown tickets action `{other}`")),
    }
}

// ---------------------------------------------------------- giveaways panel

async fn render_giveaways(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let active = db::giveaways::count_active(&data.db, guild_id)
        .await
        .unwrap_or(0);
    let view = panels::GiveawayView { active };
    let (embed, rows) = panels::giveaways_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn giveaways_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_giveaways(ctx, data, interaction, guild_id).await,
        "create" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("prize", "Prize")),
                CreateActionRow::InputText(components::short("minutes", "Minutes")),
                CreateActionRow::InputText(components::short("winners", "Winners (default 1)")),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:giveaways_create_modal",
                "New giveaway",
                inputs,
            )
            .await
        }
        "list" => {
            let embed = embeds::brand(format!("{} Giveaways", icons::GIFT))
                .color(colors::GOLD)
                .description("_Giveaways post in the channel they were created in. End them from the Giveaways panel._")
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        other => Err(anyhow::anyhow!("unknown giveaways action `{other}`")),
    }
}

// ----------------------------------------------------------- messages panel

async fn render_messages(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let groups =
        db::messages::visible_groups(&data.db, guild_id, interaction.user.id.get(), true).await?;
    let mut total: i64 = 0;
    for group in &groups {
        total += db::messages::group_message_count(&data.db, group.id)
            .await
            .unwrap_or(0);
    }
    let view = panels::MessagesView {
        group_count: groups.len(),
        message_count: total,
    };
    let (embed, rows) = panels::messages_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn messages_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_messages(ctx, data, interaction, guild_id).await,
        "groups" => {
            let groups =
                db::messages::visible_groups(&data.db, guild_id, interaction.user.id.get(), true)
                    .await?;
            let body = if groups.is_empty() {
                "_No groups yet. Create one with **Save**._".to_string()
            } else {
                groups
                    .iter()
                    .map(|group| format!("`{}` · **{}**", group.id, group.name))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let embed = embeds::brand(format!("{} Message groups", icons::MESSAGES))
                .color(colors::TEAL)
                .description(format!("{}\n{}", theme::DIVIDER, body))
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        "create" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("group", "Group id or name")),
                CreateActionRow::InputText(components::short("name", "Message name")),
                CreateActionRow::InputText(
                    components::paragraph("text", "Plain text (or leave empty for embed)")
                        .required(false),
                ),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:messages_create_modal",
                "Save message",
                inputs,
            )
            .await
        }
        "send" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("group", "Group id or name")),
                CreateActionRow::InputText(components::short("name", "Message name")),
                CreateActionRow::InputText(components::short("channel", "Channel id (optional)")),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:messages_send_modal",
                "Send message",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown messages action `{other}`")),
    }
}

// ----------------------------------------------------------- cleanup panel

async fn cleanup_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    let _ = guild_id;
    match sub {
        "" | "refresh" => {
            let (embed, rows) = panels::cleanup_panel();
            update_panel(ctx, data, interaction, embed, rows).await
        }
        "clear" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "amount",
                "How many recent messages to scan (1-100)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:cleanup_clear_modal",
                "Clear messages",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown cleanup action `{other}`")),
    }
}

// ---------------------------------------------------------- sneakpeek panel

async fn render_sneakpeek(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::sneak_peek::settings(&data.db, guild_id).await?;
    let state = db::sneak_peek::state(&data.db).await.unwrap_or_default();
    let view = panels::SneakPeekView {
        channel: settings.channel(),
        last_article: state.last_url,
        last_checked: state.checked_at,
    };
    let (embed, rows) = panels::sneakpeek_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn sneakpeek_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_sneakpeek(ctx, data, interaction, guild_id).await,
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:sneakpeek_channel_modal",
                "Sneak Peek channel",
                inputs,
            )
            .await
        }
        "check" => {
            let destinations = db::sneak_peek::destinations(&data.db).await?;
            if destinations.is_empty() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Not configured", "Set a Sneak Peek channel first."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            match crate::sneakpeek::fetch_latest_url().await {
                Ok(Some(url)) => {
                    let state = db::sneak_peek::state(&data.db).await.unwrap_or_default();
                    let status = if state.last_url.as_deref() == Some(url.as_str()) {
                        "No new article since the last check."
                    } else {
                        "New article found — it will be posted by the background poller within ~20 minutes."
                    };
                    reply(
                        ctx,
                        interaction,
                        embeds::info("Sneak Peek", format!("{status}\nLatest: {url}")),
                        Vec::new(),
                        true,
                    )
                    .await
                }
                Ok(None) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::warn(
                            "No article",
                            "No sneak-peek article found on the wiki right now.",
                        ),
                        Vec::new(),
                        true,
                    )
                    .await
                }
                Err(err) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::error("Check failed", format!("`{}`", crate::error::public(&err))),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        "sendlink" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "url",
                "Wiki article URL (e.g. https://www.whiteoutsurvival.wiki/sneak-peek/080366853en/)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:sneakpeek_sendlink_modal",
                "Post a Sneak Peek",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown sneakpeek action `{other}`")),
    }
}

// --------------------------------------------------------- server index panel

async fn render_index(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let settings = db::server_index::get(&data.db, guild_id).await?;
    let view = panels::ServerIndexView {
        channel: settings.channel(),
        enabled: settings.enabled(),
        include_threads: settings.include_threads(),
        include_voice: settings.include_voice(),
        message_count: settings.message_ids().len(),
    };
    let (embed, rows) = panels::server_index_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn index_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_index(ctx, data, interaction, guild_id).await,
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none` to clear)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:index_channel_modal",
                "Index channel",
                inputs,
            )
            .await
        }
        "toggle" => {
            let settings = db::server_index::get(&data.db, guild_id).await?;
            db::server_index::set_enabled(&data.db, guild_id, !settings.enabled()).await?;
            render_index(ctx, data, interaction, guild_id).await
        }
        "threads" => {
            let settings = db::server_index::get(&data.db, guild_id).await?;
            db::server_index::set_include_threads(&data.db, guild_id, !settings.include_threads())
                .await?;
            render_index(ctx, data, interaction, guild_id).await
        }
        "voice" => {
            let settings = db::server_index::get(&data.db, guild_id).await?;
            db::server_index::set_include_voice(&data.db, guild_id, !settings.include_voice())
                .await?;
            render_index(ctx, data, interaction, guild_id).await
        }
        "rebuild" => {
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;
            let result = crate::serverindex::publish(ctx, data, guild_id, true).await;
            let embed = match result {
                Ok(ids) => embeds::ok(format!("Index rebuilt — {} message(s).", ids.len())),
                Err(err) => embeds::error(
                    "Rebuild failed",
                    format!("`{}`", crate::error::public(&err)),
                ),
            };
            interaction
                .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                .await?;
            Ok(())
        }
        other => Err(anyhow::anyhow!("unknown index action `{other}`")),
    }
}

// --------------------------------------------------------- roles panel

fn role_name(ctx: &serenity::Context, guild_id: u64, role_id: u64) -> String {
    ctx.cache
        .guild(serenity::GuildId::new(guild_id))
        .and_then(|guild| {
            guild
                .roles
                .get(&serenity::RoleId::new(role_id))
                .map(|role| role.name.clone())
        })
        .unwrap_or_else(|| format!("role {role_id}"))
}

async fn render_roles(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let panel = db::roles::panel_settings(&data.db, guild_id)
        .await?
        .unwrap_or_default();
    let catalog = db::roles::catalog_summaries(&data.db, guild_id).await?;
    let grants = db::roles::grant_summaries(&data.db, guild_id).await?;
    let view = panels::RolesPanelView {
        channel: panel.0,
        panel_message_id: panel.1,
        catalog: catalog.into_iter().take(20).collect(),
        grants: grants
            .into_iter()
            .take(20)
            .map(|(role_id, users, roles)| (role_id, users.len(), roles.len()))
            .collect(),
    };
    let (embed, rows) = panels::roles_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn roles_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_roles(ctx, data, interaction, guild_id).await,
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none` to stop)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:roles_channel_modal",
                "Assign Role panel channel",
                inputs,
            )
            .await
        }
        "add" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "role_id",
                "Role id to add to the catalog",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:roles_add_modal",
                "Add role to catalog",
                inputs,
            )
            .await
        }
        "remove" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "role_id",
                "Role id to remove (also drops its grants)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:roles_remove_modal",
                "Remove role from catalog",
                inputs,
            )
            .await
        }
        "grant_user" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("user_id", "User id")),
                CreateActionRow::InputText(components::short(
                    "role_id",
                    "Catalog role id to let them assign",
                )),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:roles_grant_user_modal",
                "Grant user",
                inputs,
            )
            .await
        }
        "grant_role" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short(
                    "granter_role_id",
                    "Discord role id (holders can assign)",
                )),
                CreateActionRow::InputText(components::short(
                    "role_id",
                    "Catalog role id to let them assign",
                )),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:roles_grant_role_modal",
                "Grant role",
                inputs,
            )
            .await
        }
        "revoke_user" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("user_id", "User id")),
                CreateActionRow::InputText(components::short(
                    "role_id",
                    "Catalog role id to revoke",
                )),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:roles_revoke_user_modal",
                "Revoke user",
                inputs,
            )
            .await
        }
        "revoke_role" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("granter_role_id", "Discord role id")),
                CreateActionRow::InputText(components::short(
                    "role_id",
                    "Catalog role id to revoke",
                )),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:roles_revoke_role_modal",
                "Revoke role",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown roles action `{other}`")),
    }
}

// ------------------------------------------------------ notifications panel

async fn render_notifications(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let notifications = db::notifications::list_for_guild(&data.db, guild_id).await?;
    let total = notifications.len() as i64;
    let lines: Vec<String> = notifications
        .iter()
        .take(10)
        .map(crate::notifications::describe)
        .collect();
    let default_slots =
        db::settings::get(&data.db, guild_id, db::settings::keys::NOTIF_DEFAULT_SLOTS)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(2)
            .clamp(1, 5);
    let view = panels::NotificationsView {
        lines,
        default_slots,
        total,
    };
    let (embed, rows) = panels::notifications_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn notifications_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_notifications(ctx, data, interaction, guild_id).await,
        "add" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("time", "Time HH:MM (24h)")),
                CreateActionRow::InputText(
                    components::short(
                        "timezone",
                        "IANA timezone (e.g. Europe/Berlin; blank = UTC)",
                    )
                    .required(false),
                ),
                CreateActionRow::InputText(components::short("mention", "none | here | everyone")),
                CreateActionRow::InputText(components::short(
                    "repeat",
                    "Repeat every N minutes (0 = one-shot)",
                )),
                CreateActionRow::InputText(components::paragraph(
                    "description",
                    "Message text (%t = time until, %e = 🔔)",
                )),
            ];
            send_modal(
                ctx,
                interaction,
                "settings:notifications_add_modal",
                "New notification",
                inputs,
            )
            .await
        }
        "slots" => {
            let current =
                db::settings::get(&data.db, guild_id, db::settings::keys::NOTIF_DEFAULT_SLOTS)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(2)
                    .clamp(1, 5);
            let next = current % 5 + 1;
            db::settings::set(
                &data.db,
                guild_id,
                db::settings::keys::NOTIF_DEFAULT_SLOTS,
                Some(&next.to_string()),
            )
            .await?;
            render_notifications(ctx, data, interaction, guild_id).await
        }
        "toggle" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "id",
                "Notification id (from the list)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:notifications_toggle_modal",
                "Toggle notification",
                inputs,
            )
            .await
        }
        "delete" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "id",
                "Notification id (from the list)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:notifications_delete_modal",
                "Delete notification",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown notifications action `{other}`")),
    }
}

// ------------------------------------------------------- calculator panel

async fn render_calculator(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let svs = db::calculator::get(&data.db, guild_id, db::calculator::SVS).await?;
    let building = db::calculator::get(&data.db, guild_id, db::calculator::BUILDING).await?;
    let gear = db::calculator::get(&data.db, guild_id, db::calculator::CHIEF_GEAR).await?;
    let charm = db::calculator::get(&data.db, guild_id, db::calculator::CHARM).await?;
    let pets = db::calculator::get(&data.db, guild_id, db::calculator::PETS).await?;
    let waracademy = db::calculator::get(&data.db, guild_id, db::calculator::WAR_ACADEMY).await?;
    let view = panels::CalculatorView {
        svs_channel: svs.channel(),
        svs_message_id: svs.message_id(),
        building_channel: building.channel(),
        building_message_id: building.message_id(),
        gear_channel: gear.channel(),
        gear_message_id: gear.message_id(),
        charm_channel: charm.channel(),
        charm_message_id: charm.message_id(),
        pets_channel: pets.channel(),
        pets_message_id: pets.message_id(),
        waracademy_channel: waracademy.channel(),
        waracademy_message_id: waracademy.message_id(),
    };
    let (embed, rows) = panels::calculator_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn calculator_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_calculator(ctx, data, interaction, guild_id).await,
        "building_channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none` to clear)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:calculator_building_channel_modal",
                "Building panel channel",
                inputs,
            )
            .await
        }
        "building_repost" => {
            let settings =
                db::calculator::get(&data.db, guild_id, db::calculator::BUILDING).await?;
            match settings.channel() {
                Some(channel) => {
                    match crate::calculator::building::post_panel(ctx, data, guild_id, channel)
                        .await
                    {
                        Ok(id) => {
                            reply(
                                ctx,
                                interaction,
                                embeds::ok(format!("Building panel posted (`{id}`).")),
                                Vec::new(),
                                true,
                            )
                            .await
                        }
                        Err(err) => {
                            reply(
                                ctx,
                                interaction,
                                embeds::error(
                                    "Repost failed",
                                    format!("`{}`", crate::error::public(&err)),
                                ),
                                Vec::new(),
                                true,
                            )
                            .await
                        }
                    }
                }
                None => {
                    reply(
                        ctx,
                        interaction,
                        embeds::warn("Not configured", "Set a Building panel channel first."),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        "svs_channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none` to clear)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:calculator_svs_channel_modal",
                "SVS Prep panel channel",
                inputs,
            )
            .await
        }
        "svs_repost" => {
            let settings = db::calculator::get(&data.db, guild_id, db::calculator::SVS).await?;
            let Some(channel) = settings.channel() else {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Not configured", "Set an SVS panel channel first."),
                    Vec::new(),
                    true,
                )
                .await;
            };
            match crate::calculator::svs::post_panel(ctx, data, guild_id, channel).await {
                Ok(id) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::ok(format!("SVS Prep panel posted (`{id}`).")),
                        Vec::new(),
                        true,
                    )
                    .await
                }
                Err(err) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::error("Repost failed", format!("`{}`", crate::error::public(&err))),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        "gear_channel" => {
            calc_plan_channel_action(
                ctx,
                interaction,
                "settings:calculator_gear_channel_modal",
                "Chief Gear panel channel",
            )
            .await
        }
        "gear_repost" => {
            let settings =
                db::calculator::get(&data.db, guild_id, db::calculator::CHIEF_GEAR).await?;
            match settings.channel() {
                Some(_) => match crate::calculator::plan::post_panel(
                    ctx,
                    data,
                    guild_id,
                    &crate::calculator::chief_gear::CONFIG,
                )
                .await
                {
                    Ok(id) => {
                        reply(
                            ctx,
                            interaction,
                            embeds::ok(format!("Chief Gear panel posted (`{id}`).")),
                            Vec::new(),
                            true,
                        )
                        .await
                    }
                    Err(err) => {
                        reply(
                            ctx,
                            interaction,
                            embeds::error(
                                "Repost failed",
                                format!("`{}`", crate::error::public(&err)),
                            ),
                            Vec::new(),
                            true,
                        )
                        .await
                    }
                },
                None => {
                    reply(
                        ctx,
                        interaction,
                        embeds::warn("Not configured", "Set a Chief Gear panel channel first."),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        "charm_channel" => {
            calc_plan_channel_action(
                ctx,
                interaction,
                "settings:calculator_charm_channel_modal",
                "Chief Charm panel channel",
            )
            .await
        }
        "charm_repost" => {
            let settings = db::calculator::get(&data.db, guild_id, db::calculator::CHARM).await?;
            match settings.channel() {
                Some(_) => match crate::calculator::plan::post_panel(
                    ctx,
                    data,
                    guild_id,
                    &crate::calculator::charm::CONFIG,
                )
                .await
                {
                    Ok(id) => {
                        reply(
                            ctx,
                            interaction,
                            embeds::ok(format!("Chief Charm panel posted (`{id}`).")),
                            Vec::new(),
                            true,
                        )
                        .await
                    }
                    Err(err) => {
                        reply(
                            ctx,
                            interaction,
                            embeds::error(
                                "Repost failed",
                                format!("`{}`", crate::error::public(&err)),
                            ),
                            Vec::new(),
                            true,
                        )
                        .await
                    }
                },
                None => {
                    reply(
                        ctx,
                        interaction,
                        embeds::warn("Not configured", "Set a Chief Charm panel channel first."),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        "pets_channel" => {
            calc_plan_channel_action(
                ctx,
                interaction,
                "settings:calculator_pets_channel_modal",
                "Pets panel channel",
            )
            .await
        }
        "pets_repost" => {
            let settings = db::calculator::get(&data.db, guild_id, db::calculator::PETS).await?;
            match settings.channel() {
                Some(channel) => {
                    match crate::calculator::pets::post_panel(ctx, data, guild_id, channel).await {
                        Ok(()) => {
                            reply(
                                ctx,
                                interaction,
                                embeds::ok("Pets panel reposted."),
                                Vec::new(),
                                true,
                            )
                            .await
                        }
                        Err(err) => {
                            reply(
                                ctx,
                                interaction,
                                embeds::error(
                                    "Repost failed",
                                    format!("`{}`", crate::error::public(&err)),
                                ),
                                Vec::new(),
                                true,
                            )
                            .await
                        }
                    }
                }
                None => {
                    reply(
                        ctx,
                        interaction,
                        embeds::warn("Not configured", "Set a Pets panel channel first."),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        "waracademy_channel" => {
            calc_plan_channel_action(
                ctx,
                interaction,
                "settings:calculator_waracademy_channel_modal",
                "War Academy panel channel",
            )
            .await
        }
        "waracademy_repost" => {
            let settings =
                db::calculator::get(&data.db, guild_id, db::calculator::WAR_ACADEMY).await?;
            match settings.channel() {
                Some(channel) => {
                    match crate::calculator::war_academy::post_panel(ctx, data, guild_id, channel)
                        .await
                    {
                        Ok(()) => {
                            reply(
                                ctx,
                                interaction,
                                embeds::ok("War Academy panel reposted."),
                                Vec::new(),
                                true,
                            )
                            .await
                        }
                        Err(err) => {
                            reply(
                                ctx,
                                interaction,
                                embeds::error(
                                    "Repost failed",
                                    format!("`{}`", crate::error::public(&err)),
                                ),
                                Vec::new(),
                                true,
                            )
                            .await
                        }
                    }
                }
                None => {
                    reply(
                        ctx,
                        interaction,
                        embeds::warn("Not configured", "Set a War Academy panel channel first."),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        other => Err(anyhow::anyhow!("unknown calculator action `{other}`")),
    }
}

async fn calc_plan_channel_action(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    modal_id: &str,
    modal_title: &str,
) -> anyhow::Result<()> {
    let inputs = vec![CreateActionRow::InputText(components::short(
        "channel",
        "Channel id (or `none` to clear)",
    ))];
    send_modal(ctx, interaction, modal_id, modal_title, inputs).await
}

// ---------------------------------------------------------- wos relay panel

fn component_is_owner(data: &Data, interaction: &ComponentInteraction) -> bool {
    data.cfg.owner_id == Some(interaction.user.id.get())
}

async fn render_wos(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let destination = db::wos_relay::destination(&data.db, guild_id).await?;
    let hub = db::wos_relay::hub(&data.db).await.unwrap_or_default();
    let sneakpeek = db::sneak_peek::settings(&data.db, guild_id)
        .await
        .ok()
        .and_then(|s| s.channel());
    let view = panels::WosRelayView {
        destination,
        sneakpeek,
        hub_channel: if hub.guild() == Some(guild_id) {
            hub.channel()
        } else {
            None
        },
        hub_configured: hub.channel().is_some(),
        is_owner: component_is_owner(data, interaction),
    };
    let (embed, rows) = panels::wos_relay_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn wos_panel_action(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sub: &str,
    guild_id: u64,
) -> anyhow::Result<()> {
    match sub {
        "" | "refresh" => render_wos(ctx, data, interaction, guild_id).await,
        "destination" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none` to clear)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:wos_destination_modal",
                "WOS Announcements destination",
                inputs,
            )
            .await
        }
        "clear" => {
            db::wos_relay::set_destination(&data.db, guild_id, None).await?;
            render_wos(ctx, data, interaction, guild_id).await
        }
        "hub" => {
            if !component_is_owner(data, interaction) {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Hub channel id in this server",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:wos_hub_modal",
                "Set the hub channel",
                inputs,
            )
            .await
        }
        "clearhub" => {
            if !component_is_owner(data, interaction) {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            db::wos_relay::clear_hub(&data.db).await?;
            render_wos(ctx, data, interaction, guild_id).await
        }
        other => Err(anyhow::anyhow!("unknown wos action `{other}`")),
    }
}

// ----------------------------------------------------------- perm / admin

fn resolve_username(ctx: &serenity::Context, guild_id: u64, user_id: u64) -> String {
    let guild = ctx.cache.guild(serenity::GuildId::new(guild_id));
    if let Some(guild) = guild {
        if let Some(member) = guild.members.get(&serenity::UserId::new(user_id)) {
            return member.display_name().to_string();
        }
    }
    format!("User {user_id}")
}

/// The `/settings` → **Admin** page: the DB-backed permissions manager.
pub(crate) async fn admin_panel(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: u64,
) -> anyhow::Result<(CreateEmbed, Vec<CreateActionRow>)> {
    let admins = db::admin::list(&data.db).await?;
    let owner = db::admin::owner_id(&data.db).await?;

    let owner_line = match owner {
        Some(owner_id) => format!("👑 {} (`{owner_id}`)", resolve_username(ctx, guild_id, owner_id)),
        None => "⚠️ **Not yet claimed.** The first admin to click **Claim Bot Owner** below becomes the permanent owner.".to_string(),
    };
    let admin_lines: Vec<String> = admins
        .iter()
        .take(15)
        .map(|entry| {
            let icon = if entry.is_owner { "👑" } else { "🏅" };
            format!(
                "{icon} **{}** — {}",
                resolve_username(ctx, guild_id, entry.id),
                if entry.is_owner { "Bot Owner" } else { "Admin" }
            )
        })
        .collect();
    let total = admins.len();
    let admin_count = admins.iter().filter(|entry| !entry.is_owner).count();

    let embed = embeds::brand(format!("{} Permissions", icons::ADMIN))
        .color(colors::VIOLET)
        .description(concat!(
            "Permissions system controls who can manage notifications and other admins.\n",
            "Use the pickers below to add a new admin, remove one, or transfer ownership.",
        ))
        .field(
            "Tiers",
            "👑 **Bot Owner** — recovery anchor; only one; changed via Transfer Owner\n🏅 **Admin** — full access to notifications and admin management",
            false,
        )
        .field(
            "Current state",
            format!(
                "**Bot Owner:** {owner_line}\n**Total admins:** {total} ({admin_count} Admin{})",
                if admin_count == 1 { "" } else { "s" }
            ),
            false,
        )
        .field(
            format!("Admins ({total})"),
            if admin_lines.is_empty() {
                "_No admins yet — use the picker below to add the first one._".to_string()
            } else {
                admin_lines.join("\n")
            },
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let mut rows: Vec<CreateActionRow> = Vec::new();

    if owner.is_none() && !admins.is_empty() {
        rows.push(CreateActionRow::Buttons(vec![components::success(
            "perm:claim",
            format!("{} Claim Bot Owner", icons::CROWN),
        )]));
    }

    rows.push(CreateActionRow::SelectMenu(
        serenity::CreateSelectMenu::new(
            "perm:adduser",
            serenity::CreateSelectMenuKind::User {
                default_users: None,
            },
        )
        .placeholder("+ Add admin (pick a user)…")
        .min_values(0)
        .max_values(1),
    ));

    if admins.iter().any(|entry| !entry.is_owner) {
        let options: Vec<serenity::CreateSelectMenuOption> = admins
            .iter()
            .filter(|entry| !entry.is_owner)
            .take(25)
            .map(|entry| {
                let name = resolve_username(ctx, guild_id, entry.id);
                serenity::CreateSelectMenuOption::new(
                    name.chars().take(100).collect::<String>(),
                    entry.id.to_string(),
                )
                .description("Admin — tap to remove")
            })
            .collect();
        rows.push(CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                "perm:removeuser",
                serenity::CreateSelectMenuKind::String { options },
            )
            .placeholder("Remove an admin…")
            .min_values(1)
            .max_values(1),
        ));
    }

    if admins.len() > 1 {
        let options: Vec<serenity::CreateSelectMenuOption> = admins
            .iter()
            .filter(|entry| !entry.is_owner)
            .take(25)
            .map(|entry| {
                let name = resolve_username(ctx, guild_id, entry.id);
                serenity::CreateSelectMenuOption::new(
                    name.chars().take(100).collect::<String>(),
                    entry.id.to_string(),
                )
            })
            .collect();
        rows.push(CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                "perm:transferuser",
                serenity::CreateSelectMenuKind::String { options },
            )
            .placeholder("Transfer Bot Owner to…")
            .min_values(1)
            .max_values(1),
        ));
    }

    rows.push(CreateActionRow::Buttons(vec![
        components::secondary("perm:audit", format!("{} Audit log", icons::LIST)),
        components::secondary("perm:backup", format!("{} Backup DB", icons::SAVE)),
        components::secondary("settings:home", format!("{} Back", icons::BACK)),
    ]));

    Ok((embed, rows))
}

async fn is_effective_owner(data: &Data, user_id: u64) -> bool {
    if data.cfg.owner_id == Some(user_id) {
        return true;
    }
    db::admin::is_owner(&data.db, user_id).await
}

async fn refresh_perm_panel(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let (embed, rows) = admin_panel(ctx, data, guild_id).await?;
    update_response(ctx, data, interaction, embed, rows).await
}

async fn perm_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("permissions only work inside a server"))?;
    let user_id = interaction.user.id.get();

    match action {
        "claim" => {
            let admins = db::admin::list(&data.db).await?;
            if admins.is_empty() {
                return update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn(
                        "No admins",
                        "Add an admin first — the first admin becomes the owner.",
                    ),
                    Vec::new(),
                )
                .await;
            }
            if db::admin::claim_owner(&data.db, user_id, user_id).await? {
                refresh_perm_panel(ctx, data, interaction, guild_id).await
            } else {
                update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn(
                        "Already claimed",
                        "A bot owner already exists — transfer ownership instead.",
                    ),
                    Vec::new(),
                )
                .await
            }
        }
        "adduser" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::UserSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a user picker")),
            };
            let Some(target) = values.first() else {
                return refresh_perm_panel(ctx, data, interaction, guild_id).await;
            };
            let became_owner = db::admin::add(&data.db, target.get(), false, user_id).await?;
            let note = if became_owner {
                " added as the first admin — they are now **Bot Owner**."
            } else {
                " added as **Admin**."
            };
            let embed = embeds::ok(format!("<@{}>{note}", target.get()));
            update_response(ctx, data, interaction, embed, Vec::new()).await
        }
        "removeuser" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an admin picker")),
            };
            let Some(target) = values.first().and_then(|v| v.parse::<u64>().ok()) else {
                return refresh_perm_panel(ctx, data, interaction, guild_id).await;
            };
            if is_effective_owner(data, target).await {
                return update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn(
                        "Protected",
                        "The bot owner can only be changed via **Transfer Bot Owner**.",
                    ),
                    Vec::new(),
                )
                .await;
            }
            let list = db::admin::list(&data.db).await?;
            if !list.iter().any(|entry| entry.id == target) {
                return update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn("Not an admin", "That user is not an admin."),
                    Vec::new(),
                )
                .await;
            }
            match db::admin::remove(&data.db, target, user_id).await? {
                Ok(()) => {
                    let embed = embeds::ok(format!("<@{target}> removed from the admin list."));
                    update_response(ctx, data, interaction, embed, Vec::new()).await
                }
                Err(message) => {
                    update_response(
                        ctx,
                        data,
                        interaction,
                        embeds::warn("Not removed", message),
                        Vec::new(),
                    )
                    .await
                }
            }
        }
        "transferuser" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an admin picker")),
            };
            let Some(target) = values.first().and_then(|v| v.parse::<u64>().ok()) else {
                return refresh_perm_panel(ctx, data, interaction, guild_id).await;
            };
            let embed = embeds::warn(
                "Transfer Bot Owner?",
                format!(
                    "<@{target}> will become the **Bot Owner**. You will lose ownership.\nThis cannot be undone from this menu."
                ),
            );
            let rows = vec![CreateActionRow::Buttons(vec![
                components::danger(
                    format!("perm:transferconfirm:{target}"),
                    "Transfer ownership",
                ),
                components::secondary("perm:transfercancel", "Cancel"),
            ])];
            update_response(ctx, data, interaction, embed, rows).await
        }
        other if other.starts_with("transferconfirm:") => {
            let target = other
                .trim_start_matches("transferconfirm:")
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("malformed transfer id"))?;
            match db::admin::transfer_owner(&data.db, target, user_id).await? {
                Ok(()) => {
                    let embed = embeds::ok(format!("<@{target}> is now the **Bot Owner**."));
                    update_response(ctx, data, interaction, embed, Vec::new()).await
                }
                Err(message) => {
                    update_response(
                        ctx,
                        data,
                        interaction,
                        embeds::warn("Not transferred", message),
                        Vec::new(),
                    )
                    .await
                }
            }
        }
        "transfercancel" => refresh_perm_panel(ctx, data, interaction, guild_id).await,
        "audit" => {
            let entries = db::admin::audit_log(&data.db, 10).await?;
            let body = if entries.is_empty() {
                "_No admin changes recorded yet._".to_string()
            } else {
                entries
                    .iter()
                    .map(|entry| {
                        let target = entry.target_id.as_deref().unwrap_or("—");
                        format!(
                            "`#{}` · `{}` · <@{}> · {}{}",
                            entry.id,
                            entry.action,
                            entry.actor_id,
                            target,
                            if entry.target_id.is_some() {
                                format!(" → <@{target}>")
                            } else {
                                String::new()
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let embed = embeds::brand(format!("{} Audit log", icons::LIST))
                .color(colors::VIOLET)
                .description(format!("{}\n{}", theme::DIVIDER, body))
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        "backup" => {
            if !is_effective_owner(data, user_id).await {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;

            let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(&data.db.pool)
                .await;

            let path = data.cfg.database_path.clone();
            let bytes = std::fs::read(&path)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
            const MAX_UPLOAD: usize = 10 * 1024 * 1024;
            if bytes.len() > MAX_UPLOAD {
                interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(format!(
                            "Backup is {:.1} MB — too large to upload here. Pull the database file from the server instead.",
                            bytes.len() as f64 / 1_048_576.0
                        )),
                    )
                    .await?;
                return Ok(());
            }
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("bot.sqlite")
                .to_string();
            let size_mb = bytes.len() as f64 / 1_048_576.0;
            interaction
                .create_followup(
                    &ctx.http,
                    serenity::CreateInteractionResponseFollowup::new()
                        .content(format!("Here's your database backup ({size_mb:.1} MB)."))
                        .ephemeral(true)
                        .add_file(serenity::CreateAttachment::bytes(bytes, filename)),
                )
                .await?;
            Ok(())
        }
        other => Err(anyhow::anyhow!("unknown permissions action `{other}`")),
    }
}

// ------------------------------------------------------------ role flow

fn member_role_ids(interaction: &ComponentInteraction) -> Vec<u64> {
    crate::roles::actor_role_ids(interaction)
}

async fn start_assign_flow(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
    update_existing: bool,
) -> anyhow::Result<()> {
    let actor = interaction.user.id.get();
    let is_admin = permissions::component_is_admin(ctx, data, interaction).await;
    let actor_roles = member_role_ids(interaction);
    let assignable =
        crate::roles::access_for(data, guild_id, actor, &actor_roles, is_admin).await?;

    if assignable.is_empty() {
        let embed = embeds::denied("a role from the catalog");
        if update_existing {
            return update_response(ctx, data, interaction, embed, Vec::new()).await;
        }
        return reply(ctx, interaction, embed, Vec::new(), true).await;
    }

    let sid = crate::roles::new_session(guild_id, actor);
    let embed = embeds::brand("🏷️ Assign Role — Step 1/3")
        .color(colors::BRAND)
        .description("Select one or more members to update (up to 25).")
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let select = serenity::CreateSelectMenu::new(
        format!("roleassign:{sid}:pickuser"),
        serenity::CreateSelectMenuKind::User {
            default_users: None,
        },
    )
    .placeholder("Select members…")
    .min_values(1)
    .max_values(25);
    let rows = vec![CreateActionRow::SelectMenu(select)];
    if update_existing {
        update_response(ctx, data, interaction, embed, rows).await
    } else {
        reply(ctx, interaction, embed, rows, true).await
    }
}

async fn update_response(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    embed: serenity::CreateEmbed,
    components: Vec<CreateActionRow>,
) -> anyhow::Result<()> {
    let is_post = db::admin_toolkit::find_post(&data.db, interaction.message.id.get())
        .await
        .ok()
        .flatten()
        .is_some();
    if is_post {
        interaction
            .create_response(&ctx.http, message(embed, components, true))
            .await?;
        if let Ok(response) = interaction.get_response(&ctx.http).await {
            crate::admintoolkit::track_ephemeral(response.id.get());
        }
        return Ok(());
    }
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(components),
            ),
        )
        .await?;
    Ok(())
}

// ------------------------------------------------------------------ music

fn panel_response(view: panels::NowPlayingView) -> (CreateEmbed, Vec<CreateActionRow>) {
    panels::music(&view)
}

async fn music_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("music controls only work inside a server"))?;

    if action == "queue" {
        let state = data.music.state(guild_id).await.unwrap_or_default();
        let mut lines = Vec::new();
        if let Some(current) = state.current.as_ref() {
            lines.push(format!(
                "{} **Now** · {} `{}`",
                icons::PLAY,
                crate::error::truncate(current.title(), 60),
                theme::duration(current.length())
            ));
        }
        for (index, track) in state.queue.iter().take(15).enumerate() {
            lines.push(format!(
                "`{:>2}.` {} `{}`",
                index + 1,
                crate::error::truncate(track.title(), 60),
                theme::duration(track.length())
            ));
        }
        let body = if lines.is_empty() {
            "_The queue is empty._".to_string()
        } else {
            lines.join("\n")
        };
        let embed = panels::paged_list(
            &format!("{} Queue", icons::QUEUE),
            0,
            1,
            format!("{}\n{}", theme::DIVIDER, body),
        );
        return reply(ctx, interaction, embed, Vec::new(), true).await;
    }

    // Playback controls are only for people sitting in the bot's voice channel.
    let bot_id = ctx.cache.current_user().id;
    let (user_vc, bot_vc) = ctx
        .cache
        .guild(serenity::GuildId::new(guild_id))
        .map(|guild| {
            (
                guild
                    .voice_states
                    .get(&interaction.user.id)
                    .and_then(|s| s.channel_id),
                guild.voice_states.get(&bot_id).and_then(|s| s.channel_id),
            )
        })
        .unwrap_or((None, None));
    if bot_vc.is_some() && user_vc != bot_vc {
        return reply(
            ctx,
            interaction,
            embeds::warn(
                "Join the voice channel",
                "You need to be in the same voice channel as the bot to use these controls.",
            ),
            Vec::new(),
            true,
        )
        .await;
    }

    let state = data.music.state(guild_id).await.unwrap_or_default();
    match action {
        "pause" => {
            data.music.set_paused(guild_id, true).await?;
        }
        "resume" => {
            data.music.set_paused(guild_id, false).await?;
        }
        "skip" => {
            data.music.advance(guild_id).await;
        }
        "stop" => {
            data.music.stop(ctx, guild_id).await?;
        }
        "previous" => {
            if !data.music.restart_current(guild_id).await {
                return reply(
                    ctx,
                    interaction,
                    embeds::info("Nothing to restart", "There is no current track."),
                    Vec::new(),
                    true,
                )
                .await;
            }
        }
        "shuffle" => {
            data.music.shuffle(guild_id).await;
        }
        "loop" => {
            let next = state.loop_mode.next();
            data.music.set_loop_state(guild_id, next).await;
            db::music::set_loop(&data.db, guild_id, next).await?;
        }
        "volup" | "voldown" => {
            if state.current.is_none() {
                return reply(
                    ctx,
                    interaction,
                    embeds::info(
                        "Nothing is playing",
                        "Start a track from the Music panel first.",
                    ),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let current = if state.volume == 0 { 80 } else { state.volume };
            let next = if action == "volup" {
                (current + 10).min(200)
            } else {
                current.saturating_sub(10)
            };
            data.music.set_volume(guild_id, next).await?;
            db::music::set_volume(&data.db, guild_id, next).await?;
        }
        "volume_modal" => {
            let embed = embeds::brand(format!("{} Volume", icons::VOLUME))
                .description("Open the volume control from a slash command: `/volume <0-200>`.");
            return reply(ctx, interaction, embed, Vec::new(), true).await;
        }
        other => return Err(anyhow::anyhow!("unknown music action `{other}`")),
    }

    let view = data.music.view(guild_id).await;
    let (embed, rows) = panel_response(view);
    update_panel(ctx, data, interaction, embed, rows).await
}

// ------------------------------------------------------------------ lookup

async fn lookup_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("lookup only works inside a server"))?;
    match action {
        "player_search" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "query",
                "FID or in-game name",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal("lookup:player_modal", "Player lookup", inputs),
                )
                .await?;
            Ok(())
        }
        "alliance_search" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("abbr", "Alliance tag")),
                CreateActionRow::InputText(
                    components::short("state", "State (optional)").required(false),
                ),
            ];
            interaction
                .create_response(
                    &ctx.http,
                    modal("lookup:alliance_modal", "Alliance lookup", inputs),
                )
                .await?;
            Ok(())
        }
        "state_search" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "state",
                "State number",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal("lookup:state_modal", "State overview", inputs),
                )
                .await?;
            Ok(())
        }
        "leaderboard" => {
            let rows = db::players::page_for_guild(&data.db, guild_id, 10, 0).await?;
            let body = if rows.is_empty() {
                "_No tracked players yet._".to_string()
            } else {
                rows.iter()
                    .enumerate()
                    .map(|(index, row)| {
                        let medal = match index {
                            0 => icons::CROWN,
                            1 => icons::MEDAL,
                            2 => icons::BOLT,
                            _ => "▫️",
                        };
                        format!(
                            "{medal} `{:>2}.` **{}** · `{}`",
                            index + 1,
                            crate::error::truncate(row.label(), 28),
                            row.power
                                .map(theme::compact_number)
                                .unwrap_or_else(|| "—".into())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let embed = panels::paged_list(
                &format!("{} Power leaderboard", icons::CROWN),
                0,
                1,
                format!("{}\n{}", theme::DIVIDER, body),
            );
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        "refresh" => {
            let (embed, rows) = crate::commands::oracle::status_panel(data).await;
            update_panel(ctx, data, interaction, embed, rows).await
        }
        other => Err(anyhow::anyhow!("unknown lookup action `{other}`")),
    }
}

// ------------------------------------------------------------------ oracle

async fn oracle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    // Oracle credentials are global to the bot: bot owner only (matches the JS bot).
    if !is_effective_owner(data, interaction.user.id.get()).await {
        return reply(
            ctx,
            interaction,
            embeds::denied("Bot Owner"),
            Vec::new(),
            true,
        )
        .await;
    }
    if action == "paste" {
        let inputs = vec![CreateActionRow::InputText(components::paragraph(
            "snippet",
            "cURL command or Cookie/JSON paste",
        ))];
        interaction
            .create_response(
                &ctx.http,
                modal("oracle:paste_modal", "Paste Oracle session", inputs),
            )
            .await?;
        return Ok(());
    }

    // Long-running actions: acknowledge first, then edit the ephemeral reply.
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Defer(
                CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;

    let embed = match action {
        "capture" => {
            if !data.cfg.browser_enabled {
                embeds::warn(
                    "Browser capture is disabled",
                    "This build was compiled without a browser, or `ORACLE_BROWSER=0` is set. Use **Paste cURL** instead.",
                )
            } else {
                match data.oracle.refresh_session().await {
                    Ok(()) => embeds::success(
                        "Browser capture finished",
                        "A fresh session was stored and will be used immediately.",
                    ),
                    Err(err) => embeds::error(
                        "Browser capture failed",
                        format!("`{}`", crate::error::public(&err)),
                    ),
                }
            }
        }
        "test" => match data.oracle.test_and_record().await {
            Ok(message) => embeds::success("Session is valid", format!("`{message}`")),
            Err(err) => embeds::error(
                "Session failed",
                format!("`{}`", crate::error::public(&err)),
            ),
        },
        "sync" => match crate::giftcode::sync(data).await {
            Ok(outcome) => embeds::success(
                "Sync finished",
                format!(
                    "**{}** active codes · **{}** new · **{}** channel(s) notified.",
                    outcome.total,
                    outcome.fresh.len(),
                    outcome.announcements
                ),
            ),
            Err(err) => embeds::error("Sync failed", format!("`{}`", crate::error::public(&err))),
        },
        "clear" => {
            let removed = data.oracle.forget().await?;
            if removed {
                embeds::ok("Stored Oracle credentials were deleted.")
            } else {
                embeds::info("Nothing to clear", "No stored session was found.")
            }
        }
        other => return Err(anyhow::anyhow!("unknown oracle action `{other}`")),
    };

    interaction
        .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
        .await?;
    Ok(())
}

// ------------------------------------------------------------- giveaway

async fn giveaway_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let mut parts = action.split(':');
    let kind = parts.next().unwrap_or("");
    let id: i64 = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing giveaway id"))?
        .parse()?;
    let guild_id = interaction.guild_id.map(|id| id.get()).unwrap_or(0);
    match kind {
        "enter" => {
            let giveaway = db::giveaways::get(&data.db, id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no giveaway with id {id}"))?;
            if giveaway.guild_id != guild_id.to_string() {
                return Err(anyhow::anyhow!("giveaway belongs to another server"));
            }
            if !giveaway.is_active() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Giveaway ended", format!("Giveaway #{id} is over.")),
                    Vec::new(),
                    true,
                )
                .await;
            }
            if giveaway.ends_at <= db::now() * 1000 {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Giveaway ended", format!("Giveaway #{id} is over.")),
                    Vec::new(),
                    true,
                )
                .await;
            }
            if let Some(role_id) = giveaway.required_role_id() {
                let has_role = interaction
                    .member
                    .as_ref()
                    .map(|member| member.roles.iter().any(|id| id.get() == role_id))
                    .unwrap_or(false);
                if !has_role {
                    return reply(
                        ctx,
                        interaction,
                        embeds::error(
                            "Not eligible",
                            format!("You need <@&{role_id}> to enter this giveaway."),
                        ),
                        Vec::new(),
                        true,
                    )
                    .await;
                }
            }
            if let Some(member) = interaction.member.as_ref() {
                if let Err(reason) =
                    crate::commands::giveaways::check_eligibility(data, &giveaway, member).await
                {
                    return reply(
                        ctx,
                        interaction,
                        embeds::error("Not eligible", reason),
                        Vec::new(),
                        true,
                    )
                    .await;
                }
            }
            let added = db::giveaways::add_entry(&data.db, id, interaction.user.id.get()).await?;
            if added {
                let entries = db::giveaways::count_entries(&data.db, id).await?;
                reply(
                    ctx,
                    interaction,
                    embeds::ok(format!(
                        "You entered giveaway #{id}! Entries: **{entries}**"
                    )),
                    Vec::new(),
                    true,
                )
                .await
            } else {
                reply(
                    ctx,
                    interaction,
                    embeds::info(
                        "Already entered",
                        format!("You are already in giveaway #{id}."),
                    ),
                    Vec::new(),
                    true,
                )
                .await
            }
        }
        other => Err(anyhow::anyhow!("unknown giveaway action `{other}`")),
    }
}

// ------------------------------------------------------------- translate

async fn translate_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    namespace: &str,
    action: &str,
) -> anyhow::Result<()> {
    let message_id: u64 = action
        .parse()
        .map_err(|_| anyhow::anyhow!("malformed translate custom_id"))?;

    match namespace {
        "translate" => {
            let embed = embeds::info(
                "Choose a language",
                "Pick a language below — the translated copy is private to you.",
            );
            reply(
                ctx,
                interaction,
                embed,
                vec![crate::translate::language_select_row(message_id)],
                true,
            )
            .await
        }
        "translate_lang" => {
            let target = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("no language selected"))?,
                _ => return Err(anyhow::anyhow!("expected a language select menu")),
            };
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;

            let record = crate::db::translate::get(&data.db, message_id).await?;
            let client = crate::translate::translation_client();
            match record {
                None => {
                    interaction
                        .edit_response(
                            &ctx.http,
                            EditInteractionResponse::new()
                                .content("Could not find the original message to translate."),
                        )
                        .await?;
                }
                Some(record) => {
                    if record.embed_json.is_some() {
                        let embed =
                            crate::translate::translate_stored(client, &record, &target).await;
                        if let Some(embed) = embed {
                            interaction
                                .edit_response(
                                    &ctx.http,
                                    EditInteractionResponse::new().embed(embed),
                                )
                                .await?;
                        } else {
                            interaction
                                .edit_response(
                                    &ctx.http,
                                    EditInteractionResponse::new()
                                        .content("Could not decode the stored embed."),
                                )
                                .await?;
                        }
                    } else if let Some(content) = record.content {
                        let translated = crate::translate::translate_batch(
                            client,
                            std::slice::from_ref(&content),
                            &target,
                        )
                        .await;
                        let text = format!(
                            "**Private translation — only you can see this.**\n{}",
                            translated.first().cloned().unwrap_or_default()
                        );
                        interaction
                            .edit_response(&ctx.http, EditInteractionResponse::new().content(text))
                            .await?;
                    } else {
                        interaction
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new()
                                    .content("Nothing to translate for this message."),
                            )
                            .await?;
                    }
                }
            }
            Ok(())
        }
        other => Err(anyhow::anyhow!("unknown translate action `{other}`")),
    }
}

// -------------------------------------------------------------------- gift

async fn gift_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    match action {
        "add" => {
            let inputs = vec![
                CreateActionRow::InputText(components::short("code", "Gift code")),
                CreateActionRow::InputText(
                    components::short("vip", "VIP? (yes/no)").required(false),
                ),
            ];
            interaction
                .create_response(&ctx.http, modal("gift:add_modal", "Add gift code", inputs))
                .await?;
            Ok(())
        }
        "delete" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "code",
                "Code to remove",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal("gift:delete_modal", "Remove gift code", inputs),
                )
                .await?;
            Ok(())
        }
        "list" => {
            let rows = db::giftcodes::list_recent(&data.db, 15).await?;
            let body = if rows.is_empty() {
                "_No codes stored yet._".to_string()
            } else {
                rows.iter()
                    .map(|row| {
                        format!(
                            "`{}` {} · **{}** · {}",
                            row.code,
                            if row.is_vip() {
                                icons::CROWN
                            } else {
                                icons::GIFT
                            },
                            row.status,
                            embeds::db_relative(row.added_at)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let embed = embeds::brand(format!("{} Gift codes", icons::GIFT))
                .color(colors::GOLD)
                .description(format!("{}\n{}", theme::DIVIDER, body))
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        "sync" => {
            if !permissions::component_is_admin(ctx, data, interaction).await {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Manage Server"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let embed = match crate::giftcode::sync(data).await {
                Ok(outcome) => embeds::success(
                    "Gift-code sync complete",
                    format!(
                        "**{}** active · **{}** new · **{}** channel(s).",
                        outcome.total,
                        outcome.fresh.len(),
                        outcome.announcements
                    ),
                ),
                Err(err) => {
                    embeds::error("Sync failed", format!("`{}`", crate::error::public(&err)))
                }
            };
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        "announce" => {
            let embed = match crate::giftcode::announce_pending(data, 15).await {
                Ok(0) => embeds::info(
                    "Nothing to announce",
                    "Every stored code is already announced.",
                ),
                Ok(count) => embeds::ok(format!("Announced **{count}** channel(s).")),
                Err(err) => embeds::error(
                    "Announcement failed",
                    format!("`{}`", crate::error::public(&err)),
                ),
            };
            reply(ctx, interaction, embed, Vec::new(), true).await
        }
        other => Err(anyhow::anyhow!("unknown gift action `{other}`")),
    }
}

// -------------------------------------------------------------------- help

async fn help_component(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let (title, body) = match action {
        "players" => (
            format!("{} Players", icons::PLAYER),
            "**Players** — track WoS IDs, power, alliances & state.\n➡️ /settings → **Players**",
        ),
        "lookup" => (
            format!("{} Lookup", icons::LOOKUP),
            "**Lookup panel** — search players, alliances, states & leaderboards from a public panel.\n➡️ /settings → **Lookup** (or tap Search on the deployed panel)",
        ),
        "music" => (
            format!("{} Music", icons::MUSIC),
            "**Music panel** — play, pause, resume, skip, queue, volume, loop.\n➡️ /settings → **Music** → **Deploy panel here**",
        ),
        "gift" => (
            format!("{} Gift codes", icons::GIFT),
            "**Gift Codes** — sync, announce and manage WoS codes.\n➡️ /settings → **Gift Codes**",
        ),
        "oracle" => (
            format!("{} Oracle", icons::ORACLE),
            "**Oracle** — connect the session used for enrichment & code sync.\n➡️ /settings → **Oracle**",
        ),
        other => return Err(anyhow::anyhow!("unknown help section `{other}`")),
    };
    let embed = embeds::brand(title).description(format!("{}\n{}", theme::DIVIDER, body));
    reply(
        ctx,
        interaction,
        embed,
        vec![components::row(vec![components::secondary(
            "help:home",
            format!("{} Help", icons::BRAND),
        )])],
        true,
    )
    .await
}

// -------------------------------------------------------------- svs flow

fn svs_rows(session: &crate::calculator::svs::SvsSession) -> Vec<CreateActionRow> {
    use crate::calculator::svs::{day, day_activities_resolved, DAYS};

    let day_options: Vec<serenity::CreateSelectMenuOption> = DAYS
        .iter()
        .map(|(key, label, short, _, _)| {
            serenity::CreateSelectMenuOption::new((*label).to_string(), *key)
                .description((*short).to_string())
        })
        .collect();

    let (_, day_label, _, _, _) = day(&session.day_key);
    let activity_options: Vec<serenity::CreateSelectMenuOption> =
        day_activities_resolved(&session.day_key)
            .into_iter()
            .take(25)
            .map(|activity| {
                let label = format!(
                    "{} {}",
                    crate::calculator::svs::priority_emoji(&activity.priority),
                    activity.label
                );
                serenity::CreateSelectMenuOption::new(
                    label.chars().take(100).collect::<String>(),
                    activity.key,
                )
                .description(format!(
                    "{} · {} pts each",
                    activity.unit,
                    format_number_plain(activity.points)
                ))
            })
            .collect();

    let day_select = serenity::CreateSelectMenu::new(
        "svscalc:day",
        serenity::CreateSelectMenuKind::String {
            options: day_options,
        },
    )
    .placeholder(format!("Day — {day_label}"))
    .min_values(1)
    .max_values(1);

    let act_select = serenity::CreateSelectMenu::new(
        "svscalc:act",
        serenity::CreateSelectMenuKind::String {
            options: activity_options,
        },
    )
    .placeholder("Activity")
    .min_values(1)
    .max_values(1);

    vec![
        CreateActionRow::SelectMenu(day_select),
        CreateActionRow::SelectMenu(act_select),
        CreateActionRow::Buttons(vec![
            components::primary("svscalc:qty", "🔢 Quantity"),
            components::secondary("svscalc:target", "🎯 Target"),
            components::secondary("svscalc:overview", "📊 Overview"),
            components::secondary("svscalc:tips", "💡 Tips"),
        ]),
    ]
}

fn format_number_plain(n: i64) -> String {
    let abs = n.unsigned_abs();
    let s = abs.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (index, ch) in s.chars().enumerate() {
        if index > 0 && (s.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

async fn render_svs_planner(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
) -> anyhow::Result<()> {
    let session = crate::calculator::svs::session_for(user_id);
    let embed = crate::calculator::svs::planner_embed(&session);
    let rows = svs_rows(&session);
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

async fn render_svs_tips(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
) -> anyhow::Result<()> {
    let session = crate::calculator::svs::session_for(user_id);
    let tips = crate::calculator::svs::tips_for(&session.day_key);
    let (_, label, _, emoji, _) = crate::calculator::svs::day(&session.day_key);
    let body = tips
        .iter()
        .map(|tip| format!("• {tip}"))
        .collect::<Vec<_>>()
        .join("\n");
    let embed = embeds::brand(format!("{emoji} {label} — tips"))
        .color(colors::BRAND)
        .description(body)
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let rows = vec![CreateActionRow::Buttons(vec![components::secondary(
        "svscalc:back",
        format!("{} Back to planner", icons::BACK),
    )])];
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

async fn render_svs_overview(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
) -> anyhow::Result<()> {
    let session = crate::calculator::svs::session_for(user_id);
    let all = crate::calculator::svs::compute_all(&session.plans);
    let mut lines = Vec::new();
    for (key, _, short, emoji, _) in crate::calculator::svs::DAYS {
        let score = all.by_day.get(*key).cloned().unwrap_or_default();
        lines.push(format!(
            "{emoji} **{short}** — **{}** pts ({} lines)",
            crate::calculator::game_numbers::format_game_amount(score.total),
            score.lines.len()
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "💎 **Total** — **{}**",
        crate::calculator::game_numbers::format_game_amount(all.grand)
    ));
    let embed = embeds::brand("📊 SVS Prep — 5-day overview")
        .color(colors::BRAND)
        .description(lines.join("\n"))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let rows = vec![CreateActionRow::Buttons(vec![components::secondary(
        "svscalc:back",
        format!("{} Back to planner", icons::BACK),
    )])];
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

async fn svs_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let _ = data;
    let _ = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("that only works inside a server"))?;
    match action {
        "open" => {
            let user = interaction.user.id.get();
            let session = crate::calculator::svs::session_for(user);
            let embed = crate::calculator::svs::planner_embed(&session);
            let rows = svs_rows(&session);
            reply(ctx, interaction, embed, rows, true).await
        }
        other => Err(anyhow::anyhow!("unknown svs action `{other}`")),
    }
}

async fn svscalc_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let _ = data;
    let user_id = interaction.user.id.get();
    match action {
        "day" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a day picker")),
            };
            let Some(day) = values.first().cloned() else {
                return render_svs_planner(ctx, interaction, user_id).await;
            };
            let acts = crate::calculator::svs::day_activities(&day);
            let first = acts.first().map(|(key, _)| key.to_string());
            crate::calculator::svs::update_session(user_id, |session| {
                session.day_key = day;
                session.activity_key = first;
            });
            render_svs_planner(ctx, interaction, user_id).await
        }
        "act" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an activity picker")),
            };
            let Some(activity) = values.first().cloned() else {
                return render_svs_planner(ctx, interaction, user_id).await;
            };
            crate::calculator::svs::update_session(user_id, |session| {
                session.activity_key = Some(activity)
            });
            render_svs_planner(ctx, interaction, user_id).await
        }
        "qty" => {
            let session = crate::calculator::svs::session_for(user_id);
            let current = session
                .activity_key
                .as_ref()
                .and_then(|key| {
                    session
                        .plans
                        .get(&session.day_key)
                        .and_then(|plan| plan.get(key).copied())
                })
                .unwrap_or(0);
            let inputs = vec![CreateActionRow::InputText(
                components::short("quantity", "Quantity (0 clears it)").value(current.to_string()),
            )];
            interaction
                .create_response(&ctx.http, modal("svscalc:qty_modal", "Quantity", inputs))
                .await?;
            Ok(())
        }
        "target" => {
            let session = crate::calculator::svs::session_for(user_id);
            let inputs = vec![CreateActionRow::InputText(
                components::short("target", "Target SVS points (0 clears)")
                    .value(session.target.to_string()),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal("svscalc:target_modal", "Target score", inputs),
                )
                .await?;
            Ok(())
        }
        "overview" => render_svs_overview(ctx, interaction, user_id).await,
        "tips" => render_svs_tips(ctx, interaction, user_id).await,
        "back" => render_svs_planner(ctx, interaction, user_id).await,
        other => Err(anyhow::anyhow!("unknown svscalc action `{other}`")),
    }
}

// ------------------------------------------------------- building calc

fn building_plan_rows(
    session: &crate::calculator::building::BuildingSession,
) -> Vec<CreateActionRow> {
    let mut rows = vec![
        CreateActionRow::Buttons(vec![
            components::primary("buildingcalc:add", "➕ Add building"),
            components::secondary("buildingcalc:buffs", "🧙 Buffs"),
            components::success("buildingcalc:totals", "📊 Show totals"),
        ]),
        CreateActionRow::Buttons(vec![components::danger(
            "buildingcalc:clear",
            "🗑️ Clear plan",
        )]),
    ];
    if !session.entries.is_empty() {
        let options: Vec<serenity::CreateSelectMenuOption> = session
            .entries
            .iter()
            .take(25)
            .map(|entry| {
                let label = crate::calculator::building::building(&entry.building_key)
                    .map(|b| b.label.clone())
                    .unwrap_or_else(|| entry.building_key.clone());
                serenity::CreateSelectMenuOption::new(
                    format!(
                        "{label} {}→{}",
                        crate::calculator::building::level_display(entry.from),
                        crate::calculator::building::level_display(entry.to)
                    )
                    .chars()
                    .take(100)
                    .collect::<String>(),
                    entry.id.to_string(),
                )
            })
            .collect();
        rows.push(CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                "buildingcalc:remove",
                serenity::CreateSelectMenuKind::String { options },
            )
            .placeholder("Remove an entry…")
            .min_values(1)
            .max_values(1),
        ));
    }
    rows
}

fn building_back_row() -> CreateActionRow {
    CreateActionRow::Buttons(vec![components::secondary(
        "buildingcalc:back",
        format!("{} Back to plan", icons::BACK),
    )])
}

async fn send_building_hub(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
    update_existing: bool,
) -> anyhow::Result<()> {
    let session = crate::calculator::building::session_for(user_id);
    let embed = crate::calculator::building::plan_embed(&session);
    let rows = building_plan_rows(&session);
    if update_existing {
        interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(rows),
                ),
            )
            .await?;
    } else {
        reply(ctx, interaction, embed, rows, true).await?;
    }
    Ok(())
}

async fn buildingcalc_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let _ = data;
    let user_id = interaction.user.id.get();
    match action {
        "open" => send_building_hub(ctx, interaction, user_id, false).await,
        "add" => {
            let options: Vec<serenity::CreateSelectMenuOption> =
                crate::calculator::building::building_list()
                    .iter()
                    .map(|(key, label)| serenity::CreateSelectMenuOption::new(*label, *key))
                    .collect();
            let embed = embeds::brand("🏗️ Building — pick a building")
                .color(colors::BRAND)
                .description("Choose which building to upgrade, then set the level range.")
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            let rows = vec![
                CreateActionRow::SelectMenu(
                    serenity::CreateSelectMenu::new(
                        "buildingcalc:building",
                        serenity::CreateSelectMenuKind::String { options },
                    )
                    .placeholder("Select a building…")
                    .min_values(1)
                    .max_values(1),
                ),
                building_back_row(),
            ];
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(embed)
                            .components(rows),
                    ),
                )
                .await?;
            Ok(())
        }
        "building" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a building picker")),
            };
            let Some(key) = values.first().cloned() else {
                return send_building_hub(ctx, interaction, user_id, true).await;
            };
            let title = crate::calculator::building::building(&key)
                .map(|b| format!("Upgrade **{}** from which level to which level?", b.label))
                .unwrap_or_else(|| "Enter the level range.".to_string());
            let inputs = vec![
                CreateActionRow::InputText(components::short(
                    "from",
                    "From level (e.g. 25, `FC3`, `30-2`, `0` = not built)",
                )),
                CreateActionRow::InputText(components::short(
                    "to",
                    "To level (e.g. 28, `FC4`, `30-4`)",
                )),
            ];
            interaction
                .create_response(
                    &ctx.http,
                    modal(&format!("buildingcalc:range_modal:{key}"), &title, inputs),
                )
                .await?;
            Ok(())
        }
        "buffs" => {
            let session = crate::calculator::building::session_for(user_id);
            let buffs = session.buffs;
            let inputs = vec![
                CreateActionRow::InputText(
                    components::short("construction_speed", "Construction speed % (0-100)")
                        .value(buffs.construction_speed.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("flat_speed", "VP/Double Time: 0, 10, 15, 20, 30, 35")
                        .value(buffs.flat_speed.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("pet_level", "Pet level (0-5)")
                        .value(buffs.pet_level.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("zinman_level", "Zinman level (0-5, each = -3% RSS)")
                        .value(buffs.zinman_level.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("expert_level", "Expert level (0-5, shaves time)")
                        .value(buffs.expert_level.to_string()),
                ),
            ];
            interaction
                .create_response(
                    &ctx.http,
                    modal("buildingcalc:buffs_modal", "Buffs", inputs),
                )
                .await?;
            Ok(())
        }
        "totals" => {
            let session = crate::calculator::building::session_for(user_id);
            let Some(embed) = crate::calculator::building::totals_embed(&session) else {
                return update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn("Empty plan", "Add a building before showing totals."),
                    vec![building_back_row()],
                )
                .await;
            };
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(embed)
                            .components(vec![building_back_row()]),
                    ),
                )
                .await?;
            Ok(())
        }
        "back" => send_building_hub(ctx, interaction, user_id, true).await,
        "clear" => {
            crate::calculator::building::update_session(user_id, |session| session.entries.clear());
            send_building_hub(ctx, interaction, user_id, true).await
        }
        "remove" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an entry picker")),
            };
            let Some(id) = values.first().and_then(|v| v.parse::<u64>().ok()) else {
                return send_building_hub(ctx, interaction, user_id, true).await;
            };
            crate::calculator::building::update_session(user_id, |session| {
                session.entries.retain(|entry| entry.id != id)
            });
            send_building_hub(ctx, interaction, user_id, true).await
        }
        other => Err(anyhow::anyhow!("unknown buildingcalc action `{other}`")),
    }
}

// -------------------------------------------------- chief gear / charm flow

fn plan_config(kind: u8) -> &'static crate::calculator::plan::PlanConfig {
    match kind {
        0 => &crate::calculator::chief_gear::CONFIG,
        _ => &crate::calculator::charm::CONFIG,
    }
}

fn plan_namespace(kind: u8) -> &'static str {
    match kind {
        0 => "chiefgearcalc",
        _ => "charmcalc",
    }
}

async fn send_plan_hub(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
    kind: u8,
    update_existing: bool,
) -> anyhow::Result<()> {
    let config = plan_config(kind);
    let session = crate::calculator::plan::session_for(kind, user_id);
    let embed = crate::calculator::plan::plan_embed(config, &session);
    let rows = crate::calculator::plan::plan_rows(config, &session, plan_namespace(kind));
    if update_existing {
        interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(rows),
                ),
            )
            .await?;
    } else {
        reply(ctx, interaction, embed, rows, true).await?;
    }
    Ok(())
}

async fn render_plan_add_screen(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
    kind: u8,
) -> anyhow::Result<()> {
    let config = plan_config(kind);
    let session = crate::calculator::plan::session_for(kind, user_id);
    let namespace = plan_namespace(kind);

    if let Some(state) = &session.add {
        let embed = embeds::brand(format!("{} — pick a level", config.emoji))
            .color(colors::BRAND)
            .description(format!(
                "Current: {}\nTarget: {}\n\nTap **✓ Add to plan** when you're done.",
                state
                    .current
                    .as_deref()
                    .map(|lvl| config.label_of(lvl))
                    .unwrap_or_else(|| "—".to_string()),
                state
                    .target
                    .as_deref()
                    .map(|lvl| config.label_of(lvl))
                    .unwrap_or_else(|| "—".to_string()),
            ))
            .footer(serenity::CreateEmbedFooter::new(theme::footer()));
        let rows = crate::calculator::plan::add_rows(config, state, namespace);
        interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(rows),
                ),
            )
            .await?;
        return Ok(());
    }

    let options: Vec<serenity::CreateSelectMenuOption> = (config.pieces)()
        .iter()
        .map(|piece| serenity::CreateSelectMenuOption::new(piece.label, piece.key))
        .collect();
    let embed = embeds::brand(format!("{} — pick a piece", config.emoji))
        .color(colors::BRAND)
        .description("Choose what you're upgrading:")
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let rows = vec![
        CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                format!("{namespace}:piece"),
                serenity::CreateSelectMenuKind::String { options },
            )
            .placeholder("Select a piece…")
            .min_values(1)
            .max_values(1),
        ),
        CreateActionRow::Buttons(vec![components::secondary(
            format!("{namespace}:backplan"),
            "↩ Back to plan",
        )]),
    ];
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

async fn plan_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
    kind: u8,
) -> anyhow::Result<()> {
    let _ = data;
    let config = plan_config(kind);
    let namespace = plan_namespace(kind);
    let user_id = interaction.user.id.get();

    match action {
        "open" => send_plan_hub(ctx, interaction, user_id, kind, false).await,

        "add" => {
            crate::calculator::plan::update_session(kind, user_id, |session| {
                session.add = Some(crate::calculator::plan::AddState::default());
            });
            render_plan_add_screen(ctx, interaction, user_id, kind).await
        }

        "piece" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an item picker")),
            };
            let Some(selected) = values.first().cloned() else {
                return render_plan_add_screen(ctx, interaction, user_id, kind).await;
            };
            crate::calculator::plan::update_session(kind, user_id, |session| {
                let state = session
                    .add
                    .get_or_insert(crate::calculator::plan::AddState::default());
                state.slots_key = selected;
                state.current = None;
                state.target = None;
                state.current_page = 0;
                state.target_page = 0;
            });
            render_plan_add_screen(ctx, interaction, user_id, kind).await
        }

        "current_select" | "target_select" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a level picker")),
            };
            let Some(selected) = values.first().cloned() else {
                return render_plan_add_screen(ctx, interaction, user_id, kind).await;
            };
            let is_current = action == "current_select";
            crate::calculator::plan::update_session(kind, user_id, |session| {
                let state = session
                    .add
                    .get_or_insert(crate::calculator::plan::AddState::default());
                if is_current {
                    state.current = Some(selected);
                    if let Some(current) = &state.current {
                        if let Some(target) = &state.target {
                            let cur_pos = (config.levels)()
                                .iter()
                                .position(|(k, _)| *k == current.as_str());
                            let tgt_pos = (config.levels)()
                                .iter()
                                .position(|(k, _)| *k == target.as_str());
                            if cur_pos > tgt_pos {
                                state.target = None;
                            }
                        }
                    }
                } else {
                    state.target = Some(selected);
                }
            });
            render_plan_add_screen(ctx, interaction, user_id, kind).await
        }

        "currentnav" | "targetnav" => {
            let mut parts = action.split(':');
            let which = parts.next().unwrap_or("");
            let page: i64 = parts
                .next()
                .and_then(|p| p.parse().ok())
                .unwrap_or(0)
                .max(0);
            crate::calculator::plan::update_session(kind, user_id, |session| {
                let state = session
                    .add
                    .get_or_insert(crate::calculator::plan::AddState::default());
                if which == "currentnav" {
                    state.current_page = page;
                } else {
                    state.target_page = page;
                }
            });
            render_plan_add_screen(ctx, interaction, user_id, kind).await
        }

        "confirm_add" => {
            let session = crate::calculator::plan::session_for(kind, user_id);
            let valid = session.add.as_ref().is_some_and(|state| {
                let from = state
                    .current
                    .as_deref()
                    .and_then(|cur| (config.levels)().iter().position(|(k, _)| *k == cur));
                let to = state
                    .target
                    .as_deref()
                    .and_then(|tgt| (config.levels)().iter().position(|(k, _)| *k == tgt));
                matches!((from, to), (Some(from), Some(to)) if to > from)
            });
            if !valid {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn(
                        "Pick your levels",
                        "Choose a current level below the target level first.",
                    ),
                    Vec::new(),
                    true,
                )
                .await;
            }
            crate::calculator::plan::update_session(kind, user_id, |session| {
                if let Some(state) = session.add.take() {
                    session.entries.push(crate::calculator::plan::PlanEntry {
                        id: session.next_id,
                        slots_key: state.slots_key,
                        from: state.current.unwrap_or_default(),
                        to: state.target.unwrap_or_default(),
                    });
                    session.next_id += 1;
                }
            });
            send_plan_hub(ctx, interaction, user_id, kind, true).await
        }

        "backplan" => {
            crate::calculator::plan::update_session(kind, user_id, |session| {
                session.add = None;
            });
            send_plan_hub(ctx, interaction, user_id, kind, true).await
        }

        "totals" => {
            let session = crate::calculator::plan::session_for(kind, user_id);
            let namespace = plan_namespace(kind);
            let Some(embed) = crate::calculator::plan::totals_embed(config, &session) else {
                return update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn("Empty plan", "Add something to your plan first."),
                    vec![CreateActionRow::Buttons(vec![components::secondary(
                        format!("{namespace}:backplan"),
                        "↩ Back to plan",
                    )])],
                )
                .await;
            };
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(embed)
                            .components(vec![CreateActionRow::Buttons(vec![
                                components::secondary(
                                    format!("{namespace}:backplan"),
                                    "↩ Back to plan",
                                ),
                            ])]),
                    ),
                )
                .await?;
            Ok(())
        }

        "clear" => {
            crate::calculator::plan::update_session(kind, user_id, |session| {
                session.entries.clear();
            });
            send_plan_hub(ctx, interaction, user_id, kind, true).await
        }

        "remove" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an entry picker")),
            };
            let Some(id) = values.first().and_then(|v| v.parse::<u64>().ok()) else {
                return send_plan_hub(ctx, interaction, user_id, kind, true).await;
            };
            crate::calculator::plan::update_session(kind, user_id, |session| {
                session.entries.retain(|entry| entry.id != id)
            });
            send_plan_hub(ctx, interaction, user_id, kind, true).await
        }

        other => Err(anyhow::anyhow!("unknown {namespace} action `{other}`")),
    }
}

// ------------------------------------------------------------ role flow

async fn role_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("that only works inside a server"))?;
    match action {
        "assign" => start_assign_flow(ctx, data, interaction, guild_id, false).await,
        other => Err(anyhow::anyhow!("unknown role action `{other}`")),
    }
}

async fn roleassign_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let mut parts = action.split(':');
    let sid = parts.next().unwrap_or_default().to_string();
    let step = parts.next().unwrap_or_default().to_string();

    if sid == "new" && step == "again" {
        let guild_id = interaction
            .guild_id
            .map(|id| id.get())
            .ok_or_else(|| anyhow::anyhow!("that only works inside a server"))?;
        return start_assign_flow(ctx, data, interaction, guild_id, true).await;
    }

    // The session belongs to whoever opened the flow; nobody else may drive it.
    match crate::roles::get_session(&sid) {
        Some(session) if session.actor_id == interaction.user.id.get() => {}
        _ => return expired(ctx, data, interaction).await,
    }

    match step.as_str() {
        "pickuser" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::UserSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a member select menu")),
            };
            let ids: Vec<u64> = values.iter().map(|id| id.get()).collect();
            crate::roles::update_session(&sid, |session| session.target_user_ids = ids);
            show_pick_roles(ctx, data, interaction, &sid).await
        }
        "pickrole" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                serenity::ComponentInteractionDataKind::Button => {
                    return show_pick_roles(ctx, data, interaction, &sid).await;
                }
                _ => return Err(anyhow::anyhow!("expected a role select menu")),
            };
            let ids: Vec<u64> = values.iter().filter_map(|v| v.parse().ok()).collect();
            crate::roles::update_session(&sid, |session| session.role_ids = ids);
            show_confirm(ctx, data, interaction, &sid).await
        }
        "confirm" => {
            let which = parts.next().unwrap_or("");
            handle_confirm(ctx, data, interaction, &sid, which).await
        }
        "back" => {
            let session = crate::roles::get_session(&sid);
            crate::roles::update_session(&sid, |session| {
                session.target_user_ids.clear();
                session.role_ids.clear();
            });
            start_assign_flow(
                ctx,
                data,
                interaction,
                session.map(|s| s.guild_id).unwrap_or(0),
                true,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown roleassign step `{other}`")),
    }
}

async fn show_pick_roles(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sid: &str,
) -> anyhow::Result<()> {
    let Some(session) = crate::roles::get_session(sid) else {
        return expired(ctx, data, interaction).await;
    };
    let guild_id = session.guild_id;
    let actor = interaction.user.id.get();
    let is_admin = permissions::component_is_admin(ctx, data, interaction).await;
    let actor_roles = member_role_ids(interaction);
    let assignable =
        crate::roles::access_for(data, guild_id, actor, &actor_roles, is_admin).await?;
    if assignable.is_empty() {
        return update_response(
            ctx,
            data,
            interaction,
            embeds::denied("a role from the catalog"),
            Vec::new(),
        )
        .await;
    }

    let count = session.target_user_ids.len();
    let names: Vec<String> = session
        .target_user_ids
        .iter()
        .take(5)
        .map(|id| format!("<@{id}>"))
        .collect();
    let member_label = if count == 1 {
        format!("<@{}>", session.target_user_ids[0])
    } else if count <= 5 {
        names.join(", ")
    } else {
        format!("**{count}** members")
    };

    let options: Vec<serenity::CreateSelectMenuOption> = {
        let cache_guild = ctx.cache.guild(serenity::GuildId::new(guild_id));
        assignable
            .iter()
            .take(25)
            .map(|role_id| {
                let name = cache_guild
                    .as_ref()
                    .and_then(|guild| guild.roles.get(&serenity::RoleId::new(*role_id)))
                    .map(|role| role.name.clone())
                    .unwrap_or_else(|| format!("Role {role_id}"));
                serenity::CreateSelectMenuOption::new(
                    name.chars().take(100).collect::<String>(),
                    format!("{role_id}"),
                )
            })
            .collect()
    };

    let embed = embeds::brand("🏷️ Assign Role — Step 2/3")
        .color(colors::BRAND)
        .description(format!(
            "Member(s): {member_label}\nSelect one or more roles from the roles you're permitted to assign."
        ))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let select = serenity::CreateSelectMenu::new(
        format!("roleassign:{sid}:pickrole"),
        serenity::CreateSelectMenuKind::String { options },
    )
    .placeholder("Select roles…")
    .min_values(1)
    .max_values(assignable.len().min(25) as u8);
    let rows = vec![
        CreateActionRow::SelectMenu(select),
        CreateActionRow::Buttons(vec![components::secondary(
            format!("roleassign:{sid}:back"),
            format!("{} Back", icons::BACK),
        )]),
    ];
    update_response(ctx, data, interaction, embed, rows).await
}

async fn show_confirm(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sid: &str,
) -> anyhow::Result<()> {
    let Some(session) = crate::roles::get_session(sid) else {
        return expired(ctx, data, interaction).await;
    };
    let guild_id = session.guild_id;
    let is_admin = permissions::component_is_admin(ctx, data, interaction).await;
    let actor_roles = member_role_ids(interaction);
    let allowed = crate::roles::access_for(
        data,
        guild_id,
        interaction.user.id.get(),
        &actor_roles,
        is_admin,
    )
    .await?;
    let disallowed: Vec<u64> = session
        .role_ids
        .iter()
        .copied()
        .filter(|id| !allowed.contains(id))
        .collect();
    if !disallowed.is_empty() {
        return update_response(
            ctx,
            data,
            interaction,
            embeds::denied("one or more of the selected roles"),
            Vec::new(),
        )
        .await;
    }

    let count = session.target_user_ids.len();
    let member_label = if count == 1 {
        format!("<@{}>", session.target_user_ids[0])
    } else {
        format!("**{count}** members")
    };
    let role_names: Vec<String> = session
        .role_ids
        .iter()
        .map(|id| format!("<@&{id}>"))
        .collect();
    let embed = embeds::brand("🏷️ Grant or remove?")
        .color(colors::GOLD)
        .description(format!(
            "Member(s): {member_label}\nRole(s): {}\n\n**Grant** adds every selected role to every selected member (skipping anyone who already has it).\n**Remove** strips every selected role from every selected member (skipping anyone who doesn't have it).",
            role_names.join(", ")
        ))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let rows = vec![CreateActionRow::Buttons(vec![
        components::success(format!("roleassign:{sid}:confirm:add"), "Grant"),
        components::danger(format!("roleassign:{sid}:confirm:remove"), "Remove"),
        components::secondary(format!("roleassign:{sid}:pickrole"), "Cancel"),
    ])];
    update_response(ctx, data, interaction, embed, rows).await
}

async fn handle_confirm(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    sid: &str,
    action: &str,
) -> anyhow::Result<()> {
    let Some(session) = crate::roles::get_session(sid) else {
        return expired(ctx, data, interaction).await;
    };
    let guild_id = session.guild_id;
    let is_admin = permissions::component_is_admin(ctx, data, interaction).await;
    let actor_roles = member_role_ids(interaction);
    let allowed = crate::roles::access_for(
        data,
        guild_id,
        interaction.user.id.get(),
        &actor_roles,
        is_admin,
    )
    .await?;
    let disallowed: Vec<u64> = session
        .role_ids
        .iter()
        .copied()
        .filter(|id| !allowed.contains(id))
        .collect();
    if !disallowed.is_empty() {
        return update_response(
            ctx,
            data,
            interaction,
            embeds::denied("one or more of the selected roles"),
            Vec::new(),
        )
        .await;
    }

    let mut updated = 0u64;
    let mut skipped = 0u64;
    let mut failed = 0u64;
    let guild_snowflake = serenity::GuildId::new(guild_id);

    for target in &session.target_user_ids {
        let user_id = serenity::UserId::new(*target);
        let Ok(member) = ctx.http.get_member(guild_snowflake, user_id).await else {
            failed += 1;
            continue;
        };
        let member_roles: Vec<u64> = member.roles.iter().map(|id| id.get()).collect();
        let to_change: Vec<u64> = session
            .role_ids
            .iter()
            .copied()
            .filter(|role_id| {
                if action == "remove" {
                    member_roles.contains(role_id)
                } else {
                    !member_roles.contains(role_id)
                }
            })
            .collect();
        if to_change.is_empty() {
            skipped += 1;
            continue;
        }
        let mut any_ok = false;
        for role_id in &to_change {
            let role = serenity::RoleId::new(*role_id);
            let result = if action == "remove" {
                ctx.http
                    .remove_member_role(guild_snowflake, user_id, role, None)
                    .await
            } else {
                ctx.http
                    .add_member_role(guild_snowflake, user_id, role, None)
                    .await
            };
            match result {
                Ok(_) => {
                    any_ok = true;
                    let _ = crate::db::roles::log_assignment(
                        &data.db,
                        guild_id,
                        interaction.user.id.get(),
                        *target,
                        *role_id,
                        action,
                    )
                    .await;
                }
                Err(_) => failed += 1,
            }
        }
        if any_ok {
            updated += 1;
        }
    }

    let roles_text = session
        .role_ids
        .iter()
        .map(|id| format!("<@&{id}>"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut summary = format!(
        "**{roles_text}** {} **{updated}** member(s).",
        if action == "remove" {
            "removed from"
        } else {
            "granted to"
        }
    );
    if skipped > 0 {
        summary.push_str(&format!(
            "\nSkipped {skipped} member(s) who already had (or lacked) all selected roles."
        ));
    }
    if failed > 0 {
        summary.push_str(&format!(
            "\nFailed for {failed} role assignment(s) — check the bot's role sits above the selected role(s) and it has **Manage Roles**."
        ));
    }
    let embed = embeds::success("Role Assignment complete", summary);
    let rows = vec![CreateActionRow::Buttons(vec![components::primary(
        "roleassign:new:again",
        format!("{} Assign another", icons::ADD),
    )])];
    update_response(ctx, data, interaction, embed, rows).await
}

async fn expired(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
) -> anyhow::Result<()> {
    update_response(
        ctx,
        data,
        interaction,
        embeds::warn(
            "Flow expired",
            "This assign-role flow timed out after 15 minutes. Click **Assign Role** again.",
        ),
        Vec::new(),
    )
    .await
}

// ---------------------------------------------------------- panels deploy

/// Post a public music/lookup control panel in the channel the button was
/// clicked in — the button-panel replacement for the old `/setup` commands.
async fn deploy_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("that only works inside a server"))?;
    let channel_id = interaction.channel_id;

    if !permissions::component_is_admin(ctx, data, interaction).await {
        return reply(
            ctx,
            interaction,
            embeds::denied("Manage Server"),
            Vec::new(),
            true,
        )
        .await;
    }

    match action {
        "music" => {
            let volume = db::music::get_or_default(&data.db, guild_id)
                .await?
                .volume();
            let view = panels::NowPlayingView {
                volume,
                ..Default::default()
            };
            let (embed, rows) = panels::music(&view);
            let message = channel_id
                .send_message(
                    &ctx.http,
                    serenity::CreateMessage::new().embed(embed).components(rows),
                )
                .await?;
            db::music::set_panel(&data.db, guild_id, channel_id.get(), Some(message.id.get()))
                .await?;
            data.music.set_volume_state(guild_id, volume).await;
            data.music
                .set_panel(guild_id, channel_id.get(), message.id.get())
                .await;
            reply(
                ctx,
                interaction,
                embeds::ok(format!("Music panel deployed in <#{}>.", channel_id.get())),
                Vec::new(),
                true,
            )
            .await
        }
        "lookup" => {
            let (embed, rows) = panels::lookup_public_panel();
            let message = channel_id
                .send_message(
                    &ctx.http,
                    serenity::CreateMessage::new().embed(embed).components(rows),
                )
                .await?;
            db::lookup::set_channel(&data.db, guild_id, Some(channel_id.get())).await?;
            db::lookup::set_panel(
                &data.db,
                guild_id,
                db::lookup::PanelKind::Player,
                Some(message.id.get()),
            )
            .await?;
            reply(
                ctx,
                interaction,
                embeds::ok(format!(
                    "Lookup panels deployed in <#{}>.",
                    channel_id.get()
                )),
                Vec::new(),
                true,
            )
            .await
        }
        "gift_channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none`)",
            ))];
            send_modal(
                ctx,
                interaction,
                "settings:gift_announce_modal",
                "Gift-code announce channel",
                inputs,
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown deploy target `{other}`")),
    }
}

// ------------------------------------------------------------------ modals

pub async fn handle_modal(ctx: &serenity::Context, data: &Data, interaction: &ModalInteraction) {
    let custom_id = interaction.data.custom_id.clone();
    if let Err(err) = route_modal(ctx, data, interaction, &custom_id).await {
        tracing::warn!(error = %crate::error::display_anyhow(&err), id = %custom_id, "modal failed");
        let embed = embeds::error(
            "That did not work",
            format!("`{}`", crate::error::public(&err)),
        );
        let _ = interaction
            .create_response(&ctx.http, message(embed, Vec::new(), true))
            .await;
    }
}

async fn route_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
    custom_id: &str,
) -> anyhow::Result<()> {
    if let Some(range) = custom_id.strip_prefix("buildingcalc:range_modal:") {
        return building_range_modal(ctx, data, interaction, range).await;
    }
    if let Some(rest) = custom_id.strip_prefix("ownermodal:") {
        // ids look like `ownermodal:idcap:<guild_id>` / `ownermodal:giftprio:<guild_id>`
        let mut parts = rest.splitn(2, ':');
        let prefix = parts.next().unwrap_or("");
        let guild_id: u64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        return crate::owner::owner_modal(ctx, data, interaction, prefix, guild_id).await;
    }
    if let Some(rest) = custom_id.strip_prefix("giveawayrulesmodal:") {
        let mut parts = rest.splitn(2, ':');
        let prefix = parts.next().unwrap_or("");
        let id: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        return giveawayrules_modal(ctx, data, interaction, prefix, id).await;
    }
    if let Some(rest) = custom_id.strip_prefix("ballotmodal:extend:") {
        let id: i64 = rest.parse().unwrap_or(0);
        return ballot_extend_modal(ctx, data, interaction, id).await;
    }
    match custom_id {
        "oracle:paste_modal" => oracle_paste_modal(ctx, data, interaction).await,
        "gift:add_modal" => gift_add_modal(ctx, data, interaction).await,
        "gift:delete_modal" => gift_delete_modal(ctx, data, interaction).await,
        "lookup:player_modal" => lookup_player_modal(ctx, data, interaction).await,
        "lookupmodal:apikey" => lookup_apikey_modal(ctx, data, interaction).await,
        "lookup:alliance_modal" => lookup_alliance_modal(ctx, data, interaction).await,
        "lookup:state_modal" => lookup_state_modal(ctx, data, interaction).await,
        "music:volume_modal" => music_volume_modal(ctx, data, interaction).await,
        "settings:leveling_xp_modal" => leveling_xp_modal(ctx, data, interaction).await,
        "settings:leveling_cooldown_modal" => leveling_cooldown_modal(ctx, data, interaction).await,
        "settings:leveling_voice_modal" => leveling_voice_modal(ctx, data, interaction).await,
        "settings:leveling_message_modal" => leveling_message_modal(ctx, data, interaction).await,
        "settings:leveling_reset_modal" => leveling_reset_modal(ctx, data, interaction).await,
        "settings:welcome_channel_modal" => welcome_channel_modal(ctx, data, interaction).await,
        "settings:welcome_title_modal" => welcome_title_modal(ctx, data, interaction).await,
        "settings:welcome_description_modal" => {
            welcome_description_modal(ctx, data, interaction).await
        }
        "settings:logging_channel_modal" => logging_channel_modal(ctx, data, interaction).await,
        "settings:logging_categories_modal" => {
            logging_categories_modal(ctx, data, interaction).await
        }
        "settings:tempvoice_lobby_modal" => tempvoice_lobby_modal(ctx, data, interaction).await,
        "settings:tempvoice_category_modal" => {
            tempvoice_category_modal(ctx, data, interaction).await
        }
        "settings:tempvoice_template_modal" => {
            tempvoice_template_modal(ctx, data, interaction).await
        }
        "settings:tempvoice_limit_modal" => tempvoice_limit_modal(ctx, data, interaction).await,
        "settings:voting_channel_modal" => voting_channel_modal(ctx, data, interaction).await,
        "settings:voting_results_modal" => voting_results_modal(ctx, data, interaction).await,
        "settings:birthday_channel_modal" => birthday_channel_modal(ctx, data, interaction).await,
        "settings:tickets_category_modal" => tickets_category_modal(ctx, data, interaction).await,
        "settings:tickets_staff_modal" => tickets_staff_modal(ctx, data, interaction).await,
        "settings:tickets_maxopen_modal" => tickets_maxopen_modal(ctx, data, interaction).await,
        "settings:giveaways_create_modal" => giveaways_create_modal(ctx, data, interaction).await,
        "settings:messages_create_modal" => messages_create_modal(ctx, data, interaction).await,
        "settings:messages_send_modal" => messages_send_modal(ctx, data, interaction).await,
        "settings:cleanup_clear_modal" => cleanup_clear_modal(ctx, data, interaction).await,
        "settings:sneakpeek_channel_modal" => sneakpeek_channel_modal(ctx, data, interaction).await,
        "settings:sneakpeek_sendlink_modal" => {
            sneakpeek_sendlink_modal(ctx, data, interaction).await
        }
        "settings:wos_destination_modal" => wos_destination_modal(ctx, data, interaction).await,
        "settings:wos_hub_modal" => wos_hub_modal(ctx, data, interaction).await,
        "settings:notifications_add_modal" => notifications_add_modal(ctx, data, interaction).await,
        "settings:notifications_toggle_modal" => {
            notifications_toggle_modal(ctx, data, interaction).await
        }
        "settings:notifications_delete_modal" => {
            notifications_delete_modal(ctx, data, interaction).await
        }
        "settings:index_channel_modal" => index_channel_modal(ctx, data, interaction).await,
        "settings:roles_channel_modal" => roles_channel_modal(ctx, data, interaction).await,
        "settings:roles_add_modal" => roles_add_modal(ctx, data, interaction).await,
        "settings:roles_remove_modal" => roles_remove_modal(ctx, data, interaction).await,
        "settings:roles_grant_user_modal" => roles_grant_user_modal(ctx, data, interaction).await,
        "settings:roles_grant_role_modal" => roles_grant_role_modal(ctx, data, interaction).await,
        "settings:roles_revoke_user_modal" => roles_revoke_user_modal(ctx, data, interaction).await,
        "settings:roles_revoke_role_modal" => roles_revoke_role_modal(ctx, data, interaction).await,
        "settings:calculator_svs_channel_modal" => {
            calculator_svs_channel_modal(ctx, data, interaction).await
        }
        "settings:calculator_building_channel_modal" => {
            calculator_building_channel_modal(ctx, data, interaction).await
        }
        "settings:calculator_gear_channel_modal" => {
            calculator_plan_channel_modal(
                ctx,
                data,
                interaction,
                crate::db::calculator::CHIEF_GEAR,
                "Chief Gear",
            )
            .await
        }
        "settings:calculator_charm_channel_modal" => {
            calculator_plan_channel_modal(
                ctx,
                data,
                interaction,
                crate::db::calculator::CHARM,
                "Chief Charm",
            )
            .await
        }
        "settings:calculator_pets_channel_modal" => {
            calculator_pets_channel_modal(ctx, data, interaction).await
        }
        "settings:calculator_waracademy_channel_modal" => {
            calculator_war_academy_channel_modal(ctx, data, interaction).await
        }
        "petcalcmodal:inventory" => pets_inventory_modal(ctx, data, interaction).await,
        "waracademycalcmodal:levels" => war_academy_levels_modal(ctx, data, interaction).await,
        "waracademycalcmodal:buffs" => war_academy_buffs_modal(ctx, data, interaction).await,
        "settings:gift_announce_modal" => gift_announce_modal(ctx, data, interaction).await,
        "brandingmodal:contact" => branding_modal(ctx, data, interaction, "contact").await,
        "brandingmodal:nickname" => branding_modal(ctx, data, interaction, "nickname").await,
        "brandingmodal:avatar" => branding_modal(ctx, data, interaction, "avatar").await,
        "ministrymodal:googlecode" => ministry_googlecode_modal(ctx, data, interaction).await,
        "ministrymodal:svs" => ministry_svs_modal(ctx, data, interaction).await,
        "ministrymodal:channel" => ministry_channel_modal(ctx, data, interaction).await,
        "buildingcalc:buffs_modal" => building_buffs_modal(ctx, data, interaction).await,
        "ballotmodal:create" => ballot_create_modal(ctx, data, interaction).await,
        "ballotmodal:addq" => ballot_add_question_modal(ctx, data, interaction).await,
        "tvcmodal:rename" | "tvcmodal:limit" => tempvoice_modal(ctx, data, interaction).await,
        "svscalc:qty_modal" => svs_qty_modal(ctx, data, interaction).await,
        "svscalc:target_modal" => svs_target_modal(ctx, data, interaction).await,
        other => Err(anyhow::anyhow!("unknown modal `{other}`")),
    }
}

async fn oracle_paste_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !is_effective_owner(data, interaction.user.id.get()).await {
        return finish_modal(ctx, interaction, embeds::denied("Bot Owner")).await;
    }
    let snippet = trimmed(interaction, "snippet")
        .ok_or_else(|| anyhow::anyhow!("paste a cURL command or cookie/token pair"))?;
    let parsed = crate::oracle::parse_curl(&snippet)
        .map_err(|err| anyhow::anyhow!("could not read the paste: {err}"))?;
    data.oracle
        .save(&parsed, Some(interaction.user.id.get()))
        .await?;
    defer_modal(ctx, interaction).await?;

    let embed = match data.oracle.test_and_record().await {
        Ok(message) => embeds::success(
            "Oracle connected",
            format!("Session saved and verified.\n`{message}`"),
        ),
        Err(err) => embeds::warn(
            "Session saved, test failed",
            format!(
                "`{}`\nCheck that the paste came from a signed-in session.",
                crate::error::public(&err)
            ),
        ),
    };
    finish_modal(ctx, interaction, embed).await
}

async fn gift_add_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !is_effective_owner(data, interaction.user.id.get()).await {
        return finish_modal(ctx, interaction, embeds::denied("Bot Owner")).await;
    }
    let code = trimmed(interaction, "code")
        .ok_or_else(|| anyhow::anyhow!("enter a gift code"))?
        .to_uppercase();
    if !crate::oracle::valid_code(&code) {
        return finish_modal(
            ctx,
            interaction,
            embeds::error("Invalid code", "Codes are 4–32 letters or digits."),
        )
        .await;
    }
    let vip = trimmed(interaction, "vip")
        .map(|value| crate::wos::truthy_flag(&value))
        .unwrap_or(false);
    defer_modal(ctx, interaction).await?;

    let guild = guild_of(interaction)?;
    let fresh = db::giftcodes::insert(
        &data.db,
        &code,
        interaction.user.id.get(),
        guild,
        "manual",
        None,
        vip,
    )
    .await?;
    let embed = if fresh {
        embeds::success(
            "Gift code added",
            format!(
                "`{code}` is on the list{}. Use **Announce** to post it.",
                if vip { " as VIP" } else { "" }
            ),
        )
    } else {
        embeds::info("Already known", format!("`{code}` was added earlier."))
    };
    finish_modal(ctx, interaction, embed).await
}

async fn gift_delete_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !is_effective_owner(data, interaction.user.id.get()).await {
        return finish_modal(ctx, interaction, embeds::denied("Bot Owner")).await;
    }
    let code = trimmed(interaction, "code")
        .ok_or_else(|| anyhow::anyhow!("enter the code to remove"))?
        .to_uppercase();
    defer_modal(ctx, interaction).await?;

    let removed = db::giftcodes::delete(&data.db, &code).await?;
    let embed = if removed {
        embeds::ok(format!("`{code}` was removed."))
    } else {
        embeds::warn("Not found", format!("`{code}` is not in the list."))
    };
    finish_modal(ctx, interaction, embed).await
}

async fn lookup_player_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let guild = guild_of(interaction)?;
    let query =
        trimmed(interaction, "query").ok_or_else(|| anyhow::anyhow!("enter a FID or name"))?;
    defer_modal(ctx, interaction).await?;

    let outcome = match crate::commands::lookup::player_by_query(data, guild, &query).await {
        Ok((view, raw)) => {
            let (embed, _rows) = panels::lookup_player(&view);
            (embed, raw, Some(view))
        }
        Err(err) => (
            embeds::error(
                "Player not found",
                format!(
                    "`{query}` did not match a tracked or live player.\n`{}`",
                    crate::error::public(&err)
                ),
            ),
            None,
            None,
        ),
    };

    let (embed, raw, view) = outcome;
    // Render the image cards. Oracle-raw data unlocks the full set
    // (arena, chief gear, cosmetics, standings); tracked DB rows get the
    // player overview card from what the DB has.
    let asset_base = data.cfg.oracle_site.clone();
    let mut files: Vec<serenity::CreateAttachment> = Vec::new();
    {
        let mut card_tasks: Vec<(&str, Vec<u8>)> = Vec::new();

        // Player overview banner (raw Oracle shape, or the tracked DB row)
        let overview = match raw.as_ref() {
            Some(raw) => Some(overview_from_raw(raw)),
            None => view.as_ref().map(overview_from_view),
        };
        if let Some(overview) = overview {
            if let Ok(bytes) = crate::cards::render_player_overview_card(&overview).await {
                card_tasks.push(("player.png", bytes));
            }
        }

        if let Some(raw) = raw.as_ref() {
            // Heroes & gear (arena lineup)
            let heroes = raw
                .get("heroes")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Ok(Some(bytes)) = crate::cards::render_arena_lineup(&heroes, &asset_base).await {
                card_tasks.push(("arena.png", bytes));
            }

            // Lord gear + per-hero cards from the loadout blob
            let loadout_source = raw
                .get("oracleLoadout")
                .or_else(|| raw.get("loadout"))
                .unwrap_or(raw);
            let loadout = crate::cards::render_loadout_cards(loadout_source, &asset_base).await;
            if let Some(bytes) = loadout.chief {
                card_tasks.push(("chief-gear.png", bytes));
            }

            // Cosmetics strip
            let cosmetics =
                crate::cards::extract_cosmetics(&asset_base, Some(raw), Some(loadout_source));
            if let Ok(Some(bytes)) =
                crate::cards::render_cosmetics_card(&cosmetics, &asset_base).await
            {
                card_tasks.push(("cosmetics.png", bytes));
            }

            // Standings table (when the profile carries leaderboards)
            let standings = standings_from_raw(raw);
            if !standings.is_empty() {
                let name = raw
                    .get("nick_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&query)
                    .to_string();
                let kid = raw.get("kid").and_then(serde_json::Value::as_i64);
                if let Ok(bytes) = crate::cards::render_standings_card(&name, kid, &standings) {
                    card_tasks.push(("standings.png", bytes));
                }
            }
        }

        for (filename, bytes) in card_tasks {
            files.push(serenity::CreateAttachment::bytes(bytes, filename));
        }
    }

    let mut edit = EditInteractionResponse::new().embed(embed);
    for file in files {
        edit = edit.new_attachment(file);
    }
    interaction.edit_response(&ctx.http, edit).await?;
    Ok(())
}

/// Build the overview-card view from a raw Oracle/Whiteout Stats profile.
fn overview_from_raw(raw: &serde_json::Value) -> crate::cards::PlayerOverview {
    let str_field = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| raw.get(*key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let num_field = |keys: &[&str]| {
        keys.iter().find_map(|key| raw.get(*key)).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|v| v as i64))
                .or_else(|| {
                    value
                        .as_str()
                        .and_then(|s| s.replace(',', "").parse::<f64>().ok())
                        .map(|v| v as i64)
                })
        })
    };

    let tg = str_field(&["tg_label", "level_label"])
        .or_else(|| num_field(&["tg_level"]).map(|level| format!("FC{level}")));
    let alliance = raw.get("alliance");
    let alliance_tag = str_field(&["alliance_abbr", "alliance_tag", "tag"]).or_else(|| {
        alliance.and_then(|a| {
            a.get("tag")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    });
    let alliance_name = str_field(&["alliance_name"]).or_else(|| {
        alliance.and_then(|a| {
            a.get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    });
    let alliance_rank_label = str_field(&["alliance_rank_label"]).or_else(|| {
        alliance.and_then(|a| {
            a.get("rankLabel")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    });

    let fid = str_field(&["fid", "governor_id", "chief_id", "uid"])
        .or_else(|| num_field(&["fid"]).map(|n| n.to_string()));
    let kid = num_field(&["kid", "state", "server"]);
    let x = num_field(&["x", "pos_x"]);
    let y = num_field(&["y", "pos_y"]);
    let location = match (x, y) {
        (Some(x), Some(y)) => Some(format!("X {x} · Y {y}")),
        _ => None,
    };
    let shield = raw.get("shield");
    let burn = raw.get("burn");
    let (shield_label, shield_active) = match shield {
        Some(s) => (
            s.get("label").and_then(|v| v.as_str()).map(str::to_string)
                .or_else(|| if s.get("active").and_then(|v| v.as_bool()).unwrap_or(false) { Some("Shielded".into()) } else { Some("No shield".into()) }),
            s.get("active").and_then(|v| v.as_bool()).unwrap_or(false),
        ),
        None => (Some("No shield".into()), false),
    };
    let (burn_label, burn_active) = match burn {
        Some(b) => (
            b.get("label").and_then(|v| v.as_str()).map(str::to_string)
                .or_else(|| if b.get("active").and_then(|v| v.as_bool()).unwrap_or(false) { Some("Burning".into()) } else { Some("Not burning".into()) }),
            b.get("active").and_then(|v| v.as_bool()).unwrap_or(false),
        ),
        None => (Some("Not burning".into()), false),
    };
    // Boards / ranks
    let boards = raw.get("boards").or_else(|| raw.get("leaderboards")).and_then(|v| v.as_array());
    let board_rank = |keys: &[&str]| -> Option<i64> {
        let list = boards?;
        for k in keys {
            for b in list {
                let key = b.get("key").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                if key == *k || name.contains(&k.replace('_', " ")) {
                    if let Some(r) = b.get("rank").and_then(|v| v.as_i64()) {
                        return Some(r);
                    }
                    if let Some(r) = b.get("kingdom_rank").and_then(|v| v.as_i64()) {
                        return Some(r);
                    }
                }
            }
        }
        None
    };
    let mut troop_labels = Vec::new();
    let mut troop_lines = Vec::new();
    if let Some(arr) = raw.get("troop").and_then(|v| v.as_array()) {
        for t in arr {
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("Troop");
            let tier = t.get("tier").and_then(|v| v.as_i64());
            let fc = t.get("fc").and_then(|v| v.as_i64());
            if let Some(tier) = tier {
                troop_labels.push(name.to_string());
                troop_lines.push(format!(
                    "T{tier} · FC{}",
                    fc.map(|n| n.to_string()).unwrap_or_else(|| "—".into())
                ));
            }
        }
    }

    crate::cards::PlayerOverview {
        name: str_field(&["nick_name", "name", "nickname"]).unwrap_or_else(|| "Chief".to_string()),
        alliance_tag,
        alliance_name,
        alliance_rank_label,
        alliance_members: num_field(&["alliance_members"]).or_else(|| {
            alliance.and_then(|a| a.get("members").and_then(|v| v.as_i64()))
        }),
        alliance_flag_url: str_field(&["alliance_flag_url", "flag_url"]),
        role: str_field(&["role", "alliance_role"]),
        fid: fid.clone(),
        kid,
        furnace: num_field(&["stove_lv", "furnace_level", "furnace"]).map(|n| format!("F{n}")),
        level_label: tg,
        avatar_url: str_field(&["avatar_url", "avatar", "icon"]),
        power: num_field(&["power", "total_power"]),
        kills: num_field(&["kills", "total_kills"]),
        vip: num_field(&["vip_level", "vip"]).map(|n| n.to_string()),
        power_rank: num_field(&["power_rank", "powerRank"]).or_else(|| board_rank(&["personal_power", "power", "3"])),
        kills_rank: num_field(&["kills_rank", "killsRank"]).or_else(|| board_rank(&["kill_count", "kills", "4"])),
        migrant_score: num_field(&["migrant_score", "transfer_score", "migrantScore"]),
        migrant_rank: num_field(&["migrant_rank", "migrantRank"]).or_else(|| board_rank(&["transfer_score", "migrant", "19"])),
        lab_progress: num_field(&["labyrinth", "mystic_trial", "labyrinth_progress"]),
        lab_rank: num_field(&["lab_rank", "mystic_rank", "labRank"]).or_else(|| board_rank(&["labyrinth", "20", "the labyrinth", "mystic_trial"])),
        shield_label,
        shield_active,
        burn_label,
        burn_active,
        location,
        chief_id: fid,
        last_power_change: None,
        last_updated: None,
        crowns: num_field(&["crowns", "crown"]),
        troop_lines,
        troop_labels,
        redeem_note: str_field(&["redeem_note", "redeemNote"]),
        as_of: None,
    }
}

/// Build the overview-card view from a tracked DB row (`/lookup` with no
/// live Oracle payload).
fn overview_from_view(view: &panels::LookupPlayerView) -> crate::cards::PlayerOverview {
    let alliance = view.alliance.clone().unwrap_or_default();
    let tag = alliance
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .map(str::to_string)
        .filter(|tag| !tag.is_empty());
    let name = alliance
        .find("] ")
        .map(|index| alliance[index + 2..].trim().to_string())
        .filter(|name| !name.is_empty());

    crate::cards::PlayerOverview {
        name: view.game_name.clone().unwrap_or_else(|| view.fid.clone()),
        alliance_tag: tag,
        alliance_name: name,
        alliance_rank_label: None,
        alliance_members: None,
        alliance_flag_url: None,
        role: None,
        fid: Some(view.fid.clone()),
        kid: view.state,
        furnace: view.town_hall.map(|level| format!("F{level}")),
        level_label: None,
        avatar_url: view.avatar_url.clone(),
        power: view.power,
        kills: view.kills,
        vip: None,
        power_rank: None,
        kills_rank: None,
        migrant_score: None,
        migrant_rank: None,
        lab_progress: None,
        lab_rank: None,
        shield_label: None,
        shield_active: false,
        burn_label: None,
        burn_active: false,
        location: None,
        chief_id: Some(view.fid.clone()),
        last_power_change: None,
        last_updated: None,
        crowns: None,
        troop_lines: Vec::new(),
        troop_labels: Vec::new(),
        redeem_note: None,
        as_of: None,
    }
}

/// Build standings rows from a raw profile carrying `leaderboards`.
fn standings_from_raw(raw: &serde_json::Value) -> Vec<crate::cards::StandingRow> {
    raw.get("leaderboards")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|board| {
                    let name = board
                        .get("name")
                        .and_then(serde_json::Value::as_str)?
                        .to_string();
                    let score = board
                        .get("value_label")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            board
                                .get("value")
                                .and_then(serde_json::Value::as_i64)
                                .map(|v| v.to_string())
                        })
                        .unwrap_or_else(|| "—".to_string());
                    let rank = board
                        .get("kingdom_rank_label")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            board
                                .get("kingdom_rank")
                                .and_then(serde_json::Value::as_i64)
                                .map(|v| format!("#{v}"))
                        })
                        .unwrap_or_else(|| "—".to_string());
                    Some(crate::cards::StandingRow {
                        board: name,
                        score,
                        rank,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn lookup_alliance_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let guild = guild_of(interaction)?;
    let raw =
        trimmed(interaction, "abbr").ok_or_else(|| anyhow::anyhow!("enter an alliance tag"))?;
    let abbr = crate::wos::normalize_alliance_tag(&raw).unwrap_or_else(|| raw.to_uppercase());
    let state = trimmed(interaction, "state").and_then(|value| value.parse::<i64>().ok());
    defer_modal(ctx, interaction).await?;

    let tracked = db::players::by_alliance(&data.db, guild, &abbr, 200).await?;
    let mut roster: Vec<(String, Option<i64>, Option<String>)> = Vec::new();
    for row in tracked.iter() {
        let role = row.town_hall_level.map(|lv| format!("F{lv}"));
        roster.push((row.label().to_string(), row.power, role));
    }
    roster.sort_by_key(|(_, power, _)| std::cmp::Reverse(power.unwrap_or(0)));
    roster.truncate(5);

    let tracked_state = tracked.iter().filter_map(|row| row.state).next();

    let (embed, alliance_card) = if !tracked.is_empty() {
        let mut top: Vec<(String, i64)> = tracked
            .iter()
            .map(|row| (row.label().to_string(), row.power.unwrap_or(0)))
            .collect();
        top.sort_by_key(|item| std::cmp::Reverse(item.1));
        let members = tracked.len();
        let power: i64 = tracked.iter().filter_map(|row| row.power).sum();
        let abbr_display = abbr.clone();
        let view = panels::LookupAllianceView {
            abbr: abbr.clone(),
            name: tracked.iter().find_map(|row| row.alliance_name.clone()),
            members,
            power,
            kills: tracked.iter().filter_map(|row| row.kills).sum(),
            top,
            synced: tracked.iter().filter_map(|row| row.last_oracle_sync).max(),
        };
        let card = crate::cards::AllianceOverview {
            tag: abbr_display,
            name: view.name.clone().unwrap_or_else(|| abbr.clone()),
            state_id: tracked_state,
            leader: None,
            leader_role: None,
            power: Some(power),
            power_rank: None,
            kills: Some(view.kills),
            kills_rank: None,
            members: Some(members as i64),
            member_max: None,
            rank: None,
            level: None,
            labyrinth: None,
            lab_rank: None,
            notice: None,
            roster,
        };
        (panels::lookup_alliance(&view).0, Some(card))
    } else {
        match data.lookup.alliance(&abbr, state).await {
            Ok(profile) => {
                let view = panels::LookupAllianceView {
                    abbr: profile.abbr.clone(),
                    name: profile.name.clone(),
                    members: profile.members.len(),
                    power: profile
                        .power
                        .unwrap_or_else(|| profile.members.iter().map(|m| m.power).sum()),
                    kills: profile.kills.unwrap_or_default(),
                    top: profile
                        .members
                        .iter()
                        .take(10)
                        .map(|member| (member.name.clone(), member.power))
                        .collect::<Vec<_>>(),
                    synced: None,
                };
                let mut roster: Vec<(String, Option<i64>, Option<String>)> = profile
                    .members
                    .iter()
                    .take(6)
                    .map(|member| (member.name.clone(), Some(member.power), None))
                    .collect();
                roster.sort_by_key(|(_, power, _)| std::cmp::Reverse(power.unwrap_or(0)));
                let card = crate::cards::AllianceOverview {
                    tag: profile.abbr.clone(),
                    name: profile.name.clone().unwrap_or_else(|| abbr.clone()),
                    state_id: profile.state,
                    leader: None,
                    leader_role: None,
                    power: Some(view.power),
                    power_rank: None,
                    kills: Some(view.kills),
                    kills_rank: None,
                    members: Some(profile.members.len() as i64),
                    member_max: None,
                    rank: None,
                    level: None,
                    labyrinth: None,
                    lab_rank: None,
                    notice: None,
                    roster,
                };
                (panels::lookup_alliance(&view).0, Some(card))
            }
            Err(err) => (
                embeds::error(
                    "Alliance not found",
                    format!("`[{abbr}]`\n`{}`", crate::error::public(&err)),
                ),
                None,
            ),
        }
    };

    let mut files: Vec<serenity::CreateAttachment> = Vec::new();
    if let Some(card) = alliance_card {
        if let Ok(bytes) = crate::cards::render_alliance_overview_card(&card) {
            files.push(serenity::CreateAttachment::bytes(bytes, "alliance.png"));
        }
    }
    let mut edit = EditInteractionResponse::new().embed(embed);
    for file in files {
        edit = edit.new_attachment(file);
    }
    interaction.edit_response(&ctx.http, edit).await?;
    Ok(())
}

async fn lookup_state_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let state = trimmed(interaction, "state")
        .ok_or_else(|| anyhow::anyhow!("enter a state number"))?
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("state numbers are numeric"))?;
    defer_modal(ctx, interaction).await?;

    let (embed, state_card) = match data.lookup.state(state).await {
        Ok(profile) => {
            let alliances = profile.alliances.len();
            let players = profile.players.len();
            let power: i64 = profile.alliances.iter().map(|a| a.power).sum();
            let top_power = profile.alliances.iter().map(|a| a.power).max();
            let view = panels::StateView {
                state: state.max(0) as u64,
                alliances,
                players,
                power,
                top_alliances: profile
                    .alliances
                    .iter()
                    .map(|a| (a.abbr.clone(), a.power, a.members))
                    .collect::<Vec<_>>(),
            };
            let card = crate::cards::StateOverview {
                kid: state.max(0),
                players: Some(players as i64),
                alliances: Some(alliances as i64),
                power: Some(power),
                avg_power: (players > 0).then_some(power / players as i64),
                top_power,
                ..Default::default()
            };
            (panels::lookup_state(&view).0, Some(card))
        }
        Err(err) => (
            embeds::error(
                "State not found",
                format!("`#{state}`\n`{}`", crate::error::public(&err)),
            ),
            None,
        ),
    };

    let mut files: Vec<serenity::CreateAttachment> = Vec::new();
    if let Some(card) = state_card {
        if let Ok(bytes) = crate::cards::render_state_overview_card(&card) {
            files.push(serenity::CreateAttachment::bytes(bytes, "state.png"));
        }
    }
    let mut edit = EditInteractionResponse::new().embed(embed);
    for file in files {
        edit = edit.new_attachment(file);
    }
    interaction.edit_response(&ctx.http, edit).await?;
    Ok(())
}

async fn lookup_apikey_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if data.cfg.owner_id != Some(interaction.user.id.get()) {
        return finish_modal(ctx, interaction, embeds::denied("Bot Owner")).await;
    }
    let raw = trimmed(interaction, "api_key").unwrap_or_default();
    let Some(key) = db::wos_credentials::parse_api_key_paste(&raw) else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Key not understood",
                "Paste just the key (e.g. `wos_xxxx`) or the full `Authorization: Bearer wos_xxxx` header.",
            ),
        )
        .await;
    };
    defer_modal(ctx, interaction).await?;
    db::wos_credentials::set(&data.db, &key, interaction.user.id.get()).await?;
    data.lookup.set_api_key(Some(key));
    finish_modal(
        ctx,
        interaction,
        embeds::ok("Whiteout Stats API key saved. Press **Test** to verify the connection."),
    )
    .await
}

async fn music_volume_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return finish_modal(
            ctx,
            interaction,
            embeds::error("Not in a server", "Volume is per-server."),
        )
        .await;
    };
    let percent = trimmed(interaction, "volume")
        .ok_or_else(|| anyhow::anyhow!("enter a number between 0 and 200"))?
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("volume must be a number"))?
        .min(200);
    defer_modal(ctx, interaction).await?;

    let embed = match data.music.set_volume(guild_id, percent).await {
        Ok(()) => {
            let _ = db::music::set_volume(&data.db, guild_id, percent).await;
            embeds::ok(format!("Volume set to **{percent}%**."))
        }
        Err(err) => embeds::error("Volume failed", format!("`{}`", crate::error::public(&err))),
    };
    finish_modal(ctx, interaction, embed).await
}

// -------------------------------------------------------- leveling modals

async fn leveling_xp_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let xp_min = trimmed(interaction, "xp_min")
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| anyhow::anyhow!("xp_min must be a number"))?;
    let xp_max = trimmed(interaction, "xp_max")
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| anyhow::anyhow!("xp_max must be a number"))?;
    defer_modal(ctx, interaction).await?;

    db::leveling::set_xp_min(&data.db, guild, xp_min.max(0)).await?;
    db::leveling::set_xp_max(&data.db, guild, xp_max.max(xp_min.max(0))).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("XP range set to **{xp_min}–{xp_max}**.")),
    )
    .await
}

async fn leveling_cooldown_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let cooldown = trimmed(interaction, "cooldown")
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| anyhow::anyhow!("cooldown must be a number of seconds"))?;
    defer_modal(ctx, interaction).await?;

    db::leveling::set_cooldown(&data.db, guild, cooldown.clamp(0, 3600)).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "XP cooldown set to **{}s**.",
            cooldown.clamp(0, 3600)
        )),
    )
    .await
}

async fn leveling_voice_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let voice_xp = trimmed(interaction, "voice_xp")
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| anyhow::anyhow!("voice_xp must be a number"))?;
    defer_modal(ctx, interaction).await?;

    db::leveling::set_voice_xp(&data.db, guild, voice_xp.clamp(0, 1000)).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "Voice XP set to **{}**/min.",
            voice_xp.clamp(0, 1000)
        )),
    )
    .await
}

async fn leveling_message_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let message = trimmed(interaction, "announce_message")
        .ok_or_else(|| anyhow::anyhow!("message cannot be empty"))?;
    defer_modal(ctx, interaction).await?;

    db::leveling::set_announce_message(&data.db, guild, &message).await?;
    finish_modal(ctx, interaction, embeds::ok("Level-up message saved.")).await
}

async fn leveling_reset_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let confirm = trimmed(interaction, "confirm").unwrap_or_default();
    defer_modal(ctx, interaction).await?;

    if !confirm.eq_ignore_ascii_case("RESET") {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Not reset", "Type `RESET` exactly to wipe the leaderboard."),
        )
        .await;
    }
    let removed = db::leveling::reset_guild(&data.db, guild).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Wiped **{removed}** XP record(s).")),
    )
    .await
}

// -------------------------------------------------------- welcome modals

async fn welcome_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let raw = trimmed(interaction, "channel").unwrap_or_else(|| "none".to_string());
    let channel = opt_parse_channel(&raw)?;
    defer_modal(ctx, interaction).await?;

    db::welcome::set_welcome_channel(&data.db, guild, channel).await?;
    let embed = match channel {
        Some(id) => embeds::ok(format!("Welcome messages now post in <#{id}>.")),
        None => embeds::info("Welcome disabled", "No welcome channel will be posted to."),
    };
    finish_modal(ctx, interaction, embed).await
}

async fn welcome_title_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let title =
        trimmed(interaction, "title").ok_or_else(|| anyhow::anyhow!("title cannot be empty"))?;
    defer_modal(ctx, interaction).await?;

    db::welcome::set_welcome_field(&data.db, guild, db::welcome::WelcomeField::Title, &title)
        .await?;
    finish_modal(ctx, interaction, embeds::ok("Welcome title saved.")).await
}

async fn welcome_description_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let description = trimmed(interaction, "description")
        .ok_or_else(|| anyhow::anyhow!("description cannot be empty"))?;
    defer_modal(ctx, interaction).await?;

    db::welcome::set_welcome_field(
        &data.db,
        guild,
        db::welcome::WelcomeField::Description,
        &description,
    )
    .await?;
    finish_modal(ctx, interaction, embeds::ok("Welcome description saved.")).await
}

// -------------------------------------------------------- logging modals

async fn logging_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let raw = trimmed(interaction, "channel").unwrap_or_else(|| "none".to_string());
    let channel = opt_parse_channel(&raw)?;
    defer_modal(ctx, interaction).await?;

    db::logging::set_channel(&data.db, guild, channel).await?;
    finish_modal(ctx, interaction, embeds::ok("Log channel updated.")).await
}

async fn logging_categories_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let category = trimmed(interaction, "category")
        .ok_or_else(|| anyhow::anyhow!("`category` is required"))?
        .to_ascii_lowercase();
    let state = trimmed(interaction, "state")
        .ok_or_else(|| anyhow::anyhow!("`state` must be on or off"))?
        .to_ascii_lowercase();
    if !db::logging::CATEGORIES.contains(&category.as_str()) {
        return finish_modal(
            ctx,
            interaction,
            embeds::error(
                "Unknown category",
                format!("`{category}` — pick one of the listed ones."),
            ),
        )
        .await;
    }
    defer_modal(ctx, interaction).await?;

    let mut disabled = db::logging::disabled(&data.db, guild).await?;
    match state.as_str() {
        "off" => {
            if !disabled.iter().any(|c| c == &category) {
                disabled.push(category.clone());
            }
        }
        _ => disabled.retain(|c| c != &category),
    }
    db::logging::set_disabled(&data.db, guild, &disabled).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("`{category}` toggled.")),
    )
    .await
}

fn channel_from_interaction(
    interaction: &ModalInteraction,
    id: &str,
) -> Result<Option<u64>, anyhow::Error> {
    let raw = trimmed(interaction, id).unwrap_or_else(|| "none".to_string());
    opt_parse_channel(&raw)
}

// ----------------------------------------------------- tempvoice modals

async fn tempvoice_lobby_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::tempvoice::set_lobby_opt(&data.db, guild, channel).await?;
    finish_modal(ctx, interaction, embeds::ok("Lobby updated.")).await
}

async fn tempvoice_category_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::tempvoice::set_category(&data.db, guild, channel).await?;
    finish_modal(ctx, interaction, embeds::ok("Category updated.")).await
}

async fn tempvoice_template_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let template = trimmed(interaction, "template")
        .ok_or_else(|| anyhow::anyhow!("template cannot be empty"))?;
    defer_modal(ctx, interaction).await?;

    db::tempvoice::set_name_template(&data.db, guild, template.trim()).await?;
    finish_modal(ctx, interaction, embeds::ok("Name template saved.")).await
}

async fn tempvoice_limit_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let limit = trimmed(interaction, "limit")
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or_else(|| anyhow::anyhow!("limit must be a number between 0 and 99"))?;
    defer_modal(ctx, interaction).await?;

    db::tempvoice::set_user_limit(&data.db, guild, limit).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("User limit set to **{limit}**.")),
    )
    .await
}

// -------------------------------------------------------- voting modals

async fn voting_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::voting::set_channel(&data.db, guild, channel).await?;
    finish_modal(ctx, interaction, embeds::ok("Vote channel updated.")).await
}

async fn voting_results_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::voting::set_results(&data.db, guild, channel).await?;
    finish_modal(ctx, interaction, embeds::ok("Results channel updated.")).await
}

// -------------------------------------------------------- birthday modals

async fn birthday_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::birthdays::set_channel(&data.db, guild, channel, None).await?;
    let embed = match channel {
        Some(id) => embeds::ok(format!("Birthdays will post in <#{id}>.")),
        None => embeds::ok("Birthday announcements disabled."),
    };
    finish_modal(ctx, interaction, embed).await
}

// -------------------------------------------------------- tickets modals

async fn tickets_category_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::tickets::set_category(&data.db, guild, channel).await?;
    finish_modal(ctx, interaction, embeds::ok("Ticket category updated.")).await
}

async fn tickets_staff_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let raw = trimmed(interaction, "roles").unwrap_or_else(|| "none".to_string());
    let mut roles: Vec<u64> = Vec::new();
    if !raw.eq_ignore_ascii_case("none") {
        for part in raw.split(',') {
            let part = part
                .trim()
                .trim_start_matches(['<', '@', '&'])
                .trim_end_matches('>');
            if let Ok(id) = part.parse::<u64>() {
                roles.push(id);
            }
        }
    }
    defer_modal(ctx, interaction).await?;

    db::tickets::set_staff_roles(&data.db, guild, &roles).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Staff roles updated ({}).", roles.len())),
    )
    .await
}

async fn tickets_maxopen_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let max = trimmed(interaction, "maxopen")
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!("max open must be a number"))?
        .clamp(1, 10);
    defer_modal(ctx, interaction).await?;

    db::tickets::set_max_open(&data.db, guild, max).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Max open tickets **{max}**.")),
    )
    .await
}

// ----------------------------------------------------- giveaways modals

async fn giveaways_create_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let prize =
        trimmed(interaction, "prize").ok_or_else(|| anyhow::anyhow!("prize is required"))?;
    let minutes: i64 = trimmed(interaction, "minutes")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("minutes must be a number"))?;
    let winners: u64 = trimmed(interaction, "winners")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .clamp(1, 20);
    defer_modal(ctx, interaction).await?;

    let channel = interaction.channel_id.get();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let ends_at = now_ms + minutes.max(1) * 60_000;
    let id = db::giveaways::create(
        &data.db,
        guild,
        channel,
        prize.trim(),
        None,
        winners,
        interaction.user.id.get(),
        None,
        ends_at,
    )
    .await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Giveaway #{id} created.")),
    )
    .await
}

// -------------------------------------------------------- messages modals

async fn group_id_or_name(data: &Data, guild: u64, raw: &str) -> Result<i64, anyhow::Error> {
    let raw = raw.trim();
    if let Ok(id) = raw.parse::<i64>() {
        return Ok(id);
    }
    let groups = db::messages::visible_groups(&data.db, guild, 0, true).await?;
    groups
        .into_iter()
        .find(|g| g.name.eq_ignore_ascii_case(raw))
        .map(|g| g.id)
        .ok_or_else(|| anyhow::anyhow!("no group matching `{raw}`"))
}

async fn messages_create_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let group =
        trimmed(interaction, "group").ok_or_else(|| anyhow::anyhow!("group is required"))?;
    let name = trimmed(interaction, "name").ok_or_else(|| anyhow::anyhow!("name is required"))?;
    let text = trimmed(interaction, "text");
    defer_modal(ctx, interaction).await?;

    let group_id = group_id_or_name(data, guild, &group).await?;
    let input = crate::db::messages::MessageInput {
        name: name.trim(),
        is_embed: text.is_none(),
        plain_text: text.as_deref(),
        mention_type: None,
        title: None,
        description: None,
        color: None,
        footer: None,
        author: None,
        image_url: None,
        thumbnail_url: None,
    };
    // Create the group if it does not exist yet.
    if group.parse::<i64>().is_err() {
        let _ = db::messages::create_group(
            &data.db,
            guild,
            group.trim(),
            db::messages::Visibility::Selected,
            interaction.user.id.get(),
        )
        .await;
    }
    db::messages::upsert_message(&data.db, group_id, &input, interaction.user.id.get()).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Saved `{}`.", name.trim())),
    )
    .await
}

async fn messages_send_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let group =
        trimmed(interaction, "group").ok_or_else(|| anyhow::anyhow!("group is required"))?;
    let name = trimmed(interaction, "name").ok_or_else(|| anyhow::anyhow!("name is required"))?;
    let channel = trimmed(interaction, "channel").and_then(|v| v.parse::<u64>().ok());
    defer_modal(ctx, interaction).await?;

    let group_id = group_id_or_name(data, guild, &group).await?;
    let record = db::messages::get_message(&data.db, group_id, name.trim()).await?;
    let Some((message, _embed)) = record else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Unknown message",
                format!("No message named `{}` in group {group_id}.", name.trim()),
            ),
        )
        .await;
    };

    let target = channel.unwrap_or_else(|| interaction.channel_id.get());
    let content = message.plain_text.unwrap_or_default();
    let mention = message.mention_type.as_deref().unwrap_or("none");
    let mention_text = match mention {
        "here" => "@here\n".to_string(),
        "everyone" => "@everyone\n".to_string(),
        _ => String::new(),
    };

    let sent = serenity::ChannelId::new(target)
        .send_message(
            &ctx.http,
            serenity::CreateMessage::new()
                .content(format!("{mention_text}{content}"))
                // Only the stored mention type may ping; stray @everyone / roles / users
                // in the text must not.
                .allowed_mentions(
                    serenity::CreateAllowedMentions::new()
                        .everyone(matches!(mention, "here" | "everyone")),
                ),
        )
        .await?;

    // Attach the owner's contact button when branding is enabled (matches the
    // legacy "Message Necromancer" link under Messages-feature posts).
    if let Ok(branding) = db::branding::branding(&data.db).await {
        if branding.is_enabled() {
            if let Some(url) = branding.contact_url.as_deref().filter(|u| !u.is_empty()) {
                let row = serenity::CreateActionRow::Buttons(vec![crate::ui::components::link(
                    branding.label(),
                    url,
                )]);
                let _ = serenity::ChannelId::new(target)
                    .edit_message(
                        &ctx.http,
                        sent.id,
                        serenity::EditMessage::new().components(vec![row]),
                    )
                    .await;
            }
        }
    }

    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("`{}` posted.", message.name)),
    )
    .await
}

// -------------------------------------------------------- cleanup modals

async fn cleanup_clear_modal(
    ctx: &serenity::Context,
    _data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, _data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let amount = trimmed(interaction, "amount")
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!("amount must be a number between 1 and 100"))?
        .clamp(1, 100);
    defer_modal(ctx, interaction).await?;

    let channel_id = interaction.channel_id.get();
    let channel_id = serenity::ChannelId::new(channel_id);
    let mut deleted = 0u64;
    let mut skipped_old = 0u64;
    let mut last_id: Option<serenity::MessageId> = None;
    // Discord refuses bulk-deleting messages older than 14 days (1h safety margin).
    let cutoff = chrono::Utc::now().timestamp() - 14 * 24 * 3600 + 3600;
    while deleted < amount {
        let builder = serenity::GetMessages::new().limit(100u8);
        let builder = if let Some(id) = last_id {
            builder.before(id)
        } else {
            builder
        };
        let messages = channel_id.messages(&ctx.http, builder).await?;
        if messages.is_empty() {
            break;
        }
        last_id = messages.last().map(|m| m.id);
        let remaining = (amount - deleted) as usize;
        let mut batch: Vec<serenity::MessageId> = Vec::new();
        for m in messages.iter().take(remaining) {
            if m.id.created_at().unix_timestamp() >= cutoff {
                batch.push(m.id);
            } else {
                skipped_old += 1;
            }
        }
        match batch.len() {
            0 => {}
            1 => channel_id.delete_message(&ctx.http, batch[0]).await?,
            _ => channel_id.delete_messages(&ctx.http, batch.clone()).await?,
        }
        deleted += batch.len() as u64;
        // Messages arrive newest-first: once one is too old, the rest are too.
        if skipped_old > 0 {
            break;
        }
    }

    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "Deleted {deleted} messages.{}",
            if skipped_old > 0 {
                " Messages older than 14 days can't be bulk-deleted and were skipped."
            } else {
                ""
            }
        )),
    )
    .await
}

// -------------------------------------------------- sneak peek + wos modals

async fn sneakpeek_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::sneak_peek::set_channel(&data.db, guild, channel).await?;
    let embed = match channel {
        Some(id) => embeds::ok(format!("Sneak Peeks will post in <#{id}>.")),
        None => embeds::ok("Sneak Peek auto-posts disabled."),
    };
    finish_modal(ctx, interaction, embed).await
}

async fn sneakpeek_sendlink_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let url = trimmed(interaction, "url").ok_or_else(|| anyhow::anyhow!("url is required"))?;
    if !url.starts_with("https://www.whiteoutsurvival.wiki/sneak-peek/") {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Bad URL",
                "Paste a full `whiteoutsurvival.wiki/sneak-peek/<id>en/` link.",
            ),
        )
        .await;
    }
    let channel = db::sneak_peek::settings(&data.db, guild).await?.channel();
    let Some(channel_id) = channel else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Not configured", "Set a Sneak Peek channel first."),
        )
        .await;
    };
    defer_modal(ctx, interaction).await?;

    match crate::sneakpeek::fetch_post(&url).await {
        Ok(post) => {
            let embed = crate::sneakpeek::embed(&post);
            let message = serenity::ChannelId::new(channel_id)
                .send_message(&ctx.http, serenity::CreateMessage::new().embed(embed))
                .await?;
            let _ = serenity::ChannelId::new(channel_id)
                .edit_message(
                    &ctx.http,
                    message.id,
                    serenity::EditMessage::new()
                        .components(vec![crate::translate::translate_row(message.id.get())]),
                )
                .await;
            let _ = crate::translate::register(
                &data.db,
                message.id.get(),
                crate::db::translate::NewTranslatable {
                    guild_id: Some(guild),
                    channel_id: Some(channel_id),
                    kind: "sneakpeek",
                    embed_json: crate::sneakpeek::stored_embed(&post),
                    content: None,
                    owner_id: None,
                },
            )
            .await;
            finish_modal(ctx, interaction, embeds::ok("Sneak Peek posted.")).await
        }
        Err(err) => {
            finish_modal(
                ctx,
                interaction,
                embeds::error("Fetch failed", format!("`{}`", crate::error::public(&err))),
            )
            .await
        }
    }
}

async fn wos_destination_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::wos_relay::set_destination(&data.db, guild, channel).await?;
    let embed = match channel {
        Some(id) => embeds::ok(format!("Relayed posts will appear in <#{id}>.")),
        None => embeds::ok("WOS relay destination cleared."),
    };
    finish_modal(ctx, interaction, embed).await
}

async fn wos_hub_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if data.cfg.owner_id != Some(interaction.user.id.get()) {
        return finish_modal(ctx, interaction, embeds::denied("Bot Owner")).await;
    }
    let guild = guild_of(interaction)?;
    let raw = trimmed(interaction, "channel").unwrap_or_default();
    let channel: u64 = raw
        .trim_start_matches(['<', '#'])
        .trim_end_matches('>')
        .parse()
        .map_err(|_| anyhow::anyhow!("not a channel id"))?;
    defer_modal(ctx, interaction).await?;

    db::wos_relay::set_hub(&data.db, guild, channel).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Hub set to <#{channel}>.")),
    )
    .await
}

async fn index_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    db::server_index::set_channel(&data.db, guild, channel).await?;
    let embed = match channel {
        Some(id) => embeds::ok(format!("Index will post in <#{id}>.")),
        None => embeds::ok("Server Index channel cleared."),
    };
    finish_modal(ctx, interaction, embed).await
}

fn parse_role_id(raw: &str) -> Result<u64, anyhow::Error> {
    let cleaned = raw
        .trim()
        .trim_start_matches(['<', '@', '&', '#'])
        .trim_end_matches('>')
        .trim();
    cleaned
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("`{raw}` is not a role id"))
}

fn parse_user_id(raw: &str) -> Result<u64, anyhow::Error> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    digits
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("`{raw}` is not a user id"))
}

async fn roles_require_catalog_role(
    data: &Data,
    guild: u64,
    raw_role: &str,
) -> anyhow::Result<u64> {
    let role_id = parse_role_id(raw_role)?;
    if !db::roles::in_catalog(&data.db, guild, role_id).await? {
        return Err(anyhow::anyhow!(
            "role `{role_id}` is not in the catalog — add it first"
        ));
    }
    Ok(role_id)
}

async fn roles_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let raw = trimmed(interaction, "channel").unwrap_or_else(|| "none".to_string());
    let channel = opt_parse_channel(&raw)?;
    defer_modal(ctx, interaction).await?;

    match channel {
        Some(channel_id) => {
            let _ = crate::roles::post_panel(ctx, data, guild, channel_id).await?;
            finish_modal(
                ctx,
                interaction,
                embeds::ok(format!("Assign Role panel posted in <#{channel_id}>.")),
            )
            .await
        }
        None => {
            db::roles::set_panel(&data.db, guild, 0, 0).await?; // 0/0 marks cleared
            finish_modal(ctx, interaction, embeds::ok("Assign Role panel cleared.")).await
        }
    }
}

async fn roles_add_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let role_id = parse_role_id(
        &trimmed(interaction, "role_id").ok_or_else(|| anyhow::anyhow!("role_id is required"))?,
    )?;
    defer_modal(ctx, interaction).await?;

    db::roles::add_to_catalog(&data.db, guild, role_id, interaction.user.id.get()).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("<@&{role_id}> added to the catalog.")),
    )
    .await
}

async fn roles_remove_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let role_id = parse_role_id(
        &trimmed(interaction, "role_id").ok_or_else(|| anyhow::anyhow!("role_id is required"))?,
    )?;
    defer_modal(ctx, interaction).await?;

    db::roles::remove_from_catalog(&data.db, guild, role_id).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("<@&{role_id}> removed (grants dropped).")),
    )
    .await
}

async fn roles_grant_user_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let user = parse_user_id(&trimmed(interaction, "user_id").unwrap_or_default())?;
    let role_raw =
        trimmed(interaction, "role_id").ok_or_else(|| anyhow::anyhow!("role_id is required"))?;
    let role_id = roles_require_catalog_role(data, guild, &role_raw).await?;
    defer_modal(ctx, interaction).await?;

    db::roles::grant_user(&data.db, guild, user, role_id, interaction.user.id.get()).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("<@{user}> can now assign <@&{role_id}>.")),
    )
    .await
}

async fn roles_revoke_user_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let user = parse_user_id(&trimmed(interaction, "user_id").unwrap_or_default())?;
    let role_id = parse_role_id(
        &trimmed(interaction, "role_id").ok_or_else(|| anyhow::anyhow!("role_id is required"))?,
    )?;
    defer_modal(ctx, interaction).await?;

    db::roles::revoke_user(&data.db, guild, user, role_id).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Revoked <@&{role_id}> from <@{user}>.")),
    )
    .await
}

async fn roles_grant_role_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let granter = parse_role_id(
        &trimmed(interaction, "granter_role_id")
            .ok_or_else(|| anyhow::anyhow!("granter_role_id is required"))?,
    )?;
    let role_raw =
        trimmed(interaction, "role_id").ok_or_else(|| anyhow::anyhow!("role_id is required"))?;
    let role_id = roles_require_catalog_role(data, guild, &role_raw).await?;
    defer_modal(ctx, interaction).await?;

    db::roles::grant_role(&data.db, guild, granter, role_id, interaction.user.id.get()).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "Holders of <@&{granter}> can now assign <@&{role_id}>."
        )),
    )
    .await
}

async fn roles_revoke_role_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let granter = parse_role_id(
        &trimmed(interaction, "granter_role_id")
            .ok_or_else(|| anyhow::anyhow!("granter_role_id is required"))?,
    )?;
    let role_id = parse_role_id(
        &trimmed(interaction, "role_id").ok_or_else(|| anyhow::anyhow!("role_id is required"))?,
    )?;
    defer_modal(ctx, interaction).await?;

    db::roles::revoke_role(&data.db, guild, granter, role_id).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "Revoked <@&{role_id}> from holders of <@&{granter}>."
        )),
    )
    .await
}

// ----------------------------------------------------- notifications modals

async fn notifications_add_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel_id = interaction.channel_id.get();
    let time =
        trimmed(interaction, "time").ok_or_else(|| anyhow::anyhow!("time is required (HH:MM)"))?;
    let (hour, minute) = time
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("time must look like `HH:MM`"))?;
    let hour: i64 = hour
        .parse()
        .map_err(|_| anyhow::anyhow!("hour must be 00-23"))?;
    let minute: i64 = minute
        .parse()
        .map_err(|_| anyhow::anyhow!("minute must be 00-59"))?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Bad time", "Hour must be 00-23 and minute 00-59."),
        )
        .await;
    }
    let tz_raw = trimmed(interaction, "timezone").unwrap_or_else(|| "UTC".to_string());
    let timezone: chrono_tz::Tz = tz_raw
        .parse()
        .map_err(|_| anyhow::anyhow!("`{tz_raw}` is not an IANA timezone"))?;
    let mention_raw = trimmed(interaction, "mention")
        .unwrap_or_else(|| "none".to_string())
        .to_ascii_lowercase();
    let mention = match mention_raw.as_str() {
        "none" | "here" | "everyone" => mention_raw,
        _ => {
            return finish_modal(
                ctx,
                interaction,
                embeds::warn("Bad mention", "Use `none`, `here` or `everyone`."),
            )
            .await
        }
    };
    let repeat_minutes: i64 = trimmed(interaction, "repeat")
        .and_then(|v| v.parse().ok())
        .map(|v: i64| v.clamp(0, 1440))
        .unwrap_or(0);
    let description = trimmed(interaction, "description")
        .ok_or_else(|| anyhow::anyhow!("description is required"))?;

    let default_slots = db::settings::get(&data.db, guild, db::settings::keys::NOTIF_DEFAULT_SLOTS)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(2)
        .clamp(1, 5);

    defer_modal(ctx, interaction).await?;

    let id = db::notifications::create(
        &data.db,
        guild,
        db::notifications::NewNotification {
            channel_id,
            hour,
            minute,
            timezone: timezone.name(),
            description: description.trim(),
            notification_type: default_slots,
            mention_type: &mention,
            repeat_enabled: repeat_minutes > 0,
            repeat_minutes,
            created_by: interaction.user.id.get(),
            custom_delete_delay_minutes: None,
        },
    )
    .await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "Notification #{id} scheduled for **{hour:02}:{minute:02}** ({}).",
            timezone.name()
        )),
    )
    .await
}

async fn notifications_toggle_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let id: i64 = match trimmed(interaction, "id").and_then(|v| v.parse().ok()) {
        Some(id) => id,
        None => {
            return finish_modal(
                ctx,
                interaction,
                embeds::warn("Bad id", "Enter a numeric notification id."),
            )
            .await
        }
    };
    defer_modal(ctx, interaction).await?;

    let list = db::notifications::list_for_guild(&data.db, guild).await?;
    let Some(notification) = list.into_iter().find(|n| n.id == id) else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Unknown id",
                format!("No notification #{id} in this server."),
            ),
        )
        .await;
    };
    let enabled = !notification.enabled();
    db::notifications::set_enabled(&data.db, id, enabled).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "Notification #{id} {}.",
            if enabled { "enabled" } else { "paused" }
        )),
    )
    .await
}

async fn notifications_delete_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let id: i64 = match trimmed(interaction, "id").and_then(|v| v.parse().ok()) {
        Some(id) => id,
        None => {
            return finish_modal(
                ctx,
                interaction,
                embeds::warn("Bad id", "Enter a numeric notification id."),
            )
            .await
        }
    };
    defer_modal(ctx, interaction).await?;

    let list = db::notifications::list_for_guild(&data.db, guild).await?;
    if !list.iter().any(|n| n.id == id) {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Unknown id",
                format!("No notification #{id} in this server."),
            ),
        )
        .await;
    }
    db::notifications::delete(&data.db, id).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("Notification #{id} deleted.")),
    )
    .await
}

// -------------------------------------------------------- gift modal

async fn gift_announce_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    crate::db::settings::set(
        &data.db,
        guild,
        crate::db::settings::keys::GIFT_ANNOUNCE_CHANNEL,
        channel.map(|id| id.to_string()).as_deref(),
    )
    .await?;
    let embed = match channel {
        Some(id) => embeds::ok(format!("Gift-code announcements will post in <#{id}>.")),
        None => embeds::ok("Gift-code announcements disabled."),
    };
    finish_modal(ctx, interaction, embed).await
}

// ---------------------------------------------------------- calculator modals

async fn calculator_building_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    if let Some(channel_id) = channel {
        match crate::calculator::building::post_panel(ctx, data, guild, channel_id).await {
            Ok(_) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::ok(format!("Building panel posted in <#{channel_id}>.")),
                )
                .await
            }
            Err(err) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::error("Couldn't post", format!("`{}`", crate::error::public(&err))),
                )
                .await
            }
        }
    } else {
        crate::db::calculator::set_channel(
            &data.db,
            guild,
            crate::db::calculator::BUILDING,
            None,
            None,
        )
        .await?;
        finish_modal(
            ctx,
            interaction,
            embeds::ok("Building panel channel cleared."),
        )
        .await
    }
}

async fn calculator_svs_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    if let Some(channel_id) = channel {
        match crate::calculator::svs::post_panel(ctx, data, guild, channel_id).await {
            Ok(_) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::ok(format!("SVS Prep panel posted in <#{channel_id}>.")),
                )
                .await
            }
            Err(err) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::error("Couldn't post", format!("`{}`", crate::error::public(&err))),
                )
                .await
            }
        }
    } else {
        crate::db::calculator::set_channel(&data.db, guild, crate::db::calculator::SVS, None, None)
            .await?;
        finish_modal(
            ctx,
            interaction,
            embeds::ok("SVS Prep panel channel cleared."),
        )
        .await
    }
}

async fn calculator_pets_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    if let Some(channel_id) = channel {
        crate::db::calculator::set_channel(
            &data.db,
            guild,
            crate::db::calculator::PETS,
            Some(channel_id),
            None,
        )
        .await?;
        match crate::calculator::pets::post_panel(ctx, data, guild, channel_id).await {
            Ok(()) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::ok(format!("Pets panel posted in <#{channel_id}>.")),
                )
                .await
            }
            Err(err) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::error("Couldn't post", format!("`{}`", crate::error::public(&err))),
                )
                .await
            }
        }
    } else {
        crate::db::calculator::set_channel(
            &data.db,
            guild,
            crate::db::calculator::PETS,
            None,
            None,
        )
        .await?;
        finish_modal(ctx, interaction, embeds::ok("Pets panel channel cleared.")).await
    }
}

async fn calculator_war_academy_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    if let Some(channel_id) = channel {
        crate::db::calculator::set_channel(
            &data.db,
            guild,
            crate::db::calculator::WAR_ACADEMY,
            Some(channel_id),
            None,
        )
        .await?;
        match crate::calculator::war_academy::post_panel(ctx, data, guild, channel_id).await {
            Ok(()) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::ok(format!("War Academy panel posted in <#{channel_id}>.")),
                )
                .await
            }
            Err(err) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::error("Couldn't post", format!("`{}`", crate::error::public(&err))),
                )
                .await
            }
        }
    } else {
        crate::db::calculator::set_channel(
            &data.db,
            guild,
            crate::db::calculator::WAR_ACADEMY,
            None,
            None,
        )
        .await?;
        finish_modal(
            ctx,
            interaction,
            embeds::ok("War Academy panel channel cleared."),
        )
        .await
    }
}

async fn calculator_plan_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
    calc_key: &str,
    label: &str,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    let config = match calc_key {
        crate::db::calculator::CHIEF_GEAR => &crate::calculator::chief_gear::CONFIG,
        crate::db::calculator::CHARM => &crate::calculator::charm::CONFIG,
        _ => {
            return finish_modal(
                ctx,
                interaction,
                embeds::warn("Unsupported", "Unknown calculator."),
            )
            .await
        }
    };

    if let Some(channel_id) = channel {
        crate::db::calculator::set_channel(&data.db, guild, calc_key, Some(channel_id), None)
            .await?;
        match crate::calculator::plan::post_panel(ctx, data, guild, config).await {
            Ok(_) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::ok(format!("{label} panel posted in <#{channel_id}>.")),
                )
                .await
            }
            Err(err) => {
                finish_modal(
                    ctx,
                    interaction,
                    embeds::error("Couldn't post", format!("`{}`", crate::error::public(&err))),
                )
                .await
            }
        }
    } else {
        crate::db::calculator::set_channel(&data.db, guild, calc_key, None, None).await?;
        finish_modal(
            ctx,
            interaction,
            embeds::ok(format!("{label} panel channel cleared.")),
        )
        .await
    }
}

// ------------------------------------------- building calc modals

async fn building_range_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
    building_key: &str,
) -> anyhow::Result<()> {
    let _ = data;
    let raw_from = trimmed(interaction, "from").unwrap_or_else(|| "0".to_string());
    let raw_to = trimmed(interaction, "to").unwrap_or_else(|| "0".to_string());
    let raw_from = raw_from.split_whitespace().collect::<Vec<_>>().join(" ");
    let raw_to = raw_to.split_whitespace().collect::<Vec<_>>().join(" ");
    let from = crate::calculator::building::parse_level_token(&raw_from, building_key).ok_or_else(
        || {
            anyhow::anyhow!(
                "`{raw_from}` is not a valid level (use `25`, `FC3`, `FC 2-3`, `30-4`, or `0`)"
            )
        },
    )?;
    let to =
        crate::calculator::building::parse_level_token(&raw_to, building_key).ok_or_else(|| {
            anyhow::anyhow!("`{raw_to}` is not a valid level (use `25`, `FC3`, `FC 2-3`, `30-4`)")
        })?;
    if to <= from {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Bad range",
                format!("To level must be higher than from level ({from} → {to})."),
            ),
        )
        .await;
    }
    let user_id = interaction.user.id.get();
    crate::calculator::building::update_session(user_id, |session| {
        let id = session.next_id;
        session.next_id += 1;
        session.entries.push(crate::calculator::building::Entry {
            id,
            building_key: building_key.to_string(),
            from,
            to,
        });
    });

    let session = crate::calculator::building::session_for(user_id);
    let embed = crate::calculator::building::plan_embed(&session);
    let rows = building_plan_rows(&session);
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

fn parse_buff_level(raw: &str) -> i64 {
    raw.trim().parse::<i64>().unwrap_or(0).clamp(0, 100)
}

async fn building_buffs_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let _ = data;
    let construction_speed = parse_buff_level(
        trimmed(interaction, "construction_speed")
            .as_deref()
            .unwrap_or("0"),
    );
    let flat_speed = parse_buff_level(trimmed(interaction, "flat_speed").as_deref().unwrap_or("0"));
    let pet_level =
        parse_buff_level(trimmed(interaction, "pet_level").as_deref().unwrap_or("0")).min(5);
    let zinman_level = parse_buff_level(
        trimmed(interaction, "zinman_level")
            .as_deref()
            .unwrap_or("0"),
    )
    .min(5);
    let expert_level = parse_buff_level(
        trimmed(interaction, "expert_level")
            .as_deref()
            .unwrap_or("0"),
    )
    .min(5);
    let user_id = interaction.user.id.get();

    crate::calculator::building::update_session(user_id, |session| {
        session.buffs = crate::calculator::building::BuildingBuffs {
            construction_speed: construction_speed.min(100),
            flat_speed: match flat_speed {
                10 | 15 | 20 | 30 | 35 => flat_speed,
                _ => 0,
            },
            pet_level,
            zinman_level,
            expert_level,
        };
    });

    let session = crate::calculator::building::session_for(user_id);
    let embed = crate::calculator::building::plan_embed(&session);
    let rows = building_plan_rows(&session);
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

// ---------------------------------------------- svs qty/target modals

async fn svs_qty_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let _ = data;
    let raw = trimmed(interaction, "quantity").unwrap_or_else(|| "0".to_string());
    let qty = crate::calculator::game_numbers::parse_game_amount(&raw);
    let user_id = interaction.user.id.get();

    let session = crate::calculator::svs::session_for(user_id);
    let Some(activity_key) = session.activity_key.clone() else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("No activity", "Pick an activity in the planner first."),
        )
        .await;
    };
    let day_key = session.day_key.clone();
    crate::calculator::svs::update_session(user_id, |session| {
        let plan = session.plans.entry(day_key).or_default();
        if qty <= 0 {
            plan.remove(&activity_key);
        } else {
            plan.insert(activity_key.clone(), qty);
        }
    });

    let session = crate::calculator::svs::session_for(user_id);
    let embed = crate::calculator::svs::planner_embed(&session);
    let rows = svs_rows(&session);
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

async fn svs_target_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let _ = data;
    let raw = trimmed(interaction, "target").unwrap_or_else(|| "0".to_string());
    let target = parse_number_input(&raw).ok_or_else(|| anyhow::anyhow!("enter a number"))?;
    let user_id = interaction.user.id.get();

    crate::calculator::svs::update_session(user_id, |session| {
        session.target = target.max(0);
    });

    let session = crate::calculator::svs::session_for(user_id);
    let embed = crate::calculator::svs::planner_embed(&session);
    let rows = svs_rows(&session);
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

// -------------------------------------------------- pets calculator flow

fn pets_plan_message(
    session: &crate::calculator::pets::PetSession,
) -> (serenity::CreateEmbed, Vec<CreateActionRow>) {
    (
        crate::calculator::pets::plan_embed(session),
        crate::calculator::pets::plan_rows(session),
    )
}

async fn pets_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let _ = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("pets calculator only works inside a server"))?;
    let user_id = interaction.user.id.get();

    match action {
        "open" => {
            crate::calculator::pets::update_session(user_id, |session| {
                *session = crate::calculator::pets::PetSession::default();
            });
            let session = crate::calculator::pets::session_for(user_id);
            let (embed, rows) = pets_plan_message(&session);
            reply(ctx, interaction, embed, rows, true).await
        }

        "select_pet" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a pet picker")),
            };
            let Some(selected) = values.first().cloned() else {
                let session = crate::calculator::pets::session_for(user_id);
                let (embed, rows) = pets_plan_message(&session);
                return update_response(ctx, data, interaction, embed, rows).await;
            };
            crate::calculator::pets::update_session(user_id, |session| {
                session.selected_key = selected;
            });
            let session = crate::calculator::pets::session_for(user_id);
            let (embed, rows) = pets_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "select_from" | "select_to" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a level picker")),
            };
            let Some(selected) = values.first().and_then(|v| v.parse::<i64>().ok()) else {
                let session = crate::calculator::pets::session_for(user_id);
                let (embed, rows) = pets_plan_message(&session);
                return update_response(ctx, data, interaction, embed, rows).await;
            };
            let is_from = action == "select_from";
            crate::calculator::pets::update_session(user_id, |session| {
                let key = session.selected_key.clone();
                let plan = session.plans.entry(key).or_default();
                if is_from {
                    plan.from = selected;
                    if plan.to < plan.from {
                        plan.to = plan.from;
                    }
                } else {
                    plan.to = selected;
                }
            });
            let session = crate::calculator::pets::session_for(user_id);
            let (embed, rows) = pets_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "inventory" => {
            let session = crate::calculator::pets::session_for(user_id);
            let inv = &session.inventory;
            let inputs = vec![
                CreateActionRow::InputText(
                    components::short("food", "Pet Food").value(inv.food.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("manual", "Taming Manuals").value(inv.manual.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("potion", "Energizing Potions").value(inv.potion.to_string()),
                ),
                CreateActionRow::InputText(
                    components::short("serum", "Strengthening Serums").value(inv.serum.to_string()),
                ),
            ];
            interaction
                .create_response(
                    &ctx.http,
                    modal("petcalcmodal:inventory", "Your inventory", inputs),
                )
                .await?;
            Ok(())
        }

        "report" => {
            let session = crate::calculator::pets::session_for(user_id);
            let embed = crate::calculator::pets::report_embed(&session);
            let rows = vec![CreateActionRow::Buttons(vec![
                components::secondary("petcalc:copy", "📋 Copy as text"),
                components::primary("petcalc:back", "↩ Back"),
            ])];
            update_response(ctx, data, interaction, embed, rows).await
        }

        "copy" => {
            let session = crate::calculator::pets::session_for(user_id);
            let text = crate::calculator::pets::report_text(&session);
            if text.trim().is_empty() {
                return reply(
                    ctx,
                    interaction,
                    embeds::info("Nothing to copy", "Plan at least one pet first."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let snippet = format!("```\n{}\n```", crate::error::truncate(&text, 1800));
            reply(
                ctx,
                interaction,
                embeds::info("Report", snippet),
                Vec::new(),
                true,
            )
            .await
        }

        "reset" => {
            crate::calculator::pets::update_session(user_id, |session| {
                *session = crate::calculator::pets::PetSession::default();
            });
            let session = crate::calculator::pets::session_for(user_id);
            let (embed, rows) = pets_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "back" => {
            let session = crate::calculator::pets::session_for(user_id);
            let (embed, rows) = pets_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        other => Err(anyhow::anyhow!("unknown petcalc action `{other}`")),
    }
}

async fn pets_inventory_modal(
    ctx: &serenity::Context,
    _data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let parse = |id: &str| {
        trimmed(interaction, id)
            .and_then(|raw| raw.replace(',', "").trim().parse::<i64>().ok())
            .unwrap_or(0)
            .max(0)
    };
    let inventory = crate::calculator::pets::Inventory {
        food: parse("food"),
        manual: parse("manual"),
        potion: parse("potion"),
        serum: parse("serum"),
    };
    let user_id = interaction.user.id.get();
    crate::calculator::pets::update_session(user_id, |session| session.inventory = inventory);
    // Modals cannot UpdateMessage — reply ephemeral with the refreshed screen.
    let session = crate::calculator::pets::session_for(user_id);
    let (embed, rows) = pets_plan_message(&session);
    interaction
        .create_response(&ctx.http, message(embed, rows, true))
        .await?;
    Ok(())
}

// -------------------------------------------- War Academy calculator flow

fn war_academy_plan_message(
    session: &crate::calculator::war_academy::WaSession,
) -> (serenity::CreateEmbed, Vec<CreateActionRow>) {
    (
        crate::calculator::war_academy::plan_embed(session),
        crate::calculator::war_academy::plan_rows(session),
    )
}

async fn war_academy_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let _ = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("war academy calculator only works inside a server"))?;
    let user_id = interaction.user.id.get();

    let mut parts = action.split(':');
    let kind = parts.next().unwrap_or("");
    let rest = parts.collect::<Vec<_>>().join(":");

    match kind {
        "open" => {
            let tier = if rest == "t12" { "t12" } else { "t11" };
            let session = crate::calculator::war_academy::new_session(user_id, tier);
            let (embed, rows) = war_academy_plan_message(&session);
            reply(ctx, interaction, embed, rows, true).await
        }

        "add" => {
            crate::calculator::war_academy::update_session(user_id, |session| {
                session.draft = Some(crate::calculator::war_academy::WaDraft::default());
            });
            war_academy_troop_step(ctx, interaction, user_id).await
        }

        "troop" => {
            let troop_key = rest.clone();
            crate::calculator::war_academy::update_session(user_id, |session| {
                let draft = session.draft.get_or_insert_with(Default::default);
                draft.troop_key = Some(troop_key);
                draft.skill_name = None;
            });
            war_academy_skill_step(ctx, interaction, user_id).await
        }

        "select_skill" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a skill picker")),
            };
            let Some(skill_name) = values.first().cloned() else {
                return war_academy_skill_step(ctx, interaction, user_id).await;
            };
            crate::calculator::war_academy::update_session(user_id, |session| {
                if let Some(draft) = &mut session.draft {
                    draft.skill_name = Some(skill_name);
                }
            });
            let session = crate::calculator::war_academy::session_for(user_id);
            let troop_key = session
                .draft
                .as_ref()
                .and_then(|d| d.troop_key.clone())
                .unwrap_or_default();
            let skill_name = session
                .draft
                .as_ref()
                .and_then(|d| d.skill_name.clone())
                .unwrap_or_default();
            let Some(skill) =
                crate::calculator::war_academy::skill(&session.tier, &troop_key, &skill_name)
            else {
                return update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn(
                        "Unknown skill",
                        format!("`{skill_name}` is not available for this troop."),
                    ),
                    vec![CreateActionRow::Buttons(vec![components::secondary(
                        "waracademycalc:backtroop",
                        "↩ Back",
                    )])],
                )
                .await;
            };
            // showModal must be the first response — respond directly with the modal.
            let title: String = format!(
                "{} levels",
                crate::calculator::war_academy::troop_label(&troop_key)
            )
            .chars()
            .take(45)
            .collect();
            let modal =
                serenity::CreateModal::new("waracademycalcmodal:levels", title).components(vec![
                    CreateActionRow::InputText(
                        components::short("current", "Current level (0 = not researched)")
                            .value("0")
                            .placeholder(format!("0 - {}", (skill.max_level - 1).max(0))),
                    ),
                    CreateActionRow::InputText(
                        components::short("target", "Target level")
                            .value(skill.max_level.to_string())
                            .placeholder(format!("1 - {}", skill.max_level)),
                    ),
                ]);
            interaction
                .create_response(&ctx.http, serenity::CreateInteractionResponse::Modal(modal))
                .await?;
            Ok(())
        }

        "buffs" => {
            let session = crate::calculator::war_academy::session_for(user_id);
            let value = if session.buffs.research_speed > 0.0 {
                session.buffs.research_speed.to_string()
            } else {
                String::new()
            };
            let modal = serenity::CreateModal::new("waracademycalcmodal:buffs", "Research buffs")
                .components(vec![CreateActionRow::InputText(
                    components::short("speed", "Research speed % (manual)")
                        .required(false)
                        .value(value)
                        .placeholder("e.g. 30"),
                )]);
            interaction
                .create_response(&ctx.http, serenity::CreateInteractionResponse::Modal(modal))
                .await?;
            Ok(())
        }

        "vpselect" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a VP picker")),
            };
            let vp: i64 = values.first().and_then(|v| v.parse().ok()).unwrap_or(0);
            crate::calculator::war_academy::update_session(user_id, |session| {
                session.buffs.vp_bonus = vp;
            });
            let session = crate::calculator::war_academy::session_for(user_id);
            let (embed, rows) = war_academy_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "vpskip" => {
            let session = crate::calculator::war_academy::session_for(user_id);
            let (embed, rows) = war_academy_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "totals" => {
            let session = crate::calculator::war_academy::session_for(user_id);
            let Some(embed) = crate::calculator::war_academy::totals_embed(&session) else {
                return update_response(
                    ctx,
                    data,
                    interaction,
                    embeds::warn("Empty plan", "Add at least one research line first."),
                    vec![CreateActionRow::Buttons(vec![components::secondary(
                        "waracademycalc:backplan",
                        "↩ Back to plan",
                    )])],
                )
                .await;
            };
            update_response(
                ctx,
                data,
                interaction,
                embed,
                vec![CreateActionRow::Buttons(vec![components::secondary(
                    "waracademycalc:backplan",
                    "↩ Back to plan",
                )])],
            )
            .await
        }

        "clear" => {
            crate::calculator::war_academy::update_session(user_id, |session| {
                session.entries.clear();
            });
            let session = crate::calculator::war_academy::session_for(user_id);
            let (embed, rows) = war_academy_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "remove" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an entry picker")),
            };
            let Some(id) = values.first().and_then(|v| v.parse::<u64>().ok()) else {
                let session = crate::calculator::war_academy::session_for(user_id);
                let (embed, rows) = war_academy_plan_message(&session);
                return update_response(ctx, data, interaction, embed, rows).await;
            };
            crate::calculator::war_academy::update_session(user_id, |session| {
                session.entries.retain(|entry| entry.id != id)
            });
            let session = crate::calculator::war_academy::session_for(user_id);
            let (embed, rows) = war_academy_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "backplan" => {
            crate::calculator::war_academy::update_session(user_id, |session| {
                session.draft = None;
            });
            let session = crate::calculator::war_academy::session_for(user_id);
            let (embed, rows) = war_academy_plan_message(&session);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "backtroop" => war_academy_troop_step(ctx, interaction, user_id).await,

        "backskill" => {
            crate::calculator::war_academy::update_session(user_id, |session| {
                if let Some(draft) = &mut session.draft {
                    draft.skill_name = None;
                }
            });
            war_academy_skill_step(ctx, interaction, user_id).await
        }

        other => Err(anyhow::anyhow!("unknown waracademycalc action `{other}`")),
    }
}

async fn war_academy_troop_step(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
) -> anyhow::Result<()> {
    let session = crate::calculator::war_academy::session_for(user_id);
    let tier_label = crate::calculator::war_academy::tier_label(&session.tier);
    let embed = embeds::brand(format!("🏛️ Add research — troop type ({})", tier_label))
        .color(colors::BRAND)
        .description("Pick which troop type to research:")
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let troop_buttons: Vec<serenity::CreateButton> =
        crate::calculator::war_academy::troops(&session.tier)
            .into_iter()
            .map(|troop| {
                components::secondary(
                    format!("waracademycalc:troop:{}", troop.key),
                    format!(
                        "{} {}",
                        crate::calculator::war_academy::troop_emoji(&troop.key),
                        troop.label
                    ),
                )
            })
            .collect();
    let rows = vec![
        CreateActionRow::Buttons(troop_buttons),
        CreateActionRow::Buttons(vec![components::secondary(
            "waracademycalc:backplan",
            format!("{} Back to plan", icons::BACK),
        )]),
    ];
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

async fn war_academy_skill_step(
    ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    user_id: u64,
) -> anyhow::Result<()> {
    use serenity::{CreateSelectMenu, CreateSelectMenuKind};

    let session = crate::calculator::war_academy::session_for(user_id);
    let troop_key = session
        .draft
        .as_ref()
        .and_then(|d| d.troop_key.clone())
        .unwrap_or_else(|| "infantry".to_string());
    let skills: Vec<(String, i64)> = crate::calculator::war_academy::troops(&session.tier)
        .into_iter()
        .find(|troop| troop.key == troop_key)
        .map(|troop| {
            troop
                .skills
                .into_iter()
                .map(|name| {
                    let max =
                        crate::calculator::war_academy::skill(&session.tier, &troop_key, &name)
                            .map(|skill| skill.max_level)
                            .unwrap_or(1);
                    (name, max)
                })
                .collect()
        })
        .unwrap_or_default();
    let options: Vec<serenity::CreateSelectMenuOption> = skills
        .iter()
        .map(|(name, max)| {
            serenity::CreateSelectMenuOption::new(name.clone(), name.clone())
                .description(format!("Lv 1-{max}"))
        })
        .collect();
    let troop_label = crate::calculator::war_academy::troop_label(&troop_key);
    let troop_emoji = crate::calculator::war_academy::troop_emoji(&troop_key);
    let embed = embeds::brand(format!(
        "{} {} — pick a research line",
        troop_emoji, troop_label
    ))
    .color(colors::BRAND)
    .description("Choose the skill to plan:")
    .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let rows = vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                "waracademycalc:select_skill",
                CreateSelectMenuKind::String { options },
            )
            .placeholder("Pick a research line")
            .min_values(1)
            .max_values(1),
        ),
        CreateActionRow::Buttons(vec![components::secondary(
            "waracademycalc:backtroop",
            format!("{} Back", icons::BACK),
        )]),
    ];
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(rows),
            ),
        )
        .await?;
    Ok(())
}

async fn war_academy_levels_modal(
    ctx: &serenity::Context,
    _data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get();
    let session = crate::calculator::war_academy::session_for(user_id);
    let troop_key = session
        .draft
        .as_ref()
        .and_then(|d| d.troop_key.clone())
        .unwrap_or_else(|| "infantry".to_string());
    let skill_name = session
        .draft
        .as_ref()
        .and_then(|d| d.skill_name.clone())
        .unwrap_or_default();
    let Some(skill) = crate::calculator::war_academy::skill(&session.tier, &troop_key, &skill_name)
    else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Expired",
                "Your draft timed out — tap **Add research** again.",
            ),
        )
        .await;
    };
    let from = trimmed(interaction, "current").and_then(|v| v.parse::<i64>().ok());
    let to = trimmed(interaction, "target").and_then(|v| v.parse::<i64>().ok());
    let (Some(from), Some(to)) = (from, to) else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Invalid input", "Enter numeric levels (e.g. 0 and 10)."),
        )
        .await;
    };
    if from < 0 || to > skill.max_level || to <= from {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Invalid range",
                format!(
                    "Current: 0-{} · Target: 1-{} · target must be higher.",
                    skill.max_level - 1,
                    skill.max_level
                ),
            ),
        )
        .await;
    }
    crate::calculator::war_academy::update_session(user_id, |session| {
        if session.draft.is_some() {
            let id = session.next_id;
            session.next_id += 1;
            session
                .entries
                .push(crate::calculator::war_academy::WaEntry {
                    id,
                    troop_key: troop_key.clone(),
                    skill_name: skill_name.clone(),
                    from,
                    to,
                });
            session.draft = None;
        }
    });
    // Modals cannot UpdateMessage — reply ephemeral with the refreshed plan hub.
    let session = crate::calculator::war_academy::session_for(user_id);
    let (embed, rows) = war_academy_plan_message(&session);
    interaction
        .create_response(&ctx.http, message(embed, rows, true))
        .await?;
    Ok(())
}

async fn war_academy_buffs_modal(
    ctx: &serenity::Context,
    _data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let speed = trimmed(interaction, "speed")
        .and_then(|raw| raw.replace(',', "").trim().parse::<f64>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 1000.0);
    let user_id = interaction.user.id.get();
    crate::calculator::war_academy::update_session(user_id, |session| {
        session.buffs.research_speed = speed;
    });

    // VP picker — a component response, so the follow-up can UpdateMessage back to the hub.
    let session = crate::calculator::war_academy::session_for(user_id);
    let embed = embeds::brand("🧙 Research buffs")
        .color(colors::BRAND)
        .description("Choose a VP bonus (optional), or skip to keep the current value.")
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    let vp_select = serenity::CreateSelectMenu::new(
        "waracademycalc:vpselect",
        serenity::CreateSelectMenuKind::String {
            options: vec![
                serenity::CreateSelectMenuOption::new("Off", "0")
                    .default_selection(session.buffs.vp_bonus == 0),
                serenity::CreateSelectMenuOption::new("VP +10%", "10")
                    .default_selection(session.buffs.vp_bonus == 10),
                serenity::CreateSelectMenuOption::new("VP +15%", "15")
                    .default_selection(session.buffs.vp_bonus == 15),
            ],
        },
    )
    .placeholder("VP bonus")
    .min_values(1)
    .max_values(1);
    let rows = vec![
        CreateActionRow::SelectMenu(vp_select),
        CreateActionRow::Buttons(vec![components::secondary(
            "waracademycalc:vpskip",
            "Keep current VP",
        )]),
    ];
    interaction
        .create_response(&ctx.http, message(embed, rows, true))
        .await?;
    Ok(())
}

// ------------------------------------------------------------- ministry

async fn ministry_view(data: &Data, guild_id: u64) -> anyhow::Result<panels::MinistryView> {
    let settings = crate::ministry::get_settings(&data.db, guild_id).await?;
    let form = crate::ministry::get_active_form(&data.db, guild_id)
        .await?
        .or(crate::ministry::get_latest_form(&data.db, guild_id).await?);
    let google = crate::ministry::google::status(&data.cfg, &data.db).await;
    let google_line = match (google.connected, google.email.clone()) {
        (true, Some(email)) => format!("🟢 connected (`{email}`)"),
        (true, None) => "🟢 connected".to_string(),
        (false, _) if google.configured => "⚪ configured — not connected".to_string(),
        (false, _) => "⚪ not configured".to_string(),
    };
    let (total, approved, pending) = crate::ministry::stats(&data.db, guild_id)
        .await
        .unwrap_or((0, 0, 0));
    let form_state = form.as_ref().map(|form| {
        (
            form.label
                .clone()
                .unwrap_or_else(|| format!("SVS #{}", form.svs_number.unwrap_or(1))),
            form.form_url.clone().unwrap_or_default(),
            !matches!(form.status.as_str(), "open" | "active"),
        )
    });
    let spreadsheet_url = form
        .as_ref()
        .and_then(|f| f.workspace_spreadsheet_id.as_deref())
        .map(|id| format!("https://docs.google.com/spreadsheets/d/{id}"));
    Ok(panels::MinistryView {
        svs_number: settings.svs_number.unwrap_or(1),
        applications_open: settings.applications_open(),
        google: google_line,
        channel: settings.admin_channel_id.and_then(|v| v.parse().ok()),
        form: form_state,
        spreadsheet_url,
        total,
        approved,
        pending,
    })
}

/// Route ministry pages: public panel message → fresh ephemeral (the panel
/// never changes); otherwise edit the calling message in place.
async fn ministry_reply(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    embed: CreateEmbed,
    rows: Vec<CreateActionRow>,
) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return reply(ctx, interaction, embed, rows, true).await;
    };
    let is_public_panel = crate::ministry::get_settings(&data.db, guild_id)
        .await
        .ok()
        .and_then(|settings| settings.admin_message_id)
        .map(|id| id == interaction.message.id.get().to_string())
        .unwrap_or(false);
    if is_public_panel {
        reply(ctx, interaction, embed, rows, true).await
    } else {
        update_response(ctx, data, interaction, embed, rows).await
    }
}

async fn ministry_settings_page(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let view = ministry_view(data, guild_id).await?;
    let (embed, rows) = panels::ministry_settings_panel(&view);
    ministry_reply(ctx, data, interaction, embed, rows).await
}

async fn post_admin_panel(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: u64,
    channel_id: u64,
) -> anyhow::Result<u64> {
    let view = ministry_view(data, guild_id).await?;
    let (embed, rows) = panels::ministry_panel(&view);
    let target = serenity::ChannelId::new(channel_id);
    let message = target
        .send_message(
            &ctx.http,
            serenity::CreateMessage::new().embed(embed).components(rows),
        )
        .await?;
    crate::ministry::set_admin_panel(&data.db, guild_id, Some(channel_id), Some(message.id.get()))
        .await?;
    Ok(message.id.get())
}

async fn render_ministry_history(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let forms = crate::ministry::list_forms(&data.db, guild_id, 5).await?;
    let forms_lines: Vec<String> = forms
        .iter()
        .map(|form| {
            let mark = match form.status.as_str() {
                "open" | "active" => "🟢",
                "closed" => "🔒",
                _ => "📦",
            };
            format!(
                "{mark} `#{}` SVS {} · {} · {}",
                form.id,
                form.svs_number
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                form.status,
                form.label
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect::<String>()
            )
        })
        .collect();
    let subs = crate::ministry::list_submissions(&data.db, guild_id, None, 5).await?;
    let recent: Vec<String> = subs.iter().map(|s| s.line()).collect();
    let pending =
        crate::ministry::list_submissions(&data.db, guild_id, Some("pending"), 25).await?;
    let pending_options: Vec<serenity::CreateSelectMenuOption> = pending
        .iter()
        .map(|s| {
            serenity::CreateSelectMenuOption::new(
                format!(
                    "#{} · {} {}",
                    s.id,
                    s.applicant_name
                        .as_deref()
                        .unwrap_or("?")
                        .chars()
                        .take(40)
                        .collect::<String>(),
                    s.events(),
                )
                .chars()
                .take(100)
                .collect::<String>(),
                s.id.to_string(),
            )
            .description(format!("FID {}", s.fid))
        })
        .collect();
    let view = ministry_view(data, guild_id).await?;
    let (embed, rows) =
        panels::ministry_history_panel(&view, &forms_lines, &recent, &pending_options);
    ministry_reply(ctx, data, interaction, embed, rows).await
}

async fn ministry_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("ministry only works inside a server"))?;
    if !permissions::component_is_admin(ctx, data, interaction).await {
        return reply(
            ctx,
            interaction,
            embeds::denied("Manage Server"),
            Vec::new(),
            true,
        )
        .await;
    }

    let mut parts = action.split(':');
    let kind = parts.next().unwrap_or("");
    let arg = parts.collect::<Vec<_>>().join(":");

    match kind {
        "home" => {
            let view = ministry_view(data, guild_id).await?;
            let (embed, rows) = panels::ministry_panel(&view);
            ministry_reply(ctx, data, interaction, embed, rows).await
        }
        "settings" => ministry_settings_page(ctx, data, interaction, guild_id).await,
        "svs" => {
            let view = ministry_view(data, guild_id).await?;
            let (embed, rows) = panels::ministry_svs_panel(&view);
            ministry_reply(ctx, data, interaction, embed, rows).await
        }
        "google" => {
            let view = ministry_view(data, guild_id).await?;
            let auth_url = crate::ministry::google::auth_url(&data.cfg).ok();
            let (embed, rows) = panels::ministry_google_panel(&view, auth_url);
            ministry_reply(ctx, data, interaction, embed, rows).await
        }
        "pastecode" => {
            // The Google token is global (shared by every server), so only the bot owner may replace it.
            if data.cfg.owner_id != Some(interaction.user.id.get()) {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let inputs = vec![CreateActionRow::InputText(components::paragraph(
                "code",
                "Paste the authorization code (or the whole redirect URL)",
            ))];
            interaction
                .create_response(
                    &ctx.http,
                    modal("ministrymodal:googlecode", "Google authorization", inputs),
                )
                .await?;
            Ok(())
        }
        "disconnect" => {
            if data.cfg.owner_id != Some(interaction.user.id.get()) {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            crate::ministry::google::disconnect(&data.db).await?;
            ministry_settings_page(ctx, data, interaction, guild_id).await
        }
        "channel" => {
            let inputs = vec![CreateActionRow::InputText(components::short(
                "channel",
                "Channel id (or `none` to clear)",
            ))];
            send_modal(
                ctx,
                interaction,
                "ministrymodal:channel",
                "Admin Panel Channel",
                inputs,
            )
            .await
        }
        "post_here" => {
            let channel_id = interaction.channel_id.get();
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;
            match post_admin_panel(ctx, data, guild_id, channel_id).await {
                Ok(message_id) => interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(format!(
                            "Ministry admin panel posted in <#{channel_id}> (`{message_id}`)."
                        )),
                    )
                    .await
                    .map(|_| ())
                    .map_err(Into::into),
                Err(err) => interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(format!(
                            "Couldn't post the panel: `{}`",
                            crate::error::public(&err)
                        )),
                    )
                    .await
                    .map(|_| ())
                    .map_err(Into::into),
            }
        }
        "repost" => {
            let settings = crate::ministry::get_settings(&data.db, guild_id).await?;
            match settings
                .admin_channel_id
                .and_then(|v| v.parse::<u64>().ok())
            {
                Some(channel_id) => {
                    interaction
                        .create_response(
                            &ctx.http,
                            CreateInteractionResponse::Defer(
                                CreateInteractionResponseMessage::new().ephemeral(true),
                            ),
                        )
                        .await?;
                    match post_admin_panel(ctx, data, guild_id, channel_id).await {
                        Ok(message_id) => interaction
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content(format!(
                                    "Ministry admin panel re-posted in <#{channel_id}> (`{message_id}`)."
                                )),
                            )
                            .await
                            .map(|_| ()).map_err(Into::into),
                        Err(err) => interaction
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content(format!("Couldn't repost: `{}`", crate::error::public(&err))),
                            )
                            .await
                            .map(|_| ()).map_err(Into::into),
                    }
                }
                None => {
                    reply(
                        ctx,
                        interaction,
                        embeds::warn("Not configured", "Set an Admin Panel channel first."),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }
        "svsset" => {
            let settings = crate::ministry::get_settings(&data.db, guild_id).await?;
            let inputs = vec![CreateActionRow::InputText(
                components::short("svs", "SVS number")
                    .value(settings.svs_number.unwrap_or(1).to_string()),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal("ministrymodal:svs", "Set SVS number", inputs),
                )
                .await?;
            Ok(())
        }
        "toggleapps" => {
            let settings = crate::ministry::get_settings(&data.db, guild_id).await?;
            crate::ministry::set_applications_open(
                &data.db,
                guild_id,
                !settings.applications_open(),
            )
            .await?;
            ministry_settings_page(ctx, data, interaction, guild_id).await
        }
        "create" => {
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;
            match crate::ministry::create_unified_form(&data.cfg, &data.db, guild_id).await {
                Ok((form, sheet_url)) => {
                    let view = ministry_view(data, guild_id).await?;
                    let embed = embeds::success(
                        "SVS form created",
                        format!(
                            "**{}**\nForm: <{}>\nWorkspace: <{}>",
                            form.label.as_deref().unwrap_or("SVS form"),
                            form.form_url.as_deref().unwrap_or("?"),
                            sheet_url,
                        ),
                    );
                    let _ = view;
                    interaction
                        .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                        .await?;
                    Ok(())
                }
                Err(err) => interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(format!(
                            "Couldn't create the form: `{}`",
                            crate::error::public(&err)
                        )),
                    )
                    .await
                    .map(|_| ())
                    .map_err(Into::into),
            }
        }
        "close" => match crate::ministry::close_form(&data.db, guild_id).await? {
            Some(_) => {
                let view = ministry_view(data, guild_id).await?;
                let (embed, rows) = panels::ministry_svs_panel(&view);
                ministry_reply(ctx, data, interaction, embed, rows).await
            }
            None => {
                reply(
                    ctx,
                    interaction,
                    embeds::warn("No open form", "There is no open SVS form to close."),
                    Vec::new(),
                    true,
                )
                .await
            }
        },
        "share" => {
            let form = crate::ministry::get_active_form(&data.db, guild_id)
                .await?
                .or(crate::ministry::get_latest_form(&data.db, guild_id).await?);
            let Some(form) = form else {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("No form", "Create the SVS form first."),
                    Vec::new(),
                    true,
                )
                .await;
            };
            let spreadsheet = form.workspace_spreadsheet_id.as_deref().unwrap_or("");
            if spreadsheet.is_empty() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("No workspace", "This form has no workspace spreadsheet."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;
            match crate::ministry::share_form_sheet(&data.cfg, &data.db, spreadsheet).await {
                Ok(()) => interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(
                            "Workspace spreadsheet is now shared with anyone who has the link (edit)."
                        ),
                    )
                    .await
                    .map(|_| ()).map_err(Into::into),
                Err(err) => interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(format!("Couldn't share: `{}`", crate::error::public(&err))),
                    )
                    .await
                    .map(|_| ()).map_err(Into::into),
            }
        }
        "copy" => {
            let form = crate::ministry::get_active_form(&data.db, guild_id)
                .await?
                .or(crate::ministry::get_latest_form(&data.db, guild_id).await?);
            let Some(form) = form else {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("No form", "Create the SVS form first."),
                    Vec::new(),
                    true,
                )
                .await;
            };
            let form_url = form.form_url.as_deref().unwrap_or("?");
            let sheet = form.workspace_spreadsheet_id.as_deref().unwrap_or("?");
            let content = format!(
                "SVS form: <{form_url}>\nReservation workspace: <https://docs.google.com/spreadsheets/d/{sheet}>"
            );
            reply(
                ctx,
                interaction,
                embeds::info("Ministry links", content),
                Vec::new(),
                true,
            )
            .await
        }
        "export" | "export_xlsx" | "export_png" => {
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;
            let deliver = |err: anyhow::Error| -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                let content = format!("Export failed: `{}`", crate::error::public(&err));
                let http = ctx.http.clone();
                Box::pin(async move {
                    let _ = interaction
                        .edit_response(
                            &http,
                            EditInteractionResponse::new().content(content),
                        )
                        .await;
                })
            };
            match action {
                "export" => match crate::ministry::export_csv(&data.db, guild_id).await {
                    Ok(csv) => {
                        interaction
                            .create_followup(
                                &ctx.http,
                                serenity::CreateInteractionResponseFollowup::new()
                                    .content("Ministry submissions (CSV):")
                                    .ephemeral(true)
                                    .add_file(serenity::CreateAttachment::bytes(
                                        csv.into_bytes(),
                                        format!("ministry_svs_{guild_id}.csv"),
                                    )),
                            )
                            .await?;
                        Ok(())
                    }
                    Err(err) => {
                        deliver(err).await;
                        Ok(())
                    }
                },
                "export_xlsx" => match crate::ministry::export_xlsx(&data.db, guild_id).await {
                    Ok(bytes) => {
                        interaction
                            .create_followup(
                                &ctx.http,
                                serenity::CreateInteractionResponseFollowup::new()
                                    .content("Ministry submissions (Excel):")
                                    .ephemeral(true)
                                    .add_file(serenity::CreateAttachment::bytes(
                                        bytes,
                                        format!("ministry_svs_{guild_id}.xlsx"),
                                    )),
                            )
                            .await?;
                        Ok(())
                    }
                    Err(err) => {
                        deliver(err).await;
                        Ok(())
                    }
                },
                _ => {
                    // export_png — cap at 16 rows so the card stays shareable
                    match crate::ministry::export_png(&data.db, guild_id, 16).await {
                        Ok(bytes) => {
                            interaction
                                .create_followup(
                                    &ctx.http,
                                    serenity::CreateInteractionResponseFollowup::new()
                                        .content("Ministry submissions (roster image):")
                                        .ephemeral(true)
                                        .add_file(serenity::CreateAttachment::bytes(
                                            bytes,
                                            format!("ministry_svs_{guild_id}.png"),
                                        )),
                                )
                                .await?;
                            Ok(())
                        }
                        Err(err) => {
                            deliver(err).await;
                            Ok(())
                        }
                    }
                }
            }
        }
        "history" => render_ministry_history(ctx, data, interaction, guild_id).await,
        "review" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a submission picker")),
            };
            let Some(id) = values.first().and_then(|v| v.parse::<i64>().ok()) else {
                return render_ministry_history(ctx, data, interaction, guild_id).await;
            };
            let subs = crate::ministry::list_submissions(&data.db, guild_id, None, 100).await?;
            let Some(sub) = subs.into_iter().find(|s| s.id == id) else {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Not found", format!("Submission #{id} no longer exists.")),
                    Vec::new(),
                    true,
                )
                .await;
            };
            let view = ministry_view(data, guild_id).await?;
            let (embed, rows) = panels::ministry_review_panel(&view, &sub);
            ministry_reply(ctx, data, interaction, embed, rows).await
        }
        "approve" | "reject" => {
            let Some(id) = arg.parse::<i64>().ok() else {
                return Err(anyhow::anyhow!("missing submission id"));
            };
            let approve = kind == "approve";
            let changed = crate::ministry::review_submission(
                &data.db,
                guild_id,
                id,
                approve,
                interaction.user.id.get(),
            )
            .await?;
            if !changed {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Not found", format!("Submission #{id} no longer exists.")),
                    Vec::new(),
                    true,
                )
                .await;
            }
            render_ministry_history(ctx, data, interaction, guild_id).await
        }
        "refresh" => {
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;
            match crate::ministry::sync_form(&data.cfg, &data.db, guild_id).await {
                Ok((added, scanned)) => {
                    let view = ministry_view(data, guild_id).await?;
                    let embed = embeds::success(
                        "Sync complete",
                        format!(
                            "Scanned **{scanned}** response(s) — **{added}** new.\n**Submissions:** `{}` total · `{}` approved · `{}` pending",
                            view.total, view.approved, view.pending
                        ),
                    );
                    interaction
                        .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
                        .await?;
                    Ok(())
                }
                Err(err) => interaction
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(format!("Sync failed: `{}`", crate::error::public(&err))),
                    )
                    .await
                    .map(|_| ())
                    .map_err(Into::into),
            }
        }
        other => Err(anyhow::anyhow!("unknown ministry action `{other}`")),
    }
}

async fn ministry_googlecode_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    // Global credential: bot owner only.
    if data.cfg.owner_id != Some(interaction.user.id.get()) {
        return finish_modal(ctx, interaction, embeds::denied("Bot Owner")).await;
    }
    let code = trimmed(interaction, "code")
        .ok_or_else(|| anyhow::anyhow!("paste the authorization code"))?;
    defer_modal(ctx, interaction).await?;

    match crate::ministry::google::exchange_code(&data.cfg, &data.db, &code).await {
        Ok(()) => {
            finish_modal(
                ctx,
                interaction,
                embeds::success(
                    "Google connected",
                    "Ministry can now create forms and write to sheets.",
                ),
            )
            .await
        }
        Err(err) => {
            finish_modal(
                ctx,
                interaction,
                embeds::error(
                    "Couldn't connect",
                    format!("`{}`", crate::error::public(&err)),
                ),
            )
            .await
        }
    }
}

async fn ministry_svs_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let svs: i64 = trimmed(interaction, "svs")
        .and_then(|v| v.parse().ok())
        .map(|v: i64| v.max(1))
        .ok_or_else(|| anyhow::anyhow!("svs must be a positive number"))?;
    defer_modal(ctx, interaction).await?;

    crate::ministry::set_svs_number(&data.db, guild, svs).await?;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!("SVS number set to **#{svs}**.")),
    )
    .await
}

async fn ministry_channel_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let channel = channel_from_interaction(interaction, "channel")?;
    defer_modal(ctx, interaction).await?;

    match channel {
        Some(channel_id) => {
            crate::ministry::set_admin_panel(&data.db, guild, Some(channel_id), None).await?;
            match post_admin_panel(ctx, data, guild, channel_id).await {
                Ok(message_id) => {
                    finish_modal(
                        ctx,
                        interaction,
                        embeds::ok(format!(
                            "Ministry panel posted in <#{channel_id}> (`{message_id}`)."
                        )),
                    )
                    .await
                }
                Err(err) => {
                    finish_modal(
                        ctx,
                        interaction,
                        embeds::error("Couldn't post", format!("`{}`", crate::error::public(&err))),
                    )
                    .await
                }
            }
        }
        None => {
            crate::ministry::set_admin_panel(&data.db, guild, None, None).await?;
            finish_modal(
                ctx,
                interaction,
                embeds::ok("Ministry panel channel cleared."),
            )
            .await
        }
    }
}

// -------------------------------------------------- tempvoice interface

fn tvc_everyone_overwrite(
    channel: &serenity::GuildChannel,
) -> Option<serenity::PermissionOverwrite> {
    channel
        .permission_overwrites
        .iter()
        .find(|overwrite| {
            overwrite.kind
                == serenity::PermissionOverwriteType::Role(serenity::RoleId::new(
                    channel.guild_id.get(),
                ))
        })
        .cloned()
}

fn tvc_is_locked(channel: &serenity::GuildChannel) -> bool {
    tvc_everyone_overwrite(channel)
        .map(|overwrite| overwrite.deny.contains(serenity::Permissions::CONNECT))
        .unwrap_or(false)
}

async fn tvc_fetch_channel(
    ctx: &serenity::Context,
    channel_id: u64,
) -> anyhow::Result<serenity::GuildChannel> {
    match ctx
        .http
        .get_channel(serenity::ChannelId::new(channel_id))
        .await?
    {
        serenity::Channel::Guild(channel) => Ok(channel),
        _ => Err(anyhow::anyhow!("not a guild channel")),
    }
}

fn tvc_is_owner(
    interaction: &ComponentInteraction,
    row: &crate::db::tempvoice::TempVoiceChannel,
) -> bool {
    row.owner_id() == Some(interaction.user.id.get())
}

async fn tvc_require_owner(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    row: &crate::db::tempvoice::TempVoiceChannel,
) -> anyhow::Result<bool> {
    if tvc_is_owner(interaction, row) {
        return Ok(true);
    }
    if permissions::component_is_admin(ctx, data, interaction).await {
        return Ok(true);
    }
    reply(
        ctx,
        interaction,
        embeds::denied("the channel owner"),
        Vec::new(),
        true,
    )
    .await
    .map(|_| false)
}

fn tvc_row_label(row: &crate::db::tempvoice::TempVoiceChannel) -> String {
    format!("<#{}>", row.channel_id)
}

async fn tempvoice_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return reply(
            ctx,
            interaction,
            embeds::warn("Server only", "Temp voice works inside a server."),
            Vec::new(),
            true,
        )
        .await;
    };

    let mut parts = action.split(':');
    let kind = parts.next().unwrap_or("");
    let arg = parts.collect::<Vec<_>>().join(":");

    let channel_id = interaction.channel_id.get();
    let Some(row) = crate::db::tempvoice::get_channel(&data.db, channel_id).await? else {
        return reply(
            ctx,
            interaction,
            embeds::warn("Not temporary", "This isn't a temporary voice channel."),
            Vec::new(),
            true,
        )
        .await;
    };
    let owner_id = row.owner_id().unwrap_or(0);

    let mut channel = match tvc_fetch_channel(ctx, channel_id).await {
        Ok(channel) => channel,
        Err(err) => {
            return reply(
                ctx,
                interaction,
                embeds::error(
                    "Can't reach channel",
                    format!("`{}`", crate::error::public(&err)),
                ),
                Vec::new(),
                true,
            )
            .await
        }
    };
    let everyone = serenity::RoleId::new(guild_id);
    let user_id = interaction.user.id.get();

    // ── Claim (allowed for anyone inside the channel, even if not owner) ──
    if kind == "claim" {
        let in_channel = ctx
            .cache
            .guild(serenity::GuildId::new(guild_id))
            .map(|guild| {
                guild
                    .voice_states
                    .values()
                    .any(|state| state.channel_id.map(|c| c.get()) == Some(channel_id))
            })
            .unwrap_or(false);
        let in_voice = ctx
            .cache
            .guild(serenity::GuildId::new(guild_id))
            .and_then(|guild| {
                guild
                    .voice_states
                    .get(&serenity::UserId::new(user_id))
                    .map(|state| state.channel_id.map(|c| c.get()) == Some(channel_id))
            })
            .unwrap_or(false);
        let _ = in_channel;
        if !in_voice {
            return reply(
                ctx,
                interaction,
                embeds::warn("Join first", "Join the voice channel before claiming it."),
                Vec::new(),
                true,
            )
            .await;
        }
        let owner_still_inside = ctx
            .cache
            .guild(serenity::GuildId::new(guild_id))
            .map(|guild| {
                guild.voice_states.values().any(|state| {
                    state.user_id.get() == owner_id
                        && state.channel_id.map(|c| c.get()) == Some(channel_id)
                })
            })
            .unwrap_or(false);
        if owner_still_inside {
            return reply(
                ctx,
                interaction,
                embeds::warn("Owner is here", "The owner is still in the channel."),
                Vec::new(),
                true,
            )
            .await;
        }
        crate::db::tempvoice::set_owner(&data.db, channel_id, user_id).await?;
        let _ = channel
            .create_permission(
                &ctx.http,
                serenity::PermissionOverwrite {
                    allow: crate::tempvoice::OWNER_ALLOW,
                    deny: serenity::Permissions::empty(),
                    kind: serenity::PermissionOverwriteType::Member(serenity::UserId::new(user_id)),
                },
            )
            .await;
        let _ = channel
            .delete_permission(
                &ctx.http,
                serenity::PermissionOverwriteType::Member(serenity::UserId::new(owner_id)),
            )
            .await;
        let (embed, rows) = crate::tempvoice::build_control_panel(user_id, tvc_is_locked(&channel));
        return update_response(ctx, data, interaction, embed, rows).await;
    }

    // All other actions require owner (or admin).
    if !tvc_require_owner(ctx, data, interaction, &row).await? {
        return Ok(());
    }

    match kind {
        "name" | "rename" => {
            let inputs = vec![CreateActionRow::InputText(
                components::short("name", "New channel name")
                    .value(channel.name.chars().take(100).collect::<String>()),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal("tvcmodal:rename", "Rename Channel", inputs),
                )
                .await?;
            Ok(())
        }

        "limit" => {
            let inputs = vec![CreateActionRow::InputText(
                components::short("limit", "Max users (0 = unlimited, up to 99)")
                    .value(channel.user_limit.unwrap_or(0).to_string())
                    .max_length(2),
            )];
            interaction
                .create_response(&ctx.http, modal("tvcmodal:limit", "User Limit", inputs))
                .await?;
            Ok(())
        }

        "privacy" | "lock" => {
            let locked = tvc_is_locked(&channel);
            if locked {
                let _ = channel
                    .delete_permission(&ctx.http, serenity::PermissionOverwriteType::Role(everyone))
                    .await;
            } else {
                let _ = channel
                    .create_permission(
                        &ctx.http,
                        serenity::PermissionOverwrite {
                            allow: serenity::Permissions::empty(),
                            deny: serenity::Permissions::CONNECT,
                            kind: serenity::PermissionOverwriteType::Role(everyone),
                        },
                    )
                    .await;
            }
            let (embed, rows) = crate::tempvoice::build_control_panel(owner_id, !locked);
            update_response(ctx, data, interaction, embed, rows).await
        }

        "waiting" => {
            if let Some(waiting_id) = row.waiting_channel_id() {
                let _ = ctx
                    .http
                    .delete_channel(
                        serenity::ChannelId::new(waiting_id),
                        Some("Waiting room closed"),
                    )
                    .await;
                crate::db::tempvoice::set_waiting_channel(&data.db, channel_id, None).await?;
                return reply(
                    ctx,
                    interaction,
                    embeds::ok("Waiting room **closed**."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let waiting = {
                let mut builder = serenity::CreateChannel::new(
                    format!("🚪 {}", channel.name)
                        .chars()
                        .take(100)
                        .collect::<String>(),
                )
                .kind(serenity::ChannelType::Voice)
                .permissions(vec![
                    serenity::PermissionOverwrite {
                        allow: serenity::Permissions::CONNECT
                            .union(serenity::Permissions::VIEW_CHANNEL),
                        deny: serenity::Permissions::empty(),
                        kind: serenity::PermissionOverwriteType::Role(everyone),
                    },
                    serenity::PermissionOverwrite {
                        allow: crate::tempvoice::OWNER_ALLOW,
                        deny: serenity::Permissions::empty(),
                        kind: serenity::PermissionOverwriteType::Member(serenity::UserId::new(
                            owner_id,
                        )),
                    },
                ])
                .audit_log_reason("Temp voice waiting room");
                if let Some(parent) = channel.parent_id {
                    builder = builder.category(parent);
                }
                serenity::GuildId::new(guild_id)
                    .create_channel(&ctx.http, builder)
                    .await
            };
            match waiting {
                Ok(waiting) => {
                    crate::db::tempvoice::set_waiting_channel(
                        &data.db,
                        channel_id,
                        Some(waiting.id.get()),
                    )
                    .await?;
                    if !tvc_is_locked(&channel) {
                        let _ = channel
                            .create_permission(
                                &ctx.http,
                                serenity::PermissionOverwrite {
                                    allow: serenity::Permissions::empty(),
                                    deny: serenity::Permissions::CONNECT,
                                    kind: serenity::PermissionOverwriteType::Role(everyone),
                                },
                            )
                            .await;
                    }
                    let (embed, rows) = crate::tempvoice::build_control_panel(owner_id, true);
                    update_response(ctx, data, interaction, embed, rows).await
                }
                Err(err) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::error(
                            "Couldn't create waiting room",
                            format!("`{}`", crate::error::public(&err)),
                        ),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }

        "chat" => {
            if let Some(chat_id) = row.chat_channel_id() {
                let _ = ctx
                    .http
                    .delete_channel(serenity::ChannelId::new(chat_id), Some("Temp chat closed"))
                    .await;
                crate::db::tempvoice::set_chat_channel(&data.db, channel_id, None).await?;
                return reply(
                    ctx,
                    interaction,
                    embeds::ok("Text chat **closed**."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let slug: String = channel
                .name
                .to_lowercase()
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            let chat_name = format!("💬-{slug}").chars().take(100).collect::<String>();
            let chat = {
                let mut builder = serenity::CreateChannel::new(chat_name)
                    .kind(serenity::ChannelType::Text)
                    .permissions(vec![
                        serenity::PermissionOverwrite {
                            allow: serenity::Permissions::empty(),
                            deny: serenity::Permissions::VIEW_CHANNEL,
                            kind: serenity::PermissionOverwriteType::Role(everyone),
                        },
                        serenity::PermissionOverwrite {
                            allow: serenity::Permissions::VIEW_CHANNEL
                                .union(serenity::Permissions::SEND_MESSAGES)
                                .union(serenity::Permissions::READ_MESSAGE_HISTORY)
                                .union(serenity::Permissions::MANAGE_MESSAGES),
                            deny: serenity::Permissions::empty(),
                            kind: serenity::PermissionOverwriteType::Member(serenity::UserId::new(
                                owner_id,
                            )),
                        },
                    ])
                    .audit_log_reason("Temp voice chat");
                if let Some(parent) = channel.parent_id {
                    builder = builder.category(parent);
                }
                serenity::GuildId::new(guild_id)
                    .create_channel(&ctx.http, builder)
                    .await
            };
            match chat {
                Ok(chat) => {
                    // Grant view to everyone currently in the voice channel.
                    // Collect ids first — the cache guard isn't Send across awaits.
                    let members: Vec<u64> = ctx
                        .cache
                        .guild(serenity::GuildId::new(guild_id))
                        .map(|guild| {
                            guild
                                .voice_states
                                .values()
                                .filter(|state| {
                                    state.channel_id.map(|c| c.get()) == Some(channel_id)
                                })
                                .map(|state| state.user_id.get())
                                .collect()
                        })
                        .unwrap_or_default();
                    {
                        for member_id in members {
                            if member_id == owner_id {
                                continue;
                            }
                            let _ = chat
                                .create_permission(
                                    &ctx.http,
                                    serenity::PermissionOverwrite {
                                        allow: serenity::Permissions::VIEW_CHANNEL
                                            .union(serenity::Permissions::SEND_MESSAGES)
                                            .union(serenity::Permissions::READ_MESSAGE_HISTORY),
                                        deny: serenity::Permissions::empty(),
                                        kind: serenity::PermissionOverwriteType::Member(
                                            serenity::UserId::new(member_id),
                                        ),
                                    },
                                )
                                .await;
                        }
                    }
                    crate::db::tempvoice::set_chat_channel(
                        &data.db,
                        channel_id,
                        Some(chat.id.get()),
                    )
                    .await?;
                    reply(
                        ctx,
                        interaction,
                        embeds::ok(format!("Text chat created: <#{}>", chat.id.get())),
                        Vec::new(),
                        true,
                    )
                    .await
                }
                Err(err) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::error(
                            "Couldn't create chat",
                            format!("`{}`", crate::error::public(&err)),
                        ),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }

        "invite" => {
            match serenity::ChannelId::new(channel_id)
                .create_invite(
                    &ctx.http,
                    serenity::CreateInvite::new()
                        .max_age(3600)
                        .max_uses(5)
                        .unique(true),
                )
                .await
            {
                Ok(invitation) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::ok(format!(
                            "🔗 Invite (1h, 5 uses): https://discord.gg/{}",
                            invitation.code
                        )),
                        Vec::new(),
                        true,
                    )
                    .await
                }
                Err(err) => {
                    reply(
                        ctx,
                        interaction,
                        embeds::error(
                            "Couldn't invite",
                            format!("`{}`", crate::error::public(&err)),
                        ),
                        Vec::new(),
                        true,
                    )
                    .await
                }
            }
        }

        "region" => {
            if arg == "pick" {
                let values = match &interaction.data.kind {
                    serenity::ComponentInteractionDataKind::StringSelect { values } => {
                        values.clone()
                    }
                    _ => return Err(anyhow::anyhow!("expected a region picker")),
                };
                let Some(value) = values.first().cloned() else {
                    return reply(
                        ctx,
                        interaction,
                        embeds::warn("Pick a region", "Choose one first."),
                        Vec::new(),
                        true,
                    )
                    .await;
                };
                let region: Option<String> = if value == "null" {
                    None
                } else {
                    Some(value.clone())
                };
                let label = crate::tempvoice::REGIONS
                    .iter()
                    .find(|(_, v)| *v == value)
                    .map(|(label, _)| *label)
                    .unwrap_or(value.as_str())
                    .to_string();
                let edited = serenity::EditChannel::new().voice_region(region);
                match channel.edit(&ctx.http, edited).await {
                    Ok(_) => {
                        reply(
                            ctx,
                            interaction,
                            embeds::ok(format!("🌍 Region set to **{label}**.")),
                            Vec::new(),
                            true,
                        )
                        .await
                    }
                    Err(err) => {
                        reply(
                            ctx,
                            interaction,
                            embeds::error(
                                "Couldn't set region",
                                format!("`{}`", crate::error::public(&err)),
                            ),
                            Vec::new(),
                            true,
                        )
                        .await
                    }
                }
            } else {
                let options: Vec<serenity::CreateSelectMenuOption> = crate::tempvoice::REGIONS
                    .iter()
                    .map(|(label, value)| serenity::CreateSelectMenuOption::new(*label, *value))
                    .collect();
                let select = serenity::CreateSelectMenu::new(
                    "tvc:region:pick",
                    serenity::CreateSelectMenuKind::String { options },
                )
                .placeholder("Choose a voice region…")
                .min_values(1)
                .max_values(1);
                let embed = embeds::brand("🌍 Voice region")
                    .description("Pick a voice region for this channel:");
                reply(
                    ctx,
                    interaction,
                    embed,
                    vec![CreateActionRow::SelectMenu(select)],
                    true,
                )
                .await
            }
        }

        "delete" => {
            reply(
                ctx,
                interaction,
                embeds::ok("Deleting this temporary channel…"),
                Vec::new(),
                true,
            )
            .await?;
            crate::tempvoice::delete_temp(
                ctx,
                data,
                channel_id,
                &format!("Deleted by owner <@{user_id}>"),
            )
            .await;
            Ok(())
        }

        "trust" | "untrust" | "kick" | "block" | "unblock" | "transfer" => {
            if arg == "pick" {
                // User-select follow-through from the picker messages below.
                let values = match &interaction.data.kind {
                    serenity::ComponentInteractionDataKind::UserSelect { values } => values.clone(),
                    _ => return Err(anyhow::anyhow!("expected a member picker")),
                };
                let Some(target) = values.first() else {
                    return reply(
                        ctx,
                        interaction,
                        embeds::warn("Pick a member", "Choose someone first."),
                        Vec::new(),
                        true,
                    )
                    .await;
                };
                let target_id = target.get();
                match kind {
                    "trust" => {
                        let _ = channel
                            .create_permission(
                                &ctx.http,
                                serenity::PermissionOverwrite {
                                    allow: serenity::Permissions::CONNECT
                                        .union(serenity::Permissions::VIEW_CHANNEL),
                                    deny: serenity::Permissions::empty(),
                                    kind: serenity::PermissionOverwriteType::Member(*target),
                                },
                            )
                            .await;
                        if let Some(chat_id) = row.chat_channel_id() {
                            if let Ok(chat) = tvc_fetch_channel(ctx, chat_id).await {
                                let _ = chat
                                    .create_permission(
                                        &ctx.http,
                                        serenity::PermissionOverwrite {
                                            allow: serenity::Permissions::VIEW_CHANNEL
                                                .union(serenity::Permissions::SEND_MESSAGES)
                                                .union(serenity::Permissions::READ_MESSAGE_HISTORY),
                                            deny: serenity::Permissions::empty(),
                                            kind: serenity::PermissionOverwriteType::Member(
                                                *target,
                                            ),
                                        },
                                    )
                                    .await;
                            }
                        }
                        return reply(
                            ctx,
                            interaction,
                            embeds::ok(format!(
                                "✅ Trusted <@{target_id}> — they can join even when locked."
                            )),
                            Vec::new(),
                            true,
                        )
                        .await;
                    }
                    "untrust" => {
                        let _ = channel
                            .delete_permission(
                                &ctx.http,
                                serenity::PermissionOverwriteType::Member(*target),
                            )
                            .await;
                        return reply(
                            ctx,
                            interaction,
                            embeds::ok(format!("⛔ Untrusted <@{target_id}>.")),
                            Vec::new(),
                            true,
                        )
                        .await;
                    }
                    "kick" => {
                        if target_id == user_id {
                            return reply(
                                ctx,
                                interaction,
                                embeds::warn("Not yourself", "You can't do that to yourself."),
                                Vec::new(),
                                true,
                            )
                            .await;
                        }
                        let member = ctx
                            .http
                            .get_member(serenity::GuildId::new(guild_id), *target)
                            .await;
                        match member {
                            Ok(member) => {
                                let _ = member.disconnect_from_voice(&ctx.http).await;
                                return reply(
                                    ctx,
                                    interaction,
                                    embeds::ok(format!("👢 Kicked <@{target_id}>.")),
                                    Vec::new(),
                                    true,
                                )
                                .await;
                            }
                            Err(_) => {
                                return reply(
                                    ctx,
                                    interaction,
                                    embeds::warn(
                                        "Not in channel",
                                        "That member is not in this voice channel.",
                                    ),
                                    Vec::new(),
                                    true,
                                )
                                .await;
                            }
                        }
                    }
                    "block" => {
                        if target_id == user_id {
                            return reply(
                                ctx,
                                interaction,
                                embeds::warn("Not yourself", "You can't do that to yourself."),
                                Vec::new(),
                                true,
                            )
                            .await;
                        }
                        let _ = channel
                            .create_permission(
                                &ctx.http,
                                serenity::PermissionOverwrite {
                                    allow: serenity::Permissions::empty(),
                                    deny: serenity::Permissions::CONNECT
                                        .union(serenity::Permissions::VIEW_CHANNEL),
                                    kind: serenity::PermissionOverwriteType::Member(*target),
                                },
                            )
                            .await;
                        if let Ok(member) = ctx
                            .http
                            .get_member(serenity::GuildId::new(guild_id), *target)
                            .await
                        {
                            let _ = member.disconnect_from_voice(&ctx.http).await;
                        }
                        return reply(
                            ctx,
                            interaction,
                            embeds::ok(format!("🚫 Blocked <@{target_id}>.")),
                            Vec::new(),
                            true,
                        )
                        .await;
                    }
                    "unblock" => {
                        let _ = channel
                            .delete_permission(
                                &ctx.http,
                                serenity::PermissionOverwriteType::Member(*target),
                            )
                            .await;
                        return reply(
                            ctx,
                            interaction,
                            embeds::ok(format!("🆓 Unblocked <@{target_id}>.")),
                            Vec::new(),
                            true,
                        )
                        .await;
                    }
                    "transfer" => {
                        let in_voice = ctx
                            .cache
                            .guild(serenity::GuildId::new(guild_id))
                            .map(|guild| {
                                guild.voice_states.values().any(|state| {
                                    state.user_id.get() == target_id
                                        && state.channel_id.map(|c| c.get()) == Some(channel_id)
                                })
                            })
                            .unwrap_or(false);
                        if !in_voice {
                            return reply(ctx, interaction, embeds::warn("Not in channel", "That member must be in the voice channel to receive ownership."), Vec::new(), true).await;
                        }
                        crate::db::tempvoice::set_owner(&data.db, channel_id, target_id).await?;
                        let _ = channel
                            .delete_permission(
                                &ctx.http,
                                serenity::PermissionOverwriteType::Member(serenity::UserId::new(
                                    owner_id,
                                )),
                            )
                            .await;
                        let _ = channel
                            .create_permission(
                                &ctx.http,
                                serenity::PermissionOverwrite {
                                    allow: crate::tempvoice::OWNER_ALLOW,
                                    deny: serenity::Permissions::empty(),
                                    kind: serenity::PermissionOverwriteType::Member(*target),
                                },
                            )
                            .await;
                        let (embed, rows) = crate::tempvoice::build_control_panel(
                            target_id,
                            tvc_is_locked(&channel),
                        );
                        update_response(ctx, data, interaction, embed, rows).await?;
                        let _ = serenity::ChannelId::new(channel_id)
                            .send_message(
                                &ctx.http,
                                serenity::CreateMessage::new().content(format!(
                                    "🔁 Ownership transferred to <@{target_id}>."
                                )),
                            )
                            .await;
                        return Ok(());
                    }
                    _ => return Err(anyhow::anyhow!("unknown transfer target")),
                }
            }
            let (placeholder, mention) = match kind {
                "trust" => ("Trust a member…", false),
                "untrust" => ("Untrust a member…", false),
                "kick" => ("Kick a member…", false),
                "block" => ("Block a member…", false),
                "unblock" => ("Unblock a member…", false),
                _ => ("Transfer ownership…", false),
            };
            let _ = mention;
            let select = serenity::CreateSelectMenu::new(
                format!("tvc:{kind}:pick"),
                serenity::CreateSelectMenuKind::User {
                    default_users: None,
                },
            )
            .placeholder(placeholder)
            .min_values(1)
            .max_values(1);
            let prompt = match kind {
                "trust" => "Select a member to **trust** (can join even when locked):",
                "untrust" => "Select a member to **untrust**:",
                "kick" => "Select a member to **kick** from the voice channel:",
                "block" => "Select a member to **block** (cannot join):",
                "unblock" => "Select a member to **unblock**:",
                _ => "Select a member **in this voice channel** to transfer ownership to:",
            };
            reply(
                ctx,
                interaction,
                embeds::brand(format!("TempVoice — {kind}")).description(prompt),
                vec![CreateActionRow::SelectMenu(select)],
                true,
            )
            .await
        }

        other => Err(anyhow::anyhow!(
            "unknown tempvoice action `{other}` in {}",
            tvc_row_label(&row)
        )),
    }
}

// ------------------------------------------------- tempvoice interface modals

async fn tempvoice_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let Some(_guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Server only", "Temp voice works inside a server."),
        )
        .await;
    };
    let channel_id = interaction.channel_id.get();
    let Some(row) = crate::db::tempvoice::get_channel(&data.db, channel_id).await? else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Not temporary", "This isn't a temporary voice channel."),
        )
        .await;
    };
    let owner_id = row.owner_id().unwrap_or(0);
    let is_admin = permissions::modal_is_admin(ctx, data, interaction).await;
    if interaction.user.id.get() != owner_id && !is_admin {
        return finish_modal(ctx, interaction, embeds::denied("the channel owner")).await;
    }

    match interaction.data.custom_id.as_str() {
        "tvcmodal:rename" => {
            let name = trimmed(interaction, "name")
                .ok_or_else(|| anyhow::anyhow!("name can't be empty"))?
                .chars()
                .take(100)
                .collect::<String>();
            let channel = tvc_fetch_channel(ctx, channel_id).await?;
            let edited = serenity::EditChannel::new().name(name.clone());
            let mut channel = channel;
            channel.edit(&ctx.http, edited).await?;
            finish_modal(
                ctx,
                interaction,
                embeds::ok(format!(
                    "Renamed to **{name}**. (Discord limits renames to 2 per 10 minutes.)"
                )),
            )
            .await
        }
        "tvcmodal:limit" => {
            let raw = trimmed(interaction, "limit").unwrap_or_else(|| "0".to_string());
            let limit: u32 = raw
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("enter a whole number from 0 to 99"))?;
            if limit > 99 {
                return finish_modal(
                    ctx,
                    interaction,
                    embeds::warn("Out of range", "Enter a whole number from 0 to 99."),
                )
                .await;
            }
            let channel = tvc_fetch_channel(ctx, channel_id).await?;
            let edited = serenity::EditChannel::new().user_limit(limit);
            let mut channel = channel;
            channel.edit(&ctx.http, edited).await?;
            finish_modal(
                ctx,
                interaction,
                embeds::ok(if limit == 0 {
                    "Limit removed.".to_string()
                } else {
                    format!("Limit set to **{limit}**.")
                }),
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown tempvoice modal `{other}`")),
    }
}

// -------------------------------------------------- giveaway advanced rules

/// Picker for active giveaways — opens the rules editor for one.
async fn giveawayrules_pick_page(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let active = db::giveaways::list_active(&data.db, guild_id, 25).await?;
    let embed = embeds::brand(format!("{} Advanced rules", icons::GIFT))
        .color(colors::GOLD)
        .description(if active.is_empty() {
            "_No active giveaways. Create one first from **Giveaways → New giveaway**._".to_string()
        } else {
            "Pick a giveaway to tune its advanced rules (blocked roles, entry requirements, bonus entries, weight cap).".to_string()
        })
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let mut rows: Vec<CreateActionRow> = Vec::new();
    if !active.is_empty() {
        let options: Vec<serenity::CreateSelectMenuOption> = active
            .iter()
            .map(|giveaway| {
                serenity::CreateSelectMenuOption::new(
                    format!("#{} — {}", giveaway.id, giveaway.prize)
                        .chars()
                        .take(100)
                        .collect::<String>(),
                    giveaway.id.to_string(),
                )
                .description(format!("ends <t:{}:R>", giveaway.ends_at / 1000))
            })
            .collect();
        rows.push(CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                "giveawayrules:edit",
                serenity::CreateSelectMenuKind::String { options },
            )
            .placeholder("Giveaway…")
            .min_values(1)
            .max_values(1),
        ));
    }
    rows.push(CreateActionRow::Buttons(vec![components::secondary(
        "giveawayrules:back",
        format!("{} Back", icons::BACK),
    )]));
    reply(ctx, interaction, embed, rows, true).await
}

fn giveawayrules_editor_rows(
    giveaway_id: i64,
    rules: &crate::db::giveaways::GiveawayRules,
    guild_roles: &[(u64, String)],
) -> Vec<CreateActionRow> {
    let row1 = CreateActionRow::Buttons(vec![
        components::secondary(
            format!("giveawayrules:{giveaway_id}:numbers"),
            "🔢 Limits & bonuses",
        ),
        components::secondary(
            format!("giveawayrules:{giveaway_id}:limits"),
            "🎯 Level & weight cap",
        ),
    ]);

    let role_ids: Vec<serenity::RoleId> = guild_roles
        .iter()
        .map(|(id, _)| serenity::RoleId::new(*id))
        .take(25)
        .collect();
    let defaults_intersect = |defaults: &[u64]| {
        role_ids
            .iter()
            .filter(|id| defaults.contains(&id.get()))
            .cloned()
            .collect::<Vec<_>>()
    };

    let blocked_defaults = defaults_intersect(&rules.blocked_role_ids);
    let blocked_select = serenity::CreateSelectMenu::new(
        format!("giveawayrules:{giveaway_id}:blocked"),
        serenity::CreateSelectMenuKind::Role {
            default_roles: (!blocked_defaults.is_empty()).then_some(blocked_defaults),
        },
    )
    .placeholder("Blocked roles (can't enter)…")
    .min_values(0)
    .max_values(10);
    let blocked_row = CreateActionRow::SelectMenu(blocked_select);

    let bonus_defaults = defaults_intersect(&rules.bonus_role_ids);
    let bonus_select = serenity::CreateSelectMenu::new(
        format!("giveawayrules:{giveaway_id}:bonus"),
        serenity::CreateSelectMenuKind::Role {
            default_roles: (!bonus_defaults.is_empty()).then_some(bonus_defaults),
        },
    )
    .placeholder("Bonus-entry roles (+N each)…")
    .min_values(0)
    .max_values(10);
    let bonus_row = CreateActionRow::SelectMenu(bonus_select);

    let row4 = CreateActionRow::Buttons(vec![components::secondary(
        "giveawayrules:back",
        format!("{} Back", icons::BACK),
    )]);

    vec![row1, blocked_row, bonus_row, row4]
}

async fn giveawayrules_editor_page(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
    giveaway_id: i64,
    update: bool,
) -> anyhow::Result<()> {
    let giveaway = db::giveaways::get(&data.db, giveaway_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no giveaway with id {giveaway_id}"))?;
    if giveaway.guild_id != guild_id.to_string() {
        return reply(
            ctx,
            interaction,
            embeds::warn("Not yours", "That giveaway belongs to another server."),
            Vec::new(),
            true,
        )
        .await;
    }
    let rules = giveaway.rules();
    let (requirements, bonuses) = rules.describe();

    let mut description = format!(
        "**Prize:** {}\n**Status:** `{}`\n**Active entries:** `{}`\n\n",
        giveaway.prize,
        giveaway.status,
        db::giveaways::count_entries(&data.db, giveaway_id)
            .await
            .unwrap_or(0),
    );
    if requirements.is_empty() {
        description.push_str("**Requirements:** none\n");
    } else {
        description.push_str(&format!("**Requirements:** {}\n", requirements.join(", ")));
    }
    if bonuses.is_empty() {
        description.push_str("**Bonus entries:** none");
    } else {
        description.push_str(&format!("**Bonus entries:** {}", bonuses.join(", ")));
    }

    let embed = embeds::brand(format!("{} Advanced rules — #{}", icons::GIFT, giveaway.id))
        .color(colors::GOLD)
        .description(description)
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    // Guild roles for the select menus (first 25).
    let roles: Vec<(u64, String)> = ctx
        .cache
        .guild(serenity::GuildId::new(guild_id))
        .map(|guild| {
            guild
                .roles
                .iter()
                .filter(|(id, role)| id.get() != guild_id && !role.managed)
                .map(|(id, role)| (id.get(), role.name.clone()))
                .collect()
        })
        .unwrap_or_default();

    let rows = giveawayrules_editor_rows(giveaway_id, &rules, &roles);
    if update {
        update_response(ctx, data, interaction, embed, rows).await
    } else {
        reply(ctx, interaction, embed, rows, true).await
    }
}

async fn giveawayrules_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("giveaway rules only work inside a server"))?;

    if !permissions::component_is_admin(ctx, data, interaction).await {
        return reply(
            ctx,
            interaction,
            embeds::denied("Manage Server"),
            Vec::new(),
            true,
        )
        .await;
    }

    let mut parts = action.split(':');
    let kind = parts.next().unwrap_or("");
    let rest = parts.collect::<Vec<_>>().join(":");

    match kind {
        "pick" => giveawayrules_pick_page(ctx, data, interaction, guild_id).await,

        "edit" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a giveaway picker")),
            };
            let Some(id) = values.first().and_then(|v| v.parse::<i64>().ok()) else {
                return giveawayrules_pick_page(ctx, data, interaction, guild_id).await;
            };
            giveawayrules_editor_page(ctx, data, interaction, guild_id, id, false).await
        }

        "back" => {
            let view = crate::ui::panels::GiveawayView {
                active: db::giveaways::count_active(&data.db, guild_id)
                    .await
                    .unwrap_or(0),
            };
            let (embed, rows) = crate::ui::panels::giveaways_panel(&view);
            update_response(ctx, data, interaction, embed, rows).await
        }

        _ => {
            // `{id}:{action}`
            let Ok(id) = kind.parse::<i64>() else {
                return Err(anyhow::anyhow!("unknown giveawayrules action `{action}`"));
            };
            match rest.split(':').next().unwrap_or("") {
                "numbers" => {
                    let giveaway = db::giveaways::get(&data.db, id)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("no giveaway with id {id}"))?;
                    let rules = giveaway.rules();
                    let inputs = vec![
                        CreateActionRow::InputText(
                            components::short("min_level", "Min leveling level (0 = off)")
                                .value(rules.min_level.to_string()),
                        ),
                        CreateActionRow::InputText(
                            components::short(
                                "min_account_days",
                                "Min account age in days (0 = off)",
                            )
                            .value(rules.min_account_days.to_string()),
                        ),
                        CreateActionRow::InputText(
                            components::short(
                                "min_server_days",
                                "Min server age in days (0 = off)",
                            )
                            .value(rules.min_server_days.to_string()),
                        ),
                        CreateActionRow::InputText(
                            components::short("role_bonus", "Extra entries per bonus role (0-50)")
                                .value(rules.role_bonus.to_string()),
                        ),
                        CreateActionRow::InputText(
                            components::short("booster_bonus", "Extra entries for boosters (0-50)")
                                .value(rules.booster_bonus.to_string()),
                        ),
                    ];
                    interaction
                        .create_response(
                            &ctx.http,
                            modal(
                                &format!("giveawayrulesmodal:numbers:{id}"),
                                "Tune limits",
                                inputs,
                            ),
                        )
                        .await?;
                    Ok(())
                }
                "limits" => {
                    let giveaway = db::giveaways::get(&data.db, id)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("no giveaway with id {id}"))?;
                    let rules = giveaway.rules();
                    let inputs = vec![
                        CreateActionRow::InputText(
                            components::short("level_step", "+1 entry per N levels (0 = off)")
                                .value(rules.level_step.to_string()),
                        ),
                        CreateActionRow::InputText(
                            components::short("max_weight", "Max entries per member")
                                .value(rules.max_weight.to_string()),
                        ),
                    ];
                    interaction
                        .create_response(
                            &ctx.http,
                            modal(
                                &format!("giveawayrulesmodal:limits:{id}"),
                                "Level bonus & cap",
                                inputs,
                            ),
                        )
                        .await?;
                    Ok(())
                }
                "blocked" | "bonus" => {
                    let values = match &interaction.data.kind {
                        serenity::ComponentInteractionDataKind::RoleSelect { values } => {
                            values.clone()
                        }
                        _ => return Err(anyhow::anyhow!("expected a role picker")),
                    };
                    let ids: Vec<u64> = values.iter().map(|id| id.get()).collect();
                    let giveaway = db::giveaways::get(&data.db, id)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("no giveaway with id {id}"))?;
                    let mut rules = giveaway.rules();
                    if rest.split(':').next() == Some("blocked") {
                        rules.blocked_role_ids = ids;
                    } else {
                        rules.bonus_role_ids = ids;
                    }
                    rules.normalize();
                    db::giveaways::set_rules(&data.db, id, &rules).await?;
                    giveawayrules_editor_page(ctx, data, interaction, guild_id, id, true).await
                }
                other => Err(anyhow::anyhow!("unknown giveawayrules action `{other}`")),
            }
        }
    }
}

async fn giveawayrules_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
    prefix: &str,
    id: i64,
) -> anyhow::Result<()> {
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let giveaway = db::giveaways::get(&data.db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no giveaway with id {id}"))?;
    if giveaway.guild_id != guild.to_string() {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Not yours", "That giveaway belongs to another server."),
        )
        .await;
    }
    let mut rules = giveaway.rules();

    let number = |id: &str, default: i64| -> i64 {
        trimmed(interaction, id)
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .unwrap_or(default)
    };

    match prefix {
        "numbers" => {
            rules.min_level = number("min_level", rules.min_level).clamp(0, 1000);
            rules.min_account_days =
                number("min_account_days", rules.min_account_days).clamp(0, 3650);
            rules.min_server_days = number("min_server_days", rules.min_server_days).clamp(0, 3650);
            rules.role_bonus = number("role_bonus", rules.role_bonus).clamp(0, 50);
            rules.booster_bonus = number("booster_bonus", rules.booster_bonus).clamp(0, 50);
        }
        "limits" => {
            rules.level_step = number("level_step", rules.level_step).clamp(0, 1000);
            rules.max_weight = number("max_weight", rules.max_weight).clamp(1, 100);
        }
        _ => return Err(anyhow::anyhow!("unknown giveawayrules modal `{prefix}`")),
    }

    defer_modal(ctx, interaction).await?;
    rules.normalize();
    db::giveaways::set_rules(&data.db, id, &rules).await?;
    finish_modal(ctx, interaction, embeds::ok("Advanced rules saved.")).await
}

// ---------------------------------------------------------- ballot create flow

async fn ballot_create_start(
    ctx: &serenity::Context,
    _data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let _ = guild_id;
    // Acknowledge with the create modal directly.
    let inputs = vec![
        CreateActionRow::InputText(components::short("title", "Title").max_length(200)),
        CreateActionRow::InputText(
            components::paragraph("description", "Description (optional)")
                .required(false)
                .max_length(500),
        ),
        CreateActionRow::InputText(
            components::short("duration", "Duration (blank = no end, e.g. 12h or 2d)")
                .required(false)
                .max_length(30),
        ),
    ];
    interaction
        .create_response(
            &ctx.http,
            modal("ballotmodal:create", "Create Ballot", inputs),
        )
        .await?;
    Ok(())
}

fn ballot_draft_embed(draft: &crate::ballots::BallotDraft) -> serenity::CreateEmbed {
    let questions = if draft.questions.is_empty() {
        "_No questions yet — add one with **Add question**._".to_string()
    } else {
        draft
            .questions
            .iter()
            .enumerate()
            .map(|(i, q)| {
                format!(
                    "**Q{}** — {} · {} pick(s) · {} options",
                    i + 1,
                    crate::error::truncate(&q.text, 80),
                    q.max_picks,
                    q.options.len()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let ends = match draft.ends_at {
        Some(ends) => crate::ui::theme::utc_with_relative(ends / 1000),
        None => "no end — close it manually".to_string(),
    };
    let role = match draft.role_id.map(|id| id.to_string()) {
        Some(id) => format!("<@&{id}> only"),
        None => "everyone".to_string(),
    };

    embeds::brand("🗳️ New vote — draft")
        .color(colors::BRAND)
        .description(format!(
            "**{title}**\n{description}\n\n{questions}\n\n**Ends:** {ends}\n**Who can vote:** {role}\n**Votes:** {public} · **Change after voting:** {change}",
            title = draft.title,
            description = if draft.description.is_empty() {
                "no description".to_string()
            } else {
                draft.description.clone()
            },
            questions = questions,
            ends = ends,
            role = role,
            public = if draft.show_votes { "public (names visible)" } else { "private" },
            change = if draft.allow_change { "allowed" } else { "final" },
        ))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()))
}

fn ballot_draft_rows(draft: &crate::ballots::BallotDraft) -> Vec<CreateActionRow> {
    use serenity::{CreateSelectMenu, CreateSelectMenuKind};

    let mut rows = vec![
        CreateActionRow::Buttons(vec![
            components::primary("ballot:addq", "➕ Add question"),
            components::secondary(
                "ballot:togglepublic",
                format!(
                    "👁️ {}",
                    if draft.show_votes {
                        "Public"
                    } else {
                        "Private"
                    }
                ),
            ),
            components::secondary(
                "ballot:togglechange",
                format!(
                    "🔒 {}",
                    if draft.allow_change {
                        "Changeable"
                    } else {
                        "Final"
                    }
                ),
            ),
        ]),
        CreateActionRow::Buttons(vec![
            components::success("ballot:post", "📢 Post vote"),
            components::danger("ballot:discard", "🗑️ Discard"),
        ]),
    ];

    // Role restriction picker (unset = everyone).
    rows.push(CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            "ballot:setrole",
            CreateSelectMenuKind::Role {
                default_roles: draft.role_id.map(|id| vec![serenity::RoleId::new(id)]),
            },
        )
        .placeholder("Restrict to role… (everyone if unset)")
        .min_values(0)
        .max_values(1),
    ));

    if !draft.questions.is_empty() {
        // Role select (everyone when no default picked)
        rows.push(CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                "ballot:removeq",
                CreateSelectMenuKind::String {
                    options: draft
                        .questions
                        .iter()
                        .enumerate()
                        .map(|(index, q)| {
                            serenity::CreateSelectMenuOption::new(
                                format!("Q{} — {}", index + 1, crate::error::truncate(&q.text, 90)),
                                index.to_string(),
                            )
                        })
                        .collect(),
                },
            )
            .placeholder("Remove a question…")
            .min_values(1)
            .max_values(1),
        ));
    }

    rows
}

async fn ballot_draft_page(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    user_id: u64,
    update: bool,
) -> anyhow::Result<()> {
    let Some(draft) = crate::ballots::draft_get(user_id) else {
        return reply(
            ctx,
            interaction,
            embeds::warn("Draft expired", "Start again with **Create vote**."),
            Vec::new(),
            true,
        )
        .await;
    };
    let embed = ballot_draft_embed(&draft);
    let rows = ballot_draft_rows(&draft);
    if update {
        update_response(ctx, data, interaction, embed, rows).await
    } else {
        reply(ctx, interaction, embed, rows, true).await
    }
}

async fn ballot_create_modal(
    ctx: &serenity::Context,
    _data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let guild = guild_of(interaction)?;
    let title =
        trimmed(interaction, "title").ok_or_else(|| anyhow::anyhow!("title can't be empty"))?;
    let description = trimmed(interaction, "description").unwrap_or_default();
    let duration_raw = trimmed(interaction, "duration");
    let mut duration_ms: Option<i64> = None;
    if let Some(raw) = duration_raw {
        match crate::ballots::parse_duration(&raw) {
            Some(ms) => duration_ms = Some(ms),
            None => {
                return finish_modal(
                    ctx,
                    interaction,
                    embeds::warn(
                        "Duration not understood",
                        "Try `12h`, `2d`, `1w`, or leave it blank.",
                    ),
                )
                .await
            }
        }
    }

    let ends_at = duration_ms.map(|ms| crate::db::now() * 1000 + ms);
    crate::ballots::draft_new(
        interaction.user.id.get(),
        guild,
        title,
        description,
        ends_at,
    );

    let Some(draft) = crate::ballots::draft_get(interaction.user.id.get()) else {
        return finish_modal(
            ctx,
            interaction,
            embeds::error("Draft lost", "Could not start the draft — try again."),
        )
        .await;
    };
    // Reply with the draft page as a fresh ephemeral message (modals can't UpdateMessage).
    let embed = ballot_draft_embed(&draft);
    let rows = ballot_draft_rows(&draft);
    interaction
        .create_response(&ctx.http, message(embed, rows, true))
        .await?;
    Ok(())
}

async fn ballot_add_question_modal(
    ctx: &serenity::Context,
    _data: &Data,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get();
    let question = trimmed(interaction, "question")
        .ok_or_else(|| anyhow::anyhow!("question can't be empty"))?;
    let raw_options = trimmed(interaction, "options")
        .ok_or_else(|| anyhow::anyhow!("list at least 2 answers"))?;
    let picks_raw = trimmed(interaction, "picks");

    let options: Vec<String> = raw_options
        .split('\n')
        .map(|line| {
            line.trim()
                .trim_start_matches(['-', '*', '•'])
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect();
    if options.len() < 2 {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Need 2+ answers", "Add one answer per line (2 to 10)."),
        )
        .await;
    }
    if options.len() > 10 {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Too many answers", "At most 10 answers are allowed."),
        )
        .await;
    }
    if options.iter().any(|option| option.chars().count() > 80) {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn(
                "Answer too long",
                "Each answer must be 80 characters or fewer.",
            ),
        )
        .await;
    }
    {
        let mut seen = std::collections::HashSet::new();
        for option in &options {
            if !seen.insert(option.to_lowercase()) {
                return finish_modal(
                    ctx,
                    interaction,
                    embeds::warn(
                        "Duplicate answer",
                        "Answers must be different from each other.",
                    ),
                )
                .await;
            }
        }
    }

    let mut max_picks: i64 = 1;
    if let Some(raw) = picks_raw {
        let text = raw.trim().to_ascii_lowercase();
        if ["any", "all", "unlimited", "many", "multiple"].contains(&text.as_str()) {
            max_picks = options.len() as i64;
        } else {
            match text.parse::<i64>() {
                Ok(n) if n >= 1 && n <= options.len() as i64 => max_picks = n,
                _ => {
                    return finish_modal(
                        ctx,
                        interaction,
                        embeds::warn(
                            "Picks not understood",
                            format!("Use a number from 1 to {}, or `any`.", options.len()),
                        ),
                    )
                    .await
                }
            }
        }
    }

    crate::ballots::draft_set(user_id, |draft| {
        draft.questions.push(crate::db::ballots::BallotQuestion {
            text: question,
            options,
            max_picks,
        });
    });

    let Some(draft) = crate::ballots::draft_get(user_id) else {
        return finish_modal(
            ctx,
            interaction,
            embeds::warn("Draft expired", "Start again with **Create vote**."),
        )
        .await;
    };
    let embed = ballot_draft_embed(&draft);
    let rows = ballot_draft_rows(&draft);
    interaction
        .create_response(&ctx.http, message(embed, rows, true))
        .await?;
    Ok(())
}

async fn ballot_draft_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return reply(
            ctx,
            interaction,
            embeds::warn("Server only", "Votes live inside a server."),
            Vec::new(),
            true,
        )
        .await;
    };
    let user_id = interaction.user.id.get();

    if !permissions::component_is_admin(ctx, data, interaction).await {
        return reply(
            ctx,
            interaction,
            embeds::denied("Manage Server"),
            Vec::new(),
            true,
        )
        .await;
    }

    match action {
        "addq" => {
            let inputs = vec![
                CreateActionRow::InputText(
                    components::short("question", "Question").max_length(200),
                ),
                CreateActionRow::InputText(
                    components::paragraph("options", "Answers — one per line (2 to 10)")
                        .max_length(1000),
                ),
                CreateActionRow::InputText(
                    components::short("picks", "How many picks? (blank = 1, or `any`)")
                        .required(false)
                        .max_length(10),
                ),
            ];
            interaction
                .create_response(&ctx.http, modal("ballotmodal:addq", "Add Question", inputs))
                .await?;
            Ok(())
        }
        "togglepublic" => {
            crate::ballots::draft_set(user_id, |draft| draft.show_votes = !draft.show_votes);
            ballot_draft_page(ctx, data, interaction, user_id, true).await
        }
        "togglechange" => {
            crate::ballots::draft_set(user_id, |draft| draft.allow_change = !draft.allow_change);
            ballot_draft_page(ctx, data, interaction, user_id, true).await
        }
        "removeq" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a question picker")),
            };
            let Some(index) = values.first().and_then(|v| v.parse::<usize>().ok()) else {
                return ballot_draft_page(ctx, data, interaction, user_id, true).await;
            };
            crate::ballots::draft_set(user_id, |draft| {
                if index < draft.questions.len() {
                    draft.questions.remove(index);
                }
            });
            ballot_draft_page(ctx, data, interaction, user_id, true).await
        }
        "setrole" => {
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::RoleSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected a role picker")),
            };
            let role = values.first().map(|id| id.get());
            crate::ballots::draft_set(user_id, |draft| draft.role_id = role);
            ballot_draft_page(ctx, data, interaction, user_id, true).await
        }
        "post" => {
            let Some(draft) = crate::ballots::draft_get(user_id) else {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Draft expired", "Start again with **Create vote**."),
                    Vec::new(),
                    true,
                )
                .await;
            };
            if draft.guild_id != guild_id {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Wrong server", "That draft belongs to another server."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            if draft.questions.is_empty() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Add a question", "A vote needs at least one question."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let channel = db::voting::settings(&data.db, guild_id)
                .await
                .ok()
                .and_then(|s| s.channel_id)
                .unwrap_or_else(|| interaction.channel_id.get());
            let ends_at = draft.ends_at;
            let ballot_id = db::ballots::create(
                &data.db,
                guild_id,
                channel,
                &draft.title,
                Some(&draft.description),
                &draft.questions,
                draft.role_id,
                draft.show_votes,
                draft.allow_change,
                user_id,
                ends_at,
            )
            .await?;
            crate::ballots::draft_clear(user_id);
            match crate::ballots::post_ballot(ctx, data, ballot_id, channel).await {
                Ok(message_id) => {
                    let embed = embeds::success(
                        "Vote posted",
                        format!(
                            "Ballot **#{ballot_id}** is live in <#{channel}> (`{message_id}`)."
                        ),
                    );
                    update_response(ctx, data, interaction, embed, Vec::new()).await
                }
                Err(err) => {
                    update_response(
                        ctx,
                        data,
                        interaction,
                        embeds::error("Couldn't post", format!("`{}`", crate::error::public(&err))),
                        Vec::new(),
                    )
                    .await
                }
            }
        }
        "discard" => {
            crate::ballots::draft_clear(user_id);
            update_response(
                ctx,
                data,
                interaction,
                embeds::ok("Draft discarded."),
                Vec::new(),
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown ballot action `{other}`")),
    }
}

// --------------------------------------------------------- ballot vote flow

async fn ballot_fetch(
    data: &Data,
    ballot_id: i64,
    guild_id: u64,
) -> anyhow::Result<crate::db::ballots::Ballot> {
    let ballot = crate::db::ballots::get(&data.db, ballot_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no ballot with id {ballot_id}"))?;
    if ballot.guild_id != guild_id.to_string() {
        return Err(anyhow::anyhow!("that ballot belongs to another server"));
    }
    Ok(ballot)
}

async fn ballot_role_check(
    _ctx: &serenity::Context,
    interaction: &ComponentInteraction,
    ballot: &crate::db::ballots::Ballot,
) -> anyhow::Result<()> {
    let Some(role_id) = ballot.expected_role_id() else {
        return Ok(());
    };
    let has_role = interaction
        .member
        .as_ref()
        .map(|member| member.roles.iter().any(|id| id.get() == role_id))
        .unwrap_or(false);
    if has_role {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "you need <@&{role_id}> to vote in this ballot"
        ))
    }
}

async fn ballot_vote_page(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    ballot: &crate::db::ballots::Ballot,
    update_message: bool,
) -> anyhow::Result<()> {
    use serenity::{CreateActionRow, CreateSelectMenu, CreateSelectMenuKind};

    let user_id = interaction.user.id.get();
    let answers = crate::ballots::vote_session(ballot.id, user_id, &ballot.questions);
    let already_voted = crate::db::ballots::has_voted(&data.db, ballot.id, user_id).await?;

    let mut rows: Vec<CreateActionRow> = Vec::new();
    let mut description = format!("**{}**\n\n", crate::error::truncate(&ballot.title, 200));
    for (qi, question) in ballot.questions.iter().enumerate() {
        let picks = answers.get(qi).cloned().unwrap_or_default();
        let pick_text = if question.max_picks <= 1 {
            "pick 1".to_string()
        } else if question.max_picks >= question.options.len() as i64 {
            "pick any".to_string()
        } else {
            format!("pick up to {}", question.max_picks)
        };
        description.push_str(&format!(
            "**Q{}** — {} ({pick_text}) — {} selected\n",
            qi + 1,
            crate::error::truncate(&question.text, 120),
            picks.len()
        ));
        let options: Vec<serenity::CreateSelectMenuOption> = question
            .options
            .iter()
            .enumerate()
            .map(|(index, label)| {
                serenity::CreateSelectMenuOption::new(
                    label.chars().take(100).collect::<String>(),
                    index.to_string(),
                )
                .default_selection(picks.contains(&index))
            })
            .collect();
        rows.push(CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("ballot:qsel:{}:{}", ballot.id, qi),
                CreateSelectMenuKind::String { options },
            )
            .placeholder(format!(
                "Q{} — {}",
                qi + 1,
                crate::error::truncate(&question.text, 40)
            ))
            .min_values(0)
            .max_values(question.max_picks.clamp(1, 25) as u8),
        ));
    }

    let embed = embeds::brand("🗳️ Cast your vote")
        .color(colors::BRAND)
        .description(crate::error::truncate(&description, 4000))
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Ballot #{} · {}{}",
            ballot.id,
            if already_voted {
                "you already voted · "
            } else {
                ""
            },
            theme::footer()
        )));

    rows.push(CreateActionRow::Buttons(vec![
        components::success(format!("ballot:submit:{}", ballot.id), "✅ Submit"),
        components::secondary(format!("ballot:reset:{}", ballot.id), "↩️ Reset"),
    ]));

    if update_message {
        interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .embed(embed)
                        .components(rows),
                ),
            )
            .await?;
    } else {
        reply(ctx, interaction, embed, rows, true).await?;
    }
    Ok(())
}

async fn ballot_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("ballots only work inside a server"))?;

    let mut parts = action.split(':');
    let kind = parts.next().unwrap_or("");
    let id: i64 = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing ballot id"))?
        .parse()?;

    // Admin-only actions
    match kind {
        "manage" | "extend" | "close" | "remind" | "delete" | "delete_confirm" | "extend_modal"
            if !permissions::component_is_admin(ctx, data, interaction).await =>
        {
            return reply(
                ctx,
                interaction,
                embeds::denied("Manage Server"),
                Vec::new(),
                true,
            )
            .await;
        }
        _ => {}
    }

    match kind {
        "vote" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            if !ballot.is_active() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Closed", "This vote is closed."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            ballot_role_check(ctx, interaction, &ballot).await?;
            if !ballot.allow_change {
                let already =
                    crate::db::ballots::has_voted(&data.db, ballot.id, interaction.user.id.get())
                        .await?;
                if already {
                    return reply(
                        ctx,
                        interaction,
                        embeds::warn(
                            "Votes are final",
                            "This ballot doesn't allow changing your vote.",
                        ),
                        Vec::new(),
                        true,
                    )
                    .await;
                }
            }
            // Prime the session with their existing picks if they already voted.
            if let Ok(existing) =
                crate::db::ballots::answers(&data.db, &ballot, interaction.user.id.get()).await
            {
                if !existing.iter().all(|picks| picks.is_empty()) {
                    let user_id = interaction.user.id.get();
                    for (qi, picks) in existing.into_iter().enumerate() {
                        crate::ballots::vote_session_set(
                            ballot.id,
                            user_id,
                            &ballot.questions,
                            qi,
                            picks,
                        );
                    }
                }
            }
            ballot_vote_page(ctx, data, interaction, &ballot, false).await
        }

        "qsel" => {
            let question_index: usize = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing question index"))?
                .parse()?;
            let values = match &interaction.data.kind {
                serenity::ComponentInteractionDataKind::StringSelect { values } => values.clone(),
                _ => return Err(anyhow::anyhow!("expected an answer picker")),
            };
            let picks: Vec<usize> = values
                .iter()
                .filter_map(|v| v.parse::<usize>().ok())
                .collect();
            let ballot = ballot_fetch(data, id, guild_id).await?;
            if !ballot.is_active() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Closed", "This vote is closed."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            ballot_role_check(ctx, interaction, &ballot).await?;
            crate::ballots::vote_session_set(
                ballot.id,
                interaction.user.id.get(),
                &ballot.questions,
                question_index,
                picks,
            );
            // Re-render the ephemeral page with the new counts.
            ballot_vote_page(ctx, data, interaction, &ballot, true).await
        }

        "reset" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            crate::ballots::vote_session_clear(ballot.id, interaction.user.id.get());
            ballot_vote_page(ctx, data, interaction, &ballot, true).await
        }

        "clear" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            crate::db::ballots::withdraw(&data.db, ballot.id, interaction.user.id.get()).await?;
            crate::ballots::vote_session_clear(ballot.id, interaction.user.id.get());
            let _ = crate::ballots::rebuild_ballot_message(ctx, data, ballot.id).await;
            reply(
                ctx,
                interaction,
                embeds::ok("Your vote was removed."),
                Vec::new(),
                true,
            )
            .await
        }

        "submit" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            if !ballot.is_active() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Closed", "This vote is closed."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            ballot_role_check(ctx, interaction, &ballot).await?;
            if !ballot.allow_change {
                let already =
                    crate::db::ballots::has_voted(&data.db, ballot.id, interaction.user.id.get())
                        .await?;
                if already {
                    return reply(
                        ctx,
                        interaction,
                        embeds::warn(
                            "Votes are final",
                            "This ballot doesn't allow changing your vote.",
                        ),
                        Vec::new(),
                        true,
                    )
                    .await;
                }
            }
            let user_id = interaction.user.id.get();
            let answers = crate::ballots::vote_session(ballot.id, user_id, &ballot.questions);
            if answers.iter().all(|picks| picks.is_empty()) {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn(
                        "Nothing picked",
                        "Select at least one answer before submitting.",
                    ),
                    Vec::new(),
                    true,
                )
                .await;
            }
            crate::db::ballots::submit(&data.db, ballot.id, user_id, &answers).await?;
            crate::ballots::vote_session_clear(ballot.id, user_id);
            let _ = crate::ballots::rebuild_ballot_message(ctx, data, ballot.id).await;
            reply(
                ctx,
                interaction,
                embeds::success(
                    "Vote recorded",
                    format!("Thanks! Your vote on **{}** was saved.", ballot.title),
                ),
                Vec::new(),
                true,
            )
            .await
        }

        "parts" => render_ballot_parts(ctx, data, interaction, id, 0, true).await,
        "parts_page" => {
            let page: usize = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            render_ballot_parts(ctx, data, interaction, id, page, true).await
        }

        "manage" => render_ballot_manage(ctx, data, interaction, id, true).await,

        "extend_modal" => {
            let inputs = vec![CreateActionRow::InputText(
                components::short("delta", "Extend by (e.g. 24h, 3d, 1w, 90m)")
                    .value("24h".to_string()),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal(&format!("ballotmodal:extend:{id}"), "Extend timer", inputs),
                )
                .await?;
            Ok(())
        }

        "close" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            if !ballot.is_active() {
                return reply(
                    ctx,
                    interaction,
                    embeds::info("Already closed", "This vote is already closed."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let claimed = crate::db::ballots::claim_close(&data.db, ballot.id).await?;
            if !claimed {
                return reply(
                    ctx,
                    interaction,
                    embeds::info("Already closed", "Someone else closed it just now."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let _ = crate::ballots::rebuild_ballot_message(ctx, data, ballot.id).await;
            post_ballot_results(ctx, data, ballot.id).await?;
            render_ballot_manage(ctx, data, interaction, ballot.id, true).await
        }

        "remind" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            if !ballot.is_active() {
                return reply(
                    ctx,
                    interaction,
                    embeds::warn("Closed", "This vote is closed."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let now = crate::db::now() * 1000;
            let cooldown_ms: i64 = 10 * 60 * 1000;
            if now - ballot.last_reminder_at < cooldown_ms {
                let wait = ((ballot.last_reminder_at + cooldown_ms - now) / 60_000).max(1);
                return reply(
                    ctx,
                    interaction,
                    embeds::warn(
                        "Cooldown",
                        format!("Reminders were sent recently — try again in ~{wait} minutes."),
                    ),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let voter_ids = crate::db::ballots::list_voters(&data.db, &ballot)
                .await
                .map(|rows| {
                    rows.into_iter()
                        .map(|(user_id, _)| user_id)
                        .collect::<Vec<u64>>()
                })
                .unwrap_or_default();
            let pending: Vec<u64> = crate::ballots::expected_ids(ctx, guild_id, &ballot, 90)
                .into_iter()
                .filter(|id| !voter_ids.contains(id))
                .collect();
            if pending.is_empty() {
                return reply(
                    ctx,
                    interaction,
                    embeds::info("Everyone has voted", "No one to remind."),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let mentions: Vec<String> = pending
                .iter()
                .take(90)
                .map(|id| format!("<@{id}>"))
                .collect();
            let _ = serenity::ChannelId::new(
                ballot.channel_id().unwrap_or(interaction.channel_id.get()),
            )
            .send_message(
                &ctx.http,
                serenity::CreateMessage::new()
                    .content(format!(
                        "⏰ Reminder to vote on **{}**:\n{}",
                        ballot.title,
                        mentions.join(" ")
                    ))
                    .allowed_mentions(
                        serenity::CreateAllowedMentions::new().users(
                            pending
                                .iter()
                                .take(90)
                                .map(|id| serenity::UserId::new(*id))
                                .collect::<Vec<_>>(),
                        ),
                    ),
            )
            .await;
            let _ = sqlx::query("UPDATE ballots SET last_reminder_at = ? WHERE id = ?")
                .bind(now)
                .bind(ballot.id)
                .execute(&data.db.pool)
                .await;
            render_ballot_manage(ctx, data, interaction, ballot.id, true).await
        }

        "csv" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            let is_admin = permissions::component_is_admin(ctx, data, interaction).await;
            if !ballot.show_votes && !is_admin {
                return reply(ctx, interaction, embeds::denied("admins"), Vec::new(), true).await;
            }
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Defer(
                        CreateInteractionResponseMessage::new().ephemeral(true),
                    ),
                )
                .await?;
            let csv = build_ballot_csv(ctx, data, &ballot).await;
            interaction
                .create_followup(
                    &ctx.http,
                    serenity::CreateInteractionResponseFollowup::new()
                        .content("Vote export (CSV):")
                        .ephemeral(true)
                        .add_file(serenity::CreateAttachment::bytes(
                            csv.into_bytes(),
                            format!("ballot_{id}.csv"),
                        )),
                )
                .await?;
            Ok(())
        }

        "delete" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            let embed = embeds::warn(
                "Delete vote?",
                format!("Ballot **#{}** (**{}**) and all its votes will be deleted forever. This can't be undone.", ballot.id, ballot.title),
            );
            let rows = vec![CreateActionRow::Buttons(vec![
                components::danger(format!("ballot:delete_confirm:{id}"), "Delete forever"),
                components::secondary(format!("ballot:manage:{id}"), "Keep"),
            ])];
            update_response(ctx, data, interaction, embed, rows).await
        }

        "delete_confirm" => {
            let ballot = ballot_fetch(data, id, guild_id).await?;
            let _ = sqlx::query("DELETE FROM ballot_votes WHERE ballot_id = ?")
                .bind(ballot.id)
                .execute(&data.db.pool)
                .await;
            let _ = sqlx::query("DELETE FROM ballot_voters WHERE ballot_id = ?")
                .bind(ballot.id)
                .execute(&data.db.pool)
                .await;
            let _ = sqlx::query("DELETE FROM ballots WHERE id = ?")
                .bind(ballot.id)
                .execute(&data.db.pool)
                .await;
            if let (Some(channel_id), Some(message_id)) = (ballot.channel_id(), ballot.message_id())
            {
                let _ = serenity::ChannelId::new(channel_id)
                    .delete_message(&ctx.http, serenity::MessageId::new(message_id))
                    .await;
            }
            update_response(
                ctx,
                data,
                interaction,
                embeds::ok("Vote deleted."),
                Vec::new(),
            )
            .await
        }

        other => Err(anyhow::anyhow!("unknown ballot action `{other}`")),
    }
}

// -------------------------------------------------- ballot participants page

async fn render_ballot_parts(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    ballot_id: i64,
    page: usize,
    update: bool,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.map(|id| id.get()).unwrap_or(0);
    let ballot = ballot_fetch(data, ballot_id, guild_id).await?;
    let (counts, voters) = crate::ballots::tally_view(&data.db, &ballot).await?;
    let ballot_questions = ballot.questions.len();
    let is_admin = permissions::component_is_admin(ctx, data, interaction).await;
    let show_details = ballot.show_votes || is_admin;

    let voter_rows = crate::db::ballots::list_voters(&data.db, &ballot)
        .await
        .unwrap_or_default();
    let voter_ids: Vec<u64> = voter_rows.iter().map(|(id, _)| *id).collect();
    let expected = crate::ballots::expected_ids(ctx, guild_id, &ballot, 120);
    let pending: Vec<u64> = expected
        .into_iter()
        .filter(|id| !voter_ids.contains(id))
        .collect();

    // ---- embed body
    let mut body = String::new();
    if show_details {
        body.push_str(&format!("**Voted ({})**\n", voter_rows.len()));
        if voter_rows.is_empty() {
            body.push_str("_no one yet_\n");
        } else {
            const PAGE_SIZE: usize = 15;
            let pages = voter_rows.len().div_ceil(PAGE_SIZE).max(1);
            let page = page.min(pages - 1);
            for (index, (user_id, answers)) in voter_rows
                .iter()
                .skip(page * PAGE_SIZE)
                .take(PAGE_SIZE)
                .enumerate()
            {
                let name = crate::ballots::name_of(ctx, guild_id, *user_id);
                let summary = if answers.is_empty() {
                    "—".to_string()
                } else {
                    answers
                        .iter()
                        .enumerate()
                        .map(|(qi, picks)| {
                            let labels: Vec<String> = picks
                                .iter()
                                .filter_map(|oi| {
                                    ballot
                                        .questions
                                        .get(qi)
                                        .and_then(|q| q.options.get(*oi))
                                        .cloned()
                                })
                                .collect();
                            format!(
                                "Q{}: {}",
                                qi + 1,
                                if labels.is_empty() {
                                    "—".to_string()
                                } else {
                                    labels.join(", ")
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" · ")
                };
                body.push_str(&format!(
                    "`{:>2}.` **{}** — {}\n",
                    page * PAGE_SIZE + index + 1,
                    crate::error::truncate(&name, 24),
                    crate::error::truncate(&summary, 70)
                ));
            }
            body.push_str(&format!("\nPage {}/{}\n", page + 1, pages));
        }
        body.push_str(&format!("\n**Still to vote ({})**\n", pending.len()));
        if pending.is_empty() {
            body.push_str("_everyone who was expected has voted_\n");
        } else {
            let names: Vec<String> = pending
                .iter()
                .take(30)
                .map(|id| format!("<@{id}>"))
                .collect();
            body.push_str(&names.join(" "));
            if pending.len() > 30 {
                body.push_str(&format!(" …and {} more", pending.len() - 30));
            }
            body.push('\n');
        }
    } else {
        body.push_str(&format!(
            "**Participation**\n🗳️ **{}** voted · ⏳ **{}** still to vote\n\n_Votes are hidden on this ballot — only admins can see who voted._",
            voters,
            pending.len()
        ));
    }

    let _ = counts;

    let embed = embeds::brand(format!(
        "👥 Participants — {}",
        crate::error::truncate(&ballot.title, 200)
    ))
    .color(colors::BRAND)
    .description(crate::error::truncate(&body, 4000))
    .footer(serenity::CreateEmbedFooter::new(format!(
        "Ballot #{} · {} question(s) · {}",
        ballot.id,
        ballot_questions,
        theme::footer()
    )));

    const PAGE_SIZE: usize = 15;
    let pages = voter_rows.len().div_ceil(PAGE_SIZE).max(1);
    let page = page.min(pages - 1);
    let show_details = ballot.show_votes || is_admin;
    let mut rows: Vec<CreateActionRow> = Vec::new();
    if show_details && pages > 1 {
        rows.push(CreateActionRow::Buttons(vec![
            components::secondary(
                format!("ballot:parts_page:{ballot_id}:{}", page.saturating_sub(1)),
                "◀ Prev",
            )
            .disabled(page == 0),
            components::secondary(
                format!("ballot:parts_page:{ballot_id}:{}", page + 1),
                "Next ▶",
            )
            .disabled(page + 1 >= pages),
        ]));
    }
    let mut buttons = Vec::new();
    if is_admin {
        buttons.push(components::secondary(
            format!("ballot:csv:{ballot_id}"),
            "📤 Export CSV",
        ));
    }
    buttons.push(components::secondary(
        format!("ballot:vote:{ballot_id}"),
        "🗳️ Vote",
    ));
    rows.push(CreateActionRow::Buttons(buttons));

    if update {
        update_response(ctx, data, interaction, embed, rows).await
    } else {
        reply(ctx, interaction, embed, rows, true).await
    }
}

// ----------------------------------------------------- ballot manage page

async fn render_ballot_manage(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    ballot_id: i64,
    update: bool,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.map(|id| id.get()).unwrap_or(0);
    let ballot = ballot_fetch(data, ballot_id, guild_id).await?;
    let (_counts, voters) = crate::ballots::tally_view(&data.db, &ballot).await?;

    let timer = if ballot.is_active() {
        match ballot.ends_at {
            Some(ends) => crate::ui::theme::utc_with_relative(ends / 1000),
            None => "no timer — closes when you press **Close now**".to_string(),
        }
    } else {
        "closed".to_string()
    };

    let embed = embeds::brand(format!("⚙️ Manage — #{}", ballot.id))
        .color(colors::VIOLET)
        .description(format!(
            "**{title}**\n**Status:** `{status}`\n**Timer:** {timer}\n**Voters:** `{voters}`\n\nExtend the deadline, close it early, remind stragglers, export the results or delete the vote entirely.",
            title = ballot.title,
            status = ballot.status,
            timer = timer,
            voters = voters,
        ))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let mut buttons = vec![components::secondary(
        format!("ballot:extend_modal:{ballot_id}"),
        "⏰ Extend",
    )];
    if ballot.is_active() {
        buttons.push(components::danger(
            format!("ballot:close:{ballot_id}"),
            "⏱️ Close now",
        ));
        buttons.push(components::secondary(
            format!("ballot:remind:{ballot_id}"),
            "👥 Remind non-voters",
        ));
    }
    buttons.push(components::secondary(
        format!("ballot:csv:{ballot_id}"),
        "📤 Export CSV",
    ));
    buttons.push(components::danger(
        format!("ballot:delete:{ballot_id}"),
        "🗑️ Delete",
    ));

    let rows = vec![CreateActionRow::Buttons(buttons)];
    if update {
        update_response(ctx, data, interaction, embed, rows).await
    } else {
        reply(ctx, interaction, embed, rows, true).await
    }
}

async fn ballot_extend_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
    ballot_id: i64,
) -> anyhow::Result<()> {
    // Defense in depth: the button that opens this modal is already admin-gated.
    if !permissions::modal_is_admin(ctx, data, interaction).await {
        return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
    }
    let guild = guild_of(interaction)?;
    let delta = trimmed(interaction, "delta").unwrap_or_else(|| "24h".to_string());
    let add_ms = crate::ballots::parse_duration(&delta).ok_or_else(|| {
        anyhow::anyhow!("`{delta}` is not understood — try `24h`, `3d`, `1w`, `90m`")
    })?;
    defer_modal(ctx, interaction).await?;

    let ballot = ballot_fetch(data, ballot_id, guild).await?;
    let now = crate::db::now() * 1000;
    let base = ballot.ends_at.unwrap_or(now).max(now);
    let new_ends = base + add_ms;
    sqlx::query("UPDATE ballots SET ends_at = ? WHERE id = ?")
        .bind(new_ends)
        .bind(ballot.id)
        .execute(&data.db.pool)
        .await?;
    let _ = crate::ballots::rebuild_ballot_message(ctx, data, ballot.id).await;
    finish_modal(
        ctx,
        interaction,
        embeds::ok(format!(
            "Extended by **{delta}** — now closes <t:{}:R>.",
            new_ends / 1000
        )),
    )
    .await
}

// ------------------------------------------------------ results auto-post

/// Post the final results to the configured results channel (fallback: the
/// ballot's own channel) — used when a vote closes (sweeper or **Close now**).
pub async fn post_ballot_results(
    ctx: &serenity::Context,
    data: &Data,
    ballot_id: i64,
) -> anyhow::Result<()> {
    let Some(ballot) = crate::db::ballots::get(&data.db, ballot_id).await? else {
        return Ok(());
    };
    let results_channel =
        crate::db::voting::settings(&data.db, ballot.guild_id.parse().unwrap_or(0))
            .await
            .ok()
            .and_then(|s| s.results_channel_id)
            .or(ballot.channel_id());
    let Some(channel_id) = results_channel else {
        return Ok(());
    };
    let (counts, voters) = crate::ballots::tally_view(&data.db, &ballot).await?;
    let expected =
        crate::ballots::expected_count(ctx, ballot.guild_id.parse().unwrap_or(0), &ballot);
    let view = crate::ballots::BallotView {
        ballot: &ballot,
        counts,
        voters,
        expected,
    };
    let embed = crate::ballots::ballot_embed(&view);
    let rows = crate::ballots::ballot_rows(&ballot, ballot.message_id());

    let _ = serenity::ChannelId::new(channel_id)
        .send_message(
            &ctx.http,
            serenity::CreateMessage::new()
                .embed(embed)
                .components(rows)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await;
    Ok(())
}

// --------------------------------------------------------------- CSV export

async fn build_ballot_csv(
    ctx: &serenity::Context,
    data: &Data,
    ballot: &crate::db::ballots::Ballot,
) -> String {
    let guild_id = ballot.guild_id.parse().unwrap_or(0);
    let voter_rows = crate::db::ballots::list_voters(&data.db, ballot)
        .await
        .unwrap_or_default();
    let mut out = String::from("user_id,name,voted_at");
    for (qi, _) in ballot.questions.iter().enumerate() {
        out.push_str(&format!(",q{}", qi + 1));
    }
    out.push('\n');
    for (user_id, answers) in voter_rows {
        let name = crate::ballots::name_of(ctx, guild_id, user_id);
        let name = name.replace('"', "\"\"");
        out.push_str(&format!("\"{user_id}\",\"{name}\","));
        let cells: Vec<String> = answers
            .iter()
            .enumerate()
            .map(|(qi, picks)| {
                picks
                    .iter()
                    .filter_map(|oi| {
                        ballot
                            .questions
                            .get(qi)
                            .and_then(|q| q.options.get(*oi))
                            .cloned()
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
                    .replace('"', "\"\"")
            })
            .collect();
        out.push_str(
            &cells
                .into_iter()
                .map(|cell| format!("\"{cell}\""))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

// --------------------------------------------------------------- branding

async fn branding_settings_page(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    guild_id: u64,
) -> anyhow::Result<()> {
    let branding = db::branding::branding(&data.db).await.unwrap_or_default();
    let profile = db::branding::guild_profile(&data.db, guild_id)
        .await
        .unwrap_or_default();
    let is_owner = data.cfg.owner_id == Some(interaction.user.id.get());
    let view = panels::BrandingView {
        contact_enabled: branding.is_enabled(),
        contact_label: branding.label(),
        contact_url: branding.contact_url,
        can_manage: permissions::component_is_admin(ctx, data, interaction).await,
        is_owner,
        nickname: profile.nickname,
        avatar_url: profile.avatar_url,
    };
    let (embed, rows) = panels::branding_panel(&view);
    update_panel(ctx, data, interaction, embed, rows).await
}

async fn apply_branding_profile(ctx: &serenity::Context, data: &Data, guild_id: u64) {
    let Ok(profile) = db::branding::guild_profile(&data.db, guild_id).await else {
        return;
    };
    let Ok(me) = ctx.http.get_current_user().await else {
        return;
    };
    let guild = serenity::GuildId::new(guild_id);

    // Nickname via serenity (EditMember has no avatar support).
    if let Ok(mut member) = ctx.http.get_member(guild, me.id).await {
        let edit = match &profile.nickname {
            Some(nickname) => serenity::EditMember::new().nickname(nickname),
            None => serenity::EditMember::new().nickname(""), // empty resets the nickname
        };
        let _ = member.edit(&ctx.http, edit).await;
    }

    // Per-guild avatar via raw PATCH (Discord accepts a base64 data URI).
    push_guild_avatar(data, guild_id, profile.avatar_url.as_deref()).await;
}

/// PATCH /guilds/{id}/members/@me with an optional avatar data URI.
/// `None` clears the per-guild avatar.
async fn push_guild_avatar(data: &Data, guild_id: u64, avatar_url: Option<&str>) {
    let token = data.cfg.token.clone();
    if token.is_empty() {
        return;
    }

    let payload = match avatar_url {
        Some(url) => {
            const MAX_BYTES: usize = 10 * 1024 * 1024; // Discord caps the image at 10 MiB
            let client = match crate::net::builder()
                .user_agent("omnicore/2.0")
                .build()
            {
                Ok(client) => client,
                Err(_) => return,
            };
            let response = match client
                .get(url)
                .header(reqwest::header::ACCEPT, "image/*")
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    tracing::warn!(guild_id, error = %err, "branding: avatar fetch failed");
                    return;
                }
            };
            if !response.status().is_success() {
                tracing::warn!(guild_id, status = %response.status(), "branding: avatar fetch failed");
                return;
            }
            let mime = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("image/png")
                .split(';')
                .next()
                .unwrap_or("image/png")
                .trim()
                .to_ascii_lowercase();
            if !mime.starts_with("image/") {
                tracing::warn!(guild_id, %mime, "branding: avatar URL did not return an image");
                return;
            }
            let bytes = match response.bytes().await {
                Ok(bytes) if bytes.len() <= MAX_BYTES => bytes,
                Ok(_) => {
                    tracing::warn!(guild_id, "branding: avatar image larger than 10 MiB");
                    return;
                }
                Err(err) => {
                    tracing::warn!(guild_id, error = %err, "branding: avatar read failed");
                    return;
                }
            };
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            serde_json::json!({ "avatar": format!("data:{mime};base64,{encoded}") })
        }
        None => serde_json::json!({ "avatar": null }),
    };

    let response = crate::net::client()
        .patch(format!(
            "https://discord.com/api/v10/guilds/{guild_id}/members/@me"
        ))
        .header(reqwest::header::AUTHORIZATION, format!("Bot {token}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {
            tracing::info!(guild_id, "branding: per-guild avatar updated");
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(guild_id, %status, body = %crate::error::truncate(&body, 200), "branding: avatar rejected by Discord");
        }
        Err(err) => {
            tracing::warn!(guild_id, error = %err, "branding: avatar request failed");
        }
    }
}

async fn branding_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ComponentInteraction,
    action: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction
        .guild_id
        .map(|id| id.get())
        .ok_or_else(|| anyhow::anyhow!("branding only works inside a server"))?;
    let is_owner = data.cfg.owner_id == Some(interaction.user.id.get());

    match action {
        "toggle" => {
            if !is_owner {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let current = db::branding::branding(&data.db).await.unwrap_or_default();
            db::branding::set_branding_enabled(&data.db, !current.is_enabled()).await?;
            branding_settings_page(ctx, data, interaction, guild_id).await
        }
        "contact" => {
            if !is_owner {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let current = db::branding::branding(&data.db).await.unwrap_or_default();
            let inputs = vec![
                CreateActionRow::InputText(
                    components::short("label", "Button label")
                        .value(current.label())
                        .max_length(80),
                ),
                CreateActionRow::InputText(
                    components::short("url", "Link URL (https://…)")
                        .value(current.contact_url.unwrap_or_default())
                        .max_length(500),
                ),
            ];
            interaction
                .create_response(
                    &ctx.http,
                    modal("brandingmodal:contact", "Contact button", inputs),
                )
                .await?;
            Ok(())
        }
        "clearcontact" => {
            if !is_owner {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Bot Owner"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            db::branding::clear_branding(&data.db).await?;
            branding_settings_page(ctx, data, interaction, guild_id).await
        }
        "nickname" => {
            if !permissions::component_is_admin(ctx, data, interaction).await {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Manage Server"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let profile = db::branding::guild_profile(&data.db, guild_id)
                .await
                .unwrap_or_default();
            let inputs = vec![CreateActionRow::InputText(
                components::short("nickname", "Bot nickname (blank = clear)")
                    .required(false)
                    .value(profile.nickname.unwrap_or_default())
                    .max_length(32),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal("brandingmodal:nickname", "Bot nickname", inputs),
                )
                .await?;
            Ok(())
        }
        "avatar" => {
            if !permissions::component_is_admin(ctx, data, interaction).await {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Manage Server"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            let profile = db::branding::guild_profile(&data.db, guild_id)
                .await
                .unwrap_or_default();
            let inputs = vec![CreateActionRow::InputText(
                components::short("url", "Avatar image URL (blank = clear)")
                    .required(false)
                    .value(profile.avatar_url.unwrap_or_default())
                    .max_length(500),
            )];
            interaction
                .create_response(
                    &ctx.http,
                    modal("brandingmodal:avatar", "Bot avatar", inputs),
                )
                .await?;
            Ok(())
        }
        "clearnick" => {
            if !permissions::component_is_admin(ctx, data, interaction).await {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Manage Server"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            db::branding::set_nickname(&data.db, guild_id, None).await?;
            apply_branding_profile(ctx, data, guild_id).await;
            branding_settings_page(ctx, data, interaction, guild_id).await
        }
        "clearavatar" => {
            if !permissions::component_is_admin(ctx, data, interaction).await {
                return reply(
                    ctx,
                    interaction,
                    embeds::denied("Manage Server"),
                    Vec::new(),
                    true,
                )
                .await;
            }
            db::branding::set_avatar(&data.db, guild_id, None).await?;
            branding_settings_page(ctx, data, interaction, guild_id).await
        }
        other => Err(anyhow::anyhow!("unknown branding action `{other}`")),
    }
}

async fn branding_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &ModalInteraction,
    id: &str,
) -> anyhow::Result<()> {
    let guild_id = guild_of(interaction)?;
    match id {
        "contact" => {
            if data.cfg.owner_id != Some(interaction.user.id.get()) {
                return finish_modal(ctx, interaction, embeds::denied("Bot Owner")).await;
            }
            let label =
                trimmed(interaction, "label").unwrap_or_else(|| "Message Necromancer".to_string());
            let url = trimmed(interaction, "url");
            if let Some(url) = &url {
                if !(url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("mailto:"))
                {
                    return finish_modal(
                        ctx,
                        interaction,
                        embeds::warn("Bad URL", "Use an https://, http:// or mailto: link."),
                    )
                    .await;
                }
            }
            defer_modal(ctx, interaction).await?;
            db::branding::set_contact(&data.db, url.as_deref(), label.trim()).await?;
            finish_modal(
                ctx,
                interaction,
                embeds::ok("Contact button updated and enabled."),
            )
            .await
        }
        "nickname" => {
            if !permissions::modal_is_admin(ctx, data, interaction).await {
                return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
            }
            let raw = trimmed(interaction, "nickname");
            defer_modal(ctx, interaction).await?;
            db::branding::set_nickname(&data.db, guild_id, raw.as_deref()).await?;
            apply_branding_profile(ctx, data, guild_id).await;
            finish_modal(ctx, interaction, embeds::ok("Bot nickname updated.")).await
        }
        "avatar" => {
            if !permissions::modal_is_admin(ctx, data, interaction).await {
                return finish_modal(ctx, interaction, embeds::denied("Manage Server")).await;
            }
            let raw = trimmed(interaction, "url");
            defer_modal(ctx, interaction).await?;
            db::branding::set_avatar(&data.db, guild_id, raw.as_deref()).await?;
            finish_modal(
                ctx,
                interaction,
                embeds::ok("Avatar URL saved (stored; not pushed automatically)."),
            )
            .await
        }
        other => Err(anyhow::anyhow!("unknown branding modal `{other}`")),
    }
}
