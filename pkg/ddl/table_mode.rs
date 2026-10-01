// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 表模式（Table Mode）元数据与访问控制。
//
// 表模式用于在数据导入（Import）或备份恢复（Restore，如 BR/Lightning）期间
// 保护表结构：处于 Import/Restore 时禁止普通读写与 DDL，仅允许元数据查询
// 与 checksum 类操作，避免导入过程中被并发修改破坏一致性。
//
// 主要内容：
// - [`TableMode`]：Normal / Import / Restore 三种模式；
// - [`alter_table_mode`] / [`on_alter_table_mode`]：模式切换与版本递增；
// - [`table_mode_allows`]：按模式判定某类操作是否允许。

/// 表的运行模式，影响允许执行的操作集合。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableMode {
    /// 普通模式：读写与 DDL 均允许。
    #[default]
    Normal,
    /// 导入模式（如 IMPORT INTO / Lightning 导入中）：仅允许元数据与 checksum。
    Import,
    /// 恢复模式（如 BR restore 中）：权限与 Import 相同，且二者不可直接互转。
    Restore,
}
/// 表模式相关的精简表元信息（测试与状态机用）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    /// 所属 schema（数据库）ID。
    pub schema_id: i64,
    /// 表 ID；为 0 视为表不存在。
    pub table_id: i64,
    /// 当前表模式。
    pub mode: TableMode,
    /// 元数据版本；每次成功切换模式时递增。
    pub version: i64,
}
/// 切换表模式。
///
/// 返回 `Ok(true)` 表示实际发生了切换并递增了 version；
/// `Ok(false)` 表示目标模式与当前相同（幂等）；
/// Import ↔ Restore 直接互转非法。
pub fn alter_table_mode(table: &mut TableInfo, requested: TableMode) -> Result<bool, String> {
    if table.table_id == 0 {
        return Err("table not found".into());
    }
    // 目标与当前相同：幂等成功，不递增版本。
    if table.mode == requested {
        return Ok(false);
    }
    // Import 与 Restore 互转非法，必须先回到 Normal。
    if matches!(
        (table.mode, requested),
        (TableMode::Restore, TableMode::Import) | (TableMode::Import, TableMode::Restore)
    ) {
        return Err(format!(
            "invalid mode transition from {:?} to {:?}",
            table.mode, requested
        ));
    }
    table.mode = requested;
    table.version += 1;
    Ok(true)
}
/// DDL job 入口：校验 schema/table ID 后切换模式。
///
/// 实际发生切换时返回最新 version；同模式与 Go `onAlterTableMode`
/// 一致按 no-op 完成，不更新 schema 并返回零值 version。
pub fn on_alter_table_mode(
    table: &mut TableInfo,
    schema_id: i64,
    table_id: i64,
    mode: TableMode,
) -> Result<i64, String> {
    if table.schema_id != schema_id || table.table_id != table_id {
        return Err("schema or table ID mismatch".into());
    }
    if !alter_table_mode(table, mode)? {
        return Ok(0);
    }
    Ok(table.version)
}

/// 按表模式划分的访问操作类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableOperation {
    /// 元数据类（SHOW / DESCRIBE / CREATE LIKE 等）。
    Metadata,
    /// 校验和（ADMIN CHECKSUM TABLE）。
    Checksum,
    /// 普通读（SELECT 等）。
    Read,
    /// 普通写（INSERT/UPDATE/DELETE 等）。
    Write,
    /// 结构变更（ALTER TABLE 等）。
    Alter,
    /// 删除表。
    Drop,
}

/// 判断给定表模式下是否允许执行某类操作。
///
/// Normal 允许全部操作；Import/Restore 仅允许 Metadata 与 Checksum。
pub fn table_mode_allows(mode: TableMode, operation: TableOperation) -> bool {
    match mode {
        TableMode::Normal => true,
        TableMode::Import | TableMode::Restore => {
            matches!(
                operation,
                TableOperation::Metadata | TableOperation::Checksum
            )
        }
    }
}

/// Normal DDL schema publication/recovery boundary. Implementations must wait
/// for the normal owner's follower/MDL protocol and observe lease cancellation.
pub trait DdlSchemaBarrier {
    fn recover(
        &mut self,
        job: &astersql_meta_model::group_3::Job,
        lease: &dyn crate::job_worker::JobLease,
    ) -> Result<(), String>;
    fn wait(
        &mut self,
        job: &astersql_meta_model::group_3::Job,
        version: i64,
        lease: &dyn crate::job_worker::JobLease,
    ) -> Result<(), String>;
}
/// Upgrade admission belongs to the normal DDL scheduler, not crossks.
pub trait DdlJobPolicy {
    fn runnable(
        &mut self,
        session: &mut dyn crate::job_worker::DurableJobSession,
        job: &astersql_meta_model::group_3::Job,
    ) -> Result<bool, String>;
    fn error_limit(&self) -> i64;
    fn mdl_owner(&self) -> Option<String>;
}
/// Executes persistent TableMode jobs through the normal worker transaction.
pub struct NormalDdlExecutor<B, P> {
    pub barrier: B,
    pub policy: P,
    pub sequence: std::sync::Arc<std::sync::atomic::AtomicI64>,
}
impl<B: DdlSchemaBarrier, P: DdlJobPolicy> crate::job_worker::DurableJobExecutor
    for NormalDdlExecutor<B, P>
{
    fn runnable(
        &mut self,
        session: &mut dyn crate::job_worker::DurableJobSession,
        job: &astersql_meta_model::group_3::Job,
    ) -> Result<bool, String> {
        self.policy.runnable(session, job)
    }
    fn recover(
        &mut self,
        job: &astersql_meta_model::group_3::Job,
        lease: &dyn crate::job_worker::JobLease,
    ) -> Result<(), String> {
        self.barrier.recover(job, lease)
    }
    fn wait_synced(
        &mut self,
        job: &astersql_meta_model::group_3::Job,
        version: i64,
        lease: &dyn crate::job_worker::JobLease,
    ) -> Result<(), String> {
        self.barrier.wait(job, version, lease)
    }
    fn step(
        &mut self,
        session: &mut dyn crate::job_worker::DurableJobSession,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<crate::job_worker::DurableJobStep, String> {
        use astersql_meta_model::group_3::JobState;
        if job.tp != 75 {
            return Err(format!(
                "normal TableMode executor cannot execute action {}",
                job.tp
            ));
        }
        if job.state == JobState::Paused {
            return Ok(crate::job_worker::DurableJobStep {
                schema_version: 0,
                update_raw_args: false,
                removed: false,
            });
        }
        if job.state == JobState::Pausing {
            job.state = JobState::Paused;
            job.error = Some(format!("[ddl:8262]DDL job {} is paused", job.id));
            job.error_count += 1;
            return Ok(crate::job_worker::DurableJobStep {
                schema_version: 0,
                update_raw_args: false,
                removed: false,
            });
        }
        if job.state == JobState::Cancelling {
            job.state = JobState::Cancelled;
            job.error = Some("[ddl:8214]Cancelled DDL job".into());
            job.error_count += 1;
        }
        if matches!(
            job.state,
            JobState::Done | JobState::Synced | JobState::Cancelled | JobState::RollbackDone
        ) {
            return self.finish(session, job);
        }
        let mut current = astersql_meta_model::group_3::Job::decode(
            &job.encode(false).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let limit = self.policy.error_limit();
        let encoded = session.with_transaction(Box::new(move |txn| {
            let stage = txn.StageStatement().map_err(|e| e.to_string())?;
            current.state = JobState::Running;
            if current.real_start_ts == 0 {
                current.real_start_ts = txn.StartTS()
            }
            let action = on_persistent_alter_table_mode(txn, &mut current);
            match action {
                Ok(version) => {
                    txn.ReleaseStatement(stage).map_err(|e| e.to_string())?;
                    current.last_schema_version = version;
                }
                Err(error) => {
                    txn.CleanupStatement(stage).map_err(|e| e.to_string())?;
                    current.error = Some(error);
                    current.error_count += 1;
                    current.last_schema_version = 0;
                    if current.state == JobState::Running && current.error_count > limit {
                        current.state = JobState::Cancelling
                    }
                }
            }
            current.encode(false).map_err(|e| e.to_string())
        }))?;
        *job = astersql_meta_model::group_3::Job::decode(&encoded).map_err(|e| e.to_string())?;
        if job.state == JobState::Cancelled {
            return self.finish(session, job);
        }
        let version = job.last_schema_version;
        if version > 0 {
            if let Some(owner) = self.policy.mdl_owner() {
                let (columns, values) = if is_system_related_schema(&job.schema_name) {
                    (String::new(), String::new())
                } else {
                    (
                        String::from(", owner_id"),
                        format!(", {}", sql_text(&owner)),
                    )
                };
                session.query(&format!("REPLACE INTO mysql.tidb_mdl_info (job_id, version, table_ids{columns}) VALUES ({}, {version}, '{}'{values})",job.id,job.table_id),"register-mdl-info")?;
            }
        }
        Ok(crate::job_worker::DurableJobStep {
            schema_version: version,
            update_raw_args: false,
            removed: false,
        })
    }
}

impl<B: DdlSchemaBarrier, P: DdlJobPolicy> NormalDdlExecutor<B, P> {
    fn finish(
        &mut self,
        session: &mut dyn crate::job_worker::DurableJobSession,
        job: &mut astersql_meta_model::group_3::Job,
    ) -> Result<crate::job_worker::DurableJobStep, String> {
        use astersql_meta_model::group_3::JobState;
        if job.state == JobState::Done {
            job.state = JobState::Synced
        }
        if matches!(job.state, JobState::Cancelled | JobState::RollbackDone) {
            job.ru = 0.0
        }
        job.seq_num = (self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1) as u64;
        let mut current = astersql_meta_model::group_3::Job::decode(
            &job.encode(false).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let encoded = session.with_transaction(Box::new(move |txn| {
            let mut meta = astersql_meta::TransactionMutator::new(txn);
            current
                .binlog_info
                .get_or_insert_with(Default::default)
                .finished_ts = meta.start_ts();
            meta.add_history_ddl_job(&mut current)?;
            current.encode(false).map_err(|e| e.to_string())
        }))?;
        *job = astersql_meta_model::group_3::Job::decode(&encoded).map_err(|e| e.to_string())?;
        let wire = astersql_meta::encode_go_ddl_job(job, false)?;
        let hex = wire.iter().map(|b| format!("{b:02x}")).collect::<String>();
        // Go deliberately tolerates SQL history failure and always stores KV
        // history. The outer worker still fences and commits the same transaction.
        if let Err(error)=session.query(&format!("INSERT IGNORE INTO mysql.tidb_ddl_history (job_id,job_meta,db_name,table_name,schema_ids,table_ids,create_time) VALUES ({},X'{hex}',{},{},'{}','{}',{})",job.id,sql_text(&job.schema_name),sql_text(&job.table_name),job.schema_id,job.table_id,sql_text(&go_tso_datetime(job.start_ts))),"insert_history") {
            eprintln!("failed to add DDL job {} to SQL history: {error}",job.id);
        }
        session.query(
            &format!("DELETE FROM mysql.tidb_ddl_job WHERE job_id={}", job.id),
            "delete_job",
        )?;
        Ok(crate::job_worker::DurableJobStep {
            schema_version: 0,
            update_raw_args: false,
            removed: true,
        })
    }
}
pub(crate) fn sql_text(value: &str) -> String {
    // Hex text is independent of the job's SQLMode and quote/backslash rules.
    format!(
        "X'{}'",
        value
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
pub(crate) fn is_system_related_schema(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "mysql" | "sys" | "workload_schema"
    )
}
fn go_tso_datetime(ts: u64) -> String {
    // Go TSConvert2Time uses milliseconds in the physical TSO component.
    // The public parser's time formatter is wired below.
    astersql_meta::tso_history_datetime(ts)
}
/// Go onAlterTableMode: modify full table metadata inside a caller-owned stage.
/// Cancellation errors carry Go terror display codes; storage failures retain
/// Running so the normal worker can persist its error and retry.
pub fn on_persistent_alter_table_mode(
    txn: &mut dyn astersql_kv::Transaction,
    job: &mut astersql_meta_model::group_3::Job,
) -> Result<i64, String> {
    use astersql_meta_model::group_3::JobState;
    let args: serde_json::Value =
        serde_json::from_slice(&job.raw_args).map_err(|e| format!("[ddl:1105]{e}"))?;
    let args = if job.version == astersql_meta_model::group_3::JobVersion::V1 {
        args.as_array()
            .and_then(|v| v.first())
            .cloned()
            .ok_or("[ddl:1105]invalid V1 TableMode arguments")?
    } else {
        args
    };
    if !args.is_object() {
        return Err("[ddl:1105]invalid TableMode arguments".into());
    }
    let requested = match args.get("table_mode") {
        None => 0,
        Some(value) => value.as_u64().ok_or("[ddl:1105]invalid TableMode value")?,
    };
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    if meta.get_database(job.schema_id)?.is_none() {
        job.state = JobState::Cancelled;
        return Err(format!("[schema:1049]Unknown database '{}'", job.schema_id));
    }
    if let Some(mode) = meta.get_table_mode_value(job.schema_id, job.table_id)? {
        if !(0..=2).contains(&mode) {
            job.state = JobState::Cancelled;
            return Err(format!(
                "[schema:8259]invalid table mode transition from {mode} to {requested}"
            ));
        }
    }
    let Some(mut table) = meta.get_table(job.schema_id, job.table_id)? else {
        job.state = JobState::Cancelled;
        return Err(format!(
            "[schema:1146]Table '{}.{}' doesn't exist",
            job.schema_id, job.table_id
        ));
    };
    if table.State != astersql_meta_model::SchemaState::Public {
        job.state = JobState::Cancelled;
        return Err(format!("[ddl:8210]table {} is not in public", table.Name.O));
    }
    let target = match requested {
        0 => astersql_meta_model::TableMode::TableModeNormal,
        1 => astersql_meta_model::TableMode::TableModeImport,
        2 => astersql_meta_model::TableMode::TableModeRestore,
        _ => {
            job.state = JobState::Cancelled;
            return Err(format!("[schema:8259]invalid table mode {requested}"));
        }
    };
    if table.Mode == target {
        job.state = JobState::Done;
        return Ok(0);
    }
    if !table.Mode.CanTransitionTo(target) {
        job.state = JobState::Cancelled;
        return Err(format!(
            "[schema:8259]invalid table mode transition from {} to {} for {}",
            table.Mode.String(),
            target.String(),
            table.Name.O
        ));
    }
    table.Mode = target;
    let version = meta.gen_schema_version()?;
    meta.set_table_schema_diff(job, version)?;
    meta.update_table(job.schema_id, &mut table)?;
    job.finish_table_job(
        JobState::Done,
        astersql_meta_model::SchemaState::Public,
        version,
        std::sync::Arc::new(table),
    );
    Ok(version)
}
