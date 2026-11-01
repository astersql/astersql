// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Source-specific adapter over the canonical SQL parser, logical planner,
//! transaction MDL catalog and session setters; it never evaluates the SELECT.
use super::*;
use astersql_executor_importer::{QuerySessionStates, QuerySource, SourceTable};
use astersql_sessionctx_sessionstates::SessionStates;

fn unsupported_source(message: &str) -> SessionError {
    let error = astersql_util_dbterror_plannererrors::ErrNotSupportedYet
        .GenWithStackByArgs(&[message.into()]);
    SessionError::with_source(error.to_string(), error)
}

struct ResolveSource<'a> {
    session: &'a ConcreteSession,
    database: String,
    in_view: bool,
    tables: BTreeMap<i64, SourceTable>,
    views: HashSet<i64>,
    error: Option<SessionError>,
}
impl ResolveSource<'_> {
    fn resolve(&mut self, source: &mut ast::TableSource) -> SessionResult<()> {
        if source.AsOf.is_some() {
            return Err(unsupported_source("stale reads in IMPORT INTO FROM SELECT"));
        }
        if source.QuerySource.is_some() || source.FunctionSource.is_some() {
            return Ok(());
        }
        let db = if source.Source.Schema.L.is_empty() {
            self.database.clone()
        } else {
            source.Source.Schema.L.clone()
        };
        if matches!(
            db.as_str(),
            "information_schema" | "performance_schema" | "metrics_schema" | "inspection_schema"
        ) {
            return Err(unsupported_source(
                "virtual source tables in IMPORT INTO FROM SELECT",
            ));
        }
        let table = if !self.in_view {
            self.session
                .local_temporary_table(&db, &source.Source.Name.L)
        } else {
            None
        }
        .or_else(|| {
            self.session
                .mdl_stats_table(&db, &source.Source.Name.L)
                .map(|(_, t)| t)
        });
        let Some(table) = table else {
            return Ok(());
        }; // planner diagnoses unresolved names/CTEs
        if table.TempTableType != astersql_meta_model::TempTableNone {
            return Err(unsupported_source(
                "temporary source tables in IMPORT INTO FROM SELECT",
            ));
        }
        if table.IsSequence() {
            return Err(unsupported_source("sequences in IMPORT INTO FROM SELECT"));
        }
        self.tables.insert(
            table.ID,
            SourceTable {
                DBID: table.DBID,
                ID: table.ID,
                Revision: table.Revision,
            },
        );
        source.Source.Schema = ast::NewCIStr(&db);
        if let Some(view) = &table.View {
            if !self.views.insert(table.ID) {
                return Err(SessionError::new("recursive import source view"));
            }
            let mut nodes = parse(&view.SelectStmt)?;
            let create = nodes
                .remove(0)
                .into_any()
                .downcast::<ast::CreateViewStmt>()
                .map_err(|_| SessionError::new("stored view SQL is not CREATE VIEW"))?;
            let mut select = create.Select;
            let previous_db = std::mem::replace(&mut self.database, db);
            let previous_view = std::mem::replace(&mut self.in_view, true);
            ast::Walk(select.as_mut(), self);
            self.database = previous_db;
            self.in_view = previous_view;
            self.views.remove(&table.ID);
            if let Some(error) = self.error.take() {
                return Err(error);
            }
            source.QuerySource = Some(ast::NodeRef::new(select));
            source.ColumnNames = view.Cols.clone();
            if source.AsName.L.is_empty() {
                source.AsName = source.Source.Name.clone();
            }
        }
        Ok(())
    }
}
impl ast::InPlaceVisitor for ResolveSource<'_> {
    fn enter(&mut self, node: &mut dyn ast::Node) -> bool {
        if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>() {
            let message = match &expr.Kind {
                ast::ExprKind::Function { FnName, .. }
                    if matches!(FnName.L.as_str(), "nextval" | "lastval" | "setval") =>
                {
                    Some("sequences in IMPORT INTO FROM SELECT")
                }
                ast::ExprKind::ParamMarker { .. } => Some("unexpected '?'"),
                _ => None,
            };
            if let Some(message) = message {
                self.error = Some(if message == "unexpected '?'" {
                    let error =
                        astersql_parser::ErrParse.GenWithStackByArgs(&[message.into(), 1.into()]);
                    SessionError::with_source(error.to_string(), error)
                } else {
                    unsupported_source(message)
                });
            }
        }
        self.error.is_some()
    }
    fn leave(&mut self, _: &mut dyn ast::Node) -> bool {
        self.error.is_none()
    }
    fn enter_embedded(&mut self, value: &mut dyn Any) -> bool {
        if self.error.is_none()
            && let Some(source) = value.downcast_mut::<ast::TableSource>()
        {
            let was_derived = source.QuerySource.is_some();
            if let Err(error) = self.resolve(source) {
                self.error = Some(error);
            }
            // View children were resolved above under their defining database
            // and without local temporary-table shadowing. Do not walk twice.
            if !was_derived && source.QuerySource.is_some() {
                return true;
            }
        }
        self.error.is_some()
    }
}

impl ConcreteSession {
    /// Parse with the session's real dialect and SQL mode. IMPORT remains MySQL-only.
    pub fn CaptureImportQuerySource(&self, sql: &str) -> SessionResult<QuerySource> {
        self.prepare_import_query_source(sql)
            .map(|(captured, _)| captured)
    }

    pub(super) fn prepare_import_query_source(
        &self,
        sql: &str,
    ) -> SessionResult<(QuerySource, ast::NodeRef)> {
        if self.sql_dialect() != ast::SqlDialect::MySQL {
            return Err(SessionError::new("import query requires the MySQL dialect"));
        }
        let mut parser = Parser::with_dialect(self.sql_dialect());
        parser.SetSQLMode(
            astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
                .map_err(|e| SessionError::new(e.to_string()))?,
        );
        let (mut nodes, _) = parser
            .ParseSQL(sql, &[])
            .map_err(|e| session_error("parse import query", e))?;
        if nodes.len() != 1 {
            return Err(SessionError::new("import query requires a SELECT"));
        }
        let statement = nodes
            .remove(0)
            .into_any()
            .downcast::<ast::ImportIntoStmt>()
            .map_err(|_| SessionError::new("import query requires a SELECT"))?;
        let mut query = statement
            .Select
            .ok_or_else(|| SessionError::new("import query requires a SELECT"))?;
        {
            let state = self.state.borrow();
            if state.current_statement_is_stale
                || state.transaction_stale_read_ts.is_some()
                || state.pending_stale_read_ts.is_some()
                || state.session_read_staleness_seconds.is_some()
                || state.session_stale_read_ts.is_some()
            {
                return Err(unsupported_source("stale reads in IMPORT INTO FROM SELECT"));
            }
        }
        let mut resolver = ResolveSource {
            session: self,
            database: self.current_database(),
            in_view: false,
            tables: BTreeMap::new(),
            views: HashSet::new(),
            error: None,
        };
        // IMPORT is a write statement even when this adapter is invoked directly.
        struct Guard<'a>(&'a std::cell::Cell<bool>, bool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _guard = Guard(
            &self.mdl_autocommit_write,
            self.mdl_autocommit_write.replace(true),
        );
        ast::Walk(query.as_mut(), &mut resolver);
        if let Some(error) = resolver.error {
            return Err(error);
        }
        if let Some(error) = self.mdl_metadata_error.borrow_mut().take() {
            return Err(SessionError::new(error));
        }
        let base = self
            .state
            .borrow()
            .transaction_info_schema
            .clone()
            .unwrap_or_else(|| self.domain.info_schema());
        let schema_version = base.SchemaMetaVersion();
        let schema = self.transaction_mdl_schema(base);
        let context = self.AdapterPlanContext();
        let (mut builder, _) = astersql_planner_core::NewPlanBuilder().Init(
            context,
            schema,
            astersql_util_hint::NewQBHintHandler(None),
        );
        let planned_query = ast::NodeRef::new(query);
        builder
            .buildResultSetNode(
                astersql_planner_core::context::TODO(),
                &planned_query,
                false,
            )
            .map_err(|e| session_error("plan import SELECT", e))?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| session_error("capture query clock", e))?
            .as_secs() as i64;
        let mut states = self.encode_import_session_states()?;
        states
            .SystemVars
            .insert("timestamp".into(), self.select_variable("timestamp", true)?);
        states.ResourceGroupName = self
            .state
            .borrow()
            .runaway_resource_group_name
            .clone()
            .unwrap_or_default();
        let pinned = {
            let state = self.state.borrow();
            state
                .snapshot_read_ts
                .or_else(|| {
                    (state.enable_external_ts_read && state.external_read_ts != 0)
                        .then_some(state.external_read_ts)
                })
                .or_else(|| {
                    if state
                        .transaction_isolation
                        .eq_ignore_ascii_case("READ-COMMITTED")
                    {
                        None
                    } else {
                        state.transaction.as_ref().map(|txn| txn.StartTS())
                    }
                })
        };
        let read_ts = match pinned {
            Some(ts) => ts,
            None => {
                self.domain
                    .storage()
                    .with_storage(|store| store.CurrentVersion("global"))
                    .map_err(|e| session_error("import query read timestamp", e))?
                    .Ver
            }
        };
        Ok((
            QuerySource {
                ReadTS: read_ts,
                SchemaVersion: schema_version,
                Timestamp: timestamp,
                Tables: resolver.tables.into_values().collect(),
                SessionStates: states,
            },
            planned_query,
        ))
    }

    pub fn CheckCanMigrateStates(&self) -> SessionResult<()> {
        let state = self.state.borrow();
        let reason = if state.transaction.is_some() {
            Some("session has an active transaction")
        } else if !state.local_temporary_tables.is_empty() {
            Some("session has local temporary tables")
        } else if super::system_query::RUNTIME_USER_LOCKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|((domain, _), (owner, _))| {
                *domain == runtime_domain_id(&self.domain) && *owner == self.connection_id()
            })
        {
            Some("session has advisory locks")
        } else {
            None
        };
        if let Some(reason) = reason {
            let error = astersql_sessionctx_sessionstates::ErrCannotMigrateSession
                .GenWithStackByArgs(&[reason.into()]);
            return Err(SessionError::with_source(error.to_string(), error));
        }
        Ok(())
    }
}
impl QuerySessionStates for ConcreteSession {
    fn Status(&self) -> u32 {
        let state = self.state.borrow();
        (if state.autocommit { 2 } else { 0 }) | (if state.transaction.is_some() { 1 } else { 0 })
    }
    fn DecodeQueryStates(&self, states: &SessionStates) -> Result<(), String> {
        let json = serde_json::to_string(states).map_err(|e| e.to_string())?;
        self.restore_migration_system_vars(&json)
            .map_err(|e| e.to_string())?;
        let mut state = self.state.borrow_mut();
        state.current_database = states.CurrentDB.clone();
        state.runaway_resource_group_name =
            (!states.ResourceGroupName.is_empty()).then(|| states.ResourceGroupName.clone());
        state.user_variables.clear();
        state.string_user_variables.clear();
        for (name, datum) in &states.UserVars {
            let string = datum.Kind() == astersql_types::datum::KindString;
            let value = datum.ToString().map_err(|e| e.to_string())?;
            state.user_variables.insert(name.clone(), value);
            if string {
                state.string_user_variables.insert(name.clone());
            }
        }
        state.last_warnings.clear();
        state.current_warnings.clear();
        Ok(())
    }
}
