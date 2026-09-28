// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DDL 作业回滚（rolling back）转换逻辑。
//
// 当 DDL Job 执行失败或被取消时，根据当前 SchemaState
// （元信息可见性状态：None / DeleteOnly / WriteOnly / Public 等）
// 决定直接取消，或改写为反向动作进入 RollingBack，
// 以安全撤销尚未完全生效的 schema 变更。

/// DDL 动作类型，决定回滚策略分支。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobAction {
    AddIndex,
    AddPrimaryKey,
    AddColumn,
    DropColumn,
    ModifyColumn,
    DropIndex,
    ExchangePartition,
    TruncatePartition,
    AddPartition,
    DropPartition,
    ReorganizePartition,
    DropTable,
    DropView,
    DropSchema,
    RenameIndex,
    TruncateTable,
    AddConstraint,
    DropConstraint,
    AlterConstraint,
    Other,
}

/// Schema 对象状态机节点，控制读写可见性与重组阶段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaState {
    None,
    DeleteOnly,
    WriteOnly,
    WriteReorganization,
    DeleteReorganization,
    ReplicaOnly,
    Public,
}

/// DDL Job 自身生命周期状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Running,
    Cancelling,
    RollingBack,
    RollbackDone,
    Cancelled,
}

/// 待回滚的作业快照：动作、schema 状态、错误信息与参数是否已改写。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RollbackJob {
    pub id: i64,
    pub action: JobAction,
    pub schema_state: SchemaState,
    pub state: JobState,
    pub error: Option<String>,
    pub error_count: usize,
    pub args_rewritten: bool,
}

/// 回滚转换结果错误：不可取消、已取消、或状态非法。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RollbackError {
    CannotCancel,
    Cancelled,
    InvalidState,
}

/// 将正在运行的作业转为回滚/取消路径，并记录触发错误。
pub fn convert_job_to_rollback(
    job: &mut RollbackJob,
    occurred_error: impl Into<String>,
) -> Result<(), RollbackError> {
    job.error = Some(occurred_error.into());
    match job.action {
        JobAction::AddIndex | JobAction::AddPrimaryKey => convert_add_index(job),
        JobAction::AddColumn => rollback_add_column(job),
        JobAction::DropColumn => rollback_drop_column(job),
        JobAction::ModifyColumn => rollback_modify_column(job),
        JobAction::DropIndex => rollback_drop_index(job),
        JobAction::ExchangePartition => rollback_exchange_partition(job),
        JobAction::TruncatePartition => convert_truncate_partition(job),
        JobAction::AddPartition => convert_add_partition(job),
        JobAction::ReorganizePartition => convert_reorganize_partition(job),
        JobAction::DropTable
        | JobAction::DropView
        | JobAction::DropSchema
        | JobAction::DropPartition => cancel_only_unhandled(job),
        JobAction::RenameIndex | JobAction::TruncateTable => cancel_only_unhandled(job),
        JobAction::AddConstraint => rollback_add_constraint(job),
        JobAction::DropConstraint | JobAction::AlterConstraint => {
            rollback_drop_or_alter_constraint(job)
        }
        JobAction::Other => cancel(job),
    }
}

/// 添加索引/主键：已有索引元数据时统一改写为 DeleteOnly 回滚。
fn convert_add_index(job: &mut RollbackJob) -> Result<(), RollbackError> {
    match job.schema_state {
        SchemaState::None => cancel(job),
        SchemaState::DeleteOnly | SchemaState::WriteOnly | SchemaState::WriteReorganization => {
            job.state = JobState::RollingBack;
            job.schema_state = SchemaState::DeleteOnly;
            job.args_rewritten = true;
            Ok(())
        }
        SchemaState::DeleteReorganization | SchemaState::ReplicaOnly | SchemaState::Public => {
            job.state = JobState::RollingBack;
            job.schema_state = SchemaState::DeleteOnly;
            job.args_rewritten = true;
            Ok(())
        }
    }
}

/// 加列回滚：保留 AddColumn 动作并将回滚参数改写为删列参数。
fn rollback_add_column(job: &mut RollbackJob) -> Result<(), RollbackError> {
    match job.schema_state {
        SchemaState::None => cancel(job),
        SchemaState::DeleteOnly
        | SchemaState::WriteOnly
        | SchemaState::WriteReorganization
        | SchemaState::DeleteReorganization
        | SchemaState::ReplicaOnly
        | SchemaState::Public => {
            job.state = JobState::RollingBack;
            job.schema_state = SchemaState::DeleteOnly;
            job.args_rewritten = true;
            Ok(())
        }
    }
}

/// 删列回滚：Public 时直接取消，开始删除后继续前滚。
fn rollback_drop_column(job: &mut RollbackJob) -> Result<(), RollbackError> {
    match job.schema_state {
        SchemaState::Public => cancel(job),
        SchemaState::None
        | SchemaState::DeleteOnly
        | SchemaState::WriteOnly
        | SchemaState::WriteReorganization
        | SchemaState::DeleteReorganization => {
            job.state = JobState::Running;
            Ok(())
        }
        SchemaState::ReplicaOnly => Err(RollbackError::InvalidState),
    }
}

/// 改列回滚：未开始时取消、重组阶段回滚、Public 时继续前滚。
fn rollback_modify_column(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if job.schema_state == SchemaState::None {
        cancel(job)
    } else if matches!(
        job.schema_state,
        SchemaState::DeleteOnly | SchemaState::WriteOnly | SchemaState::WriteReorganization
    ) {
        job.state = JobState::RollingBack;
        job.args_rewritten = true;
        Ok(())
    } else if job.schema_state == SchemaState::Public {
        job.state = JobState::Running;
        Ok(())
    } else {
        Err(RollbackError::InvalidState)
    }
}

/// 删索引回滚：Public 时取消，开始删除后继续前滚。
fn rollback_drop_index(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if job.schema_state == SchemaState::Public {
        return cancel(job);
    }
    if matches!(
        job.schema_state,
        SchemaState::WriteOnly | SchemaState::DeleteOnly
    ) {
        job.state = JobState::Running;
        return Ok(());
    }
    if matches!(
        job.schema_state,
        SchemaState::None | SchemaState::DeleteReorganization
    ) {
        job.state = JobState::Running;
        return Ok(());
    }
    Err(RollbackError::InvalidState)
}

/// 交换分区回滚：None 直接取消，否则恢复 Public 并完成回滚。
fn rollback_exchange_partition(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if job.schema_state == SchemaState::None {
        cancel(job)
    } else {
        job.state = JobState::RollbackDone;
        job.schema_state = SchemaState::Public;
        Ok(())
    }
}

/// 清空分区：Go 仅允许 Public/WriteOnly 阶段取消，其余阶段继续前滚。
fn convert_truncate_partition(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if matches!(
        job.schema_state,
        SchemaState::Public | SchemaState::WriteOnly
    ) {
        cancel(job)
    } else {
        job.state = JobState::Running;
        Ok(())
    }
}

/// 加分区回滚：无新增分区时取消，否则保留动作并填充回滚参数。
fn convert_add_partition(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if matches!(job.schema_state, SchemaState::None) {
        cancel(job)
    } else {
        job.state = JobState::RollingBack;
        job.args_rewritten = true;
        Ok(())
    }
}

/// 重组分区：未开始时取消，Public 时继续前滚，中间态进入 RollingBack。
fn convert_reorganize_partition(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if job.schema_state == SchemaState::None {
        cancel(job)
    } else if job.schema_state == SchemaState::Public {
        job.state = JobState::Running;
        Ok(())
    } else {
        job.state = JobState::RollingBack;
        job.args_rewritten = true;
        Ok(())
    }
}

/// 添加约束回滚：未写入元数据时取消，否则移除约束并进入回滚。
fn rollback_add_constraint(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if job.schema_state == SchemaState::None {
        cancel(job)
    } else {
        job.state = JobState::RollingBack;
        job.args_rewritten = true;
        Ok(())
    }
}

fn rollback_drop_or_alter_constraint(job: &mut RollbackJob) -> Result<(), RollbackError> {
    if job.schema_state == SchemaState::Public {
        cancel(job)
    } else {
        job.state = JobState::Running;
        Ok(())
    }
}

/// 按 Go 动作各自的初始状态取消；动作已推进时恢复 Running 继续前滚。
fn cancel_only_unhandled(job: &mut RollbackJob) -> Result<(), RollbackError> {
    let initial_state = match job.action {
        JobAction::DropTable
        | JobAction::DropView
        | JobAction::DropSchema
        | JobAction::DropPartition
        | JobAction::RenameIndex => SchemaState::Public,
        _ => SchemaState::None,
    };
    if job.schema_state == initial_state {
        cancel(job)
    } else {
        job.state = JobState::Running;
        Ok(())
    }
}

/// 将作业标记为 Cancelled，并以 `Cancelled` 错误返回（对齐 Go 取消语义）。
fn cancel(job: &mut RollbackJob) -> Result<(), RollbackError> {
    job.state = JobState::Cancelled;
    Err(RollbackError::Cancelled)
}

/// 列可空性：用于回滚索引时把相关列改回可空。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnNullability {
    pub column_id: i64,
    pub not_null: bool,
    pub prevent_null_insert: bool,
}

/// 按索引列 ID 设置 `not_null` 并清除临时的 `prevent_null_insert` 标志。
pub fn update_columns_null_to_not_null(
    columns: &mut [ColumnNullability],
    index_column_ids: &[i64],
) -> Result<(), RollbackError> {
    for id in index_column_ids {
        let column = columns
            .iter_mut()
            .find(|column| column.column_id == *id)
            .ok_or(RollbackError::InvalidState)?;
        column.not_null = true;
        column.prevent_null_insert = false;
    }
    Ok(())
}

/// 记录一次取消转回滚时的转换错误；超过全局上限后按 Go 文案取消作业。
pub fn record_rollback_conversion_error(job: &mut RollbackJob, limit: usize) {
    job.error_count += 1;
    if job.error_count > limit {
        job.error = Some(format!(
            "[ddl:-1]rollback DDL job error count exceed the limit {limit}, cancelled it now"
        ));
        job.state = JobState::Cancelled;
    }
}
