//! Permission checks shared by commands and component handlers.

use poise::serenity_prelude as serenity;
use serenity::Permissions;

use crate::error::OmniError;
use crate::state::Data;

/// Owner override configured through `.env` or poise's owner list.
pub fn is_owner(ctx: &crate::commands::Context<'_>) -> bool {
    let user = ctx.author().id.get();
    if ctx.data().cfg.owner_id == Some(user) {
        return true;
    }
    ctx.framework().options().owners.contains(&ctx.author().id)
}

pub async fn member_permissions(ctx: &crate::commands::Context<'_>) -> Option<Permissions> {
    // Do not hold the cache guard across `.await` — it is not `Send`. Fetch the
    // member first, then look the guild up from the cache afterwards.
    let member = ctx.author_member().await?;
    let guild_id = ctx.guild_id()?;
    let guild = ctx.cache().guild(guild_id)?;
    Some(guild.member_permissions(&member))
}

/// DB-backed bot admins (legacy `admin` table) also count as admins.
async fn db_is_admin(data: &crate::state::Data, user_id: u64) -> bool {
    crate::db::admin::is_admin(&data.db, user_id).await
}

/// Admins are: the bot owner, anyone with Administrator, or anyone with
/// Manage Server. This mirrors the legacy permission manager closely enough.
pub async fn is_admin(ctx: &crate::commands::Context<'_>) -> bool {
    if is_owner(ctx) {
        return true;
    }
    if db_is_admin(ctx.data(), ctx.author().id.get()).await {
        return true;
    }
    member_permissions(ctx).await.is_some_and(|perms| {
        perms.contains(Permissions::ADMINISTRATOR) || perms.contains(Permissions::MANAGE_GUILD)
    })
}

pub async fn require_admin(ctx: &crate::commands::Context<'_>) -> Result<(), anyhow::Error> {
    if is_admin(ctx).await {
        Ok(())
    } else {
        Err(OmniError::MissingPermission("Manage Server").into())
    }
}

pub async fn require_owner(ctx: &crate::commands::Context<'_>) -> Result<(), anyhow::Error> {
    if is_owner(ctx) {
        Ok(())
    } else {
        Err(OmniError::MissingPermission("Bot Owner").into())
    }
}

/// Same checks for raw component interactions, which do not have a poise
/// context.
pub async fn component_is_admin(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::ComponentInteraction,
) -> bool {
    if data.cfg.owner_id == Some(interaction.user.id.get()) {
        return true;
    }
    if db_is_admin(data, interaction.user.id.get()).await {
        return true;
    }
    if crate::admintoolkit::elevates_component(&data.db, interaction).await {
        return true;
    }
    let Some(member) = interaction.member.as_ref() else {
        return false;
    };
    let guild_id = interaction.guild_id.unwrap_or(member.guild_id);
    let Some(guild) = ctx.cache.guild(guild_id) else {
        return false;
    };
    member_has_admin(&guild, member)
}

/// Same family as `component_is_admin`, plus per-server staff (Server Bot
/// Admin / Sub-Admin) — mirrors `canManageGuild` in the legacy bot.
pub async fn component_can_manage_guild(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::ComponentInteraction,
    guild_id: u64,
) -> bool {
    if component_is_admin(ctx, data, interaction).await {
        return true;
    }
    crate::db::subscriptions::is_server_staff(&data.db, guild_id, interaction.user.id.get()).await
}

pub async fn modal_is_admin(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::ModalInteraction,
) -> bool {
    if data.cfg.owner_id == Some(interaction.user.id.get()) {
        return true;
    }
    if db_is_admin(data, interaction.user.id.get()).await {
        return true;
    }
    if crate::admintoolkit::elevates_modal(&data.db, interaction).await {
        return true;
    }
    let Some(member) = interaction.member.as_ref() else {
        return false;
    };
    let guild_id = interaction.guild_id.unwrap_or(member.guild_id);
    let Some(guild) = ctx.cache.guild(guild_id) else {
        return false;
    };
    member_has_admin(&guild, member)
}

fn member_has_admin(guild: &serenity::Guild, member: &serenity::Member) -> bool {
    let perms = guild.member_permissions(member);
    perms.contains(Permissions::ADMINISTRATOR) || perms.contains(Permissions::MANAGE_GUILD)
}
