use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::AppError;

fn default_true() -> bool {
    true
}

/// Core status page entity.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StatusPage {
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub is_public: i64,
    pub created_at: String,
    #[serde(default = "default_true")]
    #[sqlx(default)]
    pub allow_indexing: bool,
    #[sqlx(default)]
    pub meta_title: Option<String>,
    #[sqlx(default)]
    pub meta_description: Option<String>,
    #[sqlx(default)]
    pub og_image_url: Option<String>,
    #[sqlx(default)]
    pub canonical_url: Option<String>,
}

/// Status page entity including count of assigned monitors.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StatusPageWithCount {
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub is_public: i64,
    pub created_at: String,
    pub monitor_count: i64,
    #[serde(default = "default_true")]
    #[sqlx(default)]
    pub allow_indexing: bool,
    #[sqlx(default)]
    pub meta_title: Option<String>,
    #[sqlx(default)]
    pub meta_description: Option<String>,
    #[sqlx(default)]
    pub og_image_url: Option<String>,
    #[sqlx(default)]
    pub canonical_url: Option<String>,
}

/// Monitor assignment configuration for a status page.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StatusPageMonitorAssignment {
    pub monitor_id: String,
    pub group_name: String,
    pub display_order: i64,
}

/// Detailed status page view including assigned monitor metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusPageDetail {
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub created_at: String,
    pub monitors: Vec<StatusPageMonitorAssignment>,
    #[serde(default = "default_true")]
    pub allow_indexing: bool,
    pub meta_title: Option<String>,
    pub meta_description: Option<String>,
    pub og_image_url: Option<String>,
    pub canonical_url: Option<String>,
}

/// Monitor row returned for public status page rendering.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PublicStatusPageMonitorRow {
    pub id: String,
    pub name: String,
    pub url: String,
    pub group_name: String,
    pub display_order: i64,
}

/// Query monitors assigned to a public status page.
/// Performs an INNER JOIN between `status_page_monitors` and `monitors`.
/// If `slug == "default"` and no monitors are explicitly assigned,
/// falls back to querying all active monitors.
/// For custom slugs, returns strictly the assigned monitors.
#[allow(dead_code)]
pub async fn get_public_status_page_monitors(
    pool: &SqlitePool,
    slug: &str,
) -> Result<Vec<PublicStatusPageMonitorRow>, AppError> {
    let assigned = sqlx::query_as::<_, PublicStatusPageMonitorRow>(
        r#"
        SELECT 
            m.id, m.name, m.url,
            spm.group_name, spm.display_order
        FROM status_page_monitors spm
        INNER JOIN monitors m ON m.id = spm.monitor_id
        WHERE LOWER(spm.status_page_slug) = LOWER(?)
        ORDER BY spm.display_order ASC, m.name ASC
        "#,
    )
    .bind(slug)
    .fetch_all(pool)
    .await?;

    if assigned.is_empty() && slug == "default" {
        let all_active = sqlx::query_as::<_, PublicStatusPageMonitorRow>(
            r#"
            SELECT 
                id, name, url, 
                'Services' AS group_name, 
                0 AS display_order
            FROM monitors 
            WHERE is_public = 1 AND is_active = 1
            ORDER BY name ASC
            "#,
        )
        .fetch_all(pool)
        .await?;
        return Ok(all_active);
    }

    Ok(assigned)
}

/// Fetch public status page metadata and assigned monitors.
#[allow(dead_code)]
pub async fn get_public_status_page(
    pool: &SqlitePool,
    slug: &str,
) -> Result<Option<(StatusPage, Vec<PublicStatusPageMonitorRow>)>, AppError> {
    let page = match get_status_page(pool, slug).await? {
        Some(p) => p,
        None => return Ok(None),
    };
    if page.is_public == 0 {
        return Ok(None);
    }
    let monitors = get_public_status_page_monitors(pool, slug).await?;
    Ok(Some((page, monitors)))
}

/// Query all status pages.
pub async fn get_all_status_pages(pool: &SqlitePool) -> Result<Vec<StatusPage>, AppError> {
    let pages = sqlx::query_as::<_, StatusPage>(
        "SELECT slug, title, description, is_public, created_at FROM status_pages ORDER BY created_at ASC"
    )
    .fetch_all(pool)
    .await?;

    Ok(pages)
}

/// List all status pages with their assigned monitor counts.
pub async fn list_status_pages(pool: &SqlitePool) -> Result<Vec<StatusPageWithCount>, AppError> {
    let pages = sqlx::query_as::<_, StatusPageWithCount>(
        r#"
        SELECT 
            sp.slug,
            sp.title,
            sp.description,
            sp.is_public,
            sp.created_at,
            COUNT(spm.monitor_id) AS monitor_count,
            sp.allow_indexing,
            sp.meta_title,
            sp.meta_description,
            sp.og_image_url,
            sp.canonical_url
        FROM status_pages sp
        LEFT JOIN status_page_monitors spm ON sp.slug = spm.status_page_slug
        GROUP BY sp.slug
        ORDER BY (sp.slug = 'default') DESC, sp.title ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(pages)
}

/// Get a single status page by slug.
pub async fn get_status_page(
    pool: &SqlitePool,
    slug: &str,
) -> Result<Option<StatusPage>, AppError> {
    let page = sqlx::query_as::<_, StatusPage>(
        "SELECT slug, title, description, is_public, created_at, allow_indexing, meta_title, meta_description, og_image_url, canonical_url FROM status_pages WHERE LOWER(slug) = LOWER(?)",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await?;

    Ok(page)
}

/// Get monitor assignments for a status page.
pub async fn get_status_page_monitors(
    pool: &SqlitePool,
    slug: &str,
) -> Result<Vec<StatusPageMonitorAssignment>, AppError> {
    let monitors = sqlx::query_as::<_, StatusPageMonitorAssignment>(
        r#"
        SELECT monitor_id, group_name, display_order 
        FROM status_page_monitors 
        WHERE status_page_slug = ? 
        ORDER BY display_order ASC, group_name ASC
        "#,
    )
    .bind(slug)
    .fetch_all(pool)
    .await?;

    Ok(monitors)
}

/// Validate slug format (alphanumeric, dashes, underscores).
pub fn validate_slug(slug: &str) -> Result<(), AppError> {
    let s = slug.trim();
    if s.is_empty() {
        return Err(AppError::BadRequest("Slug cannot be empty".to_string()));
    }
    if s.len() > 64 {
        return Err(AppError::BadRequest("Slug cannot exceed 64 characters".to_string()));
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(AppError::BadRequest(
            "Slug must contain only alphanumeric characters, dashes, or underscores".to_string(),
        ));
    }
    Ok(())
}

/// Create a new status page.
pub async fn create_status_page(
    pool: &SqlitePool,
    slug: &str,
    title: &str,
    description: Option<&str>,
    is_public: bool,
    monitors: Option<Vec<StatusPageMonitorAssignment>>,
    allow_indexing: bool,
    meta_title: Option<&str>,
    meta_description: Option<&str>,
    og_image_url: Option<&str>,
    canonical_url: Option<&str>,
) -> Result<StatusPage, AppError> {
    validate_slug(slug)?;
    let title_clean = title.trim();
    if title_clean.is_empty() {
        return Err(AppError::BadRequest("Title cannot be empty".to_string()));
    }

    let is_pub_int = if is_public { 1i64 } else { 0i64 };
    let allow_idx_int = if allow_indexing { 1i64 } else { 0i64 };

    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO status_pages (slug, title, description, is_public, allow_indexing, meta_title, meta_description, og_image_url, canonical_url) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(slug.trim())
    .bind(title_clean)
    .bind(description.map(|d| d.trim()))
    .bind(is_pub_int)
    .bind(allow_idx_int)
    .bind(meta_title.map(|m| m.trim()))
    .bind(meta_description.map(|m| m.trim()))
    .bind(og_image_url.map(|m| m.trim()))
    .bind(canonical_url.map(|m| m.trim()))
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(db_err) = &e {
            if db_err.is_unique_violation() {
                return AppError::BadRequest(format!("Status page with slug '{slug}' already exists"));
            }
        }
        AppError::Database(e)
    })?;

    let mut inserted_count = 0;
    if let Some(assignments) = monitors {
        sqlx::query("DELETE FROM status_page_monitors WHERE status_page_slug = ?")
            .bind(slug.trim())
            .execute(&mut *tx)
            .await?;

        for a in assignments {
            let grp = if a.group_name.trim().is_empty() {
                "Services"
            } else {
                a.group_name.trim()
            };

            sqlx::query(
                r#"
                INSERT INTO status_page_monitors (status_page_slug, monitor_id, group_name, display_order)
                VALUES (?, ?, ?, ?)
                "#,
            )
            .bind(slug.trim())
            .bind(&a.monitor_id)
            .bind(grp)
            .bind(a.display_order)
            .execute(&mut *tx)
            .await?;
            inserted_count += 1;
        }
    }

    tx.commit().await?;

    tracing::info!(slug = %slug, count = inserted_count, "Persisted status page monitor assignments");

    let page = get_status_page(pool, slug.trim())
        .await?
        .ok_or_else(|| AppError::NotFound("Status page not found after creation".to_string()))?;

    Ok(page)
}

/// Update an existing status page, and optionally its monitor assignments.
pub async fn update_status_page(
    pool: &SqlitePool,
    slug: &str,
    title: &str,
    description: Option<&str>,
    is_public: bool,
    monitors: Option<Vec<StatusPageMonitorAssignment>>,
    allow_indexing: bool,
    meta_title: Option<&str>,
    meta_description: Option<&str>,
    og_image_url: Option<&str>,
    canonical_url: Option<&str>,
) -> Result<StatusPageDetail, AppError> {
    let title_clean = title.trim();
    if title_clean.is_empty() {
        return Err(AppError::BadRequest("Title cannot be empty".to_string()));
    }

    // Verify status page exists
    let _ = get_status_page(pool, slug)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Status page '{slug}' not found")))?;

    let is_pub_int = if is_public { 1i64 } else { 0i64 };
    let allow_idx_int = if allow_indexing { 1i64 } else { 0i64 };

    let mut tx = pool.begin().await?;

    sqlx::query(
        "UPDATE status_pages SET title = ?, description = ?, is_public = ?, allow_indexing = ?, meta_title = ?, meta_description = ?, og_image_url = ?, canonical_url = ? WHERE slug = ?",
    )
    .bind(title_clean)
    .bind(description.map(|d| d.trim()))
    .bind(is_pub_int)
    .bind(allow_idx_int)
    .bind(meta_title.map(|m| m.trim()))
    .bind(meta_description.map(|m| m.trim()))
    .bind(og_image_url.map(|m| m.trim()))
    .bind(canonical_url.map(|m| m.trim()))
    .bind(slug)
    .execute(&mut *tx)
    .await?;

    let mut inserted_count = 0;
    if let Some(assignments) = monitors {
        sqlx::query("DELETE FROM status_page_monitors WHERE status_page_slug = ?")
            .bind(slug)
            .execute(&mut *tx)
            .await?;

        for a in assignments {
            let grp = if a.group_name.trim().is_empty() {
                "Services"
            } else {
                a.group_name.trim()
            };

            sqlx::query(
                r#"
                INSERT INTO status_page_monitors (status_page_slug, monitor_id, group_name, display_order)
                VALUES (?, ?, ?, ?)
                "#,
            )
            .bind(slug)
            .bind(&a.monitor_id)
            .bind(grp)
            .bind(a.display_order)
            .execute(&mut *tx)
            .await?;
            inserted_count += 1;
        }
    }

    tx.commit().await?;

    tracing::info!(slug = %slug, count = inserted_count, "Updated status page and persisted monitor assignments");

    let updated_page = get_status_page(pool, slug)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Status page '{slug}' not found")))?;
    let updated_monitors = get_status_page_monitors(pool, slug).await?;

    Ok(StatusPageDetail {
        slug: updated_page.slug,
        title: updated_page.title,
        description: updated_page.description,
        is_public: updated_page.is_public == 1,
        created_at: updated_page.created_at,
        monitors: updated_monitors,
        allow_indexing: updated_page.allow_indexing,
        meta_title: updated_page.meta_title,
        meta_description: updated_page.meta_description,
        og_image_url: updated_page.og_image_url,
        canonical_url: updated_page.canonical_url,
    })
}

/// Delete a status page (preventing deleting default).
pub async fn delete_status_page(pool: &SqlitePool, slug: &str) -> Result<(), AppError> {
    if slug == "default" {
        return Err(AppError::BadRequest(
            "Cannot delete the default status page".to_string(),
        ));
    }

    let rows_affected = sqlx::query("DELETE FROM status_pages WHERE slug = ?")
        .bind(slug)
        .execute(pool)
        .await?
        .rows_affected();

    if rows_affected == 0 {
        return Err(AppError::NotFound(format!("Status page '{slug}' not found")));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("Failed to connect to in-memory SQLite");

        crate::db::run_migrations(&pool)
            .await
            .expect("Failed to run migrations");

        pool
    }

    #[tokio::test]
    async fn test_default_status_page_seeded() {
        let pool = setup_test_db().await;
        let default_page = get_status_page(&pool, "default").await.unwrap();
        assert!(default_page.is_some());
        let page = default_page.unwrap();
        assert_eq!(page.slug, "default");
        assert_eq!(page.title, "System Status");
    }

    #[tokio::test]
    async fn test_status_page_crud_lifecycle() {
        let pool = setup_test_db().await;

        // 1. Create custom page
        let created = create_status_page(
            &pool,
            "api-services",
            "API Systems Status",
            Some("Public APIs and gateways"),
            true,
            None,
            true,
            Some("SEO Title"),
            Some("SEO Description"),
            Some("https://example.com/og.png"),
            Some("https://status.example.com/api-services"),
        )
        .await
        .unwrap();

        assert_eq!(created.slug, "api-services");
        assert_eq!(created.title, "API Systems Status");
        assert!(created.allow_indexing);
        assert_eq!(created.meta_title.as_deref(), Some("SEO Title"));

        // 2. Insert test monitors
        sqlx::query("INSERT INTO monitors (id, name, url) VALUES ('m1', 'Users API', 'https://api.test/users')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO monitors (id, name, url) VALUES ('m2', 'Billing API', 'https://api.test/billing')")
            .execute(&pool)
            .await
            .unwrap();

        // 3. Update with monitor assignments and grouping
        let assignments = vec![
            StatusPageMonitorAssignment {
                monitor_id: "m1".to_string(),
                group_name: "Core APIs".to_string(),
                display_order: 1,
            },
            StatusPageMonitorAssignment {
                monitor_id: "m2".to_string(),
                group_name: "Finance".to_string(),
                display_order: 2,
            },
        ];

        let updated = update_status_page(
            &pool,
            "api-services",
            "Updated API Status",
            Some("Updated description"),
            true,
            Some(assignments),
            false,
            Some("Updated SEO Title"),
            Some("Updated SEO Desc"),
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(updated.title, "Updated API Status");
        assert!(!updated.allow_indexing);
        assert_eq!(updated.meta_title.as_deref(), Some("Updated SEO Title"));
        assert_eq!(updated.monitors.len(), 2);
        assert_eq!(updated.monitors[0].group_name, "Core APIs");
        assert_eq!(updated.monitors[1].group_name, "Finance");

        // 4. Verify list contains both default and custom page
        let list = list_status_pages(&pool).await.unwrap();
        assert_eq!(list.len(), 2);
        let api_page = list.iter().find(|p| p.slug == "api-services").unwrap();
        assert_eq!(api_page.monitor_count, 2);

        // 5. Verify default page cannot be deleted
        let del_default = delete_status_page(&pool, "default").await;
        assert!(del_default.is_err());

        // 6. Delete custom page
        delete_status_page(&pool, "api-services").await.unwrap();
        let after_delete = get_status_page(&pool, "api-services").await.unwrap();
        assert!(after_delete.is_none());
    }
}
