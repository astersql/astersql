// Copyright 2026 AsterSQL.
//! Explicit JSONB casts always revalidate through the canonical value parser.
use super::*;
use astersql_parser_types::FieldType;
use astersql_types::pg_jsonb::PgJsonb;

pub(super) fn validate_cast(source: &FieldType, target: &FieldType) -> SessionResult<()> {
    use astersql_parser_mysql::r#type::*;
    let source_jsonb = source.GetType() == TypePgJsonb;
    let target_jsonb = target.GetType() == TypePgJsonb;
    let text = source.PostgreSQLTextOid() == Some(25)
        || (matches!(source.GetType(), TypeVarString | TypeVarchar | TypeString)
            && source.GetCharset() != "binary"
            && source.PostgreSQLTextOid().is_none());
    if (target_jsonb
        && (source_jsonb || source.GetType() == TypePgJson || text || source.GetType() == TypeNull))
        || (source_jsonb
            && (target.GetType() == TypePgJson
                || matches!(target.PostgreSQLTextOid(), Some(25 | 1042 | 1043))))
    {
        Ok(())
    } else {
        Err(PgJsonCastError::Unsupported.session_error())
    }
}

pub(super) fn cast_value(value: Option<String>) -> SessionResult<Option<String>> {
    value
        .map(|text| {
            text.parse::<PgJsonb>()
                .map(|jsonb| jsonb.to_string())
                .map_err(|error| PgJsonCastError::Jsonb(error).session_error())
        })
        .transpose()
}
