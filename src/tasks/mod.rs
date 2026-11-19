//! Background jobs: gift-code sync, Oracle enrichment and music-panel ticks.
//!
//! Every loop is fail-soft: an upstream outage logs and retries on the next
//! interval, it never takes the gateway connection down.

use std::time::Duration;

use poise::serenity_prelude as serenity;
use tokio::time::{interval, MissedTickBehavior};

use crate::db;
use crate::state::Data;

/// Runs a background job forever: if it panics it is logged and started again after 5 s
/// (a panic would otherwise end that job silently while the bot looked healthy). A job
/// that returns normally is not restarted.
fn spawn_restarting<F, Fut>(name: &'static str, make: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            match tokio::spawn(make()).await {
                Ok(()) => break,
                Err(err) if err.is_panic() => {
                    tracing::error!(
                        job = name,
                        "background job panicked: {err} — restarting in 5 s"
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(_) => break, // cancelled (runtime shutting down)
            }
        }
    });
}

/// Start every long-running job. Called once the bot id is known.
pub fn spawn(ctx: &serenity::Context, data: Data) {
    spawn_gift_sync(data.clone());
    spawn_oracle_enrichment(data.clone());
    spawn_music_ticks(ctx.clone(), data.clone());
    spawn_voice_xp(ctx.clone(), data.clone());
    spawn_join_reminders(ctx.clone(), data.clone());
    spawn_birthday_announcer(ctx.clone(), data.clone());
    spawn_giveaway_ender(ctx.clone(), data.clone());
    spawn_ballot_closer(ctx.clone(), data.clone());
    spawn_notification_sweep(ctx.clone(), data.clone());
    spawn_sneak_peek_poller(ctx.clone(), data.clone());
    crate::ministry::spawn_sync_loop(ctx.clone(), data);
}

fn spawn_notification_sweep(ctx: serenity::Context, data: Data) {
    spawn_restarting("notification_sweep", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(15));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await; // don't fire at startup
            loop {
                ticker.tick().await;
                if let Err(err) = crate::notifications::check(&ctx, &data).await {
                    tracing::warn!(error = %crate::error::display_anyhow(&err), "notification sweep failed");
                }
                if let Err(err) = crate::notifications::sweep_deletions(&ctx, &data).await {
                    tracing::warn!(error = %crate::error::display_anyhow(&err), "notification delete sweep failed");
                }
            }
        }
    });
}

fn spawn_gift_sync(data: Data) {
    let minutes = data.cfg.giftcode_sync_minutes.max(5);
    spawn_restarting("gift_sync", move || {
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(minutes * 60));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            // Give the gateway a moment to settle before the first run.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if data.oracle.is_configured().await {
                    run_gift_sync(&data).await;
                }
            }
        }
    });
}

async fn run_gift_sync(data: &Data) {
    match crate::giftcode::sync(data).await {
        Ok(outcome) if !outcome.fresh.is_empty() => {
            tracing::info!(
                total = outcome.total,
                fresh = outcome.fresh.len(),
                channels = outcome.announcements,
                "gift-code sync"
            );
            if outcome.announcements == 0 {
                tracing::debug!("no gift-code announcement channel configured");
            }
        }
        Ok(_) => {}
        Err(err) => {
            let message = crate::error::display_anyhow(&err);
            tracing::warn!(error = %message, "gift-code sync failed");
            let _ = crate::db::oracle::record_error(&data.db, &message).await;
        }
    }
}

fn spawn_oracle_enrichment(data: Data) {
    let minutes = data.cfg.lookup_sync_minutes.max(10);
    spawn_restarting("oracle_enrichment", move || {
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(minutes * 60));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if data.oracle.is_configured().await {
                    run_enrichment(&data).await;
                }
            }
        }
    });
}

async fn run_enrichment(data: &Data) {
    // Refresh anything older than six hours, oldest first.
    let cutoff = crate::db::now() - 6 * 3600;
    let fids = match crate::db::players::stale_fids(&data.db, cutoff, 25).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, "reading stale players failed");
            return;
        }
    };

    let mut enriched = 0i64;
    for fid in &fids {
        match data.oracle.fetch_player(fid).await {
            Ok(value) => {
                let enrichment = crate::oracle::enrichment_from_player(&value);
                if let Err(err) =
                    crate::db::players::apply_enrichment(&data.db, fid, &enrichment).await
                {
                    tracing::debug!(error = %err, fid = %fid, "applying enrichment failed");
                } else {
                    enriched += 1;
                }
            }
            Err(crate::oracle::OracleError::NotConfigured) => return,
            Err(crate::oracle::OracleError::Expired(status)) => {
                tracing::warn!(status, "oracle session expired during enrichment");
                let _ = crate::db::oracle::record_error(
                    &data.db,
                    &format!("session expired (HTTP {status})"),
                )
                .await;
                return;
            }
            Err(err) => {
                tracing::debug!(error = %err, fid = %fid, "player enrichment failed");
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    if enriched > 0 {
        let _ = crate::db::oracle::touch_lookup_sync(&data.db, enriched).await;
        tracing::info!(players = enriched, "oracle enrichment");
    }
}

fn spawn_join_reminders(ctx: serenity::Context, data: Data) {
    spawn_restarting("join_reminders", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(6 * 3600)); // 6h
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await; // skip the first immediate tick
            loop {
                ticker.tick().await;
                let now_ms = crate::db::now() * 1000;
                match crate::db::welcome::due_reminders(&data.db, now_ms, 50).await {
                    Ok(due) => {
                        for (guild_id, user_id) in due {
                            crate::welcome::send_reminder(&ctx, &data, guild_id, user_id).await;
                            // Be gentle: never hammer the gateway.
                            tokio::time::sleep(Duration::from_millis(1200)).await;
                        }
                    }
                    Err(err) => {
                        tracing::debug!(error = %err, "reading due join reminders failed")
                    }
                }
            }
        }
    });
}

fn spawn_sneak_peek_poller(ctx: serenity::Context, data: Data) {
    spawn_restarting("sneak_peek_poller", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            use rand::Rng;
            let mut ticker = interval(Duration::from_secs(20 * 60)); // ~20 min baseline
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await; // skip immediate tick
            loop {
                ticker.tick().await;
                // Only poll if any server wants sneak peeks.
                let Ok(destinations) = crate::db::sneak_peek::destinations(&data.db).await else {
                    continue;
                };
                if destinations.is_empty() {
                    continue;
                }
                let Some(latest) = crate::sneakpeek::fetch_latest_url().await.ok().flatten() else {
                    let _ = crate::db::sneak_peek::touch_checked(&data.db).await;
                    continue;
                };
                let state = crate::db::sneak_peek::state(&data.db)
                    .await
                    .unwrap_or_default();
                if state.last_url.as_deref() == Some(latest.as_str()) {
                    let _ = crate::db::sneak_peek::touch_checked(&data.db).await;
                    continue;
                }
                let Ok(post) = crate::sneakpeek::fetch_post(&latest).await else {
                    let _ = crate::db::sneak_peek::touch_checked(&data.db).await;
                    continue;
                };
                match broadcast(&ctx, &data, &post).await {
                    Ok(count) => {
                        if count > 0 {
                            tracing::info!(channels = count, url = %latest, "sneak peek broadcast");
                        }
                        let _ = crate::db::sneak_peek::set_state(&data.db, &latest, None).await;
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "sneak peek broadcast failed");
                    }
                }
                let _ = rand::thread_rng().gen_range(0..30u64); // jitter hook (cheap no-op)
            }
        }
    });
}

async fn broadcast(
    ctx: &serenity::Context,
    data: &Data,
    post: &crate::sneakpeek::SneakPeekPost,
) -> anyhow::Result<usize> {
    let destinations = crate::db::sneak_peek::destinations(&data.db).await?;
    if destinations.is_empty() {
        return Ok(0);
    }
    let mut delivered = 0usize;
    for (guild_id, channel_id) in destinations {
        let embed = crate::sneakpeek::embed(post);
        match serenity::ChannelId::new(channel_id)
            .send_message(
                &ctx.http,
                serenity::CreateMessage::new()
                    .embed(embed)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
        {
            Ok(message) => {
                delivered += 1;
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
                        guild_id: Some(guild_id),
                        channel_id: Some(channel_id),
                        kind: "sneakpeek",
                        embed_json: crate::sneakpeek::stored_embed(post),
                        content: None,
                        owner_id: None,
                    },
                )
                .await;
            }
            Err(err) => {
                tracing::warn!(error = %err, channel = channel_id, "sneak peek send failed")
            }
        }
    }
    Ok(delivered)
}

fn spawn_giveaway_ender(ctx: serenity::Context, data: Data) {
    spawn_restarting("giveaway_ender", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(10));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let due = db::giveaways::list_due(&data.db, db::now() * 1000)
                    .await
                    .unwrap_or_default();
                for giveaway in due {
                    if let Err(err) =
                        crate::commands::giveaways::end_giveaway(&ctx, &data, &giveaway).await
                    {
                        tracing::warn!(giveaway = giveaway.id, error = %crate::error::display_anyhow(&err), "ending giveaway failed; will retry");
                    }
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                }
            }
        }
    });
}

fn spawn_ballot_closer(ctx: serenity::Context, data: Data) {
    spawn_restarting("ballot_closer", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(30));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let due = db::ballots::list_due(&data.db, db::now() * 1000)
                    .await
                    .unwrap_or_default();
                for ballot in due {
                    let claimed = db::ballots::claim_close(&data.db, ballot.id)
                        .await
                        .unwrap_or(false);
                    if claimed {
                        let _ =
                            crate::ballots::rebuild_ballot_message(&ctx, &data, ballot.id).await;
                        let _ = crate::commands::interactions::post_ballot_results(
                            &ctx, &data, ballot.id,
                        )
                        .await;
                    }
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                }
            }
        }
    });
}

fn spawn_birthday_announcer(ctx: serenity::Context, data: Data) {
    spawn_restarting("birthday_announcer", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(60));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await; // skip the first immediate tick
            loop {
                ticker.tick().await;
                let today = db::birthdays::due_today(&data.db).await.unwrap_or_default();
                for (guild_id, user_id) in today {
                    announce_birthday(&ctx, &data, guild_id, user_id).await;
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                }
            }
        }
    });
}

async fn announce_birthday(ctx: &serenity::Context, data: &Data, guild_id: u64, user_id: u64) {
    let Ok(settings) = db::birthdays::settings(&data.db, guild_id).await else {
        return;
    };
    let Some(channel_id) = settings.channel() else {
        return;
    };

    let server = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| "this server".to_string());
    let member_count = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.members.len() as u64)
        .unwrap_or(0);

    let (title, description, color) =
        db::birthdays::render(&settings, &format!("<@{user_id}>"), &server, member_count).await;

    let embed = serenity::CreateEmbed::new()
        .title(title)
        .description(description)
        .color(color)
        .footer(serenity::CreateEmbedFooter::new(crate::ui::theme::footer()));

    let _ = serenity::ChannelId::new(channel_id)
        .send_message(
            &ctx.http,
            serenity::CreateMessage::new()
                .content(format!("<@{user_id}>"))
                .embed(embed),
        )
        .await;

    let _ = db::birthdays::mark_announced(&data.db, guild_id, user_id).await;
}

fn spawn_voice_xp(ctx: serenity::Context, data: Data) {
    spawn_restarting("voice_xp", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(60));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await; // skip the first immediate tick
            loop {
                ticker.tick().await;
                crate::leveling::voice_tick(&ctx, &data).await;
            }
        }
    });
}

fn spawn_music_ticks(ctx: serenity::Context, data: Data) {
    spawn_restarting("music_ticks", move || {
        let ctx = ctx.clone();
        let data = data.clone();
        async move {
            let mut ticker = interval(Duration::from_secs(10));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let guilds = data.music.guild_ids().await;
                for guild_id in guilds {
                    if data.music.tick_position(guild_id, 10_000).await {
                        crate::music::refresh_panel(&ctx, guild_id, &data.music).await;
                    }
                }
            }
        }
    });
}
