//! Welcome message, auto-role and join-DM plumbing.

use poise::serenity_prelude as serenity;
use serenity::UserId;

use crate::db;
use crate::state::Data;

/// Welcome a new member: send the configured embed and assign auto roles.
pub async fn on_member_join(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    if member.user.bot {
        return;
    }

    let guild_id = member.guild_id.get();

    welcome_message(ctx, data, member).await;
    apply_auto_roles(ctx, data, member).await;
    join_dm(ctx, data, member).await;

    let _ = guild_id;
}

/// Forget the member (join-DM reminder tracking cleanup).
pub async fn on_member_remove(data: &Data, guild_id: u64, user_id: u64) {
    let _ = db::welcome::remove_join(&data.db, guild_id, user_id).await;
}

// ---------------------------------------------------------------- welcome

pub async fn welcome_message(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    let guild_id = member.guild_id.get();
    let Ok(settings) = db::welcome::welcome(&data.db, guild_id).await else {
        return;
    };
    let Some(channel_id) = settings.channel() else {
        return;
    };

    let server = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| format!("guild {guild_id}"));
    let member_count = ctx
        .cache
        .guild(member.guild_id)
        .map(|guild| guild.members.len() as u64)
        .unwrap_or(0);
    let rendered = settings.render(
        &format!("<@{}>", member.user.id.get()),
        &server,
        member_count,
    );

    let mut embed = serenity::CreateEmbed::new()
        .title(&rendered.title)
        .description(&rendered.description)
        .color(rendered.color as u32)
        .footer(serenity::CreateEmbedFooter::new(crate::ui::theme::footer()));

    match rendered.avatar_mode.as_str() {
        "member" => {
            embed = embed.thumbnail(member.user.face());
        }
        "server" => {}
        _ => {}
    }

    let _ = serenity::ChannelId::new(channel_id)
        .send_message(
            &ctx.http,
            serenity::CreateMessage::new()
                .add_embed(embed)
                .allowed_mentions(serenity::CreateAllowedMentions::new().users([member.user.id])),
        )
        .await;
}

// ---------------------------------------------------------------- autorole

pub async fn apply_auto_roles(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    // Members still in Membership Screening can't receive roles meaningfully.
    if member.pending {
        return;
    }

    let guild_id = member.guild_id.get();
    let Ok(settings) = db::welcome::autorole(&data.db, guild_id).await else {
        return;
    };
    if !settings.enabled() {
        return;
    }

    let wanted = if member.user.bot {
        settings.bot_roles()
    } else {
        settings.member_roles()
    };
    if wanted.is_empty() {
        return;
    }

    // Read everything from the cache in one borrow, then drop it before the await.
    let assignable: Vec<serenity::RoleId> = {
        let Some(guild) = ctx.cache.guild(member.guild_id) else {
            return;
        };

        let bot_id = ctx.cache.current_user().id;
        let bot_top = guild
            .members
            .get(&bot_id)
            .map(|bot| {
                bot.roles
                    .iter()
                    .filter_map(|role_id| guild.roles.get(role_id))
                    .map(|role| role.position)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        wanted
            .into_iter()
            .filter_map(|role_id| {
                guild
                    .roles
                    .get(&serenity::RoleId::new(role_id))
                    .filter(|role| !role.managed && role.position < bot_top)
                    .map(|role| role.id)
            })
            .filter(|role_id| !member.roles.contains(role_id))
            .collect()
    };

    if assignable.is_empty() {
        return;
    }

    let _ = member.add_roles(&ctx.http, &assignable).await;
}

// ---------------------------------------------------------------- join DM

pub async fn join_dm(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    let guild_id = member.guild_id.get();
    if member.user.bot {
        return;
    }

    let _ = db::welcome::track_join(&data.db, guild_id, member.user.id.get()).await;

    let Ok(settings) = db::welcome::join_dm(&data.db, guild_id).await else {
        return;
    };
    if !settings.join_dm_enabled() {
        return;
    }

    let server = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| "this server".to_string());

    let embed = serenity::CreateEmbed::new()
        .color(crate::ui::theme::colors::BRAND)
        .title(format!("👋 Welcome to {server}"))
        .description(
            "Gift codes are only auto-redeemed for members who register their in-game Player ID.\n\
             Tap **Register my Play ID** below to link it now.",
        )
        .footer(serenity::CreateEmbedFooter::new(crate::ui::theme::footer()));

    let register = serenity::CreateButton::new(format!("wreg:{}", guild_id))
        .label("Register my Play ID")
        .style(serenity::ButtonStyle::Primary);

    let _ = member
        .user
        .dm(
            &ctx.http,
            serenity::CreateMessage::new()
                .embed(embed)
                .components(vec![serenity::CreateActionRow::Buttons(vec![register])]),
        )
        .await;

    let _ = db::welcome::mark_join_dm_sent(&data.db, guild_id, member.user.id.get()).await;
}

/// Send a registration reminder DM (called by the background loop).
pub async fn send_reminder(ctx: &serenity::Context, data: &Data, guild_id: u64, user_id: u64) {
    let server = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| "this server".to_string());

    let embed = serenity::CreateEmbed::new()
        .color(crate::ui::theme::colors::GOLD)
        .title("Still haven't registered?")
        .description(format!(
            "Register your Player ID in **{server}** to get gift codes auto-redeemed. \n\
             Tap the button below."
        ))
        .footer(serenity::CreateEmbedFooter::new(crate::ui::theme::footer()));

    let register = serenity::CreateButton::new(format!("wreg:{guild_id}"))
        .label("Register my Play ID")
        .style(serenity::ButtonStyle::Primary);

    let user = match ctx.http.get_user(UserId::new(user_id)).await {
        Ok(user) => user,
        Err(err) => {
            tracing::debug!(error = %err, guild = guild_id, user = user_id, "reminder target gone");
            return;
        }
    };

    let _ = user
        .dm(
            &ctx.http,
            serenity::CreateMessage::new()
                .embed(embed)
                .components(vec![serenity::CreateActionRow::Buttons(vec![register])]),
        )
        .await;

    let _ = db::welcome::mark_reminder_sent(&data.db, guild_id, user_id).await;
}
