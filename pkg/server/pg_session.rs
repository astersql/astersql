// Copyright 2026 AsterSQL.
//! Connection-private PostgreSQL search path; never changes the native SQL mode.
use crate::conn::{ColumnInfo, NativeType, PreparedMetadata, QueryResult, TiDBContext, Value};
use crate::pg_catalog_query::{ParseResult, Token, lex};

#[derive(Clone, Debug)]
pub(crate) enum SessionQuery {
    Show,
    IntervalStyleShow,
    IntervalStyleSet(astersql_types::pg_temporal::PgIntervalSetting),
    DateStyleShow,
    DateStyleSet(Option<astersql_types::pg_temporal::PgDateOrder>),
    DateStyleReset,
    Set(Vec<String>),
    Reset,
    Schema(String),
    Database(String),
}
#[derive(Debug)]
pub(crate) struct PgSession {
    path: Vec<String>,
    failed_transaction: bool,
    // Original relation cursor from name adaptation, consumed by this request's
    // Q/Parse error response and reset before the next frontend command.
    pub(crate) relation_error_position: Option<usize>,
}
impl Default for PgSession {
    fn default() -> Self {
        Self {
            path: vec!["public".into()],
            failed_transaction: false,
            relation_error_position: None,
        }
    }
}
impl PgSession {
    pub(crate) fn transaction_status(&mut self, context: &dyn TiDBContext) -> u8 {
        if !context.in_transaction() {
            self.failed_transaction = false;
            b'I'
        } else if self.failed_transaction {
            b'E'
        } else {
            b'T'
        }
    }

    pub(crate) fn check_transaction_command(
        &mut self,
        command: Option<&str>,
        context: &dyn TiDBContext,
    ) -> Result<(), (&'static str, String)> {
        if !context.in_transaction() {
            self.failed_transaction = false;
            return Ok(());
        }
        if self.failed_transaction && command != Some("ROLLBACK") {
            return Err((
                "25P02",
                "current transaction is aborted, commands ignored until end of transaction block"
                    .into(),
            ));
        }
        if matches!(
            command,
            Some(
                "CREATE TABLE"
                    | "DROP TABLE"
                    | "CREATE DATABASE"
                    | "DROP DATABASE"
                    | "ALTER TABLE"
                    | "TRUNCATE TABLE"
            )
        ) {
            return Err((
                "0A000",
                "DDL inside a PostgreSQL transaction is unsupported".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn record_error(&mut self, context: &dyn TiDBContext) {
        if context.in_transaction() {
            self.failed_transaction = true;
        }
    }

    pub(crate) fn record_success(&mut self, command: Option<&str>, context: &dyn TiDBContext) {
        if command == Some("ROLLBACK") || !context.in_transaction() {
            self.failed_transaction = false;
        }
    }

    /// Expand only namespaces exposed by the authoritative directory provider.
    /// Missing schemas (including an absent user schema) are silently skipped;
    /// implicit pg_catalog belongs to relation lookup, not current_schema().
    pub(crate) fn effective_explicit_path(
        &self,
        context: &dyn TiDBContext,
    ) -> crate::conn::ConnResult<Vec<String>> {
        let namespaces = crate::pg_catalog::namespace_names(context)?;
        let identity = context.user_identity()?;
        let user = identity
            .rsplit_once('@')
            .map_or(identity.as_str(), |(user, _)| user);
        let mut effective = Vec::new();
        for entry in &self.path {
            let name = if entry == "$user" { user } else { entry };
            if namespaces.iter().any(|namespace| namespace == name)
                && !effective.iter().any(|existing| existing == name)
            {
                effective.push(name.to_owned());
            }
        }
        Ok(effective)
    }
    pub(crate) fn effective_schema(
        &self,
        context: &dyn TiDBContext,
    ) -> crate::conn::ConnResult<Option<String>> {
        Ok(self.effective_explicit_path(context)?.into_iter().next())
    }
    pub(crate) fn execute(
        &mut self,
        query: &SessionQuery,
        context: &dyn TiDBContext,
    ) -> crate::conn::ConnResult<QueryResult> {
        let value = match query {
            SessionQuery::IntervalStyleShow => {
                Some(Value::Text(context.pg_interval_style(None)?.name().into()))
            }
            SessionQuery::IntervalStyleSet(setting) => {
                context.pg_interval_style(Some(*setting))?;
                None
            }
            SessionQuery::DateStyleShow => Some(Value::Text(format!(
                "ISO, {}",
                context.pg_date_context(None)?.order.name()
            ))),
            SessionQuery::DateStyleSet(order) => {
                context.pg_date_context(*order)?;
                None
            }
            SessionQuery::DateStyleReset => {
                context.pg_date_context(Some(Default::default()))?;
                None
            }
            // PostgreSQL canonicalizes the empty string list item as a quoted
            // empty identifier. It has no effective namespace, but SHOW is not NULL.
            SessionQuery::Show if self.path.is_empty() => Some(Value::Text("\"\"".into())),
            SessionQuery::Show => Some(Value::Text(
                self.path
                    .iter()
                    .map(|name| {
                        if name == "$user"
                            || name.chars().any(|c| {
                                !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                            })
                        {
                            format!("\"{}\"", name.replace('"', "\"\""))
                        } else {
                            name.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
            SessionQuery::Set(path) => {
                self.path = path.clone();
                None
            }
            SessionQuery::Reset => {
                *self = Self::default();
                None
            }
            SessionQuery::Schema(_) => Some(
                self.effective_schema(context)?
                    .map_or(Value::Null, Value::Text),
            ),
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
    pub(crate) fn notices(&self, context: &dyn TiDBContext) -> Vec<(u8, Vec<u8>)> {
        use astersql_types::pg_temporal::PgIntervalSetting;
        if !context.in_transaction()
            && matches!(
                self,
                Self::IntervalStyleSet(
                    PgIntervalSetting::Set { local: true, .. } | PgIntervalSetting::ResetLocal
                )
            )
        {
            vec![(b'N',b"SWARNING\0VWARNING\0C25P01\0MSET LOCAL can only be used in transaction blocks\0\0".to_vec())]
        } else {
            Vec::new()
        }
    }
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Self::Show | Self::DateStyleShow | Self::IntervalStyleShow => "SHOW",
            Self::IntervalStyleSet(astersql_types::pg_temporal::PgIntervalSetting::Reset) => {
                "RESET"
            }
            Self::Set(_) | Self::DateStyleSet(_) | Self::IntervalStyleSet(_) => "SET",
            Self::Reset | Self::DateStyleReset => "RESET",
            _ => "SELECT",
        }
    }
    pub(crate) fn metadata(&self) -> PreparedMetadata {
        let name = match self {
            Self::Show => Some("search_path"),
            Self::DateStyleShow => Some("DateStyle"),
            Self::IntervalStyleShow => Some("IntervalStyle"),
            Self::Schema(name) | Self::Database(name) => Some(name.as_str()),
            _ => None,
        };
        let mut result = PreparedMetadata {
            catalog_column_origins: Vec::new(),
            column_origins: Vec::new(),
            parameter_cast_oids: Vec::new(),
            statement_id: 0,
            parameter_count: 0,
            columns: Vec::new(),
            native_types: Vec::new(),
        };
        if let Some(name) = name {
            let code = if matches!(self, Self::Schema(_)) {
                crate::pg_result::CatalogColumnType::Name as u8
            } else {
                253
            };
            result.columns.push(ColumnInfo {
                name: name.into(),
                column_type: code,
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
                code,
                flags: 0,
                length: 64,
                decimal: 0,
                postgresql_text_oid: None,
                postgresql_array_element: None,
                postgresql_record_fields: None,
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
        let scoped = word(tokens.get(1), "local") || word(tokens.get(1), "session");
        let index = if scoped { 2 } else { 1 };
        if word(tokens.get(index), "intervalstyle") {
            use astersql_types::pg_temporal::{PgIntervalSetting, PgIntervalStyle};
            if !scoped && word(tokens.first(), "show") && tokens.len() == 2 {
                return Ok(Some(Self::IntervalStyleShow));
            }
            if !scoped && word(tokens.first(), "reset") && tokens.len() == 2 {
                return Ok(Some(Self::IntervalStyleSet(PgIntervalSetting::Reset)));
            }
            if word(tokens.first(), "set")
                && tokens.len() == index + 3
                && (word(tokens.get(index + 1), "to")
                    || tokens.get(index + 1) == Some(&Token::Symbol('=')))
            {
                let value = match &tokens[index + 2] {
                    Token::Word(s) | Token::String(s) => s,
                    _ => return Err(("42601", "invalid IntervalStyle".into())),
                };
                let style = if word(tokens.get(index + 2), "default") {
                    if !word(tokens.get(1), "local") {
                        return Ok(Some(Self::IntervalStyleSet(PgIntervalSetting::Reset)));
                    }
                    return Ok(Some(Self::IntervalStyleSet(PgIntervalSetting::ResetLocal)));
                } else {
                    PgIntervalStyle::parse(value)
                        .ok_or(("22023", "invalid value for IntervalStyle".into()))?
                };
                return Ok(Some(Self::IntervalStyleSet(PgIntervalSetting::Set {
                    style,
                    local: word(tokens.get(1), "local"),
                })));
            }
            return Err(("42601", "invalid IntervalStyle command".into()));
        }
        if word(tokens.get(1), "datestyle") {
            use astersql_types::pg_temporal::PgDateOrder;
            if word(tokens.first(), "show") && tokens.len() == 2 {
                return Ok(Some(Self::DateStyleShow));
            }
            if word(tokens.first(), "reset") && tokens.len() == 2 {
                return Ok(Some(Self::DateStyleReset));
            }
            if word(tokens.first(), "set")
                && (word(tokens.get(2), "to") || tokens.get(2) == Some(&Token::Symbol('=')))
            {
                if tokens.len() == 4 && word(tokens.get(3), "default") {
                    return Ok(Some(Self::DateStyleReset));
                }
                let mut order = None;
                let mut expect_value = true;
                for token in &tokens[3..] {
                    if token == &Token::Symbol(',') {
                        if expect_value {
                            return Err(("42601", "expected DateStyle value".into()));
                        }
                        expect_value = true;
                        continue;
                    }
                    if !expect_value {
                        return Err(("42601", "expected comma in DateStyle".into()));
                    }
                    expect_value = false;
                    let text = match token {
                        Token::String(text) | Token::Word(text) => text.as_str(),
                        _ => return Err(("42601", "invalid DateStyle".into())),
                    };
                    for part in text.split(',').map(str::trim) {
                        match part.to_ascii_uppercase().as_str() {
                            "ISO" => {}
                            "MDY" => order = Some(PgDateOrder::Mdy),
                            "DMY" => order = Some(PgDateOrder::Dmy),
                            "YMD" => order = Some(PgDateOrder::Ymd),
                            _ => {
                                return Err((
                                    "22023",
                                    "unsupported DateStyle; use ISO with MDY, DMY or YMD".into(),
                                ));
                            }
                        }
                    }
                }
                if expect_value {
                    return Err(("42601", "expected DateStyle value".into()));
                }
                return Ok(Some(Self::DateStyleSet(order)));
            }
            return Err(("42601", "invalid DateStyle command".into()));
        }
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
            if word(tokens.get(3), "null") {
                return Err(("0A000", "NULL search_path is unsupported".into()));
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
