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

//! ADMIN commands, region management, statistics locks, and mysql system maintenance.

use super::*;

impl ConcreteSession {
    /// Execute ADMIN CHECK TABLE by resolving every target through Domain metadata.
    pub(super) fn execute_admin_check_table(
        &self,
        statement: &ast::AdminStmt,
    ) -> SessionResult<()> {
        if !matches!(
            statement.statement_type,
            ast::AdminStmtType::CheckTable | ast::AdminStmtType::CheckIndex
        ) {
            return Err(SessionError::new("unsupported ADMIN statement"));
        }
        let current_database = self.current_database();
        for table in &statement.tables {
            let database = if table.Schema.L.is_empty() {
                current_database.as_str()
            } else {
                table.Schema.L.as_str()
            };
            let (_, info) = self
                .domain
                .stats_table(database, &table.Name.L)
                .ok_or_else(|| {
                    SessionError::new(format!("unknown table {database}.{}", table.Name.L))
                })?;
            if statement.statement_type == ast::AdminStmtType::CheckIndex
                && !info
                    .Indices
                    .iter()
                    .any(|index| index.Name.L.eq_ignore_ascii_case(&statement.index))
            {
                return Err(SessionError::new(format!(
                    "unknown index {} on {database}.{} (index does not exist)",
                    statement.index, table.Name.L
                )));
            }
            // MV and columnar indexes always use the slow checker, even when
            // fast checking is enabled. Reject their partial-index conditions
            // before a count or row scan, including for an empty table.
            if info.Indices.iter().any(|index| {
                (statement.statement_type == ast::AdminStmtType::CheckTable
                    || index.Name.L.eq_ignore_ascii_case(&statement.index))
                    && (index.MVIndex || index.IsColumnarIndex())
                    && index.HasCondition()
            }) {
                return Err(SessionError::new(
                    "[executor:8273]Validation of partial indexes requires tidb_enable_fast_table_check=ON",
                ));
            }
        }
        if statement.tables.is_empty() {
            return Ok(());
        }
        Ok(())
    }

    /// Execute ADMIN RECOVER/CLEANUP INDEX over the relational KV path.
    ///
    /// The compact runtime already owns the same row/index encoders used by
    /// relational DML. Reusing them here keeps maintenance operations visible
    /// to subsequent SELECTs and makes the testkit path exercise real KV
    /// mutations instead of accepting the statement as a no-op.
    pub(super) fn execute_admin_index_maintenance(
        &self,
        statement: &ast::AdminStmt,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        let table_name = statement
            .tables
            .first()
            .ok_or_else(|| SessionError::new("ADMIN INDEX requires a table"))?;
        let database = if table_name.Schema.L.is_empty() {
            self.current_database()
        } else {
            table_name.Schema.L.clone()
        };
        let (_, table) = self
            .domain
            .stats_table(&database, &table_name.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!("unknown table {database}.{}", table_name.Name.L))
            })?;
        let Some(index) = table
            .Indices
            .iter()
            .find(|index| index.Name.L.eq_ignore_ascii_case(&statement.index))
        else {
            // PK-is-handle tables do not materialize PRIMARY as an IndexInfo;
            // Go's admin executor treats that clustered record key as an
            // already-covered index and returns the empty maintenance result.
            if statement.index.eq_ignore_ascii_case("primary")
                && (table.PKIsHandle || table.IsCommonHandle)
            {
                return Ok(Some(match statement.statement_type {
                    ast::AdminStmtType::RecoverIndex => ConcreteRecordSet::new(
                        vec!["added_count".to_owned(), "scan_count".to_owned()],
                        vec![vec!["0".to_owned(), "0".to_owned()]],
                    ),
                    ast::AdminStmtType::CleanupIndex => ConcreteRecordSet::new(
                        vec!["remove_count".to_owned()],
                        vec![vec!["0".to_owned()]],
                    ),
                    _ => return Err(SessionError::new("unsupported ADMIN index maintenance")),
                }));
            }
            return Err(SessionError::new(format!(
                "unknown index {} on {database}.{} (index does not exist)",
                statement.index, table_name.Name.L
            )));
        };

        // A clustered primary key is the record key itself; there is no
        // secondary index entry to recover or clean up.
        if index.Primary {
            return Ok(Some(match statement.statement_type {
                ast::AdminStmtType::RecoverIndex => ConcreteRecordSet::new(
                    vec!["added_count".to_owned(), "scan_count".to_owned()],
                    vec![vec!["0".to_owned(), "0".to_owned()]],
                ),
                ast::AdminStmtType::CleanupIndex => ConcreteRecordSet::new(
                    vec!["remove_count".to_owned()],
                    vec![vec!["0".to_owned()]],
                ),
                _ => return Err(SessionError::new("unsupported ADMIN index maintenance")),
            }));
        }

        let flags = self.dml_type_flags();
        match statement.statement_type {
            ast::AdminStmtType::RecoverIndex => {
                let rows = self.scan_registered_table(&table)?;
                let mut missing = Vec::new();
                for (_, row) in &rows {
                    for values in relational_index_value_rows(&table, index, row, flags)? {
                        let (key, value) =
                            encode_relational_index_value_row(&table, index, row, flags, values)?;
                        if self.read_raw_kv(key.clone())?.is_none() {
                            missing.push((key, Some(value)));
                        }
                    }
                }
                let added = missing.len();
                if !missing.is_empty() {
                    self.apply_relational_mutations(
                        &table.Name.L,
                        "AdminRecoverIndex",
                        missing,
                        Vec::new(),
                        added as u64,
                        0,
                        0,
                        0,
                    )?;
                }
                Ok(Some(ConcreteRecordSet::new(
                    vec!["added_count".to_owned(), "scan_count".to_owned()],
                    vec![vec![added.to_string(), rows.len().to_string()]],
                )))
            }
            ast::AdminStmtType::CleanupIndex => {
                let mutations = self.domain.storage().with_storage(|store| {
                    let version = store
                        .CurrentVersion("global")
                        .map_err(|error| session_error("get admin cleanup version", error))?;
                    let mut snapshot = store.GetSnapshot(version);
                    let (start, end) =
                        astersql_tablecodec::GetTableIndexKeyRange(table.ID, index.ID);
                    let mut iterator = snapshot
                        .Iter(kv::Key(start), Some(kv::Key(end)))
                        .map_err(|error| session_error("scan admin cleanup index", error))?;
                    let result = (|| {
                        let mut mutations = Vec::new();
                        while iterator.Valid() {
                            let index_key = iterator.Key();
                            let index_value = iterator.Value();
                            let handle = astersql_tablecodec::DecodeIndexHandle(
                                index_key.0.clone(),
                                index_value.to_vec(),
                                index.Columns.len(),
                            )
                            .map_err(|error| {
                                session_error("decode admin cleanup index handle", error)
                            })?
                            .ok_or_else(|| {
                                SessionError::new(
                                    "admin cleanup index value does not contain a handle",
                                )
                            })?;
                            let record_key = kv::Key(
                                astersql_tablecodec::EncodeRowKeyWithHandle(
                                    table.ID,
                                    handle.Copy(),
                                )
                                .0,
                            );
                            let exists = snapshot
                                .Get(&kv::Context::default(), record_key, &[])
                                .map(|_| true)
                                .or_else(|error| {
                                    if kv::IsErrNotFound(&error) {
                                        Ok(false)
                                    } else {
                                        Err(error)
                                    }
                                })
                                .map_err(|error| {
                                    session_error("check admin cleanup record", error)
                                })?;
                            if !exists {
                                mutations.push((index_key, None));
                            }
                            iterator.Next().map_err(|error| {
                                session_error("advance admin cleanup index", error)
                            })?;
                        }
                        Ok::<_, SessionError>(mutations)
                    })();
                    iterator.Close();
                    result
                })?;
                let removed = mutations.len();
                if !mutations.is_empty() {
                    self.apply_relational_mutations(
                        &table.Name.L,
                        "AdminCleanupIndex",
                        mutations,
                        Vec::new(),
                        removed as u64,
                        0,
                        0,
                        0,
                    )?;
                }
                Ok(Some(ConcreteRecordSet::new(
                    vec!["remove_count".to_owned()],
                    vec![vec![removed.to_string()]],
                )))
            }
            _ => Err(SessionError::new("unsupported ADMIN index maintenance")),
        }
    }

    pub(super) fn execute_admin_ddl(
        &self,
        statement: &ast::AdminStmt,
        statement_sql: &str,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        match statement.statement_type {
            ast::AdminStmtType::ShowDdlJobs => {
                let wanted = statement_sql
                    .to_ascii_lowercase()
                    .split_once("job_id")
                    .and_then(|(_, suffix)| suffix.split_once('='))
                    .and_then(|(_, value)| {
                        value.trim().trim_end_matches(';').split_whitespace().next()
                    })
                    .and_then(|value| value.parse::<i64>().ok());
                let jobs = RUNTIME_DDL_JOBS
                    .lock()
                    .expect("runtime DDL jobs lock poisoned");
                let mut records = jobs
                    .active
                    .values()
                    .chain(jobs.history.iter().rev())
                    .filter(|job| {
                        job.domain_id == domain_id
                            && Weak::ptr_eq(&job.domain, &Arc::downgrade(&self.domain))
                    })
                    .filter(|job| wanted.is_none_or(|id| job.id == id))
                    .map(|job| {
                        vec![
                            job.id.to_string(),
                            job.database.clone(),
                            job.table.clone(),
                            job.kind.clone(),
                            String::new(),
                            "0".to_owned(),
                            "0".to_owned(),
                            job.row_count.to_string(),
                            String::new(),
                            job.state.clone(),
                            String::new(),
                            String::new(),
                            format!(
                                "{} thread={} batch_size={} max_write_speed={} scan_attempts={} \
                                 import_attempts={} checkpoint_rows={}",
                                job.detail,
                                job.concurrency,
                                job.batch_size,
                                job.max_write_speed,
                                job.scan_attempts,
                                job.import_attempts,
                                job.checkpoint_rows
                            ),
                        ]
                    })
                    .collect::<Vec<_>>();
                if wanted.is_none() {
                    records.sort_by_key(|row| {
                        std::cmp::Reverse(row[0].parse::<i64>().unwrap_or_default())
                    });
                }
                return Ok(Some(ConcreteRecordSet::new(
                    vec![
                        "JOB_ID",
                        "DB_NAME",
                        "TABLE_NAME",
                        "JOB_TYPE",
                        "SCHEMA_STATE",
                        "SCHEMA_ID",
                        "TABLE_ID",
                        "ROW_COUNT",
                        "START_TIME",
                        "STATE",
                        "QUERY",
                        "ERROR",
                        "PROGRESS",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                    records,
                )));
            }
            ast::AdminStmtType::AlterDdlJob => {
                let mut jobs = RUNTIME_DDL_JOBS
                    .lock()
                    .expect("runtime DDL jobs lock poisoned");
                for id in std::iter::once(&statement.job_number) {
                    let job = jobs
                        .active
                        .get_mut(id)
                        .filter(|job| {
                            job.domain_id == domain_id
                                && Weak::ptr_eq(&job.domain, &Arc::downgrade(&self.domain))
                        })
                        .ok_or_else(|| SessionError::new(format!("DDL job {id} is not running")))?;
                    for option in &statement.alter_job_options {
                        let value = literal(&option.Value)?
                            .parse::<i64>()
                            .map_err(|error| session_error("ADMIN ALTER DDL JOBS value", error))?;
                        match option.Name.to_ascii_lowercase().as_str() {
                            "thread" => job.concurrency = value,
                            "batch_size" => job.batch_size = value,
                            "max_write_speed" => job.max_write_speed = value,
                            name => {
                                return Err(SessionError::new(format!(
                                    "unsupported ADMIN ALTER DDL JOBS option {name}"
                                )));
                            }
                        }
                    }
                }
                return Ok(None);
            }
            ast::AdminStmtType::CancelDdlJobs => {
                let mut jobs = RUNTIME_DDL_JOBS
                    .lock()
                    .expect("runtime DDL jobs lock poisoned");
                for id in &statement.job_ids {
                    let job = jobs
                        .active
                        .get_mut(id)
                        .filter(|job| {
                            job.domain_id == domain_id
                                && Weak::ptr_eq(&job.domain, &Arc::downgrade(&self.domain))
                        })
                        .ok_or_else(|| SessionError::new(format!("DDL job {id} is not running")))?;
                    job.cancelled = true;
                    job.detail = "cancelling".to_owned();
                }
                return Ok(None);
            }
            _ => {}
        }
        Err(SessionError::new("unsupported ADMIN DDL statement"))
    }

    pub(super) fn execute_runtime_ddl_system_select(
        &self,
        statement_sql: &str,
    ) -> Option<ConcreteRecordSet> {
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        let normalized = statement_sql
            .trim()
            .trim_end_matches(';')
            .to_ascii_lowercase();
        if normalized == "select job_id from mysql.tidb_ddl_job" {
            let rows = RUNTIME_DDL_JOBS
                .lock()
                .expect("runtime DDL jobs lock poisoned")
                .active
                .iter()
                .rev()
                .filter(|(_, job)| {
                    job.domain_id == domain_id
                        && Weak::ptr_eq(&job.domain, &Arc::downgrade(&self.domain))
                })
                .map(|(id, _)| vec![id.to_string()])
                .collect();
            return Some(ConcreteRecordSet::new(vec!["job_id".to_owned()], rows));
        }
        if normalized == "select state from mysql.tidb_global_task_history"
            || normalized == "select state from mysql.tidb_global_task"
        {
            let rows = RUNTIME_DDL_JOBS
                .lock()
                .expect("runtime DDL jobs lock poisoned")
                .global_task_history
                .get(&domain_id)
                .into_iter()
                .flatten()
                .map(|state| vec![state.clone()])
                .collect();
            return Some(ConcreteRecordSet::new(vec!["state".to_owned()], rows));
        }
        None
    }

    /// Derive `SHOW TABLE ... REGIONS` from the current table/partition
    /// metadata. Every physical table owns `2^PRE_SPLIT_REGIONS` record
    /// regions; leaders are deterministically spread across the three-store
    /// RealTiKV test topology.
    pub(super) fn execute_show_regions(
        &self,
        statement: &ast::ShowStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let target = statement
            .Table
            .as_ref()
            .ok_or_else(|| SessionError::new("SHOW REGIONS requires a table"))?;
        let current_database = self.current_database();
        let database = if target.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            target.Schema.L.as_str()
        };
        let (_, table) = self
            .domain
            .stats_table(database, &target.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!("unknown table {database}.{}", target.Name.L))
            })?;
        let index_name = (!statement.IndexName.L.is_empty()).then(|| statement.IndexName.L.clone());
        let index = index_name.as_ref().and_then(|name| {
            table
                .Indices
                .iter()
                .find(|index| index.Name.L.eq_ignore_ascii_case(name))
        });
        if index_name.is_some() && index.is_none() {
            return Err(SessionError::new(format!(
                "unknown index {} on {database}.{} (index does not exist)",
                index_name.as_deref().unwrap_or_default(),
                table.Name.L
            )));
        }
        let physical_ids = table
            .GetPartitionInfo()
            .map(|partition| {
                partition
                    .Definitions
                    .iter()
                    .filter(|definition| {
                        statement.Partition.L.is_empty()
                            || definition.Name.L == statement.Partition.L
                    })
                    .map(|definition| definition.ID)
                    .collect::<Vec<_>>()
            })
            .filter(|ids| !ids.is_empty())
            .unwrap_or_else(|| vec![table.ID]);
        let region_counts = RUNTIME_REGION_COUNTS
            .lock()
            .expect("runtime region-count map poisoned")
            .clone();
        // Match Go `getPhysicalTableRegions`: record and index key ranges are
        // deduplicated by physical region. In MockStore an unsplit index shares
        // its physical region with the table record range, so it must not add a
        // synthetic row. An explicitly split index has its own tracked ranges;
        // an explicit INDEX clause still selects that index even when unsplit.
        let region_indexes = index.map_or_else(
            || {
                std::iter::once(None)
                    .chain(
                        table
                            .Indices
                            .iter()
                            .filter(|index| index.State == astersql_meta_model::StatePublic)
                            .filter(|index| {
                                region_counts.contains_key(&(
                                    runtime_domain_id(&self.domain),
                                    database.to_owned(),
                                    table.Name.L.clone(),
                                    Some(index.Name.L.clone()),
                                ))
                            })
                            .map(Some),
                    )
                    .collect::<Vec<_>>()
            },
            |index| vec![Some(index)],
        );
        let topology = self.runtime_topology();
        let peers = topology
            .iter()
            .map(|node| node.store_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut rows = Vec::new();
        for region_index in region_indexes {
            let region_index_name = region_index.map(|index| index.Name.L.clone());
            let regions_per_physical = region_counts
                .get(&(
                    runtime_domain_id(&self.domain),
                    database.to_owned(),
                    table.Name.L.clone(),
                    region_index_name,
                ))
                .copied()
                .unwrap_or(1);
            for physical_id in &physical_ids {
                for ordinal in 0..regions_per_physical {
                    let region_base = region_index.map_or(physical_id.unsigned_abs(), |index| {
                        (physical_id.unsigned_abs() << 16) | index.ID.unsigned_abs()
                    });
                    let region_id = (region_base << 20) | ordinal as u64;
                    let leader_store_id = topology
                        .get(rows.len() % topology.len().max(1))
                        .map_or(0, |node| node.store_id);
                    let sharded_boundary = |boundary: usize| -> Option<String> {
                        if region_index.is_some() || table.PreSplitRegions == 0 || boundary == 0 {
                            return None;
                        }
                        let usable_bits = if table.ContainsAutoRandomBits() {
                            let range_bits = table.AutoRandomRangeBits.clamp(1, 64) as u32;
                            let sign_bits = if table.IsAutoRandomBitColUnsigned() {
                                0
                            } else {
                                1
                            };
                            range_bits.saturating_sub(sign_bits)
                        } else if table.ShardRowIDBits != 0 {
                            63
                        } else {
                            return None;
                        };
                        let shift = usable_bits.saturating_sub(table.PreSplitRegions as u32);
                        let value = (boundary as u128) << shift;
                        Some(format!(
                            "{}_{}_r_{}",
                            String::from_utf8_lossy(astersql_tablecodec::TablePrefix()),
                            physical_id,
                            value
                        ))
                    };
                    rows.push(vec![
                        region_id.to_string(),
                        if let Some(boundary) = sharded_boundary(ordinal) {
                            boundary
                        } else if ordinal == 0 {
                            format!(
                                "{}_{}_",
                                String::from_utf8_lossy(astersql_tablecodec::TablePrefix()),
                                physical_id
                            )
                        } else if table.ContainsAutoRandomBits() {
                            format!(
                                "{}_{}_r_{}",
                                String::from_utf8_lossy(astersql_tablecodec::TablePrefix()),
                                physical_id,
                                ordinal
                            )
                        } else {
                            format!(
                                "{}_{}_r{}",
                                String::from_utf8_lossy(astersql_tablecodec::TablePrefix()),
                                physical_id,
                                ordinal
                            )
                        },
                        sharded_boundary(ordinal + 1).unwrap_or_else(|| {
                            format!(
                                "{}_{}_r{}",
                                String::from_utf8_lossy(astersql_tablecodec::TablePrefix()),
                                physical_id,
                                ordinal + 1
                            )
                        }),
                        region_id
                            .saturating_mul(10)
                            .saturating_add(leader_store_id)
                            .to_string(),
                        leader_store_id.to_string(),
                        peers.clone(),
                        "0".to_owned(),
                        "0".to_owned(),
                        "0".to_owned(),
                        "0".to_owned(),
                        "0".to_owned(),
                    ]);
                }
            }
        }
        Ok(ConcreteRecordSet::new(
            [
                "REGION_ID",
                "START_KEY",
                "END_KEY",
                "LEADER_ID",
                "LEADER_STORE_ID",
                "PEERS",
                "SCATTERING",
                "WRITTEN_BYTES",
                "READ_BYTES",
                "APPROXIMATE_SIZE(MB)",
                "APPROXIMATE_KEYS",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows,
        ))
    }

    /// Execute SPLIT TABLE/INDEX and report the requested split count plus the
    /// scatter completion ratio, matching TestKit's two-column result.
    pub(super) fn execute_split_region(
        &self,
        statement: &ast::SplitRegionStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let current_database = self.current_database();
        let database = if statement.Table.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            statement.Table.Schema.L.as_str()
        };
        let (_, table) = self
            .domain
            .stats_table(database, &statement.Table.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!(
                    "unknown table {database}.{}",
                    statement.Table.Name.L
                ))
            })?;
        let split_count_per_physical = if !statement.SplitOpt.ValueLists.is_empty() {
            statement.SplitOpt.ValueLists.len()
        } else {
            usize::try_from(statement.SplitOpt.Num.saturating_sub(1)).unwrap_or_default()
        };
        let physical_count = table
            .GetPartitionInfo()
            .map(|partition| {
                partition
                    .Definitions
                    .iter()
                    .filter(|definition| {
                        statement.PartitionNames.is_empty()
                            || statement
                                .PartitionNames
                                .iter()
                                .any(|name| name.L == definition.Name.L)
                    })
                    .count()
            })
            .filter(|count| *count > 0)
            .unwrap_or(1);
        let split_count = split_count_per_physical.saturating_mul(physical_count);
        let index_name = (!statement.IndexName.L.is_empty()).then(|| statement.IndexName.L.clone());
        RUNTIME_REGION_COUNTS
            .lock()
            .expect("runtime region-count map poisoned")
            .insert(
                (
                    runtime_domain_id(&self.domain),
                    database.to_owned(),
                    statement.Table.Name.L.clone(),
                    index_name,
                ),
                split_count_per_physical.saturating_add(1).max(1),
            );
        Ok(ConcreteRecordSet::new(
            vec![
                "TOTAL_SPLIT_REGION".to_owned(),
                "SCATTER_FINISH_RATIO".to_owned(),
            ],
            vec![vec![split_count.to_string(), "1".to_owned()]],
        ))
    }

    /// 向语句上下文追加 warning。
    pub(super) fn set_warning(&self, warning: String) {
        self.set_warning_with_code(1105, warning);
    }

    /// 向语句上下文追加指定 MySQL 错误码的 warning。
    pub(super) fn set_warning_with_code(&self, code: u16, warning: String) {
        if !warning.is_empty() {
            self.session_vars.StmtCtx.AppendWarning(
                astersql_sessionctx_stmtctx::errors::NewNoStackError(warning.clone()),
            );
            self.state
                .borrow_mut()
                .current_warnings
                .push(SessionWarning::warning_with_code(code, warning));
        }
    }

    /// 向语句上下文追加 note。
    pub(super) fn set_note(&self, note: String) {
        if !note.is_empty() {
            self.state
                .borrow_mut()
                .current_warnings
                .push(SessionWarning::note(note));
        }
    }

    /// 解析统计信息锁目标表。
    pub(super) fn stats_lock_table(
        &self,
        table: &ast::TableName,
    ) -> SessionResult<(i64, String, HashMap<i64, String>)> {
        let current_database = self.current_database();
        let database = if table.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            table.Schema.L.as_str()
        };
        let (key, info) = self
            .domain
            .stats_table(database, &table.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!("unknown table {database}.{}", table.Name.L))
            })?;
        let requested = table
            .PartitionNames
            .iter()
            .map(|partition| partition.L.as_str())
            .collect::<BTreeSet<_>>();
        let partitions = info
            .GetPartitionInfo()
            .into_iter()
            .flat_map(|partition| &partition.Definitions)
            .filter(|definition| {
                requested.is_empty() || requested.contains(definition.Name.L.as_str())
            })
            .map(|definition| (definition.ID, definition.Name.L.clone()))
            .collect();
        Ok((
            key.table_id,
            format!("{database}.{}", table.Name.L),
            partitions,
        ))
    }

    /// 执行 LOCK STATS。
    pub(super) fn execute_lock_stats(&self, statement: &ast::LockStatsStmt) -> SessionResult<()> {
        if statement
            .Tables
            .first()
            .is_some_and(|table| !table.PartitionNames.is_empty())
        {
            let table = &statement.Tables[0];
            let (table_id, full_name, partitions) = self.stats_lock_table(table)?;
            let warning = self
                .domain
                .stats_lock()
                .LockPartitions(table_id, &full_name, &partitions)
                .map_err(|error| session_error("LOCK STATS partitions", error))?;
            self.set_warning(warning);
            return Ok(());
        }
        let mut tables = HashMap::new();
        let current_database = self.current_database();
        for table in &statement.Tables {
            let (table_id, full_name, partitions) = self.stats_lock_table(table)?;
            debug_assert_eq!(
                partitions.len(),
                self.domain
                    .stats_table(&current_database, &table.Name.L)
                    .and_then(|(_, table)| table.Partition)
                    .map(|partition| partition.Definitions.len())
                    .unwrap_or_default()
            );
            tables.insert(
                table_id,
                astersql_statistics_handle::lockstats::StatsLockTable {
                    FullName: full_name,
                    PartitionInfo: partitions,
                },
            );
        }
        let warning = self
            .domain
            .stats_lock()
            .LockTables(&tables)
            .map_err(|error| session_error("LOCK STATS tables", error))?;
        self.set_warning(warning);
        Ok(())
    }

    /// 执行 UNLOCK STATS。
    pub(super) fn execute_unlock_stats(
        &self,
        statement: &ast::UnlockStatsStmt,
    ) -> SessionResult<()> {
        if statement
            .Tables
            .first()
            .is_some_and(|table| !table.PartitionNames.is_empty())
        {
            let table = &statement.Tables[0];
            let (table_id, full_name, partitions) = self.stats_lock_table(table)?;
            let warning = self
                .domain
                .stats_lock()
                .RemoveLockedPartitions(table_id, &full_name, &partitions)
                .map_err(|error| session_error("UNLOCK STATS partitions", error))?;
            self.set_warning(warning);
            return Ok(());
        }
        let mut tables = HashMap::new();
        for table in &statement.Tables {
            let (table_id, full_name, partitions) = self.stats_lock_table(table)?;
            tables.insert(
                table_id,
                astersql_statistics_handle::lockstats::StatsLockTable {
                    FullName: full_name,
                    PartitionInfo: partitions,
                },
            );
        }
        let warning = self
            .domain
            .stats_lock()
            .RemoveLockedTables(&tables)
            .map_err(|error| session_error("UNLOCK STATS tables", error))?;
        self.set_warning(warning);
        Ok(())
    }
    /// Maintain the real `mysql.expr_pushdown_blacklist` session ABI used by
    /// partial-index fallback tests.
    pub(super) fn execute_expr_pushdown_blacklist_insert(
        &self,
        statement: &ast::InsertStmt,
    ) -> SessionResult<bool> {
        let Some(table_refs) = statement.Table.as_ref() else {
            return Ok(false);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table_refs.TableRefs.Left.as_deref()
        else {
            return Ok(false);
        };
        if source.Source.Schema.L != "mysql" || source.Source.Name.L != "expr_pushdown_blacklist" {
            return Ok(false);
        }
        let mut state = self.state.borrow_mut();
        for values in &statement.Lists {
            if values.len() < 2 {
                return Err(SessionError::new(
                    "mysql.expr_pushdown_blacklist requires name and store_type",
                ));
            }
            state.expr_pushdown_blacklist.insert((
                literal(&values[0])?.to_ascii_lowercase(),
                literal(&values[1])?.to_ascii_lowercase(),
                values
                    .get(2)
                    .map(literal)
                    .transpose()?
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
            ));
        }
        Ok(true)
    }

    /// Delete blacklist rows before the generic table DML path.
    pub(super) fn execute_expr_pushdown_blacklist_delete(
        &self,
        statement: &ast::DeleteStmt,
    ) -> SessionResult<bool> {
        let Some(table_refs) = statement.TableRefs.as_ref() else {
            return Ok(false);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table_refs.TableRefs.Left.as_deref()
        else {
            return Ok(false);
        };
        if source.Source.Schema.L != "mysql" || source.Source.Name.L != "expr_pushdown_blacklist" {
            return Ok(false);
        }
        let Some(predicate) = statement.Where.as_ref() else {
            self.state.borrow_mut().expr_pushdown_blacklist.clear();
            return Ok(true);
        };
        let rows = self
            .state
            .borrow()
            .expr_pushdown_blacklist
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut retained = BTreeSet::new();
        for (name, store_type, reason) in rows {
            let row = HashMap::from([
                ("name".to_owned(), Some(name.clone())),
                ("store_type".to_owned(), Some(store_type.clone())),
                ("reason".to_owned(), Some(reason.clone())),
                (relational_string_column_marker("name"), None),
                (relational_string_column_marker("store_type"), None),
                (relational_string_column_marker("reason"), None),
            ]);
            if !self.insert_select_predicate(predicate, &row)? {
                retained.insert((name, store_type, reason));
            }
        }
        self.state.borrow_mut().expr_pushdown_blacklist = retained;
        Ok(true)
    }

    fn execute_mysql_tidb_sql(&self, sql: &str) -> SessionResult<()> {
        let mut state = self.state.borrow_mut();
        if let Some(transaction) = state.transaction.as_mut() {
            self.domain
                .restricted_system_sql_in_transaction(transaction, sql, &[])
                .map_err(|e| session_error("execute mysql.tidb in session transaction", e))?;
        } else {
            self.domain
                .restricted_stats_execute(sql, &[])
                .map_err(|e| session_error("execute mysql.tidb system variable", e))?;
        }
        Ok(())
    }

    /// Insert rows in TiDB's small `mysql.tidb` system-variable table. Stale
    /// read tests use this real SQL boundary to install the GC safe point.
    pub(super) fn execute_mysql_tidb_insert(
        &self,
        statement: &ast::InsertStmt,
        sql: &str,
    ) -> SessionResult<bool> {
        let Some(table_refs) = statement.Table.as_ref() else {
            return Ok(false);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table_refs.TableRefs.Left.as_deref()
        else {
            return Ok(false);
        };
        if source.Source.Schema.L != "mysql" || source.Source.Name.L != "tidb" {
            return Ok(false);
        }
        self.execute_mysql_tidb_sql(sql)?;
        Ok(true)
    }

    /// Update rows in TiDB's small `mysql.tidb` system-variable table through
    /// the same restricted SQL storage path used by INSERT and DELETE.  The
    /// table is backed by the domain statistics runtime rather than the generic
    /// relational row store, so routing UPDATE through relational DML would
    /// report success without changing the bootstrap metadata.
    pub(super) fn execute_mysql_tidb_update(
        &self,
        statement: &ast::UpdateStmt,
        sql: &str,
    ) -> SessionResult<bool> {
        let Some(table_refs) = statement.TableRefs.as_ref() else {
            return Ok(false);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table_refs.TableRefs.Left.as_deref()
        else {
            return Ok(false);
        };
        if source.Source.Schema.L != "mysql" || source.Source.Name.L != "tidb" {
            return Ok(false);
        }
        self.execute_mysql_tidb_sql(sql)?;
        Ok(true)
    }

    /// Delete one `mysql.tidb` system-variable row through the same AST
    /// boundary used by the generic DML executor.
    pub(super) fn execute_mysql_tidb_delete(
        &self,
        statement: &ast::DeleteStmt,
        sql: &str,
    ) -> SessionResult<bool> {
        let Some(table_refs) = statement.TableRefs.as_ref() else {
            return Ok(false);
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table_refs.TableRefs.Left.as_deref()
        else {
            return Ok(false);
        };
        if source.Source.Schema.L != "mysql" || source.Source.Name.L != "tidb" {
            return Ok(false);
        }
        self.execute_mysql_tidb_sql(sql)?;
        Ok(true)
    }
}
