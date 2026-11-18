//! Review reads: a session's outputs of one classification, and the healing
//! items an output could be corrected to.

use rusqlite::OptionalExtension;

use super::{
    CorrectionKind, CorrectionTool, HealingOutput, HealingOutputPage, HealingReviewError,
    OutputClassification,
};
use crate::equipment_pricing::{heal_cost_from_props, healing_profile_from_props};

pub(super) fn session_outputs(
    conn: &rusqlite::Connection,
    session_id: &str,
    classification: OutputClassification,
    offset: i64,
    limit: i64,
) -> Result<HealingOutputPage, HealingReviewError> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM healing_outputs WHERE session_id = ?1 AND classification = ?2",
        rusqlite::params![session_id, classification.as_str()],
        |row| row.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT o.id, o.observed_at, o.amount, o.classification, o.reason, o.activation_id, \
                a.tool_name, o.correction_id, c.kind, \
                EXISTS (SELECT 1 FROM healing_activations b \
                        WHERE b.confirming_output_id = o.id AND b.superseded_at IS NULL) \
         FROM healing_outputs o \
         LEFT JOIN healing_activations a ON a.id = o.activation_id \
         LEFT JOIN healing_corrections c ON c.id = o.correction_id \
         WHERE o.session_id = ?1 AND o.classification = ?2 \
         ORDER BY o.observed_at, o.id \
         LIMIT ?3 OFFSET ?4",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![session_id, classification.as_str(), limit, offset],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, bool>(9)?,
            ))
        },
    )?;
    let mut outputs = Vec::new();
    for row in rows {
        let (
            id,
            observed_at,
            amount,
            stored_classification,
            reason,
            activation_id,
            tool_name,
            correction_id,
            correction_kind,
            bills,
        ) = row?;
        let correction_kind = correction_kind
            .as_deref()
            .map(CorrectionKind::parse)
            .transpose()?;
        outputs.push(HealingOutput {
            id,
            observed_at,
            amount,
            classification: OutputClassification::parse(&stored_classification)?,
            reason,
            activation_id,
            tool_name,
            correctable: correction_id.is_none() && !bills,
            correction_id,
            correction_kind,
        });
    }
    Ok(HealingOutputPage { outputs, total })
}

pub(super) fn correction_tools(
    conn: &rusqlite::Connection,
    output_id: &str,
) -> Result<Vec<CorrectionTool>, HealingReviewError> {
    let Some(amount) = conn
        .query_row(
            "SELECT amount FROM healing_outputs WHERE id = ?1",
            [output_id],
            |row| row.get::<_, f64>(0),
        )
        .optional()?
    else {
        return Err(HealingReviewError::NotFound("Heal not found"));
    };
    let mut stmt = conn.prepare(
        "SELECT id, name, properties_json FROM equipment_library \
         WHERE item_type = 'healing' ORDER BY name, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut tools = Vec::new();
    for row in rows {
        let (equipment_id, name, properties_json) = row?;
        let (cost_per_use_ped, _) = heal_cost_from_props(&properties_json);
        let fits = healing_profile_from_props(&properties_json).confirms_activation(amount);
        tools.push(CorrectionTool {
            equipment_id,
            name,
            cost_per_use_ped,
            fits,
        });
    }
    // Fitting items first; the sort is stable, so each group stays by name.
    tools.sort_by_key(|tool| !tool.fits);
    Ok(tools)
}
