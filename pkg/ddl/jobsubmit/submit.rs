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

// DDL Job 批量提交：校验、分配全局 ID，并插入 `mysql.tidb_ddl_job` 系统表。
//
// 提交流程概要：
// 1. 检查是否存在 flashback cluster job（集群闪回任务，会阻塞新 DDL）
// 2. 读取 BDR（Bidirectional Replication，双向复制）角色与 start_ts
// 3. 规范化/校验 involving schema，应用 BDR 策略与升级期暂停规则
// 4. 在悲观事务中锁定全局 ID 键、分配 ID、插入 job 行（失败可重试）

use crate::{
    Cleanup, Error, ErrorKind, Job, JobArgs, JobSpec, JobState, JobType, OwnerNotifier,
    PartitionInfo, Session, SubmitOptions, TableInfo,
};

/// 批量提交一组 JobSpec：会话池借还、BDR/升级校验，再进入带重试的 ID 分配与插入。
pub fn submit_batch(options: &SubmitOptions, specs: &mut [JobSpec]) -> Result<(), Error> {
    if specs.is_empty() {
        return Ok(());
    }
    let mut session = options.session_pool.get()?;
    // 闭包内完成业务逻辑，确保无论成败都把 session 归还到池中。
    let result = (|| {
        let min_job_id = options.min_job_id_provider.current_min_job_id();
        // flashback cluster job 存在时禁止新增 DDL，避免与集群闪回冲突。
        if options
            .system_table_manager
            .has_flashback_cluster_job(min_job_id)?
        {
            return Err(Error::invalid(
                "Can't add ddl job, have flashback cluster job",
            ));
        }
        let (bdr_role, start_ts) = session.read_bdr_role_and_start_ts()?;
        for spec in specs.iter_mut() {
            let job = &mut spec.job;
            job.normalize_involving_schema_info();
            job.check_involving_schema_info()?;
            if job.version == 0 {
                return Err(Error::invalid("Job version should not be zero"));
            }
            job.trace_info_present = true;
            job.start_ts = start_ts;
            job.bdr_role.clone_from(&bdr_role);
            // 非系统库上，BDR 角色可能拒绝特定 DDL；MultiSchemaChange 逐子 job 检查。
            if job.cdc_write_source == 0
                && bdr_role != "none"
                && !is_system_schema(&job.schema_name)
            {
                if job.job_type == JobType::MultiSchemaChange {
                    for sub in &job.sub_jobs {
                        if let Some(job_type) = sub.job_type
                            && options.bdr_policy.is_denied(&bdr_role, job_type, &sub.args)
                        {
                            return Err(Error::invalid(format!(
                                "DDL is restricted by BDR role {bdr_role}"
                            )));
                        }
                    }
                } else if options
                    .bdr_policy
                    .is_denied(&bdr_role, job.job_type, &spec.args)
                {
                    return Err(Error::invalid(format!(
                        "DDL is restricted by BDR role {bdr_role}"
                    )));
                }
            }
            set_job_state_to_queueing(job);
            // 集群升级期间：非系统 schema 的 job 标记为系统操作并暂停排队。
            if options
                .server_state
                .as_ref()
                .is_some_and(|state| state.is_upgrading())
                && !job_has_system_schema(job)
            {
                job.admin_operator_system = true;
                job.state = JobState::Pausing;
            }
        }
        generate_ids_and_insert_jobs_with_retry(session.as_mut(), specs, options)
    })();
    options.session_pool.put(session);
    result
}

/// 在悲观事务中分配全局 ID 并插入 DDL job，可对 Retryable 错误退避重试。
pub fn generate_ids_and_insert_jobs_with_retry(
    session: &mut dyn Session,
    specs: &mut [JobSpec],
    options: &SubmitOptions,
) -> Result<(), Error> {
    let count = required_global_id_count(specs);
    let mut last_error = None;
    for attempt in 0..options.max_retry_count.max(1) {
        let mut cleanup: Option<Cleanup> = None;
        // 单次尝试：begin → 锁全局 ID → 分配 →（可选钩子）→ 插入 → commit。
        let mut transaction_started = false;
        let result: Result<(), Error> = (|| {
            session.begin()?;
            transaction_started = true;
            session.set_pessimistic();
            let for_update_ts = lock_global_id_key_with_backoff(session, options.backoff.as_ref())?;
            session.set_snapshot_ts(for_update_ts);
            let ids = session.generate_global_ids(count)?;
            assign_global_ids_for_jobs(specs, &ids)?;
            if let Some(before) = &options.before_insert_with_assigned_ids {
                cleanup = before(specs);
            }
            insert_ddl_jobs_to_table(session, specs)?;
            session.commit()?;
            cleanup = None;
            Ok(())
        })();
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                // 失败时执行钩子 cleanup、回滚事务；仅 Retryable 才退避继续。
                if let Some(cleanup) = cleanup {
                    cleanup();
                }
                if transaction_started {
                    session.rollback();
                }
                let retryable = error.kind == ErrorKind::Retryable;
                last_error = Some(error);
                if retryable {
                    (options.backoff)(attempt);
                    continue;
                }
                break;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| Error::retryable("DDL job insertion exhausted retries")))
}

/// 从预生成的全局 ID 切片中顺序取用的分配器。
struct GlobalIdAllocator<'a> {
    ids: &'a [i64],
    index: usize,
}

impl GlobalIdAllocator<'_> {
    /// 取出下一个全局 ID；耗尽则报错。
    fn next(&mut self) -> Result<i64, Error> {
        let id = self
            .ids
            .get(self.index)
            .copied()
            .ok_or_else(|| Error::invalid("insufficient generated global IDs"))?;
        self.index += 1;
        Ok(id)
    }

    /// 为表及其分区定义分配 ID。
    fn assign_table(&mut self, table: &mut TableInfo) -> Result<(), Error> {
        table.id = self.next()?;
        if let Some(partitions) = &mut table.partitions {
            self.assign_partitions(partitions)?;
        }
        Ok(())
    }

    /// 为每个分区 definition 分配 ID。
    fn assign_partitions(&mut self, partitions: &mut PartitionInfo) -> Result<(), Error> {
        for definition in &mut partitions.definitions {
            definition.id = self.next()?;
        }
        Ok(())
    }
}

/// 计算一张表需要的全局 ID 数量：表本身 1 个 + 各分区各 1 个。
pub fn id_count_for_table(table: &TableInfo) -> usize {
    1 + table
        .partitions
        .as_ref()
        .map_or(0, |info| info.definitions.len())
}

/// 统计本批 JobSpec 所需全局 ID 总数（含每个 job 自身的 job_id）。
///
/// 已标记 `id_allocated` 的 spec 只计 job_id，不再为表/分区预分配。
pub fn required_global_id_count(specs: &[JobSpec]) -> usize {
    let mut count = specs.len();
    for spec in specs {
        if spec.id_allocated {
            continue;
        }
        // 按 JobArgs 与 JobType 组合决定额外 ID 需求（表、分区、schema、资源组等）。
        count += match &spec.args {
            JobArgs::CreateTable { table }
                if matches!(
                    spec.job.job_type,
                    JobType::CreateView | JobType::CreateSequence | JobType::CreateTable
                ) =>
            {
                id_count_for_table(table)
            }
            JobArgs::BatchCreateTable { tables } => tables.iter().map(id_count_for_table).sum(),
            JobArgs::CreateSchema { .. } | JobArgs::ResourceGroup { .. } => 1,
            JobArgs::TablePartition { partition }
                if spec.job.job_type == JobType::AlterTablePartitioning =>
            {
                1 + partition.definitions.len()
            }
            JobArgs::TablePartition { partition }
                if matches!(
                    spec.job.job_type,
                    JobType::AddTablePartition
                        | JobType::ReorganizePartition
                        | JobType::RemovePartitioning
                ) =>
            {
                partition.definitions.len()
            }
            JobArgs::TruncateTable {
                old_partition_ids, ..
            } if spec.job.job_type == JobType::TruncateTable => 1 + old_partition_ids.len(),
            JobArgs::TruncateTable {
                old_partition_ids, ..
            } if spec.job.job_type == JobType::TruncateTablePartition => old_partition_ids.len(),
            _ => 0,
        };
    }
    count
}

/// 将预生成的全局 ID 写入各 JobSpec 的表/分区/schema 字段，并设置 job.id。
pub fn assign_global_ids_for_jobs(specs: &mut [JobSpec], ids: &[i64]) -> Result<(), Error> {
    if ids.len() != required_global_id_count(specs) {
        return Err(Error::invalid(
            "generated global ID count does not match requirement",
        ));
    }
    let mut allocator = GlobalIdAllocator { ids, index: 0 };
    for spec in specs {
        // 按 args/job_type 分支填充对象 ID；最后统一分配 job.id。
        match (&mut spec.args, spec.job.job_type) {
            (
                JobArgs::CreateTable { table },
                JobType::CreateView | JobType::CreateSequence | JobType::CreateTable,
            ) => {
                if !spec.id_allocated {
                    allocator.assign_table(table)?;
                }
                spec.job.table_id = table.id;
            }
            (JobArgs::BatchCreateTable { tables }, JobType::CreateTables) => {
                if !spec.id_allocated {
                    for table in tables {
                        allocator.assign_table(table)?;
                    }
                }
            }
            (JobArgs::CreateSchema { database }, JobType::CreateSchema) => {
                if !spec.id_allocated {
                    database.id = allocator.next()?;
                }
                spec.job.schema_id = database.id;
            }
            (JobArgs::ResourceGroup { group }, JobType::CreateResourceGroup) => {
                if !spec.id_allocated {
                    group.id = allocator.next()?;
                }
            }
            (JobArgs::TablePartition { partition }, JobType::AlterTablePartitioning) => {
                if !spec.id_allocated {
                    allocator.assign_partitions(partition)?;
                    partition.new_table_id = allocator.next()?;
                }
            }
            (
                JobArgs::TablePartition { partition },
                JobType::AddTablePartition | JobType::ReorganizePartition,
            ) => {
                if !spec.id_allocated {
                    allocator.assign_partitions(partition)?;
                }
            }
            (JobArgs::TablePartition { partition }, JobType::RemovePartitioning) => {
                if !spec.id_allocated {
                    allocator.assign_partitions(partition)?;
                }
                // 去分区时用第一个特殊分区的 ID 作为新表 ID。
                partition.new_table_id = partition
                    .definitions
                    .first()
                    .ok_or_else(|| {
                        Error::invalid("remove partitioning requires a special partition")
                    })?
                    .id;
            }
            (
                JobArgs::TruncateTable {
                    old_partition_ids,
                    new_table_id,
                    new_partition_ids,
                },
                JobType::TruncateTable | JobType::TruncateTablePartition,
            ) => {
                if !spec.id_allocated {
                    if spec.job.job_type == JobType::TruncateTable {
                        *new_table_id = allocator.next()?;
                    }
                    *new_partition_ids = old_partition_ids
                        .iter()
                        .map(|_| allocator.next())
                        .collect::<Result<_, _>>()?;
                }
            }
            _ => {}
        }
        spec.job.id = allocator.next()?;
    }
    Ok(())
}

/// 锁定全局 ID 键；遇 WriteConflict（写冲突）则刷新 for_update_ts 后重试。
///
/// for_update_ts 是悲观锁语义下用于校验的时间戳。
pub fn lock_global_id_key(session: &mut dyn Session) -> Result<u64, Error> {
    lock_global_id_key_with_backoff(session, &|_| {})
}

fn lock_global_id_key_with_backoff(
    session: &mut dyn Session,
    backoff: &dyn Fn(usize),
) -> Result<u64, Error> {
    let mut for_update_ts = session.transaction_start_ts()?;
    let mut iteration = 0usize;
    loop {
        match session.lock_global_id_key(for_update_ts) {
            Ok(()) => return Ok(for_update_ts),
            Err(error) if error.kind == ErrorKind::WriteConflict => {
                for_update_ts = session.current_version()?;
                backoff(iteration);
                iteration = iteration.saturating_add(1);
            }
            Err(error) => return Err(error),
        }
    }
}

/// 将各 JobSpec 编码后拼成一条 INSERT，写入 `mysql.tidb_ddl_job`。
pub fn insert_ddl_jobs_to_table(
    session: &mut dyn Session,
    specs: &mut [JobSpec],
) -> Result<(), Error> {
    if specs.is_empty() {
        return Ok(());
    }
    let mut sql = String::from(
        "insert into mysql.tidb_ddl_job(job_id,reorg,schema_ids,table_ids,job_meta,type,processing) values",
    );
    for (index, spec) in specs.iter_mut().enumerate() {
        fill_args_with_sub_jobs(spec);
        let encoded = spec.job.encode(&spec.args);
        if index > 0 {
            sql.push(',');
        }
        // job_meta 以 MySQL hex 字面量（x'...'）形式内嵌。
        sql.push_str(&format!(
            "({}, {}, '{}', '{}', {}, {}, {})",
            spec.job.id,
            spec.job.may_need_reorg(),
            job_schema_ids(spec),
            job_table_ids(spec),
            hex(&encoded),
            spec.job.job_type.code(),
            spec.job.started()
        ));
    }
    session.execute(&sql, "insert_job")
}

/// MultiSchemaChange：为每个子 job 填充 `encoded_args`（版本前缀 + Debug 序列化）。
pub fn fill_args_with_sub_jobs(spec: &mut JobSpec) {
    if spec.job.job_type == JobType::MultiSchemaChange {
        for sub in &mut spec.job.sub_jobs {
            sub.encoded_args = format!("v{}:{:?}", spec.job.version, sub.args).into_bytes();
        }
    }
}

/// 将 ID 集合去重、按十进制字符串字典序排序后用逗号拼接。
pub fn make_string_for_ids(ids: impl IntoIterator<Item = i64>) -> String {
    let mut ids = ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    ids.join(",")
}

/// 根据 job 类型提取涉及的 schema_id 列表字符串（重命名/交换分区会涉及多个）。
pub fn job_schema_ids(spec: &JobSpec) -> String {
    match &spec.args {
        JobArgs::RenameTables { tables } if spec.job.job_type == JobType::RenameTables => {
            make_string_for_ids(
                tables
                    .iter()
                    .flat_map(|info| [info.old_schema_id, info.new_schema_id]),
            )
        }
        JobArgs::RenameTable { old_schema_id } if spec.job.job_type == JobType::RenameTable => {
            make_string_for_ids([*old_schema_id, spec.job.schema_id])
        }
        JobArgs::ExchangePartition {
            partition_schema_id,
            ..
        } if spec.job.job_type == JobType::ExchangeTablePartition => {
            make_string_for_ids([spec.job.schema_id, *partition_schema_id])
        }
        _ => spec.job.schema_id.to_string(),
    }
}

/// 根据 job 类型提取涉及的 table_id 列表字符串。
pub fn job_table_ids(spec: &JobSpec) -> String {
    match &spec.args {
        JobArgs::RenameTables { tables } if spec.job.job_type == JobType::RenameTables => {
            make_string_for_ids(tables.iter().map(|info| info.table_id))
        }
        JobArgs::ExchangePartition {
            partition_table_id, ..
        } if spec.job.job_type == JobType::ExchangeTablePartition => {
            make_string_for_ids([spec.job.table_id, *partition_table_id])
        }
        JobArgs::TruncateTable { new_table_id, .. }
            if spec.job.job_type == JobType::TruncateTable =>
        {
            format!("{},{}", spec.job.table_id, new_table_id)
        }
        _ => spec.job.table_id.to_string(),
    }
}

/// 将 job（及 MultiSchemaChange 的子 job）状态置为 Queueing（排队等待执行）。
pub fn set_job_state_to_queueing(job: &mut Job) {
    if job.job_type == JobType::MultiSchemaChange {
        for sub in &mut job.sub_jobs {
            sub.state = JobState::Queueing;
        }
    }
    job.state = JobState::Queueing;
}

/// 通知 DDL owner 有新任务可调度；notifier 为空时为 no-op。
pub fn notify_ddl_owner(notifier: Option<&dyn OwnerNotifier>) {
    if let Some(notifier) = notifier {
        let _ = notifier.notify();
    }
}

/// 判断是否为系统 schema（mysql / information_schema / performance_schema / sys）。
fn is_system_schema(schema: &str) -> bool {
    matches!(
        schema.to_ascii_lowercase().as_str(),
        "mysql" | "information_schema" | "performance_schema" | "metrics_schema" | "sys"
    )
}

/// 对齐 Go `ddlutil.HasSysDB`：升级暂停判断还要检查所有涉及对象。
fn job_has_system_schema(job: &Job) -> bool {
    is_system_schema(&job.schema_name)
        || job
            .involving_schemas
            .iter()
            .any(|(schema, _)| is_system_schema(schema))
}

/// 将字节编码为 MySQL hex 字面量形式：`x'aabb...'`。
fn hex(bytes: &[u8]) -> String {
    let mut output = String::from("x'");
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output.push('\'');
    output
}
