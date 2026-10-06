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

// UPDATE 语句执行器。
//
// 将子计划产出的旧行与赋值表达式合成新行，写入存储并维护外键检查/级联。
// 通过 `UpdateRuntime` 注入会话、表元数据与存储访问；支持悲观事务下延迟重复键检查。

#![allow(non_snake_case)]

use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 更新时重复键（unique key）检查时机。
pub enum UpdateDupKeyCheckMode {
    /// 延迟到提交前统一检查（悲观/流水线事务常用）。
    Lazy,
    /// 写行时立即检查。
    InPlace,
}

/// UPDATE 运行时边界：行合成、写回、外键与生命周期钩子。
pub trait UpdateRuntime {
    type Context;
    type Request;
    type Row;
    type Schema;
    type ForeignKeyCheck;
    type ForeignKeyCascade;
    type Error;

    /// 为即将更新的行做预处理（锁、投影等）。
    fn prepare_row(&mut self, row: &Self::Row) -> Result<(), Self::Error>;
    /// 合并非生成列（用户赋值列）到新行。
    fn merge_non_generated(
        &mut self,
        old_row: &Self::Row,
        new_row: &mut Self::Row,
    ) -> Result<(), Self::Error>;
    /// 合并生成列（GENERATED COLUMN）：按表达式在求值前后写入。
    fn merge_generated(
        &mut self,
        old_row: &Self::Row,
        new_row: &mut Self::Row,
        table_index: usize,
        before_evaluation: bool,
    ) -> Result<(), Self::Error>;
    /// 将合成好的新旧行提交给存储层执行更新。
    fn execute_prepared_row(
        &mut self,
        context: &mut Self::Context,
        schema: &Self::Schema,
        row_index: usize,
        old_row: Self::Row,
        new_row: Self::Row,
        duplicate_key_check: UpdateDupKeyCheckMode,
    ) -> Result<(), Self::Error>;
    /// 清空输出请求块，准备本轮 Next。
    fn reset_request(&self, request: &mut Self::Request);
    /// 是否已耗尽：UPDATE 通常一次拉完所有待更新行后置位。
    fn drained(&self) -> bool;
    /// 标记执行器是否已完成全部更新。
    fn set_drained(&mut self, drained: bool);
    /// 从子执行器拉取并批量更新；返回首次匹配行数。
    fn update_rows(&mut self, context: &mut Self::Context) -> Result<usize, Self::Error>;
    /// 包装/记录更新失败（如 IGNORE、错误行号）。
    fn handle_update_error(&mut self, row_index: usize, error: Self::Error) -> Self::Error;
    /// 常量赋值快路径：直接合成新行，跳过表达式求值缓冲。
    fn fast_compose_new_row(
        &mut self,
        row_index: usize,
        old_row: &Self::Row,
    ) -> Result<Self::Row, Self::Error>;
    /// 通用路径：按赋值列表求值并合成新行。
    fn compose_new_row(
        &mut self,
        row_index: usize,
        old_row: &Self::Row,
    ) -> Result<Self::Row, Self::Error>;
    /// Start a fresh processed-write accounting lifecycle when runtime stats are enabled.
    fn reset_write_runtime_stats(&mut self);
    /// Charge rows matched for the first time, using runtime-owned target-table metadata.
    fn record_write_cpu_work(&mut self, matched_rows: usize);
    /// 设置客户端可见的 OK 报文信息（affected rows 等）。
    fn set_message(&mut self);
    /// 将本执行器运行时统计注册到会话。
    fn register_runtime_stats(&mut self);
    /// 是否开启运行时统计采集。
    fn collect_runtime_stats_enabled(&self) -> bool;
    /// 重置内存追踪计数。
    fn reset_memory_usage(&mut self);
    /// 关闭子执行器。
    fn close_child(&mut self) -> Result<(), Self::Error>;
    /// 打开子执行器（通常是选择待更新行的计划）。
    fn open_child(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>;
    /// 初始化表达式求值缓冲。
    fn initialize_evaluation_buffer(&mut self);
    /// 全部赋值为常量时可走 fast compose。
    fn all_assignments_are_constant(&self) -> bool;
    /// 外键约束检查器列表。
    fn foreign_key_checks(&self) -> Vec<&Self::ForeignKeyCheck>;
    /// 外键级联动作列表（ON UPDATE CASCADE 等）。
    fn foreign_key_cascades(&self) -> Vec<&Self::ForeignKeyCascade>;
}

/// UPDATE 执行器：持有注入的运行时并实现 Open/Next/Close 生命周期。
pub struct UpdateExec<R: UpdateRuntime> {
    pub runtime: R,
}

impl<R: UpdateRuntime> UpdateExec<R> {
    /// 预处理单行。
    pub fn prepare(&mut self, row: &R::Row) -> Result<(), R::Error> {
        self.runtime.prepare_row(row)
    }

    /// 合并非生成列。
    pub fn mergeNonGenerated(
        &mut self,
        row: &R::Row,
        new_data: &mut R::Row,
    ) -> Result<(), R::Error> {
        self.runtime.merge_non_generated(row, new_data)
    }

    /// 合并生成列。
    pub fn mergeGenerated(
        &mut self,
        row: &R::Row,
        new_data: &mut R::Row,
        table_index: usize,
        before_evaluation: bool,
    ) -> Result<(), R::Error> {
        self.runtime
            .merge_generated(row, new_data, table_index, before_evaluation)
    }

    /// 执行已准备行的写回。
    pub fn exec(
        &mut self,
        context: &mut R::Context,
        schema: &R::Schema,
        row_index: usize,
        row: R::Row,
        new_data: R::Row,
        duplicate_key_check: UpdateDupKeyCheckMode,
    ) -> Result<(), R::Error> {
        self.runtime.execute_prepared_row(
            context,
            schema,
            row_index,
            row,
            new_data,
            duplicate_key_check,
        )
    }

    /// Executor::Next：一次更新完所有匹配行后标记 drained。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Request,
    ) -> Result<(), R::Error> {
        self.runtime.reset_request(request);
        if self.runtime.drained() {
            return Ok(());
        }
        let matched_rows = self.runtime.update_rows(context)?;
        self.runtime.record_write_cpu_work(matched_rows);
        self.runtime.set_drained(true);
        Ok(())
    }

    /// 批量更新入口。
    pub fn updateRows(&mut self, context: &mut R::Context) -> Result<usize, R::Error> {
        self.runtime.update_rows(context)
    }

    /// 常量赋值快路径合成新行。
    pub fn fastComposeNewRow(
        &mut self,
        row_index: usize,
        old_row: &R::Row,
    ) -> Result<R::Row, R::Error> {
        self.runtime.fast_compose_new_row(row_index, old_row)
    }

    /// 通用路径合成新行。
    pub fn composeNewRow(
        &mut self,
        row_index: usize,
        old_row: &R::Row,
    ) -> Result<R::Row, R::Error> {
        self.runtime.compose_new_row(row_index, old_row)
    }

    /// 关闭：写 OK 信息、注册统计、关子执行器并重置内存。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        self.runtime.set_message();
        self.runtime.register_runtime_stats();
        let result = self.runtime.close_child();
        self.runtime.reset_memory_usage();
        result
    }

    /// 打开：复位 drained，必要时初始化求值缓冲，再打开子计划。
    pub fn Open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.runtime.reset_write_runtime_stats();
        self.runtime.set_drained(false);
        if !self.runtime.all_assignments_are_constant() {
            self.runtime.initialize_evaluation_buffer();
        }
        self.runtime.open_child(context)
    }

    /// 设置客户端消息。
    pub fn setMessage(&mut self) {
        self.runtime.set_message();
    }

    /// 是否采集运行时统计。
    pub fn collectRuntimeStatsEnabled(&self) -> bool {
        self.runtime.collect_runtime_stats_enabled()
    }

    /// 获取外键检查器。
    pub fn GetFKChecks(&self) -> Vec<&R::ForeignKeyCheck> {
        self.runtime.foreign_key_checks()
    }

    /// 获取外键级联动作。
    pub fn GetFKCascades(&self) -> Vec<&R::ForeignKeyCascade> {
        self.runtime.foreign_key_cascades()
    }

    /// 是否存在外键级联。
    pub fn HasFKCascades(&self) -> bool {
        !self.runtime.foreign_key_cascades().is_empty()
    }
}

/// 外连接未匹配行：行句柄（handle）为 NULL 时视为未命中内表。
pub fn unmatchedOuterRow(handle_is_null: bool) -> bool {
    handle_is_null
}

/// 将底层错误交给运行时附加行号等信息后返回。
pub fn handleUpdateError<R: UpdateRuntime>(
    runtime: &mut R,
    row_index: usize,
    error: R::Error,
) -> R::Error {
    runtime.handle_update_error(row_index, error)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// UPDATE 各阶段耗时：拉取、合成、检查并写回。
pub struct UpdateRuntimeStats {
    /// 从子计划拉取待更新行的耗时。
    pub fetch: Duration,
    /// 合成新行的耗时。
    pub compose: Duration,
    /// 约束检查与写存储的耗时。
    pub check_and_update: Duration,
}

impl UpdateRuntimeStats {
    /// 格式化为可读字符串（对标 Go String）。
    pub fn String(&self) -> String {
        format!(
            "fetch:{:?}, compose:{:?}, check-and-update:{:?}",
            self.fetch, self.compose, self.check_and_update
        )
    }

    /// 克隆统计快照。
    pub fn Clone(&self) -> Self {
        std::clone::Clone::clone(self)
    }

    /// 累加另一份统计（并行 worker 归并）。
    pub fn Merge(&mut self, other: &Self) {
        self.fetch += other.fetch;
        self.compose += other.compose;
        self.check_and_update += other.check_and_update;
    }

    /// 统计类型名，供运行时注册区分。
    pub fn Tp(&self) -> &'static str {
        "UpdateRuntimeStats"
    }
}

/// 按事务模式选择重复键检查策略：流水线优先用 Lazy；IGNORE 用 InPlace；
/// 其余悲观事务用 Lazy，乐观事务用 InPlace。
pub fn optimizeDupKeyCheckForUpdate(
    transaction_is_pessimistic: bool,
    transaction_is_pipelined: bool,
    ignore_needs_check_in_place: bool,
) -> UpdateDupKeyCheckMode {
    if transaction_is_pipelined {
        return UpdateDupKeyCheckMode::Lazy;
    }
    if ignore_needs_check_in_place {
        return UpdateDupKeyCheckMode::InPlace;
    }
    if transaction_is_pessimistic {
        return UpdateDupKeyCheckMode::Lazy;
    }
    UpdateDupKeyCheckMode::InPlace
}
