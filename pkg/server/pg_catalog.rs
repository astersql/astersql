// Copyright 2026 AsterSQL.
//! PG-only DataGrip catalog probes backed by canonical schema/transaction data.
use crate::conn::{
    CancellationToken, ColumnInfo, ConnError, ConnResult, NativeType, PreparedMetadata,
    QueryResult, TiDBContext, Value,
};

pub(crate) const DATABASES_SQL: &str = r#"select N.oid::bigint as id,
       datname as name, D.description, datistemplate as is_template,
       datallowconn as allow_connections,
       pg_catalog.pg_get_userbyid(N.datdba) as "owner"
from pg_catalog.pg_database N
left join pg_catalog.pg_shdescription D on N.oid = D.objoid
order by case when datname = pg_catalog.current_database() then -1::bigint else N.oid::bigint end"#;
pub(crate) const TRANSACTIONS_SQL: &str = "select L.transactionid::varchar::bigint as transaction_id from pg_catalog.pg_locks L where L.transactionid is not null order by pg_catalog.age(L.transactionid) desc limit 1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogQuery {
    Databases,
    OldestTransaction,
}
impl CatalogQuery {
    pub(crate) fn classify(sql: &str) -> Option<Self> {
        if !sql
            .as_bytes()
            .windows(b"pg_catalog".len())
            .any(|bytes| bytes.eq_ignore_ascii_case(b"pg_catalog"))
        {
            return None;
        }
        let input = tokens(sql)?;
        if input == tokens(DATABASES_SQL)? {
            Some(Self::Databases)
        } else if input == tokens(TRANSACTIONS_SQL)? {
            Some(Self::OldestTransaction)
        } else {
            None
        }
    }
    pub(crate) fn metadata(self) -> PreparedMetadata {
        let fields = match self {
            Self::Databases => vec![
                ("id", 8, 0),
                ("name", 253, 0),
                ("description", 253, 0),
                (
                    "is_template",
                    1,
                    astersql_parser_mysql::r#type::IsBooleanFlag,
                ),
                (
                    "allow_connections",
                    1,
                    astersql_parser_mysql::r#type::IsBooleanFlag,
                ),
                ("owner", 253, 0),
            ],
            Self::OldestTransaction => vec![("transaction_id", 8, 0)],
        };
        let columns = fields
            .iter()
            .map(|(name, code, flags)| ColumnInfo {
                schema: String::new(),
                table: String::new(),
                org_table: String::new(),
                name: (*name).into(),
                org_name: String::new(),
                charset: 45,
                column_length: 64,
                column_type: *code,
                flags: *flags as u16,
                decimals: 0,
                default_value: None,
            })
            .collect();
        let native_types = fields
            .into_iter()
            .map(|(_, code, flags)| NativeType {
                code,
                flags,
                length: 64,
                decimal: 0,
            })
            .collect();
        // Catalog statement identity belongs to the PG statement name, not an
        // engine prepared statement. No backend handle is allocated or closed.
        PreparedMetadata {
            statement_id: 0,
            parameter_count: 0,
            columns,
            native_types,
        }
    }
    pub(crate) fn execute(self, context: &dyn TiDBContext) -> ConnResult<QueryResult> {
        let metadata = self.metadata();
        let rows = match self {
            Self::Databases => {
                let snapshot = context
                    .schema_snapshot()
                    .ok_or_else(|| ConnError::Session("schema snapshot is unavailable".into()))?;
                let current =
                    context.execute_query("SELECT DATABASE()", false, &CancellationToken::new())?;
                let database = current
                    .first()
                    .and_then(|r| r.rows.first())
                    .and_then(|r| r.first());
                let database = match database {
                    Some(Value::Text(name)) => name.as_str(),
                    _ => "",
                };
                let mut schemas = snapshot.AllSchemas();
                schemas.sort_by_key(|schema| {
                    (
                        !schema.name.original.eq_ignore_ascii_case(database),
                        schema.id,
                    )
                });
                schemas
                    .into_iter()
                    .map(|schema| {
                        vec![
                            Value::Signed(schema.id),
                            Value::Text(schema.name.original.clone()),
                            // Native schemas have no PostgreSQL owner or description,
                            // and are neither template databases nor disabled databases.
                            Value::Null,
                            Value::Text("false".into()),
                            Value::Text("true".into()),
                            Value::Null,
                        ]
                    })
                    .collect()
            }
            Self::OldestTransaction => {
                // Native TSO start timestamps are monotonically ordered. They
                // are not PostgreSQL wraparound XIDs or fabricated lock rows.
                let results = context.execute_query(
                    "SELECT ID FROM information_schema.tidb_trx",
                    false,
                    &CancellationToken::new(),
                )?;
                let mut oldest = None;
                for result in &results {
                    for row in &result.rows {
                        let id = match row.first() {
                            Some(Value::Text(value)) => value.parse::<i64>().map_err(|e| {
                                ConnError::Session(format!("invalid native transaction ID: {e}"))
                            })?,
                            Some(Value::Signed(value)) => *value,
                            Some(Value::Unsigned(value)) => i64::try_from(*value).map_err(|e| {
                                ConnError::Session(format!(
                                    "native transaction ID exceeds bigint: {e}"
                                ))
                            })?,
                            Some(Value::Null) => continue,
                            _ => {
                                return Err(ConnError::Session(
                                    "missing native transaction ID".into(),
                                ));
                            }
                        };
                        oldest = Some(oldest.map_or(id, |previous: i64| previous.min(id)));
                    }
                }
                oldest
                    .map(|id| vec![vec![Value::Signed(id)]])
                    .unwrap_or_default()
            }
        };
        Ok(QueryResult {
            columns: metadata.columns,
            native_types: metadata.native_types,
            rows,
            state: context.state(),
            response_lifecycle: None,
        })
    }
}

/// A complete token comparison tolerates formatting/comments, while rejecting
/// query variants, batches and quoted SQL that merely contain a catalog name.
fn tokens(sql: &str) -> Option<Vec<String>> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut output = Vec::new();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"--") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth != 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth != 0 {
                return None;
            }
            continue;
        }
        let start = i;
        if matches!(bytes[i], b'\'' | b'"' | b'`') {
            let quote = bytes[i];
            i += 1;
            loop {
                let next = *bytes.get(i)?;
                i += 1;
                if next == quote {
                    if bytes.get(i) == Some(&quote) {
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            output.push(sql.get(start..i)?.to_owned());
        } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            i += 1;
            while bytes
                .get(i)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                i += 1;
            }
            output.push(sql[start..i].to_ascii_lowercase());
        } else {
            i += 1;
            output.push(sql.get(start..i)?.to_owned());
        }
    }
    if output.last().is_some_and(|token| token == ";") {
        output.pop();
    }
    Some(output)
}
