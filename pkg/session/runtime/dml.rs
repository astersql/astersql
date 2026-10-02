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

//! Relational DML storage execution, constraints, generated columns, and statistics.

use super::*;

fn ignored_integer_overflow_value(
    column: &astersql_meta_model::ColumnInfo,
    literal: &str,
) -> Option<String> {
    if !astersql_parser_mysql::util::IsIntegerType(column.GetType()) {
        return None;
    }
    let value = literal.parse::<i128>().ok()?;
    let unsigned = astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag());
    let (minimum, maximum) = if unsigned {
        let maximum = match column.GetType() {
            astersql_parser_mysql::r#type::TypeTiny => u8::MAX as i128,
            astersql_parser_mysql::r#type::TypeShort => u16::MAX as i128,
            astersql_parser_mysql::r#type::TypeInt24 => 0x00ff_ffff,
            astersql_parser_mysql::r#type::TypeLong => u32::MAX as i128,
            _ => u64::MAX as i128,
        };
        (0, maximum)
    } else {
        match column.GetType() {
            astersql_parser_mysql::r#type::TypeTiny => (i8::MIN as i128, i8::MAX as i128),
            astersql_parser_mysql::r#type::TypeShort => (i16::MIN as i128, i16::MAX as i128),
            astersql_parser_mysql::r#type::TypeInt24 => (-0x80_0000, 0x7f_ffff),
            astersql_parser_mysql::r#type::TypeLong => (i32::MIN as i128, i32::MAX as i128),
            _ => (i64::MIN as i128, i64::MAX as i128),
        }
    };
    Some(value.clamp(minimum, maximum).to_string())
}

fn ignored_not_null_value(column: &astersql_meta_model::ColumnInfo) -> String {
    match column.GetType() {
        astersql_parser_mysql::r#type::TypeDate => "0000-00-00".to_owned(),
        astersql_parser_mysql::r#type::TypeDatetime
        | astersql_parser_mysql::r#type::TypeTimestamp => "0000-00-00 00:00:00".to_owned(),
        astersql_parser_mysql::r#type::TypeDuration => "00:00:00".to_owned(),
        astersql_parser_mysql::r#type::TypeString
        | astersql_parser_mysql::r#type::TypeVarString
        | astersql_parser_mysql::r#type::TypeVarchar
        | astersql_parser_mysql::r#type::TypeBlob => String::new(),
        _ => "0".to_owned(),
    }
}

fn relational_dml_limit_window(
    limit: Option<&ast::Limit>,
) -> SessionResult<Option<RelationalLimitWindow>> {
    limit
        .map(|limit| {
            let count = limit
                .Count
                .as_ref()
                .map(|expression| {
                    literal(expression)?
                        .parse::<usize>()
                        .map_err(|error| session_error("parse DML LIMIT", error))
                })
                .transpose()?
                .unwrap_or(usize::MAX);
            let offset = limit
                .Offset
                .as_ref()
                .map(|expression| {
                    literal(expression)?
                        .parse::<usize>()
                        .map_err(|error| session_error("parse DML OFFSET", error))
                })
                .transpose()?
                .unwrap_or(0);
            Ok(RelationalLimitWindow { offset, count })
        })
        .transpose()
}

pub(super) fn runtime_unique_lock_key(table_id: i64, index_id: i64, values: &[String]) -> Vec<u8> {
    let mut key = b"astersql/runtime/unique/".to_vec();
    key.extend_from_slice(&table_id.to_be_bytes());
    key.extend_from_slice(&index_id.to_be_bytes());
    for value in values {
        key.extend_from_slice(&(value.len() as u64).to_be_bytes());
        key.extend_from_slice(value.as_bytes());
    }
    key
}

pub(super) fn unique_lock_keys_for_rows<'a>(
    table: &astersql_meta_model::TableInfo,
    rows: impl IntoIterator<Item = &'a HashMap<String, Option<String>>>,
) -> Vec<Vec<u8>> {
    rows.into_iter()
        .flat_map(|row| {
            table
                .Indices
                .iter()
                .filter(|index| index.Unique && index.State == astersql_meta_model::StatePublic)
                .filter_map(|index| {
                    let values = index
                        .Columns
                        .iter()
                        .map(|column| row.get(&column.Name.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>()?;
                    Some(runtime_unique_lock_key(table.ID, index.ID, &values))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(super) fn primary_lock_keys_for_rows<'a>(
    table: &astersql_meta_model::TableInfo,
    rows: impl IntoIterator<Item = &'a HashMap<String, Option<String>>>,
) -> Vec<Vec<u8>> {
    let primary = table
        .Columns
        .iter()
        .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
        .collect::<Vec<_>>();
    rows.into_iter()
        .filter_map(|row| {
            let values = primary
                .iter()
                .map(|column| row.get(&column.Name.L).cloned().flatten())
                .collect::<Option<Vec<_>>>()?;
            (!values.is_empty()).then(|| runtime_unique_lock_key(table.ID, i64::MIN, &values))
        })
        .collect()
}

pub(super) fn unique_lock_keys_for_predicate(
    table: &astersql_meta_model::TableInfo,
    predicate: &ast::ExprNode,
) -> Vec<Vec<u8>> {
    let column_values = match &predicate.Kind {
        ast::ExprKind::InList {
            Expr, List, Not, ..
        } if !*Not => {
            let ast::ExprKind::Column(column) = &Expr.Kind else {
                return Vec::new();
            };
            (
                column.Name.L.as_str(),
                List.iter()
                    .filter_map(|item| literal(item).ok())
                    .collect::<Vec<_>>(),
            )
        }
        ast::ExprKind::Binary { Op, L, R } if Op == "=" || Op == "==" => match (&L.Kind, &R.Kind) {
            (ast::ExprKind::Column(column), _) => (
                column.Name.L.as_str(),
                literal(R).ok().into_iter().collect::<Vec<_>>(),
            ),
            (_, ast::ExprKind::Column(column)) => (
                column.Name.L.as_str(),
                literal(L).ok().into_iter().collect::<Vec<_>>(),
            ),
            _ => return Vec::new(),
        },
        ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("and") => {
            let mut keys = unique_lock_keys_for_predicate(table, L);
            keys.extend(unique_lock_keys_for_predicate(table, R));
            return keys;
        }
        _ => return Vec::new(),
    };
    let mut keys = table
        .Indices
        .iter()
        .filter(|index| {
            index.Unique
                && index.State == astersql_meta_model::StatePublic
                && index.Columns.len() == 1
                && index.Columns[0].Name.L == column_values.0
        })
        .flat_map(|index| {
            column_values.1.iter().map(|value| {
                runtime_unique_lock_key(table.ID, index.ID, std::slice::from_ref(value))
            })
        })
        .collect::<Vec<_>>();
    let primary_columns = table
        .Columns
        .iter()
        .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
        .collect::<Vec<_>>();
    if primary_columns.len() == 1 && primary_columns[0].Name.L == column_values.0 {
        keys.extend(
            column_values.1.iter().map(|value| {
                runtime_unique_lock_key(table.ID, i64::MIN, std::slice::from_ref(value))
            }),
        );
    }
    keys
}

impl ConcreteSession {
    /// Preserve Go's transaction-context TTL counter around savepoints. Rows
    /// before any savepoint retain the existing immediate metric behavior;
    /// rows after a savepoint are held until commit so rollback-to can discard
    /// only the reverted suffix.
    fn record_ttl_insert_rows(&self, table: &astersql_meta_model::TableInfo, rows: usize) {
        if rows == 0 || !table.TTLInfo.as_ref().is_some_and(|info| info.Enable) {
            return;
        }
        let mut state = self.state.borrow_mut();
        if !state.savepoints.is_empty() {
            state.pending_ttl_insert_rows = state.pending_ttl_insert_rows.saturating_add(rows);
            return;
        }
        drop(state);
        increment_ttl_insert_rows_metric(rows);
    }

    /// 注册/持久化关系型 DML 表元数据。
    pub fn RegisterDmlTable(&self, info: astersql_meta_model::TableInfo) -> SessionResult<()> {
        if info.ID == 0 || info.Name.L.is_empty() || info.Columns.is_empty() {
            return Err(SessionError::new(
                "DML TableInfo requires ID, name and columns",
            ));
        }
        let current_database = self.current_database();
        if self
            .mdl_stats_table(&current_database, &info.Name.L)
            .is_none()
        {
            self.domain
                .ddl_create_table(&current_database, info, false)
                .map_err(|error| session_error("persist DML table metadata", error))?;
        }
        Ok(())
    }

    /// 注入下一次 DML 提交错误（测试）。
    pub fn InjectNextDmlCommitError(&self, message: impl Into<String>) {
        self.state.borrow_mut().next_dml_commit_error = Some(message.into());
    }

    /// Install the AUTO_INCREMENT IDs that a retrying statement must reuse.
    pub fn SetRetryAutoIncrementIDsForTest(&self, ids: Vec<u64>) {
        self.state.borrow_mut().retry_auto_increment_ids = ids.into();
    }

    /// 读取当前库下已注册 DML 表的全部行。
    pub fn ReadDmlRows(
        &self,
        table_name: &str,
    ) -> SessionResult<Vec<HashMap<String, Option<String>>>> {
        self.read_dml_rows_in_database(&self.current_database(), table_name)
    }

    /// 在指定库下扫描 DML 表行。
    pub(super) fn read_dml_rows_in_database(
        &self,
        database: &str,
        table_name: &str,
    ) -> SessionResult<Vec<HashMap<String, Option<String>>>> {
        let table = self
            .resolve_runtime_table(database, table_name)
            .ok_or_else(|| {
                SessionError::new(format!("unknown DML table {database}.{table_name}"))
            })?;
        let rows = self.scan_registered_table(&table)?;
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }

    /// 返回最近一次 DML 执行报告。
    pub fn LastDmlReport(&self) -> Option<crate::dml_runtime::DmlExecutionReport> {
        self.state.borrow().last_dml_report.clone()
    }

    /// Number of transport/TSO attempts made by the latest autocommit commit.
    pub fn LastAutocommitRetryAttempts(&self) -> usize {
        self.state.borrow().last_autocommit_retry_attempts
    }

    /// Inject an autocommit commit failure followed by `tso_failures` transient
    /// timestamp failures. Unlike process-wide failpoints, this hook cannot be
    /// consumed by another session running concurrently.
    pub fn InjectAutocommitRetryForTest(&self, tso_failures: usize) {
        self.state.borrow_mut().next_autocommit_retry_tso_failures = Some(tso_failures);
    }

    /// 在当前事务或快照上扫描已注册表。
    pub(super) fn backfill_relational_index(
        &self,
        table: &astersql_meta_model::TableInfo,
        index: &astersql_meta_model::IndexInfo,
    ) -> SessionResult<()> {
        let flags = self.dml_type_flags();
        let rows = self.scan_registered_table(table)?;
        let partial_condition = (!index.ConditionExprString.is_empty())
            .then(|| crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString))
            .transpose()?;
        for chunk in rows.chunks(RELATIONAL_INDEX_BACKFILL_BATCH_SIZE) {
            let mut mutations = Vec::new();
            for (_, row) in chunk {
                if partial_condition
                    .as_ref()
                    .is_some_and(|condition| !row_matches_simple_where(row, condition))
                {
                    continue;
                }
                for values in relational_index_value_rows(table, index, row, flags)? {
                    let (key, value) =
                        encode_relational_index_value_row(table, index, row, flags, values)?;
                    mutations.push((key, Some(value)));
                }
            }
            self.apply_relational_mutations(
                &table.Name.L,
                "AddIndexBackfill",
                mutations,
                Vec::new(),
                chunk.len() as u64,
                0,
                0,
                0,
            )?;
        }
        Ok(())
    }

    pub(super) fn clear_temporary_table_data(
        &self,
        table: &astersql_meta_model::TableInfo,
        operator: &str,
    ) -> SessionResult<()> {
        let flags = self.dml_type_flags();
        let rows = self.scan_registered_table(table)?;
        let mut mutations = Vec::new();
        for (_, row) in &rows {
            let (key, _) = encode_relational_row(table, row, flags)?;
            mutations.push((key, None));
            mutations.extend(relational_index_mutations(table, Some(row), None, flags)?);
        }
        self.apply_relational_mutations(
            &table.Name.L,
            operator,
            mutations,
            unique_lock_keys_for_rows(table, rows.iter().map(|(_, row)| row)),
            rows.len() as u64,
            0,
            0,
            0,
        )
    }

    /// Remove a local temporary table's rows and secondary index keys before
    /// its session catalog entry is forgotten. The enclosing DROP TABLE has
    /// already crossed the DDL implicit-commit boundary.
    pub(super) fn clear_local_temporary_table_data(
        &self,
        table: &astersql_meta_model::TableInfo,
    ) -> SessionResult<()> {
        self.clear_temporary_table_data(table, "DropTemporaryTable")
    }

    pub(super) fn record_transaction_related_table(&self, table: &astersql_meta_model::TableInfo) {
        let mut state = self.state.borrow_mut();
        if state.transaction.is_some() && table.TempTableType == astersql_meta_model::TempTableNone
        {
            state.transaction_related_table_ids.insert(table.ID);
        }
    }

    pub(super) fn record_transaction_locking_table(&self, table: &astersql_meta_model::TableInfo) {
        self.record_transaction_related_table(table);
        let mut state = self.state.borrow_mut();
        if state.transaction.is_some() && table.TempTableType == astersql_meta_model::TempTableNone
        {
            state.transaction_locking_table_ids.insert(table.ID);
        }
    }

    fn record_transaction_table_write(&self, table: &astersql_meta_model::TableInfo) {
        self.record_transaction_related_table(table);
        let mut state = self.state.borrow_mut();
        if state.transaction.is_some()
            && table.TempTableType == astersql_meta_model::TempTableGlobal
        {
            state
                .global_temporary_tables_in_transaction
                .insert(table.ID, table.clone());
        }
    }

    /// Validate child references and apply TiDB's FK check-key locking:
    /// pessimistic transactions hold compatible shared locks on parent rows,
    /// while optimistic transactions stage the checked parent keys so reverse
    /// reference order is detected as a write conflict during prewrite.
    fn validate_and_lock_foreign_keys(
        &self,
        child: &astersql_meta_model::TableInfo,
        rows: &[HashMap<String, Option<String>>],
        pending_parent_rows: &[HashMap<String, Option<String>>],
    ) -> SessionResult<()> {
        let (checks_enabled, shared_lock_checks, explicit, pessimistic, explicit_optimistic) = {
            let state = self.state.borrow();
            (
                state.foreign_key_checks,
                state.foreign_key_check_in_shared_lock,
                state.transaction.is_some(),
                state.transaction_pessimistic,
                state.transaction_explicit_optimistic,
            )
        };
        if !checks_enabled || child.ForeignKeys.is_empty() {
            return Ok(());
        }
        let current_database = self.current_database();
        let flags = self.dml_type_flags();
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        let mut checked = Vec::<(kv::Key, Vec<u8>)>::new();
        for foreign_key in &child.ForeignKeys {
            let database = if foreign_key.RefSchema.L.is_empty() {
                current_database.as_str()
            } else {
                foreign_key.RefSchema.L.as_str()
            };
            let parent = self
                .mdl_stats_table(database, &foreign_key.RefTable.L)
                .map(|(_, table)| table)
                .ok_or_else(|| {
                    SessionError::new(format!(
                        "referenced table {}.{} does not exist",
                        database, foreign_key.RefTable.O
                    ))
                })?;
            self.record_transaction_related_table(&parent);
            let parent_rows = self.scan_latest_with_transaction_overlay(&parent)?;
            let parent_rows_by_values = parent_rows
                .iter()
                .enumerate()
                .filter_map(|(index, (_, parent_row))| {
                    foreign_key
                        .RefCols
                        .iter()
                        .map(|column| parent_row.get(&column.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>()
                        .map(|values| (values, index))
                })
                .collect::<HashMap<_, _>>();
            for child_row in rows {
                let child_values = foreign_key
                    .Cols
                    .iter()
                    .map(|column| child_row.get(&column.L).cloned().flatten())
                    .collect::<Option<Vec<_>>>();
                let Some(child_values) = child_values else {
                    // SQL foreign keys do not check tuples containing NULL.
                    continue;
                };
                // A self-referencing multi-row DML statement can satisfy a
                // child row from another row staged by the same statement (or
                // from the row itself). Those rows are not visible in the KV
                // snapshot until statement commit, so include the statement
                // overlay before probing committed parent rows.
                let staged_parent_exists = parent.ID == child.ID
                    && pending_parent_rows.iter().chain(rows).any(|parent_row| {
                        let parent_values = foreign_key
                            .RefCols
                            .iter()
                            .map(|column| parent_row.get(&column.L).cloned().flatten())
                            .collect::<Option<Vec<_>>>();
                        parent_values.as_ref() == Some(&child_values)
                    });
                if staged_parent_exists {
                    continue;
                }
                let Some(parent_row) = parent_rows_by_values
                    .get(&child_values)
                    .and_then(|index| parent_rows.get(*index))
                    .map(|(_, row)| row)
                else {
                    return Err(SessionError::new(format!(
                        "[kv:1452]Cannot add or update a child row: a foreign key constraint fails \
                         (`{}`.`{}`, CONSTRAINT `{}` FOREIGN KEY)",
                        current_database, child.Name.O, foreign_key.Name.O
                    )));
                };
                checked.push(encode_relational_row(&parent, parent_row, flags)?);
            }
        }
        checked.sort_by(|left, right| left.0.0.cmp(&right.0.0));
        checked.dedup_by(|left, right| left.0 == right.0);
        let runtime_keys = checked
            .iter()
            .map(|(key, _)| RuntimeRowLockKey {
                domain_id,
                key: key.0.clone(),
            })
            .collect::<Vec<_>>();
        if explicit && pessimistic && self.state.borrow().adapter_dml_defer_fk_locks {
            self.WithSessionVars(|vars| {
                for key in &runtime_keys {
                    vars.TxnCtx
                        .AddUnchangedKeyForLock(&key.key, shared_lock_checks);
                }
            });
        } else if explicit && !explicit_optimistic {
            if shared_lock_checks {
                self.acquire_shared_row_locks(runtime_keys)?;
            } else {
                self.acquire_row_locks(runtime_keys, false, None, false)?;
            }
        } else if explicit && !pessimistic {
            // Optimistic transactions must not take a runtime row lock. Keep
            // the exact parent keys read by the FK check so prewrite can
            // compare them with the latest snapshot and report a write
            // conflict if another transaction deletes or updates the parent.
            let mut state = self.state.borrow_mut();
            for (key, _) in checked {
                state.optimistic_fk_check_keys.insert(key.0);
            }
        } else {
            // Before shared FK locks were introduced TiDB protected checked
            // parent keys with ordinary exclusive DML locks. Preserve that
            // behavior for the default OFF setting and autocommit statements.
            self.acquire_row_locks(runtime_keys, false, None, false)?;
        }
        Ok(())
    }

    /// Execute the autocommit retry state machine used by statement commit.
    /// A mock commit failure forces a fresh timestamp; transient timestamp
    /// failures are retried until the injection reports recovery.
    pub(super) fn run_autocommit_commit_retry(
        &self,
        injected_tso_failures: Option<usize>,
    ) -> SessionResult<usize> {
        if let Some(tso_failures) = injected_tso_failures {
            let mut attempts = 2;
            for _ in 0..tso_failures {
                self.sql_killer
                    .HandleSignal()
                    .map_err(|error| session_error("autocommit retry interrupted", error))?;
                attempts += 1;
            }
            return Ok(attempts);
        }
        if !astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/session/mockCommitError",
        ) {
            return Ok(1);
        }
        let mut attempts = 2;
        loop {
            self.sql_killer
                .HandleSignal()
                .map_err(|error| session_error("autocommit retry interrupted", error))?;
            if !astersql_testkit_testfailpoint::eval_bool("tikvclient/mockGetTSErrorInRetry") {
                break;
            }
            attempts += 1;
        }
        Ok(attempts)
    }

    /// 将关系型变更应用到事务。
    pub(super) fn apply_relational_mutations(
        &self,
        table: &str,
        operator: &str,
        mutations: Vec<(kv::Key, Option<Vec<u8>>)>,
        extra_lock_keys: Vec<Vec<u8>>,
        affected_rows: u64,
        last_insert_id: u64,
        alloc_count: u64,
        rebase_count: u64,
    ) -> SessionResult<()> {
        self.apply_relational_mutations_with_autocommit(
            table,
            operator,
            mutations,
            extra_lock_keys,
            affected_rows,
            last_insert_id,
            alloc_count,
            rebase_count,
            None,
        )
    }

    fn apply_relational_mutations_with_autocommit(
        &self,
        table: &str,
        operator: &str,
        mutations: Vec<(kv::Key, Option<Vec<u8>>)>,
        extra_lock_keys: Vec<Vec<u8>>,
        affected_rows: u64,
        last_insert_id: u64,
        alloc_count: u64,
        rebase_count: u64,
        autocommit_transaction: Option<Box<dyn kv::Transaction>>,
    ) -> SessionResult<()> {
        // Pin the schema used by these mutations before any commit pause. The
        // commit check must run regardless of whether a failpoint is enabled.
        let start_schema = self.domain.info_schema();
        let mut mlog_stats =
            BTreeMap::<i64, (astersql_statistics_handle::StatsTableKey, i64)>::new();
        for (key, value) in &mutations {
            if !astersql_tablecodec::IsRecordKey(&key.0) {
                continue;
            }
            let table_id =
                astersql_tablecodec::DecodeTableID(astersql_tablecodec::kv::Key(key.0.clone()));
            let Some(log_table) = start_schema.TableByID(table_id) else {
                continue;
            };
            let log_meta = log_table
                .ModelMeta()
                .map_err(|error| session_error("read MLog statistics metadata", error))?;
            if log_meta.MaterializedViewLog.is_none() || log_meta.Name.L.eq_ignore_ascii_case(table)
            {
                continue;
            }
            let database =
                astersql_infoschema::SchemaByTable(start_schema.as_ref(), log_table.Meta())
                    .ok_or_else(|| SessionError::new("MLog statistics schema is missing"))?;
            let entry = mlog_stats.entry(table_id).or_insert_with(|| {
                (
                    astersql_statistics_handle::StatsTableKey::new(
                        &database.name.lower,
                        &log_meta.Name.L,
                        table_id,
                    ),
                    0,
                )
            });
            entry.1 += if value.is_some() { 1 } else { -1 };
        }
        let write_keys = mutations.len() as u64;
        let write_bytes = mutations
            .iter()
            .map(|(key, value)| {
                key.0.len() as u64 + value.as_ref().map_or(0, |value| value.len() as u64) + 16
            })
            .sum::<u64>();
        let entry_limit = self.state.borrow().txn_entry_size_limit;
        if mutations
            .iter()
            .map(|(key, value)| key.0.len() + value.as_ref().map_or(0, Vec::len))
            .max()
            .is_some_and(|entry_size| entry_size > entry_limit)
        {
            return Err(SessionError::new(format!(
                "[kv:8025]entry too large, the max entry size is {entry_limit}"
            )));
        }
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        let mutation_runtime_keys = mutations
            .iter()
            .map(|(key, _)| RuntimeRowLockKey {
                domain_id,
                key: key.0.clone(),
            })
            .chain(
                extra_lock_keys
                    .iter()
                    .cloned()
                    .map(|key| RuntimeRowLockKey { domain_id, key }),
            )
            .collect::<Vec<_>>();
        let explicit_transaction = self.state.borrow().transaction.is_some();
        let conflict_context = self
            .mdl_stats_table(&self.current_database(), table)
            .and_then(|(_, info)| {
                mutations.iter().find_map(|(key, _)| {
                    if !astersql_tablecodec::IsRecordKey(&key.0) {
                        return None;
                    }
                    let (_, handle) = astersql_tablecodec::DecodeRecordKey(
                        astersql_tablecodec::kv::Key(key.0.clone()),
                    )
                    .ok()?;
                    let handle = if handle.IsInt() {
                        handle.IntValue().to_string()
                    } else {
                        let (_, datum) =
                            astersql_tablecodec::codec::DecodeOne(&handle.EncodedCol(0)).ok()?;
                        let value = datum.ToString().ok()?;
                        format!("{{{value}}}")
                    };
                    Some((info.Name.L.clone(), handle))
                })
            });
        let should_lock = !explicit_transaction || {
            let state = self.state.borrow();
            state.transaction_pessimistic && state.constraint_check_in_place_pessimistic
        };
        if should_lock {
            self.acquire_row_locks(
                mutations
                    .iter()
                    .map(|(key, _)| RuntimeRowLockKey {
                        domain_id,
                        key: key.0.clone(),
                    })
                    .chain(
                        extra_lock_keys
                            .into_iter()
                            .map(|key| RuntimeRowLockKey { domain_id, key }),
                    )
                    .collect(),
                false,
                None,
                false,
            )?;
        }
        let mut state = self.state.borrow_mut();
        let mut committed = false;
        let mut commit_wait = std::time::Duration::ZERO;
        if let Some(transaction) = state.transaction.as_mut() {
            for (key, value) in mutations {
                match value {
                    Some(value) => transaction.Set(key, value),
                    None => transaction.Delete(key),
                }
                .map_err(|error| session_kv_error("apply relational transaction DML", error))?;
            }
            state.txn_mem_buffer_keys = state.txn_mem_buffer_keys.saturating_add(write_keys);
            state.txn_mem_buffer_bytes = state.txn_mem_buffer_bytes.saturating_add(write_bytes);
            state
                .transaction_write_keys
                .extend(mutation_runtime_keys.iter().cloned());
            if state.transaction_conflict_context.is_none() {
                state.transaction_conflict_context = conflict_context;
            }
        } else {
            let injected_commit_error = state.next_dml_commit_error.take();
            let mut transaction = match autocommit_transaction {
                Some(transaction) => transaction,
                None => self
                    .domain
                    .storage()
                    .with_storage(|store| store.Begin(&[]))
                    .map_err(|error| session_error("begin relational autocommit", error))?,
            };
            transaction.SetOption(
                kv::EnableAsyncCommit,
                Some(Box::new(state.enable_async_commit)),
            );
            transaction.SetOption(kv::Enable1PC, Some(Box::new(state.enable_1pc)));
            transaction.SetOption(
                kv::Pessimistic,
                Some(Box::new(
                    !state.in_restricted_sql
                        && astersql_config::get_global_config()
                            .pessimistic_txn
                            .pessimistic_auto_commit
                            .load(),
                )),
            );
            state.statement_txn_start_ts = transaction.StartTS();
            state.last_observed_store_ts = transaction.StartTS();
            for (key, value) in mutations {
                let result = match value {
                    Some(value) => transaction.Set(key, value),
                    None => transaction.Delete(key),
                };
                if let Err(error) = result {
                    let _ = transaction.Rollback();
                    drop(state);
                    self.release_all_row_locks();
                    return Err(session_error("apply relational autocommit DML", error));
                }
            }
            let digest = state.current_statement_digest.clone();
            RUNTIME_TXN_INFOS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    self.row_lock_owner,
                    RuntimeTxnInfo {
                        domain_id: Arc::as_ptr(&self.domain) as usize,
                        start_ts: transaction.StartTS(),
                        current_sql_digest: digest.clone(),
                        state: "Committing".to_owned(),
                        waiting_start_time: None,
                        mem_buffer_keys: write_keys,
                        mem_buffer_bytes: write_bytes,
                        session_id: self.connection_id(),
                        database: state.current_database.clone(),
                        all_sql_digests: vec![digest],
                    },
                );
            let _txn_info_guard = RuntimeTxnInfoGuard {
                owner: self.row_lock_owner,
            };
            astersql_testkit_testfailpoint::inject("tikvclient/beforePrewrite");
            if let Some(message) = injected_commit_error {
                transaction.Rollback().map_err(|error| {
                    session_error("rollback injected relational failure", error)
                })?;
                drop(state);
                self.release_all_row_locks();
                return Err(SessionError::new(message));
            }
            let injected_tso_failures = state.next_autocommit_retry_tso_failures.take();
            state.last_autocommit_retry_attempts =
                self.run_autocommit_commit_retry(injected_tso_failures)?;
            astersql_testkit_testfailpoint::inject("tikvclient/asyncCommitDoNothing");
            let written_keys = mutation_runtime_keys
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            let related_table_ids = written_keys
                .iter()
                .map(|key| {
                    astersql_tablecodec::DecodeTableID(astersql_tablecodec::kv::Key(
                        key.key.clone(),
                    ))
                })
                .collect::<HashSet<_>>();
            let schema_check = self.transaction_schema_changed(
                &start_schema,
                &related_table_ids,
                &written_keys,
                &HashSet::new(),
            );
            if !matches!(schema_check, Ok(false)) {
                let rollback = transaction.Rollback();
                drop(state);
                self.release_all_row_locks();
                rollback
                    .map_err(|error| session_error("rollback changed autocommit schema", error))?;
                schema_check?;
                return Err(SessionError::new(
                    "[domain:8028]Information schema is changed during the execution of the statement [try again later]",
                ));
            }
            // This is deliberately immediately before Commit rather than before
            // statement execution.  An administrator can enable read-only mode
            // while a long autocommit statement is already running; Go rejects
            // that write at its commit boundary and rolls its temporary txn back.
            if runtime_read_only_mode_enabled()
                && !self.can_bypass_restricted_read_only(state.in_restricted_sql)
            {
                transaction.Rollback().map_err(|error| {
                    session_error("rollback read-only autocommit transaction", error)
                })?;
                drop(state);
                self.release_all_row_locks();
                return Err(runtime_read_only_mode_error());
            }
            let commit_started = std::time::Instant::now();
            if let Err(error) = transaction.Commit(&kv::Context::default()) {
                drop(state);
                self.release_all_row_locks();
                return Err(session_error("commit relational autocommit DML", error));
            }
            let store_commit_ts = transaction.CommitTS();
            state.last_observed_store_ts = store_commit_ts;
            state.tso_catalog_versions.insert(
                store_commit_ts,
                self.domain.stats_context().catalog_version(),
            );
            state
                .tso_wall_times
                .push((SystemTime::now(), store_commit_ts));
            let epoch = NEXT_RUNTIME_COMMIT_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
            let mut epochs = RUNTIME_KEY_COMMIT_EPOCHS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for key in &mutation_runtime_keys {
                epochs.insert(key.clone(), epoch);
            }
            state.last_commit_ts = astersql_testkit_testfailpoint::eval_string(
                "github.com/pingcap/tidb/pkg/session/mockFutureCommitTS",
            )
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(store_commit_ts);
            commit_wait = commit_started.elapsed();
            committed = true;
        }
        state.last_dml_report = Some(crate::dml_runtime::DmlExecutionReport {
            Operator: operator.to_owned(),
            Table: table.to_owned(),
            AffectedRows: affected_rows,
            LastInsertID: last_insert_id,
            WriteKeys: write_keys,
            PrewriteKeys: write_keys,
            Committed: committed,
            CommitWait: commit_wait,
            AllocCount: alloc_count,
            RebaseCount: rebase_count,
            InsertTotalTime: std::time::Duration::ZERO,
            InsertPrepareTime: std::time::Duration::ZERO,
            CheckInsertTime: std::time::Duration::ZERO,
            InsertPrefetchTime: std::time::Duration::ZERO,
            ForeignKeyCheckTime: std::time::Duration::ZERO,
            HasInsertRuntimeStats: false,
            HasForeignKeyChecks: false,
        });
        drop(state);
        for (_, (key, delta)) in mlog_stats {
            if delta != 0 {
                self.record_stats_delta(key, delta, delta.abs())?;
            }
        }
        if !explicit_transaction {
            self.release_all_row_locks();
        }
        Ok(())
    }

    /// 计算行对应的物理表 ID（含分区）。
    pub(super) fn row_physical_id(
        table: &astersql_meta_model::TableInfo,
        row: &HashMap<String, Option<String>>,
    ) -> i64 {
        astersql_table_tables::canonical_partition::CanonicalPartitionedTable::new(
            table,
            astersql_tablecodec::collate::NewCollationEnabled(),
        )
        .locate(row, partition_expression_value)
    }

    fn unmatched_range_partition_value(
        table: &astersql_meta_model::TableInfo,
        row: &HashMap<String, Option<String>>,
    ) -> Option<i64> {
        let partition = table.GetPartitionInfo()?;
        if partition.Type != astersql_meta_model::ast::model::PartitionTypeRange {
            return None;
        }
        let expression = if partition.Expr.is_empty() {
            partition.Columns.first()?.L.clone()
        } else {
            partition.Expr.replace('`', "").to_lowercase()
        };
        let value = partition_expression_value(&expression, row)?;
        let router = astersql_table_tables::canonical_partition::CanonicalPartitionedTable::new(
            table,
            astersql_tablecodec::collate::NewCollationEnabled(),
        );
        let matched = partition.Definitions.iter().any(|definition| {
            if partition.Columns.is_empty() {
                definition.LessThan.first().is_none_or(|upper| {
                    upper.eq_ignore_ascii_case("maxvalue")
                        || value < upper.parse::<i64>().unwrap_or(i64::MAX)
                })
            } else {
                router.range_columns_row_is_below(row, &partition.Columns, &definition.LessThan)
            }
        });
        (!matched).then_some(value)
    }

    fn unmatched_list_partition_warning(
        table: &astersql_meta_model::TableInfo,
        row: &HashMap<String, Option<String>>,
    ) -> Option<String> {
        let partition = table.GetPartitionInfo()?;
        if partition.Type != astersql_meta_model::ast::model::PartitionTypeList {
            return None;
        }
        let router = astersql_table_tables::canonical_partition::CanonicalPartitionedTable::new(
            table,
            astersql_tablecodec::collate::NewCollationEnabled(),
        );
        if !partition.Columns.is_empty() {
            let matched = partition.Definitions.iter().any(|definition| {
                definition.InValues.iter().any(|values| {
                    values.len() == partition.Columns.len()
                        && partition
                            .Columns
                            .iter()
                            .zip(values)
                            .all(|(column, configured)| {
                                router.list_value_matches(
                                    row.get(&column.L)
                                        .and_then(Option::as_ref)
                                        .map(String::as_str),
                                    configured,
                                    table
                                        .Columns
                                        .iter()
                                        .find(|info| info.Name.L == column.L)
                                        .map(|info| info.FieldType.GetCollate()),
                                )
                            })
                })
            });
            return (!matched)
                .then(|| "Table has no partition for value from column_list".to_owned());
        }
        let expression = partition.Expr.replace('`', "").to_lowercase();
        let value = partition_expression_value(&expression, row)?;
        let value_text = value.to_string();
        let matched = partition.Definitions.iter().any(|definition| {
            definition.InValues.iter().any(|values| {
                values.len() == 1 && router.list_value_matches(Some(&value_text), &values[0], None)
            })
        });
        (!matched).then(|| format!("Table has no partition for value {value}"))
    }

    /// 记录关系变更引起的统计增量。
    fn record_relational_stats(
        &self,
        table_name: &str,
        table: &astersql_meta_model::TableInfo,
        before: &[HashMap<String, Option<String>>],
        after: &[HashMap<String, Option<String>>],
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        let Some((key, _)) = self.mdl_stats_table(&current_database, table_name) else {
            return Ok(());
        };
        let flags = self.dml_type_flags();
        let encode_rows = |rows: &[HashMap<String, Option<String>>]| {
            rows.iter()
                .map(|row| {
                    let (storage_key, encoded) = encode_relational_row(table, row, flags)?;
                    Ok((storage_key.0, (row.clone(), encoded)))
                })
                .collect::<SessionResult<BTreeMap<_, _>>>()
        };
        let before = encode_rows(before)?;
        let after = encode_rows(after)?;
        let mut deltas = BTreeMap::<i64, (i64, i64)>::new();
        for (storage_key, (old_row, old_value)) in &before {
            match after.get(storage_key) {
                None => {
                    let delta = deltas
                        .entry(Self::row_physical_id(table, old_row))
                        .or_default();
                    delta.0 = delta.0.saturating_sub(1);
                    delta.1 = delta.1.saturating_add(1);
                }
                Some((new_row, new_value)) if new_value != old_value => {
                    let old_id = Self::row_physical_id(table, old_row);
                    let new_id = Self::row_physical_id(table, new_row);
                    if old_id == new_id {
                        deltas.entry(new_id).or_default().1 += 1;
                    } else {
                        let old_delta = deltas.entry(old_id).or_default();
                        old_delta.0 -= 1;
                        old_delta.1 += 1;
                        let new_delta = deltas.entry(new_id).or_default();
                        new_delta.0 += 1;
                        new_delta.1 += 1;
                    }
                }
                _ => {}
            }
        }
        for (storage_key, (new_row, _)) in &after {
            if !before.contains_key(storage_key) {
                let delta = deltas
                    .entry(Self::row_physical_id(table, new_row))
                    .or_default();
                delta.0 = delta.0.saturating_add(1);
                delta.1 = delta.1.saturating_add(1);
            }
        }
        for (physical_id, (row_delta, modified_rows)) in deltas {
            let mut physical_key = key.clone();
            physical_key.table_id = physical_id;
            self.record_stats_delta(physical_key, row_delta, modified_rows)?;
        }
        Ok(())
    }

    /// 累计统计 delta。
    fn record_stats_delta(
        &self,
        key: astersql_statistics_handle::StatsTableKey,
        row_delta: i64,
        modified_rows: i64,
    ) -> SessionResult<()> {
        let mut state = self.state.borrow_mut();
        if state.transaction.is_some() {
            let entry = state
                .pending_stats_deltas
                .entry(key.table_id)
                .or_insert((key, 0, 0));
            entry.1 = entry.1.saturating_add(row_delta);
            entry.2 = entry.2.saturating_add(modified_rows);
            return Ok(());
        }
        drop(state);
        self.domain
            .record_stats_mutation(&key, row_delta, modified_rows)
            .map_err(|error| session_error("record committed statistics delta", error))
    }

    /// 刷出挂起的统计增量到 Domain。
    pub(super) fn flush_pending_stats_deltas(&self) {
        let deltas = std::mem::take(&mut self.state.borrow_mut().pending_stats_deltas);
        self.domain.enqueue_stats_mutations(
            deltas
                .into_values()
                .map(|(key, row_delta, modified_rows)| (key.table_id, row_delta, modified_rows)),
        );
    }

    /// 求值生成列。
    fn evaluate_generated_columns(
        table: &astersql_meta_model::TableInfo,
        row: &mut HashMap<String, Option<String>>,
    ) -> SessionResult<()> {
        for column in table.Columns.iter().filter(|column| column.IsGenerated()) {
            if column.Hidden
                && (column
                    .GeneratedExprString
                    .to_ascii_lowercase()
                    .contains("vec_cosine_distance")
                    || column
                        .GeneratedExprString
                        .to_ascii_lowercase()
                        .contains("vec_l2_distance"))
            {
                row.insert(column.Name.L.clone(), None);
                continue;
            }
            let expression = crate::dml_runtime::ParseGeneratedExpr(&column.GeneratedExprString)?;
            let value = match &expression.Kind {
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L == "lower" && Args.len() == 1 =>
                {
                    crate::dml_runtime::EvalExpr(&Args[0], row, None)?
                        .map(|value| value.to_lowercase())
                }
                _ => crate::dml_runtime::EvalExpr(&expression, row, None)?,
            };
            row.insert(column.Name.L.clone(), value);
        }
        Ok(())
    }

    fn validate_unique_indexes<'a>(
        table: &astersql_meta_model::TableInfo,
        rows: impl IntoIterator<Item = &'a HashMap<String, Option<String>>>,
        flags: astersql_types::Flags,
    ) -> SessionResult<()> {
        let rows = rows.into_iter().collect::<Vec<_>>();
        for index in table
            .Indices
            .iter()
            .filter(|index| index.Unique && index.State == astersql_meta_model::StatePublic)
        {
            let mut seen = HashSet::new();
            for row in &rows {
                if !index.ConditionExprString.is_empty() {
                    let condition =
                        crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)?;
                    if !row_matches_simple_where(row, &condition) {
                        continue;
                    }
                }
                for values in relational_index_value_rows(table, index, row, flags)? {
                    if values.iter().any(astersql_types::datum::Datum::IsNull) {
                        continue;
                    }
                    let encoded = astersql_tablecodec::codec::EncodeKey(
                        astersql_tablecodec::time::UTC,
                        Vec::new(),
                        values.clone(),
                    )
                    .map_err(|error| session_error("encode unique index values", error))?;
                    if !seen.insert(encoded) {
                        let value = values
                            .iter()
                            .map(|value| value.ToString().unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join("-");
                        return Err(SessionError::new(format!(
                            "[kv:1062]Duplicate entry '{value}' for key '{}.{}'",
                            table.Name.O, index.Name.O
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Find an existing physical row that conflicts with an incoming unique key.
    fn relational_unique_conflict(
        table: &astersql_meta_model::TableInfo,
        rows: &HashMap<Vec<u8>, HashMap<String, Option<String>>>,
        incoming_row: &HashMap<String, Option<String>>,
        flags: astersql_types::Flags,
        exclude_key: Option<&[u8]>,
    ) -> SessionResult<Option<(Vec<u8>, String)>> {
        for index in table
            .Indices
            .iter()
            .filter(|index| index.Unique && index.State == astersql_meta_model::StatePublic)
        {
            if !index.ConditionExprString.is_empty() {
                let condition = crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)?;
                if !row_matches_simple_where(incoming_row, &condition) {
                    continue;
                }
            }
            let incoming = relational_index_value_rows(table, index, incoming_row, flags)?
                .into_iter()
                .filter(|values| !values.iter().any(astersql_types::datum::Datum::IsNull))
                .map(|values| {
                    let encoded = astersql_tablecodec::codec::EncodeKey(
                        astersql_tablecodec::time::UTC,
                        Vec::new(),
                        values.clone(),
                    )
                    .map_err(|error| session_error("encode incoming unique index values", error))?;
                    Ok((encoded, values))
                })
                .collect::<SessionResult<Vec<_>>>()?;
            for (key, existing_row) in rows {
                if exclude_key.is_some_and(|excluded| key.as_slice() == excluded) {
                    continue;
                }
                if !index.ConditionExprString.is_empty() {
                    let condition =
                        crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)?;
                    if !row_matches_simple_where(existing_row, &condition) {
                        continue;
                    }
                }
                for existing in relational_index_value_rows(table, index, existing_row, flags)? {
                    if existing.iter().any(astersql_types::datum::Datum::IsNull) {
                        continue;
                    }
                    let encoded = astersql_tablecodec::codec::EncodeKey(
                        astersql_tablecodec::time::UTC,
                        Vec::new(),
                        existing,
                    )
                    .map_err(|error| session_error("encode existing unique index values", error))?;
                    if let Some((_, values)) =
                        incoming.iter().find(|(incoming, _)| incoming == &encoded)
                    {
                        let value = values
                            .iter()
                            .map(|value| value.ToString().unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join("-");
                        return Ok(Some((
                            key.clone(),
                            format!(
                                "[kv:1062]Duplicate entry '{value}' for key '{}.{}'",
                                table.Name.O, index.Name.O
                            ),
                        )));
                    }
                }
            }
        }
        Ok(None)
    }

    /// TiDB's optimistic transaction defers uniqueness checks against rows in
    /// its start snapshot until prewrite. Conflicts created only by this
    /// transaction remain statement errors, so multi-row INSERT atomicity is
    /// unchanged.
    fn defer_optimistic_insert_constraint(
        &self,
        table: &astersql_meta_model::TableInfo,
        conflict_key: &[u8],
        message: &str,
    ) -> SessionResult<bool> {
        let snapshot_contains_conflict = {
            let state = self.state.borrow();
            let Some(transaction) = state.transaction.as_ref() else {
                return Ok(false);
            };
            if state.transaction_pessimistic
                || state.constraint_check_in_place
                || table.TempTableType != astersql_meta_model::TempTableNone
            {
                return Ok(false);
            }
            match transaction.GetSnapshot().Get(
                &kv::Context::default(),
                kv::Key(conflict_key.to_vec()),
                &[],
            ) {
                Ok(_) => true,
                Err(error) if kv::IsErrNotFound(&error) => false,
                Err(error) => {
                    return Err(session_error("read optimistic constraint snapshot", error));
                }
            }
        };
        if snapshot_contains_conflict {
            self.state
                .borrow_mut()
                .deferred_optimistic_constraint_errors
                .entry(conflict_key.to_vec())
                .or_insert_with(|| message.to_owned());
        }
        Ok(snapshot_contains_conflict)
    }

    /// Probe encoded secondary-unique index KVs for a narrowed UPDATE.
    /// Entries already owned by the current row are allowed; every other
    /// existing distinct key is a duplicate.
    fn relational_unique_kv_conflict(
        &self,
        table: &astersql_meta_model::TableInfo,
        original_row: &HashMap<String, Option<String>>,
        incoming_row: &HashMap<String, Option<String>>,
        flags: astersql_types::Flags,
    ) -> SessionResult<Option<String>> {
        let original_keys = encode_relational_unique_index_entries(table, original_row, flags)?
            .into_iter()
            .map(|(key, _, _)| key.0)
            .collect::<HashSet<_>>();
        for (key, values, index_name) in
            encode_relational_unique_index_entries(table, incoming_row, flags)?
        {
            if original_keys.contains(&key.0) {
                continue;
            }
            if self.read_raw_kv(key)?.is_some() {
                let value = values
                    .iter()
                    .map(|value| value.ToString().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("-");
                return Ok(Some(format!(
                    "[kv:1062]Duplicate entry '{value}' for key '{}.{index_name}'",
                    table.Name.O
                )));
            }
        }
        Ok(None)
    }

    fn cascade_foreign_key_updates(
        &self,
        parent: &astersql_meta_model::TableInfo,
        before: &[HashMap<String, Option<String>>],
        after: &[HashMap<String, Option<String>>],
    ) -> SessionResult<()> {
        self.cascade_foreign_key_updates_at_depth(parent, before, after, 0)
    }

    fn cascade_foreign_key_updates_at_depth(
        &self,
        parent: &astersql_meta_model::TableInfo,
        before: &[HashMap<String, Option<String>>],
        after: &[HashMap<String, Option<String>>],
        depth: usize,
    ) -> SessionResult<()> {
        if !self.state.borrow().foreign_key_checks {
            return Ok(());
        }
        let current_database = self.current_database();
        let children = self
            .domain
            .stats_context()
            .catalog()
            .into_iter()
            .filter(|((database, _), (_, table))| {
                table.ForeignKeys.iter().any(|foreign_key| {
                    foreign_key.OnUpdate == 2
                        && foreign_key.RefTable.L == parent.Name.L
                        && (foreign_key.RefSchema.L.is_empty()
                            || foreign_key.RefSchema.L == *database
                            || foreign_key.RefSchema.L == current_database)
                })
            })
            .map(|((database, name), (_, table))| (database, name, table))
            .collect::<Vec<_>>();
        let flags = self.dml_type_flags();
        for (database, child_name, child) in children {
            let mlog = RuntimeMLog::for_table(self, &database, &child, flags)?;
            let mut child_rows = self
                .scan_registered_table(&child)?
                .into_iter()
                .map(|(_, row)| row)
                .collect::<Vec<_>>();
            let original = child_rows.clone();
            for foreign_key in child.ForeignKeys.iter().filter(|foreign_key| {
                foreign_key.OnUpdate == 2
                    && foreign_key.RefTable.L == parent.Name.L
                    && (foreign_key.RefSchema.L.is_empty()
                        || foreign_key.RefSchema.L == database
                        || foreign_key.RefSchema.L == current_database)
            }) {
                let mut updated_parent_values = HashMap::new();
                for (old_parent, new_parent) in before.iter().zip(after) {
                    let old_values = foreign_key
                        .RefCols
                        .iter()
                        .map(|column| old_parent.get(&column.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>();
                    let new_values = foreign_key
                        .RefCols
                        .iter()
                        .map(|column| new_parent.get(&column.L).cloned().flatten())
                        .collect::<Vec<_>>();
                    let Some(old_values) = old_values else {
                        continue;
                    };
                    if new_values.iter().all(Option::is_some)
                        && new_values
                            .iter()
                            .filter_map(Clone::clone)
                            .eq(old_values.iter().cloned())
                    {
                        continue;
                    }
                    // Multiple parent rows may expose the same non-unique old
                    // key. TiDB cascades using the first changed parent row;
                    // once the child key changes it no longer matches later
                    // entries. `or_insert` preserves that ordering contract.
                    updated_parent_values
                        .entry(old_values)
                        .or_insert(new_values);
                }
                for child_row in &mut child_rows {
                    let child_values = foreign_key
                        .Cols
                        .iter()
                        .map(|column| child_row.get(&column.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>();
                    if let Some(new_values) = child_values
                        .as_ref()
                        .and_then(|values| updated_parent_values.get(values))
                    {
                        for (column, value) in foreign_key.Cols.iter().zip(new_values) {
                            child_row.insert(column.L.clone(), value.clone());
                        }
                    }
                }
            }
            let mut mutations = Vec::new();
            let mut changed_before = Vec::new();
            let mut changed_after = Vec::new();
            for (old, new) in original.iter().zip(&child_rows) {
                if old == new {
                    continue;
                }
                changed_before.push(old.clone());
                changed_after.push(new.clone());
                let (old_key, _) = encode_relational_row(&child, old, flags)?;
                let (new_key, new_value) =
                    self.encode_relational_row_for_write(&child, new, flags)?;
                if old_key != new_key {
                    mutations.push((old_key, None));
                }
                mutations.push((new_key, Some(new_value)));
                if let Some(mlog) = &mlog
                    && mlog.tracked_changed(&child, old, new)
                {
                    mlog.append(
                        &child,
                        old,
                        astersql_table::mview_log::MLogDMLType::Update,
                        -1,
                        &mut mutations,
                    )?;
                    mlog.append(
                        &child,
                        new,
                        astersql_table::mview_log::MLogDMLType::Update,
                        1,
                        &mut mutations,
                    )?;
                }
            }
            if !mutations.is_empty() {
                if depth >= 15 {
                    return Err(SessionError::new("foreign-key cascade depth exceeded"));
                }
                Self::validate_unique_indexes(&child, child_rows.iter(), flags)?;
                // Cascading an update into the child is itself a parent-key
                // update for any tables that reference that child. Validate
                // their RESTRICT/NO ACTION edges before staging mutations so
                // the whole statement rolls back atomically on failure.
                self.validate_restrict_foreign_key_updates(&child, &original, &child_rows)?;
                self.cascade_foreign_key_updates_at_depth(
                    &child,
                    &changed_before,
                    &changed_after,
                    depth + 1,
                )?;
                let affected = mutations.len() as u64;
                self.apply_relational_mutations(
                    &child_name,
                    "ForeignKeyCascadeUpdate",
                    mutations,
                    unique_lock_keys_for_rows(&child, child_rows.iter()),
                    affected,
                    0,
                    0,
                    0,
                )?;
            }
        }
        Ok(())
    }

    fn validate_restrict_foreign_key_updates(
        &self,
        parent: &astersql_meta_model::TableInfo,
        before: &[HashMap<String, Option<String>>],
        after: &[HashMap<String, Option<String>>],
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        for ((database, child_name), (_, child)) in self.domain.stats_context().catalog() {
            for foreign_key in child.ForeignKeys.iter().filter(|foreign_key| {
                foreign_key.OnUpdate != 2
                    && foreign_key.RefTable.L == parent.Name.L
                    && (foreign_key.RefSchema.L.is_empty()
                        || foreign_key.RefSchema.L == database
                        || foreign_key.RefSchema.L == current_database)
            }) {
                let child_rows = self.scan_latest_with_transaction_overlay(&child)?;
                for (old_parent, new_parent) in before.iter().zip(after) {
                    let old_values = foreign_key
                        .RefCols
                        .iter()
                        .map(|column| old_parent.get(&column.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>();
                    let new_values = foreign_key
                        .RefCols
                        .iter()
                        .map(|column| new_parent.get(&column.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>();
                    let Some(old_values) = old_values else {
                        continue;
                    };
                    if new_values.as_ref() == Some(&old_values) {
                        continue;
                    }
                    let referenced = child_rows.iter().any(|(_, row)| {
                        foreign_key
                            .Cols
                            .iter()
                            .map(|column| row.get(&column.L).cloned().flatten())
                            .collect::<Option<Vec<_>>>()
                            .as_ref()
                            == Some(&old_values)
                    });
                    if referenced {
                        return Err(SessionError::new(format!(
                            "[kv:1451]Cannot delete or update a parent row: a foreign key \
                             constraint fails (`{database}`.`{child_name}`, CONSTRAINT `{}`)",
                            foreign_key.Name.O
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn cascade_foreign_key_deletes(
        &self,
        parent: &astersql_meta_model::TableInfo,
        deleted: &[HashMap<String, Option<String>>],
    ) -> SessionResult<()> {
        if self.state.borrow().adapter_dml_statement_staged {
            self.state.borrow_mut().pending_fk_delete_cascades.push(
                RuntimeForeignKeyDeleteCascade {
                    parent: parent.clone(),
                    deleted: deleted.to_vec(),
                },
            );
            return Ok(());
        }
        self.cascade_foreign_key_deletes_at_depth(parent, deleted, 0)
            .map(|_| ())
    }

    pub(super) fn execute_pending_fk_delete_cascade(
        &self,
        pending: &RuntimeForeignKeyDeleteCascade,
    ) -> SessionResult<()> {
        self.cascade_foreign_key_deletes_at_depth(&pending.parent, &pending.deleted, 0)
            .map(|_| ())
    }

    fn cascade_foreign_key_deletes_at_depth(
        &self,
        parent: &astersql_meta_model::TableInfo,
        deleted: &[HashMap<String, Option<String>>],
        depth: usize,
    ) -> SessionResult<HashSet<Vec<u8>>> {
        if !self.state.borrow().foreign_key_checks {
            return Ok(HashSet::new());
        }
        if depth >= 15 && !deleted.is_empty() {
            return Err(SessionError::new("foreign-key cascade depth exceeded"));
        }
        let current_database = self.current_database();
        let children = self
            .domain
            .stats_context()
            .catalog()
            .into_iter()
            .map(|((database, name), (_, table))| (database, name, table))
            .collect::<Vec<_>>();
        let flags = self.dml_type_flags();
        let mut cascaded_deleted_keys = HashSet::new();
        for (database, child_name, child) in children {
            let relevant_foreign_keys = child
                .ForeignKeys
                .iter()
                .filter(|foreign_key| {
                    foreign_key.RefTable.L == parent.Name.L
                        && (foreign_key.RefSchema.L.is_empty()
                            || foreign_key.RefSchema.L == database
                            || foreign_key.RefSchema.L == current_database)
                })
                .collect::<Vec<_>>();
            if relevant_foreign_keys.is_empty() {
                continue;
            }
            let mlog = RuntimeMLog::for_table(self, &database, &child, flags)?;
            let parent_key_sets = relevant_foreign_keys
                .iter()
                .map(|foreign_key| {
                    let keys = deleted
                        .iter()
                        .filter_map(|parent_row| {
                            foreign_key
                                .RefCols
                                .iter()
                                .map(|column| parent_row.get(&column.L).cloned().flatten())
                                .collect::<Option<Vec<_>>>()
                        })
                        .collect::<HashSet<_>>();
                    (*foreign_key, keys)
                })
                .collect::<Vec<_>>();
            // Pessimistic FK checks are current reads. A parent DELETE may
            // have waited for a concurrent child INSERT to commit, so using
            // the transaction's original snapshot here would miss that row.
            let child_rows = self
                .scan_latest_with_transaction_overlay(&child)?
                .into_iter()
                .map(|(_, row)| row)
                .collect::<Vec<_>>();
            let mut deleted_children = Vec::new();
            let mut updated_children = Vec::new();
            let mut deleted_child_keys = HashSet::new();
            let mut updated_child_keys = HashSet::new();
            for row in &child_rows {
                // A self-referencing row such as `(id,pid)=(0,0)` is already
                // part of the parent's deletion set. Do not enqueue it again,
                // otherwise recursion never makes progress.
                if child.ID == parent.ID && deleted.iter().any(|deleted| deleted == row) {
                    continue;
                }
                for (foreign_key, parent_keys) in &parent_key_sets {
                    let matches_parent = foreign_key
                        .Cols
                        .iter()
                        .map(|column| row.get(&column.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>()
                        .is_some_and(|child_values| parent_keys.contains(&child_values));
                    if !matches_parent {
                        continue;
                    }
                    match foreign_key.OnDelete {
                        2 => {
                            let row_key = encode_relational_row(&child, row, flags)?.0.0;
                            if deleted_child_keys.insert(row_key) {
                                deleted_children.push(row.clone());
                            }
                        }
                        3 => {
                            let mut updated = row.clone();
                            for column in &foreign_key.Cols {
                                updated.insert(column.L.clone(), None);
                            }
                            let row_key = encode_relational_row(&child, row, flags)?.0.0;
                            if updated_child_keys.insert(row_key) {
                                updated_children.push((row.clone(), updated));
                            }
                        }
                        _ => {
                            return Err(SessionError::new(format!(
                                "[kv:1451]Cannot delete or update a parent row: a foreign key \
                                 constraint fails (`{database}`.`{child_name}`, CONSTRAINT `{}`)",
                                foreign_key.Name.O
                            )));
                        }
                    }
                }
            }
            if deleted_children.is_empty() && updated_children.is_empty() {
                continue;
            }
            let recursively_deleted =
                self.cascade_foreign_key_deletes_at_depth(&child, &deleted_children, depth + 1)?;
            cascaded_deleted_keys.extend(recursively_deleted);
            for row in &deleted_children {
                cascaded_deleted_keys.insert(encode_relational_row(&child, row, flags)?.0.0);
            }
            // If multiple foreign keys (possibly at different recursion
            // levels) affect the same row, DELETE dominates SET NULL.
            // Otherwise an outer UPDATE staged after an inner DELETE would
            // resurrect the row.
            updated_children.retain(|(old, _)| {
                encode_relational_row(&child, old, flags)
                    .is_ok_and(|(key, _)| !cascaded_deleted_keys.contains(&key.0))
            });
            if !updated_children.is_empty() {
                let before = updated_children
                    .iter()
                    .map(|(old, _)| old.clone())
                    .collect::<Vec<_>>();
                let after = updated_children
                    .iter()
                    .map(|(_, new)| new.clone())
                    .collect::<Vec<_>>();
                // SET NULL is an UPDATE of the child. Any table referencing a
                // changed child key must be checked (or cascaded) before a
                // mutation is staged, preserving statement atomicity.
                self.validate_restrict_foreign_key_updates(&child, &before, &after)?;
                self.cascade_foreign_key_updates_at_depth(&child, &before, &after, depth + 1)?;
            }
            let mut mutations = Vec::new();
            for row in &deleted_children {
                let (key, _) = encode_relational_row(&child, row, flags)?;
                mutations.push((key, None));
                mutations.extend(relational_index_mutations(&child, Some(row), None, flags)?);
                if let Some(mlog) = &mlog {
                    mlog.append(
                        &child,
                        row,
                        astersql_table::mview_log::MLogDMLType::Delete,
                        -1,
                        &mut mutations,
                    )?;
                }
            }
            for (old, new) in &updated_children {
                let (old_key, _) = encode_relational_row(&child, old, flags)?;
                let (new_key, new_value) =
                    self.encode_relational_row_for_write(&child, new, flags)?;
                if old_key != new_key {
                    mutations.push((old_key, None));
                }
                mutations.extend(relational_index_mutations(
                    &child,
                    Some(old),
                    Some(new),
                    flags,
                )?);
                mutations.push((new_key, Some(new_value)));
                if let Some(mlog) = &mlog
                    && mlog.tracked_changed(&child, old, new)
                {
                    mlog.append(
                        &child,
                        old,
                        astersql_table::mview_log::MLogDMLType::Update,
                        -1,
                        &mut mutations,
                    )?;
                    mlog.append(
                        &child,
                        new,
                        astersql_table::mview_log::MLogDMLType::Update,
                        1,
                        &mut mutations,
                    )?;
                }
            }
            if !mutations.is_empty() {
                let lock_rows = deleted_children
                    .iter()
                    .chain(updated_children.iter().map(|(old, _)| old));
                self.apply_relational_mutations(
                    &child_name,
                    "ForeignKeyCascadeDelete",
                    mutations,
                    unique_lock_keys_for_rows(&child, lock_rows),
                    (deleted_children.len() + updated_children.len()) as u64,
                    0,
                    0,
                    0,
                )?;
            }
        }
        Ok(cascaded_deleted_keys)
    }

    /// 普通主键 INSERT 可逐键检查冲突，无需先把整张表加载到内存。
    pub(crate) fn relational_insert_requires_existing_rows(
        replace: bool,
        ignore: bool,
        has_on_duplicate: bool,
        has_secondary_unique_index: bool,
    ) -> bool {
        replace || ignore || has_on_duplicate || has_secondary_unique_index
    }

    pub(crate) fn bulk_load_skips_committed_primary_key_check(
        configured_table: Option<&str>,
        database: &str,
        table: &str,
        requires_existing_rows: bool,
    ) -> bool {
        if requires_existing_rows {
            return false;
        }
        configured_table
            .and_then(|configured| configured.split_once('.'))
            .is_some_and(|(configured_database, configured_table)| {
                configured_database.eq_ignore_ascii_case(database)
                    && configured_table.eq_ignore_ascii_case(table)
            })
    }

    /// 执行关系型 INSERT 计划。
    pub(super) fn execute_relational_insert(
        &self,
        statement: &ast::InsertStmt,
        plan: crate::dml_runtime::InsertPlan,
        from_select: bool,
        select_columns: Option<&[String]>,
    ) -> SessionResult<()> {
        self.execute_relational_insert_with_load_counts(
            statement,
            plan,
            from_select,
            select_columns,
        )
        .map(|_| ())
    }

    /// Return rows actually copied/deleted, excluding unchanged REPLACE rows.
    pub(super) fn execute_relational_insert_with_load_counts(
        &self,
        statement: &ast::InsertStmt,
        plan: crate::dml_runtime::InsertPlan,
        from_select: bool,
        select_columns: Option<&[String]>,
    ) -> SessionResult<(u64, u64)> {
        let insert_started = std::time::Instant::now();
        // Go's ResetContextOfStmt turns truncation into warnings for INSERT
        // IGNORE even under a strict sql_mode, preserving a valid numeric
        // prefix such as `1` from `1a`.
        let flags = self.dml_type_flags();
        let flags = flags.WithTruncateAsWarning(flags.TruncateAsWarning() || plan.Ignore);
        let client_found_rows = self.state.borrow().client_capability
            & astersql_parser_mysql::r#const::ClientFoundRows
            != 0;
        let sql_mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .unwrap_or(astersql_parser_mysql::r#const::SQLMode(0));
        let strict_sql_mode = sql_mode.HasStrictMode();
        let no_auto_value_on_zero =
            sql_mode.0 & astersql_parser_mysql::r#const::ModeNoAutoValueOnZero.0 != 0;
        let current_database = self.current_database();
        let database = statement
            .Table
            .as_ref()
            .and_then(|table| table.TableRefs.Left.as_deref())
            .and_then(|source| match source {
                ast::ResultSetNode::TableSource(source) => Some(source.Source.Schema.L.as_str()),
                _ => None,
            })
            .filter(|database| !database.is_empty())
            .unwrap_or(current_database.as_str());
        let mut table = self
            .resolve_runtime_table(database, &plan.Table)
            .ok_or_else(|| SessionError::new(format!("unknown DML table {}", plan.Table)))?;
        let mlog = RuntimeMLog::for_table(self, database, &table, flags)?;
        let has_foreign_key_checks = !table.ForeignKeys.is_empty();
        self.record_transaction_table_write(&table);
        if let Some(indexes) = RUNTIME_PENDING_WRITE_INDEXES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(
                runtime_domain_id(&self.domain),
                database.to_ascii_lowercase(),
                plan.Table.to_ascii_lowercase(),
            ))
            .cloned()
        {
            for index in indexes {
                if !table
                    .Indices
                    .iter()
                    .any(|existing| existing.Name.L == index.Name.L)
                {
                    table.Indices.push(index);
                }
            }
        }
        let input_columns = if statement.Columns.is_empty() {
            let all_columns = table
                .Columns
                .iter()
                .map(|column| column.Name.L.clone())
                .collect::<Vec<_>>();
            if statement
                .Lists
                .first()
                .is_some_and(|expressions| expressions.len() == all_columns.len())
            {
                // MySQL accepts explicit DEFAULT placeholders for generated
                // columns in a full-width VALUES tuple. Generated values are
                // recalculated below before the row is encoded.
                all_columns
            } else {
                table
                    .Columns
                    .iter()
                    .filter(|column| !column.Hidden && !column.IsGenerated())
                    .map(|column| column.Name.L.clone())
                    .collect::<Vec<_>>()
            }
        } else {
            statement
                .Columns
                .iter()
                .map(|column| column.Name.L.clone())
                .collect()
        };
        let mut mutations = Vec::new();
        let records = statement.Lists.len() as u64;
        let mut affected_rows = 0;
        let mut copied_rows = 0;
        let mut deleted_rows = 0;
        let mut touched_rows = 0;
        let mut updated_record_rows = 0;
        let mut has_deferred_optimistic_constraint = false;
        let mut last_insert_id = 0;
        let mut alloc_count = 0;
        let mut rebase_count = 0;
        let mut lock_rows = Vec::new();
        let mut unchanged_lock_rows = Vec::new();
        let has_secondary_unique_index = table.Indices.iter().any(|index| {
            index.Unique
                // A nonclustered PRIMARY KEY is a separate unique index; its
                // row handle is newly allocated and cannot detect duplicates.
                && ((!index.Primary && !index.Name.L.eq_ignore_ascii_case("primary"))
                    || (!table.PKIsHandle && !table.IsCommonHandle))
                && index.State == astersql_meta_model::StatePublic
        });
        let requires_existing_rows = Self::relational_insert_requires_existing_rows(
            plan.Replace,
            plan.Ignore,
            !plan.OnDuplicate.is_empty(),
            has_secondary_unique_index,
        );
        let skip_committed_primary_key_check = Self::bulk_load_skips_committed_primary_key_check(
            std::env::var("ASTERSQL_BULK_LOAD_ASSUME_ABSENT_TABLE")
                .ok()
                .as_deref(),
            database,
            &plan.Table,
            requires_existing_rows,
        );
        let pessimistic_transaction = {
            let state = self.state.borrow();
            state.transaction.is_some()
                && state.transaction_pessimistic
                && state.constraint_check_in_place_pessimistic
        };
        // Go performs autocommit INSERT conflict reads and writes in the same
        // transaction. Reusing one transaction keeps every VALUES tuple on one
        // statement snapshot and avoids an extra PD timestamp before commit.
        let mut statement_transaction =
            if !pessimistic_transaction && self.state.borrow().transaction.is_none() {
                Some(
                    self.domain
                        .storage()
                        .with_storage(|store| store.Begin(&[]))
                        .map_err(|error| session_error("begin INSERT transaction", error))?,
                )
            } else {
                None
            };
        let check_insert_started = std::time::Instant::now();
        let prefetch_started = std::time::Instant::now();
        let mut working_rows = if requires_existing_rows {
            if pessimistic_transaction {
                self.scan_latest_with_transaction_overlay(&table)?
            } else if let Some(transaction) = statement_transaction.as_ref() {
                scan_relational_rows_with_limit(transaction.as_ref(), &table, None)?
            } else {
                self.scan_registered_table(&table)?
            }
            .into_iter()
            .map(|(_, row)| {
                let (key, _) = encode_relational_row(&table, &row, flags)?;
                Ok((key.0, row))
            })
            .collect::<SessionResult<HashMap<_, _>>>()?
        } else {
            HashMap::new()
        };
        let prefetch_time = prefetch_started.elapsed();
        let mut foreign_key_check_time = std::time::Duration::ZERO;
        let stats_before = working_rows.values().cloned().collect::<Vec<_>>();
        for (row_index, expressions) in statement.Lists.iter().enumerate() {
            let row_columns = if statement.Columns.is_empty() && expressions.is_empty() {
                &[][..]
            } else {
                input_columns.as_slice()
            };
            if expressions.len() != row_columns.len() {
                return Err(SessionError::new(
                    "relational INSERT column/value count mismatch",
                ));
            }
            let mut row = HashMap::new();
            for (column, expression) in row_columns.iter().zip(expressions) {
                let column_info = table.Columns.iter().find(|info| info.Name.L == *column);
                let mut value = if matches!(
                    &expression.Kind,
                    ast::ExprKind::DefaultValue | ast::ExprKind::NamedDefault(_)
                ) {
                    column_info.and_then(insert_default_runtime_value)
                } else if plan.Ignore
                    && column_info.is_some_and(|column| {
                        column.GetType() == astersql_parser_mysql::r#type::TypeEnum
                    })
                    && literal(expression).ok().as_deref() == Some("0")
                {
                    Some(String::new())
                } else if let Some(column_info) =
                    column_info.filter(|_| is_typed_literal(expression))
                {
                    match typed_literal_to_runtime_value_with_warning(
                        expression,
                        column_info,
                        flags,
                    ) {
                        Ok((value, conversion_was_truncated)) => {
                            if conversion_was_truncated {
                                let literal = literal(expression)?;
                                let message = if from_select {
                                    format!("Truncated incorrect DOUBLE value: '{literal}'")
                                } else {
                                    let type_name = astersql_parser_types::TypeToStr(
                                        column_info.GetType(),
                                        &column_info.GetCharset(),
                                    );
                                    format!(
                                        "Incorrect {type_name} value: '{literal}' for column '{}' at row {}",
                                        column_info.Name.O,
                                        row_index + 1
                                    )
                                };
                                self.set_warning_with_code(
                                    if from_select { 1292 } else { 1366 },
                                    message,
                                );
                            }
                            value
                        }
                        Err(_error) if plan.Ignore => {
                            let literal = literal(expression)?;
                            if let Some(value) =
                                ignored_integer_overflow_value(column_info, &literal)
                            {
                                self.set_warning_with_code(
                                    1264,
                                    format!(
                                        "Out of range value for column '{}' at row {}",
                                        column_info.Name.O,
                                        row_index + 1
                                    ),
                                );
                                row.insert(column.clone(), Some(value));
                                continue;
                            }
                            let message = if from_select {
                                format!("Truncated incorrect DOUBLE value: '{literal}'")
                            } else {
                                let type_name = astersql_parser_types::TypeToStr(
                                    column_info.GetType(),
                                    &column_info.GetCharset(),
                                );
                                format!(
                                    "Incorrect {type_name} value: '{literal}' for column '{}' at row {}",
                                    column_info.Name.O,
                                    row_index + 1
                                )
                            };
                            self.set_warning_with_code(
                                if from_select { 1292 } else { 1366 },
                                message,
                            );
                            Some("0".to_owned())
                        }
                        Err(error) => return Err(error),
                    }
                } else if matches!(expression.Kind, ast::ExprKind::Function { .. }) {
                    match relational_expression_value(expression, &row) {
                        Ok(value) => value,
                        Err(error) if plan.Ignore => {
                            self.set_warning_with_code(1292, error.to_string());
                            Some("0".to_owned())
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    match crate::dml_runtime::EvalExpr(expression, &row, None) {
                        Ok(value) => value,
                        Err(error) if plan.Ignore => {
                            self.set_warning_with_code(1292, error.to_string());
                            Some("0".to_owned())
                        }
                        Err(error) => return Err(error),
                    }
                };
                if column_info.is_some_and(|column| {
                    column.GetType() == astersql_parser_mysql::r#type::TypeJSON
                }) && let Some(runtime_value) = value.as_deref()
                {
                    let temporal_kind = match &expression.Kind {
                        ast::ExprKind::Function { FnName, .. }
                            if FnName.L.eq_ignore_ascii_case("date") =>
                        {
                            Some("DATE")
                        }
                        ast::ExprKind::Cast { Tp, .. } => match Tp.GetType() {
                            astersql_parser_mysql::r#type::TypeDate => Some("DATE"),
                            astersql_parser_mysql::r#type::TypeDatetime
                            | astersql_parser_mysql::r#type::TypeTimestamp => Some("DATETIME"),
                            astersql_parser_mysql::r#type::TypeDuration => Some("TIME"),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(kind) = temporal_kind {
                        value = Some(
                            serde_json::to_string(&format!(
                                "__ASTER_TEMPORAL_{kind}__:{runtime_value}"
                            ))
                            .map_err(|error| session_error("encode JSON temporal", error))?,
                        );
                    }
                }
                if plan.Ignore
                    && column_info.is_some_and(|column| {
                        column.GetType() == astersql_parser_mysql::r#type::TypeEnum
                    })
                    && value.as_deref() == Some("0")
                {
                    value = Some(String::new());
                }
                if value.is_none()
                    && matches!(
                        expression.Kind,
                        ast::ExprKind::DefaultValue | ast::ExprKind::NamedDefault(_)
                    )
                    && column_info.is_some_and(|column| {
                        !strict_sql_mode
                            && column.GetType()
                                == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                            && astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag())
                            && column.GetDefaultValue().is_none()
                    })
                {
                    value = Some("[]".to_owned());
                }
                if let (Some(column_info), Some(runtime_value)) = (column_info, value.as_deref())
                    && column_info.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                {
                    let vector = astersql_types::vector::ParseVectorFloat32(runtime_value)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    vector
                        .CheckDimsFitColumn(column_info.GetFlen() as i32)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    value = Some(vector.String());
                }
                if value.is_none() {
                    if let Some(column_info) =
                        table.Columns.iter().find(|info| info.Name.L == *column)
                    {
                        let auto_generated = table
                            .GetAutoIncrementColInfo()
                            .is_some_and(|auto| auto.ID == column_info.ID)
                            || (table.ContainsAutoRandomBits()
                                && table
                                    .GetPkColInfo()
                                    .is_some_and(|primary| primary.ID == column_info.ID));
                        if !auto_generated
                            && astersql_parser_mysql::r#type::HasNotNullFlag(column_info.GetFlag())
                        {
                            let message = format!("Column '{}' cannot be null", column_info.Name.O);
                            if plan.Ignore {
                                self.set_warning_with_code(1048, message);
                                value = Some(ignored_not_null_value(column_info));
                            } else {
                                return Err(SessionError::new(message));
                            }
                        }
                    }
                }
                row.insert(column.clone(), value);
            }
            for column in &table.Columns {
                if row.contains_key(&column.Name.L)
                    || column.IsGenerated()
                    || table
                        .GetAutoIncrementColInfo()
                        .is_some_and(|auto| auto.ID == column.ID)
                    || (table.ContainsAutoRandomBits()
                        && table.GetPkColInfo().is_some_and(|pk| pk.ID == column.ID))
                {
                    continue;
                }
                let mut value = if strict_sql_mode
                    && column.GetDefaultValue().is_none()
                    && !column.DefaultIsExpr
                {
                    None
                } else {
                    insert_default_runtime_value(column)
                }
                .or_else(|| {
                    (!strict_sql_mode
                        && column.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                        && astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag())
                        && column.GetDefaultValue().is_none())
                    .then(|| "[]".to_owned())
                });
                if value.is_none()
                    && !strict_sql_mode
                    && astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag())
                {
                    value = Some("0".to_owned());
                }
                if let Some(runtime_value) = value.as_deref()
                    && column.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                {
                    let vector = astersql_types::vector::ParseVectorFloat32(runtime_value)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    vector
                        .CheckDimsFitColumn(column.GetFlen() as i32)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    value = Some(vector.String());
                }
                if value.is_none()
                    && astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag())
                {
                    let message = format!("Field '{}' doesn't have a default value", column.Name.O);
                    if plan.Ignore {
                        self.set_warning_with_code(1364, message);
                        value = Some("0".to_owned());
                    } else {
                        return Err(SessionError::new(message));
                    }
                }
                row.insert(column.Name.L.clone(), value);
            }
            let mut candidate_insert_id = None;
            let auto_increment_column = table.GetAutoIncrementColInfo();
            let auto_random_column = table
                .ContainsAutoRandomBits()
                .then(|| table.GetPkColInfo())
                .flatten();
            let mut auto_columns = Vec::new();
            if let Some(column) = auto_increment_column {
                auto_columns.push((column, false));
            }
            if let Some(column) = auto_random_column
                && auto_increment_column.is_none_or(|auto| auto.ID != column.ID)
            {
                auto_columns.push((column, true));
            }
            for (auto_column, is_auto_random) in auto_columns {
                let explicit_value = row
                    .get(&auto_column.Name.L)
                    .and_then(Option::as_ref)
                    .cloned();
                if is_auto_random
                    && explicit_value.is_some()
                    && !self.state.borrow().allow_auto_random_explicit_insert
                {
                    return Err(SessionError::new(
                        "[ddl:8216]Invalid auto random: Explicit insertion on auto_random column \
                         is disabled. Try to set @@allow_auto_random_explicit_insert = true.",
                    ));
                }
                let explicit_is_negative = explicit_value.as_deref().is_some_and(|value| {
                    value
                        .parse::<i128>()
                        .is_ok_and(|parsed| parsed.is_negative())
                });
                let retry_id = if !is_auto_random && explicit_value.is_none() {
                    self.state.borrow_mut().retry_auto_increment_ids.pop_front()
                } else {
                    None
                };
                if let Some(retry_id) = retry_id {
                    row.insert(auto_column.Name.L.clone(), Some(retry_id.to_string()));
                } else if !explicit_is_negative {
                    if !is_auto_random
                        && explicit_value.is_none()
                        && table.AutoIDCache == 1
                        && astersql_testkit_testfailpoint::eval_bool(
                            "github.com/pingcap/tidb/pkg/autoid_service/mockErr",
                        )
                    {
                        return Err(SessionError::new("auto increment action failed"));
                    }
                    let explicit = explicit_value
                        .as_deref()
                        .filter(|value| *value != "0" || no_auto_value_on_zero)
                        .map(|value| -> SessionResult<u64> {
                            if let Ok(integer) = value.parse::<u64>() {
                                return Ok(integer);
                            }
                            let numeric = value
                                .parse::<f64>()
                                .map_err(|error| session_error("parse explicit auto ID", error))?;
                            if !numeric.is_finite() || numeric.is_sign_negative() {
                                return Err(SessionError::new("parse explicit auto ID"));
                            }
                            Ok(numeric.round() as u64)
                        })
                        .transpose()?;
                    let (increment, offset) = {
                        let state = self.state.borrow();
                        (state.auto_increment_increment, state.auto_increment_offset)
                    };
                    let (mut allocated, rebased) = self.allocate_runtime_auto_id(
                        table.ID,
                        explicit,
                        if is_auto_random { 1 } else { 0 },
                        if is_auto_random { 1 } else { increment },
                        if is_auto_random { 1 } else { offset },
                    )?;
                    if explicit.is_some() {
                        if rebased {
                            rebase_count += 1;
                        }
                    } else {
                        alloc_count += 1;
                    }
                    if is_auto_random && explicit.is_none() {
                        let available_bits = if table.IsAutoRandomBitColUnsigned() {
                            64_u32
                        } else {
                            63_u32
                        };
                        let incremental_bits =
                            available_bits.saturating_sub(table.AutoRandomBits as u32);
                        let mask = if incremental_bits == 64 {
                            u64::MAX
                        } else {
                            (1_u64 << incremental_bits) - 1
                        };
                        if allocated > mask {
                            return Err(SessionError::new(
                                "[autoid:8211]Failed to read auto-random value from storage engine",
                            ));
                        }
                        let shard_bits = table.AutoRandomBits.min(16) as u32;
                        let shard = allocated
                            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                            .rotate_left(17)
                            >> (64 - shard_bits);
                        allocated = (shard << incremental_bits) | (allocated & mask);
                    }
                    // LAST_INSERT_ID() changes only when the server allocated the
                    // value; explicitly supplied AUTO_INCREMENT values merely
                    // rebase the allocator.
                    if !is_auto_random && explicit.is_none() {
                        candidate_insert_id = Some(allocated);
                    }
                    if explicit.is_none() {
                        row.insert(auto_column.Name.L.clone(), Some(allocated.to_string()));
                    }
                }
            }
            if !table.PKIsHandle && !table.IsCommonHandle {
                let (mut handle, _) = self.allocate_runtime_auto_id(table.ID, None, 2, 1, 1)?;
                if table.ShardRowIDBits != 0 {
                    let shard_bits = table.ShardRowIDBits.min(15) as u32;
                    let incremental_bits = 63_u32.saturating_sub(shard_bits);
                    let incremental_mask = (1_u64 << incremental_bits) - 1;
                    if handle > incremental_mask {
                        return Err(SessionError::new(
                            "[autoid:1467]Failed to read auto-increment value from storage engine",
                        ));
                    }
                    let shard = handle.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(17)
                        >> (64 - shard_bits);
                    handle = (shard << incremental_bits) | handle;
                }
                row.insert("_tidb_rowid".to_owned(), Some(handle.to_string()));
            }
            Self::evaluate_generated_columns(&table, &mut row)?;
            if let Some(value) = Self::unmatched_range_partition_value(&table, &row) {
                let message = format!("Table has no partition for value {value}");
                if plan.Ignore {
                    self.set_warning_with_code(1526, message);
                    continue;
                }
                return Err(SessionError::new(message));
            }
            if let Some(message) = Self::unmatched_list_partition_warning(&table, &row) {
                if plan.Ignore {
                    self.set_warning_with_code(1526, message);
                    continue;
                }
                return Err(SessionError::new(message));
            }
            let (key, value) = self.encode_relational_row_for_write(&table, &row, flags)?;
            // Go InsertValues.removeRow compares table columns as binary datums;
            // the hidden handle of a nonclustered primary key is not a column.
            let unchanged = |existing: &HashMap<String, Option<String>>| {
                table
                    .Columns
                    .iter()
                    .all(|column| existing.get(&column.Name.L) == row.get(&column.Name.L))
            };
            let mut unchanged_row = if plan.Replace {
                working_rows
                    .get(&key.0)
                    .filter(|existing| unchanged(existing))
                    .cloned()
            } else {
                None
            };
            let mut replaced_unique_keys = BTreeSet::new();
            let mut replaced_rows = Vec::new();
            if plan.Replace {
                for index in table
                    .Indices
                    .iter()
                    .filter(|index| index.Unique && index.State == astersql_meta_model::StatePublic)
                {
                    if !index.ConditionExprString.is_empty() {
                        let condition =
                            crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)?;
                        if !row_matches_simple_where(&row, &condition) {
                            continue;
                        }
                    }
                    let incoming = index
                        .Columns
                        .iter()
                        .map(|column| row.get(&column.Name.L).cloned().flatten())
                        .collect::<Option<Vec<_>>>();
                    let Some(incoming) = incoming else {
                        continue;
                    };
                    if plan.Replace && !index.Primary {
                        for values in relational_index_value_rows(&table, index, &row, flags)? {
                            let (index_key, _) = encode_relational_index_value_row(
                                &table, index, &row, flags, values,
                            )?;
                            let dangling_value = self.read_raw_kv(index_key)?;
                            let dangling = dangling_value.is_some()
                                && !working_rows.values().any(|existing_row| {
                                    index.Columns.iter().all(|column| {
                                        existing_row.get(&column.Name.L) == row.get(&column.Name.L)
                                    })
                                });
                            if dangling {
                                let handle = astersql_tablecodec::DecodeHandleInIndexValue(
                                    dangling_value.expect("dangling index value was present"),
                                )
                                .map_err(|error| {
                                    session_error("decode dangling index handle", error)
                                })?
                                .ok_or_else(|| {
                                    SessionError::new("dangling unique index has no row handle")
                                })?;
                                return Err(SessionError::new(format!(
                                    "can not be duplicated row, due to old row not found. handle {} not found",
                                    handle.String()
                                )));
                            }
                        }
                    }
                    for (existing_key, existing_row) in &working_rows {
                        if !index.ConditionExprString.is_empty() {
                            let condition =
                                crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)?;
                            if !row_matches_simple_where(existing_row, &condition) {
                                continue;
                            }
                        }
                        let existing = index
                            .Columns
                            .iter()
                            .map(|column| existing_row.get(&column.Name.L).cloned().flatten())
                            .collect::<Option<Vec<_>>>();
                        if existing.as_ref() == Some(&incoming) {
                            if unchanged(existing_row) {
                                unchanged_row = Some(existing_row.clone());
                            }
                            replaced_unique_keys.insert(existing_key.clone());
                        }
                    }
                }
                if let Some(existing_row) = unchanged_row {
                    affected_rows += 1;
                    unchanged_lock_rows.push(existing_row);
                    continue;
                }
                for existing_key in &replaced_unique_keys {
                    if let Some(existing_row) = working_rows.remove(existing_key) {
                        if let Some(mlog) = &mlog {
                            mlog.append(
                                &table,
                                &existing_row,
                                astersql_table::mview_log::MLogDMLType::Update,
                                -1,
                                &mut mutations,
                            )?;
                        }
                        replaced_rows.push(existing_row.clone());
                        mutations.extend(relational_index_mutations(
                            &table,
                            Some(&existing_row),
                            None,
                            flags,
                        )?);
                    }
                    mutations.push((kv::Key(existing_key.clone()), None));
                }
                if !replaced_rows.is_empty() {
                    let replacement_rows = vec![row.clone(); replaced_rows.len()];
                    self.validate_restrict_foreign_key_updates(
                        &table,
                        &replaced_rows,
                        &replacement_rows,
                    )?;
                }
            }

            if pessimistic_transaction {
                self.acquire_row_lock(
                    RuntimeRowLockKey {
                        domain_id: Arc::as_ptr(&self.domain) as usize,
                        key: key.0.clone(),
                    },
                    false,
                    None,
                    false,
                )?;
            }
            let committed_exists = if skip_committed_primary_key_check {
                false
            } else if pessimistic_transaction {
                let runtime_key = RuntimeRowLockKey {
                    domain_id: Arc::as_ptr(&self.domain) as usize,
                    key: key.0.clone(),
                };
                if self
                    .state
                    .borrow()
                    .transaction_write_keys
                    .contains(&runtime_key)
                {
                    self.read_raw_kv(key.clone())?.is_some()
                } else {
                    self.read_latest_raw_kv(key.clone())?.is_some()
                }
            } else if let Some(transaction) = statement_transaction.as_ref() {
                match transaction.Get(&kv::Context::default(), key.clone(), &[]) {
                    Ok(_) => true,
                    Err(error) if kv::IsErrNotFound(&error) => false,
                    Err(error) => {
                        return Err(session_error("read relational KV", error));
                    }
                }
            } else {
                self.read_raw_kv(key.clone())?.is_some()
            };
            let existing = working_rows.contains_key(&key.0)
                || (committed_exists && !replaced_unique_keys.contains(&key.0));
            let conflict = if existing {
                Some((
                    key.0.clone(),
                    duplicate_primary_key_error(&table, &row).to_string(),
                ))
            } else if plan.Replace {
                None
            } else {
                Self::relational_unique_conflict(&table, &working_rows, &row, flags, None)?
            };
            if let Some((_, message)) = conflict.as_ref()
                && plan.Ignore
            {
                self.set_warning(message.clone());
                continue;
            }
            if let Some((conflict_key, message)) = conflict.as_ref()
                && !plan.Replace
                && plan.OnDuplicate.is_empty()
            {
                if self.defer_optimistic_insert_constraint(&table, conflict_key, message)? {
                    has_deferred_optimistic_constraint = true;
                } else {
                    return Err(SessionError::new(message.clone()));
                }
            }
            if let Some((conflict_key, _)) = conflict
                && !plan.OnDuplicate.is_empty()
            {
                touched_rows += 1;
                let mut updated = if let Some(row) = working_rows.get(&conflict_key).cloned() {
                    row
                } else {
                    self.scan_latest_with_transaction_overlay(&table)?
                        .into_iter()
                        .find_map(|(_, row)| {
                            encode_relational_row(&table, &row, flags)
                                .ok()
                                .filter(|(candidate, _)| {
                                    candidate.0.as_slice() == conflict_key.as_slice()
                                })
                                .map(|_| row)
                        })
                        .ok_or_else(|| {
                            SessionError::new("duplicate relational row was not decoded")
                        })?
                };
                let original = updated.clone();
                let mut incoming = row;
                if let Some(select_columns) = select_columns {
                    // Go resolves unqualified ON DUPLICATE expressions against
                    // both the target row and the INSERT ... SELECT source.
                    // The VALUES-shaped execution path keeps only target
                    // column names, so restore the source projection names by
                    // position before evaluating the update expression.
                    for (source_column, target_column) in
                        select_columns.iter().zip(row_columns.iter())
                    {
                        if let Some(value) = incoming.get(target_column).cloned() {
                            incoming
                                .entry(source_column.to_ascii_lowercase())
                                .or_insert(value);
                        }
                    }
                }
                for (column, expression) in &plan.OnDuplicate {
                    let value = relational_on_duplicate_value(expression, &updated, &incoming)?;
                    updated.insert(column.clone(), value);
                }
                let auto_id_column = table
                    .GetAutoIncrementColInfo()
                    .map(|column| (column, 0))
                    .or_else(|| {
                        table
                            .ContainsAutoRandomBits()
                            .then(|| table.GetPkColInfo().map(|column| (column, 1)))
                            .flatten()
                    });
                if let Some((auto_column, allocator_kind)) = auto_id_column
                    && plan
                        .OnDuplicate
                        .iter()
                        .any(|(column, _)| column == &auto_column.Name.L)
                    && original.get(&auto_column.Name.L) != updated.get(&auto_column.Name.L)
                {
                    let value = updated[&auto_column.Name.L]
                        .as_ref()
                        .ok_or_else(|| SessionError::new("ON DUPLICATE auto ID is NULL"))?
                        .parse::<u64>()
                        .map_err(|error| session_error("parse ON DUPLICATE auto ID", error))?;
                    let (_, rebased) =
                        self.allocate_runtime_auto_id(table.ID, Some(value), allocator_kind, 1, 1)?;
                    if rebased {
                        rebase_count += 1;
                    }
                }
                Self::evaluate_generated_columns(&table, &mut updated)?;
                let foreign_key_check_started = std::time::Instant::now();
                let foreign_key_result = self.validate_and_lock_foreign_keys(
                    &table,
                    std::slice::from_ref(&updated),
                    &lock_rows,
                );
                foreign_key_check_time += foreign_key_check_started.elapsed();
                foreign_key_result?;
                self.validate_restrict_foreign_key_updates(
                    &table,
                    std::slice::from_ref(&original),
                    std::slice::from_ref(&updated),
                )?;
                self.cascade_foreign_key_updates(
                    &table,
                    std::slice::from_ref(&original),
                    std::slice::from_ref(&updated),
                )?;
                let (updated_key, updated_value) =
                    self.encode_relational_row_for_write(&table, &updated, flags)?;
                if updated_key.0 != conflict_key && working_rows.contains_key(&updated_key.0) {
                    return Err(duplicate_primary_key_error(&table, &updated));
                }
                let updated_insert_id = table
                    .GetAutoIncrementColInfo()
                    .or_else(|| {
                        table
                            .ContainsAutoRandomBits()
                            .then(|| table.GetPkColInfo())
                            .flatten()
                    })
                    .and_then(|column| updated.get(&column.Name.L))
                    .and_then(Option::as_ref)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                if updated != original {
                    if let Some(mlog) = &mlog
                        && mlog.tracked_changed(&table, &original, &updated)
                    {
                        mlog.append(
                            &table,
                            &original,
                            astersql_table::mview_log::MLogDMLType::Update,
                            -1,
                            &mut mutations,
                        )?;
                        mlog.append(
                            &table,
                            &updated,
                            astersql_table::mview_log::MLogDMLType::Update,
                            1,
                            &mut mutations,
                        )?;
                    }
                    if updated_key.0 != conflict_key {
                        working_rows.remove(&conflict_key);
                        mutations.push((kv::Key(conflict_key), None));
                    }
                    mutations.extend(relational_index_mutations(
                        &table,
                        Some(&original),
                        Some(&updated),
                        flags,
                    )?);
                    working_rows.insert(updated_key.0.clone(), updated);
                    lock_rows.push(
                        working_rows
                            .get(&updated_key.0)
                            .expect("updated row was inserted")
                            .clone(),
                    );
                    mutations.push((updated_key, Some(updated_value)));
                    affected_rows += 2;
                    updated_record_rows += 1;
                    if last_insert_id == 0 {
                        last_insert_id = updated_insert_id;
                    }
                } else if client_found_rows {
                    affected_rows += 1;
                }
                continue;
            }
            // When ON DUPLICATE KEY UPDATE finds a conflict, MySQL validates
            // the updated existing row, not the discarded candidate INSERT
            // row.  Defer this check until after conflict handling so a
            // candidate with invalid foreign-key values can still produce a
            // valid updated row.
            if plan.Ignore {
                // IGNORE decides tuple-by-tuple whether to keep a row. Plain
                // multi-row INSERT/SELECT is atomic and is validated once as
                // a batch below; checking every tuple separately repeatedly
                // scans and indexes the same parent table.
                let foreign_key_check_started = std::time::Instant::now();
                let foreign_key_result = self.validate_and_lock_foreign_keys(
                    &table,
                    std::slice::from_ref(&row),
                    &lock_rows,
                );
                foreign_key_check_time += foreign_key_check_started.elapsed();
                if let Err(error) = foreign_key_result {
                    self.set_warning(error.to_string());
                    continue;
                }
            }
            let replaced_primary_row = plan
                .Replace
                .then(|| working_rows.get(&key.0).cloned())
                .flatten();
            if let (Some(mlog), Some(previous)) = (&mlog, replaced_primary_row.as_ref()) {
                mlog.append(
                    &table,
                    previous,
                    astersql_table::mview_log::MLogDMLType::Update,
                    -1,
                    &mut mutations,
                )?;
            }
            working_rows.insert(key.0.clone(), row);
            // A regular multi-row INSERT is atomic, so validating all unique
            // indexes once after the batch is sufficient.  Re-scanning every
            // existing row after each tuple makes large VALUES batches
            // quadratic.  INSERT IGNORE still needs tuple-by-tuple validation
            // because only the conflicting tuple is discarded.
            if plan.Ignore
                && let Err(error) =
                    Self::validate_unique_indexes(&table, working_rows.values(), flags)
            {
                working_rows.remove(&key.0);
                self.set_warning(error.to_string());
                continue;
            }
            copied_rows += 1;
            if plan.Replace {
                deleted_rows += if !replaced_unique_keys.is_empty() {
                    replaced_unique_keys.len() as u64
                } else {
                    u64::from(existing)
                };
            }
            affected_rows += if !replaced_unique_keys.is_empty() {
                replaced_unique_keys.len() as u64 + 1
            } else if existing {
                2
            } else {
                1
            };
            if last_insert_id == 0 {
                last_insert_id = candidate_insert_id.unwrap_or(0);
            }
            lock_rows.push(
                working_rows
                    .get(&key.0)
                    .expect("inserted row was inserted")
                    .clone(),
            );
            mutations.extend(relational_index_mutations(
                &table,
                replaced_primary_row.as_ref(),
                working_rows.get(&key.0),
                flags,
            )?);
            mutations.push((key, Some(value)));
            if let Some(mlog) = &mlog {
                mlog.append(
                    &table,
                    lock_rows.last().expect("inserted row was locked"),
                    if plan.Replace && (!replaced_unique_keys.is_empty() || existing) {
                        astersql_table::mview_log::MLogDMLType::Update
                    } else {
                        astersql_table::mview_log::MLogDMLType::Insert
                    },
                    1,
                    &mut mutations,
                )?;
            }
        }
        if !has_deferred_optimistic_constraint {
            Self::validate_unique_indexes(&table, working_rows.values(), flags)?;
        }
        let foreign_key_check_started = std::time::Instant::now();
        let foreign_key_result = self.validate_and_lock_foreign_keys(&table, &lock_rows, &[]);
        foreign_key_check_time += foreign_key_check_started.elapsed();
        foreign_key_result?;
        let check_insert_time = check_insert_started.elapsed();
        let ttl_insert_rows = table
            .TTLInfo
            .as_ref()
            .filter(|info| info.Enable)
            .map_or(0, |_| lock_rows.len());
        self.apply_relational_mutations_with_autocommit(
            &plan.Table,
            if plan.Replace { "Replace" } else { "Insert" },
            mutations,
            unique_lock_keys_for_rows(&table, lock_rows.iter().chain(unchanged_lock_rows.iter()))
                .into_iter()
                .chain(primary_lock_keys_for_rows(
                    &table,
                    lock_rows.iter().chain(unchanged_lock_rows.iter()),
                ))
                .collect(),
            affected_rows,
            last_insert_id,
            alloc_count,
            rebase_count,
            statement_transaction.take(),
        )?;
        let insert_total_time = insert_started.elapsed();
        if let Some(report) = self.state.borrow_mut().last_dml_report.as_mut() {
            report.InsertTotalTime = insert_total_time;
            report.InsertPrepareTime = insert_total_time.saturating_sub(check_insert_time);
            report.CheckInsertTime = check_insert_time;
            report.InsertPrefetchTime = prefetch_time;
            report.ForeignKeyCheckTime = foreign_key_check_time;
            report.HasInsertRuntimeStats = true;
            report.HasForeignKeyChecks = has_foreign_key_checks;
        }
        self.record_ttl_insert_rows(&table, ttl_insert_rows);
        let stats_after = working_rows.into_values().collect::<Vec<_>>();
        self.record_relational_stats(&plan.Table, &table, &stats_before, &stats_after)?;
        {
            let mut state = self.state.borrow_mut();
            if plan.Ignore {
                for warning in &mut state.current_warnings {
                    if let Some(message) = warning.message.strip_prefix("[kv:1062]") {
                        warning.code = 1062;
                        warning.message = message.to_owned();
                    }
                }
            }
            if records > 1 || from_select {
                let duplicates = if plan.Replace {
                    affected_rows.saturating_sub(records)
                } else if plan.Ignore {
                    records.saturating_sub(affected_rows)
                } else if client_found_rows {
                    touched_rows
                } else {
                    updated_record_rows
                };
                state.last_message = format!(
                    "Records: {records}  Duplicates: {}  Warnings: {}",
                    duplicates,
                    state.current_warnings.len()
                );
            }
        }
        Ok((copied_rows, deleted_rows))
    }

    /// 执行关系型 UPDATE 计划。
    pub(super) fn execute_relational_update(
        &self,
        plan: crate::dml_runtime::UpdatePlan,
    ) -> SessionResult<()> {
        let flags = self.dml_type_flags();
        let current_database = self.current_database();
        let database = if plan.Schema.is_empty() {
            current_database.as_str()
        } else {
            plan.Schema.as_str()
        };
        let table = self
            .resolve_runtime_table(database, &plan.Table)
            .ok_or_else(|| SessionError::new(format!("unknown DML table {}", plan.Table)))?;
        let mlog = RuntimeMLog::for_table(self, database, &table, flags)?;
        self.record_transaction_table_write(&table);
        let matches_plan = |row: &HashMap<String, Option<String>>| -> SessionResult<bool> {
            if let Some(predicate) = plan.Predicate.as_ref() {
                Ok(relational_truth(
                    self.relational_query_expression_value(predicate, row, None)?
                        .as_deref(),
                ) == Some(true))
            } else {
                plan.Predicates
                    .iter()
                    .map(|(column, operator, key)| {
                        crate::dml_runtime::MatchesPredicate(row, column, operator, key)
                    })
                    .collect::<SessionResult<Vec<_>>>()
                    .map(|matches| matches.into_iter().all(|matches| matches))
            }
        };
        let select_candidates = |rows: Vec<RelationalRow>| -> SessionResult<Vec<RelationalRow>> {
            let mut candidates = rows
                .into_iter()
                .map(|row| matches_plan(&row.1).map(|matches| (row, matches)))
                .collect::<SessionResult<Vec<_>>>()?
                .into_iter()
                .filter_map(|(row, matches)| matches.then_some(row))
                .collect::<Vec<_>>();
            if !plan.Order.is_empty() {
                sort_relational_rows(&mut candidates, &plan.Order)?;
            }
            execute_relational_limit(
                candidates,
                relational_dml_limit_window(plan.Limit.as_ref())?,
            )
        };
        let primary_key_ranges = if plan.Order.is_empty() && plan.Limit.is_none() {
            plan.Predicate
                .as_ref()
                .and_then(|predicate| relational_primary_key_scan_ranges(&table, predicate, None))
        } else {
            None
        };
        // Keep the full-scan boundary ahead of locking and mutation.  The
        // production scan below uses a latest-read overlay for correct
        // pessimistic semantics, so mirror scan_registered_table's test hook
        // here instead of discovering the injected failure during the
        // post-commit statistics scan.
        if primary_key_ranges.is_none()
            && astersql_testkit_testfailpoint::eval_bool("session/updateFullTableScan")
        {
            return Err(SessionError::new(
                "injected failure: UPDATE reached a full relational table scan",
            ));
        }
        let lock_updates = {
            let state = self.state.borrow();
            state.transaction.is_none()
                || (!state.transaction_explicit_optimistic
                    && (!state.transaction_pessimistic
                        || state.constraint_check_in_place_pessimistic))
        };
        if lock_updates {
            let rows = if let Some(ranges) = primary_key_ranges.as_deref() {
                self.scan_latest_with_transaction_overlay_ranges(&table, ranges)?
            } else {
                self.scan_latest_with_transaction_overlay(&table)?
            };
            let candidates = select_candidates(rows)?;
            let domain_id = Arc::as_ptr(&self.domain) as usize;
            let mut keys = candidates
                .iter()
                .map(|(_, row)| {
                    encode_relational_row(&table, row, flags).map(|(key, _)| RuntimeRowLockKey {
                        domain_id,
                        key: key.0,
                    })
                })
                .collect::<SessionResult<Vec<_>>>()?;
            keys.extend(
                unique_lock_keys_for_rows(&table, candidates.iter().map(|(_, row)| row))
                    .into_iter()
                    .map(|key| RuntimeRowLockKey { domain_id, key }),
            );
            // A pessimistic UPDATE must also lock a unique key when no
            // committed row matches yet. Another transaction may have an
            // uncommitted INSERT for that key; block until it commits rather
            // than returning zero affected rows.
            if let Some(predicate) = plan.Predicate.as_ref() {
                keys.extend(
                    unique_lock_keys_for_predicate(&table, predicate)
                        .into_iter()
                        .map(|key| RuntimeRowLockKey { domain_id, key }),
                );
            }
            self.acquire_row_locks(keys, false, None, false)?;
        }
        let stats_before_with_handles = if lock_updates {
            if let Some(ranges) = primary_key_ranges.as_deref() {
                self.scan_latest_with_transaction_overlay_ranges(&table, ranges)?
            } else {
                self.scan_latest_with_transaction_overlay(&table)?
            }
        } else if let Some(ranges) = primary_key_ranges.as_deref() {
            self.scan_registered_table_ranges(&table, ranges)?
        } else {
            self.scan_registered_table(&table)?
        };
        let selected_keys = select_candidates(stats_before_with_handles.clone())?
            .into_iter()
            .map(|(_, row)| encode_relational_row(&table, &row, flags).map(|(key, _)| key.0))
            .collect::<SessionResult<HashSet<_>>>()?;
        let matched_rows = selected_keys.len() as u64;
        let stats_before = stats_before_with_handles
            .iter()
            .map(|(_, row)| row.clone())
            .collect::<Vec<_>>();
        let original_rows = stats_before_with_handles
            .iter()
            .map(|(_, row)| {
                encode_relational_row(&table, row, flags).map(|(key, _)| (key.0, row.clone()))
            })
            .collect::<SessionResult<HashMap<_, _>>>()?;
        let deferred_constraint_epoch = {
            let state = self.state.borrow();
            (state.transaction.is_some()
                && state.transaction_pessimistic
                && !state.constraint_check_in_place_pessimistic)
                .then_some(state.transaction_read_epoch)
        };
        if let Some(read_epoch) = deferred_constraint_epoch {
            let domain_id = Arc::as_ptr(&self.domain) as usize;
            let epochs = RUNTIME_KEY_COMMIT_EPOCHS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (_, row) in &stats_before_with_handles {
                let (key, _) = encode_relational_row(&table, row, flags)?;
                if !selected_keys.contains(&key.0) {
                    continue;
                }
                let runtime_key = RuntimeRowLockKey {
                    domain_id,
                    key: key.0,
                };
                let primary_conflict = epochs
                    .get(&runtime_key)
                    .is_some_and(|epoch| *epoch > read_epoch);
                let unique_conflict = unique_lock_keys_for_rows(&table, std::iter::once(row))
                    .into_iter()
                    .map(|key| RuntimeRowLockKey { domain_id, key })
                    .any(|key| epochs.get(&key).is_some_and(|epoch| *epoch > read_epoch));
                if primary_conflict || unique_conflict {
                    drop(epochs);
                    self.finish_transaction(false)?;
                    if unique_conflict
                        && let Some(index) = table.Indices.iter().find(|index| {
                            index.Unique && index.State == astersql_meta_model::StatePublic
                        })
                    {
                        let value = index
                            .Columns
                            .iter()
                            .filter_map(|column| row.get(&column.Name.L).cloned().flatten())
                            .collect::<Vec<_>>()
                            .join("-");
                        return Err(SessionError::new(format!(
                            "[kv:1062]Duplicate entry '{value}' for key '{}.{}'",
                            table.Name.O, index.Name.O
                        )));
                    }
                    return Err(duplicate_primary_key_error(&table, row));
                }
            }
        }
        let mut mutations = Vec::new();
        let mut affected_rows = 0;
        let mut rebase_count = 0;
        let mut updated_rows = Vec::with_capacity(stats_before.len());
        let mut lock_rows = Vec::new();
        let bit_columns = table
            .Columns
            .iter()
            .filter(|column| column.GetType() == astersql_parser_mysql::r#type::TypeBit)
            .map(|column| column.Name.L.clone())
            .collect::<Vec<_>>();
        for (_, mut row) in stats_before_with_handles.iter().cloned() {
            let (old_key, _) = encode_relational_row(&table, &row, flags)?;
            if !selected_keys.contains(&old_key.0) {
                updated_rows.push(row);
                continue;
            }
            let original = row.clone();
            for (column, expression) in &plan.Assignments {
                let column_info = table.Columns.iter().find(|info| info.Name.L == *column);
                let mut value = if let Some(column_info) =
                    column_info.filter(|_| is_typed_literal(expression))
                {
                    typed_literal_to_runtime_value(expression, column_info, flags)?
                } else {
                    match expression.Kind {
                        ast::ExprKind::Function { .. }
                        | ast::ExprKind::Subquery { .. }
                        | ast::ExprKind::CompareSubquery { .. }
                        | ast::ExprKind::InSubquery { .. }
                        | ast::ExprKind::ExistsSubquery { .. } => {
                            self.relational_query_expression_value(expression, &row, None)?
                        }
                        _ => crate::dml_runtime::EvalExprWithBitColumns(
                            expression,
                            &row,
                            None,
                            &bit_columns,
                        )?,
                    }
                };
                if let (Some(column_info), Some(runtime_value)) = (column_info, value.as_deref())
                    && column_info.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                {
                    let vector = astersql_types::vector::ParseVectorFloat32(runtime_value)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    vector
                        .CheckDimsFitColumn(column_info.GetFlen() as i32)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    value = Some(vector.String());
                }
                row.insert(column.clone(), value);
            }
            let auto_id_column = table
                .GetAutoIncrementColInfo()
                .map(|column| (column, 0))
                .or_else(|| {
                    table
                        .ContainsAutoRandomBits()
                        .then(|| table.GetPkColInfo().map(|column| (column, 1)))
                        .flatten()
                });
            if let Some((auto_column, allocator_kind)) = auto_id_column
                && plan
                    .Assignments
                    .iter()
                    .any(|(column, _)| column == &auto_column.Name.L)
            {
                let value = row[&auto_column.Name.L]
                    .as_ref()
                    .ok_or_else(|| SessionError::new("updated auto ID is NULL"))?
                    .parse::<u64>()
                    .map_err(|error| session_error("parse updated auto ID", error))?;
                self.allocate_runtime_auto_id(table.ID, Some(value), allocator_kind, 1, 1)?;
                rebase_count += 1;
            }
            Self::evaluate_generated_columns(&table, &mut row)?;
            if row == original {
                updated_rows.push(row);
                continue;
            }
            let (new_key, new_value) = self.encode_relational_row_for_write(&table, &row, flags)?;
            let unique_values_changed = old_key != new_key
                || table.Indices.iter().any(|index| {
                    index.Unique
                        && index.State == astersql_meta_model::StatePublic
                        && index
                            .Columns
                            .iter()
                            .any(|column| original.get(&column.Name.L) != row.get(&column.Name.L))
                });
            if unique_values_changed {
                if primary_key_ranges.is_some() {
                    if let Some(message) =
                        self.relational_unique_kv_conflict(&table, &original, &row, flags)?
                    {
                        return Err(SessionError::new(message));
                    }
                } else if let Some((_, message)) = Self::relational_unique_conflict(
                    &table,
                    &original_rows,
                    &row,
                    flags,
                    Some(&old_key.0),
                )? {
                    return Err(SessionError::new(message));
                }
            }
            if old_key != new_key {
                if self.read_raw_kv(new_key.clone())?.is_some() {
                    return Err(duplicate_primary_key_error(&table, &row));
                }
                mutations.push((old_key, None));
            }
            mutations.extend(relational_index_mutations(
                &table,
                Some(&original),
                Some(&row),
                flags,
            )?);
            mutations.push((new_key, Some(new_value)));
            if let Some(mlog) = &mlog
                && mlog.tracked_changed(&table, &original, &row)
            {
                mlog.append(
                    &table,
                    &original,
                    astersql_table::mview_log::MLogDMLType::Update,
                    -1,
                    &mut mutations,
                )?;
                mlog.append(
                    &table,
                    &row,
                    astersql_table::mview_log::MLogDMLType::Update,
                    1,
                    &mut mutations,
                )?;
            }
            affected_rows += 1;
            lock_rows.push(row.clone());
            updated_rows.push(row);
        }
        Self::validate_unique_indexes(&table, updated_rows.iter(), flags)?;
        self.validate_and_lock_foreign_keys(&table, &updated_rows, &[])?;
        self.validate_restrict_foreign_key_updates(&table, &stats_before, &updated_rows)?;
        self.cascade_foreign_key_updates(&table, &stats_before, &updated_rows)?;
        let reported_affected_rows = if self.state.borrow().client_capability
            & astersql_parser_mysql::r#const::ClientFoundRows
            != 0
        {
            matched_rows
        } else {
            affected_rows
        };
        self.apply_relational_mutations(
            &plan.Table,
            "Update",
            mutations,
            unique_lock_keys_for_rows(&table, lock_rows.iter()),
            reported_affected_rows,
            0,
            0,
            rebase_count,
        )?;
        let warning_count = self.state.borrow().current_warnings.len();
        self.state.borrow_mut().last_message = format!(
            "Rows matched: {matched_rows}  Changed: {affected_rows}  Warnings: {warning_count}"
        );
        let stats_after = if primary_key_ranges.is_some() {
            updated_rows
        } else {
            self.scan_registered_table(&table)?
                .into_iter()
                .map(|(_, row)| row)
                .collect::<Vec<_>>()
        };
        self.record_relational_stats(&plan.Table, &table, &stats_before, &stats_after)
    }

    fn relational_join_target_row(
        table: &astersql_meta_model::TableInfo,
        qualifier: &str,
        joined: &HashMap<String, Option<String>>,
    ) -> HashMap<String, Option<String>> {
        let mut row = table
            .Columns
            .iter()
            .map(|column| {
                let qualified = format!("{qualifier}.{}", column.Name.L);
                (
                    column.Name.L.clone(),
                    joined
                        .get(&qualified)
                        .or_else(|| joined.get(&column.Name.L))
                        .cloned()
                        .unwrap_or(None),
                )
            })
            .collect::<HashMap<_, _>>();
        if !table.PKIsHandle && !table.IsCommonHandle && table.GetAutoIncrementColInfo().is_none() {
            let qualified = format!("{qualifier}._tidb_rowid");
            row.insert(
                "_tidb_rowid".to_owned(),
                joined
                    .get(&qualified)
                    .or_else(|| joined.get("_tidb_rowid"))
                    .cloned()
                    .unwrap_or(None),
            );
        }
        row
    }

    pub(super) fn execute_relational_join_update(
        &self,
        statement: &ast::UpdateStmt,
    ) -> SessionResult<()> {
        let table_refs = statement
            .TableRefs
            .as_ref()
            .ok_or_else(|| SessionError::new("UPDATE has no table references"))?;
        let mut sources = Vec::new();
        if let Some(left) = table_refs.TableRefs.Left.as_deref() {
            collect_physical_table_sources(left, &mut sources);
        }
        if let Some(right) = table_refs.TableRefs.Right.as_deref() {
            collect_physical_table_sources(right, &mut sources);
        }
        let target_source = sources
            .first()
            .copied()
            .ok_or_else(|| SessionError::new("UPDATE join has no target table"))?;
        let database = if target_source.Source.Schema.L.is_empty() {
            self.current_database()
        } else {
            target_source.Source.Schema.L.clone()
        };
        let (_, table) = self
            .mdl_stats_table(&database, &target_source.Source.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!("unknown DML table {}", target_source.Source.Name.O))
            })?;
        let qualifier = if target_source.AsName.L.is_empty() {
            target_source.Source.Name.L.as_str()
        } else {
            target_source.AsName.L.as_str()
        };
        let flags = self.dml_type_flags();
        let mlog = RuntimeMLog::for_table(self, &database, &table, flags)?;
        let stats_before = self
            .scan_registered_table(&table)?
            .into_iter()
            .map(|(_, row)| row)
            .collect::<Vec<_>>();
        let joined = self.execute_insert_select_join(&table_refs.TableRefs)?;
        let mut mutations = Vec::new();
        let mut updated_rows = stats_before.clone();
        let mut updated_keys = HashSet::new();
        let mut affected_rows = 0_u64;
        for joined_row in joined.rows {
            if let Some(predicate) = statement.Where.as_ref()
                && !self.insert_select_predicate(predicate, &joined_row)?
            {
                continue;
            }
            let original = Self::relational_join_target_row(&table, qualifier, &joined_row);
            let (old_key, _) = encode_relational_row(&table, &original, flags)?;
            if !updated_keys.insert(old_key.0.clone()) {
                continue;
            }
            let mut updated = original.clone();
            for assignment in &statement.List {
                if !assignment.Column.Table.L.is_empty()
                    && assignment.Column.Table.L != qualifier
                    && assignment.Column.Table.L != target_source.Source.Name.L
                {
                    continue;
                }
                let value =
                    self.relational_query_expression_value(&assignment.Expr, &joined_row, None)?;
                updated.insert(assignment.Column.Name.L.clone(), value);
            }
            Self::evaluate_generated_columns(&table, &mut updated)?;
            if updated == original {
                continue;
            }
            let (new_key, new_value) =
                self.encode_relational_row_for_write(&table, &updated, flags)?;
            if new_key != old_key {
                mutations.push((old_key, None));
            }
            mutations.extend(relational_index_mutations(
                &table,
                Some(&original),
                Some(&updated),
                flags,
            )?);
            mutations.push((new_key, Some(new_value)));
            if let Some(mlog) = &mlog
                && mlog.tracked_changed(&table, &original, &updated)
            {
                mlog.append(
                    &table,
                    &original,
                    astersql_table::mview_log::MLogDMLType::Update,
                    -1,
                    &mut mutations,
                )?;
                mlog.append(
                    &table,
                    &updated,
                    astersql_table::mview_log::MLogDMLType::Update,
                    1,
                    &mut mutations,
                )?;
            }
            if let Some(row) = updated_rows.iter_mut().find(|row| **row == original) {
                *row = updated;
            }
            affected_rows += 1;
        }
        Self::validate_unique_indexes(&table, updated_rows.iter(), flags)?;
        let matched_rows = updated_keys.len() as u64;
        let reported_affected_rows = if self.state.borrow().client_capability
            & astersql_parser_mysql::r#const::ClientFoundRows
            != 0
        {
            matched_rows
        } else {
            affected_rows
        };
        self.apply_relational_mutations(
            &target_source.Source.Name.L,
            "Update",
            mutations,
            unique_lock_keys_for_rows(&table, updated_rows.iter()),
            reported_affected_rows,
            0,
            0,
            0,
        )?;
        let warning_count = self.state.borrow().current_warnings.len();
        self.state.borrow_mut().last_message = format!(
            "Rows matched: {matched_rows}  Changed: {affected_rows}  Warnings: {warning_count}"
        );
        let stats_after = self
            .scan_registered_table(&table)?
            .into_iter()
            .map(|(_, row)| row)
            .collect::<Vec<_>>();
        self.record_relational_stats(
            &target_source.Source.Name.L,
            &table,
            &stats_before,
            &stats_after,
        )
    }

    /// 执行关系型 DELETE 计划。
    pub(super) fn execute_relational_delete(
        &self,
        plan: crate::dml_runtime::DeletePlan,
    ) -> SessionResult<()> {
        let flags = self.dml_type_flags();
        let current_database = self.current_database();
        let database = if plan.Schema.is_empty() {
            current_database.as_str()
        } else {
            plan.Schema.as_str()
        };
        let table = self
            .resolve_runtime_table(database, &plan.Table)
            .ok_or_else(|| SessionError::new(format!("unknown DML table {}", plan.Table)))?;
        let mlog = RuntimeMLog::for_table(self, database, &table, flags)?;
        self.record_transaction_table_write(&table);
        let matches_plan = |row: &HashMap<String, Option<String>>| -> SessionResult<bool> {
            if let Some(predicate) = plan.Predicate.as_ref() {
                let value = match predicate.Kind {
                    ast::ExprKind::Subquery { .. }
                    | ast::ExprKind::CompareSubquery { .. }
                    | ast::ExprKind::InSubquery { .. }
                    | ast::ExprKind::ExistsSubquery { .. } => {
                        self.relational_query_expression_value(predicate, row, None)?
                    }
                    _ => relational_table_expression_value(predicate, row, &table)?,
                };
                Ok(relational_truth(value.as_deref()) == Some(true))
            } else {
                plan.Predicates
                    .iter()
                    .map(|(column, operator, key)| {
                        crate::dml_runtime::MatchesPredicate(row, column, operator, key)
                    })
                    .collect::<SessionResult<Vec<_>>>()
                    .map(|matches| matches.into_iter().all(|matches| matches))
            }
        };
        let mut stats_before_with_handles = self.scan_registered_table(&table)?;
        let mut candidates = stats_before_with_handles
            .iter()
            .map(|row| matches_plan(&row.1).map(|matches| (row.clone(), matches)))
            .collect::<SessionResult<Vec<_>>>()?
            .into_iter()
            .filter_map(|(row, matches)| matches.then_some(row))
            .collect::<Vec<_>>();
        if !plan.Order.is_empty() {
            sort_relational_rows(&mut candidates, &plan.Order)?;
        }
        let selected_keys = execute_relational_limit(
            candidates,
            relational_dml_limit_window(plan.Limit.as_ref())?,
        )?
        .into_iter()
        .map(|(_, row)| encode_relational_row(&table, &row, flags).map(|(key, _)| key.0))
        .collect::<SessionResult<HashSet<_>>>()?;
        let lock_deletes = {
            let state = self.state.borrow();
            state.transaction.is_none() || state.transaction_pessimistic
        };
        if lock_deletes {
            let domain_id = Arc::as_ptr(&self.domain) as usize;
            self.acquire_row_locks(
                selected_keys
                    .iter()
                    .cloned()
                    .map(|key| RuntimeRowLockKey { domain_id, key })
                    .collect(),
                false,
                None,
                false,
            )?;
            // A concurrent UPDATE may commit while DELETE waits for the record.
            // Build deletions from the current row, including its new index keys.
            stats_before_with_handles = self.scan_latest_with_transaction_overlay(&table)?;
        }
        let stats_before = stats_before_with_handles
            .iter()
            .map(|(_, row)| row.clone())
            .collect::<Vec<_>>();
        let mut mutations = Vec::new();
        let mut deleted_rows = Vec::new();
        let mut affected_rows = 0;
        for (_, row) in stats_before_with_handles {
            let (key, _) = encode_relational_row(&table, &row, flags)?;
            if selected_keys.contains(&key.0) && matches_plan(&row)? {
                mutations.push((key, None));
                mutations.extend(relational_index_mutations(&table, Some(&row), None, flags)?);
                if let Some(mlog) = &mlog {
                    mlog.append(
                        &table,
                        &row,
                        astersql_table::mview_log::MLogDMLType::Delete,
                        -1,
                        &mut mutations,
                    )?;
                }
                deleted_rows.push(row);
                affected_rows += 1;
            }
        }
        self.cascade_foreign_key_deletes(&table, &deleted_rows)?;
        self.apply_relational_mutations(
            &plan.Table,
            "Delete",
            mutations,
            unique_lock_keys_for_rows(&table, deleted_rows.iter()),
            affected_rows,
            0,
            0,
            0,
        )?;
        let stats_after = self
            .scan_registered_table(&table)?
            .into_iter()
            .map(|(_, row)| row)
            .collect::<Vec<_>>();
        self.record_relational_stats(&plan.Table, &table, &stats_before, &stats_after)
    }

    pub(super) fn execute_relational_join_delete(
        &self,
        statement: &ast::DeleteStmt,
    ) -> SessionResult<()> {
        let table_refs = statement
            .TableRefs
            .as_ref()
            .ok_or_else(|| SessionError::new("DELETE has no table references"))?;
        let mut sources = Vec::new();
        if let Some(left) = table_refs.TableRefs.Left.as_deref() {
            collect_physical_table_sources(left, &mut sources);
        }
        if let Some(right) = table_refs.TableRefs.Right.as_deref() {
            collect_physical_table_sources(right, &mut sources);
        }
        let target_sources = if statement.Tables.is_empty() {
            sources.first().copied().into_iter().collect::<Vec<_>>()
        } else {
            statement
                .Tables
                .iter()
                .map(|requested| {
                    sources
                        .iter()
                        .copied()
                        .find(|source| {
                            requested.Name.L == source.Source.Name.L
                                || requested.Name.L == source.AsName.L
                                || requested.Name.L.is_empty()
                        })
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "DELETE join target {} is not in table references",
                                requested.Name.O
                            ))
                        })
                })
                .collect::<SessionResult<Vec<_>>>()?
        };
        if target_sources.is_empty() {
            return Err(SessionError::new("DELETE join has no target table"));
        }
        let flags = self.dml_type_flags();
        let joined = self.execute_insert_select_join(&table_refs.TableRefs)?;
        let mut joined_rows = Vec::new();
        for joined_row in joined.rows {
            if let Some(predicate) = statement.Where.as_ref()
                && !self.insert_select_predicate(predicate, &joined_row)?
            {
                continue;
            }
            joined_rows.push(joined_row);
        }
        for target_source in target_sources {
            let database = if target_source.Source.Schema.L.is_empty() {
                self.current_database()
            } else {
                target_source.Source.Schema.L.clone()
            };
            let (_, table) = self
                .mdl_stats_table(&database, &target_source.Source.Name.L)
                .ok_or_else(|| {
                    SessionError::new(format!("unknown DML table {}", target_source.Source.Name.O))
                })?;
            let mlog = RuntimeMLog::for_table(self, &database, &table, flags)?;
            let qualifier = if target_source.AsName.L.is_empty() {
                target_source.Source.Name.L.as_str()
            } else {
                target_source.AsName.L.as_str()
            };
            let stats_before = self
                .scan_registered_table(&table)?
                .into_iter()
                .map(|(_, row)| row)
                .collect::<Vec<_>>();
            let mut mutations = Vec::new();
            let mut deleted_rows = Vec::new();
            let mut deleted_keys = HashSet::new();
            for joined_row in &joined_rows {
                let row = Self::relational_join_target_row(&table, qualifier, joined_row);
                let (key, _) = encode_relational_row(&table, &row, flags)?;
                if !deleted_keys.insert(key.0.clone()) {
                    continue;
                }
                mutations.push((key, None));
                mutations.extend(relational_index_mutations(&table, Some(&row), None, flags)?);
                if let Some(mlog) = &mlog {
                    mlog.append(
                        &table,
                        &row,
                        astersql_table::mview_log::MLogDMLType::Delete,
                        -1,
                        &mut mutations,
                    )?;
                }
                deleted_rows.push(row);
            }
            self.cascade_foreign_key_deletes(&table, &deleted_rows)?;
            self.apply_relational_mutations(
                &target_source.Source.Name.L,
                "Delete",
                mutations,
                unique_lock_keys_for_rows(&table, deleted_rows.iter()),
                deleted_rows.len() as u64,
                0,
                0,
                0,
            )?;
            let stats_after = self
                .scan_registered_table(&table)?
                .into_iter()
                .map(|(_, row)| row)
                .collect::<Vec<_>>();
            self.record_relational_stats(
                &target_source.Source.Name.L,
                &table,
                &stats_before,
                &stats_after,
            )?;
        }
        Ok(())
    }
}
