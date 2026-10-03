// Copyright 2026 AsterSQL.
//! Connection-private PostgreSQL search path; never changes the native SQL mode.
use crate::conn::{ColumnInfo, NativeType, PreparedMetadata, QueryResult, TiDBContext, Value};
use crate::pg_catalog_query::{ParseResult, Token, lex};

#[derive(Clone, Debug)]
pub(crate) enum SessionQuery {
    Show,
    Set(Vec<String>),
    Reset,
    Schema(String),
    Database(String),
}
#[derive(Debug)]
pub(crate) struct PgSession {
    path: Vec<String>,
}
impl Default for PgSession {
    fn default() -> Self {
        Self {
            path: vec!["public".into()],
        }
    }
}
impl PgSession {
    pub(crate) fn has_public(&self) -> bool {
        self.path.iter().any(|name| name == "public")
    }
    pub(crate) fn schema(&self) -> Option<&str> {
        self.path.first().map(String::as_str)
    }
    pub(crate) fn execute(
        &mut self,
        query: &SessionQuery,
        context: &dyn TiDBContext,
    ) -> crate::conn::ConnResult<QueryResult> {
        let value = match query {
            SessionQuery::Show => Some(Value::Text(self.path.join(", "))),
            SessionQuery::Set(path) => {
                self.path = path.clone();
                None
            }
            SessionQuery::Reset => {
                *self = Self::default();
                None
            }
            SessionQuery::Schema(_) => {
                Some(self.schema().map_or(Value::Null, |s| Value::Text(s.into())))
            }
            SessionQuery::Database(_) => {
                let result = context.execute_query(
                    "SELECT DATABASE()",
                    false,
                    &crate::conn::CancellationToken::new(),
                )?;
                Some(
                    result
                        .first()
                        .and_then(|r| r.rows.first())
                        .and_then(|r| r.first())
                        .cloned()
                        .unwrap_or(Value::Null),
                )
            }
        };
        let meta = query.metadata();
        Ok(QueryResult {
            columns: meta.columns,
            native_types: meta.native_types,
            rows: value.map(|v| vec![vec![v]]).unwrap_or_default(),
            state: context.state(),
            ..QueryResult::default()
        })
    }
}
impl SessionQuery {
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Self::Show => "SHOW",
            Self::Set(_) => "SET",
            Self::Reset => "RESET",
            _ => "SELECT",
        }
    }
    pub(crate) fn metadata(&self) -> PreparedMetadata {
        let name = match self {
            Self::Show => Some("search_path"),
            Self::Schema(name) | Self::Database(name) => Some(name.as_str()),
            _ => None,
        };
        let mut result = PreparedMetadata {
            statement_id: 0,
            parameter_count: 0,
            columns: Vec::new(),
            native_types: Vec::new(),
        };
        if let Some(name) = name {
            result.columns.push(ColumnInfo {
                name: name.into(),
                column_type: 253,
                charset: 45,
                column_length: 64,
                schema: String::new(),
                table: String::new(),
                org_table: String::new(),
                org_name: String::new(),
                flags: 0,
                decimals: 0,
                default_value: None,
            });
            result.native_types.push(NativeType {
                code: 253,
                flags: 0,
                length: 64,
                decimal: 0,
            });
        }
        result
    }
    pub(crate) fn parse(sql: &str) -> ParseResult<Option<Self>> {
        // Reuse the bounded PG lexer so quotes, comments and statement boundaries
        // follow the same rules as catalog queries.
        let mut tokens = match lex(sql) {
            Ok(tokens) => tokens,
            Err(_) => return Ok(None),
        };
        if tokens.last() == Some(&Token::Symbol(';')) {
            tokens.pop();
        }
        let word = |token: Option<&Token>, expected: &str| matches!(token,Some(Token::Word(w)) if w==expected);
        if word(tokens.first(), "show") && word(tokens.get(1), "search_path") {
            return if tokens.len() == 2 {
                Ok(Some(Self::Show))
            } else {
                Err(("42601", "invalid SHOW search_path".into()))
            };
        }
        if word(tokens.first(), "reset") && word(tokens.get(1), "search_path") {
            return if tokens.len() == 2 {
                Ok(Some(Self::Reset))
            } else {
                Err(("42601", "invalid RESET search_path".into()))
            };
        }
        if word(tokens.first(), "set") && word(tokens.get(1), "search_path") {
            if !(word(tokens.get(2), "to") || tokens.get(2) == Some(&Token::Symbol('='))) {
                return Err(("42601", "expected TO or =".into()));
            }
            if tokens.len() == 4 && word(tokens.get(3), "default") {
                return Ok(Some(Self::Reset));
            }
            let values = &tokens[3..];
            if values == [Token::String(String::new())] {
                return Ok(Some(Self::Set(Vec::new())));
            }
            if values.is_empty() || values.len() % 2 == 0 {
                return Err(("42601", "invalid search_path list".into()));
            }
            let mut path = Vec::new();
            for (index, token) in values.iter().enumerate() {
                if index % 2 == 1 {
                    if token != &Token::Symbol(',') {
                        return Err(("42601", "expected schema separator".into()));
                    }
                    continue;
                }
                let name = match token {
                    Token::Word(s) | Token::Quoted(s) | Token::String(s) => s,
                    _ => return Err(("42601", "expected schema name".into())),
                };
                if !matches!(name.as_str(), "public" | "pg_catalog") {
                    return Err(("0A000", format!("unsupported PostgreSQL schema {name}")));
                }
                if path.contains(name) {
                    return Err((
                        "0A000",
                        "duplicate search_path schema is unsupported".into(),
                    ));
                }
                path.push(name.clone());
            }
            return Ok(Some(Self::Set(path)));
        }
        if !word(tokens.first(), "select") {
            return Ok(None);
        }
        let mut i = 1;
        if word(tokens.get(i), "pg_catalog") && tokens.get(i + 1) == Some(&Token::Symbol('.')) {
            i += 2;
        }
        let function = match tokens.get(i) {
            Some(Token::Word(name))
                if matches!(name.as_str(), "current_schema" | "current_database") =>
            {
                name.clone()
            }
            _ => return Ok(None),
        };
        i += 1;
        if tokens.get(i) == Some(&Token::Symbol('('))
            && tokens.get(i + 1) == Some(&Token::Symbol(')'))
        {
            i += 2;
        } else {
            return Ok(None);
        }
        let mut label = function.clone();
        if word(tokens.get(i), "as") {
            i += 1;
        }
        if let Some(Token::Word(alias) | Token::Quoted(alias)) = tokens.get(i) {
            label = alias.clone();
            i += 1;
        }
        if i != tokens.len() {
            return Ok(None);
        }
        Ok(Some(if function == "current_schema" {
            Self::Schema(label)
        } else {
            Self::Database(label)
        }))
    }
}
