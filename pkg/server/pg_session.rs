// Copyright 2026 AsterSQL.
//! Connection-private PostgreSQL search path; never changes the native SQL mode.
use crate::conn::{ColumnInfo, NativeType, PreparedMetadata, QueryResult, TiDBContext, Value};
use crate::pg_catalog_query::{ParseResult, Token, lex};

#[derive(Clone, Debug)]
pub(crate) enum SessionQuery {
    Sequence(astersql_session::runtime::PgSequenceCommand, Option<String>),
    NumericLocaleShow(bool),
    NumericLocaleSet(bool, String, bool),
    NumericLocaleReset(bool),
    LcTimeShow,
    LcTimeSet(String, bool),
    LcTimeReset,
    Show,
    IntervalStyleShow,
    IntervalStyleSet(astersql_types::pg_temporal::PgIntervalSetting),
    DateStyleShow,
    DateStyleSet(Option<astersql_types::pg_temporal::PgDateOrder>),
    DateStyleReset,
    Set(Vec<String>),
    LocalPath(Option<Vec<String>>),
    Reset,
    DefaultPath,
    SchemaUtility(astersql_session::runtime::PgSchemaCommand),
}
#[derive(Debug)]
pub(crate) struct PgSession {
    path: Vec<String>,
    default_path: Vec<String>,
    transaction_path: Option<Vec<String>>,
    // Value to retain on commit after a LOCAL override; transaction_path
    // independently retains the value to restore on rollback.
    local_path_restore: Option<Vec<String>>,
    failed_transaction: bool,
    // Original relation cursor from name adaptation, consumed by this request's
    // Q/Parse error response and reset before the next frontend command.
    pub(crate) relation_error_position: Option<usize>,
    pub(crate) error_detail: Option<&'static str>,
    pub(crate) function_error_hint: Option<&'static str>,
}
impl Default for PgSession {
    fn default() -> Self {
        Self {
            path: vec!["public".into()],
            default_path: vec!["public".into()],
            transaction_path: None,
            local_path_restore: None,
            failed_transaction: false,
            relation_error_position: None,
            function_error_hint: None,
            error_detail: None,
        }
    }
}
impl PgSession {
    /// Snapshot this connection's startup setting, independent of later SETs.
    pub(crate) fn from_startup(value: Option<&str>) -> ParseResult<Self> {
        let mut session = Self::default();
        if let Some(value) = value {
            if value.is_empty() {
                // A raw empty startup GUC displays as empty text. SQL SET TO ''
                // instead canonicalizes to the quoted empty identifier (two double quotes).
                // Neither entry resolves a namespace; retaining this spelling in
                // the existing path snapshot also preserves RESET and rollback.
                session.path = vec![String::new()];
                session.default_path.clone_from(&session.path);
                return Ok(session);
            }
            match SessionQuery::parse(&format!("SET search_path TO {value}"))? {
                Some(SessionQuery::Set(path)) => {
                    session.path = path.clone();
                    session.default_path = path;
                }
                _ => return Err(("22023", "invalid startup search_path".into())),
            }
        }
        Ok(session)
    }

    /// libpq transports command-line settings in StartupMessage.options. Only
    /// search_path is supported here; never silently accept another GUC/flag.
    pub(crate) fn from_startup_options(
        value: Option<&str>,
        options: Option<&str>,
    ) -> ParseResult<Self> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut escaped = false;
        for ch in options.unwrap_or_default().chars() {
            if escaped {
                word.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch.is_ascii_whitespace() {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                word.push(ch);
            }
        }
        if escaped {
            return Err(("22023", "incomplete startup options escape".into()));
        }
        if !word.is_empty() {
            words.push(word);
        }
        let mut path = None;
        let mut words = words.iter();
        while let Some(word) = words.next() {
            let setting = if word == "-c" {
                words.next().map(String::as_str)
            } else {
                word.strip_prefix("-c").or_else(|| word.strip_prefix("--"))
            }
            .ok_or_else(|| ("0A000", "unsupported startup option".into()))?;
            let Some(("search_path", setting)) = setting.split_once('=') else {
                return Err(("0A000", "only startup search_path is supported".into()));
            };
            path = Some(setting);
        }
        Self::from_startup(value.or(path))
    }

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
        if self.failed_transaction && !matches!(command, Some("ROLLBACK" | "COMMIT")) {
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
                    | "CREATE INDEX"
                    | "DROP VIEW"
                    | "CREATE SEQUENCE"
                    | "DROP SEQUENCE"
                    | "DROP TABLE"
                    | "CREATE DATABASE"
                    | "DROP DATABASE"
                    | "CREATE SCHEMA"
                    | "DROP SCHEMA"
                    | "GRANT"
                    | "REVOKE"
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

    /// COMMIT in an aborted block must execute a real rollback. The caller
    /// reports completion only after the shared engine confirms success.
    pub(crate) fn failed_commit(&self, command: &str, context: &dyn TiDBContext) -> bool {
        command == "COMMIT" && self.failed_transaction && context.in_transaction()
    }

    /// PostgreSQL BEGIN in an active block is a warning and a no-op. Never
    /// dispatch it to the native engine, where BEGIN commits the previous block.
    /// Call after the failed-transaction gate so an aborted block still errors.
    pub(crate) fn repeated_begin(
        &self,
        command: &str,
        context: &dyn TiDBContext,
    ) -> Option<Vec<(u8, Vec<u8>)>> {
        (command == "BEGIN" && context.in_transaction()).then(|| {
            vec![
                (
                    b'N',
                    b"SWARNING\0VWARNING\0C25001\0Mthere is already a transaction in progress\0\0"
                        .to_vec(),
                ),
                (b'C', b"BEGIN\0".to_vec()),
            ]
        })
    }

    /// Ending an idle PostgreSQL transaction reports a warning without
    /// dispatching a native transaction command or changing session state.
    pub(crate) fn outside_end(
        &self,
        command: &str,
        context: &dyn TiDBContext,
    ) -> Option<Vec<(u8, Vec<u8>)>> {
        (matches!(command, "COMMIT" | "ROLLBACK") && !context.in_transaction()).then(|| {
            vec![
                (
                    b'N',
                    b"SWARNING\0VWARNING\0C25P01\0Mthere is no transaction in progress\0\0"
                        .to_vec(),
                ),
                (b'C', format!("{command}\0").into_bytes()),
            ]
        })
    }

    pub(crate) fn record_error(&mut self, context: &dyn TiDBContext) {
        if context.in_transaction() {
            self.failed_transaction = true;
        }
    }

    pub(crate) fn record_success(&mut self, command: Option<&str>, context: &dyn TiDBContext) {
        // Capture only after the shared engine confirms an active transaction.
        // Repeated BEGIN must retain the original value. End commands take effect
        // only once the engine confirms the transaction has actually ended.
        if context.in_transaction() {
            self.transaction_path
                .get_or_insert_with(|| self.path.clone());
        } else {
            if let Some(path) = self.local_path_restore.take() {
                self.path = path;
            }
            if let Some(path) = self.transaction_path.take() {
                if command == Some("ROLLBACK") {
                    self.path = path;
                }
            }
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
        if let Some(result) = context.pg_schema_path(self.path.clone(), None) {
            return result;
        }
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
            SessionQuery::Sequence(command, name) => {
                let value = context.pg_sequence(command.clone())?;
                name.as_ref()
                    .map(|_| value.map(Value::Signed).unwrap_or(Value::Null))
            }
            SessionQuery::NumericLocaleShow(monetary) => {
                Some(Value::Text(context.pg_numeric_locale(*monetary, None)?))
            }
            SessionQuery::NumericLocaleSet(monetary, value, local) => {
                context.pg_numeric_locale(*monetary, Some((value.clone(), *local)))?;
                None
            }
            SessionQuery::NumericLocaleReset(monetary) => {
                context.pg_numeric_locale(*monetary, Some(("C".into(), false)))?;
                None
            }
            SessionQuery::LcTimeShow => Some(Value::Text(context.pg_lc_time(None)?)),
            SessionQuery::LcTimeSet(value, local) => {
                context.pg_lc_time(Some((value.clone(), *local)))?;
                None
            }
            SessionQuery::LcTimeReset => {
                context.pg_lc_time(Some(("C".into(), false)))?;
                None
            }
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
            SessionQuery::LocalPath(path) => {
                if context.in_transaction() {
                    self.local_path_restore
                        .get_or_insert_with(|| self.path.clone());
                    self.path = path.as_ref().unwrap_or(&self.default_path).clone();
                    self.effective_explicit_path(context)?;
                }
                None
            }
            SessionQuery::Set(path) => {
                self.local_path_restore = None;
                self.path = path.clone();
                self.effective_explicit_path(context)?;
                None
            }
            SessionQuery::Reset | SessionQuery::DefaultPath => {
                self.local_path_restore = None;
                self.path.clone_from(&self.default_path);
                self.effective_explicit_path(context)?;
                None
            }
            SessionQuery::SchemaUtility(command) => {
                context
                    .pg_schema_path(self.path.clone(), Some(command.clone()))
                    .ok_or_else(|| {
                        crate::conn::ConnError::Session(
                            "PostgreSQL schema metadata unavailable".into(),
                        )
                    })??;
                None
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
                ) | Self::LcTimeSet(_, true)
                    | Self::NumericLocaleSet(_, _, true)
                    | Self::LocalPath(_)
            )
        {
            vec![(b'N',b"SWARNING\0VWARNING\0C25P01\0MSET LOCAL can only be used in transaction blocks\0\0".to_vec())]
        } else {
            Vec::new()
        }
    }
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Self::Sequence(command, _) => match command {
                astersql_session::runtime::PgSequenceCommand::Create { .. } => "CREATE SEQUENCE",
                astersql_session::runtime::PgSequenceCommand::Drop { .. } => "DROP SEQUENCE",
                astersql_session::runtime::PgSequenceCommand::Grant { grant: true, .. } => "GRANT",
                astersql_session::runtime::PgSequenceCommand::Grant { grant: false, .. } => {
                    "REVOKE"
                }
                astersql_session::runtime::PgSequenceCommand::Value { .. }
                | astersql_session::runtime::PgSequenceCommand::Set { .. } => "SELECT",
            },
            Self::Show
            | Self::DateStyleShow
            | Self::IntervalStyleShow
            | Self::LcTimeShow
            | Self::NumericLocaleShow(_) => "SHOW",
            Self::LcTimeSet(..) | Self::NumericLocaleSet(..) => "SET",
            Self::LcTimeReset | Self::NumericLocaleReset(_) => "RESET",
            Self::IntervalStyleSet(astersql_types::pg_temporal::PgIntervalSetting::Reset) => {
                "RESET"
            }
            Self::SchemaUtility(command) => match command {
                astersql_session::runtime::PgSchemaCommand::Create(_) => "CREATE SCHEMA",
                astersql_session::runtime::PgSchemaCommand::Drop(_) => "DROP SCHEMA",
                astersql_session::runtime::PgSchemaCommand::Usage { grant: true, .. } => "GRANT",
                astersql_session::runtime::PgSchemaCommand::Usage { grant: false, .. } => "REVOKE",
            },
            Self::Set(_)
            | Self::DefaultPath
            | Self::LocalPath(_)
            | Self::DateStyleSet(_)
            | Self::IntervalStyleSet(_) => "SET",
            Self::Reset | Self::DateStyleReset => "RESET",
        }
    }
    pub(crate) fn metadata(&self) -> PreparedMetadata {
        let name = match self {
            Self::Sequence(_, name) => name.as_deref(),
            Self::Show => Some("search_path"),
            Self::DateStyleShow => Some("DateStyle"),
            Self::LcTimeShow => Some("lc_time"),
            Self::NumericLocaleShow(monetary) => Some(if *monetary {
                "lc_monetary"
            } else {
                "lc_numeric"
            }),
            Self::IntervalStyleShow => Some("IntervalStyle"),
            _ => None,
        };
        let mut result = PreparedMetadata {
            execution_native_types: Vec::new(),
            catalog_column_origins: Vec::new(),
            column_origins: Vec::new(),
            parameter_cast_oids: Vec::new(),
            statement_id: 0,
            parameter_count: 0,
            columns: Vec::new(),
            native_types: Vec::new(),
        };
        if let Some(name) = name {
            let sequence = matches!(self, Self::Sequence(_, Some(_)));
            let code = if sequence { 8 } else { 253 };
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
                length: if sequence { 20 } else { 64 },
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
        if let Some((command, name)) = crate::pg_sequence::parse(&tokens)? {
            return Ok(Some(Self::Sequence(command, name)));
        }
        let word = |token: Option<&Token>, expected: &str| matches!(token,Some(Token::Word(w)) if w==expected);
        use astersql_session::runtime::PgSchemaCommand;
        let identifier = |token: &Token| match token {
            Token::Word(name) | Token::Quoted(name) => Some(name.clone()),
            _ => None,
        };
        if tokens.len() == 3 && word(tokens.get(1), "schema") {
            if let Some(name) = identifier(&tokens[2]) {
                if word(tokens.first(), "create") {
                    return Ok(Some(Self::SchemaUtility(PgSchemaCommand::Create(name))));
                }
                if word(tokens.first(), "drop") {
                    return Ok(Some(Self::SchemaUtility(PgSchemaCommand::Drop(name))));
                }
            }
        }
        if tokens.len() == 7
            && (word(tokens.first(), "grant") || word(tokens.first(), "revoke"))
            && word(tokens.get(1), "usage")
            && word(tokens.get(2), "on")
            && word(tokens.get(3), "schema")
        {
            let grant = word(tokens.first(), "grant");
            if word(tokens.get(5), if grant { "to" } else { "from" })
                && let (Some(schema), Some(mut grantee)) =
                    (identifier(&tokens[4]), identifier(&tokens[6]))
            {
                if word(tokens.get(6), "public") {
                    grantee = "PUBLIC".into();
                }
                return Ok(Some(Self::SchemaUtility(PgSchemaCommand::Usage {
                    schema,
                    grantee,
                    grant,
                })));
            }
        }
        // Do not let unsupported PG namespace utility syntax fall through to
        // CREATE DATABASE or the unrelated MySQL privilege model.
        if (word(tokens.first(), "create") || word(tokens.first(), "drop"))
            && word(tokens.get(1), "schema")
            || (word(tokens.first(), "grant") || word(tokens.first(), "revoke"))
                && word(tokens.get(1), "usage")
                && word(tokens.get(3), "schema")
        {
            return Err(("0A000", "unsupported PostgreSQL schema utility form".into()));
        }
        let scoped = word(tokens.get(1), "local") || word(tokens.get(1), "session");
        let index = if scoped { 2 } else { 1 };
        if word(tokens.get(index), "lc_numeric") || word(tokens.get(index), "lc_monetary") {
            let monetary = word(tokens.get(index), "lc_monetary");
            let name = if monetary {
                "lc_monetary"
            } else {
                "lc_numeric"
            };
            if !scoped && word(tokens.first(), "show") && tokens.len() == 2 {
                return Ok(Some(Self::NumericLocaleShow(monetary)));
            }
            if !scoped && word(tokens.first(), "reset") && tokens.len() == 2 {
                return Ok(Some(Self::NumericLocaleReset(monetary)));
            }
            if word(tokens.first(), "set")
                && tokens.len() == index + 3
                && (word(tokens.get(index + 1), "to")
                    || tokens.get(index + 1) == Some(&Token::Symbol('=')))
            {
                let value = match &tokens[index + 2] {
                    Token::Word(v) if v == "default" => "C",
                    Token::Word(v) | Token::String(v) => v.as_str(),
                    _ => return Err(("42601", "invalid numeric locale".into())),
                };
                if !astersql_expression::pg_numeric_locale::validate(value, monetary) {
                    return Err((
                        "22023",
                        format!("invalid value for parameter \"{name}\": \"{value}\""),
                    ));
                }
                return Ok(Some(Self::NumericLocaleSet(
                    monetary,
                    value.into(),
                    word(tokens.get(1), "local"),
                )));
            }
            return Err(("42601", "invalid numeric locale command".into()));
        }
        if word(tokens.get(index), "lc_time") {
            if !scoped && word(tokens.first(), "show") && tokens.len() == 2 {
                return Ok(Some(Self::LcTimeShow));
            }
            if !scoped && word(tokens.first(), "reset") && tokens.len() == 2 {
                return Ok(Some(Self::LcTimeReset));
            }
            if word(tokens.first(), "set")
                && tokens.len() == index + 3
                && (word(tokens.get(index + 1), "to")
                    || tokens.get(index + 1) == Some(&Token::Symbol('=')))
            {
                let value = match &tokens[index + 2] {
                    Token::Word(v) if v == "default" => "C",
                    Token::Word(v) | Token::String(v) => v.as_str(),
                    _ => return Err(("42601", "invalid lc_time".into())),
                };
                if !astersql_expression::pg_time_locale::validate(value) {
                    return Err((
                        "22023",
                        format!("invalid value for parameter \"lc_time\": \"{value}\""),
                    ));
                }
                return Ok(Some(Self::LcTimeSet(
                    value.into(),
                    word(tokens.get(1), "local"),
                )));
            }
            return Err(("42601", "invalid lc_time command".into()));
        }
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
        let local_path = word(tokens.first(), "set")
            && word(tokens.get(1), "local")
            && word(tokens.get(2), "search_path");
        if local_path {
            tokens.remove(1);
        }
        let path_setting = |path| {
            if local_path {
                Self::LocalPath(Some(path))
            } else {
                Self::Set(path)
            }
        };
        // SESSION is an optional scope for this connection-private setting.
        if word(tokens.first(), "set")
            && word(tokens.get(1), "session")
            && word(tokens.get(2), "search_path")
        {
            tokens.remove(1);
        }
        if word(tokens.first(), "set") && word(tokens.get(1), "search_path") {
            if !(word(tokens.get(2), "to") || tokens.get(2) == Some(&Token::Symbol('='))) {
                return Err(("42601", "expected TO or =".into()));
            }
            if tokens.len() == 4 && word(tokens.get(3), "default") {
                return Ok(Some(if local_path {
                    Self::LocalPath(None)
                } else {
                    Self::DefaultPath
                }));
            }
            if word(tokens.get(3), "null") {
                return Err(("42601", "syntax error at or near NULL".into()));
            }
            let values = &tokens[3..];
            if values == [Token::String(String::new())] {
                return Ok(Some(path_setting(Vec::new())));
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
            return Ok(Some(path_setting(path)));
        }
        Ok(None)
    }
}
