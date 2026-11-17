//! A response field that is always on the wire, as a value or an
//! explicit `null`.
//!
//! `Option<T>` conflates two contracts, and schemars can only state one
//! of them: an optional field (maybe absent) or a nullable field
//! (always present, maybe null). The DTOs use `Option<T>` with
//! `skip_serializing_if` for the former; [`Nullable<T>`] states the
//! latter, serialising exactly like `Option<T>` while its schema stays
//! nullable AND required, so the generated TypeScript reads
//! `field: T | null` rather than `field?: T | null`.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

/// An always-present, possibly-null response field. Transparent over
/// `Option<T>` on the wire; required-and-nullable in the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Nullable<T>(pub Option<T>);

/// `null`: what a field read from a stored shape that predates it takes
/// under `#[serde(default)]`.
impl<T> Default for Nullable<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> From<Option<T>> for Nullable<T> {
    fn from(value: Option<T>) -> Self {
        Self(value)
    }
}

/// Reads like the `Option` it wraps (`.as_deref()`, `.is_none()`, ...).
impl<T> std::ops::Deref for Nullable<T> {
    type Target = Option<T>;

    fn deref(&self) -> &Option<T> {
        &self.0
    }
}

impl<T: PartialEq> PartialEq<Option<T>> for Nullable<T> {
    fn eq(&self, other: &Option<T>) -> bool {
        self.0 == *other
    }
}

impl<T: PartialEq> PartialEq<Nullable<T>> for Option<T> {
    fn eq(&self, other: &Nullable<T>) -> bool {
        *self == other.0
    }
}

impl<T> From<T> for Nullable<T> {
    fn from(value: T) -> Self {
        Self(Some(value))
    }
}

impl<T: JsonSchema> JsonSchema for Nullable<T> {
    fn schema_name() -> Cow<'static, str> {
        <Option<T>>::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        <Option<T>>::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        <Option<T>>::json_schema(generator)
    }

    fn inline_schema() -> bool {
        <Option<T>>::inline_schema()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, JsonSchema)]
    struct Probe {
        plain: Nullable<String>,
        behind: Option<String>,
    }

    #[test]
    fn serialises_transparently_and_schemas_as_required_nullable() {
        let some = Probe {
            plain: "x".to_string().into(),
            behind: None,
        };
        assert_eq!(
            serde_json::to_value(&some).unwrap(),
            serde_json::json!({"plain": "x", "behind": null})
        );
        let none = Probe {
            plain: Nullable(None),
            behind: None,
        };
        assert_eq!(
            serde_json::to_value(&none).unwrap()["plain"],
            serde_json::Value::Null
        );

        let schema = serde_json::to_value(schemars::schema_for!(Probe)).unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|name| name == "plain"));
        assert!(!required.iter().any(|name| name == "behind"));
    }
}
