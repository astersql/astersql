// Copyright 2026 AsterSQL.
//! Connection-private PostgreSQL search path; never changes the native SQL mode.
use crate::conn::{ColumnInfo, NativeType, PreparedMetadata, QueryResult, TiDBContext, Value};
use crate::pg_catalog_query::{ParseResult, Token, lex};

#[derive(Clone, Debug)]
pub(crate) enum SessionQuery {
    SetRole(Option<String>),
    ResetRole,
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
    DatabaseUtility(astersql_session::runtime::PgDatabaseCommand),
    RoleUtility(astersql_session::runtime::PgRoleCommand),
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
    database_notices: Vec<(u8, Vec<u8>)>,
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
            database_notices: Vec::new(),
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
                    | "ALTER FUNCTION"
                    | "ALTER SCHEMA"
                    | "ALTER TABLE"
                    | "ALTER TYPE"
                    | "ALTER DOMAIN"
                    | "ALTER VIEW"
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
    pub(crate) fn take_database_notices(&mut self) -> Vec<(u8, Vec<u8>)> {
        std::mem::take(&mut self.database_notices)
    }

    pub(crate) fn execute(
        &mut self,
        query: &SessionQuery,
        context: &dyn TiDBContext,
    ) -> crate::conn::ConnResult<QueryResult> {
        self.database_notices.clear();
        let value = match query {
            SessionQuery::SetRole(role) => {
                context.pg_set_role(role.clone())?;
                None
            }
            SessionQuery::ResetRole => {
                context.pg_set_role(None)?;
                None
            }
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
            SessionQuery::RoleUtility(command) => {
                context.pg_role_command(command.clone())?;
                None
            }
            SessionQuery::DatabaseUtility(command) => {
                if context.in_transaction() {
                    return Err(crate::conn::ConnError::PgFunction(
                        astersql_session::runtime::PgFunctionError {
                            state: "25001",
                            message: "database administration is not allowed in a transaction"
                                .into(),
                            detail: None,
                            hint: None,
                            position: None,
                        },
                    ));
                }
                for fact in context.pg_database_facts(Some(command.clone()))? {
                    if let Some((state, message)) = fact.warning {
                        self.database_notices.push((
                            b'N',
                            format!("SWARNING\0VWARNING\0C{state}\0M{message}\0\0").into_bytes(),
                        ));
                    }
                }
                None
            }
            SessionQuery::SchemaUtility(command) => {
                if matches!(
                    command,
                    astersql_session::runtime::PgSchemaCommand::FunctionPrivileges { .. }
                        | astersql_session::runtime::PgSchemaCommand::TypePrivileges { .. }
                        | astersql_session::runtime::PgSchemaCommand::Privileges { .. }
                        | astersql_session::runtime::PgSchemaCommand::TablePrivileges { .. }
                ) {
                    if let Some((state, message)) = context.pg_schema_privileges(command.clone())? {
                        self.database_notices.push((
                            b'N',
                            format!("SWARNING\0VWARNING\0C{state}\0M{message}\0\0").into_bytes(),
                        ));
                    }
                } else {
                    context
                        .pg_schema_path(self.path.clone(), Some(command.clone()))
                        .ok_or_else(|| {
                            crate::conn::ConnError::Session(
                                "PostgreSQL schema metadata unavailable".into(),
                            )
                        })??;
                }
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
            Self::SetRole(_) => "SET",
            Self::ResetRole => "RESET",
            Self::Sequence(command, _) => match command {
                astersql_session::runtime::PgSequenceCommand::AlterIdentity { .. } => "ALTER TABLE",
                astersql_session::runtime::PgSequenceCommand::Create { .. } => "CREATE SEQUENCE",
                astersql_session::runtime::PgSequenceCommand::Drop { .. } => "DROP SEQUENCE",
                astersql_session::runtime::PgSequenceCommand::OwnedBy { .. }
                | astersql_session::runtime::PgSequenceCommand::Rename { .. } => "ALTER SEQUENCE",
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
            Self::RoleUtility(command) => {
                match command {
                    astersql_session::runtime::PgRoleCommand::Create { .. } => "CREATE ROLE",
                    astersql_session::runtime::PgRoleCommand::Drop { .. } => "DROP ROLE",
                    astersql_session::runtime::PgRoleCommand::Membership { grant, .. } => {
                        if *grant { "GRANT ROLE" } else { "REVOKE ROLE" }
                    }
                }
            }
            Self::DatabaseUtility(command) => match command {
                astersql_session::runtime::PgDatabaseCommand::Owner { .. }
                | astersql_session::runtime::PgDatabaseCommand::Policy { .. } => "ALTER DATABASE",
                astersql_session::runtime::PgDatabaseCommand::Privileges {
                    grant: true, ..
                }
                | astersql_session::runtime::PgDatabaseCommand::Connect { grant: true, .. } => {
                    "GRANT"
                }
                astersql_session::runtime::PgDatabaseCommand::Privileges {
                    grant: false, ..
                }
                | astersql_session::runtime::PgDatabaseCommand::Connect { grant: false, .. } => {
                    "REVOKE"
                }
            },
            Self::SchemaUtility(command) => match command {
                astersql_session::runtime::PgSchemaCommand::Create(_)
                | astersql_session::runtime::PgSchemaCommand::CreateOwned { .. } => "CREATE SCHEMA",
                astersql_session::runtime::PgSchemaCommand::Drop(_) => "DROP SCHEMA",
                astersql_session::runtime::PgSchemaCommand::Rename { .. }
                | astersql_session::runtime::PgSchemaCommand::Owner { .. } => "ALTER SCHEMA",
                astersql_session::runtime::PgSchemaCommand::RelationOwner { .. } => "ALTER TABLE",
                astersql_session::runtime::PgSchemaCommand::FunctionOwner { .. } => {
                    "ALTER FUNCTION"
                }
                astersql_session::runtime::PgSchemaCommand::TypeOwner { domain, .. } => {
                    if *domain {
                        "ALTER DOMAIN"
                    } else {
                        "ALTER TYPE"
                    }
                }
                astersql_session::runtime::PgSchemaCommand::RevokeAll { .. } => "REVOKE",
                astersql_session::runtime::PgSchemaCommand::RelationPrivilege {
                    grant: true,
                    ..
                } => "GRANT",
                astersql_session::runtime::PgSchemaCommand::RelationPrivilege {
                    grant: false,
                    ..
                } => "REVOKE",
                astersql_session::runtime::PgSchemaCommand::TablePrivileges {
                    grant: true, ..
                }
                | astersql_session::runtime::PgSchemaCommand::FunctionPrivileges {
                    grant: true,
                    ..
                }
                | astersql_session::runtime::PgSchemaCommand::TypePrivileges {
                    grant: true, ..
                }
                | astersql_session::runtime::PgSchemaCommand::Privileges { grant: true, .. }
                | astersql_session::runtime::PgSchemaCommand::Usage { grant: true, .. } => "GRANT",
                astersql_session::runtime::PgSchemaCommand::TablePrivileges {
                    grant: false,
                    ..
                }
                | astersql_session::runtime::PgSchemaCommand::FunctionPrivileges {
                    grant: false,
                    ..
                }
                | astersql_session::runtime::PgSchemaCommand::TypePrivileges {
                    grant: false, ..
                }
                | astersql_session::runtime::PgSchemaCommand::Privileges { grant: false, .. }
                | astersql_session::runtime::PgSchemaCommand::Usage { grant: false, .. } => {
                    "REVOKE"
                }
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
        // Reject unsupported storage declarations before sequence adapters,
        // native parsing, or metadata publication. Quoted identifiers, strings,
        // comments, and column names inside parentheses are not clauses.
        let word_at =
            |i: usize, value: &str| matches!(tokens.get(i), Some(Token::Word(w)) if w == value);
        if word_at(0, "create") || word_at(0, "alter") || word_at(0, "drop") {
            // Neither native metadata nor the PG adapter can store/enforce row
            // policies. Gate every policy mutation before native parsing.
            if word_at(1, "policy") {
                return Err((
                    "0A000",
                    "PostgreSQL row security policies are not supported".into(),
                ));
            }
            if word_at(0, "alter") && word_at(1, "table") {
                let mut action = 2;
                if word_at(action, "if") && word_at(action + 1, "exists") {
                    action += 2;
                }
                action += usize::from(word_at(action, "only"));
                // Skip the relation name, including a qualified/quoted name and
                // optional inheritance marker. A column named security or a
                // string containing the clause must not trigger this gate.
                if matches!(tokens.get(action), Some(Token::Word(_) | Token::Quoted(_))) {
                    action += 1;
                    while tokens.get(action) == Some(&Token::Symbol('.'))
                        && matches!(
                            tokens.get(action + 1),
                            Some(Token::Word(_) | Token::Quoted(_))
                        )
                    {
                        action += 2;
                    }
                    if tokens.get(action) == Some(&Token::Symbol('*')) {
                        action += 1;
                    }
                    let mut depth = 0i32;
                    for i in action..tokens.len() {
                        if i == action {
                            let clause = if word_at(i, "no") && word_at(i + 1, "force") {
                                Some(i + 2)
                            } else if word_at(i, "enable")
                                || word_at(i, "disable")
                                || word_at(i, "force")
                            {
                                Some(i + 1)
                            } else {
                                None
                            };
                            if clause.is_some_and(|j| {
                                word_at(j, "row")
                                    && word_at(j + 1, "level")
                                    && word_at(j + 2, "security")
                            }) {
                                return Err((
                                    "0A000",
                                    "PostgreSQL row level security is not supported".into(),
                                ));
                            }
                        }
                        match &tokens[i] {
                            Token::Symbol('(') => depth += 1,
                            Token::Symbol(')') => depth -= 1,
                            Token::Symbol(',') if depth == 0 => action = i + 1,
                            _ => {}
                        }
                    }
                }
            }
            // No FDW object can be installed. Refuse before native parsing and
            // never include option values (which can contain secrets) in errors.
            if word_at(1, "server")
                || (word_at(1, "foreign")
                    && (word_at(2, "table") || (word_at(2, "data") && word_at(3, "wrapper"))))
            {
                return Err((
                    "0A000",
                    "PostgreSQL foreign objects are not supported".into(),
                ));
            }

            let mut depth = 0i32;
            for (i, token) in tokens.iter().enumerate() {
                match token {
                    Token::Symbol('(') => depth += 1,
                    Token::Symbol(')') => depth -= 1,
                    Token::Word(w) if depth == 0 && (w == "as" || w == "select") => break,
                    Token::Word(w) if w == "tablespace" => {
                        let object = i == 1;
                        let clause = depth == 0
                            && i > 2
                            && matches!(tokens.get(i + 1), Some(Token::Word(_) | Token::Quoted(_)))
                            && !matches!(tokens.get(i - 1), Some(Token::Symbol('.')))
                            && !matches!(tokens.get(i-1), Some(Token::Word(w)) if matches!(w.as_str(), "table" | "index" | "sequence" | "schema" | "database" | "view" | "column" | "to" | "on" | "add" | "alter" | "drop"));
                        let constraint_clause = depth > 0
                            && i >= 2
                            && word_at(i - 1, "index")
                            && word_at(i - 2, "using");
                        if object || clause || constraint_clause {
                            return Err((
                                "0A000",
                                "PostgreSQL tablespaces are not supported".into(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        let relation_acl = matches!(tokens.first(), Some(Token::Word(w)) if w == "grant" || w == "revoke")
            && tokens.windows(2).any(|w| matches!((&w[0], &w[1]), (Token::Word(a), Token::Word(b)) if a == "on" && b == "sequence"));
        if let Some((command, name)) = if relation_acl {
            None
        } else {
            crate::pg_sequence::parse(&tokens)?
        } {
            return Ok(Some(Self::Sequence(command, name)));
        }
        let word = |token: Option<&Token>, expected: &str| matches!(token,Some(Token::Word(w)) if w==expected);
        // Shared objects have no durable comment source yet. Refuse before
        // native parsing or writes, including comment removal requests.
        if word(tokens.first(), "comment")
            && word(tokens.get(1), "on")
            && ["database", "role", "tablespace"]
                .iter()
                .any(|target| word(tokens.get(2), target))
        {
            return Err((
                "0A000",
                "shared object comments have no persistent source".into(),
            ));
        }
        if (word(tokens.first(), "create") || word(tokens.first(), "alter"))
            && word(tokens.get(1), "table")
            && tokens.windows(3).any(|w| {
                word(w.first(), "nulls") && word(w.get(1), "not") && word(w.get(2), "distinct")
            })
        {
            return Err(("0A000", "NULLS NOT DISTINCT keys are not supported".into()));
        }
        use astersql_session::runtime::PgSchemaCommand;
        let identifier = |token: &Token| match token {
            Token::Word(name) | Token::Quoted(name) => Some(name.clone()),
            _ => None,
        };
        if word(tokens.first(), "reset") && word(tokens.get(1), "role") {
            if tokens.len() != 2 {
                return Err(("42601", "unexpected RESET ROLE clause".into()));
            }
            return Ok(Some(Self::ResetRole));
        }
        if word(tokens.first(), "set") && word(tokens.get(1), "role") {
            let mut index = 2;
            if word(tokens.get(index), "to") || tokens.get(index) == Some(&Token::Symbol('=')) {
                index += 1;
            }
            if tokens.len() != index + 1 {
                return Err(("42601", "expected one role name".into()));
            }
            let role = if word(tokens.get(index), "none") {
                None
            } else {
                Some(match &tokens[index] {
                    Token::Word(name) | Token::Quoted(name) | Token::String(name) => name.clone(),
                    _ => return Err(("42601", "expected role name".into())),
                })
            };
            return Ok(Some(Self::SetRole(role)));
        }
        // A single role/member form; object privilege GRANT stays with its owner.
        if (word(tokens.first(), "grant") || word(tokens.first(), "revoke"))
            && (word(tokens.get(2), "to") || word(tokens.get(2), "from"))
        {
            let grant = word(tokens.first(), "grant");
            if !word(tokens.get(2), if grant { "to" } else { "from" }) {
                return Err(("42601", "invalid membership syntax".into()));
            }
            let role = tokens
                .get(1)
                .and_then(identifier)
                .ok_or_else(|| ("42601", "expected role".into()))?;
            let member = tokens
                .get(3)
                .and_then(identifier)
                .ok_or_else(|| ("42601", "expected member".into()))?;
            let (mut admin, mut inherit, mut set) = (None, None, None);
            let mut i = 4;
            if i < tokens.len() {
                if !grant || !word(tokens.get(i), "with") {
                    return Err(("0A000", "unsupported membership clause".into()));
                }
                i += 1;
                loop {
                    let option = match tokens.get(i) {
                        Some(Token::Word(s)) => s.as_str(),
                        _ => return Err(("42601", "expected membership option".into())),
                    };
                    i += 1;
                    let value = if option == "admin" && word(tokens.get(i), "option") {
                        true
                    } else if word(tokens.get(i), "true") {
                        true
                    } else if word(tokens.get(i), "false") {
                        false
                    } else {
                        return Err(("42601", "expected option value".into()));
                    };
                    let slot = match option {
                        "admin" => &mut admin,
                        "inherit" => &mut inherit,
                        "set" => &mut set,
                        _ => return Err(("42601", "unknown membership option".into())),
                    };
                    if slot.replace(value).is_some() {
                        return Err(("42601", "duplicate membership option".into()));
                    }
                    i += 1;
                    if i == tokens.len() {
                        break;
                    }
                    if tokens.get(i) != Some(&Token::Symbol(',')) {
                        return Err(("42601", "expected option separator".into()));
                    }
                    i += 1;
                }
            }
            return Ok(Some(Self::RoleUtility(
                astersql_session::runtime::PgRoleCommand::Membership {
                    role,
                    member,
                    grant,
                    admin,
                    inherit,
                    set,
                },
            )));
        }
        if (word(tokens.first(), "create") || word(tokens.first(), "drop"))
            && (word(tokens.get(1), "role") || word(tokens.get(1), "user"))
        {
            use astersql_session::runtime::PgRoleCommand;
            let name = tokens
                .get(2)
                .and_then(identifier)
                .ok_or_else(|| ("42601", "expected role name".into()))?;
            let command = if word(tokens.first(), "drop") {
                if tokens.len() != 3 {
                    return Err(("0A000", "unsupported DROP ROLE form".into()));
                }
                PgRoleCommand::Drop { name }
            } else {
                if word(tokens.get(3), "with") && tokens.len() == 4 {
                    return Err(("42601", "expected role option after WITH".into()));
                }
                let mut can_login = word(tokens.get(1), "user");
                let mut login_seen = false;
                for (index, token) in tokens.iter().enumerate().skip(3) {
                    match token {
                        Token::Word(v) if v == "with" && index == 3 => {}
                        Token::Word(v) if v == "login" || v == "nologin" => {
                            if login_seen {
                                return Err((
                                    "42601",
                                    "conflicting or redundant role options".into(),
                                ));
                            }
                            login_seen = true;
                            can_login = v == "login";
                        }
                        Token::Word(v)
                            if matches!(
                                v.as_str(),
                                "nosuperuser" | "nocreatedb" | "noreplication" | "nobypassrls"
                            ) => {}
                        _ => return Err(("0A000", "unsupported role attribute".into())),
                    }
                }
                PgRoleCommand::Create { name, can_login }
            };
            return Ok(Some(Self::RoleUtility(command)));
        }
        if word(tokens.first(), "alter") && word(tokens.get(1), "function") {
            let mut index = 2;
            let first = tokens.get(index).and_then(&identifier);
            index += 1;
            let (schema, name) = if tokens.get(index) == Some(&Token::Symbol('.')) {
                index += 1;
                let name = tokens.get(index).and_then(&identifier);
                index += 1;
                (first, name)
            } else {
                (None, first)
            };
            if tokens.get(index) == Some(&Token::Symbol('(')) {
                index += 1;
                // All three accepted primitives have exactly one oid input.
                let qualified = tokens.get(index).and_then(&identifier).as_deref()
                    == Some("pg_catalog")
                    && tokens.get(index + 1) == Some(&Token::Symbol('.'));
                if qualified {
                    index += 2;
                }
                let oid = tokens.get(index).and_then(&identifier).as_deref() == Some("oid");
                index += 1;
                if oid
                    && tokens.get(index) == Some(&Token::Symbol(')'))
                    && word(tokens.get(index + 1), "owner")
                    && word(tokens.get(index + 2), "to")
                    && tokens.len() == index + 4
                {
                    if let (Some(name), Some(owner)) =
                        (name, tokens.get(index + 3).and_then(&identifier))
                    {
                        return Ok(Some(Self::SchemaUtility(PgSchemaCommand::FunctionOwner {
                            schema,
                            name,
                            owner,
                        })));
                    }
                }
            }
            // Never consume a compound statement or another function signature.
            return Err((
                "0A000",
                "unsupported ALTER FUNCTION signature or operation".into(),
            ));
        }
        if word(tokens.first(), "alter")
            && (word(tokens.get(1), "table")
                || word(tokens.get(1), "type")
                || word(tokens.get(1), "domain"))
        {
            let parsed =
                if tokens.len() == 6 && word(tokens.get(3), "owner") && word(tokens.get(4), "to") {
                    identifier(&tokens[2])
                        .zip(identifier(&tokens[5]))
                        .map(|(table, owner)| (None, table, owner))
                } else if tokens.len() == 8
                    && matches!(tokens.get(3), Some(Token::Symbol('.')))
                    && word(tokens.get(5), "owner")
                    && word(tokens.get(6), "to")
                {
                    identifier(&tokens[2])
                        .zip(identifier(&tokens[4]))
                        .zip(identifier(&tokens[7]))
                        .map(|((schema, table), owner)| (Some(schema), table, owner))
                } else {
                    None
                };
            if let Some((schema, table, owner)) = parsed {
                let command = if word(tokens.get(1), "table") {
                    PgSchemaCommand::RelationOwner {
                        schema,
                        table,
                        owner,
                    }
                } else {
                    PgSchemaCommand::TypeOwner {
                        domain: word(tokens.get(1), "domain"),
                        schema,
                        name: table,
                        owner,
                    }
                };
                return Ok(Some(Self::SchemaUtility(command)));
            }
        }
        use astersql_session::runtime::PgDatabaseCommand;
        if tokens.len() == 6
            && word(tokens.first(), "alter")
            && word(tokens.get(1), "database")
            && word(tokens.get(3), "owner")
            && word(tokens.get(4), "to")
        {
            if let (Some(database), Some(role)) = (identifier(&tokens[2]), identifier(&tokens[5])) {
                return Ok(Some(Self::DatabaseUtility(PgDatabaseCommand::Owner {
                    database,
                    role,
                })));
            }
        }
        // Table ACLs share namespace/database parsing boundaries, retaining the old column adapter.
        if word(tokens.first(), "grant") || word(tokens.first(), "revoke") {
            if let Some(on) = tokens.iter().position(|t| word(Some(t), "on")) {
                if !word(tokens.get(on + 1), "schema") && !word(tokens.get(on + 1), "database") {
                    let grant = word(tokens.first(), "grant");
                    let only_option = !grant
                        && word(tokens.get(1), "grant")
                        && word(tokens.get(2), "option")
                        && word(tokens.get(3), "for");
                    let start = if only_option { 4 } else { 1 };
                    let mut modes = Vec::new();
                    let mut columns = Vec::new();
                    let mut at_mode = start;
                    while at_mode < on {
                        let mode = tokens
                            .get(at_mode)
                            .and_then(identifier)
                            .ok_or_else(|| ("42601", "expected privilege".into()))?
                            .to_ascii_uppercase();
                        modes.push(mode);
                        at_mode += 1;
                        if tokens.get(at_mode) == Some(&Token::Symbol('(')) {
                            at_mode += 1;
                            loop {
                                columns.push(
                                    tokens
                                        .get(at_mode)
                                        .and_then(identifier)
                                        .ok_or_else(|| ("42601", "expected column".into()))?,
                                );
                                at_mode += 1;
                                if tokens.get(at_mode) != Some(&Token::Symbol(',')) {
                                    break;
                                }
                                at_mode += 1;
                            }
                            if tokens.get(at_mode) != Some(&Token::Symbol(')')) {
                                return Err(("42601", "expected closing parenthesis".into()));
                            }
                            at_mode += 1;
                            // Distinct privilege column lists require separate commands; never apply their union.
                            if at_mode < on {
                                return Err((
                                    "0A000",
                                    "send column privilege lists separately".into(),
                                ));
                            }
                        }
                        if word(tokens.get(at_mode), "privileges") && modes == ["ALL"] {
                            at_mode += 1;
                        }
                        if at_mode < on {
                            if tokens.get(at_mode) != Some(&Token::Symbol(',')) {
                                return Err(("42601", "expected privilege separator".into()));
                            }
                            at_mode += 1;
                        }
                    }
                    let privileges = modes.join(",");
                    if privileges != "ALL" {
                        (if word(tokens.get(on + 1), "function") {
                            astersql_session::runtime::pg_function_privileges
                        } else if word(tokens.get(on + 1), "type")
                            || word(tokens.get(on + 1), "domain")
                        {
                            astersql_session::runtime::pg_type_privileges
                        } else if word(tokens.get(on + 1), "sequence") {
                            astersql_session::runtime::pg_sequence_privileges
                        } else {
                            astersql_session::runtime::pg_table_privileges
                        })(&privileges)
                        .map_err(|_| ("42601", "invalid table privilege".into()))?;
                    }
                    let at = on
                        + 1
                        + usize::from(
                            word(tokens.get(on + 1), "function")
                                || word(tokens.get(on + 1), "type")
                                || word(tokens.get(on + 1), "domain")
                                || word(tokens.get(on + 1), "table")
                                || word(tokens.get(on + 1), "sequence"),
                        );
                    let first = tokens
                        .get(at)
                        .and_then(identifier)
                        .ok_or_else(|| ("42601", "expected relation".into()))?;
                    let (schema, table, mut end) =
                        if tokens.get(at + 1) == Some(&Token::Symbol('.')) {
                            (
                                Some(first),
                                tokens
                                    .get(at + 2)
                                    .and_then(identifier)
                                    .ok_or_else(|| ("42601", "expected relation".into()))?,
                                at + 3,
                            )
                        } else {
                            (None, first, at + 1)
                        };
                    let function = word(tokens.get(on + 1), "function");
                    let mut function_arguments = Vec::new();
                    if function {
                        if tokens.get(end) != Some(&Token::Symbol('(')) {
                            return Err(("42601", "expected function signature".into()));
                        }
                        end += 1;
                        while tokens.get(end) != Some(&Token::Symbol(')')) {
                            if word(tokens.get(end), "pg_catalog")
                                && tokens.get(end + 1) == Some(&Token::Symbol('.'))
                            {
                                end += 2;
                            }
                            function_arguments.push(match tokens.get(end) {
                                Some(Token::Word(t)) => match t.as_str() {
                                    "oid" => 26,
                                    "integer" | "int4" | "int" => 23,
                                    "bigint" | "int8" => 20,
                                    "text" => 25,
                                    _ => return Err(("42883", "function does not exist".into())),
                                },
                                _ => return Err(("42601", "expected function input type".into())),
                            });
                            end += 1;
                            if tokens.get(end) == Some(&Token::Symbol(',')) {
                                end += 1;
                            } else if tokens.get(end) != Some(&Token::Symbol(')')) {
                                return Err((
                                    "42601",
                                    "expected function signature delimiter".into(),
                                ));
                            }
                        }
                        end += 1;
                    }
                    if !word(tokens.get(end), if grant { "to" } else { "from" }) {
                        return Err(("0A000", "expected a single table grantee".into()));
                    }
                    let grantee = tokens
                        .get(end + 1)
                        .and_then(identifier)
                        .ok_or_else(|| ("42601", "expected grantee".into()))?;
                    let public = word(tokens.get(end + 1), "public");
                    let suffix = &tokens[end + 2..];
                    let option = only_option
                        || grant
                            && suffix.len() == 3
                            && word(suffix.first(), "with")
                            && word(suffix.get(1), "grant")
                            && word(suffix.get(2), "option");
                    let cascade = !grant && suffix.len() == 1 && word(suffix.first(), "cascade");
                    if !suffix.is_empty()
                        && !(grant && option
                            || !grant
                                && suffix.len() == 1
                                && (cascade || word(suffix.first(), "restrict")))
                    {
                        return Err(("0A000", "unsupported table grant syntax".into()));
                    }
                    if function {
                        if !columns.is_empty() {
                            return Err((
                                "42601",
                                "function privileges cannot specify columns".into(),
                            ));
                        }
                        return Ok(Some(Self::SchemaUtility(
                            PgSchemaCommand::FunctionPrivileges {
                                arguments: function_arguments,
                                schema,
                                name: table,
                                grantee,
                                public,
                                privileges,
                                grant,
                                grant_option: option,
                                cascade,
                            },
                        )));
                    }
                    if word(tokens.get(on + 1), "type") || word(tokens.get(on + 1), "domain") {
                        if !columns.is_empty() {
                            return Err(("42601", "type privileges cannot specify columns".into()));
                        }
                        return Ok(Some(Self::SchemaUtility(PgSchemaCommand::TypePrivileges {
                            domain: word(tokens.get(on + 1), "domain"),
                            schema,
                            name: table,
                            grantee,
                            public,
                            privileges,
                            grant,
                            grant_option: option,
                            cascade,
                        })));
                    }
                    return Ok(Some(Self::SchemaUtility(
                        PgSchemaCommand::TablePrivileges {
                            sequence: word(tokens.get(on + 1), "sequence"),
                            columns,
                            schema,
                            table,
                            grantee,
                            public,
                            privileges,
                            grant,
                            grant_option: option,
                            cascade,
                        },
                    )));
                }
            }
        }
        // Object ACL syntax dispatches to the existing database/namespace persistent records.
        if word(tokens.first(), "grant") || word(tokens.first(), "revoke") {
            if let Some(on) = tokens.iter().position(|t| word(Some(t), "on")) {
                if word(tokens.get(on + 1), "database") || word(tokens.get(on + 1), "schema") {
                    let schema_acl = word(tokens.get(on + 1), "schema");
                    let grant = word(tokens.first(), "grant");
                    let only_option = !grant
                        && word(tokens.get(1), "grant")
                        && word(tokens.get(2), "option")
                        && word(tokens.get(3), "for");
                    let start = if only_option { 4 } else { 1 };
                    let mut privileges = Vec::new();
                    for (index, token) in tokens[start..on].iter().enumerate() {
                        if index % 2 == 1 {
                            if word(Some(token), "privileges")
                                && privileges == ["ALL"]
                                && start + index + 1 == on
                            {
                                continue;
                            }
                            if *token != Token::Symbol(',') {
                                return Err((
                                    "42601",
                                    "expected database privilege separator".into(),
                                ));
                            }
                        } else if let Some(name) = identifier(token) {
                            privileges.push(name.to_ascii_uppercase());
                        } else {
                            return Err(("42601", "expected database privilege".into()));
                        }
                    }
                    let privileges = privileges.join(",");
                    if privileges != "ALL" {
                        (if schema_acl {
                            astersql_session::runtime::pg_schema_privileges(&privileges)
                        } else {
                            astersql_session::runtime::pg_database_privileges(&privileges)
                        })
                        .map_err(|_| ("42601", "invalid database privilege".into()))?;
                    }
                    let database = tokens
                        .get(on + 2)
                        .and_then(identifier)
                        .ok_or_else(|| ("42601", "expected database name".into()))?;
                    if !word(tokens.get(on + 3), if grant { "to" } else { "from" }) {
                        return Err(("42601", "expected database grantee".into()));
                    }
                    let mut role = tokens
                        .get(on + 4)
                        .and_then(identifier)
                        .ok_or_else(|| ("42601", "expected database grantee".into()))?;
                    let public = word(tokens.get(on + 4), "public");
                    if public {
                        role = "PUBLIC".into();
                    }
                    let suffix = &tokens[on + 5..];
                    let option = only_option
                        || grant
                            && suffix.len() == 3
                            && word(suffix.first(), "with")
                            && word(suffix.get(1), "grant")
                            && word(suffix.get(2), "option");
                    let cascade = !grant && suffix.len() == 1 && word(suffix.first(), "cascade");
                    if !suffix.is_empty()
                        && !(grant && option
                            || !grant
                                && suffix.len() == 1
                                && (cascade || word(suffix.first(), "restrict")))
                    {
                        return Err(("0A000", "unsupported database grant syntax".into()));
                    }
                    if schema_acl {
                        return Ok(Some(Self::SchemaUtility(PgSchemaCommand::Privileges {
                            schema: database,
                            grantee: role,
                            public,
                            privileges,
                            grant,
                            grant_option: option,
                            cascade,
                        })));
                    }
                    if privileges == "CONNECT"
                        && !option
                        && !cascade
                        && suffix.is_empty()
                        && (public || role != "PUBLIC")
                    {
                        return Ok(Some(Self::DatabaseUtility(PgDatabaseCommand::Connect {
                            database,
                            role,
                            grant,
                        })));
                    }
                    return Ok(Some(Self::DatabaseUtility(PgDatabaseCommand::Privileges {
                        database,
                        role,
                        public,
                        privileges,
                        grant,
                        grant_option: option,
                        cascade,
                    })));
                }
            }
        }
        if tokens.len() == 7
            && (word(tokens.first(), "grant") || word(tokens.first(), "revoke"))
            && word(tokens.get(1), "connect")
            && word(tokens.get(2), "on")
            && word(tokens.get(3), "database")
        {
            let grant = word(tokens.first(), "grant");
            if word(tokens.get(5), if grant { "to" } else { "from" }) {
                if let (Some(database), Some(mut role)) =
                    (identifier(&tokens[4]), identifier(&tokens[6]))
                {
                    if word(tokens.get(6), "public") {
                        role = "PUBLIC".into();
                    }
                    return Ok(Some(Self::DatabaseUtility(PgDatabaseCommand::Connect {
                        database,
                        role,
                        grant,
                    })));
                }
            }
        }
        if tokens.len() == 5 && word(tokens.first(), "alter") && word(tokens.get(1), "database") {
            let value = if word(tokens.get(4), "true") {
                Some(true)
            } else if word(tokens.get(4), "false") {
                Some(false)
            } else {
                None
            };
            if let (Some(database), Some(value)) = (identifier(&tokens[2]), value) {
                if word(tokens.get(3), "is_template") || word(tokens.get(3), "allow_connections") {
                    return Ok(Some(Self::DatabaseUtility(PgDatabaseCommand::Policy {
                        database,
                        is_template: word(tokens.get(3), "is_template").then_some(value),
                        allow_connections: word(tokens.get(3), "allow_connections")
                            .then_some(value),
                    })));
                }
            }
        }
        if word(tokens.first(), "alter") && word(tokens.get(1), "database") {
            return Err(("0A000", "unsupported PostgreSQL database policy".into()));
        }
        if (word(tokens.first(), "grant") || word(tokens.first(), "revoke"))
            && [
                "select",
                "insert",
                "update",
                "references",
                "delete",
                "trigger",
            ]
            .iter()
            .any(|p| word(tokens.get(1), p))
        {
            use astersql_session::runtime::PgTablePrivilege;
            let privilege = if word(tokens.get(1), "insert") {
                PgTablePrivilege::Insert
            } else if word(tokens.get(1), "update") {
                PgTablePrivilege::Update
            } else if word(tokens.get(1), "references") {
                PgTablePrivilege::References
            } else if word(tokens.get(1), "trigger") {
                PgTablePrivilege::Trigger
            } else if word(tokens.get(1), "delete") {
                PgTablePrivilege::Delete
            } else {
                PgTablePrivilege::Select
            };
            let grant = word(tokens.first(), "grant");
            let (on, column) = if tokens.get(2) == Some(&Token::Symbol('('))
                && tokens.get(4) == Some(&Token::Symbol(')'))
            {
                (5, identifier(&tokens[3]))
            } else {
                (2, None)
            };
            if column.is_some()
                && matches!(
                    privilege,
                    PgTablePrivilege::Delete | PgTablePrivilege::Trigger
                )
            {
                return Err(("0A000", "this privilege does not admit columns".into()));
            }
            if on == 5 && column.is_none() {
                return Err(("42601", "expected a column identifier".into()));
            }
            if tokens.len() == on + 6
                && word(tokens.get(on), "on")
                && tokens.get(on + 2) == Some(&Token::Symbol('.'))
                && word(tokens.get(on + 4), if grant { "to" } else { "from" })
                && let (Some(schema), Some(table), Some(grantee)) = (
                    identifier(&tokens[on + 1]),
                    identifier(&tokens[on + 3]),
                    identifier(&tokens[on + 5]),
                )
            {
                return Ok(Some(Self::SchemaUtility(
                    PgSchemaCommand::RelationPrivilege {
                        privilege,
                        schema,
                        table,
                        grantee,
                        column,
                        grant,
                    },
                )));
            }
        }
        if tokens.len() == 5
            && word(tokens.first(), "create")
            && word(tokens.get(1), "schema")
            && word(tokens.get(3), "authorization")
            && let (Some(schema), Some(owner)) = (identifier(&tokens[2]), identifier(&tokens[4]))
        {
            return Ok(Some(Self::SchemaUtility(PgSchemaCommand::CreateOwned {
                schema,
                owner,
            })));
        }
        if tokens.len() == 6
            && word(tokens.first(), "alter")
            && word(tokens.get(1), "schema")
            && word(tokens.get(3), "owner")
            && word(tokens.get(4), "to")
            && let (Some(schema), Some(owner)) = (identifier(&tokens[2]), identifier(&tokens[5]))
        {
            return Ok(Some(Self::SchemaUtility(PgSchemaCommand::Owner {
                schema,
                owner,
            })));
        }
        if tokens.len() == 7
            && word(tokens.first(), "revoke")
            && word(tokens.get(1), "all")
            && word(tokens.get(2), "on")
            && word(tokens.get(3), "schema")
            && word(tokens.get(5), "from")
            && let (Some(schema), Some(mut grantee)) =
                (identifier(&tokens[4]), identifier(&tokens[6]))
        {
            if word(tokens.get(6), "public") {
                grantee = "PUBLIC".into();
            }
            return Ok(Some(Self::SchemaUtility(PgSchemaCommand::RevokeAll {
                schema,
                grantee,
            })));
        }
        if tokens.len() == 6
            && word(tokens.first(), "alter")
            && word(tokens.get(1), "schema")
            && word(tokens.get(3), "rename")
            && word(tokens.get(4), "to")
            && let (Some(schema), Some(name)) = (identifier(&tokens[2]), identifier(&tokens[5]))
        {
            return Ok(Some(Self::SchemaUtility(PgSchemaCommand::Rename {
                schema,
                name,
            })));
        }
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
            && (word(tokens.get(1), "usage") || word(tokens.get(1), "create"))
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
                    create: word(tokens.get(1), "create"),
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
                && (word(tokens.get(1), "usage") || word(tokens.get(1), "create"))
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
