//! `/settings` — the only slash command. Opens the button-panel hub.
//!
//! Inside a server it opens the server settings hub (admin-only editing;
//! members get a read-only view). In a DM with the bot it opens the bot
//! owner's control panel (the same Permissions manager).

use poise::serenity_prelude::CreateActionRow;

use super::{Context, Error};
use crate::db;
use crate::permissions;
use crate::ui::panels;
use crate::ui::theme;
use poise::serenity_prelude::CreateEmbedFooter;

#[poise::command(slash_command, category = "Settings")]
pub async fn settings(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();

    let Some(guild_id) = ctx.guild_id().map(|id| id.get()) else {
        return dm_control_center(ctx).await;
    };

    ctx.defer_ephemeral().await?;

    let guild_name = ctx
        .guild()
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| "this server".to_string());

    let player_stats = db::players::stats(&data.db, guild_id).await?;
    let music = db::music::get(&data.db, guild_id).await?;
    let lookup = db::lookup::get(&data.db, guild_id).await?;
    let oracle_state = db::oracle::state(&data.db).await.unwrap_or_default();
    let oracle_ready = data.oracle.is_configured().await;

    let view = panels::SettingsContext {
        guild_name,
        player_ids: player_stats.tracked,
        player_limit: db::subscriptions::player_id_limit(&data.db, guild_id)
            .await
            .ok()
            .flatten(),
        music_configured: music
            .as_ref()
            .is_some_and(|row| row.channel_id.is_some() && row.panel_message_id.is_some()),
        lookup_configured: lookup.channel_id.is_some(),
        gift_sync: oracle_state.last_codes_sync,
        oracle_ready,
        uptime: data.uptime(),
        latency_ms: ctx.ping().await.as_millis() as u64,
        is_owner: permissions::is_owner(&ctx),
    };

    let (mut embed, mut rows) = panels::settings_hub(&view);
    if !permissions::is_admin(&ctx).await {
        // Read-only view for regular members: strip the interactive rows.
        rows.clear();
        embed = embed.footer(CreateEmbedFooter::new(format!(
            "{} • view only (Manage Server required to edit)",
            theme::footer()
        )));
    }

    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .components(rows)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// The DM control center: the bot owner's permissions panel, button-driven.
async fn dm_control_center(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    let user_id = ctx.author().id.get();

    let is_owner =
        data.cfg.owner_id == Some(user_id) || db::admin::is_admin(&data.db, user_id).await;

    if !is_owner {
        return ctx
            .send(
                poise::CreateReply::default()
                    .content("Run `/settings` inside your Discord server. (This DM menu is for the bot owner.)")
                    .ephemeral(true),
            )
            .await
            .map(|_| ())
            .map_err(anyhow::Error::from);
    }

    ctx.defer_ephemeral().await?;

    let is_bot_owner = data.cfg.owner_id == Some(ctx.author().id.get());
    if is_bot_owner {
        // Bot owner DM: owner hub (statistics, servers, features, gift priority,
        // backup). A button jumps into the global Permissions (DM-backed admins) panel.
        let embed = crate::owner::hub_embed(data).await;
        let mut rows = crate::owner::hub_rows();
        rows.push(CreateActionRow::Buttons(vec![
            crate::ui::components::secondary(
                "owner:perm",
                format!("{} Permissions", crate::ui::theme::icons::ADMIN),
            ),
        ]));
        return ctx
            .send(
                poise::CreateReply::default()
                    .embed(embed)
                    .components(rows)
                    .ephemeral(true),
            )
            .await
            .map(|_| ())
            .map_err(anyhow::Error::from);
    }

    // Same Permissions manager as the guild hub's Admin page; guild_id = 0
    // simply yields name-less entries (there is no guild cache outside a server).
    let (embed, rows) =
        crate::commands::interactions::admin_panel(ctx.serenity_context(), data, 0).await?;

    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .components(rows)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}
