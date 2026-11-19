//! Storage for messages carrying a "Translate" button.

use sqlx::FromRow;

use super::Db;

#[derive(Debug, Clone, Default, FromRow)]
pub struct TranslatableMessage {
    pub message_id: Option<String>,
    pub guild_id: Option<String>,
    pub channel_id: Option<String>,
    pub kind: Option<String>,
    pub embed_json: Option<String>,
    pub content: Option<String>,
    pub owner_id: Option<String>,
}

pub struct NewTranslatable<'a> {
    pub guild_id: Option<u64>,
    pub channel_id: Option<u64>,
    pub kind: &'a str,
    /// Raw embed JSON (serde_json::Value of the embed), if any.
    pub embed_json: Option<String>,
    pub content: Option<String>,
    pub owner_id: Option<u64>,
}

pub async fn register(db: &Db, message_id: u64, input: NewTranslatable<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO translatable_messages (message_id, guild_id, channel_id, kind, embed_json, content, owner_id)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(message_id) DO UPDATE SET
            guild_id = excluded.guild_id, channel_id = excluded.channel_id,
            kind = excluded.kind, embed_json = excluded.embed_json,
            content = excluded.content, owner_id = excluded.owner_id",
    )
    .bind(message_id.to_string())
    .bind(input.guild_id.map(|id| id.to_string()))
    .bind(input.channel_id.map(|id| id.to_string()))
    .bind(input.kind)
    .bind(input.embed_json)
    .bind(input.content)
    .bind(input.owner_id.map(|id| id.to_string()))
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub async fn get(db: &Db, message_id: u64) -> sqlx::Result<Option<TranslatableMessage>> {
    let row = sqlx::query_as::<_, TranslatableMessage>(
        "SELECT message_id, guild_id, channel_id, kind, embed_json, content, owner_id
         FROM translatable_messages WHERE message_id = ?",
    )
    .bind(message_id.to_string())
    .fetch_optional(&db.pool)
    .await?;
    Ok(row)
}
