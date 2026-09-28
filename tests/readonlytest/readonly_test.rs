// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 本文件对应 `tests/readonlytest/readonly_test.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use astersql_kv as kv;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Database, NewTestKit, TestSession};

// `READ_ONLY_ERR_MSG` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
const READ_ONLY_ERR_MSG: &str = "Error 1836: Running in read-only mode";
// `CONFLICT_ERR_MSG` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
const CONFLICT_ERR_MSG: &str =
    "Error 1105: can't turn off tidb_super_read_only when tidb_restricted_read_only is on";
// `PRIVILEGED_ERR_MSG` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
const PRIVILEGED_ERR_MSG: &str = "Error 1227: Access denied; you need (at least one of) the SUPER or SYSTEM_VARIABLES_ADMIN privilege(s) for this operation";
// `TIDB_RESTRICTED_READ_ONLY` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
const TIDB_RESTRICTED_READ_ONLY: &str = "tidb_restricted_read_only";
// `TIDB_SUPER_READ_ONLY` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
const TIDB_SUPER_READ_ONLY: &str = "tidb_super_read_only";

// `NEXT_INSERT_VALUE` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
// 中文注释强调它为什么需要稳定。
static NEXT_INSERT_VALUE: AtomicI64 = AtomicI64::new(10_000);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// `Role` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
enum Role {
    Root,
    User,
    ReplicaWriter,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
// `SqlError` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct SqlError(String);

// 这里实现 `SqlError` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl SqlError {
    // `new` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

// 这里实现 `fmt::Display` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl fmt::Display for SqlError {
    // `fmt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
// `StatementRecord` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct StatementRecord {
    role: Role,
    sql: String,
    succeeded: bool,
}

#[derive(Debug, Default)]
// `ClusterState` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct ClusterState {
    restricted_read_only: bool,
    super_read_only: bool,
    table_t: Option<BTreeMap<i64, i64>>,
    stats_top_n: Vec<(i64, i64, i64, String, i64)>,
    statements: Vec<StatementRecord>,
    open_handles: usize,
    open_rows: usize,
    cleanup_order: Vec<Role>,
    view_stopped: bool,
}

#[derive(Debug, Default)]
// `Cluster` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Cluster {
    state: Mutex<ClusterState>,
}

// 这里实现 `Cluster` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Cluster {
    // `execute` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn execute(&self, role: Role, sql: &str) -> Result<(), SqlError> {
        let mut state = self.state.lock().expect("cluster mutex poisoned");
        let result = execute_sql(&mut state, role, sql);
        state.statements.push(StatementRecord {
            role,
            sql: sql.to_owned(),
            succeeded: result.is_ok(),
        });
        result
    }

    // `query_variable` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn query_variable(&self, sql: &str) -> Result<(String, String), SqlError> {
        let normalized = normalize_sql(sql);
        let variable = normalized
            .strip_prefix("show variables like '")
            .and_then(|value| value.strip_suffix('\''))
            .ok_or_else(|| SqlError::new(format!("unsupported query: {sql}")))?;
        let state = self.state.lock().expect("cluster mutex poisoned");
        let on = match variable {
            TIDB_RESTRICTED_READ_ONLY => state.restricted_read_only,
            TIDB_SUPER_READ_ONLY => state.super_read_only,
            _ => return Err(SqlError::new(format!("unknown variable: {variable}"))),
        };
        Ok((
            variable.to_owned(),
            if on { "ON" } else { "OFF" }.to_owned(),
        ))
    }

    // `register_handle` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    // 这里的行为需要尽量贴近 Go 版本。
    fn register_handle(&self) {
        self.state
            .lock()
            .expect("cluster mutex poisoned")
            .open_handles += 1;
    }

    // `close_handle` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn close_handle(&self, role: Role) {
        let mut state = self.state.lock().expect("cluster mutex poisoned");
        assert!(state.open_handles > 0, "closing an unregistered SQL handle");
        state.open_handles -= 1;
        state.cleanup_order.push(role);
    }

    // `open_rows` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    // 这里的行为需要尽量贴近 Go 版本。
    fn open_rows(&self) {
        self.state.lock().expect("cluster mutex poisoned").open_rows += 1;
    }

    // `close_rows` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn close_rows(&self) {
        let mut state = self.state.lock().expect("cluster mutex poisoned");
        assert!(state.open_rows > 0, "closing an unregistered row set");
        state.open_rows -= 1;
    }
}

// `normalize_sql` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

// `parse_switch` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn parse_switch(value: &str) -> Result<bool, SqlError> {
    match value
        .trim()
        .trim_end_matches(';')
        .to_ascii_lowercase()
        .as_str()
    {
        "1" | "on" => Ok(true),
        "0" | "off" | "default" => Ok(false),
        other => Err(SqlError::new(format!(
            "invalid boolean system variable value: {other}"
        ))),
    }
}

// `parse_global_assignment` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn parse_global_assignment(sql: &str) -> Option<(&str, &str)> {
    let assignment = sql.strip_prefix("set global ")?;
    let (variable, value) = assignment.split_once('=')?;
    Some((variable.trim(), value.trim()))
}

// `write_is_forbidden` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn write_is_forbidden(state: &ClusterState, role: Role) -> bool {
    (state.restricted_read_only || state.super_read_only)
        && !matches!(role, Role::ReplicaWriter | Role::Internal)
}

// `parse_insert_values` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn parse_insert_values(sql: &str) -> Result<Vec<String>, SqlError> {
    let (_, values) = sql
        .split_once("values")
        .ok_or_else(|| SqlError::new(format!("missing VALUES clause: {sql}")))?;
    let values = values
        .trim()
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| SqlError::new(format!("invalid VALUES clause: {sql}")))?;
    Ok(values
        .split(',')
        .map(|value| value.trim().trim_matches('\'').to_owned())
        .collect())
}

// `execute_sql` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn execute_sql(state: &mut ClusterState, role: Role, sql: &str) -> Result<(), SqlError> {
    let normalized = normalize_sql(sql);

    if let Some((variable, value)) = parse_global_assignment(&normalized) {
        if role != Role::Root && role != Role::Internal {
            return Err(SqlError::new(PRIVILEGED_ERR_MSG));
        }
        let on = parse_switch(value)?;
        return match variable {
            TIDB_RESTRICTED_READ_ONLY => {
                state.restricted_read_only = on;
                if on {
                    state.super_read_only = true;
                }
                Ok(())
            }
            TIDB_SUPER_READ_ONLY => {
                if !on && state.restricted_read_only {
                    Err(SqlError::new(CONFLICT_ERR_MSG))
                } else {
                    state.super_read_only = on;
                    Ok(())
                }
            }
            _ => Err(SqlError::new(format!(
                "unknown global variable: {variable}"
            ))),
        };
    }

    if normalized.starts_with("drop user if exists ")
        || normalized.starts_with("create user ")
        || normalized.starts_with("grant all privileges ")
        || normalized.starts_with("grant restricted_replica_writer_admin ")
    {
        return if role == Role::Root {
            Ok(())
        } else {
            Err(SqlError::new(PRIVILEGED_ERR_MSG))
        };
    }

    if normalized == "admin show ddl jobs" || normalized == "admin show slow recent 1" {
        return Ok(());
    }

    if normalized.starts_with("flashback cluster ") {
        return if write_is_forbidden(state, role) {
            Err(SqlError::new(READ_ONLY_ERR_MSG))
        } else {
            Ok(())
        };
    }

    if normalized == "drop table if exists t" {
        if write_is_forbidden(state, role) {
            return Err(SqlError::new(READ_ONLY_ERR_MSG));
        }
        state.table_t = None;
        return Ok(());
    }

    if normalized.starts_with("create table t") {
        if write_is_forbidden(state, role) {
            return Err(SqlError::new(READ_ONLY_ERR_MSG));
        }
        if state.table_t.is_some() {
            return Err(SqlError::new("Error 1050: Table 't' already exists"));
        }
        state.table_t = Some(BTreeMap::new());
        return Ok(());
    }

    if normalized.starts_with("insert into mysql.stats_top_n ") {
        if role != Role::Internal {
            return Err(SqlError::new(READ_ONLY_ERR_MSG));
        }
        let values = parse_insert_values(&normalized)?;
        if values.len() != 5 {
            return Err(SqlError::new("stats_top_n insert requires five values"));
        }
        state.stats_top_n.push((
            values[0]
                .parse()
                .map_err(|_| SqlError::new("invalid table_id"))?,
            values[1]
                .parse()
                .map_err(|_| SqlError::new("invalid is_index"))?,
            values[2]
                .parse()
                .map_err(|_| SqlError::new("invalid hist_id"))?,
            values[3].clone(),
            values[4]
                .parse()
                .map_err(|_| SqlError::new("invalid count"))?,
        ));
        return Ok(());
    }

    if normalized.starts_with("insert into t values ") {
        if write_is_forbidden(state, role) {
            return Err(SqlError::new(READ_ONLY_ERR_MSG));
        }
        let values = parse_insert_values(&normalized)?;
        if !(1..=2).contains(&values.len()) {
            return Err(SqlError::new("table t insert requires one or two values"));
        }
        let key = values[0]
            .parse::<i64>()
            .map_err(|_| SqlError::new("invalid integer key"))?;
        let value = values
            .get(1)
            .map(|value| value.parse::<i64>())
            .transpose()
            .map_err(|_| SqlError::new("invalid integer value"))?
            .unwrap_or_default();
        let table = state
            .table_t
            .as_mut()
            .ok_or_else(|| SqlError::new("Error 1146: Table 'test.t' doesn't exist"))?;
        if table.insert(key, value).is_some() {
            return Err(SqlError::new(
                "Error 1062: Duplicate entry for key 'PRIMARY'",
            ));
        }
        return Ok(());
    }

    if normalized == "update t set b = 2 where a = 1" {
        if write_is_forbidden(state, role) {
            return Err(SqlError::new(READ_ONLY_ERR_MSG));
        }
        let table = state
            .table_t
            .as_mut()
            .ok_or_else(|| SqlError::new("Error 1146: Table 'test.t' doesn't exist"))?;
        if let Some(value) = table.get_mut(&1) {
            *value = 2;
        }
        return Ok(());
    }

    Err(SqlError::new(format!("unsupported SQL statement: {sql}")))
}

// `Db` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Db {
    cluster: Arc<Cluster>,
    role: Role,
    closed: AtomicBool,
}

// 这里实现 `Db` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Db {
    // `open` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    // 这里的行为需要尽量贴近 Go 版本。
    fn open(cluster: Arc<Cluster>, role: Role) -> Self {
        cluster.register_handle();
        Self {
            cluster,
            role,
            closed: AtomicBool::new(false),
        }
    }

    // `exec` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn exec(&self, sql: &str) -> Result<(), SqlError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SqlError::new("database handle is closed"));
        }
        self.cluster.execute(self.role, sql)
    }

    // `query` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn query(&self, sql: &str) -> Result<Rows, SqlError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SqlError::new("database handle is closed"));
        }
        let row = self.cluster.query_variable(sql)?;
        self.cluster.open_rows();
        Ok(Rows {
            cluster: Arc::clone(&self.cluster),
            row: Some(row),
            closed: false,
        })
    }

    // `conn` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn conn(&self) -> Result<Conn, SqlError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SqlError::new("database handle is closed"));
        }
        self.cluster.register_handle();
        Ok(Conn {
            cluster: Arc::clone(&self.cluster),
            role: self.role,
            closed: false,
        })
    }

    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn close(&self) -> Result<(), SqlError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Err(SqlError::new("database handle already closed"));
        }
        self.cluster.close_handle(self.role);
        Ok(())
    }
}

// `Conn` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Conn {
    cluster: Arc<Cluster>,
    role: Role,
    closed: bool,
}

// 这里实现 `Conn` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Conn {
    // `exec_context` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn exec_context(&self, sql: &str) -> Result<(), SqlError> {
        if self.closed {
            return Err(SqlError::new("connection is closed"));
        }
        self.cluster.execute(self.role, sql)
    }

    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn close(&mut self) -> Result<(), SqlError> {
        if self.closed {
            return Err(SqlError::new("connection already closed"));
        }
        self.closed = true;
        self.cluster.close_handle(self.role);
        Ok(())
    }
}

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Drop for Conn {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn drop(&mut self) {
        if !self.closed {
            self.closed = true;
            self.cluster.close_handle(self.role);
        }
    }
}

// `Rows` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct Rows {
    cluster: Arc<Cluster>,
    row: Option<(String, String)>,
    closed: bool,
}

// 这里实现 `Rows` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Rows {
    // `next` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn next(&self) -> bool {
        self.row.is_some()
    }

    // `scan` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn scan(&mut self) -> Result<(String, String), SqlError> {
        self.row
            .take()
            .ok_or_else(|| SqlError::new("row set exhausted"))
    }

    // `close` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn close(&mut self) -> Result<(), SqlError> {
        if self.closed {
            return Err(SqlError::new("row set already closed"));
        }
        self.closed = true;
        self.cluster.close_rows();
        Ok(())
    }
}

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Drop for Rows {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn drop(&mut self) {
        if !self.closed {
            self.closed = true;
            self.cluster.close_rows();
        }
    }
}

// `ReadOnlySuite` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct ReadOnlySuite {
    cluster: Arc<Cluster>,
    db: Db,
    udb: Db,
    rdb: Db,
}

// `check_variable` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
fn check_variable(db: &Db, variable: &str, on: bool) {
    let mut rows = db
        .query(&format!("show variables like '{variable}'"))
        .expect("show variables must succeed");
    assert!(rows.next(), "show variables must return one row");
    let (name, status) = rows.scan().expect("show variables row must scan");
    assert_eq!(name, variable);
    assert_eq!(status, if on { "ON" } else { "OFF" });
    rows.close().expect("row set must close exactly once");
}

// `set_variable_no_error` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
fn set_variable_no_error(db: &Db, variable: &str, status: i32) {
    db.exec(&format!("set global {variable}={status}"))
        .expect("set global variable must succeed");
}

// `set_variable` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
// 和 Go 对齐时，状态恢复直接关系到可重复性。
fn set_variable(db: &Db, variable: &str, status: i32) -> Result<(), SqlError> {
    db.exec(&format!("set global {variable}={status}"))
}

// `create_read_only_suite` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
fn create_read_only_suite() -> ReadOnlySuite {
    let cluster = Arc::new(Cluster::default());
    let db = Db::open(Arc::clone(&cluster), Role::Root);
    set_variable_no_error(&db, TIDB_RESTRICTED_READ_ONLY, 0);
    set_variable_no_error(&db, TIDB_SUPER_READ_ONLY, 0);

    db.exec("drop user if exists 'u1'@'%'")
        .expect("drop u1 must succeed");
    db.exec("create user 'u1'@'%' identified by 'password'")
        .expect("create u1 must succeed");
    db.exec("grant all privileges on test.* to 'u1'@'%'")
        .expect("grant u1 must succeed");
    let udb = Db::open(Arc::clone(&cluster), Role::User);

    db.exec("drop user if exists 'r1'@'%'")
        .expect("drop r1 must succeed");
    db.exec("create user 'r1'@'%' identified by 'password'")
        .expect("create r1 must succeed");
    db.exec("grant all privileges on test.* to 'r1'@'%'")
        .expect("grant r1 must succeed");
    db.exec("grant RESTRICTED_REPLICA_WRITER_ADMIN on *.* to 'r1'@'%'")
        .expect("grant replica writer must succeed");
    let rdb = Db::open(Arc::clone(&cluster), Role::ReplicaWriter);

    ReadOnlySuite {
        cluster,
        db,
        udb,
        rdb,
    }
}

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Drop for ReadOnlySuite {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn drop(&mut self) {
        self.db.close().expect("root pool must close");
        self.rdb.close().expect("replica pool must close");
        self.udb.close().expect("user pool must close");
        self.cluster
            .state
            .lock()
            .expect("cluster mutex poisoned")
            .view_stopped = true;
    }
}

// `assert_error_message` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn assert_error_message(result: Result<(), SqlError>, expected: &str) {
    assert_eq!(
        result.expect_err("statement must fail").to_string(),
        expected
    );
}

// `assert_suite_cleanup` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn assert_suite_cleanup(cluster: &Cluster) {
    let state = cluster.state.lock().expect("cluster mutex poisoned");
    assert_eq!(state.open_handles, 0, "all pools/connections must close");
    assert_eq!(state.open_rows, 0, "all row sets must close");
    assert!(
        state.cleanup_order.len() >= 3,
        "suite cleanup must close all three database pools"
    );
    assert_eq!(
        &state.cleanup_order[state.cleanup_order.len() - 3..],
        &[Role::Root, Role::ReplicaWriter, Role::User],
        "suite cleanup must preserve Go's root/rdb/udb order"
    );
    assert!(
        state.cleanup_order[..state.cleanup_order.len() - 3]
            .iter()
            .all(|role| matches!(role, Role::User | Role::ReplicaWriter)),
        "only explicitly acquired user/replica connections may close before the pools"
    );
    assert!(state.view_stopped, "view worker cleanup must run last");
}

// TestRestriction maps Go TestRestriction, including exact errors and the
// restricted-read-only/super-read-only state transition.
// 测试 `test_restriction` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `test_restriction` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_restriction() {
    let cluster = {
        let suite = create_read_only_suite();
        suite
            .db
            .exec("drop table if exists t")
            .expect("drop table must succeed");
        suite
            .udb
            .exec("create table t (a int primary key, b int)")
            .expect("create table must succeed");
        suite
            .udb
            .exec("insert into t values (1, 1)")
            .expect("initial insert must succeed");
        suite
            .udb
            .exec("update t set b = 2 where a = 1")
            .expect("initial update must succeed");

        set_variable(&suite.db, TIDB_RESTRICTED_READ_ONLY, 1)
            .expect("restricted read only must turn on");
        thread::sleep(Duration::from_secs(1));

        check_variable(&suite.udb, TIDB_RESTRICTED_READ_ONLY, true);
        check_variable(&suite.udb, TIDB_SUPER_READ_ONLY, true);
        check_variable(&suite.rdb, TIDB_RESTRICTED_READ_ONLY, true);
        check_variable(&suite.rdb, TIDB_SUPER_READ_ONLY, true);

        assert_error_message(suite.udb.exec("create table t(a int)"), READ_ONLY_ERR_MSG);
        assert_error_message(
            suite.udb.exec("update t set b = 2 where a = 1"),
            READ_ONLY_ERR_MSG,
        );
        assert_error_message(
            suite.udb.exec("insert into t values (2, 3)"),
            READ_ONLY_ERR_MSG,
        );
        assert_error_message(
            set_variable(&suite.db, TIDB_SUPER_READ_ONLY, 0),
            CONFLICT_ERR_MSG,
        );
        assert_error_message(
            set_variable(&suite.udb, TIDB_SUPER_READ_ONLY, 0),
            PRIVILEGED_ERR_MSG,
        );
        assert_error_message(
            set_variable(&suite.rdb, TIDB_SUPER_READ_ONLY, 0),
            PRIVILEGED_ERR_MSG,
        );
        assert_error_message(
            suite.udb.exec("flashback cluster to timestamp ''"),
            READ_ONLY_ERR_MSG,
        );

        suite
            .udb
            .exec("admin show ddl jobs")
            .expect("read-only admin show ddl jobs must succeed");
        suite
            .udb
            .exec("admin show slow recent 1")
            .expect("read-only admin show slow must succeed");

        set_variable_no_error(&suite.db, TIDB_RESTRICTED_READ_ONLY, 0);
        check_variable(&suite.udb, TIDB_RESTRICTED_READ_ONLY, false);
        check_variable(&suite.rdb, TIDB_RESTRICTED_READ_ONLY, false);
        check_variable(&suite.udb, TIDB_SUPER_READ_ONLY, true);
        check_variable(&suite.rdb, TIDB_SUPER_READ_ONLY, true);

        set_variable_no_error(&suite.db, TIDB_SUPER_READ_ONLY, 0);
        check_variable(&suite.udb, TIDB_RESTRICTED_READ_ONLY, false);
        check_variable(&suite.rdb, TIDB_RESTRICTED_READ_ONLY, false);
        check_variable(&suite.udb, TIDB_SUPER_READ_ONLY, false);
        check_variable(&suite.rdb, TIDB_SUPER_READ_ONLY, false);

        let state = suite.cluster.state.lock().expect("cluster mutex poisoned");
        assert_eq!(
            state.table_t.as_ref().and_then(|table| table.get(&1)),
            Some(&2)
        );
        assert_eq!(state.open_rows, 0);
        drop(state);
        Arc::clone(&suite.cluster)
    };
    assert_suite_cleanup(&cluster);
}

// TestRestrictionWithConnectionPool maps Go's persistent sql.Conn loop. The
// worker must first write successfully, then observe the exact read-only error.
// 测试 `test_restriction_with_connection_pool` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `test_restriction_with_connection_pool` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_restriction_with_connection_pool() {
    let cluster = {
        let suite = create_read_only_suite();
        suite
            .db
            .exec("drop table if exists t")
            .expect("drop table must succeed");
        suite
            .db
            .exec("create table t (a int)")
            .expect("create table must succeed");

        let mut conn = suite.udb.conn().expect("acquire pooled connection");
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut started = false;
            loop {
                thread::sleep(Duration::from_millis(50));
                let value = NEXT_INSERT_VALUE.fetch_add(1, Ordering::Relaxed);
                match conn.exec_context(&format!("insert into t values ({value})")) {
                    Ok(()) => {
                        if !started {
                            started = true;
                            started_tx.send(()).expect("report first successful write");
                        }
                    }
                    Err(error) => {
                        done_tx
                            .send(error.to_string() == READ_ONLY_ERR_MSG)
                            .expect("report read-only observation");
                        conn.close().expect("pooled connection must close");
                        return;
                    }
                }
            }
        });

        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("connection loop must write before read-only mode");
        thread::sleep(Duration::from_secs(1));
        set_variable_no_error(&suite.db, TIDB_RESTRICTED_READ_ONLY, 1);
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("connection loop must observe read-only mode")
        );
        worker.join().expect("connection worker must not panic");

        let state = suite.cluster.state.lock().expect("cluster mutex poisoned");
        assert!(
            state.statements.iter().any(|record| {
                record.role == Role::User
                    && record.sql.starts_with("insert into t values")
                    && record.succeeded
            }),
            "pooled connection must perform successful SQL before the switch"
        );
        assert!(
            state.statements.iter().any(|record| {
                record.role == Role::User
                    && record.sql.starts_with("insert into t values")
                    && !record.succeeded
            }),
            "the same pooled connection must be rejected after the switch"
        );
        drop(state);
        Arc::clone(&suite.cluster)
    };
    assert_suite_cleanup(&cluster);
}

// TestReplicationWriter maps Go's privileged replication writer loop. It joins
// the worker so success during the three-second read-only window is observable.
// 测试 `test_replication_writer` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `test_replication_writer` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_replication_writer() {
    let cluster = {
        let suite = create_read_only_suite();
        suite
            .db
            .exec("set global tidb_restricted_read_only=0")
            .expect("reset restricted read only");
        suite
            .db
            .exec("drop table if exists t")
            .expect("drop table must succeed");
        suite
            .db
            .exec("create table t (a int)")
            .expect("create table must succeed");

        let mut conn = suite.rdb.conn().expect("acquire replica connection");
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut started = false;
            loop {
                if stop_rx.try_recv().is_ok() {
                    conn.close().expect("replica connection must close");
                    return;
                }
                thread::sleep(Duration::from_millis(50));
                let value = NEXT_INSERT_VALUE.fetch_add(1, Ordering::Relaxed);
                conn.exec_context(&format!("insert into t values ({value})"))
                    .expect("replica writer must remain writable");
                if !started {
                    started = true;
                    started_tx.send(()).expect("report first replica write");
                }
            }
        });

        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("replica writer must start");
        thread::sleep(Duration::from_secs(1));
        suite
            .db
            .exec("set global tidb_restricted_read_only=1")
            .expect("enable restricted read only");
        assert_error_message(suite.db.exec("insert into t values (1)"), READ_ONLY_ERR_MSG);

        let successful_before = suite
            .cluster
            .state
            .lock()
            .expect("cluster mutex poisoned")
            .statements
            .iter()
            .filter(|record| record.role == Role::ReplicaWriter && record.succeeded)
            .count();
        thread::sleep(Duration::from_secs(3));
        stop_tx.send(()).expect("stop replica writer");
        worker.join().expect("replica worker must not panic");
        let state = suite.cluster.state.lock().expect("cluster mutex poisoned");
        let successful_after = state
            .statements
            .iter()
            .filter(|record| record.role == Role::ReplicaWriter && record.succeeded)
            .count();
        assert!(
            successful_after > successful_before,
            "replica writer must continue SQL writes while restricted read-only is on"
        );
        drop(state);
        Arc::clone(&suite.cluster)
    };
    assert_suite_cleanup(&cluster);
}

// `ReadOnlyVariableReset` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
struct ReadOnlyVariableReset {
    session: TestSession,
    errors: Arc<Mutex<Vec<String>>>,
}

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl Drop for ReadOnlyVariableReset {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn drop(&mut self) {
        for sql in [
            "set global tidb_restricted_read_only=default",
            "set global tidb_super_read_only=default",
        ] {
            if let Err(error) = self.session.database().execute(sql, &[]) {
                self.errors
                    .lock()
                    .expect("cleanup error mutex poisoned")
                    .push(format!("{sql}: {error}"));
            }
        }
    }
}

// TestInternalSQL maps Go TestInternalSQL: external writes are blocked by both
// switches, while the real TestKit session internal path can update stats.
// 测试 `test_internal_sql` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// 这里只补场景意图，保持失败信号不变。
#[test]
// `test_internal_sql` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
// 保持这层拆分可以让后续定位回归更直接。
fn test_internal_sql() {
    // `INSERT_STATS` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    // 中文注释强调它为什么需要稳定。
    const INSERT_STATS: &str = "insert into mysql.stats_top_n (table_id, is_index, hist_id, value, count) values (874, 0, 1, 'a', 3)";
    // `QUERY_STATS` 记录跨函数共享的固定约束、错误文本或全局状态。
    // 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
    // 中文注释强调它为什么需要稳定。
    const QUERY_STATS: &str = "select table_id, is_index, hist_id, value, count from mysql.stats_top_n where table_id=874";

    // The two-server network boundary is unavailable in the Rust test
    // environment. Keep the external rejection in the stateful SQL harness.
    let external_cluster = Arc::new(Cluster::default());
    let external_root = Db::open(Arc::clone(&external_cluster), Role::Root);
    let external_user = Db::open(Arc::clone(&external_cluster), Role::User);
    external_root
        .exec("set global tidb_restricted_read_only=On")
        .expect("restricted read only must turn on");
    external_root
        .exec("set global tidb_super_read_only=On")
        .expect("super read only must turn on");
    assert_error_message(external_user.exec(INSERT_STATS), READ_ONLY_ERR_MSG);
    external_root.close().expect("external root must close");
    external_user.close().expect("external user must close");

    // Go uses CreateMockStore + NewTestKit + Session.ExecuteInternal. The Rust
    // AnalyzeStatsStore creates a thread-pinned ConcreteSession, so these SQL
    // statements traverse the real parser/catalog/executor rather than a
    // pre-registered result table.
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut testkit = NewTestKit(store.clone());
    testkit.MustExec("set global tidb_restricted_read_only=On", Vec::new());
    testkit.MustExec("set global tidb_super_read_only=On", Vec::new());
    let cleanup_errors = Arc::new(Mutex::new(Vec::new()));
    let reset = ReadOnlyVariableReset {
        session: testkit.Session(),
        errors: Arc::clone(&cleanup_errors),
    };

    let context =
        kv::WithInternalSourceType(kv::Context::todo(), kv::InternalTxnStatsForegroundPriority);
    assert_eq!(
        kv::GetInternalSourceType(&context),
        kv::InternalTxnStatsForegroundPriority
    );
    let execution = testkit
        .Session()
        .ExecuteInternal(&context, INSERT_STATS, &[])
        .expect("internal stats SQL must bypass read-only mode");
    assert_eq!(execution.affected_rows, 1);
    testkit
        .MustQuery(QUERY_STATS, Vec::new())
        .Check(vec![vec!["874", "0", "1", "a", "3"]]);

    let missing_source = testkit
        .Session()
        .ExecuteInternal(&kv::Context::todo(), INSERT_STATS, &[])
        .expect_err("ordinary context must not enter ExecuteInternal");
    assert_eq!(
        missing_source.to_string(),
        "ExecuteInternal requires an internal request source"
    );

    drop(reset);
    assert!(
        cleanup_errors
            .lock()
            .expect("cleanup error mutex poisoned")
            .is_empty(),
        "read-only variable cleanup must succeed: {:?}",
        cleanup_errors.lock().expect("cleanup error mutex poisoned")
    );
    testkit
        .Session()
        .close()
        .expect("TestKit session must close");
    assert_eq!(store.active_session_count(), 0);
    store.close().expect("AnalyzeStatsStore must shut down");
}
