//! The limited-set catalogue.
//!
//! Only limited sets need an identity: their markup prices their decay.
//! Unlimited protection is one pooled repair stream with nothing to set
//! up, so sets created before the pool (stored as `unlimited`) are no
//! longer read as sets at all.

use super::{
    map_constraint, read, ProtectionError, ProtectionService, ProtectionSet, ProtectionSetKind,
};

fn valid_markup(markup_percent: f64) -> Result<f64, ProtectionError> {
    if markup_percent.is_finite() && markup_percent >= 100.0 {
        Ok(markup_percent)
    } else {
        Err(ProtectionError::Invalid(
            "Limited sets require an average markup of at least 100%",
        ))
    }
}

fn valid_name(name: &str) -> Result<String, ProtectionError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ProtectionError::Invalid("Set name is required"));
    }
    Ok(name.to_string())
}

impl ProtectionService {
    pub async fn create_set(
        &self,
        kind: ProtectionSetKind,
        name: &str,
        markup_percent: f64,
    ) -> Result<ProtectionSet, ProtectionError> {
        let name = valid_name(name)?;
        let markup = valid_markup(markup_percent)?;
        let kind_text = kind.as_str();
        let now = self.now();
        let id = self
            .db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO protection_sets \
                     (kind, name, economy_kind, markup_percent, created_at) \
                     VALUES (?1, ?2, 'limited', ?3, ?4)",
                    rusqlite::params![kind_text, name, markup, now],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .await
            .map_err(map_constraint("An active set already uses that name"))?;
        self.active_set(id).await
    }

    /// Rename a set, or correct its markup before its first reading. The
    /// markup prices every reading taken against it, so it is frozen once
    /// one exists; a materially new acquisition is a new set.
    pub async fn update_set(
        &self,
        set_id: i64,
        name: &str,
        markup_percent: f64,
    ) -> Result<ProtectionSet, ProtectionError> {
        let existing = self.active_set(set_id).await?;
        let name = valid_name(name)?;
        let markup = valid_markup(markup_percent)?;
        if existing.basis_locked && (existing.markup_percent - markup).abs() > f64::EPSILON {
            return Err(ProtectionError::Conflict(
                "A set's markup cannot change after its first reading",
            ));
        }
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "UPDATE protection_sets SET name = ?1, markup_percent = ?2 \
                     WHERE id = ?3 AND archived_at IS NULL",
                    rusqlite::params![name, markup, set_id],
                )?;
                Ok(())
            })
            .await
            .map_err(map_constraint("An active set already uses that name"))?;
        self.active_set(set_id).await
    }

    /// Retire a set from the recording surface. Its readings and the costs
    /// they booked keep its name.
    pub async fn archive_set(&self, set_id: i64) -> Result<(), ProtectionError> {
        self.active_set(set_id).await?;
        let now = self.now();
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "UPDATE protection_sets SET archived_at = ?1 \
                     WHERE id = ?2 AND archived_at IS NULL",
                    rusqlite::params![now, set_id],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}

/// The stored limited-set row, for the readers.
pub(super) fn read_set_row(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<ProtectionSet>, ProtectionError> {
    use rusqlite::OptionalExtension;

    let row = conn
        .query_row(
            "SELECT id, kind, name, markup_percent, created_at, archived_at \
             FROM protection_sets WHERE id = ?1 AND economy_kind = 'limited'",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, kind, name, markup_percent, created_at, archived_at)) = row else {
        return Ok(None);
    };
    let latest_observation = read::read_latest_observation(conn, id)?;
    Ok(Some(ProtectionSet {
        id,
        kind: ProtectionSetKind::parse(&kind)?,
        name,
        markup_percent,
        created_at,
        archived_at,
        basis_locked: latest_observation.is_some(),
        latest_observation,
    }))
}
