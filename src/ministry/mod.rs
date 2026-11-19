//! Ministry — Google Forms/Sheets-backed SVS reservation forms.
//!
//! Mirrors the legacy `src/ministry/*` feature set in button-panel form:
//!
//!   `/settings` → **Forms** (settings hub) — SVS #, applications open/closed,
//!   Google connect, admin-panel channel, repost.
//!
//! The public "Ministry Update" admin panel lives in a configured channel:
//! SVS (create/close/share), Export (CSV / Excel / PNG roster card), History
//! (review applications), Refresh All (pull new form responses).
//!
//! Fully custom forms from the legacy panel are not ported.

pub mod google;

use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use poise::serenity_prelude as serenity;
use serde::Deserialize;
use sqlx::FromRow;

use crate::cards::{TableCard, TableColumn};
use crate::config::Config;
use crate::db::Db;
use crate::state::Data;

// ------------------------------------------------------------------ types

#[derive(Debug, Clone, Default, FromRow)]
pub struct MinistrySettings {
    pub guild_id: String,
    pub target_state: String,
    pub member_channel_id: Option<String>,
    pub member_message_id: Option<String>,
    pub admin_channel_id: Option<String>,
    pub admin_message_id: Option<String>,
    pub slot_minutes: i64,
    pub slots_per_day: i64,
    pub svs_number: Option<i64>,
    pub language: Option<String>,
    pub applications_open: Option<i64>,
    pub last_sheet_sync_at: Option<String>,
}

impl MinistrySettings {
    pub fn applications_open(&self) -> bool {
        self.applications_open.unwrap_or(1) != 0
    }
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct MinistryForm {
    pub id: i64,
    pub guild_id: String,
    pub category: String,
    pub form_id: Option<String>,
    pub form_url: Option<String>,
    pub label: Option<String>,
    pub status: String,
    pub svs_number: Option<i64>,
    pub workspace_spreadsheet_id: Option<String>,
    pub created_at: String,
}

impl MinistryForm {
    pub fn spreadsheet_id(&self) -> Option<&str> {
        self.workspace_spreadsheet_id.as_deref()
    }
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct Submission {
    pub id: i64,
    pub form_id: i64,
    pub svs_number: Option<i64>,
    pub applicant_name: Option<String>,
    pub fid: String,
    pub fc: Option<String>,
    pub rfc: Option<String>,
    pub ts: Option<String>,
    pub cs: Option<String>,
    pub rs: Option<String>,
    pub gs: Option<String>,
    pub preferred_time: Option<String>,
    pub want_td: Option<i64>,
    pub want_cd: Option<i64>,
    pub want_rd: Option<i64>,
    pub app_status: String,
    pub created_at: String,
}

impl Submission {
    pub fn events(&self) -> String {
        let mut events = Vec::new();
        if self.want_td.unwrap_or(0) != 0 {
            events.push("TD");
        }
        if self.want_cd.unwrap_or(0) != 0 {
            events.push("CD");
        }
        if self.want_rd.unwrap_or(0) != 0 {
            events.push("RD");
        }
        if events.is_empty() {
            "—".into()
        } else {
            events.join(", ")
        }
    }

    pub fn line(&self) -> String {
        format!(
            "`#{}` **{}** (`{}`) — {} · <t:{}:R>",
            self.id,
            self.applicant_name.as_deref().unwrap_or("?"),
            self.fid,
            self.events(),
            self.created_at_timestamp().unwrap_or(0),
        )
    }

    fn created_at_timestamp(&self) -> Option<i64> {
        // stored as datetime('now') UTC
        chrono::NaiveDateTime::parse_from_str(&self.created_at, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|dt| dt.and_utc().timestamp())
    }
}

// ---------------------------------------------------------------- settings

pub async fn get_settings(db: &Db, guild_id: u64) -> Result<MinistrySettings> {
    let key = guild_id.to_string();
    sqlx::query(
        "INSERT INTO ministry_settings (guild_id, target_state, slot_minutes, slots_per_day)
         VALUES (?, '3105', 30, 48)
         ON CONFLICT(guild_id) DO NOTHING",
    )
    .bind(&key)
    .execute(&db.pool)
    .await?;
    let row = sqlx::query_as::<_, MinistrySettings>(
        "SELECT guild_id, target_state, member_channel_id, member_message_id, admin_channel_id, admin_message_id,
                slot_minutes, slots_per_day, svs_number, language, applications_open, last_sheet_sync_at
           FROM ministry_settings WHERE guild_id = ?",
    )
    .bind(&key)
    .fetch_one(&db.pool)
    .await?;
    Ok(row)
}

pub async fn set_svs_number(db: &Db, guild_id: u64, svs: i64) -> Result<()> {
    let key = guild_id.to_string();
    let _ = get_settings(db, guild_id).await?;
    sqlx::query("UPDATE ministry_settings SET svs_number = ?, updated_at = datetime('now') WHERE guild_id = ?")
        .bind(svs.max(1))
        .bind(&key)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn set_applications_open(db: &Db, guild_id: u64, open: bool) -> Result<()> {
    let key = guild_id.to_string();
    let _ = get_settings(db, guild_id).await?;
    sqlx::query("UPDATE ministry_settings SET applications_open = ?, updated_at = datetime('now') WHERE guild_id = ?")
        .bind(open as i64)
        .bind(&key)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn set_admin_panel(
    db: &Db,
    guild_id: u64,
    channel_id: Option<u64>,
    message_id: Option<u64>,
) -> Result<()> {
    let key = guild_id.to_string();
    let _ = get_settings(db, guild_id).await?;
    sqlx::query(
        "UPDATE ministry_settings SET admin_channel_id = ?, admin_message_id = ?, updated_at = datetime('now') WHERE guild_id = ?",
    )
    .bind(channel_id.map(|c| c.to_string()))
    .bind(message_id.map(|c| c.to_string()))
    .bind(&key)
    .execute(&db.pool)
    .await?;
    Ok(())
}

// ------------------------------------------------------------------ forms

pub async fn get_active_form(db: &Db, guild_id: u64) -> Result<Option<MinistryForm>> {
    let key = guild_id.to_string();
    let row = sqlx::query_as::<_, MinistryForm>(
        "SELECT id, guild_id, category, form_id, form_url, label, status, svs_number, workspace_spreadsheet_id, created_at
           FROM ministry_forms
          WHERE guild_id = ? AND category = 'svs' AND status IN ('open','active')
          ORDER BY id DESC LIMIT 1",
    )
    .bind(&key)
    .fetch_optional(&db.pool)
    .await?;
    Ok(row)
}

pub async fn get_latest_form(db: &Db, guild_id: u64) -> Result<Option<MinistryForm>> {
    let key = guild_id.to_string();
    let row = sqlx::query_as::<_, MinistryForm>(
        "SELECT id, guild_id, category, form_id, form_url, label, status, svs_number, workspace_spreadsheet_id, created_at
           FROM ministry_forms
          WHERE guild_id = ? AND category = 'svs'
          ORDER BY id DESC LIMIT 1",
    )
    .bind(&key)
    .fetch_optional(&db.pool)
    .await?;
    Ok(row)
}

pub async fn close_form(db: &Db, guild_id: u64) -> Result<Option<MinistryForm>> {
    let Some(form) = get_active_form(db, guild_id).await? else {
        return Ok(None);
    };
    sqlx::query("UPDATE ministry_forms SET status = 'closed' WHERE id = ?")
        .bind(form.id)
        .execute(&db.pool)
        .await?;
    set_applications_open(db, guild_id, false).await?;
    Ok(Some(form))
}

pub async fn archive_form(db: &Db, guild_id: u64) -> Result<Option<MinistryForm>> {
    let form = get_active_form(db, guild_id)
        .await?
        .or(get_latest_form(db, guild_id).await?);
    let Some(form) = form else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE ministry_forms SET status = 'archived', archived_at = datetime('now') WHERE id = ?",
    )
    .bind(form.id)
    .execute(&db.pool)
    .await?;
    Ok(Some(form))
}

// --------------------------------------------------------------- create

pub async fn create_unified_form(
    cfg: &Config,
    db: &Db,
    guild_id: u64,
) -> Result<(MinistryForm, String)> {
    if get_active_form(db, guild_id).await?.is_some() {
        return Err(anyhow!("An SVS form is already open — close it first."));
    }
    if !cfg.google_configured() {
        return Err(anyhow!(
            "Google is not configured (set GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET)."
        ));
    }

    let settings = get_settings(db, guild_id).await?;
    let svs = settings.svs_number.unwrap_or(1).max(1);
    let title = format!("SVS #{svs} — Unified Reservation Form");

    let created = google::forms_api(
        cfg,
        db,
        "",
        "POST",
        Some(serde_json::json!({
            "info": { "title": title },
        })),
    )
    .await?;
    let form_id = created
        .get("formId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("form_create_failed"))?
        .to_string();

    // Question items (same layout as the legacy unified form).
    let items = serde_json::json!([
        { "createItem": { "item": { "title": "Player Name", "questionItem": { "question": { "required": true, "textQuestion": {} } } }, "location": { "index": 0 } } },
        { "createItem": { "item": { "title": "Player ID", "questionItem": { "question": { "required": true, "textQuestion": {} } } }, "location": { "index": 1 } } },
        { "createItem": { "item": { "title": "I need reservation for", "questionItem": { "question": { "required": true, "choiceQuestion": { "type": "CHECKBOX", "options": [
            { "value": "TD — Training Day (MoE)" },
            { "value": "CD — Construction Day (VP)" },
            { "value": "RD — Research Day (VP)" },
        ] } } } }, "location": { "index": 2 } } },
        { "createItem": { "item": { "title": "How many Firecrystals (FC) do you have?", "questionItem": { "question": { "required": false, "textQuestion": {} } } }, "location": { "index": 3 } } },
        { "createItem": { "item": { "title": "How many Refined Firecrystals (rFC) do you have?", "questionItem": { "question": { "required": false, "textQuestion": {} } } }, "location": { "index": 4 } } },
        { "createItem": { "item": { "title": "How many Training Speedups (TS) do you have?", "questionItem": { "question": { "required": false, "textQuestion": {} } } }, "location": { "index": 5 } } },
        { "createItem": { "item": { "title": "How many Construction Speedups (CS) do you have?", "questionItem": { "question": { "required": false, "textQuestion": {} } } }, "location": { "index": 6 } } },
        { "createItem": { "item": { "title": "How many Research Speedups (RS) do you have?", "questionItem": { "question": { "required": false, "textQuestion": {} } } }, "location": { "index": 7 } } },
        { "createItem": { "item": { "title": "How many General Speedups (GS) do you have?", "questionItem": { "question": { "required": false, "textQuestion": {} } } }, "location": { "index": 8 } } },
        { "createItem": { "item": { "title": "Preferred Time (UTC)", "description": "Preference only — admin assigns the final slot. Example: 09:00–10:00", "questionItem": { "question": { "required": false, "textQuestion": {} } } }, "location": { "index": 9 } } },
    ]);
    google::forms_api(
        cfg,
        db,
        &format!("/{form_id}:batchUpdate"),
        "POST",
        Some(serde_json::json!({ "requests": items })),
    )
    .await?;

    let form_url = format!("https://docs.google.com/forms/d/{form_id}/viewform");
    let (spreadsheet_id, spreadsheet_url, applicant_gid, td_gid, cd_gid, rd_gid) =
        google::create_workspace_spreadsheet(
            cfg,
            db,
            &format!("SVS #{svs} — Reservation Workspace"),
        )
        .await?;

    let key = guild_id.to_string();
    sqlx::query(
        "INSERT INTO ministry_forms (guild_id, category, form_id, sheet_id, sheet_gid, form_url, label, status, svs_number,
                                     workspace_spreadsheet_id, applicant_sheet_id, td_sheet_id, cd_sheet_id, rd_sheet_id)
         VALUES (?, 'svs', ?, ?, '0', ?, ?, 'open', ?, ?, ?, ?, ?, ?)",
    )
    .bind(&key)
    .bind(&form_id)
    .bind(&spreadsheet_id)
    .bind(&form_url)
    .bind(&title)
    .bind(svs)
    .bind(&spreadsheet_id)
    .bind(&applicant_gid)
    .bind(&td_gid)
    .bind(&cd_gid)
    .bind(&rd_gid)
    .execute(&db.pool)
    .await?;

    let form = get_active_form(db, guild_id)
        .await?
        .ok_or_else(|| anyhow!("form was not stored"))?;
    let _ = share_form_sheet(cfg, db, &spreadsheet_id).await;

    Ok((form, spreadsheet_url))
}

/// Share the workspace spreadsheet with anyone-with-link (best effort).
pub async fn share_form_sheet(cfg: &Config, db: &Db, spreadsheet_id: &str) -> Result<()> {
    google::share_anyone_with_link(cfg, db, spreadsheet_id, "writer").await
}

// ------------------------------------------------------------------ sync

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(non_snake_case)]
pub struct FormItem {
    title: Option<String>,
    questionItem: Option<QuestionItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(non_snake_case)]
pub struct QuestionItem {
    question: Option<Question>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Question {
    #[serde(rename = "questionId")]
    question_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FormDefinition {
    items: Option<Vec<FormItem>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FormResponse {
    #[serde(rename = "responseId")]
    response_id: Option<String>,
    #[serde(rename = "lastSubmittedTime")]
    last_submitted_time: Option<String>,
    answers: Option<std::collections::HashMap<String, Answer>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Answer {
    #[serde(rename = "textAnswers")]
    text_answers: Option<TextAnswers>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TextAnswers {
    answers: Option<Vec<AnswerValue>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnswerValue {
    value: Option<String>,
}

fn mapped_row(def: &FormDefinition, response: &FormResponse) -> Option<serde_json::Value> {
    let mut question_id_to_title = std::collections::HashMap::new();
    for item in def.items.iter().flatten() {
        if let (Some(title), Some(qid)) = (
            item.title.clone(),
            item.questionItem
                .as_ref()
                .and_then(|q| q.question.as_ref())
                .and_then(|q| q.question_id.clone()),
        ) {
            question_id_to_title.insert(qid, title);
        }
    }

    let mut by_title: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (qid, answer) in response.answers.as_ref().into_iter().flatten() {
        let title = question_id_to_title
            .get(qid)
            .cloned()
            .unwrap_or_else(|| qid.clone());
        let texts: Vec<String> = answer
            .text_answers
            .as_ref()
            .and_then(|t| t.answers.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| a.value)
            .collect();
        if !texts.is_empty() {
            by_title.insert(title, texts.join(", "));
        }
    }

    let events = by_title
        .get("I need reservation for")
        .cloned()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let want_td = (events.contains("td") || events.contains("training")) as i64;
    let want_cd = (events.contains("cd") || events.contains("construction")) as i64;
    let want_rd = (events.contains("rd") || events.contains("research")) as i64;

    let fid: String = by_title
        .get("Player ID")
        .cloned()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    if fid.is_empty() {
        return None;
    }

    Some(serde_json::json!({
        "applicantName": by_title.get("Player Name").cloned().unwrap_or_default(),
        "fid": fid,
        "fc": by_title.get("How many Firecrystals (FC) do you have?").cloned().unwrap_or_default(),
        "rfc": by_title.get("How many Refined Firecrystals (rFC) do you have?").cloned().unwrap_or_default(),
        "ts": by_title.get("How many Training Speedups (TS) do you have?").cloned().unwrap_or_default(),
        "cs": by_title.get("How many Construction Speedups (CS) do you have?").cloned().unwrap_or_default(),
        "rs": by_title.get("How many Research Speedups (RS) do you have?").cloned().unwrap_or_default(),
        "gs": by_title.get("How many General Speedups (GS) do you have?").cloned().unwrap_or_default(),
        "preferredTime": by_title.get("Preferred Time (UTC)").cloned().unwrap_or_default(),
        "wantTd": want_td,
        "wantCd": want_cd,
        "wantRd": want_rd,
        "sheetRowKey": response.response_id.clone().unwrap_or_default(),
    }))
}

pub async fn sync_form(cfg: &Config, db: &Db, guild_id: u64) -> Result<(usize, usize)> {
    let Some(form) = get_active_form(db, guild_id).await? else {
        return Ok((0, 0));
    };
    let Some(form_id) = &form.form_id else {
        return Ok((0, 0));
    };

    let def_data = google::forms_api(cfg, db, &format!("/{form_id}"), "GET", None).await?;
    let def: FormDefinition =
        serde_json::from_value(def_data).unwrap_or(FormDefinition { items: None });
    let responses_data =
        google::forms_api(cfg, db, &format!("/{form_id}/responses"), "GET", None).await?;
    let responses: Vec<FormResponse> = responses_data
        .get("responses")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();

    let mut added = 0usize;
    let mut applicant_rows: Vec<Vec<String>> = Vec::new();
    let key = guild_id.to_string();

    for response in &responses {
        let Some(mapped) = mapped_row(&def, response) else {
            continue;
        };
        let fid = mapped["fid"].as_str().unwrap_or_default().to_string();
        let sheet_key = mapped["sheetRowKey"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let name = mapped["applicantName"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let inserted = sqlx::query(
            "INSERT INTO ministry_form_submissions (guild_id, form_id, category, svs_number, sheet_row_key, applicant_name, fid, speedups, rfc, fc, ts, cs, rs, gs, preferred_time, want_td, want_cd, want_rd, app_status, raw_json)
             VALUES (?, ?, 'svs', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?)
             ON CONFLICT(guild_id, form_id, sheet_row_key) DO NOTHING",
        )
        .bind(&key)
        .bind(form.id)
        .bind(form.svs_number)
        .bind(&sheet_key)
        .bind(&name)
        .bind(&fid)
        .bind(Option::<String>::None) // speedups (legacy aggregate) unused in unified form
        .bind(mapped["rfc"].as_str().unwrap_or_default())
        .bind(mapped["fc"].as_str().unwrap_or_default())
        .bind(mapped["ts"].as_str().unwrap_or_default())
        .bind(mapped["cs"].as_str().unwrap_or_default())
        .bind(mapped["rs"].as_str().unwrap_or_default())
        .bind(mapped["gs"].as_str().unwrap_or_default())
        .bind(mapped["preferredTime"].as_str().unwrap_or_default())
        .bind(mapped["wantTd"].as_i64().unwrap_or(0))
        .bind(mapped["wantCd"].as_i64().unwrap_or(0))
        .bind(mapped["wantRd"].as_i64().unwrap_or(0))
        .bind(serde_json::to_string(&mapped).unwrap_or_default())
        .execute(&db.pool)
        .await?;

        if inserted.rows_affected() > 0 {
            added += 1;
            applicant_rows.push(vec![
                name.clone(),
                fid.clone(),
                mapped["preferredTime"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                mapped["fc"].as_str().unwrap_or_default().to_string(),
                mapped["rfc"].as_str().unwrap_or_default().to_string(),
                mapped["ts"].as_str().unwrap_or_default().to_string(),
                mapped["cs"].as_str().unwrap_or_default().to_string(),
                mapped["rs"].as_str().unwrap_or_default().to_string(),
                mapped["gs"].as_str().unwrap_or_default().to_string(),
                if mapped["wantTd"].as_i64().unwrap_or(0) != 0 {
                    "✅".into()
                } else {
                    String::new()
                },
                if mapped["wantCd"].as_i64().unwrap_or(0) != 0 {
                    "✅".into()
                } else {
                    String::new()
                },
                if mapped["wantRd"].as_i64().unwrap_or(0) != 0 {
                    "✅".into()
                } else {
                    String::new()
                },
            ]);
            register_registry(db, guild_id, &fid, &name).await.ok();
        }
    }

    // Live-write applicant rows to the workspace spreadsheet.
    if !applicant_rows.is_empty() {
        if let Some(spreadsheet) = &form.workspace_spreadsheet_id {
            for row in applicant_rows {
                let _ =
                    google::append_values(cfg, db, spreadsheet, "'Applicant Sheet'!A:L", vec![row])
                        .await;
            }
        }
    }

    let _ = log_sync(
        db,
        Some(guild_id),
        "form_sync",
        &format!("Unified form {}: +{}", form.id, added),
        true,
    )
    .await;
    let _ = sqlx::query(
        "UPDATE ministry_settings SET last_sheet_sync_at = datetime('now') WHERE guild_id = ?",
    )
    .bind(&key)
    .execute(&db.pool)
    .await;

    Ok((added, responses.len()))
}

pub async fn register_registry(db: &Db, guild_id: u64, fid: &str, name: &str) -> Result<()> {
    let key = guild_id.to_string();
    if fid.is_empty() || name.trim().is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO ministry_player_registry (guild_id, fid, player_name, source, updated_at)
         VALUES (?, ?, ?, 'svs_form', datetime('now'))
         ON CONFLICT(guild_id, fid) DO UPDATE SET
            player_name = excluded.player_name,
            updated_at = datetime('now')",
    )
    .bind(&key)
    .bind(fid)
    .bind(name)
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn log_sync(
    db: &Db,
    guild_id: Option<u64>,
    kind: &str,
    message: &str,
    ok: bool,
) -> Result<()> {
    sqlx::query("INSERT INTO ministry_sync_log (guild_id, kind, message, ok) VALUES (?, ?, ?, ?)")
        .bind(guild_id.map(|g| g.to_string()))
        .bind(kind)
        .bind(&message[..message.len().min(500)])
        .bind(ok as i64)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// The legacy "write pending/failed" path — surface recent failures.
pub async fn sync_log(db: &Db, guild_id: u64, limit: i64) -> Result<Vec<(String, String, bool)>> {
    let key = guild_id.to_string();
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, message, ok FROM ministry_sync_log WHERE guild_id = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(&key)
    .bind(limit)
    .fetch_all(&db.pool)
    .await
    .unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|(kind, message, ok)| (kind, message, ok != 0))
        .collect())
}

// --------------------------------------------------------------- review

pub async fn list_forms(db: &Db, guild_id: u64, limit: i64) -> Result<Vec<MinistryForm>> {
    let key = guild_id.to_string();
    let rows = sqlx::query_as::<_, MinistryForm>(
        "SELECT id, guild_id, category, form_id, form_url, label, status, svs_number, workspace_spreadsheet_id, created_at
           FROM ministry_forms
          WHERE guild_id = ? AND category = 'svs'
          ORDER BY id DESC LIMIT ?",
    )
    .bind(&key)
    .bind(limit)
    .fetch_all(&db.pool)
    .await?;
    Ok(rows)
}

pub async fn list_submissions(
    db: &Db,
    guild_id: u64,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<Submission>> {
    let key = guild_id.to_string();
    let rows = match status {
        Some(status) => {
            if status == "pending" {
                sqlx::query_as::<_, Submission>(
                    "SELECT id, form_id, svs_number, applicant_name, fid, fc, rfc, ts, cs, rs, gs, preferred_time, want_td, want_cd, want_rd, app_status, created_at
                       FROM ministry_form_submissions
                      WHERE guild_id = ? AND app_status IN ('pending', 'imported')
                      ORDER BY id DESC LIMIT ?",
                )
                .bind(&key)
                .bind(limit)
                .fetch_all(&db.pool)
                .await?
            } else {
                sqlx::query_as::<_, Submission>(
                    "SELECT id, form_id, svs_number, applicant_name, fid, fc, rfc, ts, cs, rs, gs, preferred_time, want_td, want_cd, want_rd, app_status, created_at
                       FROM ministry_form_submissions
                      WHERE guild_id = ? AND app_status = ?
                      ORDER BY id DESC LIMIT ?",
                )
                .bind(&key)
                .bind(status)
                .bind(limit)
                .fetch_all(&db.pool)
                .await?
            }
        }
        None => {
            sqlx::query_as::<_, Submission>(
                "SELECT id, form_id, svs_number, applicant_name, fid, fc, rfc, ts, cs, rs, gs, preferred_time, want_td, want_cd, want_rd, app_status, created_at
                   FROM ministry_form_submissions
                  WHERE guild_id = ?
                  ORDER BY id DESC LIMIT ?",
            )
            .bind(&key)
            .bind(limit)
            .fetch_all(&db.pool)
            .await?
        }
    };
    Ok(rows)
}

pub async fn review_submission(
    db: &Db,
    guild_id: u64,
    id: i64,
    approve: bool,
    reviewer: u64,
) -> Result<bool> {
    let key = guild_id.to_string();
    let result = sqlx::query(
        "UPDATE ministry_form_submissions
            SET app_status = ?, reviewed_by = ?, reviewed_at = (strftime('%s','now'))
          WHERE guild_id = ? AND id = ?",
    )
    .bind(if approve { "approved" } else { "rejected" })
    .bind(reviewer.to_string())
    .bind(&key)
    .bind(id)
    .execute(&db.pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn count_pending(db: &Db, guild_id: u64) -> Result<i64> {
    let key = guild_id.to_string();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ministry_form_submissions WHERE guild_id = ? AND app_status IN ('pending', 'imported')",
    )
    .bind(&key)
    .fetch_one(&db.pool)
    .await
    .unwrap_or(0);
    Ok(count)
}

pub async fn stats(db: &Db, guild_id: u64) -> Result<(i64, i64, i64)> {
    let key = guild_id.to_string();
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ministry_form_submissions WHERE guild_id = ?")
            .bind(&key)
            .fetch_one(&db.pool)
            .await
            .unwrap_or(0);
    let approved: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ministry_form_submissions WHERE guild_id = ? AND app_status = 'approved'")
        .bind(&key)
        .fetch_one(&db.pool)
        .await
        .unwrap_or(0);
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ministry_form_submissions WHERE guild_id = ? AND app_status IN ('pending', 'imported')")
        .bind(&key)
        .fetch_one(&db.pool)
        .await
        .unwrap_or(0);
    Ok((total, approved, pending))
}

// ------------------------------------------------------------------ csv

pub async fn export_csv(db: &Db, guild_id: u64) -> Result<String> {
    let subs = list_submissions(db, guild_id, None, 5000).await?;
    let mut out = String::from(
        "id,svs_number,applicant_name,fid,fc,rfc,ts,cs,rs,gs,preferred_time,events,app_status,created_at\n",
    );
    for s in subs {
        let events = s.events();
        let cell = |v: &str| format!("\"{}\"", v.replace('"', "\"\""));
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            s.id,
            s.svs_number.map(|v| v.to_string()).unwrap_or_default(),
            cell(s.applicant_name.as_deref().unwrap_or("")),
            cell(&s.fid),
            cell(s.fc.as_deref().unwrap_or("")),
            cell(s.rfc.as_deref().unwrap_or("")),
            cell(s.ts.as_deref().unwrap_or("")),
            cell(s.cs.as_deref().unwrap_or("")),
            cell(s.rs.as_deref().unwrap_or("")),
            cell(s.gs.as_deref().unwrap_or("")),
            cell(s.preferred_time.as_deref().unwrap_or("")),
            cell(&events),
            cell(&s.app_status),
            cell(&s.created_at),
        ));
    }
    Ok(out)
}

/// Excel (.xlsx) export — same rows as the CSV, with formatting.
pub async fn export_xlsx(db: &Db, guild_id: u64) -> Result<Vec<u8>> {
    use rust_xlsxwriter::{Format, FormatBorder, Workbook};

    let subs = list_submissions(db, guild_id, None, 5000).await?;
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet().set_name("Ministry")?;

    let header = Format::new()
        .set_bold()
        .set_background_color(rust_xlsxwriter::Color::RGB(0x15202B))
        .set_font_color(rust_xlsxwriter::Color::RGB(0xF1F5F9))
        .set_border(FormatBorder::Thin);

    let headers = [
        "id",
        "svs_number",
        "applicant_name",
        "fid",
        "fc",
        "rfc",
        "ts",
        "cs",
        "rs",
        "gs",
        "preferred_time",
        "events",
        "app_status",
        "created_at",
    ];
    for (col, label) in headers.iter().enumerate() {
        sheet.write_with_format(0, col as u16, *label, &header)?;
    }

    for (row_index, s) in subs.iter().enumerate() {
        let row = (row_index + 1) as u32;
        let events = s.events();
        sheet.write_number(row, 0, s.id as f64)?;
        if let Some(svs) = s.svs_number {
            sheet.write_number(row, 1, svs as f64)?;
        }
        let text = |value: Option<&str>| value.unwrap_or("").to_string();
        sheet.write_string(row, 2, text(s.applicant_name.as_deref()))?;
        sheet.write_string(row, 3, &s.fid)?;
        sheet.write_string(row, 4, text(s.fc.as_deref()))?;
        sheet.write_string(row, 5, text(s.rfc.as_deref()))?;
        sheet.write_string(row, 6, text(s.ts.as_deref()))?;
        sheet.write_string(row, 7, text(s.cs.as_deref()))?;
        sheet.write_string(row, 8, text(s.rs.as_deref()))?;
        sheet.write_string(row, 9, text(s.gs.as_deref()))?;
        sheet.write_string(row, 10, text(s.preferred_time.as_deref()))?;
        sheet.write_string(row, 11, &events)?;
        sheet.write_string(row, 12, &s.app_status)?;
        sheet.write_string(row, 13, &s.created_at)?;
    }

    let widths = [
        8.0, 13.0, 22.0, 16.0, 10.0, 10.0, 18.0, 18.0, 18.0, 18.0, 22.0, 12.0, 12.0, 20.0,
    ];
    for (col, width) in widths.iter().enumerate() {
        sheet.set_column_width(col as u16, *width)?;
    }
    sheet.set_freeze_panes(1, 0)?;
    sheet.autofilter(0, 0, subs.len() as u32, (headers.len() - 1) as u16)?;

    let buffer = workbook.save_to_buffer()?;
    Ok(buffer)
}

/// Render a shareable roster image (dark card) of the first `limit` rows.
pub async fn export_png(db: &Db, guild_id: u64, limit: usize) -> Result<Vec<u8>> {
    let subs = list_submissions(db, guild_id, None, (limit + 1) as i64).await?;
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ministry_form_submissions WHERE guild_id = ?")
            .bind(guild_id.to_string())
            .fetch_one(&db.pool)
            .await?;
    let shown = subs.iter().take(limit);

    let columns = vec![
        TableColumn {
            label: "ID".into(),
            width: 46.0,
        },
        TableColumn {
            label: "Name".into(),
            width: 150.0,
        },
        TableColumn {
            label: "FID".into(),
            width: 120.0,
        },
        TableColumn {
            label: "Events".into(),
            width: 90.0,
        },
        TableColumn {
            label: "FC".into(),
            width: 70.0,
        },
        TableColumn {
            label: "RFC".into(),
            width: 70.0,
        },
        TableColumn {
            label: "Time".into(),
            width: 150.0,
        },
        TableColumn {
            label: "Status".into(),
            width: 90.0,
        },
    ];

    let rows = shown
        .map(|s| {
            vec![
                s.id.to_string(),
                s.applicant_name.clone().unwrap_or_else(|| "?".to_string()),
                s.fid.clone(),
                s.events(),
                s.fc.clone().unwrap_or_default(),
                s.rfc.clone().unwrap_or_default(),
                s.preferred_time.clone().unwrap_or_default(),
                s.app_status.clone(),
            ]
        })
        .collect::<Vec<_>>();

    let footer = if total > rows.len() as i64 {
        format!(
            "+{} more rows — use Export Excel or Export CSV for everything",
            total - rows.len() as i64
        )
    } else {
        format!("{total} submission(s)")
    };

    crate::cards::render_table_card(&TableCard {
        title: "SVS Ministry — Submissions".to_string(),
        subtitle: Some(format!("Guild {guild_id}")),
        columns,
        rows,
        footer: Some(footer),
    })
}

static SYNC_GATE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
fn sync_gate() -> &'static tokio::sync::Semaphore {
    SYNC_GATE.get_or_init(|| tokio::sync::Semaphore::new(1))
}

/// Background sync — one guild at a time, best effort.
pub fn spawn_sync_loop(ctx: serenity::Context, data: crate::state::Data) {
    tokio::spawn(async move {
        // Small delay so the bot finishes connecting first.
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            let _permit = match sync_gate().try_acquire() {
                Ok(permit) => permit,
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                    continue;
                }
            };
            match sync_all(&ctx, &data).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(forms = n, "ministry: synced forms");
                    }
                }
                Err(err) => {
                    tracing::debug!(error = %crate::error::display_anyhow(&err), "ministry sync failed")
                }
            }
            drop(_permit);
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

async fn sync_all(ctx: &serenity::Context, data: &Data) -> Result<usize> {
    let _ = ctx;
    if !data.cfg.google_configured() {
        return Ok(0);
    }
    let status = google::status(&data.cfg, &data.db).await;
    if !status.connected {
        return Ok(0);
    }

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT guild_id, id FROM ministry_forms WHERE category = 'svs' AND status IN ('open','active')",
    )
    .fetch_all(&data.db.pool)
    .await?;

    let mut synced = 0usize;
    for (guild_id, _id) in rows {
        let Ok(guild) = guild_id.parse::<u64>() else {
            continue;
        };
        match sync_form(&data.cfg, &data.db, guild).await {
            Ok((added, _)) => {
                if added > 0 {
                    synced += 1;
                }
            }
            Err(err) => {
                tracing::debug!(%guild, error = %err, "ministry: form sync failed");
                let _ = log_sync(&data.db, Some(guild), "form_sync", &err.to_string(), false).await;
            }
        }
    }
    Ok(synced)
}
