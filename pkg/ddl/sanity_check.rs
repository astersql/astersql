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

// DDL 健全性检查（sanity check）工具。
//
// 提供两类校验：
// - DeleteRange（删除范围）计数：按 DDL 动作类型推算应写入的删除任务条数，
//   与实际条数比对，防止遗漏或重复注册待回收的 KV 键区间；
// - History job（历史作业）语句形态：检查写入 DDL 历史的 SQL 文本与语句类型
//   是否合法，避免非 DDL 或异常 CREATE 语句污染历史记录。

use std::collections::BTreeSet;

use crate::delete_range::{DeleteRangeAction, DeleteRangeJob};

/// 跨多次计数调用复用的去重上下文。
///
/// ModifyColumn 等动作可能对同一索引 ID 重复出现，用集合记录已计入的索引，
/// 保证期望 DeleteRange 条数不被重复累加。
#[derive(Clone, Debug, Default)]
pub struct DeleteRangeCountContext {
    /// 已参与计数的索引 ID 集合。
    index_ids: BTreeSet<i64>,
}

impl DeleteRangeCountContext {
    /// 统计 `index_ids` 中尚未出现过的元素个数，并将新 ID 记入集合。
    pub fn deduplicate_index_count(&mut self, index_ids: &[i64]) -> usize {
        index_ids
            .iter()
            .filter(|id| self.index_ids.insert(**id))
            .count()
    }
}

/// 根据 DeleteRange 作业动作与参数，计算期望登记的删除范围条数。
///
/// DeleteRange 是 DDL 完成后异步清理旧物理表、分区或索引 KV 数据的机制；
/// 不同动作（DropSchema、AddIndex、ModifyColumn 等）对应不同的条数公式。
pub fn expected_delete_range_count(
    context: &mut DeleteRangeCountContext,
    job: &DeleteRangeJob,
) -> usize {
    // 已取消的作业不应再产生删除范围任务。
    if job.cancelled {
        return 0;
    }
    match job.action {
        DeleteRangeAction::DropSchema => job.old_physical_table_ids.len(),
        // 删表/截断表：每个旧物理表一条，外加表本身对应的额外一条。
        DeleteRangeAction::DropTable | DeleteRangeAction::TruncateTable => {
            job.old_physical_table_ids.len() + 1
        }
        DeleteRangeAction::TruncatePartition => job.old_physical_table_ids.len(),
        // 分区重组类：旧物理表与旧全局索引都需要清理。
        DeleteRangeAction::DropPartition
        | DeleteRangeAction::ReorganizePartition
        | DeleteRangeAction::RemovePartitioning
        | DeleteRangeAction::AlterTablePartitioning => {
            job.old_physical_table_ids.len() + job.old_global_indexes.len()
        }
        // 加索引/主键：全局索引按 1 计，本地索引按分区数（至少 1）计；
        // 若已完成回滚则临时与正式两套索引各计一次（×2）。
        DeleteRangeAction::AddIndex | DeleteRangeAction::AddPrimaryKey => job
            .index_arguments
            .iter()
            .map(|index| {
                let physical_count = if index.global {
                    1
                } else {
                    job.partition_ids.len().max(1)
                };
                physical_count * if job.rollback_done { 2 } else { 1 }
            })
            .sum(),
        // 列存（columnar）索引不走 DeleteRange；普通索引按分区数计。
        DeleteRangeAction::DropIndex | DeleteRangeAction::DropPrimaryKey => {
            let index = job
                .index_arguments
                .first()
                .expect("finished drop-index job must contain an index argument");
            if index.columnar {
                0
            } else {
                job.partition_ids.len().max(1)
            }
        }
        DeleteRangeAction::DropColumn => job.partition_ids.len().max(1) * job.index_ids.len(),
        // 改列：索引 ID 需去重后再与分区数相乘。
        DeleteRangeAction::ModifyColumn => {
            job.partition_ids.len().max(1) * context.deduplicate_index_count(&job.index_ids)
        }
        // 多 schema 变更：对每个子作业递归求和。
        DeleteRangeAction::MultiSchemaChange => job
            .subjobs
            .iter()
            .map(|subjob| expected_delete_range_count(context, subjob))
            .sum(),
        DeleteRangeAction::Other => 0,
    }
}

/// 期望与实际 DeleteRange 条数不一致时的错误信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteRangeCountMismatch {
    /// 相关 DDL 作业 ID。
    pub job_id: i64,
    /// 按动作公式算出的期望条数。
    pub expected: usize,
    /// 实际登记条数。
    pub actual: usize,
}

/// 校验单个 DeleteRange 作业的实际条数是否等于期望值。
pub fn check_delete_range_count(
    job: &DeleteRangeJob,
    actual: usize,
) -> Result<(), DeleteRangeCountMismatch> {
    let expected = expected_delete_range_count(&mut DeleteRangeCountContext::default(), job);
    if expected == actual {
        Ok(())
    } else {
        Err(DeleteRangeCountMismatch {
            job_id: job.id,
            expected,
            actual,
        })
    }
}

/// DDL 历史记录中解析出的语句类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryStatementKind {
    /// CREATE PLACEMENT POLICY。
    CreatePlacementPolicy,
    /// CREATE TABLE。
    CreateTable,
    /// CREATE DATABASE/SCHEMA。
    CreateSchema,
    /// CREATE SEQUENCE（序列对象）。
    CreateSequence,
    /// CREATE VIEW。
    CreateView,
    /// 其它 DDL 语句。
    Ddl,
    /// 非 DDL 语句（如 DML），不应出现在 DDL 历史中。
    NonDdl,
}

/// 影响历史作业 SQL 形态校验的 DDL 动作类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryJobAction {
    UpdateTiFlashReplicaStatus,
    UnlockTable,
    CreatePlacementPolicy,
    CreateTable,
    CreateSchema,
    CreateTables,
    Other,
}

/// 历史作业健全性检查失败原因。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistorySanityError {
    /// SQL 查询文本为空。
    EmptyQuery(i64),
    /// 该内部动作要求查询文本为空。
    QueryMustBeEmpty(i64),
    /// 除批量建表外，历史作业必须恰好解析出一条语句。
    InvalidStatementCount(i64),
    /// 历史中混入了非 DDL 语句。
    NonDdlStatement(i64),
    /// 在不允许 CREATE 的动作上出现了建表/序列/视图语句。
    UnexpectedCreateStatement(i64),
    /// 语句类型与具体 CREATE 动作不匹配。
    UnexpectedStatementKind(i64),
}

/// 检查写入 DDL 历史的查询文本与语句类型是否符合动作约束。
pub fn check_history_job(
    job_id: i64,
    action: HistoryJobAction,
    query: &str,
    statements: &[HistoryStatementKind],
) -> Result<(), HistorySanityError> {
    if matches!(
        action,
        HistoryJobAction::UpdateTiFlashReplicaStatus | HistoryJobAction::UnlockTable
    ) {
        return if query.is_empty() {
            Ok(())
        } else {
            Err(HistorySanityError::QueryMustBeEmpty(job_id))
        };
    }
    if query == "skip" {
        return Ok(());
    }
    if query.trim().is_empty() {
        return Err(HistorySanityError::EmptyQuery(job_id));
    }
    if action != HistoryJobAction::CreateTables && statements.len() != 1 {
        return Err(HistorySanityError::InvalidStatementCount(job_id));
    }
    for statement in statements {
        let valid = match action {
            HistoryJobAction::CreatePlacementPolicy => {
                *statement == HistoryStatementKind::CreatePlacementPolicy
            }
            HistoryJobAction::CreateTable => *statement == HistoryStatementKind::CreateTable,
            HistoryJobAction::CreateSchema => *statement == HistoryStatementKind::CreateSchema,
            HistoryJobAction::CreateTables => matches!(
                statement,
                HistoryStatementKind::CreateTable
                    | HistoryStatementKind::CreateSequence
                    | HistoryStatementKind::CreateView
            ),
            HistoryJobAction::Other => *statement != HistoryStatementKind::NonDdl,
            HistoryJobAction::UpdateTiFlashReplicaStatus | HistoryJobAction::UnlockTable => {
                unreachable!("empty-query actions returned before statement validation")
            }
        };
        if !valid {
            if action == HistoryJobAction::Other {
                return Err(HistorySanityError::NonDdlStatement(job_id));
            }
            return Err(HistorySanityError::UnexpectedStatementKind(job_id));
        }
    }
    Ok(())
}
