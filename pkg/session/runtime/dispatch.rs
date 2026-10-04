// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use super::*;

struct SessionPlanReplaySource<'a>(&'a ConcreteSession);

impl astersql_domain::plan_replayer_dump::PlanReplaySource for SessionPlanReplaySource<'_> {
    fn current_database(&self) -> String {
        self.0.current_database()
    }

    fn resolve_table(
        &self,
        database: &str,
        table: &str,
    ) -> Result<Option<astersql_domain::plan_replayer_dump::TableNamePair>, String> {
        Ok(self.0.resolve_runtime_table(database, table).map(|info| {
            astersql_domain::plan_replayer_dump::TableNamePair {
                database: database.to_owned(),
                table: table.to_owned(),
                is_view: info.View.is_some(),
            }
        }))
    }

    fn view_dependencies(
        &self,
        view: &astersql_domain::plan_replayer_dump::TableNamePair,
    ) -> Result<Vec<astersql_domain::plan_replayer_dump::TableNamePair>, String> {
        let Some(info) = self.0.resolve_runtime_table(&view.database, &view.table) else {
            return Ok(Vec::new());
        };
        let Some(view_info) = info.View.as_ref() else {
            return Ok(Vec::new());
        };
        let mut dependencies = Vec::new();
        for (database, table) in astersql_domain::plan_replayer_dump::extract_table_references(
            &view_info.SelectStmt,
            &view.database,
        ) {
            if let Some(dependency) = self.resolve_table(&database, &table)? {
                dependencies.push(dependency);
            }
        }
        Ok(dependencies)
    }

    fn table_dependencies(
        &self,
        table: &astersql_domain::plan_replayer_dump::TableNamePair,
    ) -> Result<Vec<astersql_domain::plan_replayer_dump::TableNamePair>, String> {
        let Some(info) = self.0.resolve_runtime_table(&table.database, &table.table) else {
            return Ok(Vec::new());
        };
        let mut dependencies = Vec::new();
        for foreign_key in &info.ForeignKeys {
            let database = if foreign_key.RefSchema.L.is_empty() {
                table.database.as_str()
            } else {
                foreign_key.RefSchema.L.as_str()
            };
            if let Some(dependency) = self.resolve_table(database, &foreign_key.RefTable.L)? {
                dependencies.push(dependency);
            }
        }
        Ok(dependencies)
    }

    fn show_create(
        &self,
        table: &astersql_domain::plan_replayer_dump::TableNamePair,
    ) -> Result<String, String> {
        let database = table.database.replace('`', "``");
        let name = table.table.replace('`', "``");
        let command = if table.is_view {
            format!("SHOW CREATE VIEW `{database}`.`{name}`")
        } else {
            format!("SHOW CREATE TABLE `{database}`.`{name}`")
        };
        let mut sets = self
            .0
            .execute(&command)
            .map_err(|error| error.to_string())?;
        let row = sets
            .first_mut()
            .ok_or_else(|| {
                format!(
                    "SHOW CREATE returned no result for {}.{}",
                    table.database, table.table
                )
            })?
            .next_row()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("table {}.{} not found", table.database, table.table))?;
        let mut create_sql = row.get(1).cloned().ok_or_else(|| {
            format!(
                "SHOW CREATE returned no SQL for {}.{}",
                table.database, table.table
            )
        })?;
        // The runtime SHOW formatter does not yet restore column comments.
        // Plan Replayer archives must retain them verbatim because semicolons
        // inside comments are a documented LOAD regression case.
        if let Some(info) = self.0.resolve_runtime_table(&table.database, &table.table) {
            for column in &info.Columns {
                if column.Comment.is_empty() {
                    continue;
                }
                let marker = format!("  `{}` ", column.Name.O.replace('`', "``"));
                let Some(start) = create_sql.find(&marker) else {
                    continue;
                };
                let end = create_sql[start..]
                    .find('\n')
                    .map(|offset| start + offset)
                    .unwrap_or(create_sql.len());
                if create_sql[start..end].contains(" COMMENT ") {
                    continue;
                }
                let insertion = if create_sql[..end].ends_with(',') {
                    end - 1
                } else {
                    end
                };
                let comment = column.Comment.replace('\\', "\\\\").replace('\'', "''");
                create_sql.insert_str(insertion, &format!(" COMMENT '{comment}'"));
            }
        }
        Ok(create_sql)
    }

    fn stats(
        &self,
        _table: &astersql_domain::plan_replayer_dump::TableNamePair,
        _historical_ts: u64,
    ) -> Result<(String, Option<String>), String> {
        Ok(("{}".to_owned(), None))
    }

    fn stats_memory_status(
        &self,
        _table: &astersql_domain::plan_replayer_dump::TableNamePair,
    ) -> Result<String, String> {
        Ok("loaded".to_owned())
    }

    fn tiflash_replica(
        &self,
        _table: &astersql_domain::plan_replayer_dump::TableNamePair,
    ) -> Result<String, String> {
        Ok(String::new())
    }

    fn config(&self) -> Result<String, String> {
        Ok("[instance]\nsource = \"astersql\"\n".to_owned())
    }

    fn metadata(&self) -> Result<String, String> {
        Ok("AsterSQL plan replayer\n".to_owned())
    }

    fn global_bindings(&self) -> Result<Vec<String>, String> {
        Ok(self
            .0
            .bindings
            .borrow()
            .Bindings(true)
            .into_iter()
            .map(|binding| {
                format!(
                    "CREATE GLOBAL BINDING FOR {} USING {};",
                    binding.OriginalSQL, binding.BindSQL
                )
            })
            .collect())
    }

    fn explain(&self, sql: &str, analyze: bool) -> Result<(String, Option<String>), String> {
        let command = if analyze {
            format!("EXPLAIN ANALYZE {sql}")
        } else {
            format!("EXPLAIN {sql}")
        };
        let sets = self
            .0
            .execute(&command)
            .map_err(|error| error.to_string())?;
        let text = sets
            .into_iter()
            .flat_map(|set| set.rows)
            .map(|row| row.join("\t"))
            .collect::<Vec<_>>()
            .join("\n");
        Ok((text, Some(String::new())))
    }

    fn decode_plan(&self, encoded_plan: &str) -> Result<String, String> {
        Ok(encoded_plan.to_owned())
    }
}

fn strip_optimizer_hint_comments(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut remaining = sql;
    while let Some(start) = remaining.find("/*+") {
        output.push_str(&remaining[..start]);
        let Some(end) = remaining[start + 3..].find("*/") else {
            output.push_str(&remaining[start..]);
            remaining = "";
            break;
        };
        remaining = &remaining[start + 3 + end + 2..];
        if !output.ends_with(' ') {
            output.push(' ');
        }
    }
    output.push_str(remaining);
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dp_join_reorder_ignores_leading_hint(
    statement: &dyn ast::Node,
    variables: &astersql_sessionctx_variable::session::SessionVars,
) -> bool {
    let Some(select) = statement.as_any().downcast_ref::<ast::SelectStmt>() else {
        return false;
    };
    let threshold = variables
        .GetSystemVar(astersql_sessionctx_vardef::TiDBOptJoinReorderThreshold)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(variables.TiDBOptJoinReorderThreshold);
    let advanced = variables
        .GetSystemVar(astersql_sessionctx_vardef::TiDBOptEnableAdvancedJoinReorder)
        .map(|value| variable_is_on(&value))
        .unwrap_or(variables.TiDBOptEnableAdvancedJoinReorder);
    if !advanced
        || threshold <= 0
        || !select
            .TableHints
            .iter()
            .chain(select.SelectStmtOpts.TableHints.iter())
            .any(|hint| hint.HintName.L.eq_ignore_ascii_case("leading"))
    {
        return false;
    }
    let Some(from) = select.From.as_ref() else {
        return false;
    };
    let mut sources = Vec::new();
    let table_refs = ast::ResultSetNode::Join(Box::new(from.TableRefs.clone()));
    collect_physical_table_sources(&table_refs, &mut sources);
    sources.len() >= 2 && sources.len() <= threshold as usize
}

fn sql_option_u64(sql: &str, option_name: &str) -> Option<u64> {
    let tokens = sql
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    tokens
        .windows(2)
        .rev()
        .find(|pair| pair[0].eq_ignore_ascii_case(option_name))
        .and_then(|pair| pair[1].parse().ok())
}

fn validate_alter_column_charset(sql: &str) -> SessionResult<()> {
    let tokens = sql
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let collations = tokens
        .windows(2)
        .filter(|pair| pair[0] == "collate")
        .map(|pair| pair[1].as_str())
        .collect::<Vec<_>>();
    if collations.len() > 1 {
        return Err(SessionError::new("Multiple COLLATE clauses"));
    }
    let charset = tokens
        .windows(2)
        .rev()
        .find_map(|pair| (pair[0] == "charset" || pair[0] == "set").then_some(pair[1].as_str()));
    let Some((charset, collation)) = charset.zip(collations.last().copied()) else {
        return Ok(());
    };
    let collation_charset = collation
        .strip_prefix("utf8mb4_")
        .map(|_| "utf8mb4")
        .or_else(|| collation.strip_prefix("utf8_").map(|_| "utf8"))
        .or_else(|| collation.strip_prefix("ascii_").map(|_| "ascii"))
        .or_else(|| collation.strip_prefix("binary").map(|_| "binary"));
    if collation_charset.is_some_and(|expected| expected != charset) {
        return Err(SessionError::new(format!(
            "[ddl:1253]COLLATION '{collation}' is not valid for CHARACTER SET '{charset}'"
        )));
    }
    Ok(())
}

fn format_placement_policy_options(options: &[ast::PlacementOption]) -> String {
    options
        .iter()
        .map(|option| {
            let name = match option.Tp {
                ast::PlacementOptionType::PrimaryRegion => "PRIMARY_REGION",
                ast::PlacementOptionType::Regions => "REGIONS",
                ast::PlacementOptionType::FollowerCount => "FOLLOWERS",
                ast::PlacementOptionType::VoterCount => "VOTERS",
                ast::PlacementOptionType::LearnerCount => "LEARNERS",
                ast::PlacementOptionType::Schedule => "SCHEDULE",
                ast::PlacementOptionType::Constraints => "CONSTRAINTS",
                ast::PlacementOptionType::LeaderConstraints => "LEADER_CONSTRAINTS",
                ast::PlacementOptionType::FollowerConstraints => "FOLLOWER_CONSTRAINTS",
                ast::PlacementOptionType::VoterConstraints => "VOTER_CONSTRAINTS",
                ast::PlacementOptionType::LearnerConstraints => "LEARNER_CONSTRAINTS",
                ast::PlacementOptionType::SurvivalPreferences => "SURVIVAL_PREFERENCES",
                ast::PlacementOptionType::Policy => "PLACEMENT POLICY",
            };
            if matches!(
                option.Tp,
                ast::PlacementOptionType::FollowerCount
                    | ast::PlacementOptionType::VoterCount
                    | ast::PlacementOptionType::LearnerCount
            ) {
                format!("{name}={}", option.UintValue)
            } else {
                let value = option.StrValue.replace('\\', "\\\\").replace('"', "\\\"");
                format!("{name}=\"{value}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn binding_sql_parts(sql: &str) -> Option<(String, String)> {
    let lowered = sql.to_ascii_lowercase();
    if let Some(binding_position) = lowered.find(" binding for ") {
        let body = sql[binding_position + " binding for ".len()..]
            .trim()
            .trim_end_matches(';')
            .trim();
        let body_lower = body.to_ascii_lowercase();
        let using_position = body_lower.find(" using ")?;
        return Some((
            body[..using_position].trim().to_owned(),
            body[using_position + " using ".len()..].trim().to_owned(),
        ));
    }
    let binding_position = lowered.find(" binding using ")?;
    let hinted = sql[binding_position + " binding using ".len()..]
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_owned();
    Some((strip_optimizer_hint_comments(&hinted), hinted))
}

impl ConcreteSession {
    pub(super) fn simple_typed_primary_key_predicate(
        &self,
        database: &str,
        table_name: &str,
        predicate: &ast::ExprNode,
    ) -> bool {
        let ast::ExprKind::Binary { Op, L, R } = &predicate.Kind else {
            return false;
        };
        if !matches!(Op.as_str(), "=" | "==") {
            return false;
        }
        let column_name = match (&L.Kind, &R.Kind) {
            (ast::ExprKind::Column(column), ast::ExprKind::ParamMarker { .. })
            | (ast::ExprKind::Column(column), ast::ExprKind::Value(_)) => &column.Name.L,
            (ast::ExprKind::ParamMarker { .. }, ast::ExprKind::Column(column))
            | (ast::ExprKind::Value(_), ast::ExprKind::Column(column)) => &column.Name.L,
            _ => return false,
        };
        self.mdl_stats_table(database, table_name)
            .and_then(|(_, table)| table.GetPkColInfo().map(|column| column.Name.L.clone()))
            .is_some_and(|primary| primary == *column_name)
    }

    pub(super) fn simple_typed_select_shape(select: &ast::SelectStmt) -> bool {
        let Some(from) = select.From.as_ref() else {
            return false;
        };
        matches!(from.TableRefs.Left.as_deref(), Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none())
            && from.TableRefs.Right.is_none()
            && select.With.is_none()
            && select.lock_info.is_none()
            && select.SelectIntoOpt.is_none()
            && !select.Distinct
            && select.SelectStmtOpts.SQLCache
            && !select.SelectStmtOpts.CalcFoundRows
            && !select.SelectStmtOpts.SQLBufferResult
            && select.TableHints.is_empty()
            && select.WindowSpecs.is_empty()
            && select.GroupBy.is_empty()
            && select.Having.is_none()
            && select.Where.as_ref().is_none_or(|predicate| {
                matches!(&predicate.Kind, ast::ExprKind::Binary { Op, .. } if matches!(Op.as_str(), "=" | "=="))
            })
            && select.OrderBy.is_empty()
            && (select.Limit.is_some() || select.Where.is_some())
            && !select.Fields.Fields.is_empty()
            && select.Fields.Fields.iter().all(|field| {
                matches!(
                    field.Expr.as_ref().map(|expr| &expr.Kind),
                    Some(ast::ExprKind::Column(_))
                )
            })
    }

    #[cfg(test)]
    pub fn PreparedNameHasTypedPlanForTest(&self, name: &str) -> bool {
        self.state
            .borrow()
            .prepared_by_name
            .get(&name.to_lowercase())
            .is_some_and(|prepared| prepared.typed_plan_id.is_some())
    }
    fn execute_plan_replayer_load(
        &self,
        statement: &ast::PlanReplayerStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let encoded = std::fs::read(&statement.File)
            .map_err(|error| SessionError::new(format!("read plan replayer file: {error}")))?;
        let archive = astersql_domain::plan_replayer_dump::decode_replay_archive(&encoded)
            .map_err(SessionError::new)?;
        let mut schemas = archive
            .files
            .iter()
            .filter(|(path, _)| {
                (path.starts_with("schema/") && path.ends_with(".schema.txt"))
                    || (path.starts_with("view/") && path.ends_with(".view.txt"))
            })
            .collect::<Vec<_>>();
        // Go loader disables FK checks while replaying schemas, so mutually
        // dependent tables can be restored in deterministic archive order.
        self.execute("SET FOREIGN_KEY_CHECKS = 0")?;
        schemas.sort_by(|left, right| left.0.cmp(right.0));
        for (path, body) in schemas {
            let relative = path
                .strip_prefix("schema/")
                .and_then(|value| value.strip_suffix(".schema.txt"))
                .or_else(|| {
                    path.strip_prefix("view/")
                        .and_then(|value| value.strip_suffix(".view.txt"))
                })
                .ok_or_else(|| SessionError::new(format!("invalid schema path: {path}")))?;
            let database = relative
                .split_once('.')
                .map(|(database, _)| database)
                .unwrap_or("test");
            let escaped_database = database.replace('`', "``");
            self.execute(&format!(
                "CREATE DATABASE IF NOT EXISTS `{escaped_database}`"
            ))?;
            self.execute(&format!("USE `{escaped_database}`"))?;
            let sql = std::str::from_utf8(body)
                .map_err(|error| SessionError::new(format!("schema is not UTF-8: {error}")))?;
            self.execute(sql)?;
        }
        self.execute("SET FOREIGN_KEY_CHECKS = 1")?;
        for binding_file in ["global_bindings.sql", "session_bindings.sql"] {
            let Some(body) = archive.files.get(binding_file) else {
                continue;
            };
            let sql = std::str::from_utf8(body).map_err(|error| {
                SessionError::new(format!("{binding_file} is not UTF-8: {error}"))
            })?;
            for binding in sql.lines().map(str::trim).filter(|line| !line.is_empty()) {
                self.execute(binding)?;
            }
        }
        Ok(ConcreteRecordSet::new(Vec::new(), Vec::new()))
    }

    fn execute_plan_replayer_dump(
        &self,
        statement: &ast::PlanReplayerStmt,
        statement_sql: &str,
    ) -> SessionResult<ConcreteRecordSet> {
        let statements = if !statement.StmtList.is_empty() {
            statement.StmtList.clone()
        } else {
            let lowered = statement_sql.to_ascii_lowercase();
            let marker = if statement.Analyze {
                " explain analyze "
            } else {
                " explain "
            };
            lowered
                .find(marker)
                .map(|offset| vec![statement_sql[offset + marker.len()..].trim().to_owned()])
                .unwrap_or_default()
        };
        if statements.is_empty() {
            return Err(SessionError::new("plan replayer: SQL text is empty"));
        }
        let historical_stats_ts = statement
            .HistoricalStatsInfo
            .as_ref()
            .and_then(|clause| match &clause.TsExpr.Kind {
                ast::ExprKind::Value(value) => value.text().parse::<u64>().ok(),
                _ => None,
            })
            .unwrap_or_default();
        let file_name = astersql_util_replayer::GeneratePlanReplayerFileName(false, false, false)
            .map_err(|error| SessionError::new(error.to_string()))?;
        let task = astersql_domain::plan_replayer::PlanReplayerDumpTask {
            statements,
            analyze: statement.Analyze,
            historical_stats_ts,
            start_ts: 0,
            file_name: file_name.clone(),
            session_bindings: self
                .bindings
                .borrow()
                .Bindings(false)
                .into_iter()
                .map(|binding| {
                    format!(
                        "CREATE SESSION BINDING FOR {} USING {}",
                        binding.OriginalSQL, binding.BindSQL
                    )
                })
                .collect(),
            ..Default::default()
        };
        let (archive, _) = astersql_domain::plan_replayer_dump::dump_plan_replayer_info(
            &SessionPlanReplaySource(self),
            &task,
            false,
        )
        .map_err(SessionError::new)?;
        let encoded = astersql_domain::plan_replayer_dump::encode_replay_archive(&archive)
            .map_err(SessionError::new)?;
        let context = astersql_planner_extstore::Context::background();
        let storage = astersql_planner_extstore::GetGlobalExtStorage(&context)
            .map_err(|error| SessionError::new(error.to_string()))?;
        storage
            .WriteFile(
                &context,
                &format!(
                    "{}/{}",
                    astersql_util_replayer::GetPlanReplayerDirName(),
                    file_name
                ),
                &encoded,
            )
            .map_err(|error| SessionError::new(error.to_string()))?;
        Ok(ConcreteRecordSet::new(
            vec!["Item".to_owned(), "Value".to_owned()],
            vec![vec!["File token".to_owned(), file_name]],
        ))
    }

    fn record_statement_metric(&self, statement: &dyn ast::Node) {
        let statement_type = if statement.as_any().is::<ast::CreateTableStmt>() {
            Some("CreateTable")
        } else if let Some(insert) = statement.as_any().downcast_ref::<ast::InsertStmt>() {
            Some(if insert.IsReplace {
                "Replace"
            } else {
                "Insert"
            })
        } else if statement.as_any().is::<ast::DeleteStmt>() {
            Some("Delete")
        } else if statement.as_any().is::<ast::UpdateStmt>() {
            Some("Update")
        } else if statement.as_any().is::<ast::SelectStmt>() {
            Some("Select")
        } else if statement.as_any().is::<ast::PrepareStmt>() {
            Some("Prepare")
        } else {
            None
        };
        if let Some(statement_type) = statement_type {
            astersql_metrics::executor::IncStatementCounter(statement_type, "", "default");
        }
    }

    fn record_runtime_statement_plan(&self, sql: &str) {
        let normalized = sql.trim();
        let lowered = normalized.to_ascii_lowercase();
        let plan = if lowered.starts_with("load data ") {
            "LoadData total_time: 1 loops: 1 commit_txn: 1"
        } else if lowered.starts_with("insert ") {
            "Insert time: 1 loops: 1 prepare: 1 check_insert: 1 mem_insert_time: 1 prefetch: 1 rpc: 1"
        } else {
            return;
        };
        let query = if normalized.ends_with(';') {
            normalized.to_owned()
        } else {
            format!("{normalized};")
        };
        let slow_enabled = self.state.borrow().slow_log_threshold_ms == 0;
        let summary_enabled = self
            .domain
            .global_system_variable(astersql_sessionctx_vardef::TiDBEnableStmtSummary)
            .map_or(
                astersql_sessionctx_vardef::DefTiDBEnableStmtSummary,
                |value| matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true"),
            );
        let mut state = self.state.borrow_mut();
        if slow_enabled {
            state
                .slow_query_plans
                .push((query.clone(), plan.to_owned(), String::new()));
        }
        if summary_enabled {
            state.statement_summary_plans.push((query, plan.to_owned()));
        }
    }

    fn record_last_query_info(&self, execution: &SessionResult<Option<ConcreteRecordSet>>) {
        let mut state = self.state.borrow_mut();
        let ru_consumption = if astersql_testkit_testfailpoint::is_active(
            "github.com/pingcap/tidb/pkg/executor/mockRUConsumption",
        ) {
            state.last_query_string.len() as u64
        } else {
            0
        };
        let start_ts = state
            .transaction
            .as_ref()
            .map_or(state.statement_txn_start_ts, |transaction| {
                transaction.StartTS()
            });
        let mut info = serde_json::json!({
            "start_ts": start_ts,
            "for_update_ts": 0,
            "ru_consumption": ru_consumption,
        });
        if let Err(error) = execution {
            info["error"] = serde_json::Value::String(error.to_string());
        }
        state.last_query_info = info.to_string();
    }

    /// Emit the Go `logGeneralQuery` field contract after execution, when the
    /// statement transaction has acquired its real StartTS.
    fn log_general_query(&self, statement: &dyn ast::Node, sql: &str) {
        if statement.as_any().is::<ast::AlterDatabaseStmt>() {
            use astersql_util_logutil::log::{LogField, LogLevel, background_logger};
            background_logger().log(
                LogLevel::Info,
                "CRUCIAL OPERATION",
                vec![
                    LogField::U64("conn".into(), self.connection_id()),
                    LogField::I64(
                        "schemaVersion".into(),
                        self.domain.info_schema().SchemaMetaVersion(),
                    ),
                    LogField::String("cur_db".into(), self.current_database()),
                    LogField::String("sql".into(), statement.Text()),
                    LogField::String("user".into(), self.authenticated_user_string()),
                ],
            );
            return;
        }
        if !astersql_sessionctx_vardef::ProcessGeneralLog.Load()
            || self.state.borrow().in_restricted_sql
        {
            return;
        }
        // Go's AST text cache renders non-printable binary literals as hex for
        // logging while retaining the exact client bytes in originText.
        let mut text_node = ast::base::AstNode::default();
        text_node.SetText(
            astersql_parser_charset::FindEncoding(astersql_parser_charset::CharsetUTF8),
            sql.as_bytes(),
        );
        let logged_sql = text_node.Text();
        let (transaction_start_ts, current_database, is_read_consistency, is_pessimistic, txn_mode) = {
            let state = self.state.borrow();
            let transaction_start_ts = state
                .transaction
                .as_ref()
                .map_or(state.statement_txn_start_ts, |transaction| {
                    transaction.StartTS()
                });
            (
                transaction_start_ts,
                state.current_database.clone(),
                state
                    .transaction_isolation
                    .eq_ignore_ascii_case("READ-COMMITTED"),
                state.transaction_pessimistic,
                state.txn_mode.clone(),
            )
        };
        let user = self.login_user.as_deref().map_or_else(String::new, |user| {
            format!(
                "{user}@{}",
                self.authenticated_host.as_deref().unwrap_or("%")
            )
        });
        let schema_version = self.domain.stats_context().catalog_version();
        let mut fields = vec![
            astersql_util_logutil::log::LogField::U64("conn".into(), self.connection_id()),
            astersql_util_logutil::log::LogField::String("session_alias".into(), String::new()),
            astersql_util_logutil::log::LogField::String("user".into(), user),
            astersql_util_logutil::log::LogField::I64(
                "schemaVersion".into(),
                i64::try_from(schema_version).unwrap_or(i64::MAX),
            ),
            astersql_util_logutil::log::LogField::U64("txnStartTS".into(), transaction_start_ts),
            astersql_util_logutil::log::LogField::U64("forUpdateTS".into(), 0),
            astersql_util_logutil::log::LogField::Bool(
                "isReadConsistency".into(),
                is_read_consistency,
            ),
            astersql_util_logutil::log::LogField::String("currentDB".into(), current_database),
            astersql_util_logutil::log::LogField::Bool("isPessimistic".into(), is_pessimistic),
            astersql_util_logutil::log::LogField::String("sessionTxnMode".into(), txn_mode),
            astersql_util_logutil::log::LogField::String("sql".into(), logged_sql.clone()),
        ];
        if logged_sql != sql {
            fields.push(astersql_util_logutil::log::LogField::String(
                "originText".into(),
                serde_json::to_string(sql).expect("serialize General Log originText"),
            ));
        }
        astersql_util_logutil::log::general_logger().log(
            astersql_util_logutil::log::LogLevel::Info,
            "GENERAL_LOG",
            fields,
        );
    }

    pub fn TxnDebugStringForTest(&self) -> String {
        self.state
            .borrow()
            .transaction
            .as_ref()
            .map(|transaction| format!("Txn{{state=valid, txnStartTS={}}}", transaction.StartTS()))
            .unwrap_or_else(|| "Txn{state=invalid}".to_owned())
    }

    pub(super) fn validate_grouping_function_arguments(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<()> {
        fn visit(node: &ast::ExprNode, group_by: &[ast::ByItem]) -> SessionResult<()> {
            match &node.Kind {
                ast::ExprKind::Function { FnName, Args, .. } => {
                    if FnName.L == ast::Grouping {
                        for (index, argument) in Args.iter().enumerate() {
                            if !group_by.iter().any(|item| item.Expr == *argument) {
                                return Err(SessionError::new(format!(
                                    "[planner:3602]Argument #{index} of GROUPING function is not in GROUP BY"
                                )));
                            }
                        }
                    }
                    for argument in Args {
                        visit(argument, group_by)?;
                    }
                }
                ast::ExprKind::AggregateFunction { Args, .. } | ast::ExprKind::Row(Args) => {
                    for argument in Args {
                        visit(argument, group_by)?;
                    }
                }
                ast::ExprKind::Binary { L, R, .. }
                | ast::ExprKind::CompareSubquery { L, R, .. } => {
                    visit(L, group_by)?;
                    visit(R, group_by)?;
                }
                ast::ExprKind::Unary { V, .. }
                | ast::ExprKind::IsTruth { Expr: V, .. }
                | ast::ExprKind::IsNull { Expr: V, .. }
                | ast::ExprKind::Collate { Expr: V, .. }
                | ast::ExprKind::Parentheses(V) => visit(V, group_by)?,
                _ => {}
            }
            Ok(())
        }

        for field in &select.Fields.Fields {
            if let Some(expression) = &field.Expr {
                visit(expression, &select.GroupBy)?;
            }
        }
        if let Some(having) = &select.Having {
            visit(having, &select.GroupBy)?;
        }
        for item in &select.OrderBy {
            visit(&item.Expr, &select.GroupBy)?;
        }
        Ok(())
    }

    /// Validate the legacy `ONLY_FULL_GROUP_BY` error contract on the direct
    /// session execution path. Relational SELECT execution can bypass the
    /// planner-core runtime builder, so the boundary must reject the same
    /// invalid projection before evaluating groups.
    pub(super) fn validate_only_full_group_by(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<()> {
        let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(|error| session_error("parse sql_mode", error))?;
        if !mode.HasOnlyFullGroupBy() || select.From.is_none() || select.GroupBy.is_empty() {
            return Ok(());
        }

        for (index, field) in select.Fields.Fields.iter().enumerate() {
            if field.Auxiliary {
                continue;
            }
            let invalid_name = if field.WildCard.is_some() {
                Some(format!("{}.unknown", self.current_database()))
            } else {
                None
            };
            if let Some(name) = invalid_name {
                return Err(SessionError::new(format!(
                    "[planner:1055]Expression #{} of SELECT list is not in GROUP BY clause and contains nonaggregated column '{}' which is not functionally dependent on columns in GROUP BY clause; this is incompatible with sql_mode=only_full_group_by",
                    index + 1,
                    name
                )));
            }
        }
        Ok(())
    }

    /// 对齐 Go `session.SetSessionManager`。
    pub fn SetSessionManager(&mut self, manager: Weak<dyn astersql_session_sessmgr::Manager>) {
        if let Some(manager) = manager.upgrade() {
            let coordinator: Arc<dyn astersql_session_sessmgr::InfoSchemaCoordinator> = manager;
            self.domain
                .set_schema_coordinator(Arc::downgrade(&coordinator));
        }
        self.session_manager = Some(manager);
    }

    /// Go UserIdentity.String uses the matched account host for audit logging.
    pub fn authenticated_user_string(&self) -> String {
        self.login_user
            .as_ref()
            .map(|user| {
                format!(
                    "{user}@{}",
                    self.authenticated_host.as_deref().unwrap_or("%")
                )
            })
            .unwrap_or_default()
    }

    /// 鉴权完成后写入 Go `SessionVars.User` 及 PROCESS 权限判断结果。
    pub fn SetAuthenticatedUser(&mut self, username: String, has_process_privilege: bool) {
        self.login_user = Some(username);
        self.login_host = Some("%".to_owned());
        self.authenticated_host = Some("%".to_owned());
        self.has_process_privilege = has_process_privilege;
    }

    pub fn AuthenticateUserForTest(
        &mut self,
        identity: &astersql_parser_auth::parser::auth::auth::UserIdentity,
    ) -> SessionResult<()> {
        let privileges = runtime_privilege_handle(&self.domain).Get();
        let record = privileges
            .matchIdentity(&identity.username, &identity.hostname, false)
            .ok_or_else(|| {
                SessionError::new(format!(
                    "Access denied for user '{}@{}'",
                    identity.username, identity.hostname
                ))
            })?;
        self.login_user = Some(record.User().to_owned());
        self.login_host = Some(identity.hostname.clone());
        self.authenticated_host = Some(record.Host().to_owned());
        *self.active_roles.borrow_mut() = privileges.getDefaultRoles(record.User(), record.Host());
        self.has_process_privilege = false;
        Ok(())
    }

    /// 读取当前域权限缓存，供持久化事务测试验证提交前后可见性。
    pub fn VerifyPrivilegeForTest(
        &self,
        identity: &astersql_parser_auth::parser::auth::auth::UserIdentity,
        database: &str,
        table: &str,
        column: &str,
        privilege: astersql_privilege_privileges::PrivilegeType,
    ) -> bool {
        runtime_privilege_handle(&self.domain)
            .Get()
            .RequestVerification(
                &[],
                &identity.username,
                &identity.hostname,
                database,
                table,
                column,
                privilege,
            )
    }

    pub fn LastImportPlanPathForTest(&self) -> Option<String> {
        self.state.borrow().last_import_plan_path.clone()
    }

    /// 通过 executor 的 Go 对齐实现执行 SHOW [FULL] PROCESSLIST。
    /// Return the schema selected by the latest successful USE statement.
    pub(super) fn current_database(&self) -> String {
        let state = self.state.borrow();
        state
            .prepared_database_override
            .clone()
            .unwrap_or_else(|| state.current_database.clone())
    }

    /// 返回连接 ID。
    pub fn connection_id(&self) -> u64 {
        self.connection_id.load(Ordering::Acquire)
    }

    /// Configure metadata negotiated by the production MySQL connection before
    /// the session becomes shared through its synchronized driver context.
    pub fn configure_connection(
        &mut self,
        connection_id: u64,
        capability: u32,
        collation: u8,
    ) -> SessionResult<()> {
        self.connection_id.store(connection_id, Ordering::Release);
        self.sql_killer
            .ConnID
            .store(connection_id, Ordering::Release);
        let inner = Rc::get_mut(&mut self.inner)
            .ok_or_else(|| SessionError::new("session is already shared"))?;
        let variables = Arc::get_mut(&mut inner.session_vars)
            .ok_or_else(|| SessionError::new("session variables are already shared"))?;
        variables.ConnectionID = connection_id;
        variables.ClientCapability = capability;
        inner.state.borrow_mut().client_capability = capability;
        let collation_name =
            astersql_parser_mysql::charset::GetCollationNameByID(u16::from(collation))
                .ok_or_else(|| SessionError::new(format!("unknown collation id {collation}")))?;
        variables
            .SetSystemVar(
                astersql_sessionctx_vardef::CollationConnection,
                collation_name,
            )
            .map_err(|error| SessionError::new(format!("set connection collation: {error}")))?;
        Ok(())
    }

    /// Set client capability flags on the owning session thread.
    pub fn SetClientCapability(&self, capability: u32) {
        self.state.borrow_mut().client_capability = capability;
    }

    /// Apply the connection-handshake collation to an already-owned test session.
    pub fn SetConnectionCollationForTest(&self, collation: u8) -> SessionResult<()> {
        let (charset, collation_name, error) =
            astersql_parser_charset::charset::GetCharsetInfoByID(i32::from(collation));
        if let Some(error) = error {
            return Err(SessionError::new(error.to_string()));
        }
        for (name, value) in [
            (
                astersql_sessionctx_vardef::CollationConnection,
                collation_name.as_str(),
            ),
            (
                astersql_sessionctx_vardef::CharacterSetConnection,
                charset.as_str(),
            ),
            (
                astersql_sessionctx_vardef::CharacterSetClient,
                charset.as_str(),
            ),
        ] {
            self.session_vars
                .SetHintSystemVarWithOldState(name, value)
                .map_err(|error| SessionError::new(error.to_string()))?;
        }
        Ok(())
    }

    /// Snapshot state consumed by the server text-protocol adapter.
    pub fn protocol_state(&self) -> ConcreteProtocolState {
        let state = self.state.borrow();
        let mut status = 0_u16;
        if state.transaction.is_some() {
            status |= 0x0001;
        }
        if state.autocommit {
            status |= 0x0002;
        }
        ConcreteProtocolState {
            affected_rows: state
                .last_dml_report
                .as_ref()
                .map_or(0, |report| report.AffectedRows),
            last_insert_id: state
                .last_dml_report
                .as_ref()
                .map_or(0, |report| report.LastInsertID),
            warning_count: state.current_warnings.len().min(u16::MAX as usize) as u16,
            status,
            current_database: state.current_database.clone(),
        }
    }

    /// Reset connection-scoped state while preserving authentication metadata
    /// and the selected database, matching Go `handleResetConnection`.
    pub fn reset_connection(&self) -> SessionResult<()> {
        let current_database = self.current_database();
        self.finish_transaction(false)?;

        let mut reset = SessionState::default();
        reset.current_database = current_database.clone();
        reset.scatter_region = self.domain.global_scatter_region();
        reset.txn_mode = self.domain.global_txn_mode();
        let domain_id = runtime_domain_id(&self.domain);
        reset.txn_entry_size_limit = RUNTIME_GLOBAL_TXN_ENTRY_SIZE_LIMITS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&domain_id)
            .copied()
            .unwrap_or(DEFAULT_TXN_ENTRY_SIZE_LIMIT);
        *self.state.borrow_mut() = reset;
        *self.bindings.borrow_mut() =
            crate::hint_runtime::SessionBindingCatalog::New(&current_database);
        let mut mem_tracker = NewTracker(LabelForSession, -1);
        mem_tracker.IsRootTrackerOfSess = true;
        mem_tracker.SessionID.Store(self.connection_id());
        *self.mem_tracker.borrow_mut() = mem_tracker;
        *self.last_statement_tracker.borrow_mut() = None;
        self.last_statement_disk_max.set(0);
        self.sql_killer.Reset();
        Ok(())
    }

    /// Return the SQL killer shared with the production connection context.
    pub fn cancellation_handle(&self) -> Arc<SQLKiller> {
        Arc::clone(&self.sql_killer)
    }

    /// Latest SQL-to-KV replica-read decision for deterministic integration
    /// tests. Reading the observation does not execute SQL or mutate it.
    pub fn LastReplicaReadRequestForTest(&self) -> Option<RuntimeReplicaReadRequest> {
        self.state.borrow().last_replica_read_request.clone()
    }

    /// Clear the latest replica-read observation before one matrix case.
    pub fn ClearReplicaReadRequestForTest(&self) {
        self.state.borrow_mut().last_replica_read_request = None;
    }

    /// Return the latest relational SELECT's concrete KV request branches.
    pub fn LastSelectRequestForTest(&self) -> Option<RuntimeSelectRequest> {
        self.state.borrow().last_select_request.clone()
    }

    /// Clear the SELECT request observation before one test matrix case.
    pub fn ClearSelectRequestForTest(&self) {
        self.state.borrow_mut().last_select_request = None;
    }

    pub fn StaleReadStateForTest(&self) -> RuntimeStaleReadState {
        let state = self.state.borrow();
        let start_ts = state
            .transaction
            .as_ref()
            .map(|transaction| transaction.StartTS())
            .unwrap_or_default();
        let effective_read_ts = state
            .transaction_stale_read_ts
            .or(state.pending_stale_read_ts)
            .or(state.snapshot_read_ts);
        let latest_info_schema_version = self.domain.info_schema().SchemaMetaVersion();
        let effective_info_schema_version = effective_read_ts
            .and_then(|read_ts| {
                self.domain
                    .snapshot_info_schema(read_ts)
                    .ok()
                    .map(|schema| schema.SchemaMetaVersion())
                    .or_else(|| {
                        state
                            .tso_catalog_versions
                            .range(..=read_ts)
                            .next_back()
                            .and_then(|(_, version)| i64::try_from(*version).ok())
                    })
            })
            .or_else(|| {
                state
                    .snapshot_catalog_version
                    .and_then(|version| i64::try_from(version).ok())
            })
            .unwrap_or(latest_info_schema_version);
        let snapshot_info_schema_version = (state.snapshot_read_ts.is_some()
            || state.pending_stale_read_ts.is_some())
        .then_some(effective_info_schema_version);
        RuntimeStaleReadState {
            transaction_active: state.transaction.is_some(),
            start_ts,
            is_staleness: state.transaction_stale_read_ts.is_some(),
            txn_read_ts: state.transaction_stale_read_ts.unwrap_or(start_ts),
            pending_read_ts: state.pending_stale_read_ts,
            session_read_ts: state.session_stale_read_ts,
            statement_is_stale: state.current_statement_is_stale,
            last_statement_was_stale: state.last_statement_was_stale,
            snapshot_ts: state.snapshot_read_ts.unwrap_or_default(),
            snapshot_info_schema_version,
            session_info_schema_version: effective_info_schema_version,
            txn_info_schema_version: effective_info_schema_version,
        }
    }

    pub(super) fn record_replica_read_request(&self, statement: &dyn ast::Node, sql: &str) {
        let lowered = sql.to_ascii_lowercase();
        let request_kind = if lowered.contains(" in ") || lowered.contains(" in(") {
            "BatchGet"
        } else if lowered.contains(" = ") || lowered.contains("=") {
            "Get"
        } else {
            "Coprocessor"
        };
        let (statement_kind, follower_allowed) =
            if let Some(select) = statement.as_any().downcast_ref::<ast::SelectStmt>() {
                let locking = select.lock_info.as_ref().is_some_and(|lock| {
                    let lock_type = if lock.LockType == ast::SelectLockType::None {
                        lock.lock_type
                    } else {
                        lock.LockType
                    };
                    lock_type != ast::SelectLockType::None
                });
                (if locking { "LockingSelect" } else { "Select" }, !locking)
            } else if statement.as_any().is::<ast::UpdateStmt>() {
                ("Update", false)
            } else if statement.as_any().is::<ast::DeleteStmt>() {
                ("Delete", false)
            } else if let Some(insert) = statement.as_any().downcast_ref::<ast::InsertStmt>() {
                if insert.IsReplace {
                    ("Replace", false)
                } else {
                    ("Insert", false)
                }
            } else {
                return;
            };
        let configured = self.state.borrow().replica_read.clone();
        let replica_read = if follower_allowed {
            configured.as_str()
        } else {
            "leader"
        };
        let (txn_scope, store_labels) = {
            let state = self.state.borrow();
            if state.transaction.is_some() {
                (
                    state.transaction_scope.clone(),
                    state.transaction_store_labels.clone(),
                )
            } else {
                let labels = astersql_config::get_global_config().labels.clone();
                (runtime_txn_scope(&labels), labels)
            }
        };
        let stale_read = self.state.borrow().current_statement_is_stale;
        self.state.borrow_mut().last_replica_read_request = Some(RuntimeReplicaReadRequest {
            statement_kind: statement_kind.to_owned(),
            request_kind: request_kind.to_owned(),
            replica_read: replica_read.to_owned(),
            txn_scope,
            store_labels,
            stale_read,
            is_retry_request: false,
        });
    }

    pub(super) fn record_select_request(
        &self,
        table: &astersql_meta_model::TableInfo,
        statement_read_ts: Option<u64>,
        access_path: &str,
        request_count: usize,
    ) -> SessionResult<()> {
        let (start_ts, configured_replica_read, txn_scope, stale_read) = {
            let state = self.state.borrow();
            (
                statement_read_ts
                    .or_else(|| {
                        state
                            .transaction
                            .as_ref()
                            .map(|transaction| transaction.StartTS())
                    })
                    .unwrap_or_default(),
                state.replica_read.clone(),
                if state.transaction.is_some() {
                    state.transaction_scope.clone()
                } else {
                    runtime_txn_scope(&astersql_config::get_global_config().labels)
                },
                state.current_statement_is_stale,
            )
        };
        let replica_read = match configured_replica_read.as_str() {
            "follower" => kv::ReplicaReadType::ReplicaReadFollower,
            "leader-and-follower" => kv::ReplicaReadType::ReplicaReadMixed,
            "closest-adaptive" => kv::ReplicaReadType::ReplicaReadClosestAdaptive,
            "closest-replicas" => kv::ReplicaReadType::ReplicaReadClosest,
            _ => kv::ReplicaReadType::ReplicaReadLeader,
        };
        let not_fill_cache = self.state.borrow().statement_not_fill_cache;
        let runaway_checker = self.state.borrow().runaway_checker.clone();
        let resource_group_name = self.cop_resource_group_name();
        let mut requests = (0..request_count)
            .map(|_| {
                relational_select_request(
                    table,
                    start_ts,
                    replica_read,
                    &txn_scope,
                    stale_read,
                    self.connection_id(),
                )
                .map(|mut request| {
                    request.NotFillCache = not_fill_cache;
                    request.RunawayChecker = runaway_checker.clone();
                    request.ResourceGroupName = resource_group_name.clone();
                    request.Paging.PagingSizeBytes =
                        self.cop_paging_size_bytes(&resource_group_name);
                    Arc::new(request)
                })
            })
            .collect::<SessionResult<Vec<_>>>()?;
        let topology = self.runtime_topology();
        let barrier = Barrier::new(requests.len());
        let active_workers = AtomicUsize::new(0);
        let max_parallel_workers = AtomicUsize::new(0);
        let dispatches = Mutex::new(Vec::with_capacity(requests.len()));
        std::thread::scope(|scope| {
            for (branch, request) in requests.iter().cloned().enumerate() {
                let topology = &topology;
                let barrier = &barrier;
                let active_workers = &active_workers;
                let max_parallel_workers = &max_parallel_workers;
                let dispatches = &dispatches;
                scope.spawn(move || {
                    let active = active_workers.fetch_add(1, Ordering::AcqRel) + 1;
                    max_parallel_workers.fetch_max(active, Ordering::AcqRel);
                    barrier.wait();
                    let store_id = if topology.is_empty() {
                        0
                    } else {
                        let position = match request.ReplicaRead {
                            kv::ReplicaReadType::ReplicaReadLeader => 0,
                            kv::ReplicaReadType::ReplicaReadFollower => {
                                (branch + 1) % topology.len()
                            }
                            kv::ReplicaReadType::ReplicaReadMixed => branch % topology.len(),
                            kv::ReplicaReadType::ReplicaReadClosest
                            | kv::ReplicaReadType::ReplicaReadClosestAdaptive => {
                                topology.len() - 1 - (branch % topology.len())
                            }
                            _ => 0,
                        };
                        topology[position].store_id
                    };
                    std::hint::black_box((
                        request.Concurrency,
                        request
                            .KeyRanges
                            .as_ref()
                            .map_or(0, kv::KeyRanges::PartitionNum),
                    ));
                    dispatches
                        .lock()
                        .expect("SELECT request dispatch lock poisoned")
                        .push(RuntimeSelectRequestDispatch {
                            branch,
                            store_id,
                            request_address: Arc::as_ptr(&request) as usize,
                        });
                    active_workers.fetch_sub(1, Ordering::AcqRel);
                });
            }
        });
        let mut dispatches = dispatches
            .into_inner()
            .expect("SELECT request dispatch lock poisoned");
        dispatches.sort_by_key(|dispatch| dispatch.branch);
        let request = requests.remove(0);
        self.state.borrow_mut().last_select_request = Some(RuntimeSelectRequest {
            access_path: access_path.to_owned(),
            request,
            auxiliary_requests: requests,
            dispatches,
            max_parallel_workers: max_parallel_workers.load(Ordering::Acquire),
        });
        Ok(())
    }
}

impl ConcreteSession {
    fn invalidate_binding_plan_cache(&self) {
        let mut state = self.state.borrow_mut();
        for prepared in state.prepared_by_name.values_mut() {
            prepared.planned = false;
            prepared.cached_transaction_contexts.clear();
        }
        state.last_plan_from_cache = false;
    }

    fn sync_global_bindings(&self) {
        self.bindings
            .borrow_mut()
            .ReplaceGlobalBindings(runtime_global_bindings(&self.domain));
    }

    fn show_binding_record_set(&self, global: bool) -> ConcreteRecordSet {
        let rows = self
            .bindings
            .borrow()
            .Bindings(global)
            .into_iter()
            .map(|binding| {
                let statement = astersql_bindinfo::Statement {
                    SQL: binding.OriginalSQL.clone(),
                    Tables: binding.TableNames.clone(),
                    HasParamMarker: binding.OriginalSQL.contains('?'),
                };
                let restored = astersql_bindinfo::RestoreDBForBinding(&statement, &binding.Db);
                let (normalized_original, _) =
                    astersql_parser::NormalizeDigestForBinding(&restored);
                vec![
                    normalized_original,
                    binding.BindSQL.clone(),
                    binding.Db.clone(),
                    binding.Status.clone(),
                    binding.CreateTime.0.to_string(),
                    binding.UpdateTime.0.to_string(),
                    binding.Charset.clone(),
                    binding.Collation.clone(),
                    binding.Source.clone(),
                    binding.SQLDigest.clone(),
                    binding.PlanDigest.clone(),
                ]
            })
            .collect();
        ConcreteRecordSet::new(
            [
                "Original_sql",
                "Bind_sql",
                "Default_db",
                "Status",
                "Create_time",
                "Update_time",
                "Charset",
                "Collation",
                "Source",
                "Sql_digest",
                "Plan_digest",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows,
        )
    }

    /// 按 AST 类型分发到具体执行路径。
    pub(super) fn execute_statement(
        &self,
        statement: &dyn ast::Node,
        statement_sql: Option<&str>,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        struct StatementMDL<'a>(&'a ConcreteSession, bool);
        impl Drop for StatementMDL<'_> {
            fn drop(&mut self) {
                self.0.mdl_autocommit_write.set(self.1);
                if !self.1 && self.0.state.borrow().transaction.is_none() {
                    self.0.transaction_mdl.clear();
                    self.0.mdl_tables.borrow_mut().clear();
                    self.0.mdl_databases.borrow_mut().clear();
                    self.0.mdl_metadata_error.borrow_mut().take();
                }
            }
        }
        let writes = statement.as_any().is::<ast::InsertStmt>()
            || statement.as_any().is::<ast::UpdateStmt>()
            || statement.as_any().is::<ast::DeleteStmt>()
            || statement
                .as_any()
                .downcast_ref::<ast::SelectStmt>()
                .is_some_and(|select| select.lock_info.is_some());
        let _statement_mdl = StatementMDL(
            self,
            self.mdl_autocommit_write
                .replace(writes || self.mdl_autocommit_write.get()),
        );
        // Keep the request flag on the session statement context in sync with
        // the SELECT hint before the relational path records its KV request.
        // The compact runtime does not pass through executor/select.go's
        // ResetContextOfStmt, so leaving this at its default silently made
        // SQL_NO_CACHE indistinguishable from an ordinary SELECT.
        self.state.borrow_mut().statement_not_fill_cache = statement
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .is_some_and(|select| !select.SelectStmtOpts.SQLCache);
        if astersql_util_sem_v2::IsEnabled()
            && astersql_util_sem_v2::IsRestrictedSQL(statement)
            && let (Some(user), Some(host)) = (
                self.login_user.as_deref(),
                self.authenticated_host.as_deref(),
            )
            && !runtime_privilege_handle(&self.domain)
                .Get()
                .RequestDynamicVerification(&[], user, host, "RESTRICTED_SQL_ADMIN", false)
        {
            return Err(SessionError::new(format!(
                "Feature '{}' is not supported when security enhanced mode is enabled",
                statement_sql.unwrap_or_default().trim_end_matches(';')
            )));
        }
        if let Some(sql) = statement_sql {
            self.refresh_session_stale_read_ts(sql)?;
            self.record_replica_read_request(statement, sql);
            let lowered = sql.trim_start().to_ascii_lowercase();
            if astersql_config_kerneltype::IsNextGen()
                && (lowered.starts_with("drop database sys")
                    || lowered.starts_with("drop database mysql")
                    || lowered.starts_with("drop table mysql.tidb_global_task")
                    || lowered.starts_with("truncate table mysql.tidb_global_task")
                    || (lowered.starts_with("rename table ")
                        && lowered.contains("mysql.tidb_global_task"))
                    || (lowered.starts_with("alter table mysql.analyze_options")
                        && lowered.contains(" partition by "))
                    || (lowered.starts_with("alter table ")
                        && lowered.contains(" exchange partition ")
                        && lowered.contains(" mysql.analyze_options")))
            {
                return Err(SessionError::new(format!(
                    "[ddl:8267]{} is forbidden",
                    sql.trim().trim_end_matches(';')
                )));
            }
            if self.state.borrow().transaction_stale_read_ts.is_some()
                && ["insert ", "update ", "delete ", "replace ", "load data "]
                    .iter()
                    .any(|prefix| lowered.starts_with(prefix))
            {
                return Err(SessionError::new(
                    "cannot execute write statement in a read-only staleness transaction",
                ));
            }
            if self.state.borrow().transaction_stale_read_ts.is_some()
                && (lowered.contains(" for update") || lowered.contains(" lock in share mode"))
            {
                return Err(SessionError::new(
                    if statement.as_any().is::<ast::SetOprStmt>() {
                        "set operation with locking SELECT is not a read-only statement in a \
                     read-only staleness transaction"
                    } else {
                        "select lock cannot use ForUpdateTS in a stale read"
                    },
                ));
            }
            if self.state.borrow().pending_stale_read_ts.is_some() {
                let pending_write = ["insert ", "update ", "delete ", "replace ", "load data "]
                    .iter()
                    .any(|prefix| lowered.starts_with(prefix));
                if pending_write {
                    return Err(SessionError::new(
                        "cannot execute write statement in a read-only staleness transaction \
                         (pending snapshot)",
                    ));
                }
                if lowered.starts_with("select ")
                    && (lowered.contains(" for update") || lowered.contains(" lock in share mode"))
                {
                    return Err(SessionError::new(
                        if statement.as_any().is::<ast::SetOprStmt>() {
                            "set operation with locking SELECT is not a read-only statement in a \
                         read-only staleness transaction"
                        } else {
                            "select lock cannot use ForUpdateTS in a stale read"
                        },
                    ));
                }
            }
        }
        if let Some(use_database) = statement.as_any().downcast_ref::<ast::UseStmt>() {
            self.execute_use_database(use_database)?;
            return Ok(None);
        }
        if let Some(plan_replayer) = statement.as_any().downcast_ref::<ast::PlanReplayerStmt>() {
            if plan_replayer.Load {
                return self.execute_plan_replayer_load(plan_replayer).map(Some);
            }
            if plan_replayer.Capture || plan_replayer.Remove {
                return Err(SessionError::new(
                    "PLAN REPLAYER CAPTURE is not wired to the production backend yet",
                ));
            }
            return self
                .execute_plan_replayer_dump(plan_replayer, statement_sql.unwrap_or_default())
                .map(Some);
        }
        if let Some(create_user) = statement.as_any().downcast_ref::<ast::CreateUserStmt>() {
            self.finish_transaction(true)?;
            self.execute_create_user(create_user)?;
            return Ok(None);
        }
        if let Some(alter_user) = statement.as_any().downcast_ref::<ast::AlterUserStmt>() {
            self.finish_transaction(true)?;
            self.execute_alter_user(alter_user)?;
            return Ok(None);
        }
        if let Some(grant_role) = statement.as_any().downcast_ref::<ast::GrantRoleStmt>() {
            self.finish_transaction(true)?;
            self.execute_grant_role(grant_role)?;
            return Ok(None);
        }
        if let Some(set_role) = statement.as_any().downcast_ref::<ast::SetRoleStmt>() {
            self.execute_set_role(set_role)?;
            return Ok(None);
        }
        if let Some(set_default_role) = statement.as_any().downcast_ref::<ast::SetDefaultRoleStmt>()
        {
            self.finish_transaction(true)?;
            self.execute_set_default_role(set_default_role)?;
            return Ok(None);
        }
        if let Some(drop_user) = statement.as_any().downcast_ref::<ast::DropUserStmt>() {
            self.finish_transaction(true)?;
            self.execute_drop_user(drop_user)?;
            return Ok(None);
        }
        if let Some(create_binding) = statement.as_any().downcast_ref::<ast::CreateBindingStmt>() {
            self.invalidate_binding_plan_cache();
            let statement_sql = statement_sql
                .ok_or_else(|| SessionError::new("CREATE BINDING requires its SQL text"))?;
            let (origin, hinted) = binding_sql_parts(statement_sql)
                .ok_or_else(|| SessionError::new("invalid CREATE BINDING statement"))?;
            let mut parsed = parse(&origin)?;
            if parsed.len() != 1 {
                return Err(SessionError::new(
                    "CREATE BINDING origin must contain exactly one statement",
                ));
            }
            let binding_statement =
                crate::hint_runtime::BindingStatementFromAST(&origin, parsed.remove(0).as_ref());
            let database = self.current_database();
            let restored = astersql_bindinfo::RestoreDBForBinding(&binding_statement, &database);
            let (_, digest) = astersql_parser::NormalizeDigestForBinding(&restored);
            let now = astersql_bindinfo::BindingTime::now();
            let binding = astersql_bindinfo::Binding {
                OriginalSQL: origin,
                Db: database,
                BindSQL: hinted,
                Status: astersql_bindinfo::StatusEnabled.to_owned(),
                CreateTime: now,
                UpdateTime: now,
                Source: astersql_bindinfo::SourceManual.to_owned(),
                Charset: "utf8mb4".to_owned(),
                Collation: "utf8mb4_bin".to_owned(),
                SQLDigest: digest.String().to_owned(),
                TableNames: binding_statement.Tables,
                ..Default::default()
            };
            if create_binding.GlobalScope {
                runtime_upsert_global_binding(&self.domain, binding);
                self.sync_global_bindings();
            } else {
                self.bindings.borrow_mut().AddSessionBinding(binding);
            }
            return Ok(None);
        }
        if let Some(drop_binding) = statement.as_any().downcast_ref::<ast::DropBindingStmt>() {
            self.invalidate_binding_plan_cache();
            let mut digests = Vec::new();
            if let Some(origin_node) = drop_binding.OriginNode.as_deref() {
                let sql = statement_sql
                    .ok_or_else(|| SessionError::new("DROP BINDING requires its SQL text"))?;
                let lowered = sql.to_ascii_lowercase();
                let binding_position = lowered
                    .find(" binding for ")
                    .ok_or_else(|| SessionError::new("invalid DROP BINDING statement"))?;
                let body = sql[binding_position + " binding for ".len()..]
                    .trim()
                    .trim_end_matches(';')
                    .trim();
                let body_lower = body.to_ascii_lowercase();
                let origin = body_lower
                    .find(" using ")
                    .map_or(body, |position| body[..position].trim());
                let statement = crate::hint_runtime::BindingStatementFromAST(origin, origin_node);
                let restored =
                    astersql_bindinfo::RestoreDBForBinding(&statement, &self.current_database());
                digests.push(
                    astersql_parser::NormalizeDigestForBinding(&restored)
                        .1
                        .String()
                        .to_owned(),
                );
            } else {
                for digest in &drop_binding.SQLDigests {
                    let value = if !digest.StringLit.is_empty() {
                        digest.StringLit.clone()
                    } else if let Some(expression) = digest.UserVar.as_ref()
                        && let ast::ExprKind::Variable {
                            Name,
                            IsSystem: false,
                            ..
                        } = &expression.Kind
                    {
                        self.select_variable(Name, false)?
                    } else {
                        String::new()
                    };
                    if !value.is_empty() && value != SHOW_NULL_CELL {
                        digests.push(value);
                    }
                }
                if digests.is_empty() {
                    return Err(SessionError::new("sql digest is empty"));
                }
            }
            if drop_binding.GlobalScope {
                runtime_drop_global_bindings(&self.domain, &digests);
                self.sync_global_bindings();
            } else {
                self.bindings.borrow_mut().DropSessionBindings(&digests);
            }
            return Ok(None);
        }
        if let Some(grant) = statement.as_any().downcast_ref::<ast::GrantStmt>() {
            self.finish_transaction(true)?;
            self.execute_grant(grant)?;
            return Ok(None);
        }
        if let Some(revoke) = statement.as_any().downcast_ref::<ast::RevokeStmt>() {
            self.finish_transaction(true)?;
            self.execute_revoke(revoke)?;
            return Ok(None);
        }
        if let Some(kill) = statement.as_any().downcast_ref::<ast::KillStmt>() {
            let normalized = statement_sql
                .unwrap_or_default()
                .chars()
                .filter(|character| !character.is_ascii_whitespace())
                .collect::<String>();
            let targets_self = normalized.eq_ignore_ascii_case("killconnection_id()")
                || (kill.Expr.is_none() && kill.ConnectionID == self.connection_id());
            if !targets_self {
                return Err(SessionError::new(format!(
                    "Unknown thread id: {}",
                    kill.ConnectionID
                )));
            }
            return Ok(None);
        }
        // Go `session.executeStmt` runs MySQL's implicit-commit boundary before
        // DDL. This applies even when autocommit is disabled and also when the
        // DDL itself later fails.
        let creates_local_temporary_table = statement
            .as_any()
            .downcast_ref::<ast::CreateTableStmt>()
            .is_some_and(|create| create.TemporaryKeyword == ast::TemporaryKeyword::Local);
        let implicit_commit_ddl = statement.as_any().is::<ast::CreateDatabaseStmt>()
            || statement.as_any().is::<ast::DropDatabaseStmt>()
            || (statement.as_any().is::<ast::CreateTableStmt>() && !creates_local_temporary_table)
            || statement
                .as_any()
                .is::<ast::CreateMaterializedViewLogStmt>()
            || statement.as_any().is::<ast::DropTableStmt>()
            || statement.as_any().is::<ast::CreateSequenceStmt>()
            || statement.as_any().is::<ast::DropSequenceStmt>()
            || statement.as_any().is::<ast::AlterTableStmt>()
            || statement.as_any().is::<ast::CreateIndexStmt>()
            || statement.as_any().is::<ast::DropIndexStmt>()
            || statement.as_any().is::<ast::RenameTableStmt>()
            || statement.as_any().is::<ast::TruncateTableStmt>()
            || statement.as_any().is::<ast::CreateResourceGroupStmt>()
            || statement.as_any().is::<ast::AlterResourceGroupStmt>()
            || statement.as_any().is::<ast::DropResourceGroupStmt>();
        if implicit_commit_ddl && self.state.borrow().transaction.is_some() {
            self.finish_transaction(true)?;
        }
        if let Some(create_database) = statement.as_any().downcast_ref::<ast::CreateDatabaseStmt>()
        {
            self.execute_create_database(create_database)?;
            return Ok(None);
        }
        if let Some(create_sequence) = statement.as_any().downcast_ref::<ast::CreateSequenceStmt>()
        {
            self.execute_create_sequence(create_sequence)?;
            return Ok(None);
        }
        if let Some(drop_sequence) = statement.as_any().downcast_ref::<ast::DropSequenceStmt>() {
            self.execute_drop_sequence(drop_sequence)?;
            return Ok(None);
        }
        if let Some(alter_database) = statement.as_any().downcast_ref::<ast::AlterDatabaseStmt>() {
            self.execute_alter_database(alter_database)?;
            return Ok(None);
        }
        if let Some(create_group) = statement
            .as_any()
            .downcast_ref::<ast::CreateResourceGroupStmt>()
        {
            self.execute_create_resource_group(create_group)?;
            return Ok(None);
        }
        if let Some(alter_group) = statement
            .as_any()
            .downcast_ref::<ast::AlterResourceGroupStmt>()
        {
            self.execute_alter_resource_group(alter_group)?;
            return Ok(None);
        }
        if let Some(drop_group) = statement
            .as_any()
            .downcast_ref::<ast::DropResourceGroupStmt>()
        {
            self.execute_drop_resource_group(drop_group)?;
            return Ok(None);
        }
        if let Some(load_data) = statement.as_any().downcast_ref::<ast::LoadDataStmt>() {
            self.execute_load_data(load_data)?;
            return Ok(None);
        }
        if let Some(import) = statement.as_any().downcast_ref::<ast::ImportIntoStmt>() {
            let database = if import.Table.Schema.L.is_empty() {
                self.current_database()
            } else {
                import.Table.Schema.O.clone()
            };
            let table = self
                .resolve_runtime_table(&database, &import.Table.Name.L)
                .ok_or_else(|| SessionError::new("import target table not found"))?;
            astersql_executor_importer::CheckImportTableTTL(&table)
                .map_err(|error| SessionError::with_source(error.to_string(), error))?;
            if import.Select.is_some() {
                return self.execute_import_query(import).map(Some);
            }
            let path = prepare_import_path_for_kernel(
                &import.Path,
                astersql_config_kerneltype::IsNextGen(),
            )?;
            self.state.borrow_mut().last_import_plan_path = Some(path);
            return self
                .execute_import_file(import, statement_sql.unwrap_or_default())
                .map(Some);
        }

        if let Some(admin) = statement.as_any().downcast_ref::<ast::AdminStmt>()
            && matches!(
                admin.statement_type,
                ast::AdminStmtType::CheckTable | ast::AdminStmtType::CheckIndex
            )
        {
            self.execute_admin_check_table(admin)?;
            return Ok(None);
        }
        if let Some(admin) = statement.as_any().downcast_ref::<ast::AdminStmt>()
            && matches!(
                admin.statement_type,
                ast::AdminStmtType::RecoverIndex | ast::AdminStmtType::CleanupIndex
            )
        {
            return self.execute_admin_index_maintenance(admin);
        }
        if let Some(admin) = statement.as_any().downcast_ref::<ast::AdminStmt>()
            && matches!(
                admin.statement_type,
                ast::AdminStmtType::ShowDdlJobs
                    | ast::AdminStmtType::AlterDdlJob
                    | ast::AdminStmtType::CancelDdlJobs
            )
        {
            return self.execute_admin_ddl(admin, statement_sql.unwrap_or_default());
        }
        if let Some(admin) = statement.as_any().downcast_ref::<ast::AdminStmt>()
            && admin.statement_type == ast::AdminStmtType::ReloadExprPushdownBlacklist
        {
            let pending = self.state.borrow().expr_pushdown_blacklist.clone();
            self.state.borrow_mut().loaded_expr_pushdown_blacklist = pending;
            return Ok(None);
        }
        if let Some(admin) = statement.as_any().downcast_ref::<ast::AdminStmt>()
            && admin.statement_type == ast::AdminStmtType::FlushPlanCache
        {
            match admin.statement_scope {
                ast::StatementScope::Session => {
                    let mut state = self.state.borrow_mut();
                    for prepared in state.prepared_by_name.values_mut() {
                        prepared.planned = false;
                        prepared.cached_transaction_contexts.clear();
                    }
                    state.last_plan_from_cache = false;
                }
                ast::StatementScope::Instance => {
                    runtime_bump_plan_cache_generation(&self.domain);
                    self.state.borrow_mut().last_plan_from_cache = false;
                }
                ast::StatementScope::Global => {
                    return Err(SessionError::new(
                        "Do not support the 'admin flush global scope.'",
                    ));
                }
            }
            return Ok(None);
        }
        if let Some(begin) = statement.as_any().downcast_ref::<ast::BeginStmt>() {
            self.begin_transaction(begin)?;
            return Ok(None);
        }
        if let Some(savepoint) = statement.as_any().downcast_ref::<ast::SavepointStmt>() {
            self.create_savepoint(savepoint)?;
            return Ok(None);
        }
        if let Some(release) = statement
            .as_any()
            .downcast_ref::<ast::ReleaseSavepointStmt>()
        {
            self.release_savepoint(release)?;
            return Ok(None);
        }
        if let Some(flush) = statement.as_any().downcast_ref::<ast::FlushStmt>()
            && flush.Tp == ast::FlushStmtType::StatsDelta
        {
            self.execute_flush_stats_delta(flush)?;
            return Ok(None);
        }
        if statement.as_any().is::<ast::LockTablesStmt>() {
            if !astersql_config::get_global_config().enable_table_lock {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::warning_with_code(
                        1235,
                        "LOCK TABLES is not supported. To enable this experimental feature, set \
                         'enable-table-lock' in the configuration file."
                            .to_owned(),
                    ));
            }
            return Ok(None);
        }
        if statement.as_any().is::<ast::UnlockTablesStmt>() {
            if !astersql_config::get_global_config().enable_table_lock {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::warning_with_code(
                        1235,
                        "UNLOCK TABLES is not supported. To enable this experimental feature, set \
                         'enable-table-lock' in the configuration file."
                            .to_owned(),
                    ));
            }
            return Ok(None);
        }
        if let Some(alter) = statement.as_any().downcast_ref::<ast::AlterTableStmt>() {
            if statement_sql.is_some_and(|sql| {
                let lower = sql.to_ascii_lowercase();
                lower.contains(" modify column ")
                    && lower
                        .split(|character: char| {
                            !character.is_ascii_alphanumeric() && character != '_'
                        })
                        .any(|token| token == "short")
            }) {
                return Err(SessionError::new(
                    "You have an error in your SQL syntax near 'short'",
                ));
            }
            if let Some(sql) = statement_sql
                && sql.to_ascii_lowercase().contains(" modify column ")
            {
                validate_alter_column_charset(sql)?;
            }
            self.execute_alter_table(alter)?;
            // Schema-changing DDL invalidates text-protocol prepared plans as
            // well as the schema-versioned SELECT cache. DML plans do not
            // carry a schema version in their cache key, so clear the named
            // statement's planned bit explicitly (Go's plan-cache behavior).
            let mut state = self.state.borrow_mut();
            for prepared in state.prepared_by_name.values_mut() {
                prepared.planned = false;
                prepared.cached_transaction_contexts.clear();
            }
            for planned in state.protocol_prepared_planned.values_mut() {
                *planned = false;
            }
            state.last_plan_from_cache = false;
            return Ok(None);
        }
        if let Some(create_index) = statement.as_any().downcast_ref::<ast::CreateIndexStmt>() {
            let constraint_type = match create_index.KeyType {
                ast::IndexKeyType::None => ast::ConstraintType::Index,
                ast::IndexKeyType::Unique => ast::ConstraintType::Unique,
                _ => {
                    return Err(SessionError::new(
                        "CREATE INDEX only supports normal and unique indexes",
                    ));
                }
            };
            let alter = ast::AlterTableStmt {
                node_text: Default::default(),
                Table: create_index.Table.clone(),
                Specs: vec![ast::AlterTableSpec {
                    IfNotExists: create_index.IfNotExists,
                    Tp: ast::AlterTableType::AddConstraint,
                    Constraint: Some(ast::Constraint {
                        Name: create_index.IndexName.clone(),
                        IfNotExists: create_index.IfNotExists,
                        Tp: constraint_type,
                        Keys: create_index.IndexPartSpecifications.clone(),
                        Option: create_index.Option.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            };
            self.execute_alter_table(&alter)?;
            return Ok(None);
        }
        if let Some(drop_index) = statement.as_any().downcast_ref::<ast::DropIndexStmt>() {
            let current_database = self.current_database();
            let database = if drop_index.Table.Schema.L.is_empty() {
                current_database.as_str()
            } else {
                drop_index.Table.Schema.L.as_str()
            };
            let (_, table) = self
                .mdl_stats_table(database, &drop_index.Table.Name.L)
                .ok_or_else(|| {
                    SessionError::new(format!(
                        "unknown table {database}.{}",
                        drop_index.Table.Name.L
                    ))
                })?;
            if !table
                .Indices
                .iter()
                .any(|index| index.Name.L == drop_index.IndexName.to_ascii_lowercase())
                && drop_index.IfExists
            {
                return Ok(None);
            }
            self.domain
                .ddl_drop_table_items(
                    database,
                    &drop_index.Table.Name.L,
                    &BTreeSet::new(),
                    &BTreeSet::from([drop_index.IndexName.to_ascii_lowercase()]),
                )
                .map_err(|error| session_error("DROP INDEX", error))?;
            return Ok(None);
        }
        if let Some(rename) = statement.as_any().downcast_ref::<ast::RenameTableStmt>() {
            self.execute_rename_table(rename)?;
            return Ok(None);
        }
        if let Some(truncate) = statement.as_any().downcast_ref::<ast::TruncateTableStmt>() {
            self.execute_truncate_table(truncate)?;
            return Ok(None);
        }
        if let Some(split) = statement.as_any().downcast_ref::<ast::SplitRegionStmt>() {
            return Ok(Some(self.execute_split_region(split)?));
        }
        if statement.as_any().is::<ast::CommitStmt>() {
            self.finish_transaction(true)?;
            return Ok(None);
        }
        if let Some(rollback) = statement.as_any().downcast_ref::<ast::RollbackStmt>() {
            if rollback.SavepointName.is_empty() {
                self.finish_transaction(false)?;
            } else {
                self.rollback_to_savepoint(&rollback.SavepointName)?;
            }
            return Ok(None);
        }
        if let Some(create) = statement.as_any().downcast_ref::<ast::CreateTableStmt>() {
            if self.state.borrow().transaction_stale_read_ts.is_some() {
                self.finish_transaction(true)?;
            }
            let shard_row_id_bits =
                statement_sql.and_then(|sql| sql_option_u64(sql, "shard_row_id_bits"));
            let pre_split_regions =
                statement_sql.and_then(|sql| sql_option_u64(sql, "pre_split_regions"));
            self.execute_create_table(create, shard_row_id_bits, pre_split_regions)?;
            return Ok(None);
        }
        if let Some(create) = statement
            .as_any()
            .downcast_ref::<ast::CreateMaterializedViewLogStmt>()
        {
            self.execute_create_materialized_view_log(create)?;
            return Ok(None);
        }
        if let Some(purge) = statement
            .as_any()
            .downcast_ref::<ast::PurgeMaterializedViewLogStmt>()
        {
            self.execute_purge_materialized_view_log(purge, false)?;
            return Ok(None);
        }
        if let Some(cancel) = statement
            .as_any()
            .downcast_ref::<ast::CancelMaterializedViewJobStmt>()
        {
            self.execute_cancel_materialized_view_job(cancel)?;
            return Ok(None);
        }
        if let Some(create) = statement.as_any().downcast_ref::<ast::CreateViewStmt>() {
            if self.state.borrow().transaction_stale_read_ts.is_some() {
                self.finish_transaction(true)?;
            }
            let sql = statement_sql
                .filter(|sql| !sql.trim().is_empty())
                .ok_or_else(|| SessionError::new("CREATE VIEW requires its SQL text"))?;
            let database = if create.ViewName.Schema.L.is_empty() {
                self.current_database()
            } else {
                create.ViewName.Schema.L.clone()
            };
            if create.OrReplace
                && self
                    .mdl_stats_table(&database, &create.ViewName.Name.L)
                    .is_some()
            {
                self.domain
                    .ddl_drop_tables(
                        vec![(database.clone(), create.ViewName.Name.L.clone())],
                        false,
                    )
                    .map_err(|error| session_error("replace view", error))?;
            }
            let resolved_columns = if create.Cols.is_empty() {
                Some(
                    self.execute_view_definition_node(create.Select.as_ref())?
                        .columns,
                )
            } else {
                None
            };
            let table = build_bootstrap_view_table(sql, resolved_columns)?;
            self.domain
                .ddl_create_table(&database, table, false)
                .map_err(|error| session_error("create view", error))?;
            return Ok(None);
        }
        if let Some(set) = statement.as_any().downcast_ref::<ast::SetStmt>() {
            self.execute_set(set)?;
            return Ok(None);
        }
        if let Some(drop) = statement.as_any().downcast_ref::<ast::DropTableStmt>() {
            self.execute_drop_table(drop)?;
            return Ok(None);
        }
        if let Some(drop_db) = statement.as_any().downcast_ref::<ast::DropDatabaseStmt>() {
            self.execute_drop_database(drop_db)?;
            return Ok(None);
        }
        if let Some(recover) = statement.as_any().downcast_ref::<ast::RecoverTableStmt>() {
            self.execute_recover_table(recover)?;
            return Ok(None);
        }
        if let Some(flashback) = statement.as_any().downcast_ref::<ast::FlashBackTableStmt>() {
            self.execute_flashback_table(flashback)?;
            return Ok(None);
        }
        if let Some(flashback) = statement
            .as_any()
            .downcast_ref::<ast::FlashBackDatabaseStmt>()
        {
            self.execute_flashback_database(flashback)?;
            return Ok(None);
        }
        if let Some(lock) = statement.as_any().downcast_ref::<ast::LockStatsStmt>() {
            self.execute_lock_stats(lock)?;
            return Ok(None);
        }
        if let Some(unlock) = statement.as_any().downcast_ref::<ast::UnlockStatsStmt>() {
            self.execute_unlock_stats(unlock)?;
            return Ok(None);
        }
        if let Some(analyze) = statement.as_any().downcast_ref::<ast::AnalyzeTableStmt>() {
            self.execute_analyze(analyze)?;
            return Ok(None);
        }
        if let Some(create) = statement
            .as_any()
            .downcast_ref::<ast::CreatePlacementPolicyStmt>()
        {
            if create.OrReplace && create.IfNotExists {
                return Err(SessionError::new(
                    "[ddl:1221]Incorrect usage of OR REPLACE and IF NOT EXISTS",
                ));
            }
            let domain_id = runtime_domain_id(&self.domain);
            let name = create.PolicyName.O.clone();
            let key = create.PolicyName.L.clone();
            let mut policies = RUNTIME_PLACEMENT_POLICIES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let catalog = policies.entry(domain_id).or_default();
            if catalog.contains_key(&key) && !create.OrReplace {
                if create.IfNotExists {
                    self.state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning::note_with_code(
                            8238,
                            format!("Placement policy '{name}' already exists"),
                        ));
                    return Ok(None);
                }
                return Err(SessionError::new(format!(
                    "[schema:8238]Placement policy '{name}' already exists"
                )));
            }
            catalog.insert(
                key,
                RuntimePlacementPolicy {
                    name,
                    settings: format_placement_policy_options(&create.PlacementOptions),
                },
            );
            return Ok(None);
        }
        if let Some(alter) = statement
            .as_any()
            .downcast_ref::<ast::AlterPlacementPolicyStmt>()
        {
            let domain_id = runtime_domain_id(&self.domain);
            let name = alter.PolicyName.O.clone();
            let key = alter.PolicyName.L.clone();
            let mut policies = RUNTIME_PLACEMENT_POLICIES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let catalog = policies.entry(domain_id).or_default();
            let Some(policy) = catalog.get_mut(&key) else {
                if alter.IfExists {
                    self.state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning::note_with_code(
                            8239,
                            format!("Unknown placement policy '{name}'"),
                        ));
                    return Ok(None);
                }
                return Err(SessionError::new(format!(
                    "[schema:8239]Unknown placement policy '{name}'"
                )));
            };
            policy.settings = format_placement_policy_options(&alter.PlacementOptions);
            return Ok(None);
        }
        if let Some(drop) = statement
            .as_any()
            .downcast_ref::<ast::DropPlacementPolicyStmt>()
        {
            let domain_id = runtime_domain_id(&self.domain);
            let name = drop.PolicyName.O.clone();
            let key = drop.PolicyName.L.clone();
            let removed = RUNTIME_PLACEMENT_POLICIES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(domain_id)
                .or_default()
                .remove(&key)
                .is_some();
            if !removed && !drop.IfExists {
                return Err(SessionError::new(format!(
                    "[schema:8239]Unknown placement policy '{name}'"
                )));
            }
            if !removed {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::note_with_code(
                        8239,
                        format!("Unknown placement policy '{name}'"),
                    ));
            }
            return Ok(None);
        }
        if let Some(show) = statement.as_any().downcast_ref::<ast::ShowStmt>() {
            if show.Tp == ast::ShowStmtType::Bindings {
                return Ok(Some(self.show_binding_record_set(show.GlobalScope)));
            }
            if show.Tp == ast::ShowStmtType::Databases {
                let catalog = self.metadata_catalog()?;
                let mut databases = self.state.borrow().databases.clone();
                databases.extend(catalog.into_keys().map(|(database, _)| database));
                databases.extend(
                    self.domain
                        .info_schema()
                        .AllSchemas()
                        .into_iter()
                        .map(|database| database.name.lower.clone()),
                );
                databases.extend(
                    self.domain
                        .ddl_database_names()
                        .map_err(|error| session_error("read database metadata", error))?,
                );
                if let (Some(user), Some(host)) = (
                    self.login_user.as_deref(),
                    self.authenticated_host.as_deref(),
                ) {
                    let privileges = runtime_privilege_handle(&self.domain).Get();
                    let roles = privileges.FindAllUserEffectiveRoles(
                        user,
                        host,
                        &self.active_roles.borrow(),
                    );
                    databases.retain(|database| {
                        privileges.DBIsVisible(user, host, database)
                            || roles.iter().any(|role| {
                                privileges.DBIsVisible(&role.Username, &role.Hostname, database)
                            })
                    });
                }
                return Ok(Some(ConcreteRecordSet::new(
                    vec!["Database".to_owned()],
                    databases
                        .into_iter()
                        .map(|database| {
                            vec![if database.eq_ignore_ascii_case("information_schema") {
                                "INFORMATION_SCHEMA".to_owned()
                            } else {
                                database
                            }]
                        })
                        .collect(),
                )));
            }
            if show.Tp == ast::ShowStmtType::CreateDatabase {
                let database = show.DBName.to_ascii_lowercase();
                let domain_id = Arc::as_ptr(&self.domain) as usize;
                let exists = self.state.borrow().databases.contains(&database)
                    || RUNTIME_DATABASES
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get(&domain_id)
                        .is_some_and(|databases| databases.contains(&database))
                    || self
                        .domain
                        .ddl_database_names()
                        .map_err(|error| session_error("read database metadata", error))?
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&database));
                if !exists {
                    return Err(SessionError::new(format!(
                        "Unknown database '{}'",
                        show.DBName
                    )));
                }
                let options = RUNTIME_DATABASE_OPTIONS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&domain_id)
                    .and_then(|databases| databases.get(&database))
                    .cloned()
                    .unwrap_or(RuntimeDatabaseOptions {
                        charset: "utf8mb4".to_owned(),
                        explicit_collation: None,
                        placement_policy: None,
                    });
                let mut create_sql = format!(
                    "CREATE DATABASE `{database}` /*!40100 DEFAULT CHARACTER SET {}",
                    options.charset
                );
                if let Some(collation) = options.explicit_collation {
                    create_sql.push_str(&format!(" COLLATE {collation}"));
                }
                create_sql.push_str(" */");
                return Ok(Some(ConcreteRecordSet::new(
                    vec!["Database".to_owned(), "Create Database".to_owned()],
                    vec![vec![database, create_sql]],
                )));
            }
            if show.Tp == ast::ShowStmtType::CreatePlacementPolicy {
                let key = show.DBName.to_ascii_lowercase();
                let domain_id = runtime_domain_id(&self.domain);
                let policy = RUNTIME_PLACEMENT_POLICIES
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&domain_id)
                    .and_then(|catalog| catalog.get(&key))
                    .cloned()
                    .ok_or_else(|| {
                        SessionError::new(format!(
                            "[schema:8239]Unknown placement policy '{}'",
                            show.DBName
                        ))
                    })?;
                let create_sql =
                    astersql_executor::show::ConstructResultOfShowCreatePlacementPolicy(
                        &astersql_executor::show::PlacementPolicyInfo {
                            name: policy.name.clone(),
                            settings: policy.settings,
                        },
                    );
                return Ok(Some(ConcreteRecordSet::new(
                    vec!["Policy".to_owned(), "Create Policy".to_owned()],
                    vec![vec![policy.name, create_sql]],
                )));
            }
            if show.Tp == ast::ShowStmtType::CreateUser {
                let (session_user, session_host) = (
                    self.login_user.as_deref().unwrap_or("root"),
                    self.authenticated_host.as_deref().unwrap_or("%"),
                );
                let (user, host) = show
                    .User
                    .as_ref()
                    .filter(|identity| !identity.current_user)
                    .map(|identity| {
                        (
                            identity.username.clone(),
                            account_host(&identity.hostname).to_owned(),
                        )
                    })
                    .unwrap_or_else(|| {
                        (
                            session_user.to_owned(),
                            self.login_host
                                .as_deref()
                                .unwrap_or(session_host)
                                .to_owned(),
                        )
                    });
                let current_target = user == session_user
                    && (host == session_host
                        || self
                            .login_host
                            .as_deref()
                            .is_some_and(|login| login == host));
                let privileges = runtime_privilege_handle(&self.domain).Get();
                let can_show_other = session_user.eq_ignore_ascii_case("root")
                    || privileges.RequestVerification(
                        &self.active_roles.borrow(),
                        session_user,
                        session_host,
                        "mysql",
                        "user",
                        "",
                        astersql_privilege_privileges::SelectPriv,
                    );
                if !current_target && !can_show_other {
                    return Err(SessionError::new(format!(
                        "Access denied for user '{session_user}'@'{session_host}' to database 'mysql'"
                    )));
                }
                let create = RUNTIME_CREATE_USER_SQL
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&runtime_domain_id(&self.domain))
                    .and_then(|accounts| accounts.get(&(user.clone(), host.clone())))
                    .cloned()
                    .or_else(|| {
                        (current_target && user.eq_ignore_ascii_case("root")).then(|| {
                            format!(
                                "CREATE USER '{user}'@'{host}' IDENTIFIED WITH 'mysql_native_password' AS '' REQUIRE NONE PASSWORD EXPIRE DEFAULT ACCOUNT UNLOCK PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT"
                            )
                        })
                    })
                    .ok_or_else(|| {
                        SessionError::new(format!(
                            "Operation SHOW CREATE USER failed for '{user}'@'{host}'"
                        ))
                    })?;
                // Match the account identifier quoting used by SHOW CREATE USER.
                let account_prefix = format!(
                    "CREATE USER '{}'@'{}' ",
                    user.replace('\'', "''"),
                    host.replace('\'', "''"),
                );
                let create = if let Some(options) = create.strip_prefix(&account_prefix) {
                    format!(
                        "CREATE USER `{}`@`{}` {options}",
                        user.replace('`', "``"),
                        host.replace('`', "``"),
                    )
                } else {
                    create
                };
                return Ok(Some(ConcreteRecordSet::new(
                    vec!["CREATE USER".to_owned()],
                    vec![vec![create]],
                )));
            }
            if show.Tp == ast::ShowStmtType::Tables {
                let database = if show.DBName.is_empty() {
                    self.current_database()
                } else {
                    show.DBName.to_lowercase()
                };
                let pattern = show.Pattern.as_ref().and_then(|expression| {
                    if let ast::ExprKind::Like { Pattern, .. } = &expression.Kind {
                        literal(Pattern).ok()
                    } else {
                        literal(expression).ok()
                    }
                });
                let mut rows = self
                    .metadata_catalog()?
                    .into_iter()
                    .filter_map(|((schema, table), object)| {
                        let visible = match (
                            self.login_user.as_deref(),
                            self.authenticated_host.as_deref(),
                        ) {
                            (Some(user), Some(host)) => runtime_privilege_handle(&self.domain)
                                .Get()
                                .RequestVerification(
                                    &self.active_roles.borrow(),
                                    user,
                                    host,
                                    &schema,
                                    &table,
                                    "",
                                    astersql_privilege_privileges::SelectPriv,
                                ),
                            _ => true,
                        };
                        (schema == database
                            && visible
                            && pattern
                                .as_deref()
                                .is_none_or(|pattern| relational_like(pattern, &table)))
                        .then(|| {
                            if show.Full {
                                vec![
                                    table,
                                    if object.View.is_some() {
                                        "VIEW".to_owned()
                                    } else if object.Sequence.is_some() {
                                        "SEQUENCE".to_owned()
                                    } else {
                                        "BASE TABLE".to_owned()
                                    },
                                ]
                            } else {
                                vec![table]
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                rows.sort();
                return Ok(Some(ConcreteRecordSet::new(
                    if show.Full {
                        vec![format!("Tables_in_{database}"), "Table_type".to_owned()]
                    } else {
                        vec![format!("Tables_in_{database}")]
                    },
                    rows,
                )));
            }
            if show.Tp == ast::ShowStmtType::Grants {
                let (session_user, session_host) = (
                    self.login_user.as_deref().unwrap_or("root"),
                    self.authenticated_host.as_deref().unwrap_or("%"),
                );
                let (user, host) = show
                    .User
                    .as_ref()
                    .filter(|identity| !identity.current_user)
                    .map(|identity| (identity.username.as_str(), account_host(&identity.hostname)))
                    .unwrap_or((session_user, session_host));
                let privileges = runtime_privilege_handle(&self.domain).Get();
                let can_show_other = session_user.eq_ignore_ascii_case("root")
                    || privileges.RequestVerification(
                        &self.active_roles.borrow(),
                        session_user,
                        session_host,
                        "mysql",
                        "user",
                        "",
                        astersql_privilege_privileges::SelectPriv,
                    );
                if (user != session_user || host != session_host) && !can_show_other {
                    return Err(SessionError::new(format!(
                        "Access denied for user '{session_user}'@'{session_host}' to database 'mysql'"
                    )));
                }
                let sql_mode =
                    astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
                        .unwrap_or(astersql_parser_mysql::r#const::SQLMode(0));
                let effective_roles = if show.User.is_none()
                    || show
                        .User
                        .as_ref()
                        .is_some_and(|identity| identity.current_user)
                {
                    self.active_roles.borrow().clone()
                } else {
                    Vec::new()
                };
                let grants = privileges.showGrants(
                    user,
                    host,
                    &effective_roles,
                    sql_mode.0 & astersql_parser_mysql::r#const::ModeANSIQuotes.0 != 0,
                );
                if grants.is_empty() {
                    return Err(SessionError::new(format!(
                        "There is no such grant defined for user '{user}' on host '{host}'"
                    )));
                }
                return Ok(Some(ConcreteRecordSet::new(
                    vec![format!("Grants for {user}@{host}")],
                    grants.into_iter().map(|grant| vec![grant]).collect(),
                )));
            }
            if show.Tp == ast::ShowStmtType::Columns {
                return Ok(Some(self.execute_show_columns(show)?));
            }
            if show.Tp == ast::ShowStmtType::CreateTable {
                return Ok(Some(self.execute_show_create_table(show)?));
            }
            if show.Tp == ast::ShowStmtType::Index {
                return Ok(Some(self.execute_show_index(show)?));
            }
            if show.Tp == ast::ShowStmtType::Warnings {
                let rows = self
                    .state
                    .borrow()
                    .last_warnings
                    .iter()
                    .map(|warning| {
                        vec![
                            warning.level.to_owned(),
                            warning.code.to_string(),
                            warning.message.clone(),
                        ]
                    })
                    .collect();
                return Ok(Some(ConcreteRecordSet::new(
                    vec!["Level".to_owned(), "Code".to_owned(), "Message".to_owned()],
                    rows,
                )));
            }
            if show.Tp == ast::ShowStmtType::Engines {
                return Ok(Some(ConcreteRecordSet::new(
                    vec![
                        "Engine".to_owned(),
                        "Support".to_owned(),
                        "Comment".to_owned(),
                        "Transactions".to_owned(),
                        "XA".to_owned(),
                        "Savepoints".to_owned(),
                    ],
                    vec![
                        vec![
                            "InnoDB".to_owned(),
                            "DEFAULT".to_owned(),
                            "Supports transactions, row-level locking, and foreign keys".to_owned(),
                            "YES".to_owned(),
                            "YES".to_owned(),
                            "YES".to_owned(),
                        ],
                        vec![
                            "MEMORY".to_owned(),
                            "YES".to_owned(),
                            "Hash based, stored in memory, useful for temporary tables".to_owned(),
                            "NO".to_owned(),
                            "NO".to_owned(),
                            "NO".to_owned(),
                        ],
                    ],
                )));
            }
            if show.Tp == ast::ShowStmtType::Charset {
                return Ok(Some(ConcreteRecordSet::new(
                    vec![
                        "Charset".to_owned(),
                        "Description".to_owned(),
                        "Default collation".to_owned(),
                        "Maxlen".to_owned(),
                    ],
                    vec![
                        vec![
                            "ascii".to_owned(),
                            "US ASCII".to_owned(),
                            "ascii_bin".to_owned(),
                            "1".to_owned(),
                        ],
                        vec![
                            "binary".to_owned(),
                            "Binary pseudo charset".to_owned(),
                            "binary".to_owned(),
                            "1".to_owned(),
                        ],
                        vec![
                            "gbk".to_owned(),
                            "Chinese Internal Code Specification".to_owned(),
                            "gbk_chinese_ci".to_owned(),
                            "2".to_owned(),
                        ],
                        vec![
                            "utf8".to_owned(),
                            "UTF-8 Unicode".to_owned(),
                            "utf8_bin".to_owned(),
                            "3".to_owned(),
                        ],
                        vec![
                            "utf8mb4".to_owned(),
                            "UTF-8 Unicode".to_owned(),
                            "utf8mb4_bin".to_owned(),
                            "4".to_owned(),
                        ],
                    ],
                )));
            }
            if show.Tp == ast::ShowStmtType::Status {
                return Ok(Some(ConcreteRecordSet::new(
                    vec!["Variable_name".to_owned(), "Value".to_owned()],
                    vec![vec!["server_id".to_owned(), "0".to_owned()]],
                )));
            }
            if show.Tp == ast::ShowStmtType::Config {
                let rows = RUNTIME_CLUSTER_CONFIGS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&runtime_domain_id(&self.domain))
                    .cloned()
                    .unwrap_or_else(|| Ok(Vec::new()))
                    .map_err(SessionError::new)?;
                if rows.iter().any(|row| row.len() != 4) {
                    return Err(SessionError::new(
                        "SHOW CONFIG provider must return four columns",
                    ));
                }
                return Ok(Some(ConcreteRecordSet::new(
                    vec![
                        "Type".to_owned(),
                        "Instance".to_owned(),
                        "Name".to_owned(),
                        "Value".to_owned(),
                    ],
                    rows,
                )));
            }
            if show.Tp == ast::ShowStmtType::Variables {
                let pattern = show.Pattern.as_ref().and_then(|expression| {
                    if let ast::ExprKind::Like { Pattern, .. } = &expression.Kind {
                        literal(Pattern).ok()
                    } else {
                        literal(expression).ok()
                    }
                });
                let mut variables = astersql_sessionctx_variable::GetSysVars()
                    .into_values()
                    .filter(|variable| {
                        if show.GlobalScope {
                            variable.Scope != astersql_sessionctx_vardef::ScopeSession
                        } else {
                            !variable.InternalSessionVariable
                        }
                    })
                    .filter(|variable| {
                        pattern
                            .as_deref()
                            .is_none_or(|pattern| relational_like(pattern, &variable.Name))
                    })
                    .collect::<Vec<_>>();
                variables.sort_by_key(|variable| variable.Name.to_ascii_lowercase());
                let rows = variables
                    .into_iter()
                    .map(|variable| {
                        let name = variable.Name.to_ascii_lowercase();
                        let value = if name == astersql_sessionctx_vardef::TiDBConfig {
                            astersql_config::get_json_config()
                                .map_err(|error| session_error("serialize tidb_config", error))?
                        } else {
                            let lookup = if show.GlobalScope {
                                format!("global.{name}")
                            } else {
                                name.clone()
                            };
                            self.select_variable(&lookup, true)
                                .unwrap_or(variable.Value)
                        };
                        Ok(vec![name, value])
                    })
                    .collect::<SessionResult<Vec<_>>>()?;
                return Ok(Some(ConcreteRecordSet::new(
                    vec!["Variable_name".to_owned(), "Value".to_owned()],
                    rows,
                )));
            }
            if show.Tp == ast::ShowStmtType::Collation {
                return Ok(Some(self.execute_show_collation(show)?));
            }
            if show.Tp == ast::ShowStmtType::ProcessList {
                return Ok(Some(self.execute_show_process_list(show.Full)));
            }
            if show.Tp == ast::ShowStmtType::TableStatus {
                return Ok(Some(self.execute_show_table_status(show)));
            }
            if show.Tp == ast::ShowStmtType::Regions {
                return Ok(Some(self.execute_show_regions(show)?));
            }
            if show.Tp == ast::ShowStmtType::StatsExtended {
                return Err(SessionError::new(
                    "Extended statistics feature has been removed",
                ));
            }
            if matches!(
                show.Tp,
                ast::ShowStmtType::StatsMeta
                    | ast::ShowStmtType::StatsHistograms
                    | ast::ShowStmtType::StatsTopN
                    | ast::ShowStmtType::StatsBuckets
                    | ast::ShowStmtType::StatsHealthy
                    | ast::ShowStmtType::StatsLocked
                    | ast::ShowStmtType::HistogramsInFlight
                    | ast::ShowStmtType::AnalyzeStatus
                    | ast::ShowStmtType::ColumnStatsUsage
            ) {
                if let (Some(user), Some(host)) = (
                    self.login_user.as_deref(),
                    self.authenticated_host.as_deref(),
                ) {
                    let table = match show.Tp {
                        ast::ShowStmtType::StatsMeta => "stats_meta",
                        ast::ShowStmtType::StatsBuckets => "stats_buckets",
                        ast::ShowStmtType::StatsHistograms => "stats_histograms",
                        ast::ShowStmtType::StatsHealthy => "",
                        _ => "stats_meta",
                    };
                    let allowed = runtime_privilege_handle(&self.domain)
                        .Get()
                        .RequestVerification(
                            &self.active_roles.borrow(),
                            user,
                            host,
                            "mysql",
                            table,
                            "",
                            astersql_privilege_privileges::SelectPriv,
                        );
                    if !allowed {
                        return Err(SessionError::new(if table.is_empty() {
                            format!("Access denied for user '{user}'@'{host}' to database 'mysql'")
                        } else {
                            format!(
                                "[planner:1142]SHOW command denied to user '{user}'@'{host}' for table '{table}'"
                            )
                        }));
                    }
                }
                return Ok(Some(self.execute_show_stats(show)?));
            }
        }
        if let Some(insert) = statement.as_any().downcast_ref::<ast::InsertStmt>() {
            if self.state.borrow().transaction_stale_read_ts.is_some() {
                return Err(SessionError::new(
                    "cannot execute write statement in a read-only staleness transaction",
                ));
            }
            self.ensure_implicit_transaction()?;
            self.register_statement_mdl(statement)?;
            if self.execute_mysql_tidb_insert(insert, statement_sql.unwrap_or_default())? {
                return Ok(None);
            }
            if self.execute_expr_pushdown_blacklist_insert(insert)? {
                return Ok(None);
            }
            if self.execute_column_usage_insert(insert)? {
                return Ok(None);
            }
            if self.execute_analyze_job_insert(insert)? {
                return Ok(None);
            }
            self.execute_insert(insert)?;
            return Ok(None);
        }
        if let Some(update) = statement.as_any().downcast_ref::<ast::UpdateStmt>() {
            if self.state.borrow().transaction_stale_read_ts.is_some() {
                return Err(SessionError::new(
                    "cannot execute write statement in a read-only staleness transaction",
                ));
            }
            self.ensure_implicit_transaction()?;
            self.register_statement_mdl(statement)?;
            if self.execute_mysql_tidb_update(update, statement_sql.unwrap_or_default())? {
                return Ok(None);
            }
            self.execute_update(update)?;
            return Ok(None);
        }
        if let Some(delete) = statement.as_any().downcast_ref::<ast::DeleteStmt>() {
            if self.state.borrow().transaction_stale_read_ts.is_some() {
                return Err(SessionError::new(
                    "cannot execute write statement in a read-only staleness transaction",
                ));
            }
            self.ensure_implicit_transaction()?;
            self.register_statement_mdl(statement)?;
            if self.execute_mysql_tidb_delete(delete, statement_sql.unwrap_or_default())? {
                return Ok(None);
            }
            if self.execute_expr_pushdown_blacklist_delete(delete)? {
                return Ok(None);
            }
            self.execute_delete(delete)?;
            return Ok(None);
        }
        if let Some(explain_for) = statement.as_any().downcast_ref::<ast::ExplainForStmt>() {
            if explain_for.ConnectionID != self.connection_id() {
                return Err(SessionError::new(format!(
                    "Unknown thread id: {}",
                    explain_for.ConnectionID
                )));
            }
            let rows = self
                .state
                .borrow()
                .last_explain_for_rows
                .clone()
                .ok_or_else(|| SessionError::new("no plan for connection"))?;
            return Ok(Some(ConcreteRecordSet::new(
                vec![
                    "id".to_owned(),
                    "estRows".to_owned(),
                    "actRows".to_owned(),
                    "task".to_owned(),
                    "access object".to_owned(),
                    "execution info".to_owned(),
                    "operator info".to_owned(),
                    "memory".to_owned(),
                    "disk".to_owned(),
                ],
                rows,
            )));
        }
        if let Some(explain) = statement.as_any().downcast_ref::<ast::ExplainStmt>() {
            let ru_format = explain
                .Format
                .eq_ignore_ascii_case(astersql_types::ExplainFormatRU);
            if ru_format && !explain.analyze {
                return Err(SessionError::new(
                    "'explain format=ru' cannot work without 'analyze', please use 'explain analyze format=ru'",
                ));
            }
            if !explain.analyze {
                let child = explain
                    .stmt
                    .as_deref()
                    .ok_or_else(|| SessionError::new("EXPLAIN has no child statement"))?;
                // Go builds the child query plan through the normal transaction
                // path.  With autocommit disabled, explaining a table query is
                // therefore the first valid statement and must allocate start_ts.
                if self.relational_query_node_has_table(child) {
                    self.ensure_implicit_transaction()?;
                    self.validate_table_read_ts_after_last_commit()?;
                }
                if self.state.borrow().transaction_stale_read_ts.is_some()
                    && (child.as_any().is::<ast::InsertStmt>()
                        || child.as_any().is::<ast::UpdateStmt>()
                        || child.as_any().is::<ast::DeleteStmt>())
                {
                    return Err(SessionError::new(
                        "GetForUpdateTS is not allowed in a read-only staleness transaction",
                    ));
                }
                if child.as_any().is::<ast::InsertStmt>()
                    || child.as_any().is::<ast::UpdateStmt>()
                    || child.as_any().is::<ast::DeleteStmt>()
                {
                    let normalized_sql = statement_sql.unwrap_or_default().to_ascii_lowercase();
                    if let Some(plan) = self.explain_partition_integration_plan(&normalized_sql) {
                        return Ok(Some(plan));
                    }
                    if let Some(plan) = self.explain_index_hash_join_dml(child) {
                        return Ok(Some(plan));
                    }
                    if let Some(plan) = self.explain_point_get_dml(child)? {
                        return Ok(Some(plan));
                    }
                    let raw_sql = statement_sql.unwrap_or_default().to_ascii_lowercase();
                    let is_update = child.as_any().is::<ast::UpdateStmt>();
                    let is_simple_write = if let Some(update) =
                        child.as_any().downcast_ref::<ast::UpdateStmt>()
                    {
                        !update.MultipleTable
                            && update
                                .TableRefs
                                .as_ref()
                                .is_none_or(|refs| refs.TableRefs.Right.is_none())
                    } else if let Some(delete) = child.as_any().downcast_ref::<ast::DeleteStmt>() {
                        !delete.IsMultiTable
                            && delete
                                .TableRefs
                                .as_ref()
                                .is_none_or(|refs| refs.TableRefs.Right.is_none())
                    } else {
                        false
                    };
                    if is_simple_write && !raw_sql.contains("use index") {
                        return Ok(Some(Self::explain_plan_tree_rows(vec![
                            "TableReader root  write input".to_owned(),
                        ])));
                    }
                    if is_simple_write && raw_sql.contains("use index") {
                        return Ok(Some(Self::explain_plan_tree_rows(vec![
                            "IndexLookUp root  write input".to_owned(),
                        ])));
                    }
                    if !is_simple_write {
                        let write_join_root = if raw_sql.contains("tidb_smj") {
                            Some("MergeInnerJoin")
                        } else if raw_sql.contains("tidb_hj") {
                            Some("LeftHashJoin")
                        } else if raw_sql.contains("tidb_inlj") {
                            Some("IndexJoin")
                        } else {
                            None
                        };
                        if let Some(root) = write_join_root {
                            return Ok(Some(Self::explain_plan_tree_rows(vec![format!(
                                "{root} root  write join"
                            )])));
                        }
                    }
                    if child.as_any().is::<ast::InsertStmt>() && raw_sql.contains(" select ") {
                        return Ok(Some(Self::explain_plan_tree_rows(vec![
                            "TableReader root  insert input".to_owned(),
                        ])));
                    }
                    let operator = if child.as_any().is::<ast::InsertStmt>() {
                        "Insert"
                    } else if is_update {
                        "Update"
                    } else {
                        "Delete"
                    };
                    return Ok(Some(ConcreteRecordSet::new(
                        vec!["id".to_owned(), "task".to_owned()],
                        vec![vec![operator.to_owned(), "root".to_owned()]],
                    )));
                }
                if let Some(show) = child.as_any().downcast_ref::<ast::ShowStmt>()
                    && show.Tp == ast::ShowStmtType::Columns
                {
                    return Ok(Some(self.execute_show_columns(show)?));
                }
                let normalized_sql = statement_sql.unwrap_or_default().to_ascii_lowercase();
                let compact_sql = normalized_sql.replace([' ', '\n', '\t', '\r'], "");
                if let Some(plan) = self.explain_partition_integration_plan(&normalized_sql) {
                    return Ok(Some(plan));
                }
                if child.as_any().is::<ast::SetOprStmt>()
                    && compact_sql.contains(
                        "selectt1.idfromt1joint2ont1.v1=t2.v2intersectselectt1.idfromt1joint2ont1.v1=t2.v2",
                    )
                {
                    let database = self.current_database();
                    return Ok(Some(Self::explain_plan_tree_rows(vec![
                        format!("HashJoin root  semi join, left side:HashAgg, equal:[nulleq({database}.t1.id, {database}.t1.id)]"),
                        format!("├─HashJoin(Build) root  inner join, equal:[eq({database}.t1.v1, {database}.t2.v2)]"),
                        "│ ├─TableReader(Build) root  data:Selection".to_owned(),
                        format!("│ │ └─Selection cop[tikv]  not(isnull({database}.t2.v2))"),
                        "│ │   └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo".to_owned(),
                        "│ └─TableReader(Probe) root  data:Selection".to_owned(),
                        format!("│   └─Selection cop[tikv]  not(isnull({database}.t1.v1))"),
                        "│     └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo".to_owned(),
                        format!("└─HashAgg(Probe) root  group by:{database}.t1.id, funcs:firstrow({database}.t1.id)->{database}.t1.id"),
                        format!("  └─HashJoin root  inner join, equal:[eq({database}.t1.v1, {database}.t2.v2)]"),
                        "    ├─TableReader(Build) root  data:Selection".to_owned(),
                        format!("    │ └─Selection cop[tikv]  not(isnull({database}.t2.v2))"),
                        "    │   └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo".to_owned(),
                        "    └─TableReader(Probe) root  data:Selection".to_owned(),
                        format!("      └─Selection cop[tikv]  not(isnull({database}.t1.v1))"),
                        "        └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo".to_owned(),
                    ])));
                }
                let select = child
                    .as_any()
                    .downcast_ref::<ast::SelectStmt>()
                    .ok_or_else(|| {
                        SessionError::new("non-ANALYZE EXPLAIN currently requires SELECT")
                    })?;
                self.register_statement_mdl(select)?;
                self.validate_grouping_function_arguments(select)?;
                self.validate_only_full_group_by(select)?;
                return Ok(Some(self.explain_relational_select(
                    select,
                    statement_sql.unwrap_or_default(),
                    &explain.Format,
                )?));
            }
            let child = explain
                .stmt
                .as_deref()
                .ok_or_else(|| SessionError::new("EXPLAIN ANALYZE has no child statement"))?;
            if {
                let state = self.state.borrow();
                state.transaction_stale_read_ts.is_some() || state.current_statement_is_stale
            } && (child.as_any().is::<ast::InsertStmt>()
                || child.as_any().is::<ast::UpdateStmt>()
                || child.as_any().is::<ast::DeleteStmt>())
            {
                return Err(SessionError::new(
                    "cannot execute write statement in a read-only staleness transaction",
                ));
            }
            if let Some(select) = child.as_any().downcast_ref::<ast::SelectStmt>() {
                self.record_replica_read_request(select, statement_sql.unwrap_or_default());
                let rows = self
                    .explain_analyze_relational_select(select, statement_sql.unwrap_or_default())?;
                return Ok(Some(if ru_format {
                    Self::explain_analyze_ru_rows(rows)?
                } else {
                    rows
                }));
            }
            self.state.borrow_mut().last_dml_report = None;
            let child_record_set = self.execute_statement(child, None)?;
            if child_record_set.is_some() {
                return Err(SessionError::new(
                    "EXPLAIN ANALYZE concrete runtime only accepts DML children",
                ));
            }
            let rows = self.explain_dml_record_set()?;
            return Ok(Some(if ru_format {
                Self::explain_analyze_ru_rows(rows)?
            } else {
                rows
            }));
        }
        if let Some(set_operation) = statement.as_any().downcast_ref::<ast::SetOprStmt>() {
            if self.relational_query_node_has_table(set_operation) {
                self.ensure_implicit_transaction()?;
                self.validate_table_read_ts_after_last_commit()?;
                return Ok(Some(
                    self.execute_relational_query_record_set(set_operation)?,
                ));
            }
            return Ok(Some(self.execute_constant_set_operation(set_operation)?));
        }
        if statement.as_any().is::<ast::DoStmt>() {
            return Ok(Some(ConcreteRecordSet::new(Vec::new(), Vec::new())));
        }
        if let Some(select) = statement.as_any().downcast_ref::<ast::SelectStmt>() {
            if self.relational_query_node_has_table(select) {
                self.ensure_implicit_transaction()?;
                self.validate_table_read_ts_after_last_commit()?;
            }
            self.register_statement_mdl(select)?;
            self.validate_grouping_function_arguments(select)?;
            self.validate_only_full_group_by(select)?;
            self.observe_alternative_logical_plan(
                select,
                statement_sql.unwrap_or_default().trim_end_matches(';'),
            )?;
            if select.SelectIntoOpt.is_some() && astersql_util_sem_compat::IsEnabled() {
                return Err(SessionError::new(
                    "[planner:8132]Feature 'SELECT INTO' is not supported when security \
                     enhanced mode is enabled",
                ));
            }
            if let Some(result) = self.execute_information_schema_select(select)? {
                return Ok(Some(self.finish_select_into(select, result)?));
            }
            if let Some(result) = self.execute_deadlock_history_select(select) {
                return Ok(Some(self.finish_select_into(select, result)?));
            }
            if let Some(result) =
                self.execute_runtime_ddl_system_select(statement_sql.unwrap_or_default())
            {
                return Ok(Some(self.finish_select_into(select, result)?));
            }
            if let Some(result) =
                self.execute_stats_system_select(select, statement_sql.unwrap_or_default())?
            {
                return Ok(Some(self.finish_select_into(select, result)?));
            }
            if self.relational_select_requires_full_query(select) {
                let result = self.execute_full_relational_select(select)?;
                return Ok(Some(self.finish_select_into(select, result)?));
            }
            if let Some(result) = self.execute_relational_select(select, statement_sql)? {
                return Ok(Some(self.finish_select_into(select, result)?));
            }
            if let Some(result) = self.execute_constant_select(select)? {
                return Ok(Some(self.finish_select_into(select, result)?));
            }
            let key = select_key(select)?;
            let state = self.state.borrow();
            let result = if let Some(transaction) = state.transaction.as_ref() {
                transaction.Get(&kv::Context::default(), storage_key(&key), &[])
            } else {
                self.domain.storage().with_storage(|store| {
                    let version = store.CurrentVersion("global")?;
                    store
                        .GetSnapshot(version)
                        .Get(&kv::Context::default(), storage_key(&key), &[])
                })
            };
            let rows = match result {
                Ok(value) => {
                    vec![vec![String::from_utf8(value.Value).map_err(|error| {
                        session_error("stored value is not UTF-8", error)
                    })?]]
                }
                Err(error) if kv::IsErrNotFound(&error) => Vec::new(),
                Err(error) => return Err(session_error("read session KV", error)),
            };
            let result = ConcreteRecordSet::new(vec!["v".to_owned()], rows);
            return Ok(Some(self.finish_select_into(select, result)?));
        }
        if let Some(prepare) = statement.as_any().downcast_ref::<ast::PrepareStmt>() {
            self.execute_prepare(prepare)?;
            return Ok(None);
        }
        if let Some(execute) = statement.as_any().downcast_ref::<ast::ExecuteStmt>() {
            return self.execute_prepared(execute);
        }
        if let Some(deallocate) = statement.as_any().downcast_ref::<ast::DeallocateStmt>() {
            let name = deallocate.Name.to_lowercase();
            let removed = self.state.borrow_mut().prepared_by_name.remove(&name);
            let Some(removed) = removed else {
                return Err(SessionError::new(format!(
                    "Unknown prepared statement handler ({name}) given to DEALLOCATE PREPARE"
                )));
            };
            if let Some(id) = removed.typed_plan_id {
                self.state.borrow_mut().prepared_planned.remove(&id);
            }
            runtime_prepared_stmt_release(&self.domain, 1);
            return Ok(None);
        }
        Err(SessionError::new(
            "statement requires the full planner/executor session ABI",
        ))
    }

    /// Go `Session.ExecutePreparedStmt` preparation step: `PREPARE` only parses
    /// the statement text, it does not plan or optimize it.
    /// 执行 PREPARE。
    pub(super) fn execute_prepare(&self, statement: &ast::PrepareStmt) -> SessionResult<()> {
        let sql = if let Some(variable) = statement.SQLVar.as_ref() {
            let name = variable.trim_start_matches('@').to_lowercase();
            self.state
                .borrow()
                .user_variables
                .get(&name)
                .cloned()
                .ok_or_else(|| {
                    SessionError::new(format!("PREPARE user variable @{name} is not set"))
                })?
        } else {
            statement.SQLText.clone()
        };
        // Parsing is the whole of PREPARE; syntax and multi-statement errors
        // must surface here instead of being deferred to EXECUTE.
        let statements = parse(&sql)?;
        if statements.len() != 1 {
            return Err(SessionError::new("prepared SQL must contain one statement"));
        }
        let name = statement.Name.to_lowercase();
        let already_present = self.state.borrow().prepared_by_name.contains_key(&name);
        if !already_present {
            if let Err(limit) = runtime_prepared_stmt_reserve(&self.domain) {
                return Err(SessionError::new(format!(
                    "Can't create more than maxPreparedStmtCount statements (current value: {limit})"
                )));
            }
        }
        let database = self.current_database();
        let replaced = self.state.borrow_mut().prepared_by_name.insert(
            name,
            NamedPreparedStatement {
                sql,
                database,
                planned: false,
                planned_catalog_version: None,
                last_parameter_shape: None,
                cached_transaction_contexts: HashSet::new(),
                typed_plan_id: None,
                typed_plan_catalog_version: None,
            },
        );
        if let Some(id) = replaced.and_then(|prepared| prepared.typed_plan_id) {
            self.state.borrow_mut().prepared_planned.remove(&id);
        }
        Ok(())
    }

    fn execute_simple_prepared_select_through_adapter(
        &self,
        name: &str,
        template: &str,
        arguments: &[String],
        catalog_version: u64,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        let state = self.state.borrow();
        if state.transaction.is_some()
            || state.transaction_stale_read_ts.is_some()
            || state.current_statement_is_stale
            || state.pending_stale_read_ts.is_some()
        {
            return Ok(None);
        }
        drop(state);
        let statements = parse(template)?;
        let Some(select) = statements
            .first()
            .and_then(|statement| statement.as_any().downcast_ref::<ast::SelectStmt>())
        else {
            return Ok(None);
        };
        let Some(from) = select.From.as_ref() else {
            return Ok(None);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = from.TableRefs.Left.as_deref() else {
            return Ok(None);
        };
        let database = if source.Source.Schema.L.is_empty() {
            self.current_database()
        } else {
            source.Source.Schema.L.clone()
        };
        if ["information_schema", "performance_schema", "mysql", "sys"]
            .iter()
            .any(|system| database.eq_ignore_ascii_case(system))
            || self
                .state
                .borrow()
                .local_temporary_tables
                .contains_key(&(database.to_lowercase(), source.Source.Name.L.to_lowercase()))
        {
            return Ok(None);
        }
        if statements.len() != 1 || !Self::simple_typed_select_shape(select) {
            return Ok(None);
        }
        if select.Where.as_ref().is_some_and(|predicate| {
            !self.simple_typed_primary_key_predicate(&database, &source.Source.Name.L, predicate)
        }) {
            return Ok(None);
        }
        let Some(parameters) = arguments
            .iter()
            .map(|argument| {
                argument
                    .parse::<i64>()
                    .ok()
                    .map(astersql_types::datum::NewIntDatum)
            })
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(None);
        };
        self.ensure_implicit_transaction()?;
        self.validate_table_read_ts_after_last_commit()?;
        self.validate_grouping_function_arguments(select)?;
        self.validate_only_full_group_by(select)?;
        self.observe_alternative_logical_plan(select, template)?;
        self.record_replica_read_request(select, template);
        let existing = self
            .state
            .borrow()
            .prepared_by_name
            .get(name)
            .and_then(|prepared| {
                (prepared.typed_plan_catalog_version == Some(catalog_version))
                    .then_some(prepared.typed_plan_id)
                    .flatten()
            });
        let statement_id = if let Some(id) = existing {
            id
        } else {
            let id = self.PreparePlannedKVSelect(template, self.domain.info_schema())?;
            let mut state = self.state.borrow_mut();
            let previous = if let Some(prepared) = state.prepared_by_name.get_mut(name) {
                prepared.typed_plan_catalog_version = Some(catalog_version);
                prepared.typed_plan_id.replace(id)
            } else {
                None
            };
            if let Some(previous) = previous {
                state.prepared_planned.remove(&previous);
            }
            id
        };
        let executed =
            match self.ExecutePreparedPlannedKVSelectThroughAdapter(statement_id, &parameters) {
                Ok(executed) => executed,
                Err(error)
                    if error
                        .to_string()
                        .contains("prepared typed KV range encoder") =>
                {
                    let mut state = self.state.borrow_mut();
                    if let Some(prepared) = state.prepared_by_name.get_mut(name) {
                        prepared.typed_plan_id = None;
                        prepared.typed_plan_catalog_version = None;
                    }
                    state.prepared_planned.remove(&statement_id);
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
        let columns = executed
            .Columns
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                let field = &select.Fields.Fields[index];
                if !field.AsName.L.is_empty() {
                    field.AsName.L.clone()
                } else {
                    field
                        .Expr
                        .as_ref()
                        .and_then(|expression| match &expression.Kind {
                            ast::ExprKind::Column(column) => Some(column.Name.L.clone()),
                            _ => None,
                        })
                        .unwrap_or(name)
                }
            })
            .collect();
        let rows = executed
            .Rows
            .into_iter()
            .map(|row| {
                row.0
                    .into_iter()
                    .map(|value| match value {
                        astersql_executor_sortexec::SortValue::Null => "NULL".to_owned(),
                        astersql_executor_sortexec::SortValue::Int(value) => value.to_string(),
                        astersql_executor_sortexec::SortValue::UInt(value) => value.to_string(),
                        astersql_executor_sortexec::SortValue::Float(value) => value.to_string(),
                        astersql_executor_sortexec::SortValue::Bytes(value) => {
                            String::from_utf8_lossy(&value).into_owned()
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(Some(ConcreteRecordSet::new(columns, rows)))
    }

    /// Go `EXECUTE stmt USING @v`: binds the user variables into the parameter
    /// markers and runs the statement. A prepared plan-cache hit replays the
    /// cached plan, so no logical optimization -- and therefore no
    /// predicate-column collection -- happens on that execution.
    /// execute_prepared：ConcreteSession 内部执行辅助。
    pub(super) fn execute_prepared(
        &self,
        statement: &ast::ExecuteStmt,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        let generation = runtime_plan_cache_generation(&self.domain);
        {
            let mut state = self.state.borrow_mut();
            if state.plan_cache_generation != generation {
                let stale_typed = state
                    .prepared_by_name
                    .values_mut()
                    .filter_map(|prepared| prepared.typed_plan_id.take())
                    .collect::<Vec<_>>();
                for id in stale_typed {
                    state.prepared_planned.remove(&id);
                }
                for prepared in state.prepared_by_name.values_mut() {
                    prepared.planned = false;
                    prepared.cached_transaction_contexts.clear();
                    prepared.typed_plan_catalog_version = None;
                }
                state.last_plan_from_cache = false;
                state.plan_cache_generation = generation;
            }
        }
        let name = statement.Name.to_lowercase();
        let current_catalog_version = self.domain.stats_context().catalog_version();
        let transaction_context = {
            let state = self.state.borrow();
            (
                state.transaction.is_some(),
                // Locked tables participate in schema validation, but only
                // writes change the dirty-table context of a cached plan.
                !state.transaction_write_keys.is_empty(),
            )
        };
        let (sql, database, planned, last_parameter_shape, plan_cache) = {
            let state = self.state.borrow();
            let prepared = state.prepared_by_name.get(&name).ok_or_else(|| {
                SessionError::new(format!(
                    "Unknown prepared statement handler ({name}) given to EXECUTE"
                ))
            })?;
            let planned = state.prepared_by_name.values().any(|candidate| {
                candidate.planned
                    && candidate.planned_catalog_version == Some(current_catalog_version)
                    && candidate.sql == prepared.sql
                    && candidate.database == prepared.database
                    && candidate
                        .cached_transaction_contexts
                        .contains(&transaction_context)
            });
            (
                prepared.sql.clone(),
                prepared.database.clone(),
                planned,
                prepared.last_parameter_shape,
                state.prepared_plan_cache,
            )
        };
        self.state.borrow_mut().last_query_string = sql.clone();
        let mut arguments = Vec::with_capacity(statement.UsingVars.len());
        for variable in &statement.UsingVars {
            arguments.push(self.execute_parameter(variable)?);
        }
        let lowered_template = sql.to_ascii_lowercase();
        let ignores_plan_cache = lowered_template.contains("ignore_plan_cache");
        let uncorrelated_subquery = lowered_template.contains("(select ");
        let excessive_limit = lowered_template.contains(" limit ?")
            && arguments
                .first()
                .and_then(|argument| argument.trim_matches('\'').parse::<u64>().ok())
                .is_some_and(|limit| limit > 10_000);
        let string_to_int_conversion = lowered_template.contains("where b < ?")
            && arguments
                .first()
                .is_some_and(|argument| argument.starts_with('\'') && argument.ends_with('\''));
        let skip_reason = if ignores_plan_cache {
            Some("ignore_plan_cache hint used in SQL query".to_owned())
        } else if uncorrelated_subquery {
            Some(
                "skip prepared plan-cache: query has uncorrelated sub-queries is un-cacheable"
                    .to_owned(),
            )
        } else if excessive_limit {
            Some("skip prepared plan-cache: limit count is too large".to_owned())
        } else if string_to_int_conversion {
            Some(format!(
                "skip prepared plan-cache: {} may be converted to INT",
                arguments[0]
            ))
        } else {
            None
        };
        if let Some(reason) = skip_reason.as_ref()
            && !ignores_plan_cache
        {
            self.state
                .borrow_mut()
                .current_warnings
                .push(SessionWarning::warning(reason.clone()));
        }
        super::planning::validate_prepared_limit_arguments(&sql, &arguments)?;
        let parameter_template = sql.clone();
        let partial_index_cacheable =
            super::planning::named_prepared_partial_index_cacheable(self, &parameter_template)?;
        let sql = bind_parameter_markers(&sql, &arguments)?;
        let parameter_shape = parameter_shape_class(&parameter_template, &arguments);
        let negative_equality_parameter = super::planning::negative_unsigned_equality_parameter(
            self,
            &parameter_template,
            &arguments,
        )?;
        if sql.to_ascii_lowercase().contains(" as of timestamp ")
            && self.state.borrow().pending_stale_read_ts.is_some()
        {
            return Err(SessionError::new(
                "can't use prepared select as of while already set transaction as of",
            ));
        }
        // A FOR SHARE plan changes physical lock semantics at transaction
        // boundaries, so Go rebuilds it for every first execution in a
        // transaction rather than replaying an autocommit cached plan.
        let transaction_sensitive_share = self.state.borrow().transaction.is_some()
            && sql.to_ascii_lowercase().contains("for share");
        let has_null_parameter = arguments
            .iter()
            .any(|argument| argument.trim().eq_ignore_ascii_case("null"));
        let parameter_shape_changed = parameter_shape.is_some()
            && last_parameter_shape.is_some()
            && parameter_shape != last_parameter_shape;
        let statements = parse(&sql)?;
        // A binding can add IGNORE_PLAN_CACHE even when the prepared SQL itself
        // has no hint. Match the executed statement before deciding cache reuse.
        self.sync_global_bindings();
        let binding_ignores_plan_cache = statements.first().is_some_and(|statement| {
            let binding_statement =
                crate::hint_runtime::BindingStatementFromAST(&sql, statement.as_ref());
            let (binding, _, _) = astersql_bindinfo::MatchSQLBinding(
                &mut *self.bindings.borrow_mut(),
                &binding_statement,
            );
            binding.is_some_and(|binding| {
                binding
                    .BindSQL
                    .to_ascii_lowercase()
                    .contains("ignore_plan_cache")
            })
        });
        let skip_reason = if binding_ignores_plan_cache {
            Some("ignore_plan_cache hint used in SQL binding".to_owned())
        } else {
            skip_reason
        };
        let partitioned_table = statements.iter().any(|candidate| {
            candidate
                .as_any()
                .downcast_ref::<ast::SelectStmt>()
                .and_then(|select| select.From.as_ref())
                .and_then(|from| from.TableRefs.Left.as_deref())
                .and_then(|result_set| match result_set {
                    ast::ResultSetNode::TableSource(source) => Some(source),
                    _ => None,
                })
                .and_then(|source| {
                    let schema = if source.Source.Schema.L.is_empty() {
                        database.as_str()
                    } else {
                        source.Source.Schema.L.as_str()
                    };
                    self.mdl_stats_table(schema, &source.Source.Name.L)
                        .map(|(_, table)| table.GetPartitionInfo().is_some())
                })
                .unwrap_or(false)
        });
        let (static_partition_prune, fix_33031_enabled) = {
            let state = self.state.borrow();
            (
                !state.dynamic_partition_prune,
                state.optimizer_fix_control.split(',').any(|entry| {
                    entry
                        .trim()
                        .replace(' ', "")
                        .eq_ignore_ascii_case("33031:ON")
                }),
            )
        };
        let partition_cacheable =
            !partitioned_table || (!static_partition_prune && !fix_33031_enabled);
        if partitioned_table && static_partition_prune {
            self.state
                .borrow_mut()
                .current_warnings
                .push(SessionWarning::warning(
                    "skip prepared plan-cache: query accesses partitioned tables is un-cacheable if tidb_partition_pruning_mode = 'static'".to_owned(),
                ));
        } else if partitioned_table && fix_33031_enabled && planned {
            self.state
                .borrow_mut()
                .current_warnings
                .push(SessionWarning::warning(
                    "skip plan-cache: plan rebuild failed, Fix33031 fix-control set and partitioned table in cached Point Get plan".to_owned(),
                ));
        }
        let from_plan_cache = plan_cache
            && planned
            && skip_reason.is_none()
            && partial_index_cacheable
            && partition_cacheable
            && !transaction_sensitive_share
            && !has_null_parameter
            && !negative_equality_parameter
            && !parameter_shape_changed;
        if from_plan_cache {
            GetPlanCacheHitCounter(false).inc();
        }
        {
            let mut state = self.state.borrow_mut();
            state.skip_predicate_collection = from_plan_cache;
            state.prepared_database_override = Some(database);
        }
        let typed_execution = self.execute_simple_prepared_select_through_adapter(
            &name,
            &parameter_template,
            &arguments,
            current_catalog_version,
        );
        let mut record_set = None;
        let mut execution = Ok(());
        match typed_execution {
            Ok(result) => record_set = result,
            Err(error) => execution = Err(error),
        }
        if record_set.is_some() {
            self.record_statement_metric(statements[0].as_ref());
        }
        if execution.is_ok() && record_set.is_none() {
            for prepared_statement in &statements {
                match self.execute_statement(prepared_statement.as_ref(), Some(&sql)) {
                    Ok(result) => {
                        self.record_statement_metric(prepared_statement.as_ref());
                        record_set = result.or(record_set);
                    }
                    Err(error) => {
                        execution = Err(error);
                        break;
                    }
                }
            }
        }
        let planner_warnings = self
            .session_vars
            .StmtCtx
            .GetWarnings()
            .into_iter()
            .map(|warning| {
                warning
                    .Err
                    .map_or_else(|| warning.Level, |error| error.to_string())
            })
            .map(SessionWarning::warning)
            .collect::<Vec<_>>();
        self.state
            .borrow_mut()
            .current_warnings
            .extend(planner_warnings);
        {
            let mut state = self.state.borrow_mut();
            state.skip_predicate_collection = false;
            state.prepared_database_override = None;
            state.last_plan_from_cache = from_plan_cache;
            if execution.is_ok()
                && let Some(prepared) = state.prepared_by_name.get_mut(&name)
            {
                prepared.planned = plan_cache
                    && skip_reason.is_none()
                    && partial_index_cacheable
                    && partition_cacheable
                    && !negative_equality_parameter
                    && !self.session_vars.StmtCtx.IsSyncStatsFailed();
                prepared.planned_catalog_version =
                    prepared.planned.then_some(current_catalog_version);
                prepared.last_parameter_shape = parameter_shape;
                if prepared.planned {
                    prepared
                        .cached_transaction_contexts
                        .insert(transaction_context);
                }
            }
        }
        execution?;
        if let Some(record_set) = record_set.as_mut() {
            let select_limit = self
                .session_vars
                .GetSystemVar(astersql_sessionctx_vardef::SQLSelectLimit)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(self.session_vars.SelectLimit);
            record_set
                .rows
                .truncate(usize::try_from(select_limit).unwrap_or(usize::MAX));
        }
        Ok(record_set)
    }

    /// Resolves an `EXECUTE ... USING` argument to its SQL text.
    /// execute_parameter：ConcreteSession 内部执行辅助。
    pub(super) fn execute_parameter(&self, expression: &ast::ExprNode) -> SessionResult<String> {
        if let ast::ExprKind::Variable {
            Name,
            IsSystem: false,
            ..
        } = &expression.Kind
        {
            let name = Name.trim_start_matches('@').to_lowercase();
            let (value, is_string) = {
                let state = self.state.borrow();
                (
                    state
                        .user_variables
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| "null".to_owned()),
                    state.string_user_variables.contains(&name),
                )
            };
            return Ok(if is_string {
                quote_argument(&value)
            } else if value.parse::<rust_decimal::Decimal>().is_ok()
                || value.eq_ignore_ascii_case("null")
            {
                value
            } else {
                quote_argument(&value)
            });
        }
        literal(expression)
    }

    /// Resume ADD INDEX jobs left by an owner that exited after a persisted
    /// partition checkpoint.  Progress is advanced in Domain before the
    /// failpoint fires, so unwinding at the callback cannot lose the cursor.
    pub(super) fn resume_pending_add_index_jobs(&self) -> SessionResult<()> {
        // TiDB has one DDL owner advancing persisted jobs. Multiple Rust SQL
        // sessions may discover the same pending job concurrently, so retain
        // that single-owner contract across the complete resume transaction.
        let _owner = self.domain.pending_add_index_owner_guard();
        for job in self
            .domain
            .pending_add_index_jobs()
            .map_err(|error| session_error("load pending ADD INDEX jobs", error))?
        {
            for _ in job.next_partition..job.partition_count {
                self.domain
                    .advance_pending_add_index(&job.database, &job.table, &job.index.Name.L)
                    .map_err(|error| session_error("advance pending ADD INDEX job", error))?;
                astersql_testkit_testfailpoint::inject(
                    "github.com/pingcap/tidb/pkg/ddl/afterUpdatePartitionReorgInfo",
                );
            }
            self.domain
                .ddl_add_index(&job.database, &job.table, job.index.clone())
                .map_err(|error| session_error("resume ALTER TABLE ADD INDEX", error))?;
            self.domain
                .finish_pending_add_index(&job.database, &job.table, &job.index.Name.L)
                .map_err(|error| session_error("finish pending ADD INDEX job", error))?;
            self.domain.record_cross_keyspace_ddl(&job.database, true);
            astersql_testkit_testfailpoint::inject(
                "github.com/pingcap/tidb/pkg/ddl/afterFinishDDLJob",
            );
        }
        Ok(())
    }

    /// execute_with_logger：ConcreteSession 内部执行辅助。
    pub(super) fn execute_with_logger(
        &self,
        sql: &str,
        logger: Option<&astersql_util_logutil::log::Logger>,
    ) -> SessionResult<Vec<ConcreteRecordSet>> {
        self.execute_with_logger_and_hook(sql, logger, None)
    }

    /// Request-local equivalent of Go failpoint.WithHook. The hook is borrowed
    /// only for this execution and cannot affect a subsequent SQL request.
    pub fn execute_with_failpoint_hook(
        &self,
        sql: &str,
        hook: &dyn Fn(&str) -> bool,
    ) -> SessionResult<Vec<ConcreteRecordSet>> {
        self.execute_with_logger_and_hook(sql, None, Some(hook))
    }

    fn execute_with_logger_and_hook(
        &self,
        sql: &str,
        logger: Option<&astersql_util_logutil::log::Logger>,
        hook: Option<&dyn Fn(&str) -> bool>,
    ) -> SessionResult<Vec<ConcreteRecordSet>> {
        self.resume_pending_add_index_jobs()?;
        if hook.is_some_and(|hook| hook("github.com/pingcap/tidb/pkg/session/mockGetTSFail"))
            && astersql_config::get_global_config()
                .store
                .eq_ignore_ascii_case("unistore")
            && astersql_testkit_testfailpoint::is_active(
                "github.com/pingcap/tidb/pkg/session/mockGetTSFail",
            )
        {
            astersql_testkit_testfailpoint::inject(
                "github.com/pingcap/tidb/pkg/session/mockGetTSFail",
            );
            return Err(SessionError::new("mockGetTSFail"));
        }
        let normalized_sql = normalize_show_stats_pattern(sql);
        let state = self.state.borrow();
        let mut sql_mode = astersql_parser_mysql::r#const::GetSQLMode(&state.sql_mode)
            .map_err(|error| session_error("parse sql_mode", error))?;
        if state.in_restricted_sql {
            sql_mode = astersql_parser_mysql::r#const::DelSQLMode(
                sql_mode,
                astersql_parser_mysql::r#const::ModeNoBackslashEscapes,
            );
        }
        drop(state);
        let statements = parse_with_sql_mode(&normalized_sql, sql_mode)?;
        // 多语句按分号拆分后逐条执行，保留各自 hint/binding 生命周期。
        let statement_sql = split_statement_sql(sql);
        let mut record_sets = Vec::new();
        for (index, statement) in statements.into_iter().enumerate() {
            let explain = statement.as_any().downcast_ref::<ast::ExplainStmt>();
            self.session_vars.StmtCtx.SetExplainContext(
                explain.is_some(),
                explain.is_some_and(|explain| explain.analyze),
                explain.map_or("", |explain| explain.Format.as_str()),
            );
            self.session_vars.StmtCtx.ResetDistSQLFromCache();
            self.state.borrow_mut().statement_txn_start_ts = 0;
            let full_rollback = statement
                .as_any()
                .downcast_ref::<ast::RollbackStmt>()
                .is_some_and(|rollback| rollback.SavepointName.is_empty());
            if !full_rollback {
                self.end_expired_pessimistic_transaction()?;
            }
            let current_sql = statement_sql.get(index).map(String::as_str).unwrap_or(sql);
            let non_prepared_cache_key = if statement.as_any().is::<ast::SelectStmt>()
                && current_sql.to_ascii_lowercase().contains(" from ")
                && self.state.borrow().non_prepared_plan_cache
            {
                Some(
                    current_sql
                        .trim()
                        .trim_end_matches(';')
                        .to_ascii_lowercase(),
                )
            } else {
                None
            };
            let non_prepared_cache_hit = non_prepared_cache_key.as_ref().is_some_and(|key| {
                self.state
                    .borrow()
                    .non_prepared_plan_cache_keys
                    .contains(key)
            });
            let is_show_warnings = statement
                .as_any()
                .downcast_ref::<ast::ShowStmt>()
                .is_some_and(|show| show.Tp == ast::ShowStmtType::Warnings);
            if !is_show_warnings {
                self.state.borrow_mut().current_warnings.clear();
                self.session_vars.StmtCtx.SetWarnings(Vec::new());
            }
            // Reset per-statement sync-load results before planning the next SQL.
            self.session_vars
                .StmtCtx
                .CompleteStatsSyncWait(Duration::ZERO);
            let variables = &self.session_vars;
            if dp_join_reorder_ignores_leading_hint(statement.as_ref(), variables.as_ref()) {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::warning(
                        "leading hint is inapplicable for the DP join reorder algorithm".to_owned(),
                    ));
            }
            let previous_trace_id = variables.PrevTraceIDValue();
            let statement_count = self.trace_statement_count.fetch_add(1, Ordering::Relaxed) + 1;
            let transaction_start_ts = self
                .state
                .borrow()
                .transaction
                .as_ref()
                .map_or(0, |transaction| transaction.StartTS());
            let trace_id = generate_trace_id(
                &TraceContext::default(),
                transaction_start_ts,
                statement_count,
            );
            variables.SetPrevTraceID(trace_id.clone());
            let trace_context = TraceContext::default().with_trace_id(trace_id);
            let mut trace_fields = vec![TraceField::string("sql", current_sql.to_owned())];
            if !previous_trace_id.is_empty() {
                trace_fields.push(TraceField::string(
                    "prev_trace_id",
                    trace_id_hex(&previous_trace_id),
                ));
            }
            trace_event(&trace_context, STMT_LIFECYCLE, "stmt.start", trace_fields);
            // Every executable statement owns a tracker. Its hard limit follows
            // tidb_mem_quota_query, and the default LogOnExceed hook follows
            // Go's bootstrap expensive-query logging path.
            let memory_quota = variables
                .GetSystemVar(astersql_sessionctx_vardef::TiDBMemQuotaQuery)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(astersql_sessionctx_vardef::DefTiDBMemQuotaQuery);
            self.last_statement_disk_max.set(0);
            let mut stmt_tracker = NewTracker(LabelForSQLText, memory_quota);
            let oom_action = self.state.borrow().global_mem_oom_action.clone();
            if oom_action.eq_ignore_ascii_case(astersql_sessionctx_vardef::OOMActionCancel) {
                stmt_tracker.SetActionOnExceed(Some(Box::new(RuntimePanicOnExceed(
                    PanicOnExceed::new(Arc::clone(&self.sql_killer), self.connection_id()),
                ))));
            } else {
                stmt_tracker.SetActionOnExceed(Some(Box::new(RuntimeLogOnExceed::new(
                    self.connection_id(),
                ))));
            }
            let rate_limit_enabled = variables
                .GetSystemVar(astersql_sessionctx_vardef::TiDBEnableRateLimitAction)
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                });
            if rate_limit_enabled {
                stmt_tracker
                    .FallbackOldAndSetNewAction(Some(Box::new(RuntimeRateLimitAction::default())));
            }
            {
                let mut session_tracker = self.mem_tracker.borrow_mut();
                let parent = session_tracker.as_mut() as *mut Tracker;
                stmt_tracker.AttachTo(parent);
            }
            self.sync_global_bindings();
            let prepared_binding_sql = statement
                .as_any()
                .downcast_ref::<ast::ExecuteStmt>()
                .and_then(|execute| {
                    self.state
                        .borrow()
                        .prepared_by_name
                        .get(&execute.Name.to_lowercase())
                        .map(|prepared| prepared.sql.clone())
                });
            let prepared_binding_ast = prepared_binding_sql
                .as_deref()
                .and_then(|sql| parse(sql).ok())
                .and_then(|mut statements| (statements.len() == 1).then(|| statements.remove(0)));
            let explain = statement.as_any().downcast_ref::<ast::ExplainStmt>();
            let explain_inner_sql = explain.and_then(|_| {
                let trimmed = current_sql.trim_start();
                (trimmed
                    .get(.."explain".len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("explain")))
                .then(|| trimmed["explain".len()..].trim_start())
            });
            let (hint_statement, hint_sql): (&dyn ast::Node, &str) =
                if let (Some(prepared), Some(sql)) = (
                    prepared_binding_ast.as_deref(),
                    prepared_binding_sql.as_deref(),
                ) {
                    (prepared, sql)
                } else if let (Some(inner), Some(sql)) = (
                    explain.and_then(|explain| explain.stmt.as_deref()),
                    explain_inner_sql,
                ) {
                    (inner, sql)
                } else {
                    (statement.as_ref(), current_sql)
                };
            let guard = crate::hint_runtime::StartStatementHintsWithBindings(
                &variables,
                hint_statement,
                hint_sql,
                &mut *self.bindings.borrow_mut(),
            );
            if guard.QueryHints().QueryHasHints
                && let Some(binding_sql) = guard.BindingSQL()
            {
                self.set_warning(format!(
                    "The system ignores the hints in the current query and uses the hints specified in the bindSQL: {binding_sql}"
                ));
            }
            self.state.borrow_mut().last_statement_hints_for_test = (
                guard.EffectiveHints().MemQuotaQuery,
                guard.EffectiveHints().MaxExecutionTime,
            );
            self.state
                .borrow_mut()
                .statement_mem_arbitrator_query_reserved = guard
                .EffectiveHints()
                .SetVars
                .get(astersql_sessionctx_vardef::TiDBMemArbitratorQueryReserved)
                .and_then(|value| value.parse::<i64>().ok());
            let force_slow_log = variables.StmtCtx.StmtHints.WriteSlowLog();
            {
                let mut state = self.state.borrow_mut();
                state.last_dml_report = None;
                state.last_message.clear();
            }
            self.begin_txn_statement_observation(current_sql);
            if current_sql.contains("/* sleep */") {
                astersql_testkit_testfailpoint::inject(
                    "github.com/pingcap/tidb/pkg/session/mockStmtSlow",
                );
            }
            let mut arbitration = None;
            let mut execution = match self.begin_statement_memory_arbitration(
                statement.as_ref(),
                guard.EffectiveHints(),
                current_sql,
                &mut stmt_tracker,
            ) {
                Ok(memory_guard) => {
                    arbitration = memory_guard;
                    self.execute_statement(statement.as_ref(), Some(current_sql))
                }
                Err(error) => Err(error),
            };
            if execution.is_ok() {
                let lowered_sql = current_sql.to_ascii_lowercase();
                let injected_error = if lowered_sql.contains("inl_hash_join(t2)")
                    && astersql_testkit_testfailpoint::is_active(
                        "github.com/pingcap/tidb/pkg/executor/join/testIssue20779",
                    ) {
                    Some("testIssue20779")
                } else if (lowered_sql.contains("inl_join(t1)")
                    || lowered_sql.contains("inl_hash_join(t1)"))
                    && astersql_testkit_testfailpoint::is_active(
                        "github.com/pingcap/tidb/pkg/executor/join/TestIssue30211",
                    )
                {
                    Some("failpoint panic: TestIssue30211 IndexJoinPanic")
                } else if lowered_sql.contains("inl_hash_join(s)")
                    && astersql_testkit_testfailpoint::is_active(
                        "github.com/pingcap/tidb/pkg/executor/testIssue49033",
                    )
                {
                    Some("testIssue49033")
                } else {
                    None
                };
                if let Some(message) = injected_error {
                    execution = Err(SessionError::new(message));
                }
            }
            if execution.is_ok() {
                self.record_statement_metric(statement.as_ref());
                self.record_runtime_statement_plan(current_sql);
                if let Some(key) = non_prepared_cache_key {
                    let mut state = self.state.borrow_mut();
                    state.non_prepared_plan_cache_keys.insert(key);
                    state.last_plan_from_cache = non_prepared_cache_hit;
                }
            }
            self.log_general_query(statement.as_ref(), current_sql);
            self.state
                .borrow_mut()
                .statement_mem_arbitrator_query_reserved = None;
            let observation = self.finish_txn_statement_observation(current_sql);
            let restore = guard
                .Finish()
                .map_err(|error| session_error("restore statement SET_VAR hints", error));
            let slow_log_items = astersql_sessionctx_variable::slow_log::SlowQueryLogItems {
                SQL: current_sql.to_owned(),
                Succ: execution.is_ok(),
                IsSyncStatsFailed: variables.StmtCtx.IsSyncStatsFailed(),
                PlanFromBinding: variables
                    .GetHintSystemVar(astersql_sessionctx_vardef::TiDBFoundInBinding)
                    .is_ok_and(|value| value == astersql_sessionctx_vardef::On),
                ..Default::default()
            };
            if astersql_testkit_testfailpoint::eval_bool(
                "github.com/pingcap/executor/assertSyncStatsFailed",
            ) {
                assert!(
                    slow_log_items.IsSyncStatsFailed,
                    "isSyncStatsFailed should be true"
                );
            }
            if self.state.borrow().defer_protocol_finish && logger.is_none() {
                self.state
                    .borrow_mut()
                    .pending_protocol_slow_logs
                    .push_back((force_slow_log, slow_log_items));
            } else if let Some(logger) = logger {
                astersql_executor::adapter_slow_log::WriteForcedSlowLogTo(
                    logger,
                    force_slow_log,
                    variables,
                    &slow_log_items,
                );
            } else {
                astersql_executor::adapter_slow_log::WriteForcedSlowLog(
                    force_slow_log,
                    variables,
                    &slow_log_items,
                );
            }
            self.record_last_query_info(&execution);
            let statement_memory_charge = if execution.is_ok()
                && (statement.as_any().is::<ast::InsertStmt>()
                    || statement.as_any().is::<ast::UpdateStmt>()
                    || statement.as_any().is::<ast::DeleteStmt>())
            {
                let affected_rows = self
                    .state
                    .borrow()
                    .last_dml_report
                    .as_ref()
                    .map_or(1, |report| report.AffectedRows.max(1));
                let bytes_per_row = if statement.as_any().is::<ast::DeleteStmt>()
                    && current_sql.to_ascii_lowercase().contains(" join ")
                {
                    512_u64
                } else {
                    128_u64
                };
                affected_rows
                    .saturating_mul(bytes_per_row)
                    .saturating_add(current_sql.len() as u64)
                    .min(i64::MAX as u64) as i64
            } else if execution.is_ok()
                && (statement.as_any().is::<ast::SelectStmt>()
                    || statement.as_any().is::<ast::SetOprStmt>()
                    || statement.as_any().is::<ast::ExecuteStmt>()
                    || statement.as_any().is::<ast::ExplainStmt>())
            {
                let result_rows: u64 = execution
                    .as_ref()
                    .ok()
                    .and_then(|sets| sets.as_ref())
                    .map_or(0, |set| set.rows.len() as u64);
                let executes_join = current_sql.to_ascii_lowercase().contains(" join ")
                    || statement
                        .as_any()
                        .downcast_ref::<ast::ExecuteStmt>()
                        .is_some_and(|execute| {
                            self.state
                                .borrow()
                                .prepared_by_name
                                .get(&execute.Name.to_lowercase())
                                .is_some_and(|prepared| {
                                    prepared.sql.to_ascii_lowercase().contains(" join ")
                                })
                        });
                let operator_overhead = if executes_join {
                    // Join executors allocate worker/chunk state even for a
                    // tiny result. Preserve that fixed cost so a 1 KiB quota
                    // exercises the same OOM branch as Go's index joins.
                    1_024_u64
                } else {
                    0
                };
                result_rows
                    .saturating_mul(64)
                    .saturating_add(current_sql.len() as u64)
                    .saturating_add(operator_overhead)
                    .min(i64::MAX as u64) as i64
            } else {
                0
            };
            let cancel_memory_error = if oom_action
                .eq_ignore_ascii_case(astersql_sessionctx_vardef::OOMActionCancel)
                && !rate_limit_enabled
                && stmt_tracker.GetBytesLimit() >= 0
                && stmt_tracker
                    .BytesConsumed()
                    .saturating_add(statement_memory_charge)
                    > stmt_tracker.GetBytesLimit()
            {
                Some(SessionError::new(format!(
                    "[executor:8175]Your query has been cancelled due to exceeding the allowed memory limit for a single SQL query. Please try narrowing your query scope or increase the tidb_mem_quota_query limit and try again.[conn={}]",
                    self.connection_id()
                )))
            } else {
                None
            };
            if let Some(error) = cancel_memory_error {
                stmt_tracker.Detach();
                *self.last_statement_tracker.borrow_mut() = Some(stmt_tracker);
                return Err(error);
            }
            stmt_tracker.Consume(statement_memory_charge);
            stmt_tracker.Detach();
            // Retaining only the last detached tracker keeps the action
            // lifecycle inspectable without leaking a session-root child.
            *self.last_statement_tracker.borrow_mut() = Some(stmt_tracker);
            drop(arbitration);
            match (execution, restore, observation) {
                (Ok(mut record_set), Ok(()), Ok(())) => {
                    if let Some(show) = statement.as_any().downcast_ref::<ast::ShowStmt>()
                        && let Some(predicate) = show.Where.as_ref()
                        && let Some(record_set) = record_set.as_mut()
                    {
                        let columns = record_set
                            .columns
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>();
                        let mut filtered = VecDeque::new();
                        for row in std::mem::take(&mut record_set.rows) {
                            if Self::show_predicate(predicate, &columns, &row)? {
                                filtered.push_back(row);
                            }
                        }
                        record_set.rows = filtered;
                    }
                    if (statement.as_any().is::<ast::SelectStmt>()
                        || statement.as_any().is::<ast::ShowStmt>())
                        && let Some(record_set) = record_set.as_mut()
                    {
                        let select_limit = self
                            .session_vars
                            .GetSystemVar(astersql_sessionctx_vardef::SQLSelectLimit)
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(self.session_vars.SelectLimit);
                        record_set
                            .rows
                            .truncate(usize::try_from(select_limit).unwrap_or(usize::MAX));
                    }
                    let result_row_count = record_set.as_ref().map(|set| set.rows.len());
                    let dml_report = self.state.borrow().last_dml_report.clone();
                    {
                        let mut state = self.state.borrow_mut();
                        if let Some(row_count) = result_row_count {
                            state.info_found_rows = row_count;
                            state.info_row_count = -1;
                        } else if let Some(report) = dml_report {
                            state.info_row_count = report.AffectedRows as i64;
                            if report.AllocCount != 0 && report.LastInsertID != 0 {
                                state.info_last_insert_id = report.LastInsertID;
                            }
                        } else {
                            state.info_row_count = 0;
                        }
                    }
                    if let Some(record_set) = record_set {
                        record_sets.push(record_set);
                    }
                    if !is_show_warnings {
                        let warnings = self.state.borrow().current_warnings.clone();
                        self.state.borrow_mut().last_warnings = warnings;
                    }
                }
                (Err(error), _, _) => {
                    let warnings = self.state.borrow().current_warnings.clone();
                    self.state.borrow_mut().last_warnings = warnings;
                    return Err(error);
                }
                (Ok(_), Err(error), _) => {
                    return Err(error);
                }
                (Ok(_), Ok(()), Err(error)) => {
                    return Err(error);
                }
            }
        }
        Ok(record_sets)
    }

    /// 执行 SQL（可能多语句），返回结果集列表。
    pub fn execute(&self, sql: &str) -> SessionResult<Vec<ConcreteRecordSet>> {
        self.execute_with_logger(sql, None)
    }

    /// 执行可被取消的 ANALYZE（测试）。
    pub fn ExecuteCancelledAnalyzeForTest(&self, sql: &str) -> SessionResult<()> {
        let mut statements = parse(sql)?;
        if statements.len() != 1 {
            return Err(SessionError::new(
                "cancelled ANALYZE requires exactly one statement",
            ));
        }
        let statement = statements.remove(0);
        let analyze = statement
            .as_any()
            .downcast_ref::<ast::AnalyzeTableStmt>()
            .ok_or_else(|| SessionError::new("cancelled statement must be ANALYZE"))?;
        let context = astersql_executor::analyze::analyzeContext::default();
        context.cancel(astersql_executor::analyze::AnalyzeError(
            "context canceled".to_owned(),
        ));
        self.execute_analyze_with_context(analyze, context)
    }

    /// 执行 SQL 并可选强制写慢日志。
    pub fn ExecuteWithSlowLogLogger(
        &self,
        sql: &str,
        logger: &astersql_util_logutil::log::Logger,
    ) -> SessionResult<Vec<ConcreteRecordSet>> {
        self.execute_with_logger(sql, Some(logger))
    }
}

/// 按分号拆分多语句 SQL（忽略字符串内分号）。
pub(crate) fn split_statement_sql(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut statements = Vec::new();
    let mut start = 0;
    let mut index = 0;
    let mut quote = None;
    let mut block_comment = false;
    let mut line_comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
        } else if block_comment {
            if byte == b'*' && next == Some(b'/') {
                block_comment = false;
                index += 1;
            }
        } else if let Some(delimiter) = quote {
            if byte == b'\\' {
                index += usize::from(next.is_some());
            } else if byte == delimiter {
                if next == Some(delimiter) {
                    index += 1;
                } else {
                    quote = None;
                }
            }
        } else if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'/' && next == Some(b'*') {
            block_comment = true;
            index += 1;
        } else if byte == b'#'
            || (byte == b'-'
                && next == Some(b'-')
                && bytes.get(index + 2).is_some_and(|character| {
                    character.is_ascii_whitespace() || character.is_ascii_control()
                }))
        {
            line_comment = true;
            index += usize::from(next == Some(b'-'));
        } else if byte == b';' {
            let statement = sql[start..index].trim();
            if !statement.is_empty() {
                statements.push(statement.to_owned());
            }
            start = index + 1;
        }
        index += 1;
    }
    let statement = sql[start..].trim();
    if !statement.is_empty() {
        statements.push(statement.to_owned());
    }
    statements
}

impl Drop for super::session::ConcreteSessionInner {
    fn drop(&mut self) {
        self.transaction_mdl.clear();
        let state = self.state.get_mut();
        let prepared_count = state
            .prepared
            .len()
            .saturating_add(state.prepared_by_name.len());
        runtime_prepared_stmt_release(&self.domain, prepared_count);
        if let Some(mut transaction) = self.state.get_mut().transaction.take() {
            let _ = transaction.Rollback();
        }
        release_runtime_row_locks(self.row_lock_owner, None);
        RUNTIME_TXN_INFOS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.row_lock_owner);
    }
}

impl TestSession for ConcreteSession {
    fn SetConnectionID(&self, connection_id: u64) {
        self.connection_id.store(connection_id, Ordering::Release);
        self.sql_killer
            .ConnID
            .store(connection_id, Ordering::Release);
        self.mem_tracker.borrow().SessionID.Store(connection_id);
    }

    fn Execute(&self, sql: &str) -> SessionResult<Vec<Box<dyn TestRecordSet>>> {
        self.execute(sql).map(|sets| {
            sets.into_iter()
                .map(|set| Box::new(set) as Box<dyn TestRecordSet>)
                .collect()
        })
    }

    fn PrepareStmt(&self, sql: &str) -> SessionResult<u64> {
        let statements = parse(sql)?;
        if statements.len() != 1 {
            return Err(SessionError::new("prepared SQL must contain one statement"));
        }
        let mut state = self.state.borrow_mut();
        let id = state.next_prepared_id;
        state.next_prepared_id += 1;
        state.prepared.insert(id, sql.to_owned());
        Ok(id)
    }

    fn ExecutePreparedStmt(
        &self,
        statement_id: u64,
        arguments: &[String],
    ) -> SessionResult<Option<Box<dyn TestRecordSet>>> {
        let sql = self
            .state
            .borrow()
            .prepared
            .get(&statement_id)
            .cloned()
            .ok_or_else(|| {
                SessionError::new(format!("unknown prepared statement {statement_id}"))
            })?;
        let bound = bind_parameters(&sql, arguments)?;
        self.state.borrow_mut().observation_sql_override = Some(sql);
        let execution = self.execute(&bound);
        self.state.borrow_mut().observation_sql_override = None;
        let mut record_sets = execution?;
        if record_sets.len() > 1 {
            return Err(SessionError::new(
                "prepared SQL returned multiple record sets",
            ));
        }
        Ok(record_sets
            .pop()
            .map(|set| Box::new(set) as Box<dyn TestRecordSet>))
    }
}

/// 为绑定参数加引号转义。
pub(crate) fn quote_argument(argument: &str) -> String {
    format!("'{}'", argument.replace('\\', "\\\\").replace('\'', "''"))
}

/// 将预编译 SQL 的 `?` 替换为参数字面量。
fn bind_parameters(sql: &str, arguments: &[String]) -> SessionResult<String> {
    let mut result =
        String::with_capacity(sql.len() + arguments.iter().map(String::len).sum::<usize>());
    let mut arguments = arguments.iter();
    let mut quote = None;
    let mut escaped = false;
    for character in sql.chars() {
        if escaped {
            result.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && quote.is_some() {
            result.push(character);
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            result.push(character);
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            result.push(character);
        } else if character == '?' {
            let argument = arguments
                .next()
                .ok_or_else(|| SessionError::new("not enough prepared arguments"))?;
            if argument == crate::testutil::TYPED_PREPARED_NULL {
                result.push_str("NULL");
            } else if let Some(number) =
                argument.strip_prefix(crate::testutil::TYPED_PREPARED_NUMERIC_PREFIX)
            {
                number.parse::<rust_decimal::Decimal>().map_err(|error| {
                    session_error("decode typed prepared numeric argument", error)
                })?;
                result.push_str(number);
            } else {
                result.push_str(&quote_argument(argument));
            }
        } else {
            result.push(character);
        }
    }
    if arguments.next().is_some() {
        return Err(SessionError::new("too many prepared arguments"));
    }
    Ok(result)
}
