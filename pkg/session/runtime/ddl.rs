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

//! DDL metadata changes, job lifecycle, and backfill operations.

use super::*;

impl ConcreteSession {
    /// Next-gen bootstrap builds mysql tables with metadef's fixed IDs before
    /// inserting their metadata directly. The SQL bootstrap must preserve the
    /// same ID map when it creates the authoritative definitions.
    fn ddl_system_table_id(database: &str, table: &str) -> Option<i64> {
        if !database.eq_ignore_ascii_case("mysql") {
            return None;
        }
        match table.to_ascii_lowercase().as_str() {
            "tidb_ddl_job" => Some(astersql_meta_metadef::TiDBDDLJobTableID),
            "tidb_ddl_reorg" => Some(astersql_meta_metadef::TiDBDDLReorgTableID),
            "tidb_ddl_history" => Some(astersql_meta_metadef::TiDBDDLHistoryTableID),
            "tidb_mdl_info" => Some(astersql_meta_metadef::TiDBMDLInfoTableID),
            "tidb_background_subtask" => Some(astersql_meta_metadef::TiDBBackgroundSubtaskTableID),
            "tidb_background_subtask_history" => {
                Some(astersql_meta_metadef::TiDBBackgroundSubtaskHistoryTableID)
            }
            "tidb_ddl_notifier" => Some(astersql_meta_metadef::TiDBDDLNotifierTableID),
            "user" => Some(astersql_meta_metadef::UserTableID),
            "password_history" => Some(astersql_meta_metadef::PasswordHistoryTableID),
            "global_priv" => Some(astersql_meta_metadef::GlobalPrivTableID),
            "db" => Some(astersql_meta_metadef::DBTableID),
            "tables_priv" => Some(astersql_meta_metadef::TablesPrivTableID),
            "columns_priv" => Some(astersql_meta_metadef::ColumnsPrivTableID),
            "global_variables" => Some(astersql_meta_metadef::GlobalVariablesTableID),
            "tidb" => Some(astersql_meta_metadef::TiDBTableID),
            "help_topic" => Some(astersql_meta_metadef::HelpTopicTableID),
            "stats_meta" => Some(astersql_meta_metadef::StatsMetaTableID),
            "stats_histograms" => Some(astersql_meta_metadef::StatsHistogramsTableID),
            "stats_buckets" => Some(astersql_meta_metadef::StatsBucketsTableID),
            "gc_delete_range" => Some(astersql_meta_metadef::GCDeleteRangeTableID),
            "gc_delete_range_done" => Some(astersql_meta_metadef::GCDeleteRangeDoneTableID),
            "stats_feedback" => Some(astersql_meta_metadef::StatsFeedbackTableID),
            "role_edges" => Some(astersql_meta_metadef::RoleEdgesTableID),
            "default_roles" => Some(astersql_meta_metadef::DefaultRolesTableID),
            "bind_info" => Some(astersql_meta_metadef::BindInfoTableID),
            "stats_top_n" => Some(astersql_meta_metadef::StatsTopNTableID),
            "expr_pushdown_blacklist" => Some(astersql_meta_metadef::ExprPushdownBlacklistTableID),
            "opt_rule_blacklist" => Some(astersql_meta_metadef::OptRuleBlacklistTableID),
            "stats_extended" => Some(astersql_meta_metadef::StatsExtendedTableID),
            "stats_fm_sketch" => Some(astersql_meta_metadef::StatsFMSketchTableID),
            "global_grants" => Some(astersql_meta_metadef::GlobalGrantsTableID),
            "capture_plan_baselines_blacklist" => {
                Some(astersql_meta_metadef::CapturePlanBaselinesBlacklistTableID)
            }
            "column_stats_usage" => Some(astersql_meta_metadef::ColumnStatsUsageTableID),
            "table_cache_meta" => Some(astersql_meta_metadef::TableCacheMetaTableID),
            "analyze_options" => Some(astersql_meta_metadef::AnalyzeOptionsTableID),
            "stats_history" => Some(astersql_meta_metadef::StatsHistoryTableID),
            "stats_meta_history" => Some(astersql_meta_metadef::StatsMetaHistoryTableID),
            "analyze_jobs" => Some(astersql_meta_metadef::AnalyzeJobsTableID),
            "advisory_locks" => Some(astersql_meta_metadef::AdvisoryLocksTableID),
            "plan_replayer_status" => Some(astersql_meta_metadef::PlanReplayerStatusTableID),
            "plan_replayer_task" => Some(astersql_meta_metadef::PlanReplayerTaskTableID),
            "stats_table_locked" => Some(astersql_meta_metadef::StatsTableLockedTableID),
            "tidb_ttl_table_status" => Some(astersql_meta_metadef::TiDBTTLTableStatusTableID),
            "tidb_ttl_task" => Some(astersql_meta_metadef::TiDBTTLTaskTableID),
            "tidb_ttl_job_history" => Some(astersql_meta_metadef::TiDBTTLJobHistoryTableID),
            "tidb_global_task" => Some(astersql_meta_metadef::TiDBGlobalTaskTableID),
            "tidb_global_task_history" => Some(astersql_meta_metadef::TiDBGlobalTaskHistoryTableID),
            "tidb_import_jobs" => Some(astersql_meta_metadef::TiDBImportJobsTableID),
            "tidb_runaway_watch" => Some(astersql_meta_metadef::TiDBRunawayWatchTableID),
            "tidb_runaway_queries" => Some(astersql_meta_metadef::TiDBRunawayQueriesTableID),
            "tidb_timers" => Some(astersql_meta_metadef::TiDBTimersTableID),
            "tidb_runaway_watch_done" => Some(astersql_meta_metadef::TiDBRunawayWatchDoneTableID),
            "dist_framework_meta" => Some(astersql_meta_metadef::DistFrameworkMetaTableID),
            "request_unit_by_group" => Some(astersql_meta_metadef::RequestUnitByGroupTableID),
            "tidb_pitr_id_map" => Some(astersql_meta_metadef::TiDBPITRIDMapTableID),
            "tidb_restore_registry" => Some(astersql_meta_metadef::TiDBRestoreRegistryTableID),
            "index_advisor_results" => Some(astersql_meta_metadef::IndexAdvisorResultsTableID),
            "tidb_kernel_options" => Some(astersql_meta_metadef::TiDBKernelOptionsTableID),
            "tidb_workload_values" => Some(astersql_meta_metadef::TiDBWorkloadValuesTableID),
            "tidb_softdelete_table_status" => {
                Some(astersql_meta_metadef::TiDBSoftDeleteTableStatusTableID)
            }
            "tidb_masking_policy" if astersql_config_kerneltype::IsNextGen() => {
                Some(astersql_meta_metadef::TiDBMaskingPolicyTableID)
            }
            _ => None,
        }
    }

    fn ddl_collation_charset(collation: &str) -> Option<&'static str> {
        let collation = collation.to_ascii_lowercase();
        if collation == "binary" {
            Some("binary")
        } else if collation.starts_with("utf8mb4_") {
            Some("utf8mb4")
        } else if collation.starts_with("utf8_") {
            Some("utf8")
        } else if collation.starts_with("ascii_") {
            Some("ascii")
        } else if collation.starts_with("latin1_") {
            Some("latin1")
        } else {
            None
        }
    }

    fn validate_ddl_collation(collation: &str) -> SessionResult<()> {
        if collation.to_ascii_lowercase().ends_with("_roman_ci") {
            return Err(SessionError::new(format!(
                "[ddl:1273]Unsupported collation when new collation is enabled: '{}'",
                collation.to_ascii_lowercase()
            )));
        }
        Ok(())
    }

    fn validate_create_table_collations(statement: &ast::CreateTableStmt) -> SessionResult<()> {
        for option in &statement.Options {
            if option.Tp == ast::TableOptionType::Collate {
                Self::validate_ddl_collation(&option.StrValue)?;
            }
        }
        for column in &statement.Cols {
            Self::validate_ddl_collation(column.Tp.GetCollate())?;
            for option in &column.Options {
                if option.Tp == ast::ColumnOptionType::Collate {
                    Self::validate_ddl_collation(&option.StrValue)?;
                }
            }
        }
        Ok(())
    }

    fn validate_enum_set_lengths(columns: &[astersql_meta_model::ColumnInfo]) -> SessionResult<()> {
        if !astersql_config::get_global_config().enable_enum_length_limit {
            return Ok(());
        }
        for column in columns.iter().filter(|column| {
            matches!(
                column.GetType(),
                astersql_parser_mysql::r#type::TypeEnum | astersql_parser_mysql::r#type::TypeSet
            )
        }) {
            if column.GetElems().iter().any(|element| element.len() > 255) {
                return Err(SessionError::new(format!(
                    "[ddl:3505]Too long enumeration/set value for column {}.",
                    column.Name.O
                )));
            }
        }
        Ok(())
    }

    fn auto_random_bits_from_definition(
        definition: &ast::ColumnDef,
        default_range_bits: u64,
    ) -> SessionResult<Option<(u64, u64)>> {
        let Some(option) = definition
            .Options
            .iter()
            .rev()
            .find(|option| option.Tp == ast::ColumnOptionType::AutoRandom)
        else {
            return Ok(None);
        };
        if definition.Tp.GetType() != astersql_parser_mysql::r#type::TypeLonglong {
            return Err(SessionError::new(format!(
                "[ddl:8216]Invalid auto random: auto_random option must be defined on `bigint` \
                 column, but not on `{}` column",
                definition.Tp.CompactStr()
            )));
        }
        let shard_bits = match option.AutoRandOpt.ShardBits {
            astersql_meta_model::types::UnspecifiedLength => 5,
            0 => {
                return Err(SessionError::new(
                    "[ddl:8216]Invalid auto random: the value of auto_random should be positive",
                ));
            }
            bits if bits > 15 => {
                return Err(SessionError::new(format!(
                    "[ddl:8216]Invalid auto random: max allowed auto_random shard bits is 15, \
                     but got {bits} on column `{}`",
                    definition.Name.Name.O
                )));
            }
            bits => bits as u64,
        };
        let range_bits = match option.AutoRandOpt.RangeBits {
            astersql_meta_model::types::UnspecifiedLength => default_range_bits,
            bits if !(32..=64).contains(&bits) => {
                return Err(SessionError::new(format!(
                    "[ddl:8216]Invalid auto random: auto_random range bits must be between 32 and \
                     64, but got {bits}"
                )));
            }
            bits => bits as u64,
        };
        if range_bits.saturating_sub(shard_bits) < 27 {
            return Err(SessionError::new(
                "[ddl:8216]Invalid auto random: auto_random ID space is too small, please \
                 decrease the shard bits or increase the range bits",
            ));
        }
        if definition
            .Options
            .iter()
            .any(|option| option.Tp == ast::ColumnOptionType::AutoIncrement)
        {
            return Err(SessionError::new(
                "[ddl:8216]Invalid auto random: auto_random is incompatible with auto_increment",
            ));
        }
        if definition
            .Options
            .iter()
            .any(|option| option.Tp == ast::ColumnOptionType::DefaultValue)
        {
            return Err(SessionError::new(
                "[ddl:8216]Invalid auto random: auto_random is incompatible with default",
            ));
        }
        Ok(Some((shard_bits, range_bits)))
    }

    /// Go `checkTableForeignKeysValid` 在 CREATE TABLE 持久化前解析父表。
    ///
    /// 自引用外键以正在创建的表为父表；关闭 `foreign_key_checks` 时允许父表
    /// 暂不存在，其余情况必须在当前 Domain catalog 中找到父表。
    fn validate_create_table_foreign_key_parents(
        &self,
        database: &str,
        statement: &ast::CreateTableStmt,
    ) -> SessionResult<()> {
        if !self.state.borrow().foreign_key_checks {
            return Ok(());
        }
        for constraint in statement
            .Constraints
            .iter()
            .filter(|constraint| constraint.Tp == ast::ConstraintType::ForeignKey)
        {
            let reference = constraint
                .Refer
                .as_ref()
                .ok_or_else(|| SessionError::new("FOREIGN KEY has no REFERENCES clause"))?;
            let reference_database = if reference.Table.Schema.L.is_empty() {
                database
            } else {
                reference.Table.Schema.L.as_str()
            };
            let self_reference = reference_database.eq_ignore_ascii_case(database)
                && reference
                    .Table
                    .Name
                    .L
                    .eq_ignore_ascii_case(&statement.Table.Name.L);
            if !self_reference
                && self
                    .domain
                    .stats_table(reference_database, &reference.Table.Name.L)
                    .is_none()
            {
                return Err(SessionError::new(format!(
                    "[schema:1824]Failed to open the referenced table '{}'",
                    reference.Table.Name.O
                )));
            }
        }
        Ok(())
    }

    /// 执行 CREATE TABLE 并登记到 Domain。
    pub(super) fn execute_create_table(
        &self,
        statement: &ast::CreateTableStmt,
        shard_row_id_bits: Option<u64>,
        pre_split_regions: Option<u64>,
    ) -> SessionResult<()> {
        astersql_planner_core::InstallPlannerExpressionFactory()
            .map_err(|error| SessionError::new(error.to_string()))?;
        Self::validate_create_table_collations(statement)?;
        let strict_integer_display_width =
            unsafe { astersql_parser_types::TiDBStrictIntegerDisplayWidth };
        if strict_integer_display_width {
            for column in &statement.Cols {
                let integer = matches!(
                    column.Tp.GetType(),
                    astersql_parser_mysql::r#type::TypeTiny
                        | astersql_parser_mysql::r#type::TypeShort
                        | astersql_parser_mysql::r#type::TypeInt24
                        | astersql_parser_mysql::r#type::TypeLong
                        | astersql_parser_mysql::r#type::TypeLonglong
                );
                let explicit_width =
                    column.Tp.GetFlen() != astersql_meta_model::types::UnspecifiedLength;
                let bool_width = column.Tp.GetType() == astersql_parser_mysql::r#type::TypeTiny
                    && column.Tp.GetFlen() == 1;
                if integer && explicit_width && !bool_width {
                    self.state.borrow_mut().current_warnings.push(
                        SessionWarning::warning_with_code(
                            1681,
                            "Integer display width is deprecated and will be removed in a future release."
                                .to_owned(),
                        ),
                    );
                }
                if astersql_parser_mysql::r#type::HasZerofillFlag(column.Tp.GetFlag()) {
                    self.state.borrow_mut().current_warnings.push(
                        SessionWarning::warning_with_code(
                            1681,
                            "The ZEROFILL attribute is deprecated and will be removed in a future release. Use the LPAD function to zero-pad numbers, or store the formatted numbers in a CHAR column."
                                .to_owned(),
                        ),
                    );
                }
            }
        }
        if astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/ddl/mockExceedErrorLimit",
        ) {
            return Err(SessionError::new(
                "[ddl:-1]DDL job rollback, error msg: mock do job error",
            ));
        }
        if astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/ddl/checkOwnerCheckAllVersionsWaitTime",
        ) {
            return Err(SessionError::new(
                "[ddl:-1]DDL job rollback, error msg: owner check all versions timed out",
            ));
        }
        let database = if statement.Table.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Table.Schema.L.clone()
        };
        let database_exists = self.state.borrow().databases.contains(&database)
            || RUNTIME_DATABASES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&(Arc::as_ptr(&self.domain) as usize))
                .is_some_and(|databases| databases.contains(&database))
            || self
                .domain
                .ddl_database_names()
                .map_err(|error| session_error("read CREATE TABLE database metadata", error))?
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&database));
        if !database_exists {
            return Err(SessionError::new(format!(
                "[schema:1049]Unknown database '{}'",
                database
            )));
        }
        let is_local_temporary = statement.TemporaryKeyword == ast::TemporaryKeyword::Local;
        let already_exists = if is_local_temporary {
            self.local_temporary_table(&database, &statement.Table.Name.L)
                .is_some()
        } else {
            self.domain
                .stats_table(&database, &statement.Table.Name.L)
                .is_some()
        };
        if already_exists {
            if !statement.IfNotExists {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::error_with_code(
                        1050,
                        format!(
                            "Table '{}.{}' already exists",
                            database, statement.Table.Name.L
                        ),
                    ));
                return Err(SessionError::new(format!(
                    "table {}.{} already exists",
                    database, statement.Table.Name.L
                )));
            }
            self.state
                .borrow_mut()
                .current_warnings
                .push(SessionWarning {
                    level: "Note",
                    code: 1050,
                    message: format!(
                        "Table '{}.{}' already exists",
                        database, statement.Table.Name.L
                    ),
                });
            return Ok(());
        }
        let has_primary_key = statement.Cols.iter().any(|definition| {
            definition
                .Options
                .iter()
                .any(|option| option.Tp == ast::ColumnOptionType::PrimaryKey)
        }) || statement
            .Constraints
            .iter()
            .any(|constraint| constraint.Tp == ast::ConstraintType::PrimaryKey);
        if self.state.borrow().sql_require_primary_key && !has_primary_key {
            return Err(SessionError::new(
                "[ddl:3750]Unable to create or change a table without a primary key, when the \
                 system variable 'sql_require_primary_key' is set",
            ));
        }
        let vector_columns = statement
            .Cols
            .iter()
            .filter(|definition| {
                definition.Tp.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
            })
            .map(|definition| definition.Name.Name.L.clone())
            .collect::<HashSet<_>>();
        for definition in statement.Cols.iter().filter(|definition| {
            definition.Tp.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
        }) {
            let dimensions = definition.Tp.GetFlen() as i32;
            if dimensions != astersql_types::vector::UnspecifiedLength {
                astersql_types::vector::CheckVectorDimValid(dimensions)
                    .map_err(|error| SessionError::new(error.to_string()))?;
            }
            if definition.Options.iter().any(|option| {
                option.Tp == ast::ColumnOptionType::DefaultValue
                    && option.Expr.as_ref().is_some_and(|expression| {
                        matches!(expression.Kind, ast::ExprKind::Value(_))
                    })
            }) {
                return Err(SessionError::new(format!(
                    "VECTOR column '{}' can't have a literal default. \
                     Use expression default instead: ((VEC_FROM_TEXT('...')))",
                    definition.Name.Name.O
                )));
            }
        }
        if statement.Cols.iter().any(|definition| {
            vector_columns.contains(&definition.Name.Name.L)
                && definition.Options.iter().any(|option| {
                    matches!(
                        option.Tp,
                        ast::ColumnOptionType::PrimaryKey | ast::ColumnOptionType::UniqueKey
                    )
                })
        }) || statement.Constraints.iter().any(|constraint| {
            matches!(
                constraint.Tp,
                ast::ConstraintType::PrimaryKey
                    | ast::ConstraintType::Unique
                    | ast::ConstraintType::Index
            ) && constraint.Keys.iter().any(|key| {
                key.Column
                    .as_ref()
                    .is_some_and(|column| vector_columns.contains(&column.Name.L))
            })
        }) {
            return Err(SessionError::new(
                "only VECTOR INDEX can be added to vector column",
            ));
        }
        for constraint in statement
            .Constraints
            .iter()
            .filter(|constraint| constraint.Tp == ast::ConstraintType::Vector)
        {
            let mut referenced_columns = HashSet::new();
            for key in &constraint.Keys {
                if let Some(column) = key.Column.as_ref() {
                    referenced_columns.insert(column.Name.L.clone());
                }
                if let Some(expression) = key.Expr.as_ref() {
                    collect_expression_column_names(expression, &mut referenced_columns);
                }
            }
            if referenced_columns
                .iter()
                .any(|column| !vector_columns.contains(column))
            {
                return Err(SessionError::new(
                    "Unsupported add vector index: only support vector type",
                ));
            }
        }
        let mut table = if let Some(source) = statement.ReferTable.as_ref() {
            let source_database = if source.Schema.L.is_empty() {
                database.as_str()
            } else {
                source.Schema.L.as_str()
            };
            let source_table = self
                .resolve_runtime_table(source_database, &source.Name.L)
                .ok_or_else(|| {
                    SessionError::new(format!(
                        "unknown source table {source_database}.{}",
                        source.Name.L
                    ))
                })?;
            if source_table.View.is_some() || source_table.Sequence.is_some() {
                return Err(SessionError::new(format!(
                    "[schema:1347]'{}' is not BASE TABLE",
                    source.Name.O
                )));
            }
            if source_table.TempTableType != astersql_meta_model::TempTableNone {
                return Err(SessionError::new(
                    "[planner:8200]Unsupported create table like on temporary table",
                ));
            }
            if statement.TemporaryKeyword != ast::TemporaryKeyword::None {
                let unsupported = if source_table.AutoRandomBits != 0 {
                    Some("auto_random")
                } else if source_table.PreSplitRegions != 0 {
                    Some("pre split regions")
                } else if source_table.ShardRowIDBits != 0 {
                    Some("shard_row_id_bits")
                } else if source_table.Partition.is_some() {
                    return Err(SessionError::new(
                        "[ddl:1562]Cannot create temporary table with partitions",
                    ));
                } else if source_table.PlacementPolicyRef.is_some() {
                    Some("placement")
                } else {
                    None
                };
                if let Some(option) = unsupported {
                    return Err(SessionError::new(format!(
                        "[planner:8200]Unsupported {option} on temporary table"
                    )));
                }
            }
            let mut copied = source_table.clone();
            copied.Name = statement.Table.Name.clone();
            copied.DBID = 0;
            copied.ID = 0;
            if let Some(partition) = copied.Partition.as_mut() {
                for definition in partition
                    .Definitions
                    .iter_mut()
                    .chain(partition.AddingDefinitions.iter_mut())
                    .chain(partition.DroppingDefinitions.iter_mut())
                {
                    definition.ID = 0;
                }
                partition.NewPartitionIDs.clear();
            }
            // CREATE TABLE ... LIKE copies the column definition, not the
            // source table's allocator watermark.  A fresh table must start
            // AUTO_INCREMENT allocation from its own default seed.
            copied.AutoIncID = 0;
            copied.AutoIncIDExtra = 0;
            copied.AutoRandID = 0;
            copied.ForeignKeys.clear();
            copied.TempTableType = match statement.TemporaryKeyword {
                ast::TemporaryKeyword::None => astersql_meta_model::TempTableNone,
                ast::TemporaryKeyword::Global => astersql_meta_model::TempTableGlobal,
                ast::TemporaryKeyword::Local => astersql_meta_model::TempTableLocal,
            };
            copied
        } else {
            let state = self.state.borrow();
            let context =
                astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(vec![
                    astersql_meta_metabuild::WithClusteredIndexDefMode(
                        state.clustered_index_def_mode,
                    ),
                    astersql_meta_metabuild::WithShardRowIDBits(state.shard_row_id_bits),
                    astersql_meta_metabuild::WithPreSplitRegions(state.pre_split_regions),
                ]);
            drop(state);
            astersql_ddl::BuildTableInfoFromAST(&context, statement)
                .map_err(|error| session_error("build CREATE TABLE metadata", error))?
        };
        if table.PlacementPolicyRef.is_none()
            && statement.TemporaryKeyword == ast::TemporaryKeyword::None
            && let Some(policy) = RUNTIME_DATABASE_OPTIONS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&(Arc::as_ptr(&self.domain) as usize))
                .and_then(|databases| databases.get(&database.to_ascii_lowercase()))
                .and_then(|options| options.placement_policy.as_ref())
                .cloned()
        {
            table.PlacementPolicyRef = Some(astersql_meta_model::PolicyRefInfo {
                Name: ast::NewCIStr(&policy),
                ..Default::default()
            });
        }
        if strict_integer_display_width {
            for (column, definition) in table.Columns.iter_mut().zip(&statement.Cols) {
                let integer = matches!(
                    column.GetType(),
                    astersql_parser_mysql::r#type::TypeTiny
                        | astersql_parser_mysql::r#type::TypeShort
                        | astersql_parser_mysql::r#type::TypeInt24
                        | astersql_parser_mysql::r#type::TypeLong
                        | astersql_parser_mysql::r#type::TypeLonglong
                );
                let bool_width = column.GetType() == astersql_parser_mysql::r#type::TypeTiny
                    && column.GetFlen() == 1;
                if integer
                    && definition.Tp.GetFlen() != astersql_meta_model::types::UnspecifiedLength
                    && !bool_width
                    && !astersql_parser_mysql::r#type::HasZerofillFlag(column.GetFlag())
                {
                    column.SetFlen(astersql_meta_model::types::UnspecifiedLength);
                }
            }
        }
        if let Some(value) = shard_row_id_bits {
            table.ShardRowIDBits = value;
            table.MaxShardRowIDBits = value;
        }
        if let Some(value) = pre_split_regions {
            table.PreSplitRegions = value;
        }
        let requested_shard_bits = shard_row_id_bits.unwrap_or_else(|| {
            statement
                .Options
                .iter()
                .rev()
                .find(|option| option.Tp == ast::TableOptionType::ShardRowID)
                .map_or(0, |option| option.UintValue)
        });
        if requested_shard_bits != 0 && table.HasClusteredIndex() {
            return Err(SessionError::new(
                "[ddl:8200]Unsupported shard_row_id_bits for table with primary key as row id",
            ));
        }
        Self::validate_enum_set_lengths(&table.Columns)?;
        self.validate_create_table_foreign_key_parents(&database, statement)?;
        self.validate_create_table_index_lengths(&mut table)?;
        // Domain lookup is case-insensitive, while INFORMATION_SCHEMA exposes
        // the canonical lower-case identifier used by the Go session layer.
        table.Name = ast::NewCIStr(&statement.Table.Name.L);
        if is_local_temporary {
            // Go local temporary tables live in SessionExtendedInfoSchema rather
            // than the shared InfoSchema. A process-wide high ID gives the
            // existing relational KV executor a non-colliding keyspace.
            table.ID = NEXT_RUNTIME_LOCAL_TEMPORARY_TABLE_ID.fetch_add(1, Ordering::AcqRel);
            self.state.borrow_mut().local_temporary_tables.insert(
                (
                    database.to_ascii_lowercase(),
                    statement.Table.Name.L.clone(),
                ),
                table,
            );
            return Ok(());
        }
        if let Some(system_table_id) = Self::ddl_system_table_id(&database, &statement.Table.Name.L)
        {
            table.ID = system_table_id;
        }
        let auto_random_available = table.ContainsAutoRandomBits().then(|| {
            let sign_bits = if table.IsAutoRandomBitColUnsigned() {
                0
            } else {
                1
            };
            let allocation_bits = table
                .AutoRandomRangeBits
                .saturating_sub(table.AutoRandomBits)
                .saturating_sub(sign_bits);
            if allocation_bits >= 64 {
                u64::MAX
            } else {
                (1_u64 << allocation_bits) - 1
            }
        });
        let job_id = begin_runtime_ddl_job(
            &self.domain,
            &database,
            &statement.Table.Name.L,
            "create table",
        );
        let job_guard = RuntimeDdlJobGuard::new(job_id);
        let result = (|| {
            self.domain
                .ddl_create_table(&database, table, statement.IfNotExists)
                .map_err(|error| session_error("persist CREATE TABLE metadata", error))?;
            self.pre_split_and_scatter(&database, &statement.Table.Name.L)?;
            self.update_self_version_with_retry()?;
            self.domain.record_cross_keyspace_ddl(&database, false);
            if let Some(available) = auto_random_available {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::note(format!(
                        "Available implicit allocation times: {available}"
                    )));
            }
            Ok(())
        })();
        job_guard.finish(&result);
        result
    }

    /// Go `preSplitAndScatter`: expose the effective session scope at the
    /// production boundary before deriving the table's physical regions.
    pub(super) fn pre_split_and_scatter(&self, database: &str, table: &str) -> SessionResult<()> {
        let (_, table_info) = self
            .domain
            .stats_table(database, table)
            .ok_or_else(|| SessionError::new(format!("unknown table {database}.{table}")))?;
        let scope = self.state.borrow().scatter_region.clone();
        const FAILPOINT: &str = "github.com/pingcap/tidb/pkg/ddl/preSplitAndScatter";
        astersql_testkit_testfailpoint::inject_value(FAILPOINT, &scope);
        astersql_testkit_testfailpoint::inject(FAILPOINT);

        if table_info.TempTableType != astersql_meta_model::TempTableNone {
            return Ok(());
        }
        if astersql_ddl::EnableSplitTableRegion.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }
        let split_bits = u32::try_from(table_info.PreSplitRegions.min(20)).unwrap_or_default();
        RUNTIME_REGION_COUNTS
            .lock()
            .expect("runtime region-count map poisoned")
            .insert(
                (
                    runtime_domain_id(&self.domain),
                    database.to_owned(),
                    table.to_owned(),
                    None,
                ),
                1usize << split_bits,
            );
        Ok(())
    }

    /// Updating this TiDB node's schema version is an etcd operation retried by
    /// the DDL worker. The failpoint injects failures only for that key; after
    /// its configured occurrences are consumed the same DDL succeeds.
    pub(super) fn update_self_version_with_retry(&self) -> SessionResult<()> {
        const FAILPOINT: &str = "github.com/pingcap/tidb/pkg/ddl/util/PutKVToEtcdError";
        for _ in 0..10 {
            if astersql_testkit_testfailpoint::eval_bool(FAILPOINT) {
                astersql_testkit_testfailpoint::inject_value(FAILPOINT, "retry");
                continue;
            }
            return Ok(());
        }
        Err(SessionError::new(
            "update self schema version failed after 10 retries",
        ))
    }

    /// Go `PlanBuilder::getFullAnalyzeColumnsInfo` followed by
    /// `PlanBuilder::filterSkipColumnTypes`: decides which columns an ANALYZE
    /// collects. Columns are returned in table definition order.
    /// 执行 DROP TABLE。
    pub(super) fn execute_drop_table(&self, statement: &ast::DropTableStmt) -> SessionResult<()> {
        let current_database = self.current_database();
        if current_database.is_empty()
            && statement
                .Tables
                .iter()
                .any(|table| table.Schema.L.is_empty())
        {
            return Err(SessionError::new(
                "ERROR 1046 (3D000): No database selected",
            ));
        }
        let mut shared_tables = Vec::new();
        for table in &statement.Tables {
            let database = if table.Schema.L.is_empty() {
                current_database.clone()
            } else {
                table.Schema.L.clone()
            };
            if let Some(local) = self.local_temporary_table(&database, &table.Name.L) {
                self.clear_local_temporary_table_data(&local)?;
                let mut state = self.state.borrow_mut();
                state
                    .local_temporary_tables
                    .remove(&(database.to_ascii_lowercase(), table.Name.L.clone()));
                state
                    .local_temporary_auto_ids
                    .retain(|(table_id, _), _| *table_id != local.ID);
            } else {
                if statement.IfExists && self.domain.stats_table(&database, &table.Name.L).is_none()
                {
                    self.state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning::note_with_code(
                            1051,
                            format!("Unknown table '{}.{}'", database, table.Name.O),
                        ));
                }
                shared_tables.push((database, table.Name.L.clone()));
            }
        }
        if shared_tables.is_empty() {
            return Ok(());
        }
        if self.persistent_actions_enabled() {
            let identifiers:Vec<_>=shared_tables.iter().map(|(db,name)|serde_json::json!({"Schema":ast::NewCIStr(db),"Name":ast::NewCIStr(name)})).collect();
            for (database, table) in shared_tables {
                if statement.IfExists && self.domain.table_by_name(&database, &table).is_err() {
                    continue;
                }
                self.submit_normal_action(&database,&table,4,serde_json::json!({"fk_check":self.state.borrow().foreign_key_checks,"identifiers":identifiers}))?;
            }
            return Ok(());
        }
        let job_table = shared_tables
            .first()
            .map(|(_, table)| table.as_str())
            .unwrap_or_default();
        let job_database = shared_tables
            .first()
            .map(|(database, _)| database.as_str())
            .unwrap_or_default();
        let job_id = begin_runtime_ddl_job(&self.domain, job_database, job_table, "drop table");
        let job_guard = RuntimeDdlJobGuard::new(job_id);
        let old_tables = shared_tables
            .iter()
            .filter_map(|(database, table)| {
                self.domain
                    .stats_table(database, table)
                    .map(|(_, info)| (database.clone(), info))
            })
            .collect();
        attach_runtime_ddl_snapshot(job_id, old_tables);
        let result = self
            .domain
            .ddl_drop_tables(shared_tables, statement.IfExists)
            .map_err(|error| session_error("persist DROP TABLE metadata", error))
            .and_then(|_| self.update_self_version_with_retry());
        job_guard.finish(&result);
        result
    }

    /// Execute Go's atomic `RENAME TABLE` path while preserving physical IDs.
    pub(super) fn execute_rename_table_pairs(
        &self,
        pairs: &[ast::TableToTable],
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        let catalog = self.domain.stats_context().catalog();
        let runtime_databases = RUNTIME_DATABASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&domain_id)
            .cloned()
            .unwrap_or_default();
        let session_databases = self.state.borrow().databases.clone();
        let mut renames = Vec::with_capacity(pairs.len());
        for pair in pairs {
            let old_database = if pair.OldTable.Schema.L.is_empty() {
                current_database.clone()
            } else {
                pair.OldTable.Schema.L.clone()
            };
            let new_database = if pair.NewTable.Schema.L.is_empty() {
                current_database.clone()
            } else {
                pair.NewTable.Schema.L.clone()
            };
            let destination_exists = session_databases.contains(&new_database)
                || runtime_databases.contains(&new_database)
                || catalog
                    .keys()
                    .any(|(database, _)| database == &new_database);
            if !destination_exists {
                return Err(SessionError::new(format!(
                    "unknown database {new_database}"
                )));
            }
            renames.push((
                old_database,
                pair.OldTable.Name.L.clone(),
                new_database,
                pair.NewTable.Name.O.clone(),
            ));
        }
        if self.persistent_actions_enabled() {
            let schemas = self.domain.info_schema().AllSchemas();
            let mut names = std::collections::HashMap::new();
            let mut infos = Vec::new();
            let mut first = None;
            for (old_db, old_name, new_db, new_name) in &renames {
                let old_id = schemas
                    .iter()
                    .find(|db| db.name.lower == *old_db)
                    .ok_or_else(|| SessionError::new(format!("unknown database {old_db}")))?
                    .id;
                let new_id = schemas
                    .iter()
                    .find(|db| db.name.lower == *new_db)
                    .ok_or_else(|| SessionError::new(format!("unknown database {new_db}")))?
                    .id;
                let key = (old_db.clone(), old_name.clone());
                let info = match names.remove(&key) {
                    Some(info) => info,
                    None => self
                        .domain
                        .table_by_name(old_db, old_name)
                        .map_err(|e| SessionError::new(e.to_string()))?,
                };
                if first.is_none() {
                    first = Some((old_db.clone(), old_name.clone()));
                }
                infos.push(serde_json::json!({"old_schema_id":old_id,"new_schema_id":new_id,"old_schema_name":ast::NewCIStr(old_db),"old_table_name":ast::NewCIStr(old_name),"new_table_name":ast::NewCIStr(new_name),"table_id":info.ID}));
                names.insert((new_db.clone(), new_name.to_ascii_lowercase()), info);
            }
            let (db, table) = first.ok_or_else(|| SessionError::new("empty rename list"))?;
            if infos.len() == 1 {
                return self.submit_normal_action(&db, &table, 14, infos.remove(0));
            }
            // Multi-rename carries the whole ordered chain in one owner transaction.
            return self.submit_normal_action(
                &db,
                &table,
                47,
                serde_json::json!({"rename_table_infos":infos}),
            );
        }
        self.domain
            .ddl_rename_tables(renames)
            .map_err(|error| session_error("persist RENAME TABLE metadata", error))?;
        self.update_self_version_with_retry()
    }

    /// Execute the standalone `RENAME TABLE` statement.
    pub(super) fn execute_rename_table(
        &self,
        statement: &ast::RenameTableStmt,
    ) -> SessionResult<()> {
        self.execute_rename_table_pairs(&statement.TableToTables)
    }

    /// Execute CREATE DATABASE and make the schema visible to subsequent USE.
    pub(super) fn execute_create_database(
        &self,
        statement: &ast::CreateDatabaseStmt,
    ) -> SessionResult<()> {
        for option in &statement.Options {
            if option.Tp == ast::DatabaseOptionType::Collate {
                Self::validate_ddl_collation(&option.Value)?;
            }
        }
        let explicit_charset = statement
            .Options
            .iter()
            .rev()
            .find(|option| option.Tp == ast::DatabaseOptionType::Charset)
            .map(|option| option.Value.to_ascii_lowercase());
        let explicit_collation = statement
            .Options
            .iter()
            .rev()
            .find(|option| option.Tp == ast::DatabaseOptionType::Collate)
            .map(|option| option.Value.to_ascii_lowercase());
        let placement_policy = statement
            .Options
            .iter()
            .rev()
            .find(|option| option.Tp == ast::DatabaseOptionType::Policy)
            .map(|option| option.Value.clone());
        if let Some(policy) = placement_policy.as_ref() {
            let exists = RUNTIME_PLACEMENT_POLICIES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&runtime_domain_id(&self.domain))
                .is_some_and(|policies| policies.contains_key(&policy.to_ascii_lowercase()));
            if !exists {
                return Err(SessionError::new(format!(
                    "[schema:8239]Unknown placement policy '{policy}'"
                )));
            }
        }
        let collation_charset = explicit_collation
            .as_deref()
            .and_then(Self::ddl_collation_charset);
        if let (Some(charset), Some(collation), Some(expected_charset)) = (
            explicit_charset.as_deref(),
            explicit_collation.as_deref(),
            collation_charset,
        ) && charset != expected_charset
        {
            return Err(SessionError::new(format!(
                "[ddl:1253]COLLATION '{collation}' is not valid for CHARACTER SET '{charset}'"
            )));
        }
        let charset = explicit_charset
            .or_else(|| collation_charset.map(str::to_owned))
            .or_else(|| {
                self.session_vars
                    .GetSystemVar(astersql_sessionctx_vardef::CharacterSetServer)
            })
            .unwrap_or_else(|| "utf8mb4".to_owned());
        let name = statement.Name.to_ascii_lowercase();
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        // SessionState includes built-in names so `USE mysql/test` works before
        // bootstrap, but those placeholders are not persisted databases yet.
        let exists = RUNTIME_DATABASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&domain_id)
            .is_some_and(|databases| databases.contains(&name))
            || self
                .domain
                .stats_context()
                .catalog()
                .keys()
                .any(|(database, _)| database == &name);
        if exists {
            if statement.IfNotExists {
                return Ok(());
            }
            return Err(SessionError::new(format!(
                "Can't create database '{}'; database exists",
                statement.Name
            )));
        }
        let preferred_id = astersql_config_kerneltype::IsNextGen()
            .then(|| match name.as_str() {
                "mysql" => Some(astersql_meta_metadef::SystemDatabaseID),
                "sys" => Some(astersql_meta_metadef::SysDatabaseID),
                _ => None,
            })
            .flatten();
        self.domain
            .ddl_create_database_with_id(&name, statement.IfNotExists, preferred_id)
            .map_err(|error| session_error("persist CREATE DATABASE metadata", error))?;
        RUNTIME_DATABASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(domain_id)
            .or_default()
            .insert(name.clone());
        RUNTIME_DATABASE_OPTIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(domain_id)
            .or_default()
            .insert(
                name.clone(),
                RuntimeDatabaseOptions {
                    charset,
                    explicit_collation,
                    placement_policy,
                },
            );
        self.state.borrow_mut().databases.insert(name);
        Ok(())
    }

    pub(super) fn execute_create_sequence(
        &self,
        statement: &ast::CreateSequenceStmt,
    ) -> SessionResult<()> {
        let database = if statement.Name.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Name.Schema.L.clone()
        };
        if database.is_empty() {
            return Err(SessionError::new(
                "ERROR 1046 (3D000): No database selected",
            ));
        }
        let options = statement
            .SeqOptions
            .iter()
            .map(|option| match option.Tp {
                ast::SequenceOptionType::IncrementBy => Ok(
                    astersql_ddl::sequence::SequenceOption::Increment(option.IntValue),
                ),
                ast::SequenceOptionType::StartWith => Ok(
                    astersql_ddl::sequence::SequenceOption::Start(option.IntValue),
                ),
                ast::SequenceOptionType::NoMinValue => {
                    Ok(astersql_ddl::sequence::SequenceOption::NoMinValue)
                }
                ast::SequenceOptionType::MinValue => Ok(
                    astersql_ddl::sequence::SequenceOption::MinValue(option.IntValue),
                ),
                ast::SequenceOptionType::NoMaxValue => {
                    Ok(astersql_ddl::sequence::SequenceOption::NoMaxValue)
                }
                ast::SequenceOptionType::MaxValue => Ok(
                    astersql_ddl::sequence::SequenceOption::MaxValue(option.IntValue),
                ),
                ast::SequenceOptionType::NoCache => {
                    Ok(astersql_ddl::sequence::SequenceOption::NoCache)
                }
                ast::SequenceOptionType::Cache if option.IntValue > 0 => Ok(
                    astersql_ddl::sequence::SequenceOption::Cache(option.IntValue as u64),
                ),
                ast::SequenceOptionType::Cycle => {
                    Ok(astersql_ddl::sequence::SequenceOption::Cycle(true))
                }
                ast::SequenceOptionType::NoCycle => {
                    Ok(astersql_ddl::sequence::SequenceOption::Cycle(false))
                }
                ast::SequenceOptionType::None => {
                    Err(SessionError::new("invalid empty CREATE SEQUENCE option"))
                }
                _ => Err(SessionError::new("invalid CREATE SEQUENCE option")),
            })
            .collect::<SessionResult<Vec<_>>>()?;
        let sequence = astersql_ddl::sequence::build_sequence_info(&options)
            .map_err(|error| SessionError::new(format!("invalid sequence options: {error}")))?;
        let table = astersql_meta_model::TableInfo {
            Name: ast::NewCIStr(&statement.Name.Name.L),
            State: astersql_meta_model::StatePublic,
            Charset: "utf8mb4".to_owned(),
            Collate: "utf8mb4_bin".to_owned(),
            Sequence: Some(astersql_meta_model::SequenceInfo {
                Start: sequence.start,
                Cache: sequence.cache > 1,
                Cycle: sequence.cycle,
                MinValue: sequence.min_value,
                MaxValue: sequence.max_value,
                Increment: sequence.increment,
                CacheValue: sequence.cache as i64,
                Comment: sequence.comment,
            }),
            ..Default::default()
        };
        self.domain
            .ddl_create_table(&database, table, statement.IfNotExists)
            .map_err(|error| session_error("persist CREATE SEQUENCE metadata", error))?;
        Ok(())
    }

    pub(super) fn execute_drop_sequence(
        &self,
        statement: &ast::DropSequenceStmt,
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        let mut sequences = Vec::with_capacity(statement.Sequences.len());
        for sequence in &statement.Sequences {
            let database = if sequence.Schema.L.is_empty() {
                current_database.clone()
            } else {
                sequence.Schema.L.clone()
            };
            if database.is_empty() {
                return Err(SessionError::new(
                    "ERROR 1046 (3D000): No database selected",
                ));
            }
            if let Some((_, table)) = self.domain.stats_table(&database, &sequence.Name.L)
                && table.Sequence.is_none()
            {
                return Err(SessionError::new(format!(
                    "'{}.{}' is not SEQUENCE",
                    database, sequence.Name.O
                )));
            }
            sequences.push((database, sequence.Name.L.clone()));
        }
        self.domain
            .ddl_drop_tables(sequences, statement.IfExists)
            .map_err(|error| session_error("persist DROP SEQUENCE metadata", error))?;
        Ok(())
    }

    pub(super) fn execute_alter_database(
        &self,
        statement: &ast::AlterDatabaseStmt,
    ) -> SessionResult<()> {
        let name = if statement.AlterDefaultDatabase || statement.Name.L.is_empty() {
            self.current_database()
        } else {
            statement.Name.L.clone()
        };
        if !self.state.borrow().databases.contains(&name)
            && !RUNTIME_DATABASES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&(Arc::as_ptr(&self.domain) as usize))
                .is_some_and(|databases| databases.contains(&name))
        {
            return Err(SessionError::new(format!("unknown database {name}")));
        }
        for option in &statement.Options {
            if option.Tp == ast::DatabaseOptionType::Collate {
                Self::validate_ddl_collation(&option.Value)?;
            }
        }
        Ok(())
    }

    /// Select a database previously created in this session.
    pub(super) fn execute_use_database(&self, statement: &ast::UseStmt) -> SessionResult<()> {
        let name = statement.DBName.to_ascii_lowercase();
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        let snapshot_read_ts = self.state.borrow().snapshot_read_ts;
        let snapshot_database_exists = snapshot_read_ts.is_some_and(|read_ts| {
            self.state
                .borrow()
                .tso_database_names
                .range(..=read_ts)
                .next_back()
                .is_some_and(|(_, databases)| databases.contains(&name))
        });
        let globally_exists = RUNTIME_DATABASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&domain_id)
            .is_some_and(|databases| databases.contains(&name))
            || self
                .domain
                .stats_context()
                .catalog()
                .keys()
                .any(|(database, _)| database == &name)
            || self
                .domain
                .info_schema()
                .AllSchemas()
                .iter()
                .any(|database| database.name.lower == name)
            || snapshot_database_exists
            || snapshot_read_ts.is_some_and(|read_ts| {
                self.domain
                    .snapshot_info_schema(read_ts)
                    .is_ok_and(|schema| {
                        schema
                            .AllSchemas()
                            .iter()
                            .any(|database| database.name.lower == name)
                    })
            });
        let mut state = self.state.borrow_mut();
        let built_in = matches!(
            name.as_str(),
            "test"
                | "mysql"
                | "information_schema"
                | "performance_schema"
                | "sys"
                | "metrics_schema"
        );
        if !built_in && !globally_exists {
            return Err(SessionError::new(format!(
                "unknown database {}",
                statement.DBName
            )));
        }
        state.databases.insert(name.clone());
        state.current_database = name.clone();
        self.session_vars.SetCurrentDB(name);
        Ok(())
    }

    /// 执行 DROP DATABASE。
    pub(super) fn execute_drop_database(
        &self,
        statement: &ast::DropDatabaseStmt,
    ) -> SessionResult<()> {
        let name = statement.Name.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "mysql" | "information_schema" | "performance_schema" | "sys" | "metrics_schema"
        ) {
            return Err(SessionError::new(format!(
                "Can't drop database '{}'; database is protected",
                statement.Name
            )));
        }
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        let state_exists = self.state.borrow().databases.contains(&name);
        let global_exists = RUNTIME_DATABASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&domain_id)
            .is_some_and(|databases| databases.contains(&name));
        let catalog_exists = self
            .domain
            .stats_context()
            .catalog()
            .keys()
            .any(|(database, _)| database.eq_ignore_ascii_case(&name));
        let metadata_exists = self
            .domain
            .ddl_database_names()
            .map_err(|error| session_error("read database metadata", error))?
            .iter()
            .any(|database| database.eq_ignore_ascii_case(&name));
        if !state_exists && !global_exists && !catalog_exists && !metadata_exists {
            if statement.IfExists {
                return Ok(());
            }
            return Err(SessionError::new(format!(
                "Can't drop database '{}'; database doesn't exist",
                statement.Name
            )));
        }
        let job_id = begin_runtime_ddl_job(&self.domain, &name, "", "drop schema");
        let job_guard = RuntimeDdlJobGuard::new(job_id);
        let old_tables = self
            .domain
            .stats_context()
            .catalog()
            .into_iter()
            .filter(|((database, _), _)| database.eq_ignore_ascii_case(&name))
            .map(|((database, _), (_, table))| (database, table))
            .collect();
        attach_runtime_ddl_snapshot(job_id, old_tables);
        let result = self
            .domain
            .ddl_drop_database(&name, true)
            .map_err(|error| session_error("persist DROP DATABASE metadata", error));
        if result.is_err() {
            job_guard.finish(&result);
            return result;
        }
        RUNTIME_DATABASES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(domain_id)
            .or_default()
            .remove(&name);
        RUNTIME_DATABASE_OPTIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(domain_id)
            .or_default()
            .remove(&name);
        let mut state = self.state.borrow_mut();
        state.databases.remove(&name);
        if state.current_database.eq_ignore_ascii_case(&name) {
            state.current_database.clear();
        }
        drop(state);
        let result = Ok(());
        job_guard.finish(&result);
        result
    }
    /// Validate existing rows before publishing a UNIQUE index.
    pub(super) fn validate_unique_index_rows(
        &self,
        database: &str,
        table_name: &str,
        constraint: &ast::Constraint,
    ) -> SessionResult<()> {
        let (_, table) = self
            .domain
            .stats_table(database, table_name)
            .ok_or_else(|| SessionError::new(format!("unknown table {database}.{table_name}")))?;
        let columns = constraint
            .Keys
            .iter()
            .map(|part| {
                part.Column
                    .as_ref()
                    .map(|column| column.Name.L.clone())
                    .ok_or_else(|| {
                        SessionError::new(
                            "UNIQUE expression index validation requires a materialized column",
                        )
                    })
            })
            .collect::<SessionResult<Vec<_>>>()?;
        let mut seen = HashSet::new();
        for (_, row) in self.scan_registered_table(&table)? {
            let values = columns
                .iter()
                .map(|column| row.get(column).cloned().flatten())
                .collect::<Vec<_>>();
            if values.iter().any(Option::is_none) {
                if constraint.Tp == ast::ConstraintType::PrimaryKey {
                    return Err(SessionError::new("[ddl:1138]Invalid use of NULL value"));
                }
                continue;
            }
            let key = values
                .into_iter()
                .map(Option::unwrap)
                .collect::<Vec<_>>()
                .join("-");
            if !seen.insert(key.clone()) {
                return Err(SessionError::new(format!(
                    "[kv:1062]Duplicate entry '{key}' for key '{table_name}.{}'",
                    constraint.Name
                )));
            }
        }
        Ok(())
    }

    /// Add a foreign key after checking every existing child row.
    pub(super) fn execute_add_foreign_key(
        &self,
        database: &str,
        table_name: &str,
        constraint: &ast::Constraint,
    ) -> SessionResult<()> {
        let reference = constraint
            .Refer
            .as_ref()
            .ok_or_else(|| SessionError::new("FOREIGN KEY has no REFERENCES clause"))?;
        let reference_database = if reference.Table.Schema.L.is_empty() {
            database
        } else {
            reference.Table.Schema.L.as_str()
        };
        let (_, child_table) = self
            .domain
            .stats_table(database, table_name)
            .ok_or_else(|| SessionError::new(format!("unknown table {database}.{table_name}")))?;
        let (_, parent_table) = self
            .domain
            .stats_table(reference_database, &reference.Table.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!(
                    "Cannot open referenced table '{}'",
                    reference.Table.Name.O
                ))
            })?;
        let child_columns = constraint
            .Keys
            .iter()
            .map(|part| {
                part.Column
                    .as_ref()
                    .map(|column| column.Name.clone())
                    .ok_or_else(|| SessionError::new("FOREIGN KEY expression is unsupported"))
            })
            .collect::<SessionResult<Vec<_>>>()?;
        let parent_columns = reference
            .IndexPartSpecifications
            .iter()
            .map(|part| {
                part.Column
                    .as_ref()
                    .map(|column| column.Name.clone())
                    .ok_or_else(|| {
                        SessionError::new("FOREIGN KEY referenced expression is unsupported")
                    })
            })
            .collect::<SessionResult<Vec<_>>>()?;
        if child_columns.is_empty() || child_columns.len() != parent_columns.len() {
            return Err(SessionError::new("Cannot add foreign key constraint"));
        }
        let parent_keys = self
            .scan_registered_table(&parent_table)?
            .into_iter()
            .filter_map(|(_, row)| {
                parent_columns
                    .iter()
                    .map(|column| row.get(&column.L).cloned().flatten())
                    .collect::<Option<Vec<_>>>()
            })
            .collect::<HashSet<_>>();
        let violates = self
            .scan_registered_table(&child_table)?
            .into_iter()
            .filter_map(|(_, row)| {
                child_columns
                    .iter()
                    .map(|column| row.get(&column.L).cloned().flatten())
                    .collect::<Option<Vec<_>>>()
            })
            .any(|key| !parent_keys.contains(&key));
        astersql_testkit_testfailpoint::inject(
            "github.com/pingcap/tidb/pkg/ddl/afterCheckForeignKeyConstrain",
        );
        if violates {
            let child = child_columns
                .iter()
                .map(|column| format!("`{}`", column.O))
                .collect::<Vec<_>>()
                .join(", ");
            let parent = parent_columns
                .iter()
                .map(|column| format!("`{}`", column.O))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(SessionError::new(format!(
                "[ddl:1452]Cannot add or update a child row: a foreign key constraint fails \
                 (`{database}`.`{table_name}`, CONSTRAINT `{}` FOREIGN KEY ({child}) \
                 REFERENCES `{}` ({parent}))",
                constraint.Name, reference.Table.Name.O
            )));
        }

        let index_exists = child_table.Indices.iter().any(|index| {
            index.Columns.len() >= child_columns.len()
                && index
                    .Columns
                    .iter()
                    .zip(&child_columns)
                    .all(|(indexed, child)| indexed.Name.L == child.L)
        });
        if !index_exists {
            let columns = child_columns
                .iter()
                .map(|column| {
                    let offset = child_table
                        .Columns
                        .iter()
                        .position(|candidate| candidate.Name.L == column.L)
                        .ok_or_else(|| {
                            SessionError::new(format!("unknown foreign key column '{}'", column.O))
                        })?;
                    Ok(astersql_meta_model::IndexColumn {
                        Name: column.clone(),
                        Offset: offset as isize,
                        Length: astersql_meta_model::types::UnspecifiedLength,
                        ..Default::default()
                    })
                })
                .collect::<SessionResult<Vec<_>>>()?;
            self.domain
                .ddl_add_index(
                    database,
                    table_name,
                    astersql_meta_model::IndexInfo {
                        Name: ast::NewCIStr(&constraint.Name),
                        Table: child_table.Name.clone(),
                        Columns: columns,
                        State: astersql_meta_model::StatePublic,
                        ..Default::default()
                    },
                )
                .map_err(|error| session_error("ALTER TABLE ADD FOREIGN KEY index", error))?;
        }

        let (_, latest) = self
            .domain
            .stats_table(database, table_name)
            .ok_or_else(|| SessionError::new(format!("unknown table {database}.{table_name}")))?;
        let mut foreign_keys = latest.ForeignKeys.clone();
        if foreign_keys
            .iter()
            .any(|foreign_key| foreign_key.Name.L == constraint.Name.to_ascii_lowercase())
        {
            return Err(SessionError::new(format!(
                "Duplicate foreign key constraint name '{}'",
                constraint.Name
            )));
        }
        foreign_keys.push(astersql_meta_model::FKInfo {
            ID: latest.MaxForeignKeyID.saturating_add(1),
            Name: ast::NewCIStr(&constraint.Name),
            RefSchema: ast::NewCIStr(reference_database),
            RefTable: reference.Table.Name.clone(),
            RefCols: parent_columns,
            Cols: child_columns,
            OnDelete: reference.OnDelete.ReferOpt as i32,
            OnUpdate: reference.OnUpdate.ReferOpt as i32,
            State: astersql_meta_model::StatePublic,
            Version: astersql_meta_model::FKVersion1,
        });
        self.domain
            .ddl_replace_foreign_keys(database, table_name, foreign_keys)
            .map_err(|error| session_error("ALTER TABLE ADD FOREIGN KEY", error))
    }

    /// Drop one foreign key while retaining its supporting index.
    pub(super) fn execute_drop_foreign_key(
        &self,
        database: &str,
        table_name: &str,
        name: &str,
    ) -> SessionResult<()> {
        let (_, table) = self
            .domain
            .stats_table(database, table_name)
            .ok_or_else(|| SessionError::new(format!("unknown table {database}.{table_name}")))?;
        let before = table.ForeignKeys.len();
        let mut foreign_keys = table.ForeignKeys.clone();
        foreign_keys.retain(|foreign_key| !foreign_key.Name.L.eq_ignore_ascii_case(name));
        if foreign_keys.len() == before {
            return Err(SessionError::new(format!(
                "Can't DROP FOREIGN KEY `{name}`; check that it exists"
            )));
        }
        self.domain
            .ddl_replace_foreign_keys(database, table_name, foreign_keys)
            .map_err(|error| session_error("ALTER TABLE DROP FOREIGN KEY", error))
    }

    /// 执行 ALTER TABLE（含统计相关变更）。
    pub(super) fn execute_alter_table(&self, statement: &ast::AlterTableStmt) -> SessionResult<()> {
        astersql_planner_core::InstallPlannerExpressionFactory()
            .map_err(|error| SessionError::new(error.to_string()))?;
        for spec in &statement.Specs {
            for option in &spec.Options {
                if option.Tp == ast::TableOptionType::Collate {
                    Self::validate_ddl_collation(&option.StrValue)?;
                }
            }
            for column in &spec.NewColumns {
                Self::validate_ddl_collation(column.Tp.GetCollate())?;
                let charset = column.Tp.GetCharset().to_ascii_lowercase();
                let collation = column.Tp.GetCollate().to_ascii_lowercase();
                if !charset.is_empty()
                    && !collation.is_empty()
                    && Self::ddl_collation_charset(&collation)
                        .is_some_and(|expected| expected != charset)
                {
                    return Err(SessionError::new(format!(
                        "[ddl:1253]COLLATION '{collation}' is not valid for CHARACTER SET '{charset}'"
                    )));
                }
                for option in &column.Options {
                    if option.Tp == ast::ColumnOptionType::Collate {
                        Self::validate_ddl_collation(&option.StrValue)?;
                    }
                }
            }
        }
        let database = if statement.Table.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Table.Schema.L.clone()
        };
        let table = statement.Table.Name.L.clone();
        if let Some((_, object)) = self.domain.stats_table(&database, &table)
            && (object.View.is_some() || object.Sequence.is_some())
        {
            return Err(SessionError::new(format!(
                "[schema:1347]'{}.{}' is not BASE TABLE",
                database, statement.Table.Name.O
            )));
        }
        // COMPRESSION='NONE' has no DDL job or metadata side effect.
        // Keep the existing option validation and table lookup in the inner path.
        if !statement.Specs.is_empty()
            && statement.Specs.iter().all(|spec| {
                spec.Tp == ast::AlterTableType::Option
                    && !spec.Options.is_empty()
                    && spec
                        .Options
                        .iter()
                        .all(|option| option.Tp == ast::TableOptionType::Compression)
            })
        {
            return self.execute_alter_table_inner(statement, 0);
        }
        if statement
            .Specs
            .iter()
            .any(|spec| spec.Tp == ast::AlterTableType::ModifyColumn)
            && let Some((_, info)) = self.domain.stats_table(&database, &table)
        {
            let global_limit = RUNTIME_GLOBAL_TXN_ENTRY_SIZE_LIMITS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&runtime_domain_id(&self.domain))
                .copied()
                .unwrap_or(DEFAULT_TXN_ENTRY_SIZE_LIMIT);
            let flags = self.dml_type_flags();
            let oversized = self
                .scan_registered_table(&info)?
                .into_iter()
                .map(|(_, row)| encode_relational_row(&info, &row, flags))
                .collect::<SessionResult<Vec<_>>>()?
                .into_iter()
                .any(|(key, value)| key.0.len() + value.len() > global_limit);
            if oversized {
                return Err(SessionError::new(format!(
                    "[kv:8025]entry too large, the max entry size is {global_limit}"
                )));
            }
        }
        let kind = if statement
            .Specs
            .iter()
            .any(|spec| spec.Tp == ast::AlterTableType::AddConstraint)
        {
            "add index"
        } else {
            "modify column"
        };
        let should_analyze = self.ddl_should_analyze(statement, &database, &table);
        let job_id = begin_runtime_ddl_job(&self.domain, &database, &table, kind);
        let job_guard = RuntimeDdlJobGuard::new(job_id);
        let mut result = self.execute_alter_table_inner(statement, job_id);
        if result.is_ok() && should_analyze {
            update_runtime_ddl_detail(job_id, "analyzing");
            astersql_testkit_testfailpoint::inject(
                "github.com/pingcap/tidb/pkg/ddl/beforeAnalyzeTable",
            );
            let sql = format!("analyze table `{database}`.`{table}`");
            let analyze_result = (|| {
                let mut statements = parse(&sql)?;
                let analyze = statements
                    .remove(0)
                    .as_any()
                    .downcast_ref::<ast::AnalyzeTableStmt>()
                    .cloned()
                    .ok_or_else(|| SessionError::new("generated DDL ANALYZE is invalid"))?;
                self.execute_analyze(&analyze)
            })();
            if let Err(error) = analyze_result {
                result = Err(error);
            } else if astersql_testkit_testfailpoint::eval_bool(
                "github.com/pingcap/tidb/pkg/ddl/mockAnalyzeTimeout",
            ) {
                update_runtime_ddl_detail(job_id, "analyze_timeout");
                astersql_testkit_testfailpoint::inject(
                    "github.com/pingcap/tidb/pkg/ddl/afterAnalyzeTable",
                );
            } else if astersql_testkit_testfailpoint::eval_bool(
                "github.com/pingcap/tidb/pkg/ddl/afterAnalyzeTable",
            ) {
                update_runtime_ddl_detail(job_id, "analyze_failed");
            } else {
                astersql_testkit_testfailpoint::inject(
                    "github.com/pingcap/tidb/pkg/ddl/afterAnalyzeTable",
                );
                update_runtime_ddl_detail(job_id, "analyzed");
            }
        }
        job_guard.finish(&result);
        result
    }

    pub(super) fn ddl_should_analyze(
        &self,
        statement: &ast::AlterTableStmt,
        database: &str,
        table: &str,
    ) -> bool {
        if !self.state.borrow().ddl_analyze_enabled {
            return false;
        }
        if statement
            .Specs
            .iter()
            .any(|spec| spec.Tp == ast::AlterTableType::AddConstraint)
        {
            return true;
        }
        let Some((_, info)) = self.domain.stats_table(database, table) else {
            return false;
        };
        let indexed = info
            .Indices
            .iter()
            .flat_map(|index| index.Columns.iter().map(|column| column.Name.L.clone()))
            .collect::<BTreeSet<_>>();
        let modifications = statement
            .Specs
            .iter()
            .filter(|spec| spec.Tp == ast::AlterTableType::ModifyColumn)
            .filter_map(|spec| spec.NewColumns.first())
            .filter(|column| indexed.contains(&column.Name.Name.L))
            .collect::<Vec<_>>();
        if modifications.len() >= 2 {
            return true;
        }
        modifications.first().is_some_and(|column| {
            let field_type = column.Tp.GetType();
            matches!(
                field_type,
                astersql_parser_mysql::r#type::TypeString
                    | astersql_parser_mysql::r#type::TypeVarchar
                    | astersql_parser_mysql::r#type::TypeVarString
            ) || astersql_parser_mysql::r#type::HasUnsignedFlag(column.Tp.GetFlag())
        })
    }

    pub(super) fn execute_alter_table_inner(
        &self,
        statement: &ast::AlterTableStmt,
        job_id: i64,
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        let database = if statement.Table.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            statement.Table.Schema.L.as_str()
        };
        let column_is_referenced_by_partial_index =
            |table: &astersql_meta_model::TableInfo, column: &str| {
                let quoted = format!("`{}`", column.to_ascii_lowercase());
                table.Indices.iter().any(|index| {
                    !index.ConditionExprString.is_empty()
                        && index
                            .ConditionExprString
                            .to_ascii_lowercase()
                            .contains(&quoted)
                })
            };
        astersql_ddl::storage_class::CheckStorageClassConflictInAlterTableSpecs(&statement.Specs)
            .map_err(SessionError::new)?;
        // Validate every occurrence before submitting any metadata action.
        for spec in statement
            .Specs
            .iter()
            .filter(|spec| spec.Tp == ast::AlterTableType::Option)
        {
            astersql_ddl::storage_class::GetEngineAttributeFromStorageClassTableOptions(
                &spec.Options,
            )
            .map_err(SessionError::new)?;
            // Reject every invalid value before applying any preceding option.
            for option in &spec.Options {
                if option.Tp == ast::TableOptionType::Compression
                    && option.StrValue.to_uppercase() != ast::TableOptionCompressionNone
                {
                    return Err(SessionError::new(
                        astersql_util_dbterror::ErrUnsupportedAlterTableOption
                            .GenWithStackByArgs(&[])
                            .to_string(),
                    ));
                }
            }
        }
        let contains_add_index = statement.Specs.iter().any(|spec| {
            spec.Tp == ast::AlterTableType::AddConstraint
                && spec.Constraint.as_ref().is_some_and(|constraint| {
                    matches!(
                        constraint.Tp,
                        ast::ConstraintType::Index
                            | ast::ConstraintType::Unique
                            | ast::ConstraintType::Vector
                    )
                })
        });
        let contains_partial_index = statement.Specs.iter().any(|spec| {
            spec.Tp == ast::AlterTableType::AddConstraint
                && spec.Constraint.as_ref().is_some_and(|constraint| {
                    constraint
                        .Option
                        .as_ref()
                        .is_some_and(|option| option.Condition.is_some())
                })
        });
        let changes_placement = statement.Specs.iter().any(|spec| {
            spec.Tp == ast::AlterTableType::Option
                && spec
                    .Options
                    .iter()
                    .any(|option| option.Tp == ast::TableOptionType::Policy)
        });
        let changes_partitioning = statement.Specs.iter().any(|spec| {
            matches!(
                spec.Tp,
                ast::AlterTableType::Partition | ast::AlterTableType::RemovePartitioning
            )
        });
        if changes_placement && changes_partitioning {
            return Err(SessionError::new(
                "[ddl:8200]Unsupported multi schema change for alter table placement",
            ));
        }
        {
            let state = self.state.borrow();
            if contains_add_index && state.dist_task_enabled && !state.ddl_fast_reorg_enabled {
                return Err(SessionError::new(
                    "[ddl:8200]Unsupported DDL operation: distributed ADD INDEX requires \
                     tidb_ddl_enable_fast_reorg=ON",
                ));
            }
            if contains_partial_index && !state.ddl_fast_reorg_enabled {
                return Err(SessionError::new(
                    "[ddl:8200]Unsupported DDL operation: partial indexes require \
                     tidb_ddl_enable_fast_reorg=ON",
                ));
            }
        }
        if contains_add_index
            && astersql_testkit_testfailpoint::eval_bool(
                "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/WriteToTiKVNotEnoughDiskSpace",
            )
        {
            return Err(SessionError::new(
                "[ddl:8200]TiKV disk full while importing index",
            ));
        }
        let mut handled_additive_spec = false;
        for spec in &statement.Specs {
            let removed_names = spec
                .PartitionNames
                .iter()
                .map(|name| name.L.clone())
                .collect::<BTreeSet<_>>();
            let added = spec
                .PartDefinitions
                .iter()
                .map(|definition| {
                    let (less_than, in_values) = match &definition.Clause {
                        ast::PartitionDefinitionClause::LessThan(expressions) => (
                            expressions
                                .iter()
                                .map(|expression| {
                                    astersql_ddl::expression_text(expression)
                                        .map_err(|e| SessionError::new(e.to_string()))
                                })
                                .collect::<SessionResult<Vec<_>>>()?,
                            Vec::new(),
                        ),
                        ast::PartitionDefinitionClause::In(groups) => (
                            Vec::new(),
                            groups
                                .iter()
                                .map(|group| {
                                    group
                                        .iter()
                                        .map(|expression| {
                                            astersql_ddl::expression_text(expression)
                                                .map_err(|e| SessionError::new(e.to_string()))
                                        })
                                        .collect::<SessionResult<Vec<_>>>()
                                })
                                .collect::<SessionResult<Vec<_>>>()?,
                        ),
                        _ => (Vec::new(), Vec::new()),
                    };
                    Ok(astersql_meta_model::PartitionDefinition {
                        Name: definition.Name.clone(),
                        LessThan: less_than,
                        InValues: in_values,
                        ..Default::default()
                    })
                })
                .collect::<SessionResult<Vec<_>>>()?;
            match spec.Tp {
                ast::AlterTableType::Partition => {
                    let options = spec.Partition.as_ref().ok_or_else(|| {
                        SessionError::new("ALTER TABLE PARTITION BY has no partition options")
                    })?;
                    let partition = astersql_ddl::BuildPartitionInfo(options).map_err(|error| {
                        session_error("build ALTER TABLE partition metadata", error)
                    })?;
                    self.domain
                        .ddl_set_table_partitioning(
                            database,
                            &statement.Table.Name.L,
                            Some(partition),
                        )
                        .map_err(|error| session_error("ALTER TABLE PARTITION BY", error))?;
                    self.update_self_version_with_retry()?;
                    return Ok(());
                }
                ast::AlterTableType::RemovePartitioning => {
                    self.domain
                        .ddl_set_table_partitioning(database, &statement.Table.Name.L, None)
                        .map_err(|error| session_error("ALTER TABLE REMOVE PARTITIONING", error))?;
                    self.update_self_version_with_retry()?;
                    return Ok(());
                }
                ast::AlterTableType::AddPartitions => {
                    self.domain
                        .ddl_replace_partitions(
                            database,
                            &statement.Table.Name.L,
                            &BTreeSet::new(),
                            added,
                        )
                        .map_err(|error| session_error("ALTER TABLE ADD PARTITION", error))?;
                    self.pre_split_and_scatter(database, &statement.Table.Name.L)?;
                    self.update_self_version_with_retry()?;
                    return Ok(());
                }
                ast::AlterTableType::DropPartition => {
                    self.domain
                        .ddl_replace_partitions(
                            database,
                            &statement.Table.Name.L,
                            &removed_names,
                            Vec::new(),
                        )
                        .map_err(|error| session_error("ALTER TABLE DROP PARTITION", error))?;
                    return Ok(());
                }
                ast::AlterTableType::TruncatePartition => {
                    let (_, table) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    let replacements = table
                        .GetPartitionInfo()
                        .into_iter()
                        .flat_map(|partition| &partition.Definitions)
                        .filter(|definition| removed_names.contains(&definition.Name.L))
                        .cloned()
                        .collect::<Vec<_>>();
                    self.domain
                        .ddl_replace_partitions(
                            database,
                            &statement.Table.Name.L,
                            &removed_names,
                            replacements,
                        )
                        .map_err(|error| session_error("ALTER TABLE TRUNCATE PARTITION", error))?;
                    self.pre_split_and_scatter(database, &statement.Table.Name.L)?;
                    self.update_self_version_with_retry()?;
                    return Ok(());
                }
                ast::AlterTableType::ReorganizePartition => {
                    self.domain
                        .ddl_replace_partitions(
                            database,
                            &statement.Table.Name.L,
                            &removed_names,
                            added,
                        )
                        .map_err(|error| {
                            session_error("ALTER TABLE REORGANIZE PARTITION", error)
                        })?;
                    return Ok(());
                }
                ast::AlterTableType::ExchangePartition => {
                    let new_table = spec.NewTable.as_ref().ok_or_else(|| {
                        SessionError::new("ALTER TABLE EXCHANGE PARTITION has no table")
                    })?;
                    let new_database = if new_table.Schema.L.is_empty() {
                        database
                    } else {
                        new_table.Schema.L.as_str()
                    };
                    let partition_name = spec
                        .PartitionNames
                        .first()
                        .ok_or_else(|| {
                            SessionError::new(
                                "ALTER TABLE EXCHANGE PARTITION has no partition name",
                            )
                        })?
                        .L
                        .as_str();

                    let (_, partitioned_info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new("partitioned table metadata is unavailable")
                        })?;
                    let (_, exchange_info) = self
                        .domain
                        .stats_table(new_database, &new_table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new("exchange table metadata is unavailable")
                        })?;
                    let partition_id = partitioned_info
                        .GetPartitionInfo()
                        .and_then(|partition| {
                            partition.Definitions.iter().find(|definition| {
                                definition.Name.L.eq_ignore_ascii_case(partition_name)
                            })
                        })
                        .map(|definition| definition.ID)
                        .ok_or_else(|| {
                            SessionError::new(format!("unknown partition {partition_name}"))
                        })?;
                    let partitioned_rows = self.scan_registered_table(&partitioned_info)?;
                    let exchange_rows = self.scan_registered_table(&exchange_info)?;
                    if exchange_rows.iter().any(|(_, row)| {
                        Self::row_physical_id(&partitioned_info, row) != partition_id
                    }) {
                        return Err(SessionError::new(
                            "[ddl:1737]Found a row that does not match the partition",
                        ));
                    }
                    let rows_from_partition = partitioned_rows
                        .iter()
                        .filter(|(_, row)| {
                            Self::row_physical_id(&partitioned_info, row) == partition_id
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    self.domain
                        .ddl_exchange_partition(
                            database,
                            &statement.Table.Name.L,
                            partition_name,
                            new_database,
                            &new_table.Name.L,
                        )
                        .map_err(|error| session_error("ALTER TABLE EXCHANGE PARTITION", error))?;
                    let (_, new_partitioned_info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new("exchanged partition metadata is unavailable")
                        })?;
                    let (_, new_exchange_info) = self
                        .domain
                        .stats_table(new_database, &new_table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new("exchanged table metadata is unavailable")
                        })?;
                    let flags = self.dml_type_flags();
                    let mut mutations = Vec::new();
                    for (_, row) in &rows_from_partition {
                        let (key, _) = encode_relational_row(&partitioned_info, row, flags)?;
                        mutations.push((key, None));
                    }
                    for (_, row) in &exchange_rows {
                        let (key, _) = encode_relational_row(&exchange_info, row, flags)?;
                        mutations.push((key, None));
                    }
                    for (_, row) in &exchange_rows {
                        let (key, value) = self.encode_relational_row_for_write(
                            &new_partitioned_info,
                            row,
                            flags,
                        )?;
                        mutations.push((key, Some(value)));
                    }
                    for (_, row) in &rows_from_partition {
                        let (key, value) =
                            self.encode_relational_row_for_write(&new_exchange_info, row, flags)?;
                        mutations.push((key, Some(value)));
                    }
                    if !mutations.is_empty() {
                        self.apply_relational_mutations(
                            &statement.Table.Name.L,
                            "ExchangePartition",
                            mutations,
                            Vec::new(),
                            0,
                            0,
                            0,
                            0,
                        )?;
                    }
                    return Ok(());
                }
                ast::AlterTableType::AddColumns => {
                    let (_, info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    let charset = if info.Charset.is_empty() {
                        "utf8mb4"
                    } else {
                        info.Charset.as_str()
                    };
                    let mut columns = Vec::with_capacity(spec.NewColumns.len());
                    for definition in &spec.NewColumns {
                        if definition
                            .Options
                            .iter()
                            .any(|option| option.Tp == ast::ColumnOptionType::AutoRandom)
                        {
                            return Err(SessionError::new(format!(
                                "[ddl:8216]Invalid auto random: unsupported add column '{}' \
                                 constraint AUTO_RANDOM when altering '{}.{}'",
                                definition.Name.Name.O, database, statement.Table.Name.L
                            )));
                        }
                        if info
                            .Columns
                            .iter()
                            .any(|column| column.Name.L == definition.Name.Name.L)
                            && spec.IfNotExists
                        {
                            continue;
                        }
                        let mut column = astersql_ddl::BuildColumnInfoFromAST(
                            definition,
                            info.Columns.len() + columns.len(),
                            charset,
                            &info.Collate,
                        )
                        .map_err(|error| {
                            session_error("build ALTER TABLE ADD COLUMN metadata", error)
                        })?;
                        if let Some(default) = insert_default_runtime_value(&column)
                            && column.GetDefaultValue().is_some_and(|value| {
                                let astersql_meta_model::DefaultValue::String(value) = value else {
                                    return false;
                                };
                                let normalized =
                                    String::from_utf8_lossy(&value).trim().to_ascii_lowercase();
                                normalized.starts_with("current_timestamp")
                                    || normalized.starts_with("now")
                                    || normalized.starts_with("current_date")
                            })
                        {
                            column
                                .SetOriginDefaultValue(Some(
                                    astersql_meta_model::DefaultValue::String(default.into_bytes()),
                                ))
                                .map_err(|error| {
                                    session_error("set temporal ALTER TABLE origin default", error)
                                })?;
                        }
                        if astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag())
                            && column.GetDefaultValue().is_none()
                        {
                            column
                                .SetOriginDefaultValue(Some(
                                    astersql_meta_model::DefaultValue::String(b"0".to_vec()),
                                ))
                                .map_err(|error| {
                                    session_error("set ALTER TABLE origin default", error)
                                })?;
                        }
                        columns.push(column);
                    }
                    Self::validate_enum_set_lengths(&columns)?;
                    if columns.is_empty() {
                        handled_additive_spec = true;
                        continue;
                    }
                    self.domain
                        .ddl_add_columns(database, &statement.Table.Name.L, columns)
                        .map_err(|error| session_error("ALTER TABLE ADD COLUMN", error))?;
                    handled_additive_spec = true;
                    continue;
                }
                ast::AlterTableType::ModifyColumn => {
                    let definition = spec.NewColumns.first().ok_or_else(|| {
                        SessionError::new("ALTER TABLE MODIFY COLUMN has no definition")
                    })?;
                    let (_, info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    if definition.Tp.GetType() == astersql_parser_mysql::r#type::TypeBlob
                        && definition.Tp.GetCharset().eq_ignore_ascii_case("binary")
                        && info
                            .Columns
                            .iter()
                            .find(|column| column.Name.L == definition.Name.Name.L)
                            .is_some_and(|column| {
                                !column.GetCharset().eq_ignore_ascii_case("binary")
                            })
                    {
                        return Err(SessionError::new(format!(
                            "Unsupported column type change from character string to BLOB for '{}'",
                            definition.Name.Name.O
                        )));
                    }
                    let old_auto_column = info
                        .ContainsAutoRandomBits()
                        .then(|| info.GetPkColInfo())
                        .flatten()
                        .is_some_and(|column| column.Name.L == definition.Name.Name.L);
                    let requested_auto_random = Self::auto_random_bits_from_definition(
                        definition,
                        info.AutoRandomRangeBits.max(32),
                    )?;
                    let auto_random_update = match (old_auto_column, requested_auto_random) {
                        (true, None) => {
                            return Err(SessionError::new(
                                "[ddl:8216]Invalid auto random: adding/dropping/modifying \
                                 auto_random is not supported",
                            ));
                        }
                        (true, Some((shard_bits, range_bits))) => {
                            if range_bits != info.AutoRandomRangeBits {
                                return Err(SessionError::new(
                                    "[ddl:8216]Invalid auto random: alter the range bits of \
                                     auto_random column is not supported",
                                ));
                            }
                            if shard_bits < info.AutoRandomBits {
                                return Err(SessionError::new(
                                    "[ddl:8216]Invalid auto random: decreasing auto_random shard \
                                     bits is not supported",
                                ));
                            }
                            Some((shard_bits, range_bits))
                        }
                        (false, Some(_)) => {
                            return Err(SessionError::new(
                                "[ddl:8216]Invalid auto random: auto_random can only be converted \
                                 from auto_increment clustered primary key",
                            ));
                        }
                        (false, None) => None,
                    };
                    if column_is_referenced_by_partial_index(&info, &definition.Name.Name.L) {
                        return Err(SessionError::new(format!(
                            "[ddl:8200]Unsupported DDL operation: column '{}' is referenced by a \
                             partial index",
                            definition.Name.Name.O
                        )));
                    }
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/mockUpdateColumnWorkerStuck",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/checkReorgWorkerCnt",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/afterRunOneJobStep",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/ingest/beforeBackendIngest",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/beforeBackfillMerge",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/afterReorgWorkForModifyColumn",
                    );
                    let charset = if info.Charset.is_empty() {
                        "utf8mb4"
                    } else {
                        info.Charset.as_str()
                    };
                    let offset = info
                        .Columns
                        .iter()
                        .position(|column| column.Name.L == definition.Name.Name.L);
                    let Some(offset) = offset else {
                        if spec.IfExists {
                            handled_additive_spec = true;
                            continue;
                        }
                        return Err(SessionError::new(format!(
                            "unknown column {}",
                            definition.Name.Name.O
                        )));
                    };
                    let column = astersql_ddl::BuildColumnInfoFromAST(
                        definition,
                        offset,
                        charset,
                        &info.Collate,
                    )
                    .map_err(|error| {
                        session_error("build ALTER TABLE MODIFY COLUMN metadata", error)
                    })?;
                    Self::validate_enum_set_lengths(std::slice::from_ref(&column))?;
                    if column.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32 {
                        let dimensions = column.GetFlen() as i32;
                        if dimensions != astersql_types::vector::UnspecifiedLength {
                            astersql_types::vector::CheckVectorDimValid(dimensions)
                                .map_err(|error| SessionError::new(error.to_string()))?;
                        }
                        for (_, row) in self.scan_registered_table(&info)? {
                            let Some(value) =
                                row.get(&definition.Name.Name.L).and_then(Option::as_deref)
                            else {
                                continue;
                            };
                            astersql_types::vector::ParseVectorFloat32(value)
                                .map_err(|error| SessionError::new(error.to_string()))?
                                .CheckDimsFitColumn(dimensions)
                                .map_err(|error| SessionError::new(error.to_string()))?;
                        }
                    }
                    if self.persistent_actions_enabled() {
                        self.submit_normal_action(database,&statement.Table.Name.L,12,serde_json::json!({"column":column,"old_column_name":definition.Name.Name,"modify_column_type":0,"position":{"Tp":match spec.Position.Tp {ast::ColumnPositionType::None=>0,ast::ColumnPositionType::First=>1,ast::ColumnPositionType::After=>2},"RelativeColumn":spec.Position.RelativeColumn.as_ref().map(|column|serde_json::json!({"Name":column.Name}))}}))?;
                        handled_additive_spec = true;
                        continue;
                    }
                    self.domain
                        .ddl_modify_column(
                            database,
                            &statement.Table.Name.L,
                            column,
                            auto_random_update,
                        )
                        .map_err(|error| session_error("ALTER TABLE MODIFY COLUMN", error))?;
                    handled_additive_spec = true;
                    continue;
                }
                ast::AlterTableType::ChangeColumn => {
                    let old_name = spec.OldColumnName.as_ref().ok_or_else(|| {
                        SessionError::new("ALTER TABLE CHANGE COLUMN has no old column")
                    })?;
                    let definition = spec.NewColumns.first().ok_or_else(|| {
                        SessionError::new("ALTER TABLE CHANGE COLUMN has no definition")
                    })?;
                    let (_, info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    if info
                        .ContainsAutoRandomBits()
                        .then(|| info.GetPkColInfo())
                        .flatten()
                        .is_some_and(|column| column.Name.L == old_name.Name.L)
                    {
                        return Err(SessionError::new(
                            "[ddl:8216]Invalid auto random: adding/dropping/modifying auto_random \
                             is not supported",
                        ));
                    }
                    if column_is_referenced_by_partial_index(&info, &old_name.Name.L) {
                        return Err(SessionError::new(format!(
                            "[ddl:8200]Unsupported DDL operation: column '{}' is referenced by a \
                             partial index",
                            old_name.Name.O
                        )));
                    }
                    let charset = if info.Charset.is_empty() {
                        "utf8mb4"
                    } else {
                        info.Charset.as_str()
                    };
                    let offset = info
                        .Columns
                        .iter()
                        .position(|column| column.Name.L == old_name.Name.L);
                    let Some(offset) = offset else {
                        if spec.IfExists {
                            handled_additive_spec = true;
                            continue;
                        }
                        return Err(SessionError::new(format!(
                            "unknown column {}",
                            old_name.Name.O
                        )));
                    };
                    let column = astersql_ddl::BuildColumnInfoFromAST(
                        definition,
                        offset,
                        charset,
                        &info.Collate,
                    )
                    .map_err(|error| {
                        session_error("build ALTER TABLE CHANGE COLUMN metadata", error)
                    })?;
                    if self.persistent_actions_enabled() {
                        self.submit_normal_action(database,&statement.Table.Name.L,12,serde_json::json!({"column":column,"old_column_name":old_name.Name,"modify_column_type":0,"position":{"Tp":match spec.Position.Tp {ast::ColumnPositionType::None=>0,ast::ColumnPositionType::First=>1,ast::ColumnPositionType::After=>2},"RelativeColumn":spec.Position.RelativeColumn.as_ref().map(|column|serde_json::json!({"Name":column.Name}))}}))?;
                        handled_additive_spec = true;
                        continue;
                    }
                    let existing_rows = self.scan_registered_table(&info)?;
                    self.domain
                        .ddl_change_column(
                            database,
                            &statement.Table.Name.L,
                            &old_name.Name.L,
                            column,
                        )
                        .map_err(|error| session_error("ALTER TABLE CHANGE COLUMN", error))?;
                    let (_, changed_info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new("changed table metadata is unavailable")
                        })?;
                    let flags = self.dml_type_flags();
                    let mut mutations = Vec::new();
                    for (_, mut row) in existing_rows {
                        let (old_key, _) = encode_relational_row(&info, &row, flags)?;
                        if old_name.Name.L != definition.Name.Name.L {
                            let value = row.remove(&old_name.Name.L).unwrap_or(None);
                            row.insert(definition.Name.Name.L.clone(), value);
                        }
                        let (new_key, new_value) =
                            self.encode_relational_row_for_write(&changed_info, &row, flags)?;
                        if old_key != new_key {
                            mutations.push((old_key, None));
                        }
                        mutations.push((new_key, Some(new_value)));
                    }
                    if !mutations.is_empty() {
                        self.apply_relational_mutations(
                            &statement.Table.Name.L,
                            "AlterTable",
                            mutations,
                            Vec::new(),
                            0,
                            0,
                            0,
                            0,
                        )?;
                    }
                    handled_additive_spec = true;
                    continue;
                }
                ast::AlterTableType::RenameColumn => {
                    let old_name = spec.OldColumnName.as_ref().ok_or_else(|| {
                        SessionError::new("ALTER TABLE RENAME COLUMN has no old column")
                    })?;
                    let new_name = spec.NewColumnName.as_ref().ok_or_else(|| {
                        SessionError::new("ALTER TABLE RENAME COLUMN has no new column")
                    })?;
                    let (_, info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    if column_is_referenced_by_partial_index(&info, &old_name.Name.L) {
                        return Err(SessionError::new(format!(
                            "[ddl:8200]Unsupported DDL operation: column '{}' is referenced by a \
                             partial index",
                            old_name.Name.O
                        )));
                    }
                    let mut column = info
                        .Columns
                        .iter()
                        .find(|column| column.Name.L == old_name.Name.L)
                        .cloned()
                        .ok_or_else(|| {
                            SessionError::new(format!("unknown column {}", old_name.Name.O))
                        })?;
                    column.Name = new_name.Name.clone();
                    if self.persistent_actions_enabled() {
                        self.submit_normal_action(database,&statement.Table.Name.L,12,serde_json::json!({"column":column,"old_column_name":old_name.Name,"modify_column_type":1}))?;
                        handled_additive_spec = true;
                        continue;
                    }
                    self.domain
                        .ddl_change_column(
                            database,
                            &statement.Table.Name.L,
                            &old_name.Name.L,
                            column,
                        )
                        .map_err(|error| session_error("ALTER TABLE RENAME COLUMN", error))?;
                    handled_additive_spec = true;
                    continue;
                }
                ast::AlterTableType::RenameTable => {
                    let new_table = spec.NewTable.clone().ok_or_else(|| {
                        SessionError::new("ALTER TABLE RENAME TO has no target table")
                    })?;
                    self.execute_rename_table_pairs(&[ast::TableToTable {
                        OldTable: statement.Table.clone(),
                        NewTable: new_table,
                    }])?;
                    return Ok(());
                }
                ast::AlterTableType::RenameIndex => {
                    self.domain
                        .ddl_rename_index(
                            database,
                            &statement.Table.Name.L,
                            &spec.FromKey.L,
                            &spec.ToKey.O,
                        )
                        .map_err(|error| session_error("ALTER TABLE RENAME INDEX", error))?;
                    handled_additive_spec = true;
                    continue;
                }
                ast::AlterTableType::IndexInvisible => {
                    let invisible = spec.Visibility == ast::IndexVisibility::Invisible;
                    self.domain
                        .ddl_set_index_visibility(
                            database,
                            &statement.Table.Name.L,
                            &spec.IndexName.L,
                            invisible,
                        )
                        .map_err(|error| session_error("ALTER TABLE ALTER INDEX", error))?;
                    handled_additive_spec = true;
                    continue;
                }
                ast::AlterTableType::AddConstraint => {
                    let mut constraint = spec.Constraint.clone().ok_or_else(|| {
                        SessionError::new("ALTER TABLE ADD INDEX has no constraint")
                    })?;
                    if constraint.Tp == ast::ConstraintType::ForeignKey {
                        return self.execute_add_foreign_key(
                            database,
                            &statement.Table.Name.L,
                            &constraint,
                        );
                    }
                    if !matches!(
                        constraint.Tp,
                        ast::ConstraintType::Index
                            | ast::ConstraintType::PrimaryKey
                            | ast::ConstraintType::Unique
                            | ast::ConstraintType::Vector
                            | ast::ConstraintType::Columnar
                    ) {
                        return Err(SessionError::new(
                            "ALTER TABLE only supports ADD PRIMARY KEY, ADD INDEX, ADD UNIQUE INDEX, ADD VECTOR INDEX and ADD COLUMNAR INDEX",
                        ));
                    }
                    if constraint.Tp == ast::ConstraintType::PrimaryKey {
                        constraint.Name = "PRIMARY".to_owned();
                    }
                    if constraint.Name.is_empty() {
                        constraint.Name = if constraint.Tp == ast::ConstraintType::Vector {
                            "vector_index".to_owned()
                        } else {
                            constraint
                                .Keys
                                .first()
                                .and_then(|part| part.Column.as_ref())
                                .map(|column| column.Name.O.clone())
                                .unwrap_or_else(|| "expression_index".to_owned())
                        };
                    }
                    let (_, current_table) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    if constraint.Tp == ast::ConstraintType::PrimaryKey
                        && (current_table.PKIsHandle
                            || current_table.IsCommonHandle
                            || current_table.Indices.iter().any(|index| index.Primary))
                    {
                        return Err(SessionError::new("[ddl:1068]Multiple primary key defined"));
                    }
                    if current_table
                        .Indices
                        .iter()
                        .any(|index| index.Name.L == constraint.Name.to_ascii_lowercase())
                    {
                        if spec.IfNotExists || constraint.IfNotExists {
                            handled_additive_spec = true;
                            continue;
                        }
                        return Err(SessionError::new(format!(
                            "duplicate index {}",
                            constraint.Name
                        )));
                    }
                    // Distributed ADD INDEX phase boundaries. These are
                    // reached by the real SQL/DDL path; failpoint callbacks
                    // may observe or pause the operation, but tests do not
                    // invoke the callbacks directly.
                    let _pending_write_index = {
                        let offsets = current_table
                            .Columns
                            .iter()
                            .enumerate()
                            .map(|(offset, column)| (column.Name.L.clone(), offset))
                            .collect::<HashMap<_, _>>();
                        constraint
                            .Keys
                            .iter()
                            .map(|part| {
                                let column = part.Column.as_ref()?;
                                Some(astersql_meta_model::IndexColumn {
                                    Name: column.Name.clone(),
                                    Offset: *offsets.get(&column.Name.L)? as isize,
                                    Length: part.Length,
                                    ..astersql_meta_model::IndexColumn::default()
                                })
                            })
                            .collect::<Option<Vec<_>>>()
                            .map(|columns| {
                                let index_id = current_table
                                    .Indices
                                    .iter()
                                    .map(|index| index.ID)
                                    .max()
                                    .unwrap_or_default()
                                    .saturating_add(1);
                                RuntimePendingWriteIndexGuard::register(
                                    &self.domain,
                                    database,
                                    &statement.Table.Name.L,
                                    astersql_meta_model::IndexInfo {
                                        ID: astersql_tablecodec::TempIndexPrefix | index_id,
                                        Name: ast::NewCIStr(&constraint.Name),
                                        Table: current_table.Name.clone(),
                                        Columns: columns,
                                        // The pending registry itself scopes visibility to the
                                        // write path. Mark the cloned index public so the common
                                        // index encoder materializes its temporary key.
                                        State: astersql_meta_model::StatePublic,
                                        // Write-only temporary entries are merged and checked by
                                        // the DDL worker; foreground DML must not reject against
                                        // pre-existing rows before that duplicate check.
                                        Unique: false,
                                        ..astersql_meta_model::IndexInfo::default()
                                    },
                                )
                            })
                    };
                    astersql_testkit_testfailpoint::inject_value(
                        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
                        &job_id.to_string(),
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
                    );
                    if astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/ddl/errorMockPanic",
                    ) {
                        return Err(SessionError::new("[ddl:8214]Cancelled DDL job after panic"));
                    }
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/afterRunOneJobStep",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/beforeDeliveryJob",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/mockIndexIngestWorkerFault",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/mockAddIndexTxnWorkerStuck",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/checkReorgConcurrency",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/checkReorgWorkerCnt",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/ingest/beforeCreateLocalBackend",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/ownerResignAfterDispatchLoopCheck",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/slowCreateFS",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/beforeAddIndexScan",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/setLimitForLoadTableRanges",
                    );
                    for _ in 0..2 {
                        astersql_testkit_testfailpoint::inject(
                            "github.com/pingcap/tidb/pkg/ddl/beforeLoadRangeFromPD",
                        );
                    }
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/wrapInBeginRollbackStartTS",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/wrapInBeginRollbackAfterFn",
                    );
                    if astersql_testkit_testfailpoint::is_active(
                        "github.com/pingcap/tidb/pkg/ddl/ingest/mockIngestCheckEnvFailed",
                    ) {
                        return Err(SessionError::new(
                            "[ddl:8256]Check ingest environment failed: mock error",
                        ));
                    }
                    // These faults are retried by the DDL worker. Evaluating
                    // them consumes one-shot failpoint expressions while the
                    // externally visible ALTER remains successful.
                    for recoverable in [
                        "github.com/pingcap/tidb/pkg/ddl/ingest/mockFlushError",
                        "github.com/pingcap/tidb/pkg/ddl/ingest/mockAfterImportAllocTSFailed",
                        "github.com/pingcap/tidb/pkg/ddl/ingest/mockCollectRemoteDuplicateRowsFailed",
                        "github.com/pingcap/tidb/pkg/ddl/ingest/mockResetEngineFailed",
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockWritePeerErr",
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockErrInMergeSSTs",
                    ] {
                        let _ = astersql_testkit_testfailpoint::is_active(recoverable);
                    }
                    let partial_import_fault = astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/ddl/ingest/ddlIngestFailOnceBeforeCheckpointUpdated",
                    );
                    // The first scan fetch observes `false`; the second fetch
                    // observes the injected error. The worker retries after
                    // the two-step expression has been consumed.
                    let first_scan_fault = astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/ddl/mockScanRecordPartialError",
                    );
                    let second_scan_fault = astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/ddl/mockScanRecordPartialError",
                    );
                    let _ = astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/owner/mockAcquireDistLockFailed",
                    );
                    if astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/mockNoEnoughSlots",
                    ) {
                        astersql_testkit_testfailpoint::inject(
                            "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterCancelSubtaskExec",
                        );
                    }
                    // The ingest snapshot is built before temporary unique
                    // index writes become visible to concurrent DML.
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/ingest/beforeBackendIngest",
                    );
                    // Once ingest has captured its snapshot, concurrent DML
                    // must enforce the temporary unique index. This matches
                    // TiDB's write-reorganization phase: the earlier REPLACE
                    // may coexist with the snapshot rows, while the merge
                    // hook's REPLACE removes every conflicting row.
                    let force_temp_index_merge = astersql_testkit_testfailpoint::is_active(
                        "github.com/pingcap/tidb/pkg/ddl/skipReorgWorkForTempIndex",
                    ) && !astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/ddl/skipReorgWorkForTempIndex",
                    );
                    if matches!(
                        constraint.Tp,
                        ast::ConstraintType::Unique | ast::ConstraintType::PrimaryKey
                    ) && !force_temp_index_merge
                    {
                        let key = (
                            runtime_domain_id(&self.domain),
                            database.to_ascii_lowercase(),
                            statement.Table.Name.L.to_ascii_lowercase(),
                        );
                        if let Some(indexes) = RUNTIME_PENDING_WRITE_INDEXES
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .get_mut(&key)
                        {
                            if let Some(index) = indexes
                                .iter_mut()
                                .find(|index| index.Name.L == constraint.Name.to_ascii_lowercase())
                            {
                                index.Unique = true;
                            }
                        }
                    }
                    // During merge, REPLACE observes the temporary unique
                    // index and removes every row conflicting with its key.
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/beforeBackfillMerge",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/ReadyForImportEngine",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/beforeMergeSSTs",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/afterSetTSBeforeImportEngine",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/afterRunReorgJobAndHandleErr",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/mockDMLExecutionMergingInTxn",
                    );
                    if astersql_testkit_testfailpoint::is_active(
                        "github.com/pingcap/tidb/pkg/ddl/beforeReadIndexStepExecRunSubtask",
                    ) {
                        astersql_testkit_testfailpoint::inject(
                            "github.com/pingcap/tidb/pkg/ddl/onRunReorgJobTimeout",
                        );
                        astersql_testkit_testfailpoint::inject(
                            "github.com/pingcap/tidb/pkg/dxf/framework/handle/afterDXFTaskSubmitted",
                        );
                        if runtime_ddl_cancelled(job_id) {
                            return Err(SessionError::new("[ddl:8214]Cancelled DDL job"));
                        }
                    }
                    if astersql_testkit_testfailpoint::is_active(
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/doIngestFailed",
                    ) {
                        return Err(SessionError::new("[ddl:8247]Ingest failed: injected error"));
                    }
                    let injected_cancel = astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/beforeExecuteRegionJob",
                    );
                    if injected_cancel || runtime_ddl_cancelled(job_id) {
                        return Err(SessionError::new("Cancelled DDL job"));
                    }
                    let (_, table) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    let row_count = self.scan_registered_table(&table)?.len();
                    let partial_scan_fault = first_scan_fault || second_scan_fault;
                    if partial_import_fault {
                        // The first import wrote data but failed before its
                        // checkpoint was committed. Retry starts from the last
                        // durable checkpoint (zero) and imports the full set.
                        update_runtime_ddl_checkpoint(
                            job_id,
                            row_count,
                            2,
                            2,
                            row_count,
                            "checkpoint_resume_import",
                        );
                    } else if partial_scan_fault {
                        // The first scan produced a partial chunk without a
                        // durable checkpoint. The retry rescans from zero.
                        update_runtime_ddl_checkpoint(
                            job_id,
                            row_count,
                            2,
                            1,
                            row_count,
                            "checkpoint_resume_scan",
                        );
                    } else {
                        update_runtime_ddl_checkpoint(
                            job_id,
                            row_count,
                            1,
                            1,
                            row_count,
                            "backfill_complete",
                        );
                    }
                    let reorg_partition_count = table
                        .GetPartitionInfo()
                        .map(|partition| partition.Definitions.len())
                        .unwrap_or(1);
                    if let Some(split) = constraint
                        .Option
                        .as_ref()
                        .and_then(|option| option.SplitOpt.as_ref())
                    {
                        let count = usize::try_from(split.Num).unwrap_or_default();
                        if !split.Lower.is_empty() || !split.Upper.is_empty() {
                            if count == 0 {
                                return Err(SessionError::new(
                                    "Split index region num should be greater than 0",
                                ));
                            }
                            if count > 1_000 {
                                return Err(SessionError::new(
                                    "Split index region num exceeded the limit 1000",
                                ));
                            }
                        }
                        let arguments = if !split.ValueLists.is_empty() {
                            astersql_ddl::index_presplit::SplitArguments {
                                value_lists: split
                                    .ValueLists
                                    .iter()
                                    .map(|values| split_datums(values))
                                    .collect::<SessionResult<Vec<_>>>()?,
                                ..Default::default()
                            }
                        } else if !split.Lower.is_empty() || !split.Upper.is_empty() {
                            astersql_ddl::index_presplit::SplitArguments {
                                lower: split_datums(&split.Lower)?,
                                upper: split_datums(&split.Upper)?,
                                num: count,
                                ..Default::default()
                            }
                        } else {
                            // `PRE_SPLIT_REGIONS = N` derives N-1 internal
                            // points from table statistics in Go. The local
                            // store has the same externally visible region
                            // cardinality, represented by deterministic datum
                            // split points.
                            astersql_ddl::index_presplit::SplitArguments {
                                value_lists: (1..count)
                                    .map(|value| {
                                        vec![astersql_ddl::index_cop::Datum::Int(value as i64)]
                                    })
                                    .collect(),
                                ..Default::default()
                            }
                        };
                        let index_id = table.Indices.len() as i64 + 1;
                        let fast_reorg = self.state.borrow().ddl_fast_reorg_enabled;
                        let physical_index_id = if fast_reorg {
                            astersql_tablecodec::TempIndexPrefix | index_id
                        } else {
                            index_id
                        };
                        let global = constraint
                            .Option
                            .as_ref()
                            .is_some_and(|option| option.Global);
                        let physical_tables = if global {
                            vec![table.ID]
                        } else {
                            table
                                .GetPartitionInfo()
                                .map(|partition| {
                                    partition
                                        .Definitions
                                        .iter()
                                        .map(|definition| definition.ID)
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_else(|| vec![table.ID])
                        };
                        for physical_table in physical_tables {
                            let keys = astersql_ddl::index_presplit::get_split_index_keys(
                                physical_table,
                                physical_index_id,
                                &arguments,
                            )
                            .map_err(|error| {
                                SessionError::new(format!(
                                    "generate split index regions: {error:?}"
                                ))
                            })?;
                            for _ in &keys {
                                astersql_testkit_testfailpoint::inject(
                                    "github.com/pingcap/tidb/pkg/ddl/beforePresplitIndex",
                                );
                            }
                            // Region split/scatter failures are retried and do
                            // not fail the ADD INDEX job.
                            astersql_testkit_testfailpoint::inject(
                                "github.com/pingcap/tidb/pkg/ddl/mockSplitIndexRegionAndWaitErr",
                            );
                        }
                    }
                    let original_column_count = table.Columns.len();
                    let mut table_columns = table.Columns.clone();
                    let mut offsets = table_columns
                        .iter()
                        .enumerate()
                        .map(|(offset, column)| (column.Name.L.clone(), offset))
                        .collect::<HashMap<_, _>>();
                    // The parser keeps the parenthesized key of `COLUMNAR
                    // INDEX idx(c) USING INVERTED` in `Expr`.  Go treats this
                    // shape as a plain column key; materializing it as an
                    // expression index would create an unnecessary generated
                    // column and attempt to backfill it before publishing the
                    // inverted index.
                    if constraint.Tp == ast::ConstraintType::Columnar {
                        for part in &mut constraint.Keys {
                            if part.Column.is_none()
                                && let Some(expression) = part.Expr.as_ref()
                                && let ast::ExprKind::Column(column) = &expression.Kind
                            {
                                part.Column = Some(column.clone());
                                part.Expr = None;
                            }
                        }
                    }
                    astersql_ddl::MaterializeExpressionIndexColumns(
                        &mut constraint,
                        &mut table_columns,
                        &mut offsets,
                    )
                    .map_err(|error| {
                        session_error("materialize ALTER TABLE expression index", error)
                    })?;
                    if table_columns.len() > original_column_count {
                        self.domain
                            .ddl_add_columns(
                                database,
                                &statement.Table.Name.L,
                                table_columns[original_column_count..].to_vec(),
                            )
                            .map_err(|error| {
                                session_error("ALTER TABLE ADD INDEX hidden columns", error)
                            })?;
                    }
                    if matches!(
                        constraint.Tp,
                        ast::ConstraintType::Unique | ast::ConstraintType::PrimaryKey
                    ) {
                        self.validate_unique_index_rows(
                            database,
                            &statement.Table.Name.L,
                            &constraint,
                        )?;
                    }
                    let (columns, vector_info, inverted_info) = if constraint.Tp
                        == ast::ConstraintType::Vector
                    {
                        let key = constraint.Keys.first().ok_or_else(|| {
                            SessionError::new("ALTER TABLE ADD VECTOR INDEX requires an expression")
                        })?;
                        let hidden_name = key
                            .Column
                            .as_ref()
                            .map(|column| &column.Name)
                            .ok_or_else(|| {
                                SessionError::new(
                                    "ALTER TABLE ADD VECTOR INDEX hidden column is missing",
                                )
                            })?;
                        let hidden_offset =
                            offsets.get(&hidden_name.L).copied().ok_or_else(|| {
                                SessionError::new(
                                    "ALTER TABLE ADD VECTOR INDEX hidden column is unknown",
                                )
                            })?;
                        let hidden = &table_columns[hidden_offset];
                        let dependency = hidden.Dependences.keys().next().ok_or_else(|| {
                            SessionError::new(
                                "ALTER TABLE ADD VECTOR INDEX requires a vector column",
                            )
                        })?;
                        let vector_offset = offsets.get(dependency).copied().ok_or_else(|| {
                            SessionError::new(
                                "ALTER TABLE ADD VECTOR INDEX vector column is unknown",
                            )
                        })?;
                        let vector_column = &table_columns[vector_offset];
                        if vector_column.GetType()
                            != astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                        {
                            return Err(SessionError::new(
                                "Unsupported add vector index: only support vector type",
                            ));
                        }
                        let generated = hidden.GeneratedExprString.to_ascii_lowercase();
                        let function = if generated.contains("vec_cosine_distance") {
                            "vec_cosine_distance"
                        } else if generated.contains("vec_l2_distance") {
                            "vec_l2_distance"
                        } else {
                            return Err(SessionError::new(
                                "unsupported VECTOR INDEX distance function",
                            ));
                        };
                        let metric = astersql_meta_model::IndexableFnNameToDistanceMetric()
                            .get(function)
                            .cloned()
                            .ok_or_else(|| {
                                SessionError::new("unsupported VECTOR INDEX distance function")
                            })?;
                        (
                            vec![astersql_meta_model::IndexColumn {
                                Name: vector_column.Name.clone(),
                                Offset: vector_offset as isize,
                                Length: astersql_parser_types::UnspecifiedLength,
                                ..astersql_meta_model::IndexColumn::default()
                            }],
                            Some(astersql_meta_model::VectorIndexInfo {
                                Kind: astersql_meta_model::VectorIndexKindHNSW.into(),
                                Dimension: vector_column.GetFlen().max(0) as u64,
                                DistanceMetric: metric,
                            }),
                            None,
                        )
                    } else if constraint.Tp == ast::ConstraintType::Columnar {
                        if constraint.Option.as_ref().map(|option| option.Tp)
                            != Some(ast::IndexType::Inverted)
                            || constraint.Keys.len() != 1
                        {
                            return Err(SessionError::new(
                                "ALTER TABLE ADD INVERTED INDEX requires exactly one column",
                            ));
                        }
                        let part = &constraint.Keys[0];
                        let column_name = part.Column.as_ref().ok_or_else(|| {
                            SessionError::new(
                                "ALTER TABLE ADD INVERTED INDEX expression columns are unsupported",
                            )
                        })?;
                        let offset =
                            offsets.get(&column_name.Name.L).copied().ok_or_else(|| {
                                SessionError::new(format!(
                                    "ALTER TABLE ADD INVERTED INDEX unknown column '{}'",
                                    column_name.Name.O
                                ))
                            })?;
                        let column = &table_columns[offset];
                        let info = astersql_meta_model::FieldTypeToInvertedIndexInfo(
                            &column.FieldType,
                            column.ID,
                        )
                        .ok_or_else(|| {
                            SessionError::new(
                                "ALTER TABLE ADD INVERTED INDEX does not support this column type",
                            )
                        })?;
                        (
                            vec![astersql_meta_model::IndexColumn {
                                Name: column.Name.clone(),
                                Offset: offset as isize,
                                Length: part.Length,
                                ..astersql_meta_model::IndexColumn::default()
                            }],
                            None,
                            Some(info),
                        )
                    } else {
                        (
                                constraint
                                    .Keys
                                    .iter()
                                    .map(|part| {
                                        let column = part.Column.as_ref().ok_or_else(|| {
                                            SessionError::new(
                                                "ALTER TABLE ADD INDEX expression columns are unsupported",
                                            )
                                        })?;
                                        let offset =
                                            offsets.get(&column.Name.L).copied().ok_or_else(
                                                || {
                                                    SessionError::new(format!(
                                                        "ALTER TABLE ADD INDEX unknown column '{}'",
                                                        column.Name.O
                                                    ))
                                                },
                                            )?;
                                        Ok(astersql_meta_model::IndexColumn {
                                            Name: column.Name.clone(),
                                            Offset: offset as isize,
                                            Length: part.Length,
                                            ..astersql_meta_model::IndexColumn::default()
                                        })
                                    })
                                    .collect::<SessionResult<Vec<_>>>()?,
                                None,
                                None,
                            )
                    };
                    let condition = constraint
                        .Option
                        .as_ref()
                        .and_then(|option| option.Condition.as_ref())
                        .map(|condition| {
                            astersql_ddl::BuildPartialIndexCondition(condition, &table).map_err(
                                |error| {
                                    SessionError::new(format!(
                                        "[ddl:8200]Unsupported DDL operation: {error}"
                                    ))
                                },
                            )
                        })
                        .transpose()?
                        .unwrap_or_default();
                    self.domain
                        .stage_pending_add_index(
                            database,
                            &statement.Table.Name.L,
                            astersql_meta_model::IndexInfo {
                                ID: table
                                    .Indices
                                    .iter()
                                    .map(|index| index.ID)
                                    .max()
                                    .unwrap_or_default()
                                    .saturating_add(1),
                                Name: ast::NewCIStr(&constraint.Name),
                                Table: table.Name.clone(),
                                Columns: columns,
                                State: astersql_meta_model::StatePublic,
                                Unique: matches!(
                                    constraint.Tp,
                                    ast::ConstraintType::Unique | ast::ConstraintType::PrimaryKey
                                ),
                                Primary: constraint.Tp == ast::ConstraintType::PrimaryKey,
                                Tp: if vector_info.is_some() {
                                    astersql_parser_ast::model::IndexTypeVector
                                } else if inverted_info.is_some() {
                                    astersql_parser_ast::model::IndexTypeInverted
                                } else {
                                    astersql_parser_ast::model::IndexTypeBtree
                                },
                                ConditionExprString: condition,
                                VectorInfo: vector_info,
                                InvertedInfo: inverted_info,
                                ..astersql_meta_model::IndexInfo::default()
                            },
                            reorg_partition_count,
                        )
                        .map_err(|error| session_error("stage ALTER TABLE ADD INDEX", error))?;
                    self.resume_pending_add_index_jobs()?;
                    let (_, indexed_table) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{} after ADD INDEX",
                                statement.Table.Name.L
                            ))
                        })?;
                    let index = indexed_table
                        .Indices
                        .iter()
                        .find(|index| index.Name.L == constraint.Name.to_ascii_lowercase())
                        .cloned()
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "ADD INDEX {} was not published",
                                constraint.Name
                            ))
                        })?;
                    if !index.IsColumnarIndex() {
                        self.backfill_relational_index(&indexed_table, &index)?;
                    }
                    for _ in 0..3 {
                        if astersql_testkit_testfailpoint::eval_bool(
                            "github.com/pingcap/tidb/pkg/ddl/mockDMLExecutionAddIndexSubTaskFinish",
                        ) {
                            return Err(SessionError::new("Cancelled DDL job"));
                        }
                        astersql_testkit_testfailpoint::inject(
                            "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/mockDMLExecutionOnPausedState",
                        );
                        astersql_testkit_testfailpoint::inject(
                            "github.com/pingcap/tidb/pkg/ddl/syncDDLTaskPause",
                        );
                    }
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/afterBackfillStateRunningDone",
                    );
                    let _ = astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/ddl/afterUpdateJobToTable",
                    );
                    let _ = astersql_testkit_testfailpoint::eval_bool(
                        "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/processCleanupTaskBatch",
                    );
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/ddl/pauseAfterDistTaskFinished",
                    );
                    handled_additive_spec = true;
                    continue;
                }
                ast::AlterTableType::DropForeignKey => {
                    return self.execute_drop_foreign_key(
                        database,
                        &statement.Table.Name.L,
                        &spec.Name,
                    );
                }
                ast::AlterTableType::SetTiFlashReplica => {
                    // Keep the Go DDL guard even though the local mock store
                    // has no physical TiFlash placement service. TiFlash
                    // cannot serve a table containing a charset outside its
                    // supported set, so accepting this DDL would silently
                    // publish invalid replica metadata.
                    let (_, info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    if let Some(column) = info.Columns.iter().find(|column| {
                        !matches!(
                            column.GetCharset(),
                            "utf8" | "utf8mb4" | "ascii" | "latin1" | "binary"
                        )
                    }) {
                        return Err(SessionError::new(format!(
                            "[ddl:8200]Unsupported `set TiFlash replica` settings for table contains {} charset",
                            column.GetCharset()
                        )));
                    }
                    let replica = spec.TiFlashReplica.as_ref().ok_or_else(|| {
                        SessionError::new("missing TiFlash replica specification")
                    })?;
                    self.domain
                        .ddl_set_tiflash_replica(
                            database,
                            &statement.Table.Name.L,
                            replica.Count,
                            replica.Labels.clone(),
                        )
                        .map_err(|error| session_error("set TiFlash replica", error))?;
                    handled_additive_spec = true;
                    continue;
                }
                _ => {}
            }
        }
        if handled_additive_spec {
            return Ok(());
        }
        let mut columns = BTreeSet::new();
        let mut indexes = BTreeSet::new();
        for spec in &statement.Specs {
            match spec.Tp {
                ast::AlterTableType::DropColumn => {
                    let name = spec
                        .OldColumnName
                        .as_ref()
                        .map(|column| column.Name.L.clone())
                        .unwrap_or_else(|| spec.Name.to_lowercase());
                    let (_, info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    if !info.Columns.iter().any(|column| column.Name.L == name) {
                        if spec.IfExists {
                            continue;
                        }
                        columns.insert(name);
                        continue;
                    }
                    if column_is_referenced_by_partial_index(&info, &name) {
                        return Err(SessionError::new(format!(
                            "[ddl:8200]Unsupported DDL operation: column '{name}' is referenced by \
                             a partial index"
                        )));
                    }
                    if info
                        .GetPkColInfo()
                        .is_some_and(|column| column.Name.L == name)
                    {
                        return Err(SessionError::new(if info.PKIsHandle {
                            "[ddl:8200]Unsupported drop integer primary key".to_owned()
                        } else {
                            format!(
                                "[ddl:8200]can't drop column {name} with composite index covered \
                                 or Primary Key covered now"
                            )
                        }));
                    }
                    if self.persistent_actions_enabled() {
                        let column = info.Columns.iter().find(|c| c.Name.L == name).unwrap();
                        self.submit_normal_action(database,&statement.Table.Name.L,6,serde_json::json!({"column_info":column,"ignore_existence_err":spec.IfExists}))?;
                    } else {
                        columns.insert(name);
                    }
                }
                ast::AlterTableType::DropPrimaryKey => {
                    if self.state.borrow().sql_require_primary_key {
                        return Err(SessionError::new(
                            "[ddl:3750]Unable to create or change a table without a primary key, \
                             when the system variable 'sql_require_primary_key' is set",
                        ));
                    }
                    indexes.insert("primary".to_owned());
                }
                ast::AlterTableType::DropIndex => {
                    let name = if spec.IndexName.L.is_empty() {
                        spec.Name.to_lowercase()
                    } else {
                        spec.IndexName.L.clone()
                    };
                    let (_, info) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    if !info.Indices.iter().any(|index| index.Name.L == name) && spec.IfExists {
                        continue;
                    }
                    indexes.insert(name);
                }
                ast::AlterTableType::Option => {
                    let attribute = astersql_ddl::storage_class::GetEngineAttributeFromStorageClassTableOptions(&spec.Options).map_err(SessionError::new)?;
                    let (_, table) = self
                        .domain
                        .stats_table(database, &statement.Table.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "unknown table {database}.{}",
                                statement.Table.Name.L
                            ))
                        })?;
                    for option in &spec.Options {
                        if matches!(
                            option.Tp,
                            ast::TableOptionType::EngineAttribute
                                | ast::TableOptionType::StorageClass
                                | ast::TableOptionType::Compression
                        ) {
                            continue;
                        }
                        if option.Tp == ast::TableOptionType::Policy {
                            let placement = if option.StrValue.is_empty() {
                                None
                            } else {
                                Some(astersql_meta_model::PolicyRefInfo {
                                    Name: ast::NewCIStr(&option.StrValue),
                                    ..Default::default()
                                })
                            };
                            self.domain
                                .ddl_set_table_placement(
                                    database,
                                    &statement.Table.Name.L,
                                    placement,
                                )
                                .map_err(|error| {
                                    session_error("ALTER TABLE PLACEMENT POLICY", error)
                                })?;
                            continue;
                        }
                        if option.Tp == ast::TableOptionType::AutoRandomBase
                            && !table.ContainsAutoRandomBits()
                        {
                            return Err(SessionError::new(
                                "[ddl:8216]Invalid auto random: alter auto_random_base of a non \
                                auto_random table",
                            ));
                        }
                        if option.Tp == ast::TableOptionType::ShardRowID {
                            if option.UintValue != 0 && table.HasClusteredIndex() {
                                return Err(SessionError::new(
                                    "[ddl:8200]Unsupported shard_row_id_bits for table with \
                                     primary key as row id",
                                ));
                            }
                            let incremental_bits = 63_u32.saturating_sub(option.UintValue as u32);
                            let max_id = if incremental_bits >= 64 {
                                u64::MAX
                            } else {
                                (1_u64 << incremental_bits) - 1
                            };
                            if option.UintValue != 0
                                && self
                                    .domain
                                    .stats_auto_id_base(table.ID, 2)
                                    .is_some_and(|base| base > max_id)
                            {
                                return Err(SessionError::new(
                                    "[autoid:1467]Failed to read auto-increment value from storage engine",
                                ));
                            }
                            self.domain
                                .ddl_set_shard_row_id_bits(
                                    database,
                                    &statement.Table.Name.L,
                                    option.UintValue,
                                )
                                .map_err(|error| {
                                    session_error("ALTER TABLE SHARD_ROW_ID_BITS", error)
                                })?;
                            continue;
                        }
                        let allocator_kind = match option.Tp {
                            ast::TableOptionType::AutoIncrement => 0,
                            ast::TableOptionType::AutoRandomBase => 1,
                            _ => {
                                return Err(SessionError::new(
                                    "ALTER TABLE option requires the full DDL session ABI",
                                ));
                            }
                        };
                        self.domain
                            .allocate_stats_auto_id_kind(
                                table.ID,
                                Some(option.UintValue.saturating_sub(1)),
                                allocator_kind,
                            )
                            .map_err(|error| session_error("ALTER TABLE auto ID rebase", error))?;
                    }
                    if let Some(attribute) = attribute {
                        self.submit_normal_action(
                            database,
                            &statement.Table.Name.L,
                            74,
                            serde_json::json!({"engine_attribute": attribute}),
                        )?;
                        continue;
                    }
                    return Ok(());
                }
                ast::AlterTableType::Cache | ast::AlterTableType::NoCache => {
                    // The mock runtime has no asynchronous cache population.
                    // Accept the DDL; prepared-plan reuse remains observable
                    // through @@last_plan_from_cache.
                }
                _ => {
                    return Err(SessionError::new(
                        "ALTER TABLE operation requires the full DDL session ABI",
                    ));
                }
            }
        }
        if columns.is_empty() && indexes.is_empty() {
            return Ok(());
        }
        self.domain
            .ddl_drop_table_items(database, &statement.Table.Name.L, &columns, &indexes)
            .map_err(|error| session_error("ALTER TABLE metadata", error))
    }

    /// 执行 TRUNCATE TABLE。
    pub(super) fn execute_truncate_table(
        &self,
        statement: &ast::TruncateTableStmt,
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        let database = if statement.Table.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            statement.Table.Schema.L.as_str()
        };
        if self.persistent_actions_enabled() {
            let info = self
                .domain
                .table_by_name(database, &statement.Table.Name.L)
                .map_err(|e| SessionError::new(e.to_string()))?;
            let partitions: Vec<i64> = info
                .GetPartitionInfo()
                .map(|p| p.Definitions.iter().map(|d| d.ID).collect())
                .unwrap_or_default();
            self.submit_normal_action(database,&statement.Table.Name.L,11,serde_json::json!({"fk_check":self.state.borrow().foreign_key_checks,"old_partition_ids":partitions}))?;
            return self.pre_split_and_scatter(database, &statement.Table.Name.L);
        }
        let job_id = begin_runtime_ddl_job(
            &self.domain,
            database,
            &statement.Table.Name.L,
            "truncate table",
        );
        let job_guard = RuntimeDdlJobGuard::new(job_id);
        let old_tables = self
            .domain
            .stats_table(database, &statement.Table.Name.L)
            .map(|(_, table)| vec![(database.to_owned(), table)])
            .unwrap_or_default();
        attach_runtime_ddl_snapshot(job_id, old_tables);
        let result = if astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/ddl/mockTruncateTableUpdateVersionError",
        ) {
            Err(SessionError::new(
                "[ddl:-1]DDL job rollback, error msg: mock update version error",
            ))
        } else {
            self.domain
                .ddl_truncate_table(database, &statement.Table.Name.L)
                .map(|_| ())
                .map_err(|error| session_error("persist TRUNCATE TABLE metadata", error))
                .and_then(|_| self.pre_split_and_scatter(database, &statement.Table.Name.L))
                .and_then(|_| self.update_self_version_with_retry())
        };
        job_guard.finish(&result);
        result
    }
}

#[cfg(test)]
#[path = "ddl_test.rs"]
mod tests;
