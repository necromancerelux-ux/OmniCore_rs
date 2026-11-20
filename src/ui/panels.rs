//! The actual panels. Each returns an embed plus its component rows so the
//! caller can both send and edit messages from the same source of truth.
//!
//! Panels take plain data (no `Data`, no Discord client) which keeps them
//! cheap to render from commands, buttons, background refreshes and tests.

use poise::serenity_prelude as serenity;
use serenity::{ButtonStyle, CreateActionRow, CreateEmbed};

use super::components::{self, secondary};
use super::embeds;
use super::theme::{self, colors, icons};

/// Everything the music panel needs to draw itself.
#[derive(Debug, Clone, Default)]
pub struct NowPlayingView {
    pub title: String,
    pub author: String,
    pub uri: Option<String>,
    pub length_ms: u64,
    pub position_ms: u64,
    pub paused: bool,
    pub volume: u16,
    pub loop_label: String,
    pub requester: String,
    pub queue_len: usize,
    pub upcoming: Vec<(String, String)>,
    pub connected_channel: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LookupPlayerView {
    pub fid: String,
    pub game_name: Option<String>,
    pub alliance: Option<String>,
    pub power: Option<i64>,
    pub kills: Option<i64>,
    pub town_hall: Option<i64>,
    pub life_tree: Option<i64>,
    pub state: Option<i64>,
    pub avatar_url: Option<String>,
    pub synced: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct LookupAllianceView {
    pub abbr: String,
    pub name: Option<String>,
    pub members: usize,
    pub power: i64,
    pub kills: i64,
    pub top: Vec<(String, i64)>,
    pub synced: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct StateView {
    pub state: u64,
    pub alliances: usize,
    pub players: usize,
    pub power: i64,
    pub top_alliances: Vec<(String, i64, usize)>,
}

// ---------------------------------------------------------------- settings hub

pub struct SettingsContext {
    pub guild_name: String,
    pub player_ids: i64,
    pub player_limit: Option<i64>,
    pub music_configured: bool,
    pub lookup_configured: bool,
    pub gift_sync: Option<i64>,
    pub oracle_ready: bool,
    pub uptime: String,
    pub latency_ms: u64,
    pub is_owner: bool,
}
// Extend with new config panels added this migration.
#[derive(Debug, Clone, Default)]
pub struct LevelingView {
    pub enabled: bool,
    pub xp_min: i64,
    pub xp_max: i64,
    pub cooldown_sec: i64,
    pub voice_xp: i64,
    pub announce_mode: String,
    pub stack_rewards: bool,
    pub level: i64,
    pub total_ranked: i64,
}

pub fn settings_hub(ctx: &SettingsContext) -> (CreateEmbed, Vec<CreateActionRow>) {
    let limit = ctx
        .player_limit
        .map(|v| v.to_string())
        .unwrap_or_else(|| "∞".to_string());

    let mut embed = embeds::brand(format!("{} Settings", icons::SETTINGS))
        .color(colors::BRAND)
        .description(format!(
            "Welcome to the **{}** control centre.\nPick a category below — every panel edits live and saves instantly.",
            ctx.guild_name
        ))
        .field(
            format!("{} Overview", icons::DATABASE),
            embeds::rows(&[
                (
                    "Players".to_string(),
                    format!("`{}` tracked / `{}`", ctx.player_ids, limit),
                ),
                (
                    "Music".to_string(),
                    if ctx.music_configured { "`ready`".into() } else { "`not set up`".into() },
                ),
                (
                    "Lookup".to_string(),
                    if ctx.lookup_configured { "`live`".into() } else { "`not deployed`".into() },
                ),
                (
                    "Gift codes".to_string(),
                    match ctx.gift_sync {
                        Some(ts) => format!("synced {}", embeds::db_relative(ts)),
                        None => "`waiting for sync`".into(),
                    },
                ),
                (
                    "Oracle".to_string(),
                    if ctx.oracle_ready { "`connected`".into() } else { "`no credentials`".into() },
                ),
            ]),
            false,
        )
        .field(
            format!("{} System", icons::BOLT),
            embeds::rows(&[
                ("Uptime".to_string(), format!("`{}`", ctx.uptime)),
                ("Gateway".to_string(), format!("`{} ms`", ctx.latency_ms)),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let wide = settings_ctx_bool(ctx);
    embed = embed.field(
        format!("{} Categories", icons::LIST),
        if wide {
            "**Players** · **Music** · **Lookup** · **Gift Codes** · **Oracle** · **Admin** · **Sneak Peek** · **WOS Relay** · **Notifications** · **Index** · **Roles** · **Calculators**"
        } else {
            "**Players** · **Music** · **Lookup** · **Gift Codes** · **Oracle** · **Sneak Peek** · **WOS Relay** · **Notifications** · **Index** · **Roles** · **Calculators** · **Forms**"
        }
        .to_string(),
        false,
    );

    let mut buttons = vec![
        components::primary("settings:players", format!("{} Players", icons::PLAYER)),
        components::primary("settings:music", format!("{} Music", icons::MUSIC)),
        components::primary("settings:lookup", format!("{} Lookup", icons::LOOKUP)),
        components::primary("settings:gift", format!("{} Gift Codes", icons::GIFT)),
        components::primary("settings:oracle", format!("{} Oracle", icons::ORACLE)),
    ];
    buttons.push(components::secondary(
        "settings:leveling",
        format!("{} Leveling", icons::BOLT),
    ));
    buttons.push(components::secondary(
        "settings:welcome",
        format!("{} Welcome", icons::BELL),
    ));
    buttons.push(components::secondary(
        "settings:tempvoice",
        format!("{} Temp Voice", icons::BOLT),
    ));
    buttons.push(components::secondary(
        "settings:tickets",
        format!("{} Tickets", icons::LOCK),
    ));
    buttons.push(components::secondary(
        "settings:voting",
        format!("{} Voting", icons::SWORD),
    ));
    buttons.push(components::secondary(
        "settings:birthday",
        format!("{} Birthday", icons::GIFT),
    ));
    buttons.push(components::secondary(
        "settings:giveaways",
        format!("{} Giveaways", icons::GIFT),
    ));
    buttons.push(components::secondary(
        "settings:logging",
        format!("{} Logging", icons::DATABASE),
    ));
    buttons.push(components::secondary(
        "settings:cleanup",
        format!("{} Cleanup", icons::TRASH),
    ));
    buttons.push(components::secondary(
        "settings:sneakpeek",
        format!("{} Sneak Peek", icons::SNOWFLAKE),
    ));
    buttons.push(components::secondary(
        "settings:wos",
        format!("{} WOS Relay", icons::MEGAPHONE),
    ));
    buttons.push(components::secondary(
        "settings:notifications",
        format!("{} Notifications", icons::BELL),
    ));
    buttons.push(components::secondary(
        "settings:index",
        format!("{} Server Index", icons::SERVER),
    ));
    buttons.push(components::secondary(
        "settings:roles",
        format!("{} Role Assignment", icons::TAG),
    ));
    buttons.push(components::secondary(
        "settings:calculator",
        format!("{} Calculators", icons::BOLT),
    ));
    buttons.push(components::secondary(
        "settings:ministry",
        format!("{} Forms", icons::LIST),
    ));
    buttons.push(components::secondary(
        "settings:branding",
        format!("{} Branding", icons::TAG),
    ));
    if ctx.is_owner {
        buttons.push(components::danger(
            "settings:owner",
            format!("{} Owner Panel", icons::CROWN),
        ));
    }
    if wide {
        buttons.push(components::danger(
            "settings:admin",
            format!("{} Admin", icons::ADMIN),
        ));
    }
    // Discord allows at most 5 rows x 5 buttons = 25 per message. The owner sees two
    // extra buttons, so "Close" is dropped when there is no room (ephemeral messages
    // can be dismissed anyway).
    buttons.push(secondary("help:home", "Help"));
    if buttons.len() < 25 {
        buttons.push(secondary("settings:close", "Close"));
    }
    (embed, components::rows(buttons))
}

fn settings_ctx_bool(_ctx: &SettingsContext) -> bool {
    true
}

// ------------------------------------------------------------------- music

/// Welcome/autorole/join-DM configuration panel.
#[derive(Debug, Clone, Default)]
pub struct WelcomeView {
    pub channel: Option<u64>,
    pub avatar_mode: String,
    pub autorole_enabled: bool,
    pub member_roles: usize,
    pub join_dm: bool,
    pub reminder_interval: u64,
    pub member_count: u64,
}

pub fn welcome_panel(view: &WelcomeView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Welcome", icons::BELL))
        .color(colors::BRAND)
        .description("Greet new members, grant auto-roles and send a registration DM.")
        .field(
            "Welcome message",
            embeds::rows(&[
                (
                    "Channel".into(),
                    view.channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`off`".into()),
                ),
                ("Avatar".into(), format!("`{}`", view.avatar_mode)),
                ("Server members".into(), format!("`{}`", view.member_count)),
            ]),
            true,
        )
        .field(
            "Auto-role",
            embeds::rows(&[
                (
                    "Enabled".into(),
                    if view.autorole_enabled {
                        "`yes`".into()
                    } else {
                        "`no`".into()
                    },
                ),
                ("Member roles".into(), format!("`{}`", view.member_roles)),
            ]),
            true,
        )
        .field(
            "Join DM",
            embeds::rows(&[
                (
                    "Enabled".into(),
                    if view.join_dm {
                        "`yes`".into()
                    } else {
                        "`no`".into()
                    },
                ),
                (
                    "Reminder interval".into(),
                    format!("`{}d`", view.reminder_interval),
                ),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:welcome:channel",
            format!("{} Channel", icons::EDIT),
        ),
        components::secondary("settings:welcome:title", format!("{} Title", icons::EDIT)),
        components::secondary(
            "settings:welcome:description",
            format!("{} Description", icons::EDIT),
        ),
        components::secondary(
            "settings:welcome:avatar",
            format!("{} Avatar", icons::PLAYER),
        ),
        components::secondary(
            "settings:welcome:autorole",
            format!(
                "{} Auto-role {}",
                icons::BOLT,
                if view.autorole_enabled { "off" } else { "on" }
            ),
        ),
        components::secondary(
            "settings:welcome:joindm",
            format!("{} Join DM", icons::BELL),
        ),
        components::secondary(
            "settings:welcome:preview",
            format!("{} Preview", icons::LINK),
        ),
    ];
    (embed, components::rows(buttons))
}

/// Logging channel + category toggles panel.
#[derive(Debug, Clone)]
pub struct LoggingView {
    pub channel: Option<u64>,
    pub disabled: Vec<String>,
}

pub fn logging_panel(view: &LoggingView) -> (CreateEmbed, Vec<CreateActionRow>) {
    const CATEGORIES: &[&str] = &[
        "giftcode", "players", "members", "messages", "voice", "channels", "roles", "server",
    ];
    let body = CATEGORIES
        .iter()
        .map(|category| {
            let state = if view.disabled.iter().any(|c| c == category) {
                "off"
            } else {
                "on"
            };
            format!("`{:<9}` — **{}**", category, state)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let embed = embeds::brand(format!("{} Logging", icons::DATABASE))
        .color(colors::BRAND_DEEP)
        .description("One channel for gift-code redemptions, player-IDs and server events.")
        .field(
            "Log channel",
            view.channel
                .map(|id| format!("<#{id}>"))
                .unwrap_or_else(|| "`not set`".into()),
            false,
        )
        .field("Categories", format!("```{}```", body), false)
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:logging:channel",
            format!("{} Channel", icons::EDIT),
        ),
        components::secondary(
            "settings:logging:categories",
            format!("{} Categories", icons::LIST),
        ),
        components::secondary("settings:logging:test", format!("{} Test", icons::BOLT)),
    ];
    (embed, components::rows(buttons))
}

/// Temp voice lobby panel.
#[derive(Debug, Clone, Default)]
pub struct TempVoiceView {
    pub enabled: bool,
    pub lobby: Option<u64>,
    pub category: Option<u64>,
    pub template: String,
    pub user_limit: u32,
    pub active: i64,
}

pub fn tempvoice_panel(view: &TempVoiceView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Temp Voice", icons::BOLT))
        .color(colors::TEAL)
        .description("Members join the lobby and get their own voice channel.")
        .field(
            "Configuration",
            embeds::rows(&[
                (
                    "Enabled".into(),
                    if view.enabled {
                        "`yes`".into()
                    } else {
                        "`no`".into()
                    },
                ),
                (
                    "Lobby".into(),
                    view.lobby
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`not set`".into()),
                ),
                (
                    "Category".into(),
                    view.category
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`lobby default`".into()),
                ),
                ("Template".into(), format!("`{}`", view.template)),
                (
                    "Limit".into(),
                    format!(
                        "`{}`",
                        if view.user_limit == 0 {
                            "unlimited".to_string()
                        } else {
                            view.user_limit.to_string()
                        }
                    ),
                ),
                ("Active".into(), format!("`{}`", view.active)),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:tempvoice:toggle",
            format!(
                "{} {}",
                if view.enabled { "Disable" } else { "Enable" },
                icons::BOLT
            ),
        ),
        components::secondary("settings:tempvoice:lobby", format!("{} Lobby", icons::EDIT)),
        components::secondary(
            "settings:tempvoice:category",
            format!("{} Category", icons::EDIT),
        ),
        components::secondary(
            "settings:tempvoice:template",
            format!("{} Template", icons::EDIT),
        ),
        components::secondary("settings:tempvoice:limit", format!("{} Limit", icons::EDIT)),
    ];
    (embed, components::rows(buttons))
}

pub fn voting_panel(view: &VotingPanel) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} WoS Voting", icons::SWORD))
        .color(colors::VIOLET)
        .description("Multi-question votes for ministry elections and similar.")
        .field(
            "Channels",
            embeds::rows(&[
                (
                    "Vote channel".into(),
                    view.voting_channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`not set`".into()),
                ),
                (
                    "Results channel".into(),
                    view.results_channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`same channel`".into()),
                ),
                ("Active votes".into(), format!("`{}`", view.active)),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:voting_create",
            format!("{} Create vote", icons::ADD),
        ),
        components::primary(
            "settings:voting:channel",
            format!("{} Vote channel", icons::EDIT),
        ),
        components::secondary(
            "settings:voting:results",
            format!("{} Results channel", icons::EDIT),
        ),
    ];
    (embed, components::rows(buttons))
}

#[derive(Debug, Clone, Default)]
pub struct VotingPanel {
    pub voting_channel: Option<u64>,
    pub results_channel: Option<u64>,
    pub active: i64,
}

/// Birthday announcements panel.
#[derive(Debug, Clone, Default)]
pub struct BirthdayView {
    pub channel: Option<u64>,
    pub count: i64,
}

pub fn birthday_panel(view: &BirthdayView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Birthdays", icons::GIFT))
        .color(colors::VIOLET)
        .description("Members share their birthday and get a personalised embed.")
        .field(
            "Configuration",
            embeds::rows(&[
                (
                    "Channel".into(),
                    view.channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`off`".into()),
                ),
                ("Stored birthdays".into(), format!("`{}`", view.count)),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![components::primary(
        "settings:birthday:channel",
        format!("{} Channel", icons::EDIT),
    )];
    (embed, components::rows(buttons))
}

/// Tickets panel.
#[derive(Debug, Clone, Default)]
pub struct TicketsView {
    pub panel_channel: Option<u64>,
    pub category: Option<u64>,
    pub log_channel: Option<u64>,
    pub staff_roles: usize,
    pub max_open: i64,
    pub next_number: i64,
}

pub fn tickets_panel(view: &TicketsView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Tickets", icons::LOCK))
        .color(colors::BRAND_DEEP)
        .description("Private support channels opened from a public panel.")
        .field(
            "Panel",
            embeds::rows(&[
                (
                    "Channel".into(),
                    view.panel_channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`not deployed`".into()),
                ),
                (
                    "Category".into(),
                    view.category
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`none`".into()),
                ),
                (
                    "Log channel".into(),
                    view.log_channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`none`".into()),
                ),
            ]),
            false,
        )
        .field(
            "Rules",
            embeds::rows(&[
                ("Staff roles".into(), format!("`{}`", view.staff_roles)),
                ("Max open".into(), format!("`{}`", view.max_open)),
                ("Next number".into(), format!("`{:04}`", view.next_number)),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary("settings:tickets:panel", format!("{} Panel", icons::LIST)),
        components::secondary(
            "settings:tickets:category",
            format!("{} Category", icons::EDIT),
        ),
        components::secondary("settings:tickets:staff", format!("{} Staff", icons::PLAYER)),
        components::secondary(
            "settings:tickets:maxopen",
            format!("{} Max open", icons::EDIT),
        ),
    ];
    (embed, components::rows(buttons))
}

/// Giveaways panel.
#[derive(Debug, Clone, Default)]
pub struct GiveawayView {
    pub active: i64,
}

pub fn giveaways_panel(view: &GiveawayView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Giveaways", icons::GIFT))
        .color(colors::GOLD)
        .description("Members enter with one tap; winners are drawn automatically.")
        .field(
            "Active",
            format!("**{}** giveaway(s) running", view.active),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:giveaways:create",
            format!("{} New giveaway", icons::ADD),
        ),
        components::secondary("settings:giveaways:list", format!("{} List", icons::LIST)),
        components::secondary(
            "giveawayrules:pick",
            format!("{} Advanced rules", icons::EDIT),
        ),
    ];
    (embed, components::rows(buttons))
}

/// Message library panel.
#[derive(Debug, Clone, Default)]
pub struct MessagesView {
    pub group_count: usize,
    pub message_count: i64,
}

pub fn messages_panel(view: &MessagesView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Messages", icons::MESSAGES))
        .color(colors::TEAL)
        .description("Reusable announcement templates — saved, edited and posted from Discord.")
        .field(
            "Library",
            embeds::rows(&[
                ("Groups".into(), format!("`{}`", view.group_count)),
                ("Messages".into(), format!("`{}`", view.message_count)),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:messages:groups",
            format!("{} Groups", icons::LIST),
        ),
        components::secondary("settings:messages:create", format!("{} Save", icons::SAVE)),
        components::secondary("settings:messages:send", format!("{} Send", icons::FORWARD)),
    ];
    (embed, components::rows(buttons))
}

/// Server Index (auto-updating channel directory) panel.
#[derive(Debug, Clone, Default)]
pub struct ServerIndexView {
    pub channel: Option<u64>,
    pub enabled: bool,
    pub include_threads: bool,
    pub include_voice: bool,
    pub message_count: usize,
}

pub fn server_index_panel(view: &ServerIndexView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Server Index", icons::SERVER))
        .color(colors::BRAND)
        .description("Auto-maintained public directory of channels and forum posts.")
        .field(
            "Configuration",
            embeds::rows(&[
                (
                    "Channel".into(),
                    view.channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`not set`".into()),
                ),
                (
                    "Auto update".into(),
                    if view.enabled {
                        "`on`".into()
                    } else {
                        "`off`".into()
                    },
                ),
                (
                    "Forum threads".into(),
                    if view.include_threads {
                        "`included`".into()
                    } else {
                        "`excluded`".into()
                    },
                ),
                (
                    "Voice channels".into(),
                    if view.include_voice {
                        "`included`".into()
                    } else {
                        "`excluded`".into()
                    },
                ),
                ("Index messages".into(), format!("`{}`", view.message_count)),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary("settings:index:channel", format!("{} Channel", icons::EDIT)),
        components::secondary(
            "settings:index:toggle",
            format!(
                "{} Auto {}",
                icons::BOLT,
                if view.enabled { "OFF" } else { "ON" }
            ),
        ),
        components::secondary(
            "settings:index:threads",
            format!(
                "{} Threads {}",
                icons::LIST,
                if view.include_threads { "ON" } else { "OFF" }
            ),
        ),
        components::secondary(
            "settings:index:voice",
            format!(
                "{} Voice {}",
                icons::VOLUME,
                if view.include_voice { "ON" } else { "OFF" }
            ),
        ),
        components::primary(
            "settings:index:rebuild",
            format!("{} Rebuild now", icons::REFRESH),
        ),
    ];
    (embed, components::rows(buttons))
}

/// Calculator hub panel.
#[derive(Debug, Clone, Default)]
pub struct CalculatorView {
    pub svs_channel: Option<u64>,
    pub svs_message_id: Option<u64>,
    pub building_channel: Option<u64>,
    pub building_message_id: Option<u64>,
    pub gear_channel: Option<u64>,
    pub gear_message_id: Option<u64>,
    pub charm_channel: Option<u64>,
    pub charm_message_id: Option<u64>,
    pub pets_channel: Option<u64>,
    pub pets_message_id: Option<u64>,
    pub waracademy_channel: Option<u64>,
    pub waracademy_message_id: Option<u64>,
}

pub fn calculator_panel(view: &CalculatorView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let channel_line = |channel: Option<u64>| {
        channel
            .map(|id| format!("<#{id}>"))
            .unwrap_or_else(|| "`not set`".to_string())
    };
    let msg_line = |id: Option<u64>| {
        id.map(|id| format!("`{id}`"))
            .unwrap_or_else(|| "`not posted`".into())
    };

    let embed = embeds::brand(format!("{} Calculators", icons::BOLT))
        .color(colors::BRAND)
        .description("Persistent calculator panels members use to plan upgrades.")
        .field(
            "🏗️ Building",
            embeds::rows(&[
                ("Panel channel".into(), channel_line(view.building_channel)),
                ("Message id".into(), msg_line(view.building_message_id)),
            ]),
            true,
        )
        .field(
            "🗓️ SVS Prep Planner",
            embeds::rows(&[
                ("Panel channel".into(), channel_line(view.svs_channel)),
                ("Message id".into(), msg_line(view.svs_message_id)),
            ]),
            true,
        )
        .field(
            "🎖️ Chief Gear",
            embeds::rows(&[
                ("Panel channel".into(), channel_line(view.gear_channel)),
                ("Message id".into(), msg_line(view.gear_message_id)),
            ]),
            true,
        )
        .field(
            "🌀 Chief Charm",
            embeds::rows(&[
                ("Panel channel".into(), channel_line(view.charm_channel)),
                ("Message id".into(), msg_line(view.charm_message_id)),
            ]),
            true,
        )
        .field(
            "🐾 Pets",
            embeds::rows(&[
                ("Panel channel".into(), channel_line(view.pets_channel)),
                ("Message id".into(), msg_line(view.pets_message_id)),
            ]),
            true,
        )
        .field(
            "🏛️ War Academy",
            embeds::rows(&[
                (
                    "Panel channel".into(),
                    channel_line(view.waracademy_channel),
                ),
                ("Message id".into(), msg_line(view.waracademy_message_id)),
            ]),
            true,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:calculator:building_channel",
            format!("{} Building channel", icons::EDIT),
        ),
        components::secondary(
            "settings:calculator:building_repost",
            format!("{} Repost Building", icons::REFRESH),
        ),
        components::primary(
            "settings:calculator:svs_channel",
            format!("{} SVS channel", icons::EDIT),
        ),
        components::secondary(
            "settings:calculator:svs_repost",
            format!("{} Repost SVS", icons::REFRESH),
        ),
        components::primary(
            "settings:calculator:gear_channel",
            format!("{} Gear channel", icons::EDIT),
        ),
        components::secondary(
            "settings:calculator:gear_repost",
            format!("{} Repost Gear", icons::REFRESH),
        ),
        components::primary(
            "settings:calculator:charm_channel",
            format!("{} Charm channel", icons::EDIT),
        ),
        components::secondary(
            "settings:calculator:charm_repost",
            format!("{} Repost Charm", icons::REFRESH),
        ),
        components::primary(
            "settings:calculator:pets_channel",
            format!("{} Pets channel", icons::EDIT),
        ),
        components::secondary(
            "settings:calculator:pets_repost",
            format!("{} Repost Pets", icons::REFRESH),
        ),
        components::primary(
            "settings:calculator:waracademy_channel",
            format!("{} War Academy channel", icons::EDIT),
        ),
        components::secondary(
            "settings:calculator:waracademy_repost",
            format!("{} Repost War Academy", icons::REFRESH),
        ),
        components::secondary("settings:home", format!("{} Back", icons::BACK)),
    ];
    (embed, components::rows(buttons))
}

/// Role Assignment panel (staff-only catalog + grants).
#[derive(Debug, Clone, Default)]
pub struct RolesPanelView {
    pub channel: Option<u64>,
    pub panel_message_id: Option<u64>,
    /// `(role_id, grant_count)` lines, already truncated for the embed.
    pub catalog: Vec<(u64, usize)>,
    /// `(role_id, user_grants, role_grants)` per catalog role.
    pub grants: Vec<(u64, usize, usize)>,
}

pub fn roles_panel(view: &RolesPanelView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let catalog_body = if view.catalog.is_empty() {
        "_No roles in the catalog yet — add one below._".to_string()
    } else {
        view.catalog
            .iter()
            .map(|(role_id, grants)| {
                format!(
                    "<@&{role_id}> — `{grants}` grant{}",
                    if *grants == 1 { "" } else { "s" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let grants_body = view
        .grants
        .iter()
        .filter(|(_, users, roles)| *users > 0 || *roles > 0)
        .map(|(role_id, users, roles)| format!("<@&{role_id}> — {users} user · {roles} role(s)"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut embed = embeds::brand(format!("{} Role Assignment", icons::TAG))
        .color(colors::BRAND)
        .description("Staff-only tool for granting/removing Discord roles on members — not self-service. Anyone with perms/access can use the panel message in the channel below.")
        .field(
            "Panel",
            embeds::rows(&[
                ("Channel".into(), view.channel.map(|id| format!("<#{id}>")).unwrap_or_else(|| "`not set`".into())),
                ("Message".into(), view.panel_message_id.map(|id| format!("`{id}`")).unwrap_or_else(|| "`not posted`".into())),
            ]),
            false,
        )
        .field(format!("Catalog ({})", view.catalog.len()), catalog_body, false);
    if !grants_body.is_empty() {
        embed = embed.field("Grants", grants_body, false);
    }
    embed = embed.footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let mut rows = components::rows(vec![
        components::primary(
            "settings:roles:channel",
            format!("{} Panel Channel", icons::EDIT),
        ),
        components::secondary("settings:roles:add", format!("{} Add role", icons::ADD)),
        components::danger(
            "settings:roles:remove",
            format!("{} Remove role", icons::REMOVE),
        ),
        components::secondary(
            "settings:roles:refresh",
            format!("{} Refresh", icons::REFRESH),
        ),
    ]);
    rows.extend(components::rows(vec![
        components::secondary(
            "settings:roles:grant_user",
            format!("{} Grant user", icons::PLAYER),
        ),
        components::secondary(
            "settings:roles:grant_role",
            format!("{} Grant role", icons::LOCK),
        ),
        components::secondary(
            "settings:roles:revoke_user",
            format!("{} Revoke user", icons::REMOVE),
        ),
        components::secondary(
            "settings:roles:revoke_role",
            format!("{} Revoke role", icons::REMOVE),
        ),
    ]));
    (embed, rows)
}

/// Notification (Bear Trap style scheduled reminders) panel.
#[derive(Debug, Clone, Default)]
pub struct NotificationsView {
    /// Pre-rendered lines (description, enabled state), one per notification.
    pub lines: Vec<String>,
    pub default_slots: i64,
    pub total: i64,
}

pub fn notifications_panel(view: &NotificationsView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let body = if view.lines.is_empty() {
        "_No notifications yet — tap **Add** to schedule one._".to_string()
    } else {
        view.lines.join("\n")
    };
    let slot_label = match view.default_slots {
        1 => "30/10/5/0",
        2 => "10/5/0",
        3 => "5/0",
        4 => "5",
        _ => "0 (at event)",
    };

    let embed = embeds::brand(format!("{} Notifications", icons::BELL))
        .color(colors::BRAND)
        .description("Scheduled reminders for game events (Bear Trap & co).")
        .field(format!("Scheduled ({})", view.total), body, false)
        .field("Default slots", format!("`{slot_label} min before`"), true)
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary("settings:notifications:add", format!("{} Add", icons::ADD)),
        components::secondary(
            "settings:notifications:slots",
            format!("{} Slots", icons::CLOCK),
        ),
        components::secondary(
            "settings:notifications:toggle",
            format!("{} Toggle", icons::BOLT),
        ),
        components::danger(
            "settings:notifications:delete",
            format!("{} Delete", icons::TRASH),
        ),
    ];
    (embed, components::rows(buttons))
}

/// WOS Announcements relay panel.
#[derive(Debug, Clone, Default)]
pub struct WosRelayView {
    pub destination: Option<u64>,
    pub sneakpeek: Option<u64>,
    pub hub_channel: Option<u64>,
    pub hub_configured: bool,
    pub is_owner: bool,
}

pub fn wos_relay_panel(view: &WosRelayView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let hub_line = if view.hub_configured {
        match view.hub_channel {
            Some(id) => format!("<#{id}>"),
            None => "configured (in another server)".to_string(),
        }
    } else {
        "not set up yet".to_string()
    };

    let mut lines = vec![
        "Relays the official Whiteout Survival server's announcements into this server, with an on-demand Translate button on each one.".to_string(),
        String::new(),
        format!("**This server's destination** — where relayed posts appear here (currently {})", view.destination.map(|id| format!("<#{id}>")).unwrap_or_else(|| "not set".to_string())),
        format!("**Sneak Peek channel** — auto-posts Chief Concierge articles (currently {})", view.sneakpeek.map(|id| format!("<#{id}>")).unwrap_or_else(|| "not set".to_string())),
    ];
    if view.is_owner {
        lines.push(format!(
            "**Hub channel** (owner-only, shared across every server) — {hub_line}"
        ));
        lines.push(String::new());
        lines.push("**One-time hub setup (once, ever):**".to_string());
        lines.push("1. Pick a server only you manage, and add this bot to it.".to_string());
        lines.push("2. Open this menu there and hit **Set as Hub**.".to_string());
        lines.push("3. Join the official WOS Discord server as yourself.".to_string());
        lines.push(
            "4. Open a WOS 📣 channel → **Follow Channel** → pick the hub channel.".to_string(),
        );
        lines.push(
            "5. Repeat for any other WOS 📣 Announcement channel you want relayed.".to_string(),
        );
    } else if !view.hub_configured {
        lines.push(String::new());
        lines.push(
            "The bot owner hasn't set up the shared hub channel yet — ask them to.".to_string(),
        );
    }
    lines.push(String::new());
    lines.push("Every server that sets a destination channel above gets a copy automatically once the hub is live.".to_string());

    let embed = embeds::brand(format!("{} WOS Announcements", icons::MEGAPHONE))
        .color(colors::BRAND)
        .description(lines.join("\n"))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let mut buttons = vec![components::primary(
        "settings:wos:destination",
        format!("{} Destination", icons::EDIT),
    )];
    if view.destination.is_some() {
        buttons.push(components::danger(
            "settings:wos:clear",
            format!("{} Clear", icons::TRASH),
        ));
    }
    buttons.push(components::secondary(
        "settings:sneakpeek",
        format!("{} Sneak Peek", icons::SNOWFLAKE),
    ));
    if view.is_owner {
        buttons.push(components::primary(
            "settings:wos:hub",
            format!("{} Set as Hub", icons::SERVER),
        ));
        if view.hub_configured {
            buttons.push(components::danger(
                "settings:wos:clearhub",
                format!("{} Clear Hub", icons::TRASH),
            ));
        }
    }

    (embed, components::rows(buttons))
}

/// Sneak Peek auto-detect panel.
#[derive(Debug, Clone, Default)]
pub struct SneakPeekView {
    pub channel: Option<u64>,
    pub last_article: Option<String>,
    pub last_checked: Option<String>,
}

pub fn sneakpeek_panel(view: &SneakPeekView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Sneak Peek", icons::SNOWFLAKE))
        .color(colors::TEAL)
        .description(
            "Auto-post new Chief Concierge: Sneak Peek articles from whiteoutsurvival.wiki.",
        )
        .field(
            "Configuration",
            embeds::rows(&[
                (
                    "Channel".into(),
                    view.channel
                        .map(|id| format!("<#{id}>"))
                        .unwrap_or_else(|| "`off`".into()),
                ),
                (
                    "Last article".into(),
                    view.last_article
                        .clone()
                        .unwrap_or_else(|| "`none yet`".into()),
                ),
                (
                    "Last check".into(),
                    view.last_checked
                        .clone()
                        .unwrap_or_else(|| "`never`".into()),
                ),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:sneakpeek:channel",
            format!("{} Channel", icons::EDIT),
        ),
        components::secondary(
            "settings:sneakpeek:check",
            format!("{} Check now", icons::REFRESH),
        ),
        components::secondary(
            "settings:sneakpeek:sendlink",
            format!("{} Send Link", icons::LINK),
        ),
    ];
    (embed, components::rows(buttons))
}

/// Cleanup (bulk delete) panel.
#[derive(Debug, Clone, Default)]
pub struct CleanupView;

pub fn cleanup_panel() -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Cleanup", icons::TRASH))
        .color(colors::MUTED)
        .description("Bulk-delete recent messages in this channel, with optional filters.")
        .field(
            "How to",
            "Tap **Clear** and type how many messages to scan (1–100).",
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![components::danger(
        "settings:cleanup:clear",
        format!("{} Clear", icons::TRASH),
    )];
    (embed, components::rows(buttons))
}

pub fn leveling_panel(view: &LevelingView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Leveling", icons::BOLT))
        .color(colors::VIOLET)
        .description("Reward activity with XP and levels. Configure the engine below.")
        .field(
            "XP engine",
            embeds::rows(&[
                (
                    "Enabled".into(),
                    if view.enabled {
                        "`yes`".into()
                    } else {
                        "`no`".into()
                    },
                ),
                (
                    "XP per message".into(),
                    format!("`{}–{}`", view.xp_min, view.xp_max),
                ),
                ("Cooldown".into(), format!("`{}s`", view.cooldown_sec)),
                ("Voice XP / min".into(), format!("`{}`", view.voice_xp)),
            ]),
            false,
        )
        .field(
            "Announcement",
            embeds::rows(&[
                ("Mode".into(), format!("`{}`", view.announce_mode)),
                (
                    "Stack rewards".into(),
                    if view.stack_rewards {
                        "`on`".into()
                    } else {
                        "`off`".into()
                    },
                ),
            ]),
            true,
        )
        .field(
            "This server",
            embeds::rows(&[
                ("Your level".into(), format!("`{}`", view.level)),
                ("Members ranked".into(), format!("`{}`", view.total_ranked)),
            ]),
            true,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let buttons = vec![
        components::primary(
            "settings:leveling:toggle",
            format!(
                "{} {}",
                if view.enabled { "Disable" } else { "Enable" },
                icons::BOLT
            ),
        ),
        components::secondary("settings:leveling:xp", format!("{} XP range", icons::EDIT)),
        components::secondary(
            "settings:leveling:cooldown",
            format!("{} Cooldown", icons::CLOCK),
        ),
        components::secondary(
            "settings:leveling:voice",
            format!("{} Voice XP", icons::VOLUME),
        ),
        components::secondary(
            "settings:leveling:announce",
            format!("{} Announce", icons::BELL),
        ),
        components::secondary("settings:leveling:stack", format!("{} Stack", icons::MEDAL)),
        components::secondary(
            "settings:leveling:message",
            format!("{} Message", icons::EDIT),
        ),
        components::secondary(
            "settings:leveling:rewards",
            format!("{} Rewards", icons::CROWN),
        ),
        components::secondary(
            "settings:leveling:reset",
            format!("{} Reset XP", icons::TRASH),
        ),
    ];
    (embed, components::rows(buttons))
}

/// Fancy player card — this is the panel the legacy bot posted in the music
/// channel, rebuilt with a progress bar and cleaner controls.
pub fn music(view: &NowPlayingView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let mut embed = embeds::brand(format!("{} Music", icons::MUSIC)).color(colors::BRAND_DEEP);

    if view.title.is_empty() {
        embed = embed.description(format!(
            "Nothing is playing.\n\n{}",
            "Use the Music panel's play button, or drop a link in this channel to queue something."
        ));
    } else {
        let mut desc = String::new();
        if let Some(uri) = view.uri.as_deref() {
            desc.push_str(&format!("**[{}]({})**\n", view.title, uri));
        } else {
            desc.push_str(&format!("**{}**\n", view.title));
        }
        if !view.author.is_empty() {
            desc.push_str(&format!("by `{}`\n", view.author));
        }
        desc.push_str(&format!(
            "\n{}\n`{}` {} `{}`\n{}",
            theme::DIVIDER,
            theme::duration(view.position_ms),
            theme::progress_bar(view.position_ms, view.length_ms, 18),
            theme::duration(view.length_ms),
            theme::DIVIDER,
        ));
        embed = embed.description(desc).field(
            "Status",
            embeds::rows(&[
                (
                    "Playback".to_string(),
                    if view.paused {
                        "`⏸ paused`".into()
                    } else {
                        "`▶ playing`".into()
                    },
                ),
                ("Volume".to_string(), format!("`{}%`", view.volume)),
                ("Loop".to_string(), format!("`{}`", view.loop_label)),
                (
                    "Queue".to_string(),
                    format!("`{} track(s)`", view.queue_len),
                ),
                (
                    "Voice".to_string(),
                    view.connected_channel
                        .clone()
                        .unwrap_or_else(|| "`not connected`".into()),
                ),
            ]),
            true,
        );
        if !view.requester.is_empty() {
            embed = embed.field("Requested by", view.requester.clone(), true);
        }
        if !view.upcoming.is_empty() {
            let list = view
                .upcoming
                .iter()
                .take(5)
                .enumerate()
                .map(|(i, (name, len))| {
                    format!("`{}.` {} — `{}`", i + 1, error_trunc(name, 55), len)
                })
                .collect::<Vec<_>>()
                .join("\n");
            embed = embed.field(format!("{} Up next", icons::QUEUE), list, false);
        }
    }

    if let Some(url) = view.uri.clone() {
        embed = embed.thumbnail(url);
    }

    let play_pause = if view.paused {
        components::success("music:resume", format!("{} Resume", icons::PLAY))
    } else {
        components::secondary("music:pause", format!("{} Pause", icons::PAUSE))
    };

    let row1 = components::row(vec![
        components::secondary("music:previous", icons::PREVIOUS),
        play_pause,
        components::secondary("music:skip", format!("{} Skip", icons::SKIP)),
        components::danger("music:stop", format!("{} Stop", icons::STOP)),
    ]);
    let row2 = components::row(vec![
        secondary("music:shuffle", format!("{} Shuffle", icons::SHUFFLE)),
        secondary(
            "music:loop",
            format!("{} Loop · {}", icons::REPEAT, view.loop_label),
        ),
        secondary("music:voldown", format!("{} Vol -", icons::VOLUME)),
        secondary("music:volup", format!("{} Vol +", icons::VOLUME)),
        secondary("music:queue", format!("{} Queue", icons::QUEUE)),
    ]);

    (embed, vec![row1, row2])
}

fn error_trunc(input: &str, limit: usize) -> String {
    crate::error::truncate(input, limit)
}

/// The compact "now playing" line used when a track starts.
pub fn now_playing(view: &NowPlayingView) -> CreateEmbed {
    let mut embed = embeds::base(colors::SUCCESS)
        .author(serenity::CreateEmbedAuthor::new(format!(
            "{} Now playing",
            icons::PLAY
        )))
        .title(error_trunc(&view.title, 200))
        .description(format!(
            "`{}` · requested by {}\n{}",
            view.author,
            if view.requester.is_empty() {
                "someone".to_string()
            } else {
                view.requester.clone()
            },
            theme::duration(view.length_ms)
        ))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));
    if let Some(uri) = view.uri.clone() {
        embed = embed.url(uri);
    }
    embed
}

// ------------------------------------------------------------------- lookup

pub fn lookup_player(view: &LookupPlayerView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let title = view.game_name.clone().unwrap_or_else(|| view.fid.clone());
    let mut embed = embeds::brand(format!("{} Player lookup", icons::PLAYER))
        .color(colors::INFO)
        .title(format!("{} {}", icons::PLAYER, error_trunc(&title, 120)))
        .field("FID", format!("`{}`", view.fid), true)
        .field(
            "State",
            view.state
                .map(|s| format!("`#{s}`"))
                .unwrap_or_else(|| "`unknown`".into()),
            true,
        )
        .field(
            format!("{} Alliance", icons::ALLIANCE),
            view.alliance.clone().unwrap_or_else(|| "`none`".into()),
            true,
        )
        .field(
            format!("{} Power", icons::BOLT),
            view.power
                .map(|v| format!("`{}`", theme::compact_number(v)))
                .unwrap_or_else(|| "`—`".into()),
            true,
        )
        .field(
            format!("{} Kills", icons::SWORD),
            view.kills
                .map(|v| format!("`{}`", theme::compact_number(v)))
                .unwrap_or_else(|| "`—`".into()),
            true,
        )
        .field(
            format!("{} Town Hall", icons::TOWN),
            view.town_hall
                .map(|v| format!("`{v}`"))
                .unwrap_or_else(|| "`—`".into()),
            true,
        )
        .field(
            format!("{} Life Tree", icons::TREE),
            view.life_tree
                .map(|v| format!("`{v}`"))
                .unwrap_or_else(|| "`—`".into()),
            true,
        )
        .field(
            format!("{} Last sync", icons::CLOCK),
            view.synced
                .map(embeds::db_relative)
                .unwrap_or_else(|| "`never`".into()),
            true,
        );

    if let Some(url) = view.avatar_url.clone() {
        embed = embed.thumbnail(url);
    }

    let rows = vec![components::row(vec![
        components::primary(
            "lookup:player_search",
            format!("{} Search again", icons::LOOKUP),
        ),
        secondary("lookup:refresh", format!("{} Refresh", icons::REFRESH)),
    ])];
    (embed, rows)
}

pub fn lookup_alliance(view: &LookupAllianceView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let name = view.name.clone().unwrap_or_else(|| view.abbr.clone());
    let mut embed = embeds::brand(format!("{} Alliance lookup", icons::ALLIANCE))
        .color(colors::VIOLET)
        .title(format!(
            "{} {}",
            icons::ALLIANCE,
            error_trunc(&format!("[{}] {name}", view.abbr), 120)
        ))
        .field("Members tracked", format!("`{}`", view.members), true)
        .field(
            format!("{} Total power", icons::BOLT),
            format!("`{}`", theme::compact_number(view.power)),
            true,
        )
        .field(
            format!("{} Total kills", icons::SWORD),
            format!("`{}`", theme::compact_number(view.kills)),
            true,
        )
        .field(
            format!("{} Last sync", icons::CLOCK),
            view.synced
                .map(embeds::db_relative)
                .unwrap_or_else(|| "`never`".into()),
            true,
        );

    if !view.top.is_empty() {
        let leaderboard = view
            .top
            .iter()
            .take(10)
            .enumerate()
            .map(|(i, (name, power))| {
                let medal = match i {
                    0 => icons::CROWN,
                    1 => icons::MEDAL,
                    2 => icons::BOLT,
                    _ => "▫️",
                };
                format!(
                    "{medal} **{}** — `{}`",
                    error_trunc(name, 40),
                    theme::compact_number(*power)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        embed = embed.field("Top members", leaderboard, false);
    }

    let rows = vec![components::row(vec![
        components::primary(
            "lookup:alliance_search",
            format!("{} Search", icons::LOOKUP),
        ),
        secondary("lookup:refresh", format!("{} Refresh", icons::REFRESH)),
    ])];
    (embed, rows)
}

pub fn lookup_state(view: &StateView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let mut embed = embeds::brand(format!("{} State overview", icons::STATE))
        .color(colors::TEAL)
        .title(format!("{} State #{}", icons::STATE, view.state))
        .field("Alliances", format!("`{}`", view.alliances), true)
        .field("Players", format!("`{}`", view.players), true)
        .field(
            format!("{} State power", icons::BOLT),
            format!("`{}`", theme::compact_number(view.power)),
            true,
        );

    if !view.top_alliances.is_empty() {
        let lines = view
            .top_alliances
            .iter()
            .take(12)
            .enumerate()
            .map(|(i, (abbr, power, members))| {
                format!(
                    "`{:>2}.` **{}** — `{}` power · `{}` members",
                    i + 1,
                    error_trunc(abbr, 8),
                    theme::compact_number(*power),
                    members
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        embed = embed.field("Top alliances", lines, false);
    }

    let rows = vec![components::row(vec![
        components::primary(
            "lookup:state_search",
            format!("{} Change state", icons::LOOKUP),
        ),
        secondary("lookup:refresh", format!("{} Refresh", icons::REFRESH)),
    ])];
    (embed, rows)
}

/// Public search panel — deployed to a channel from /settings → Lookup.
pub fn lookup_public_panel() -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} WoS Lookup", icons::LOOKUP))
        .color(colors::INFO)
        .description(format!(
            "Search tracked players, alliances and states.\n\n{}",
            theme::DIVIDER
        ))
        .field(
            format!("{} Player", icons::PLAYER),
            "Look up a player by FID or in-game name.",
            true,
        )
        .field(
            format!("{} Alliance", icons::ALLIANCE),
            "See every tracked member of an alliance.",
            true,
        )
        .field(
            format!("{} State", icons::STATE),
            "Rank alliances inside a state.",
            true,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let rows = vec![
        components::row(vec![
            components::primary("lookup:player_search", format!("{} Player", icons::PLAYER)),
            components::primary(
                "lookup:alliance_search",
                format!("{} Alliance", icons::ALLIANCE),
            ),
            components::primary("lookup:state_search", format!("{} State", icons::STATE)),
        ]),
        components::row(vec![
            secondary(
                "lookup:leaderboard",
                format!("{} Power board", icons::CROWN),
            ),
            secondary("lookup:refresh", format!("{} Refresh", icons::REFRESH)),
        ]),
    ];
    (embed, rows)
}

// ------------------------------------------------------------------ oracle

pub struct OracleStatus {
    pub has_credentials: bool,
    pub js_auth_preview: Option<String>,
    pub updated: Option<i64>,
    pub updated_by: Option<String>,
    pub last_test: Option<i64>,
    pub last_test_ok: Option<bool>,
    pub last_test_message: Option<String>,
    pub codes_sync: Option<i64>,
    pub lookup_sync: Option<i64>,
    pub total_codes: i64,
    pub browser_enabled: bool,
}

pub fn oracle_panel(status: &OracleStatus) -> (CreateEmbed, Vec<CreateActionRow>) {
    let mut embed = embeds::brand(format!("{} Oracle", icons::ORACLE)).color(colors::VIOLET);

    if !status.has_credentials {
        embed = embed.description(format!(
            "No Oracle session is stored yet.\n\n{}",
            "You can capture one automatically with the headless browser, or paste a `cURL` command copied from Chrome DevTools."
        ));
    } else {
        embed = embed.description(format!(
            "Session stored {}{}.\n{}",
            status
                .updated
                .map(embeds::db_relative)
                .unwrap_or_else(|| "recently".into()),
            status
                .updated_by
                .as_deref()
                .map(|u| format!(" by <@{u}>"))
                .unwrap_or_default(),
            theme::DIVIDER
        ));
        embed = embed.field(
            "Session",
            embeds::rows(&[
                (
                    "js_auth".to_string(),
                    format!(
                        "`{}`",
                        status
                            .js_auth_preview
                            .clone()
                            .unwrap_or_else(|| "••••".into())
                    ),
                ),
                (
                    "Last test".to_string(),
                    match (status.last_test, status.last_test_ok) {
                        (Some(ts), Some(true)) => format!("{} `passed`", embeds::db_relative(ts)),
                        (Some(ts), Some(false)) => format!("{} `failed`", embeds::db_relative(ts)),
                        (Some(ts), None) => format!("{} `unknown`", embeds::db_relative(ts)),
                        (None, _) => "`never`".to_string(),
                    },
                ),
            ]),
            false,
        );
        if let Some(msg) = status.last_test_message.as_deref() {
            embed = embed.field(
                "Last message",
                format!("`{}`", crate::error::truncate(msg, 300)),
                false,
            );
        }
        embed = embed.field(
            format!("{} Sync", icons::REFRESH),
            embeds::rows(&[
                (
                    "Gift codes".to_string(),
                    status
                        .codes_sync
                        .map(embeds::db_relative)
                        .unwrap_or_else(|| "`never`".into()),
                ),
                (
                    "Codes stored".to_string(),
                    format!("`{}`", status.total_codes),
                ),
                (
                    "Player enrichment".to_string(),
                    status
                        .lookup_sync
                        .map(embeds::db_relative)
                        .unwrap_or_else(|| "`never`".into()),
                ),
            ]),
            false,
        );
    }

    if !status.browser_enabled {
        embed = embed.footer(serenity::CreateEmbedFooter::new(format!(
            "{} • browser capture compiled out",
            theme::footer()
        )));
    }

    let mut buttons = vec![
        components::success(
            "oracle:capture",
            format!("{} Browser capture", icons::ORACLE),
        ),
        components::primary("oracle:paste", format!("{} Paste cURL", icons::LIST)),
        secondary("oracle:test", format!("{} Test session", icons::CHECK)),
        secondary("oracle:sync", format!("{} Sync now", icons::REFRESH)),
    ];
    if status.has_credentials {
        buttons.push(components::danger(
            "oracle:clear",
            format!("{} Forget", icons::TRASH),
        ));
    }
    (embed, components::rows(buttons))
}

// --------------------------------------------------------------- gift codes

pub struct GiftCodeView {
    pub code: String,
    pub status: String,
    pub source: String,
    pub added: i64,
    pub vip: bool,
}

pub fn giftcode_panel(
    stats: &crate::db::giftcodes::GiftCodeStats,
    recent: &[GiftCodeView],
    sync: Option<i64>,
    auto_sync: bool,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let mut embed = embeds::brand(format!("{} Gift codes", icons::GIFT))
        .color(colors::GOLD)
        .description(format!(
            "Community gift codes, synced from Oracle and added by hand.\n{}",
            theme::DIVIDER
        ))
        .field(
            format!("{} Numbers", icons::DATABASE),
            embeds::rows(&[
                ("Total".to_string(), format!("`{}`", stats.total)),
                ("Active".to_string(), format!("`{}`", stats.active)),
                ("VIP".to_string(), format!("`{}`", stats.vip)),
                ("Last 24h".to_string(), format!("`{}`", stats.today)),
            ]),
            true,
        )
        .field(
            format!("{} Sync", icons::REFRESH),
            embeds::rows(&[
                (
                    "Last sync".to_string(),
                    sync.map(embeds::db_relative)
                        .unwrap_or_else(|| "`never`".into()),
                ),
                (
                    "Auto sync".to_string(),
                    if auto_sync {
                        "`on`".to_string()
                    } else {
                        "`off`".to_string()
                    },
                ),
            ]),
            true,
        );

    if recent.is_empty() {
        embed = embed.field("Recent codes", "_No codes stored yet._", false);
    } else {
        let list = recent
            .iter()
            .take(10)
            .map(|c| {
                format!(
                    "`{}` {}{} — {}",
                    c.code,
                    if c.vip {
                        format!("{} ", icons::CROWN)
                    } else {
                        String::new()
                    },
                    c.status,
                    embeds::db_relative(c.added)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        embed = embed.field("Recent codes", list, false);
    }

    let buttons = vec![
        components::primary("gift:add", format!("{} Add code", icons::ADD)),
        secondary("gift:list", format!("{} Browse", icons::LIST)),
        secondary("gift:sync", format!("{} Sync Oracle", icons::REFRESH)),
        secondary("gift:announce", format!("{} Announce", icons::BELL)),
        components::danger("gift:delete", format!("{} Delete", icons::TRASH)),
    ];
    (embed, components::rows(buttons))
}

/// The public "Open Ticket" panel (posted from /settings → Tickets).
pub fn ticket_panel(
    settings: &crate::db::tickets::TicketSettings,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let title = settings
        .panel_title
        .clone()
        .unwrap_or_else(|| "Support Tickets".to_string());
    let description = settings
        .panel_description
        .clone()
        .unwrap_or_else(|| {
            "Need help or want to talk to staff privately?\n\nPress **Open Ticket** below and tell us briefly what it is about.".to_string()
        });
    let color = settings
        .panel_color
        .map(|v| v as u32)
        .unwrap_or(colors::BRAND);

    let embed = embeds::base(color)
        .author(serenity::CreateEmbedAuthor::new(format!(
            "{} {}",
            icons::MESSAGES,
            title
        )))
        .description(description)
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let rows = vec![components::row(vec![components::success(
        "ticket:open",
        format!("{} Open Ticket", icons::ADD),
    )])];

    (embed, rows)
}

/// Announcement card posted when new codes appear.
pub fn giftcode_announce(codes: &[String]) -> CreateEmbed {
    let list = codes
        .iter()
        .map(|c| format!("{} `{}`", icons::GIFT, c))
        .collect::<Vec<_>>()
        .join("\n");
    embeds::base(colors::GOLD)
        .author(serenity::CreateEmbedAuthor::new(format!(
            "{} New gift code{}",
            icons::GIFT,
            if codes.len() == 1 { "" } else { "s" }
        )))
        .description(list)
        .footer(serenity::CreateEmbedFooter::new(theme::footer()))
}

// --------------------------------------------------------------------- help

pub fn help_panel(_prefix: &str) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Help", icons::BRAND))
        .color(colors::BRAND)
        .description(format!(
            "A single bot for Discord communities and Whiteout Survival alliances.\n{}",
            theme::DIVIDER
        ))
        .field(
            format!("{} Players", icons::PLAYER),
            "**Players** — track WoS IDs, power, alliances & state\n➡️ open /settings → **Players**".to_string(),
            false,
        )
        .field(
            format!("{} Lookup", icons::LOOKUP),
            "**Lookup panel** — search players, alliances, states & leaderboards on a public panel\n➡️ /settings → **Lookup**, or tap **Search** on the deployed panel".to_string(),
            false,
        )
        .field(
            format!("{} Music", icons::MUSIC),
            "**Music panel** — play, pause, queue, volume from buttons in the panel channel\n➡️ /settings → **Music** → **Deploy panel here**".to_string(),
            false,
        )
        .field(
            format!("{} Gift codes", icons::GIFT),
            "**Gift Codes** — sync, announce and manage WoS codes\n➡️ /settings → **Gift Codes**".to_string(),
            false,
        )
        .field(
            format!("{} Oracle", icons::ORACLE),
            "**Oracle** — connect the session used to enrich players & sync codes\n➡️ /settings → **Oracle**".to_string(),
            false,
        )
        .field(
            format!("{} Admin", icons::ADMIN),
            "**Everything else** — tickets, votes, welcome, leveling, music, temp voice, messages, giveaways, calculators… all from /settings".to_string(),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let rows = vec![components::row(vec![
        components::primary("help:players", format!("{} Players", icons::PLAYER)),
        components::primary("help:lookup", format!("{} Lookup", icons::LOOKUP)),
        components::primary("help:music", format!("{} Music", icons::MUSIC)),
        components::primary("help:gift", format!("{} Gift", icons::GIFT)),
        secondary("help:oracle", format!("{} Oracle", icons::ORACLE)),
    ])];
    (embed, rows)
}

/// Standard "list too long" pager embed.
pub fn paged_list(title: &str, page: usize, pages: usize, body: String) -> CreateEmbed {
    embeds::brand(title)
        .color(colors::BRAND_DEEP)
        .description(body)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Page {}/{} • {}",
            page + 1,
            pages.max(1),
            theme::footer()
        )))
}

#[allow(dead_code)]
fn unused_style(_: ButtonStyle) {}

// -------------------------------------------------------------- branding

#[derive(Debug, Clone, Default)]
pub struct BrandingView {
    pub contact_enabled: bool,
    pub contact_label: String,
    pub contact_url: Option<String>,
    pub can_manage: bool,
    pub is_owner: bool,
    pub nickname: Option<String>,
    pub avatar_url: Option<String>,
}

pub fn branding_panel(view: &BrandingView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let contact_status = if view.contact_enabled {
        format!(
            "🟢 **On** — [{}]({})",
            view.contact_label,
            view.contact_url.clone().unwrap_or_default()
        )
    } else {
        "🔴 **Off**".to_string()
    };
    let embed = embeds::brand(format!("{} Branding", icons::TAG))
        .color(colors::VIOLET)
        .description("Control how the bot presents itself in this server.")
        .field(
            "Contact button (all servers, owner only)",
            format!("{contact_status}\n_Shown under Messages-feature embeds._"),
            false,
        )
        .field(
            "This server",
            embeds::rows(&[
                (
                    "Nickname".into(),
                    view.nickname.clone().unwrap_or_else(|| "`default`".into()),
                ),
                (
                    "Avatar".into(),
                    view.avatar_url
                        .as_deref()
                        .map(|_| "`custom`".to_string())
                        .unwrap_or_else(|| "`default`".into()),
                ),
            ]),
            false,
        )
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let mut buttons = Vec::new();
    if view.is_owner {
        buttons.push(components::primary("branding:toggle", "✉️ Contact on/off"));
        buttons.push(components::secondary(
            "branding:contact",
            "✏️ Edit contact link",
        ));
        buttons.push(components::secondary(
            "branding:clearcontact",
            "🗑️ Clear contact",
        ));
    }
    if view.can_manage {
        buttons.push(components::secondary(
            "branding:nickname",
            "🏷️ Set bot nickname",
        ));
        buttons.push(components::secondary(
            "branding:avatar",
            "🖼️ Set bot avatar",
        ));
        buttons.push(components::secondary(
            "branding:clearnick",
            "🏷️ Clear nickname",
        ));
        buttons.push(components::secondary(
            "branding:clearavatar",
            "🖼️ Clear avatar",
        ));
    }

    let mut rows = Vec::new();
    if !buttons.is_empty() {
        rows.push(CreateActionRow::Buttons(buttons));
    }
    rows.push(CreateActionRow::Buttons(vec![components::secondary(
        "settings:home",
        format!("{} Back", icons::BACK),
    )]));
    (embed, rows)
}

// ---------------------------------------------------------------- ministry

/// Ministry settings page (ephemeral) and the persistent admin panel.
#[derive(Debug, Clone, Default)]
pub struct MinistryView {
    pub svs_number: i64,
    pub applications_open: bool,
    pub google: String,
    pub channel: Option<u64>,
    /// `None` when no SVS form exists; otherwise `(label, url, closed)`.
    pub form: Option<(String, String, bool)>,
    pub spreadsheet_url: Option<String>,
    pub total: i64,
    pub approved: i64,
    pub pending: i64,
}

fn ministry_embed(view: &MinistryView, title: &str) -> CreateEmbed {
    let app_line = if view.applications_open {
        "🟢 Applications: **Open**"
    } else {
        "🔴 Applications: **Closed**"
    };
    let form_line = match &view.form {
        Some((label, url, closed)) => {
            if *closed {
                format!("📄 **{label}** — closed")
            } else {
                format!("📄 **{label}** — [Open form]({url})")
            }
        }
        None => "📄 **No SVS form** — create one below".to_string(),
    };
    let sheet_line = view
        .spreadsheet_url
        .as_deref()
        .map(|url| format!("📑 [Reservation workspace]({url})"))
        .unwrap_or_else(|| "📑 Workspace will be created with the form".to_string());

    embeds::brand(title)
        .color(colors::BRAND)
        .description(format!(
            "{app_line}\n**SVS #{}**\n{form_line}\n{sheet_line}\n\n**Google:** {}\n**Submissions:** `{}` total · `{}` approved · `{}` pending",
            view.svs_number,
            view.google,
            view.total,
            view.approved,
            view.pending,
        ))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()))
}

pub fn ministry_panel(view: &MinistryView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = ministry_embed(view, &format!("{} Ministry — Forms", icons::LIST));
    let rows = vec![
        CreateActionRow::Buttons(vec![
            components::primary("ministry:svs", "📝 SVS Form / Sheet"),
            components::success("ministry:export", "📊 Export CSV"),
        ]),
        CreateActionRow::Buttons(vec![
            components::secondary("ministry:export_xlsx", "📈 Export Excel"),
            components::secondary("ministry:export_png", "🖼️ Export image"),
        ]),
        CreateActionRow::Buttons(vec![
            components::secondary("ministry:history", format!("{} History", icons::LIST)),
            components::secondary(
                "ministry:refresh",
                format!("{} Refresh All", icons::REFRESH),
            ),
        ]),
        CreateActionRow::Buttons(vec![components::secondary(
            "ministry:settings",
            format!("{} Settings", icons::EDIT),
        )]),
    ];
    (embed, rows)
}

pub fn ministry_settings_panel(view: &MinistryView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = ministry_embed(view, &format!("{} Ministry — Settings", icons::SETTINGS)).field(
        "Panel channel",
        view.channel
            .map(|id| format!("<#{id}>"))
            .unwrap_or_else(|| "`not set` — use **Admin Panel Channel**".to_string()),
        false,
    );
    let rows = vec![
        CreateActionRow::Buttons(vec![
            components::primary(
                "ministry:channel",
                format!("{} Admin Panel Channel", icons::EDIT),
            ),
            components::primary(
                "ministry:post_here",
                format!("{} Open Panel Here", icons::ADD),
            ),
            components::secondary(
                "ministry:repost",
                format!("{} Repost Panel", icons::REFRESH),
            ),
        ]),
        CreateActionRow::Buttons(vec![
            components::secondary("ministry:svsset", "#️⃣ Set SVS #"),
            components::secondary(
                "ministry:toggleapps",
                if view.applications_open {
                    "🔒 Close applications"
                } else {
                    "🔓 Open applications"
                },
            ),
            components::secondary("ministry:google", "🔗 Google"),
        ]),
        CreateActionRow::Buttons(vec![components::secondary(
            "ministry:home",
            format!("{} Back", icons::BACK),
        )]),
    ];
    (embed, rows)
}

pub fn ministry_svs_panel(view: &MinistryView) -> (CreateEmbed, Vec<CreateActionRow>) {
    let has_open_form = matches!(&view.form, Some((_, _, false)));
    let has_any_form = view.form.is_some();
    let embed = ministry_embed(view, &format!("{} Ministry — SVS", icons::SWORD))
        .field(
            "Unified SVS reservation form",
            "One open form per server. Creating a form also creates the reservation workspace spreadsheet (Applicant + TD/CD/RD tabs) and shares it anyone-with-link.",
            false,
        );
    let create_btn = components::success("ministry:create", "➕ Create Unified SVS");
    let close_btn = components::danger("ministry:close", "🔒 Close Form");
    let share_btn = components::primary("ministry:share", "🔗 Share Public");
    let copy_btn = components::secondary("ministry:copy", "📋 Copy links");
    let rows = vec![
        CreateActionRow::Buttons(vec![
            create_btn.disabled(has_open_form),
            close_btn.disabled(!has_open_form),
        ]),
        CreateActionRow::Buttons(vec![
            share_btn.disabled(!has_any_form),
            copy_btn.disabled(!has_any_form),
        ]),
        CreateActionRow::Buttons(vec![components::secondary(
            "ministry:settings",
            format!("{} Settings", icons::BACK),
        )]),
    ];
    (embed, rows)
}

pub fn ministry_google_panel(
    view: &MinistryView,
    auth_url: Option<String>,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = embeds::brand(format!("{} Ministry — Google", icons::LINK))
        .color(colors::BRAND)
        .description(format!(
            "**Google:** {}\n\nMinistry forms, the reservation workspace and the applicant-sheet live write need a Google account with Sheets + Forms + Drive access.\n\n1. Click **Continue with Google** (opens Google sign-in)\n2. Sign in and grant access\n3. Copy the code (or the whole redirect URL)\n4. Click **Paste code** and paste it here",
            view.google,
        ))
        .footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let mut rows = Vec::new();
    if let Some(url) = auth_url {
        rows.push(CreateActionRow::Buttons(vec![components::link(
            "Continue with Google",
            &url,
        )]));
    }
    rows.push(CreateActionRow::Buttons(vec![
        components::primary("ministry:pastecode", "📋 Paste code"),
        components::danger("ministry:disconnect", "🗑️ Disconnect"),
    ]));
    rows.push(CreateActionRow::Buttons(vec![components::secondary(
        "ministry:settings",
        format!("{} Back", icons::BACK),
    )]));
    (embed, rows)
}

/// History page — latest submissions + pending picker for review.
/// Picking an entry opens the review page via `ministry:review:{id}`.
pub fn ministry_history_panel(
    view: &MinistryView,
    forms: &[String],
    recent: &[String],
    pending_options: &[serenity::CreateSelectMenuOption],
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let forms_lines = if forms.is_empty() {
        "_No forms yet._".to_string()
    } else {
        forms.join("\n")
    };
    let mut lines = recent.join("\n");
    if lines.is_empty() {
        lines = "_No submissions yet — they appear here after the form syncs._".to_string();
    }
    let embed = ministry_embed(view, &format!("{} Ministry — History", icons::LIST))
        .field("Recent forms", forms_lines, false)
        .field(format!("Latest submissions ({})", view.total), lines, false);

    let mut rows = Vec::new();
    if !pending_options.is_empty() {
        rows.push(CreateActionRow::SelectMenu(
            serenity::CreateSelectMenu::new(
                "ministry:review",
                serenity::CreateSelectMenuKind::String {
                    options: pending_options.to_vec(),
                },
            )
            .placeholder(format!("Review pending ({})", pending_options.len()))
            .min_values(1)
            .max_values(1),
        ));
    }
    rows.push(CreateActionRow::Buttons(vec![components::secondary(
        "ministry:home",
        format!("{} Back", icons::BACK),
    )]));
    (embed, rows)
}

/// Review page for one submission (approve / reject).
pub fn ministry_review_panel(
    view: &MinistryView,
    submission: &crate::ministry::Submission,
) -> (CreateEmbed, Vec<CreateActionRow>) {
    let id = submission.id;
    let embed = ministry_embed(view, &format!("{} Ministry — Review #{id}", icons::LIST)).field(
        format!("Submission #{id}"),
        format!(
            "**Applicant:** {}\n**FID:** `{}`\n**Events:** {}\n**FC:** {} · **rFC:** {}\n**TS:** {} · **CS:** {} · **RS:** {} · **GS:** {}\n**Preferred time:** {}\n**Status:** `{}`",
            submission.applicant_name.as_deref().unwrap_or("?"),
            submission.fid,
            submission.events(),
            submission.fc.as_deref().unwrap_or("—"),
            submission.rfc.as_deref().unwrap_or("—"),
            submission.ts.as_deref().unwrap_or("—"),
            submission.cs.as_deref().unwrap_or("—"),
            submission.rs.as_deref().unwrap_or("—"),
            submission.gs.as_deref().unwrap_or("—"),
            submission.preferred_time.as_deref().unwrap_or("—"),
            submission.app_status,
        ),
        false,
    );
    let rows = vec![CreateActionRow::Buttons(vec![
        components::success(format!("ministry:approve:{id}"), "✅ Approve"),
        components::danger(format!("ministry:reject:{id}"), "❌ Reject"),
        components::secondary("ministry:history", format!("{} Back", icons::BACK)),
    ])];
    (embed, rows)
}
