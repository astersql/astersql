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

// DDL 测试公共工具：会话并发执行、schema 状态匹配与表模式辅助。
//
// SchemaState 表示对象在 DDL 状态机中的可见性阶段（None / Public 等）；
// TableMode 区分 Normal / Import 等表模式；CancelState 用于断言取消作业时
// 的 schema 状态（单作业或多子作业）。本模块通过 trait 抽象运行时，
// 便于在 mock 与真实 domain 间切换。

#![allow(dead_code, non_snake_case)]

use std::sync::mpsc::Sender;

/// DDL 对象 schema 状态：None（不可见）、Public（对用户可见）或其他整型编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaState {
    None,
    Public,
    Other(i32),
}

/// 表模式：Normal 普通读写；Import 导入模式（可放宽部分约束）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableMode {
    Normal,
    Import,
}

/// 索引元数据摘要（当前仅名称）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    pub name: String,
}

/// 表元数据摘要：id、名称、schema 状态、表模式与索引列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub state: SchemaState,
    pub mode: TableMode,
    pub indexes: Vec<IndexInfo>,
}

/// 库（database/schema）元数据摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DBInfo {
    pub id: i64,
    pub name: String,
}

/// 多 schema 变更中的子作业状态快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubJob {
    pub schema_state: SchemaState,
}

/// DDL 作业状态快照：当前 schema 状态、是否多 schema 变更及子作业列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub schema_state: SchemaState,
    pub multi_schema_change: bool,
    pub sub_jobs: Option<Vec<SubJob>>,
}

/// 子作业 schema 状态序列，用于匹配取消点。
pub type SubStates = Vec<SchemaState>;

/// 取消作业时期望匹配的 schema 状态：单作业或按子作业列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelState {
    Single(SchemaState),
    SubJobs(SubStates),
}

/// 刷新元数据（RefreshMeta）所需参数：库/表 id 与涉及名称。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshMetaArgs {
    pub schema_id: i64,
    pub table_id: i64,
    pub involved_db: String,
    pub involved_table: String,
}

/// 会话执行运行时：供协程辅助真正跑 SQL。
///
/// `execute` 返回是否产生结果集；DDL 辅助在产生结果集时拒绝继续。
/// Provides the real session lifecycle used by the goroutine helpers.
/// `execute` returns whether a record set was produced; DDL helpers reject it.
pub trait SessionExecRuntime: Clone + Send + 'static {
    type Error: Send + 'static;

    fn execute(&self, database: &str, sql: &str) -> Result<bool, Self::Error>;
    fn record_set_error(&self) -> Self::Error;
}

/// 在独立线程中执行单条 SQL，结果经 `done` 通道回传。
pub fn SessionExecInGoroutine<R: SessionExecRuntime>(
    runtime: R,
    database: String,
    sql: String,
    done: Sender<Result<(), R::Error>>,
) {
    ExecMultiSQLInGoroutine(runtime, database, vec![sql], done);
}

/// 在独立线程中顺序执行多条 SQL；任一条失败或通道关闭则停止。
pub fn ExecMultiSQLInGoroutine<R: SessionExecRuntime>(
    runtime: R,
    database: String,
    statements: Vec<String>,
    done: Sender<Result<(), R::Error>>,
) {
    std::thread::spawn(move || {
        for sql in statements {
            // 有结果集视为错误（DDL 不应返回 record set）；其余透传执行错误。
            let result = match runtime.execute(&database, &sql) {
                Ok(false) => Ok(()),
                Ok(true) => Err(runtime.record_set_error()),
                Err(error) => Err(error),
            };
            let failed = result.is_err();
            if done.send(result).is_err() || failed {
                return;
            }
        }
    });
}

/// DDL 测试运行时：查表句柄、索引、表信息，以及改表模式与刷新元数据。
pub trait DDLTestRuntime {
    type Error;

    fn extract_table_handles(
        &mut self,
        database: &str,
        table: &str,
    ) -> Result<Vec<i64>, Self::Error>;
    fn find_index(&mut self, database: &str, table: &str, index: &str) -> Option<IndexInfo>;
    fn table_info(&mut self, database_id: i64, table_id: i64) -> Result<TableInfo, Self::Error>;
    fn alter_table_mode(
        &mut self,
        database_id: i64,
        table_id: i64,
        mode: TableMode,
    ) -> Result<(), Self::Error>;
    fn refresh_meta(&mut self, args: RefreshMetaArgs) -> Result<(), Self::Error>;
}

/// 提取指定库表的全部物理句柄（含分区表各分区 id）。
pub fn ExtractAllTableHandles<R: DDLTestRuntime>(
    runtime: &mut R,
    database: &str,
    table: &str,
) -> Result<Vec<i64>, R::Error> {
    runtime.extract_table_handles(database, table)
}

/// 按名称查找索引元数据；不存在时返回 None。
pub fn FindIdxInfo<R: DDLTestRuntime>(
    runtime: &mut R,
    database: &str,
    table: &str,
    index: &str,
) -> Option<IndexInfo> {
    runtime.find_index(database, table, index)
}

/// 判断作业当前 schema 状态是否与期望的取消点一致。
pub fn MatchCancelState(job: &Job, cancel_state: &CancelState, _sql: &str) -> bool {
    match cancel_state {
        // 单作业：非 multi_schema_change 且 schema_state 相等。
        CancelState::Single(state) => !job.multi_schema_change && job.schema_state == *state,
        // 多子作业：子作业数量与各 schema_state 均对齐。
        CancelState::SubJobs(states) => job.sub_jobs.as_ref().is_some_and(|jobs| {
            jobs.len() == states.len()
                && jobs
                    .iter()
                    .zip(states)
                    .all(|(job, state)| job.schema_state == *state)
        }),
    }
}

/// 检查表是否达到期望 schema 状态；`None` 期望视为始终通过。
pub fn checkTableState<R: DDLTestRuntime>(
    runtime: &mut R,
    database: &DBInfo,
    table: &TableInfo,
    expected: SchemaState,
) -> Result<bool, R::Error> {
    let actual = runtime.table_info(database.id, table.id)?;
    Ok(expected == SchemaState::None || (actual.name == table.name && actual.state == expected))
}

/// 检查表模式是否与期望一致。
pub fn CheckTableMode<R: DDLTestRuntime>(
    runtime: &mut R,
    database: &DBInfo,
    table: &TableInfo,
    expected: TableMode,
) -> Result<bool, R::Error> {
    runtime
        .table_info(database.id, table.id)
        .map(|actual| actual.mode == expected)
}

/// 设置表模式并确认表已 Public 且模式匹配。
pub fn SetTableMode<R: DDLTestRuntime>(
    runtime: &mut R,
    database: &DBInfo,
    table: &TableInfo,
    mode: TableMode,
) -> Result<bool, R::Error> {
    runtime.alter_table_mode(database.id, table.id, mode)?;
    let public = checkTableState(runtime, database, table, SchemaState::Public)?;
    let mode_matches = CheckTableMode(runtime, database, table, mode)?;
    Ok(public && mode_matches)
}

/// 通过事务路径读取表信息（委托 `DDLTestRuntime::table_info`）。
pub fn GetTableInfoByTxn<R: DDLTestRuntime>(
    runtime: &mut R,
    database_id: i64,
    table_id: i64,
) -> Result<TableInfo, R::Error> {
    runtime.table_info(database_id, table_id)
}

/// 触发元数据刷新：按库/表 id 与名称构造 `RefreshMetaArgs`。
pub fn RefreshMeta<R: DDLTestRuntime>(
    runtime: &mut R,
    database_id: i64,
    table_id: i64,
    database_name: &str,
    table_name: &str,
) -> Result<(), R::Error> {
    runtime.refresh_meta(RefreshMetaArgs {
        schema_id: database_id,
        table_id,
        involved_db: database_name.to_owned(),
        involved_table: table_name.to_owned(),
    })
}
