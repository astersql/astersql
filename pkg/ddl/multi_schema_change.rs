// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 多 schema 变更（Multi-Schema Change）DDL 逻辑。
//
// 一条 `ALTER TABLE` 可包含多个子操作（加列、删索引等）。本模块把它们拆成
// 若干 SubJob，在可回滚（revertible）阶段逐个或批量推进 schema 状态，
// 进入不可回滚阶段后继续收尾；并提供列/索引冲突校验、合并 AddIndex、
// 外键依赖检查、磁盘满暂停提升与完成 job 等辅助逻辑。
//

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 多 schema 父 job / 子 job 的执行状态（对齐 DDL job 状态机）。
pub enum MultiJobState {
    /// 尚未入队。
    None,
    /// 排队等待调度。
    Queueing,
    /// 正在执行。
    Running,
    /// 正在暂停。
    Pausing,
    /// 已暂停。
    Paused,
    /// 正在取消。
    Cancelling,
    /// 已取消。
    Cancelled,
    /// 正在回滚。
    RollingBack,
    /// 回滚完成。
    RollbackDone,
    /// 成功完成。
    Done,
}

impl MultiJobState {
    /// 是否已到终态（取消完成/回滚完成/成功）。
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Cancelled | Self::RollbackDone | Self::Done)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 可放入 multi-schema change 的子操作类型。
pub enum MultiAction {
    /// 加列：可带生成列依赖与 AFTER 定位列。
    AddColumn {
        name: String,
        dependencies: Vec<String>,
        after: Option<String>,
    },
    /// 删列。
    DropColumn(String),
    /// 改列（可改名/类型），可选 AFTER 定位。
    ModifyColumn {
        old: String,
        new: String,
        after: Option<String>,
    },
    /// 设置列默认值。
    SetDefault(String),
    /// 加二级索引：列列表及隐藏列依赖。
    AddIndex {
        name: String,
        columns: Vec<String>,
        hidden_dependencies: Vec<String>,
    },
    /// 删二级索引。
    DropIndex(String),
    /// 重命名索引。
    RenameIndex { from: String, to: String },
    /// 修改索引可见性（VISIBLE/INVISIBLE）。
    AlterIndexVisibility(String),
    /// 加主键。
    AddPrimaryKey { name: String, columns: Vec<String> },
    /// 删主键。
    DropPrimaryKey(String),
    /// 加外键（可能依赖同语句中新建的索引顺序）。
    AddForeignKey { name: String, columns: Vec<String> },
    /// 删外键。
    DropForeignKey(String),
    /// 调整自增 ID 基线。
    RebaseAutoId,
    /// 修改表注释。
    ModifyComment,
    /// 修改表字符集/排序规则。
    ModifyCharset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// multi-schema 中的单个子任务及其推进状态。
pub struct SubJob {
    /// 子操作内容。
    pub action: MultiAction,
    /// 子 job 当前状态。
    pub state: MultiJobState,
    /// 该子 job 产生的最大 schema 版本号。
    pub schema_version: u64,
    /// 是否仍处于可回滚阶段。
    pub revertible: bool,
    /// 是否需要数据重组（reorg/backfill）。
    pub need_reorg: bool,
    /// 子 job 错误信息。
    pub error: Option<String>,
    /// 非致命告警，完成后可回写到 session。
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
/// 多 schema 变更的汇总元信息：子 job 列表与冲突检测用的列/索引集合。
pub struct MultiSchemaInfo {
    /// 父 job 是否仍可整体回滚。
    pub revertible: bool,
    /// 有序子 job 列表。
    pub sub_jobs: Vec<SubJob>,
    /// 本次新增的列名（小写）。
    pub add_columns: Vec<String>,
    /// 本次删除的列名。
    pub drop_columns: Vec<String>,
    /// 本次原地修改的列名。
    pub modify_columns: Vec<String>,
    /// AFTER 子句引用的相对列。
    pub position_columns: Vec<String>,
    /// 索引/生成列等依赖的相对列。
    pub relative_columns: Vec<String>,
    /// 本次新增的索引名。
    pub add_indexes: Vec<String>,
    /// 本次删除的索引名。
    pub drop_indexes: Vec<String>,
    /// 本次修改可见性的索引名。
    pub alter_indexes: Vec<String>,
    /// 本次新增外键规格。
    pub add_foreign_keys: Vec<ForeignKeySpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 外键简要规格：名称与引用列前缀。
pub struct ForeignKeySpec {
    /// 外键名。
    pub name: String,
    /// 外键列（与索引左前缀匹配）。
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 多 schema 变更父 job：聚合状态、子信息与暂停/恢复原因。
pub struct MultiSchemaJob {
    /// 父 job 状态。
    pub state: MultiJobState,
    /// 子 job 与冲突检测字段。
    pub info: MultiSchemaInfo,
    /// 恢复原因（若有）。
    pub resume_reason: Option<String>,
    /// 暂停原因（如 KV 磁盘满）。
    pub pause_reason: Option<String>,
    /// 父 job 级错误。
    pub error: Option<String>,
    /// 已产生的最大 schema 版本。
    pub schema_version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 冲突校验用的表外形：可见列数、列上限、是否分区、索引列表。
pub struct TableShape {
    /// 当前可见列数量。
    pub visible_columns: usize,
    /// 允许的最大列数。
    pub max_columns: usize,
    /// 是否为分区表。
    pub partitioned: bool,
    /// 现有索引外形。
    pub indexes: Vec<IndexShape>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引外形：名称、列前缀、是否处于写重组状态。
pub struct IndexShape {
    /// 索引名。
    pub name: String,
    /// 索引列顺序。
    pub columns: Vec<String>,
    /// 是否处于 WriteReorganization（写重组/回填）状态。
    pub write_reorganization: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// multi-schema 校验与执行过程中的错误类别。
pub enum MultiSchemaError {
    /// 不支持放入 multi-schema 的动作。
    UnsupportedAction,
    /// 同一语句重复操作同一列。
    OperateSameColumn(String),
    /// 同一语句重复操作同一索引。
    OperateSameIndex(String),
    /// 删列后可见列过少。
    TooFewColumns,
    /// 加列后超过列数上限。
    TooManyColumns,
    /// 删除的索引正是新增外键唯一可用依赖。
    IndexNeededByForeignKey(String),
    /// 变更被取消。
    Cancelled,
    /// 物理/存储层不可恢复错误。
    Physical(String),
    /// 子 job 执行失败。
    SubJob(String),
}

impl std::fmt::Display for MultiSchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MultiSchemaError {}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 单次 `run_step` 的结果：版本、状态、错误与磁盘满暂停标记。
pub struct StepResult {
    /// 本次步进产生的 schema 版本（0 表示未生成）。
    pub version: u64,
    /// 步进后的子 job 状态。
    pub state: MultiJobState,
    /// 步进错误。
    pub error: Option<String>,
    /// 是否因 KV 磁盘满而暂停。
    pub paused_for_disk_full: bool,
    /// 暂停原因文案。
    pub pause_reason: Option<String>,
}

/// 子 job 单步执行器：由上层注入真实 DDL 步进或测试桩。
pub trait SubJobRunner {
    /// 推进一个子 job；`rolling_back` 表示回滚路径，`skip_version` 表示复用已生成版本。
    fn run_step(
        &mut self,
        sub_job: &SubJob,
        rolling_back: bool,
        skip_version: bool,
    ) -> Result<StepResult, MultiSchemaError>;
}

/// 填充冲突检测字段后，追加一个默认可回滚的 SubJob。
pub fn append_to_sub_jobs(
    info: &mut MultiSchemaInfo,
    action: MultiAction,
    need_reorg: bool,
) -> Result<(), MultiSchemaError> {
    fill_multi_schema_info(info, &action)?;
    info.sub_jobs.push(SubJob {
        action,
        state: MultiJobState::None,
        schema_version: 0,
        revertible: true,
        need_reorg,
        error: None,
        warning: None,
    });
    Ok(())
}

/// 按子操作类型收集会影响的列、索引与外键，供后续冲突校验。
pub fn fill_multi_schema_info(
    info: &mut MultiSchemaInfo,
    action: &MultiAction,
) -> Result<(), MultiSchemaError> {
    let lower = |name: &str| name.to_ascii_lowercase();
    match action {
        MultiAction::AddColumn {
            name,
            dependencies,
            after,
        } => {
            info.add_columns.push(lower(name));
            info.relative_columns
                .extend(dependencies.iter().map(|name| lower(name)));
            if let Some(name) = after {
                info.position_columns.push(lower(name));
            }
        }
        MultiAction::DropColumn(name) => info.drop_columns.push(lower(name)),
        MultiAction::ModifyColumn { old, new, after } => {
            if old.eq_ignore_ascii_case(new) {
                info.modify_columns.push(lower(new));
            } else {
                info.drop_columns.push(lower(old));
                info.add_columns.push(lower(new));
            }
            if let Some(name) = after {
                info.position_columns.push(lower(name));
            }
        }
        MultiAction::SetDefault(name) => info.modify_columns.push(lower(name)),
        MultiAction::AddIndex {
            name,
            columns,
            hidden_dependencies,
        } => {
            info.add_indexes.push(lower(name));
            info.relative_columns
                .extend(columns.iter().map(|name| lower(name)));
            info.relative_columns
                .extend(hidden_dependencies.iter().map(|name| lower(name)));
        }
        MultiAction::AddPrimaryKey { name, columns } => {
            info.add_indexes.push(lower(name));
            info.relative_columns
                .extend(columns.iter().map(|name| lower(name)));
        }
        MultiAction::DropIndex(name) | MultiAction::DropPrimaryKey(name) => {
            info.drop_indexes.push(lower(name))
        }
        MultiAction::RenameIndex { from, to } => {
            info.add_indexes.push(lower(from));
            info.drop_indexes.push(lower(to));
        }
        MultiAction::AlterIndexVisibility(name) => info.alter_indexes.push(lower(name)),
        MultiAction::AddForeignKey { name, columns } => {
            info.add_foreign_keys.push(ForeignKeySpec {
                name: lower(name),
                columns: columns.iter().map(|name| lower(name)).collect(),
            })
        }
        MultiAction::DropForeignKey(_)
        | MultiAction::RebaseAutoId
        | MultiAction::ModifyComment
        | MultiAction::ModifyCharset => {}
    }
    Ok(())
}

/// 检查同一 multi-schema change 是否重复操作同一列或同一索引。
pub fn check_operate_same_col_and_idx(info: &MultiSchemaInfo) -> Result<(), MultiSchemaError> {
    let mut modified_columns = BTreeSet::new();
    for name in info
        .add_columns
        .iter()
        .chain(&info.drop_columns)
        .chain(&info.modify_columns)
    {
        let name = name.to_ascii_lowercase();
        if !modified_columns.insert(name.clone()) {
            return Err(MultiSchemaError::OperateSameColumn(name));
        }
    }
    for name in info.position_columns.iter().chain(&info.relative_columns) {
        let name = name.to_ascii_lowercase();
        if modified_columns.contains(&name) {
            return Err(MultiSchemaError::OperateSameColumn(name));
        }
    }
    let mut modified_indexes = BTreeSet::new();
    for name in info
        .add_indexes
        .iter()
        .chain(&info.drop_indexes)
        .chain(&info.alter_indexes)
    {
        let name = name.to_ascii_lowercase();
        if !modified_indexes.insert(name.clone()) {
            return Err(MultiSchemaError::OperateSameIndex(name));
        }
    }
    Ok(())
}

/// 合并多个 AddIndex 子 job；若存在 AddForeignKey 则保持顺序不合并。
pub fn merge_add_index(info: &mut MultiSchemaInfo) {
    if info
        .sub_jobs
        .iter()
        .any(|job| matches!(job.action, MultiAction::AddForeignKey { .. }))
    {
        // 外键要求加索引顺序不变，因此整段跳过合并。
        return;
    }
    let add_count = info
        .sub_jobs
        .iter()
        .filter(|job| matches!(job.action, MultiAction::AddIndex { .. }))
        .count();
    if add_count <= 1 {
        return;
    }
    let mut names = Vec::new();
    let mut columns = Vec::new();
    let mut hidden = Vec::new();
    let mut template = None;
    info.sub_jobs.retain(|job| {
        if let MultiAction::AddIndex {
            name,
            columns: job_columns,
            hidden_dependencies,
        } = &job.action
        {
            names.push(name.clone());
            columns.extend(job_columns.clone());
            hidden.extend(hidden_dependencies.clone());
            if template.is_none() {
                template = Some(job.clone());
            }
            false
        } else {
            true
        }
    });
    if let Some(mut merged) = template {
        merged.action = MultiAction::AddIndex {
            name: names.join(","),
            columns,
            hidden_dependencies: hidden,
        };
        info.sub_jobs.push(merged);
    }
}

/// 判断 DDL 结束后是否需要 analyze：需开启 DDL analyze、version=2、非分区且有写重组索引。
pub fn check_need_analyze(
    enable_ddl_analyze: bool,
    analyze_version: u64,
    table: &TableShape,
) -> bool {
    enable_ddl_analyze
        && analyze_version == 2
        && !table.partitioned
        && table.indexes.iter().any(|index| index.write_reorganization)
}

/// 防止本次 drop 的索引正好是新增外键唯一可用的依赖索引。
pub fn check_operate_drop_index_used_by_foreign_key(
    info: &MultiSchemaInfo,
    table: &TableShape,
) -> Result<(), MultiSchemaError> {
    let dropping: Vec<&IndexShape> = table
        .indexes
        .iter()
        .filter(|index| {
            info.drop_indexes
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&index.name))
        })
        .collect();
    let remaining: Vec<&IndexShape> = table
        .indexes
        .iter()
        .filter(|index| {
            !info
                .drop_indexes
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&index.name))
        })
        .collect();
    for foreign_key in &info.add_foreign_keys {
        let supports = |index: &&IndexShape| {
            index.columns.len() >= foreign_key.columns.len()
                && index
                    .columns
                    .iter()
                    .zip(&foreign_key.columns)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right))
        };
        if let Some(index) = dropping.iter().find(|index| supports(index)) {
            if !remaining.iter().any(|index| supports(index)) {
                return Err(MultiSchemaError::IndexNeededByForeignKey(
                    index.name.clone(),
                ));
            }
        }
    }
    Ok(())
}

/// 汇总列/索引冲突、列数上下限与外键依赖校验。
pub fn check_multi_schema_info(
    info: &MultiSchemaInfo,
    table: &TableShape,
) -> Result<(), MultiSchemaError> {
    check_operate_same_col_and_idx(info)?;
    let final_columns = table.visible_columns + info.add_columns.len();
    let final_columns = final_columns
        .checked_sub(info.drop_columns.len())
        .ok_or(MultiSchemaError::TooFewColumns)?;
    if final_columns == 0 {
        return Err(MultiSchemaError::TooFewColumns);
    }
    if final_columns > table.max_columns {
        return Err(MultiSchemaError::TooManyColumns);
    }
    check_operate_drop_index_used_by_foreign_key(info, table)
}

/// 把子 job 的 KV 磁盘满暂停提升到父 job，并恢复子 job 暂停前状态以免卡死。
pub fn promote_disk_full_pause(
    parent: &mut MultiSchemaJob,
    sub_index: usize,
    previous: MultiJobState,
    result: &StepResult,
) -> bool {
    if !result.paused_for_disk_full {
        return false;
    }
    parent.info.sub_jobs[sub_index].state = previous;
    parent.state = result.state;
    parent.resume_reason = None;
    parent.pause_reason = result.pause_reason.clone();
    parent.error = result
        .error
        .clone()
        .or_else(|| Some("DDL auto-paused because KV disk is full".to_string()));
    true
}

/// 可回滚阶段遇异常时，将父 job 与未完成子 job 切入回滚/取消。
pub fn handle_revertible_exception(
    job: &mut MultiSchemaJob,
    sub_index: usize,
    error: Option<String>,
) {
    if matches!(
        job.info.sub_jobs[sub_index].state,
        MultiJobState::None | MultiJobState::Queueing | MultiJobState::Running
    ) {
        return;
    }
    job.state = MultiJobState::RollingBack;
    job.error = error;
    for sub in &mut job.info.sub_jobs {
        sub.state = match sub.state {
            MultiJobState::Running => MultiJobState::Cancelling,
            MultiJobState::None | MultiJobState::Queueing => MultiJobState::Cancelled,
            state => state,
        };
    }
}

/// 取消路径：可回滚则标记子 job cancelling/cancelled；不可回滚则恢复 Running。
pub fn rolling_back_multi_schema_change(job: &mut MultiSchemaJob) -> Result<(), MultiSchemaError> {
    if !job.info.revertible {
        job.state = MultiJobState::Running;
        return Ok(());
    }
    for sub in &mut job.info.sub_jobs {
        sub.state = match sub.state {
            MultiJobState::Running => MultiJobState::Cancelling,
            MultiJobState::None | MultiJobState::Queueing => MultiJobState::Cancelled,
            state => state,
        };
    }
    job.state = MultiJobState::RollingBack;
    Err(MultiSchemaError::Cancelled)
}

/// 多 schema 主状态机：回滚 / 可回滚推进 / 批量进入不可回滚 / 收尾非回滚子 job。
pub fn run_multi_schema_change(
    job: &mut MultiSchemaJob,
    runner: &mut dyn SubJobRunner,
) -> Result<u64, MultiSchemaError> {
    // 回滚阶段：逆序推进尚未结束的子 job。
    if job.info.revertible && job.state == MultiJobState::RollingBack {
        if let Some(index) = job
            .info
            .sub_jobs
            .iter()
            .rposition(|sub| !sub.state.is_finished())
        {
            let result = runner.run_step(&job.info.sub_jobs[index], true, false)?;
            job.info.sub_jobs[index].state = result.state;
            job.info.sub_jobs[index].schema_version = result.version;
            job.info.sub_jobs[index].error = result.error;
            job.schema_version = job.schema_version.max(result.version);
            return Ok(job.schema_version);
        }
        job.state = MultiJobState::RollbackDone;
        return Ok(job.schema_version);
    }
    // 可回滚阶段：先跑第一个可执行子 job；全部到边界后批量推进并标记非回滚。
    if job.info.revertible {
        if let Some(index) = job
            .info
            .sub_jobs
            .iter()
            .position(|sub| sub.revertible && !sub.state.is_finished())
        {
            let previous = job.info.sub_jobs[index].state;
            let result = runner.run_step(&job.info.sub_jobs[index], false, false)?;
            job.info.sub_jobs[index].state = result.state;
            job.info.sub_jobs[index].schema_version = result.version;
            job.info.sub_jobs[index].error = result.error.clone();
            job.schema_version = job.schema_version.max(result.version);
            if promote_disk_full_pause(job, index, previous, &result) {
                return Ok(job.schema_version);
            }
            handle_revertible_exception(job, index, result.error.clone());
            return Ok(job.schema_version);
        }
        // 一次性把剩余子 job 推到不可回滚点，仅生成一个 schema 版本。
        let mut generated = false;
        for index in 0..job.info.sub_jobs.len() {
            if job.info.sub_jobs[index].state.is_finished() {
                continue;
            }
            let result = runner.run_step(&job.info.sub_jobs[index], false, generated)?;
            if result.version != 0 {
                generated = true;
                job.schema_version = job.schema_version.max(result.version);
            }
            job.info.sub_jobs[index].state = result.state;
            job.info.sub_jobs[index].schema_version = result.version;
            job.info.sub_jobs[index].error = result.error.clone();
            if result.error.is_some() {
                handle_revertible_exception(job, index, result.error.clone());
                return Ok(job.schema_version);
            }
        }
        job.info.revertible = false;
        return Ok(job.schema_version);
    }
    // 不可回滚阶段：逐个执行剩余子 job。
    if let Some(index) = job
        .info
        .sub_jobs
        .iter()
        .position(|sub| !sub.state.is_finished())
    {
        let result = runner.run_step(&job.info.sub_jobs[index], false, false)?;
        job.info.sub_jobs[index].state = result.state;
        job.info.sub_jobs[index].schema_version = result.version;
        job.info.sub_jobs[index].error = result.error;
        job.schema_version = job.schema_version.max(result.version);
        return Ok(job.schema_version);
    }
    job.state = MultiJobState::Done;
    Ok(job.schema_version)
}

/// 收集各子 job 上的 warning，供 owner 写回 session。
pub fn collect_warnings(job: &MultiSchemaJob) -> Vec<String> {
    job.info
        .sub_jobs
        .iter()
        .filter_map(|sub| sub.warning.clone())
        .collect()
}

/// 取子 job 最大 schema 版本，将父 job 标为 Done 并返回该版本。
pub fn finish_multi_schema_job(job: &mut MultiSchemaJob) -> u64 {
    job.schema_version = job
        .info
        .sub_jobs
        .iter()
        .fold(job.schema_version, |version, sub| {
            version.max(sub.schema_version)
        });
    job.state = MultiJobState::Done;
    job.schema_version
}
