//! The message library: groups of reusable embed/plain messages.
//!
//! Uses the legacy `message_groups`, `group_admin_access`, `message_library`
//! and `message_library_embeds` tables verbatim.

use sqlx::FromRow;

use super::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Bot owner only.
    Owner,
    /// Creator plus explicitly granted admins.
    Selected,
    /// Every admin of the guild.
    All,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Selected => "selected",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct MessageGroup {
    pub id: i64,
    pub name: String,
    pub guild_id: Option<String>,
    pub visibility: String,
    pub created_by: String,
    pub created_at: String,
}

impl MessageGroup {
    pub fn visibility(&self) -> Visibility {
        match self.visibility.as_str() {
            "selected" => Visibility::Selected,
            "all" => Visibility::All,
            _ => Visibility::Owner,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct LibraryMessage {
    pub id: i64,
    pub group_id: i64,
    pub name: String,
    pub is_embed: i64,
    pub plain_text: Option<String>,
    pub mention_type: Option<String>,
    pub created_by: String,
    pub created_at: String,
}

impl LibraryMessage {
    pub fn is_embed(&self) -> bool {
        self.is_embed != 0
    }
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct MessageEmbed {
    pub library_id: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub color: Option<i64>,
    pub image_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub footer: Option<String>,
    pub author: Option<String>,
    pub images: Option<String>,
}

// ----------------------------------------------------------------- groups

/// Groups the invoking admin can see. The creator and owner-only groups follow
/// the legacy rules: owner sees all, creator sees their own, granted admins
/// see selected groups, everyone sees 'all' groups.
pub async fn visible_groups(
    db: &Db,
    guild_id: u64,
    admin_id: u64,
    is_owner: bool,
) -> sqlx::Result<Vec<MessageGroup>> {
    if is_owner {
        return sqlx::query_as::<_, MessageGroup>(
            "SELECT * FROM message_groups WHERE guild_id IS NULL OR guild_id = ? ORDER BY name",
        )
        .bind(guild_id.to_string())
        .fetch_all(&db.pool)
        .await;
    }

    sqlx::query_as::<_, MessageGroup>(
        "SELECT DISTINCT g.* FROM message_groups g
         LEFT JOIN group_admin_access a ON a.group_id = g.id AND a.admin_id = ?
         WHERE g.guild_id = ?
           AND (g.created_by = ? OR g.visibility = 'all'
                OR (g.visibility = 'selected' AND a.admin_id IS NOT NULL))
         ORDER BY g.name",
    )
    .bind(admin_id.to_string())
    .bind(guild_id.to_string())
    .bind(admin_id.to_string())
    .fetch_all(&db.pool)
    .await
}

pub async fn create_group(
    db: &Db,
    guild_id: u64,
    name: &str,
    visibility: Visibility,
    created_by: u64,
) -> sqlx::Result<i64> {
    let result = sqlx::query(
        "INSERT INTO message_groups (name, guild_id, visibility, created_by) VALUES (?, ?, ?, ?)",
    )
    .bind(name.trim())
    .bind(guild_id.to_string())
    .bind(visibility.as_str())
    .bind(created_by.to_string())
    .execute(&db.pool)
    .await?;
    Ok(result.last_insert_rowid())
}

pub async fn delete_group(db: &Db, guild_id: u64, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM message_groups WHERE id = ? AND guild_id = ?")
        .bind(id)
        .bind(guild_id.to_string())
        .execute(&db.pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_group_visibility(
    db: &Db,
    guild_id: u64,
    id: i64,
    visibility: Visibility,
) -> sqlx::Result<bool> {
    let result =
        sqlx::query("UPDATE message_groups SET visibility = ? WHERE id = ? AND guild_id = ?")
            .bind(visibility.as_str())
            .bind(id)
            .bind(guild_id.to_string())
            .execute(&db.pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn grant_admin(db: &Db, group_id: i64, admin_id: u64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO group_admin_access (group_id, admin_id) VALUES (?, ?)
         ON CONFLICT(group_id, admin_id) DO NOTHING",
    )
    .bind(group_id)
    .bind(admin_id.to_string())
    .execute(&db.pool)
    .await?;
    Ok(())
}

// --------------------------------------------------------------- messages

pub async fn list_messages(db: &Db, group_id: i64) -> sqlx::Result<Vec<LibraryMessage>> {
    sqlx::query_as::<_, LibraryMessage>(
        "SELECT * FROM message_library WHERE group_id = ? ORDER BY name",
    )
    .bind(group_id)
    .fetch_all(&db.pool)
    .await
}

#[derive(Debug, Clone, Default)]
pub struct MessageInput<'a> {
    pub name: &'a str,
    pub is_embed: bool,
    pub plain_text: Option<&'a str>,
    pub mention_type: Option<&'a str>,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub color: Option<i64>,
    pub footer: Option<&'a str>,
    pub author: Option<&'a str>,
    pub image_url: Option<&'a str>,
    pub thumbnail_url: Option<&'a str>,
}

pub async fn upsert_message(
    db: &Db,
    group_id: i64,
    input: &MessageInput<'_>,
    created_by: u64,
) -> anyhow::Result<i64> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM message_library WHERE group_id = ? AND name = ?")
            .bind(group_id)
            .bind(input.name.trim())
            .fetch_optional(&db.pool)
            .await?;

    let id = if let Some((id,)) = row {
        sqlx::query(
            "UPDATE message_library SET is_embed = ?, plain_text = ?, mention_type = ?, updated_at = datetime('now')
             WHERE id = ?",
        )
        .bind(input.is_embed as i64)
        .bind(input.plain_text.map(str::trim))
        .bind(input.mention_type.map(str::trim))
        .bind(id)
        .execute(&db.pool)
        .await?;
        id
    } else {
        let result = sqlx::query(
            "INSERT INTO message_library (group_id, name, is_embed, plain_text, mention_type, created_by)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(input.name.trim())
        .bind(input.is_embed as i64)
        .bind(input.plain_text.map(str::trim))
        .bind(input.mention_type.map(str::trim))
        .bind(created_by.to_string())
        .execute(&db.pool)
        .await?;
        result.last_insert_rowid()
    };

    // Upsert the embed payload even for plain messages (cleared to keep it tidy).
    sqlx::query(
        "INSERT INTO message_library_embeds
             (library_id, title, description, color, image_url, thumbnail_url, footer, author, images)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)
         ON CONFLICT(library_id) DO UPDATE SET
             title = excluded.title,
             description = excluded.description,
             color = excluded.color,
             image_url = excluded.image_url,
             thumbnail_url = excluded.thumbnail_url,
             footer = excluded.footer,
             author = excluded.author",
    )
    .bind(id)
    .bind(if input.is_embed { input.title.map(str::trim) } else { None })
    .bind(if input.is_embed { input.description.map(str::trim) } else { None })
    .bind(input.color)
    .bind(input.image_url.map(str::trim))
    .bind(input.thumbnail_url.map(str::trim))
    .bind(input.footer.map(str::trim))
    .bind(input.author.map(str::trim))
    .execute(&db.pool)
    .await?;

    Ok(id)
}

pub async fn get_message(
    db: &Db,
    group_id: i64,
    name: &str,
) -> sqlx::Result<Option<(LibraryMessage, MessageEmbed)>> {
    let Some(message) = sqlx::query_as::<_, LibraryMessage>(
        "SELECT * FROM message_library WHERE group_id = ? AND name = ?",
    )
    .bind(group_id)
    .bind(name.trim())
    .fetch_optional(&db.pool)
    .await?
    else {
        return Ok(None);
    };

    let embed = sqlx::query_as::<_, MessageEmbed>(
        "SELECT * FROM message_library_embeds WHERE library_id = ?",
    )
    .bind(message.id)
    .fetch_optional(&db.pool)
    .await?
    .unwrap_or(MessageEmbed {
        library_id: message.id,
        ..Default::default()
    });

    Ok(Some((message, embed)))
}

pub async fn group_message_count(db: &Db, group_id: i64) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM message_library WHERE group_id = ?")
        .bind(group_id)
        .fetch_one(&db.pool)
        .await?;
    Ok(n)
}
