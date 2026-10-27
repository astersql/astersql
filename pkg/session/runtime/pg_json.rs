// Copyright 2026 AsterSQL.
//! Explicit JSON casts preserve validated original text and an independent type.
use super::*;
use astersql_parser_types::FieldType;
use astersql_types::pg_json::{PgJson, PgJsonError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PgJsonCastError {
    Input(PgJsonError),
    Unsupported,
}
impl std::fmt::Display for PgJsonCastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(error) => error.fmt(f),
            Self::Unsupported => f.write_str("unsupported PostgreSQL JSON cast"),
        }
    }
}
impl std::error::Error for PgJsonCastError {}
impl PgJsonCastError {
    pub fn sqlstate(self) -> &'static str {
        match self {
            Self::Input(error) => error.sqlstate(),
            Self::Unsupported => "0A000",
        }
    }
    fn session_error(self) -> SessionError {
        SessionError::with_source(self.to_string(), astersql_errors::SharedError::new(self))
    }
}

/// Check eligibility before evaluating rows, including typed NULL and empty scans.
pub(super) fn validate_cast(source: &FieldType, target: &FieldType) -> SessionResult<()> {
    use astersql_parser_mysql::r#type::*;
    let source_json = source.GetType() == TypePgJson;
    let target_json = target.GetType() == TypePgJson;
    if !source_json && !target_json {
        return Ok(());
    }
    let text = source.PostgreSQLTextOid() == Some(25)
        || (matches!(source.GetType(), TypeVarString | TypeVarchar | TypeString)
            && source.GetCharset() != "binary"
            && source.PostgreSQLTextOid().is_none());
    if (target_json && (source_json || text || source.GetType() == TypeNull))
        || (source_json && target.PostgreSQLTextOid() == Some(25))
    {
        Ok(())
    } else {
        Err(PgJsonCastError::Unsupported.session_error())
    }
}

pub(super) fn cast_value(value: Option<String>) -> SessionResult<Option<String>> {
    value
        .map(|text| {
            text.parse::<PgJson>()
                .map(|value| value.to_string())
                .map_err(|error| PgJsonCastError::Input(error).session_error())
        })
        .transpose()
}
