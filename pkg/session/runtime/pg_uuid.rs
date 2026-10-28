// Copyright 2026 AsterSQL.
//! UUID casts retain their own identity while using the shared typed row carrier.
use super::*;
use astersql_parser_types::FieldType;
use astersql_types::pg_uuid::PgUuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PgUuidCastError {
    InvalidSyntax,
    Unsupported,
}
impl std::fmt::Display for PgUuidCastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidSyntax => "invalid input syntax for type uuid",
            Self::Unsupported => "unsupported PostgreSQL UUID cast",
        })
    }
}
impl std::error::Error for PgUuidCastError {}
impl PgUuidCastError {
    pub fn sqlstate(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "22P02",
            Self::Unsupported => "0A000",
        }
    }
    pub(super) fn session_error(self) -> SessionError {
        SessionError::with_source(self.to_string(), astersql_errors::SharedError::new(self))
    }
}

/// Eligibility precedes NULL/row evaluation so an empty scan cannot hide an
/// unsupported bytea/native conversion. Parameter types resolve at binding.
pub(super) fn validate_cast(source: &FieldType, target: &FieldType) -> SessionResult<()> {
    use astersql_parser_mysql::r#type::*;
    let source_uuid = source.GetType() == TypePgUuid;
    let target_uuid = target.GetType() == TypePgUuid;
    if !source_uuid && !target_uuid {
        return Ok(());
    }
    let text_source = source.PostgreSQLTextOid() == Some(25)
        || (matches!(source.GetType(), TypeVarString | TypeVarchar | TypeString)
            && source.GetCharset() != "binary"
            && source.PostgreSQLTextOid().is_none());
    if (target_uuid && (source_uuid || text_source || source.GetType() == TypeNull))
        || (source_uuid && matches!(target.PostgreSQLTextOid(), Some(25 | 1042 | 1043)))
    {
        Ok(())
    } else {
        Err(PgUuidCastError::Unsupported.session_error())
    }
}

pub(super) fn cast_value(value: Option<String>) -> SessionResult<Option<String>> {
    value
        .map(|text| {
            text.parse::<PgUuid>()
                .map(|v| v.to_string())
                .map_err(|_| PgUuidCastError::InvalidSyntax.session_error())
        })
        .transpose()
}
