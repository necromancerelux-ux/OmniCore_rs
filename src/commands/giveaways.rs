//! Shared giveaway rendering/ending logic (used by tasks and the panel).

use crate::db;
use crate::ui::theme::{colors, icons};
use crate::ui::{components, embeds, theme};
use poise::serenity_prelude as serenity;

/// Render a giveaway embed + enter button.
pub async fn render_giveaway(
    db: &crate::db::Db,
    giveaway: &db::giveaways::Giveaway,
) -> anyhow::Result<(serenity::CreateEmbed, Vec<serenity::CreateActionRow>)> {
    let entries = db::giveaways::count_entries(db, giveaway.id).await?;
    let ends_at = giveaway.ends_at / 1000;

    let mut embed = embeds::brand(format!("{} {}", icons::GIFT, giveaway.prize))
        .color(colors::GOLD)
        .field("Ends", crate::ui::theme::utc_with_relative(ends_at), true)
        .field("Entries", format!("`{entries}`"), true)
        .field("Winners", format!("`{}`", giveaway.winner_count()), true)
        .field("Hosted by", format!("<@{}>", giveaway.host_id), true);

    if let Some(description) = giveaway.description.as_deref() {
        embed = embed.description(description);
    }
    let rules = giveaway.rules();
    let (requirements, bonuses) = rules.describe();
    if !requirements.is_empty() {
        embed = embed.field("Requirements", requirements.join("\n"), false);
    }
    if !bonuses.is_empty() {
        embed = embed.field("Bonus entries", bonuses.join("\n"), false);
    }
    embed = embed.footer(serenity::CreateEmbedFooter::new(theme::footer()));

    let rows = vec![components::row(vec![components::success(
        format!("giveaway:enter:{}", giveaway.id),
        format!("{} Enter", icons::ADD),
    )])];

    Ok((embed, rows))
}

/// Check the advanced giveaway rules for a member (blocked roles, account
/// age, server age, min level). Returns the legacy-style failure reason.
pub async fn check_eligibility(
    data: &crate::state::Data,
    giveaway: &db::giveaways::Giveaway,
    member: &serenity::Member,
) -> Result<(), String> {
    if member.user.bot {
        return Err("bots can't enter".to_string());
    }
    let rules = giveaway.rules();

    if let Some(blocked) = rules
        .blocked_role_ids
        .iter()
        .find(|id| member.roles.iter().any(|r| r.get() == **id))
    {
        return Err(format!(
            "members with <@&{blocked}> can't enter this giveaway"
        ));
    }

    let now = chrono::Utc::now().timestamp();
    if rules.min_account_days > 0 {
        let age_days = (now - member.user.created_at().timestamp()) / 86_400;
        if age_days < rules.min_account_days {
            return Err(format!(
                "your Discord account must be at least **{}** day(s) old",
                rules.min_account_days
            ));
        }
    }
    if rules.min_server_days > 0 {
        let joined_days = member
            .joined_at
            .map(|joined| (now - joined.timestamp()) / 86_400)
            .unwrap_or(0);
        if joined_days < rules.min_server_days {
            return Err(format!(
                "you must have been in this server for at least **{}** day(s)",
                rules.min_server_days
            ));
        }
    }
    if rules.min_level > 0 {
        let xp =
            crate::db::leveling::get_user(&data.db, member.guild_id.get(), member.user.id.get())
                .await
                .ok()
                .map(|row| row.xp)
                .unwrap_or(0);
        let level = crate::leveling::level_of(xp) as i64;
        if level < rules.min_level {
            return Err(format!(
                "you need **level {}** (you're level {})",
                rules.min_level, level
            ));
        }
    }
    Ok(())
}

/// Ticket weight for an entry (mirrors the legacy entryWeight()).
async fn weight_of(
    data: &crate::state::Data,
    giveaway: &db::giveaways::Giveaway,
    rules: &db::giveaways::GiveawayRules,
    member: Option<&serenity::Member>,
    user_id: u64,
) -> i64 {
    let mut weight = 1i64;
    if let Some(member) = member {
        if rules.role_bonus > 0 {
            let bonus = rules
                .bonus_role_ids
                .iter()
                .filter(|id| member.roles.iter().any(|r| r.get() == **id))
                .count() as i64;
            weight += bonus * rules.role_bonus;
        }
        if rules.booster_bonus > 0 && member.premium_since.is_some() {
            weight += rules.booster_bonus;
        }
    }
    if rules.level_step > 0 {
        let xp = crate::db::leveling::get_user(
            &data.db,
            giveaway.guild_id.parse::<u64>().unwrap_or(0),
            user_id,
        )
        .await
        .ok()
        .map(|row| row.xp)
        .unwrap_or(0);
        weight += crate::leveling::level_of(xp) as i64 / rules.level_step;
    }
    weight.clamp(1, rules.max_weight.max(1))
}

/// End an active giveaway: flip status, draw winners, announce.
/// Safe to call twice — the DB claim decides who proceeds.
pub async fn end_giveaway(
    ctx: &serenity::Context,
    data: &crate::state::Data,
    giveaway: &db::giveaways::Giveaway,
) -> anyhow::Result<Vec<u64>> {
    if !giveaway.is_active() {
        return Ok(giveaway.winners());
    }
    if !db::giveaways::claim_end(&data.db, giveaway.id).await? {
        return Ok(giveaway.winners());
    }

    match end_claimed_giveaway(ctx, data, giveaway).await {
        Ok(winners) => Ok(winners),
        Err(err) => {
            // Nothing was announced yet: hand the giveaway back so the sweep retries it.
            let _ = db::giveaways::reopen(&data.db, giveaway.id).await;
            Err(err)
        }
    }
}

async fn end_claimed_giveaway(
    ctx: &serenity::Context,
    data: &crate::state::Data,
    giveaway: &db::giveaways::Giveaway,
) -> anyhow::Result<Vec<u64>> {
    let rules = giveaway.rules();
    let winners = if rules.has_bonuses() {
        // Weighted draw — fetch a member snapshot for each entry.
        let entries = db::giveaways::list_entries(&data.db, giveaway.id).await?;
        let mut weights = std::collections::HashMap::with_capacity(entries.len());
        for entry in &entries {
            // Be gentle with the REST API on big giveaways.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let member = ctx
                .http
                .get_member(
                    serenity::GuildId::new(giveaway.guild_id.parse::<u64>().unwrap_or(0)),
                    serenity::UserId::new(*entry),
                )
                .await
                .ok();
            weights.insert(
                *entry,
                weight_of(data, giveaway, &rules, member.as_ref(), *entry).await,
            );
        }
        db::giveaways::draw_winners_weighted(&data.db, giveaway, &[], |uid| {
            weights.get(&uid).copied().unwrap_or(1)
        })
        .await?
    } else {
        db::giveaways::draw_winners(&data.db, giveaway, &[]).await?
    };
    db::giveaways::set_winners(&data.db, giveaway.id, &winners).await?;
    let entries = db::giveaways::count_entries(&data.db, giveaway.id).await?;

    // Freeze the message and announce in-channel.
    if let (Some(channel_id), Some(message_id)) = (giveaway.channel_id(), giveaway.message_id()) {
        if let Ok(mut msg) = serenity::ChannelId::new(channel_id)
            .message(&ctx.http, serenity::MessageId::new(message_id))
            .await
        {
            let winners_text = if winners.is_empty() {
                "nobody entered".to_string()
            } else {
                winners
                    .iter()
                    .map(|id| format!("<@{id}>"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let embed = embeds::brand(format!("{} Ended — {}", icons::GIFT, giveaway.prize))
                .color(colors::VIOLET)
                .description(format!(
                    "{}\n**Entries:** {entries}\n**Winner(s):** {winners_text}",
                    theme::DIVIDER
                ))
                .footer(serenity::CreateEmbedFooter::new(theme::footer()));
            let _ = msg
                .edit(
                    &ctx.http,
                    serenity::EditMessage::new()
                        .embed(embed)
                        .components(Vec::new()),
                )
                .await;
        }

        let announcement = if winners.is_empty() {
            format!(
                "🎉 No valid entries for **{}** — no winner.",
                giveaway.prize
            )
        } else {
            let mentions = winners
                .iter()
                .map(|id| format!("<@{id}>"))
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "🎉 Congratulations {mentions}! You won **{}**!",
                giveaway.prize
            )
        };
        let _ = serenity::ChannelId::new(channel_id)
            .send_message(
                &ctx.http,
                serenity::CreateMessage::new()
                    .content(announcement)
                    .allowed_mentions(
                        serenity::CreateAllowedMentions::new().users(
                            winners
                                .iter()
                                .map(|id| serenity::UserId::new(*id))
                                .collect::<Vec<_>>(),
                        ),
                    ),
            )
            .await;
    }

    Ok(winners)
}
