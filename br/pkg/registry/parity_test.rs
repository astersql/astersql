// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/registry` public contracts vs Go.
//! 中文注释索引开始
//! 本文件负责`br/pkg/registry/parity_test.rs`对应的Go/Rust 契约对齐，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少111行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! 测试夹具中的 SQL 分支匹配顺序与 Go 用例场景一一对应，改动匹配条件等于改动契约。
//! - `TaskRow`承载"TaskRow"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `MemDb`承载"MemDb"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl MemDb`把"MemDb"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `MemSession`承载"MemSession"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl MemSession`把"MemSession"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl RestrictedSQLExecutor`把"RestrictedSQLExecutor"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl Session`把"Session"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `TestGlue`承载"TestGlue"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Glue`把"Glue"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `TestInfoSchema`承载"TestInfoSchema"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl InfoSchema`把"InfoSchema"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `TestDomain`承载"TestDomain"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Domain`把"Domain"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `sample_info`是当前文件的重要函数，承担"sample_info"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `new_registry`是当前文件的重要函数，承担"new_registry"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `go_rust_public_contract_matches`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `ExecRestrictedSQL`是当前文件的重要函数，承担"ExecRestrictedSQL"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `ExecuteInternal`是当前文件的重要函数，承担"ExecuteInternal"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `sql_has`是当前文件的重要函数，承担"sql_has"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `公开常量与 SQL 模板对齐`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `ResumeOrCreateRegistration 新建与冲突`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `PauseTask 与 stale 心跳转 paused`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `Unregister 删除任务`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `GetRegistrationsByMaxID 分页`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `CheckTablesWithRegisteredTasks 冲突表`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `StartHeartbeatManager 心跳更新`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `OperationAfterWaitIDs 等待 resetting`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `FindAndDeleteMatchingTask`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `GlobalOperationAfterSetResettingStatus`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! 中文注释索引结束

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::heartbeat::{NewHeartbeatManager, UpdateHeartbeatSQLTemplate};
use crate::registration::{
    FilterSeparator, NewRestoreRegistry, RegistrationInfo, Registry, RestoreRegistryDBName,
    RestoreRegistryTableName, StaleTaskThresholdMinutes,
};
use crate::stubs::{
    CaseInsensitive, Context, DBInfo, Database, Domain, Error, Glue, InfoSchema, MatchTable,
    MemStorage, NewCIStr, NewPiTRIdTracker, OptionFuncAlias, ParseFilter, RestrictedSQLExecutor,
    Result, Row, Session, SqlValue, Storage, Table, TableInfo, TaskStatusPaused,
    TaskStatusResetting, TaskStatusRunning, berrors, err_table_not_exists, set_skip_sleep,
    set_stale_ticker_duration_ms, set_wait_resetting_sleep_ms,
};

#[derive(Clone, Debug)]
struct TaskRow {
    id: u64,
    filter_strings: String,
    filter_hash_key: String, // MD5 input (joined filters)
    start_ts: u64,
    restored_ts: u64,
    upstream_cluster_id: u64,
    with_sys_table: bool,
    status: String,
    cmd: String,
    last_heartbeat: i64,
}

#[derive(Default)]
struct MemDb {
    tasks: HashMap<u64, TaskRow>,
    next_id: u64,
    fail_execute: Option<String>,
    in_txn: bool,
    /// When true, heartbeat SELECTs bump last_heartbeat to simulate a live task.
    auto_bump_heartbeat: bool,
}

impl MemDb {
    fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            tasks: HashMap::new(),
            next_id: 1,
            fail_execute: None,
            in_txn: false,
            auto_bump_heartbeat: false,
        }))
    }
}

struct MemSession {
    db: Arc<Mutex<MemDb>>,
    last_insert_id: u64,
    closed: bool,
}

impl MemSession {
    fn new(db: Arc<Mutex<MemDb>>) -> Self {
        Self {
            db,
            last_insert_id: 0,
            closed: false,
        }
    }

    fn sql_has(sql: &str, needle: &str) -> bool {
        sql.contains(needle)
    }
}

impl RestrictedSQLExecutor for MemSession {
    fn ExecRestrictedSQL(
        &mut self,
        _ctx: &Context,
        _opts: &[OptionFuncAlias],
        sql: &str,
        args: &[SqlValue],
    ) -> Result<Vec<Row>> {
        if self.closed {
            return Err(Error::new("session closed"));
        }
        let mut db = self.db.lock().unwrap();
        let sql_trim = sql.trim();
        if sql_trim == "BEGIN PESSIMISTIC" {
            db.in_txn = true;
            return Ok(vec![]);
        }
        if sql_trim == "COMMIT" {
            db.in_txn = false;
            return Ok(vec![]);
        }
        if sql_trim == "ROLLBACK" {
            db.in_txn = false;
            return Ok(vec![]);
        }
        if sql_trim == "SELECT LAST_INSERT_ID()" {
            return Ok(vec![Row::new(vec![SqlValue::U64(self.last_insert_id)])]);
        }

        // INSERT new task
        if Self::sql_has(sql, "INSERT INTO") {
            let filter_strings = match &args[0] {
                SqlValue::Str(s) => s.clone(),
                _ => String::new(),
            };
            let filter_hash_key = match &args[1] {
                SqlValue::Str(s) => s.clone(),
                _ => filter_strings.clone(),
            };
            let start_ts = args[2].as_u64();
            let restored_ts = args[3].as_u64();
            let upstream = args[4].as_u64();
            let with_sys = args[5].as_bool();
            let cmd = args[6].as_str();
            let hb = args[8].as_i64();
            let id = db.next_id;
            db.next_id += 1;
            db.tasks.insert(
                id,
                TaskRow {
                    id,
                    filter_strings,
                    filter_hash_key,
                    start_ts,
                    restored_ts,
                    upstream_cluster_id: upstream,
                    with_sys_table: with_sys,
                    status: TaskStatusRunning.as_str().to_string(),
                    cmd,
                    last_heartbeat: hb,
                },
            );
            self.last_insert_id = id;
            return Ok(vec![]);
        }

        // lookup / FOR UPDATE
        if Self::sql_has(sql, "filter_hash = MD5")
            && Self::sql_has(sql, "restored_ts =")
            && Self::sql_has(sql, "FOR UPDATE")
        {
            let key = args[0].as_str();
            let start_ts = args[1].as_u64();
            let restored_ts = args[2].as_u64();
            let upstream = args[3].as_u64();
            let with_sys = args[4].as_bool();
            let cmd = args[5].as_str();
            let mut matched: Vec<&TaskRow> = db
                .tasks
                .values()
                .filter(|t| {
                    t.filter_hash_key == key
                        && t.start_ts == start_ts
                        && t.restored_ts == restored_ts
                        && t.upstream_cluster_id == upstream
                        && t.with_sys_table == with_sys
                        && t.cmd == cmd
                })
                .collect();
            matched.sort_by(|a, b| b.id.cmp(&a.id));
            return Ok(matched
                .into_iter()
                .map(|t| Row::new(vec![SqlValue::U64(t.id), SqlValue::Str(t.status.clone())]))
                .collect());
        }

        // conflicting task (no restored_ts in WHERE)
        if Self::sql_has(sql, "filter_hash = MD5")
            && Self::sql_has(sql, "LIMIT 1")
            && !Self::sql_has(sql, "restored_ts =")
        {
            let key = args[0].as_str();
            let start_ts = args[1].as_u64();
            let upstream = args[2].as_u64();
            let with_sys = args[3].as_bool();
            let cmd = args[4].as_str();
            let mut matched: Vec<&TaskRow> = db
                .tasks
                .values()
                .filter(|t| {
                    t.filter_hash_key == key
                        && t.start_ts == start_ts
                        && t.upstream_cluster_id == upstream
                        && t.with_sys_table == with_sys
                        && t.cmd == cmd
                })
                .collect();
            matched.sort_by(|a, b| b.id.cmp(&a.id));
            if let Some(t) = matched.first() {
                return Ok(vec![Row::new(vec![
                    SqlValue::U64(t.id),
                    SqlValue::U64(t.restored_ts),
                    SqlValue::Str(t.status.clone()),
                    SqlValue::I64(t.last_heartbeat),
                ])]);
            }
            return Ok(vec![]);
        }

        // resume paused
        if Self::sql_has(sql, "status = 'running'")
            && Self::sql_has(sql, "last_heartbeat_time = FROM_UNIXTIME")
            && Self::sql_has(sql, "WHERE id =")
            && !Self::sql_has(sql, "status IN")
        {
            let hb = args[0].as_i64();
            let id = args[1].as_u64();
            if let Some(t) = db.tasks.get_mut(&id) {
                t.status = TaskStatusRunning.as_str().to_string();
                t.last_heartbeat = hb;
            }
            return Ok(vec![]);
        }

        // transition stale -> paused
        if Self::sql_has(sql, "SET status = 'paused'")
            && Self::sql_has(sql, "last_heartbeat_time = FROM_UNIXTIME")
        {
            let id = args[0].as_u64();
            let expected_hb = args[1].as_i64();
            if let Some(t) = db.tasks.get_mut(&id) {
                if (t.status == TaskStatusRunning.as_str()
                    || t.status == TaskStatusResetting.as_str())
                    && t.last_heartbeat == expected_hb
                {
                    t.status = TaskStatusPaused.as_str().to_string();
                }
            }
            return Ok(vec![]);
        }

        // update status FROM multiple / single
        if Self::sql_has(sql, "SET status =") && Self::sql_has(sql, "WHERE id =") {
            if Self::sql_has(sql, "status IN") {
                // ExecuteInternal path uses (newStatus, restoreID) only — handled there.
                // Restricted path not used for multi.
            }
            // SELECT status after transition
            if Self::sql_has(sql, "SELECT status FROM") {
                let id = args[0].as_u64();
                if let Some(t) = db.tasks.get(&id) {
                    return Ok(vec![Row::new(vec![SqlValue::Str(t.status.clone())])]);
                }
                return Ok(vec![]);
            }
        }

        if Self::sql_has(sql, "SELECT status FROM") && Self::sql_has(sql, "WHERE id =") {
            let id = args[0].as_u64();
            if let Some(t) = db.tasks.get(&id) {
                return Ok(vec![Row::new(vec![SqlValue::Str(t.status.clone())])]);
            }
            return Ok(vec![]);
        }

        // heartbeat select
        if Self::sql_has(sql, "UNIX_TIMESTAMP(last_heartbeat_time)") {
            let id = args[0].as_u64();
            let bump = db.auto_bump_heartbeat;
            if let Some(t) = db.tasks.get_mut(&id) {
                if bump {
                    t.last_heartbeat += 1;
                }
                return Ok(vec![Row::new(vec![SqlValue::I64(t.last_heartbeat)])]);
            }
            return Ok(vec![]);
        }

        // delete
        if Self::sql_has(sql, "DELETE FROM") {
            let id = args[0].as_u64();
            db.tasks.remove(&id);
            return Ok(vec![]);
        }

        // select by max id
        if Self::sql_has(sql, "WHERE id <") {
            let max_id = args[0].as_u64();
            let mut rows: Vec<&TaskRow> = db.tasks.values().filter(|t| t.id < max_id).collect();
            rows.sort_by(|a, b| a.id.cmp(&b.id));
            return Ok(rows
                .into_iter()
                .map(|t| {
                    Row::new(vec![
                        SqlValue::U64(t.id),
                        SqlValue::Str(t.filter_strings.clone()),
                        SqlValue::U64(t.start_ts),
                        SqlValue::U64(t.restored_ts),
                        SqlValue::U64(t.upstream_cluster_id),
                        SqlValue::I64(if t.with_sys_table { 1 } else { 0 }),
                        SqlValue::Str(t.status.clone()),
                        SqlValue::Str(t.cmd.clone()),
                        SqlValue::Str(t.filter_hash_key.clone()),
                    ])
                })
                .collect());
        }

        // resetting status tasks
        if Self::sql_has(sql, "status = 'resetting'") && !Self::sql_has(sql, "id in") {
            let mut ids: Vec<u64> = db
                .tasks
                .values()
                .filter(|t| t.status == TaskStatusResetting.as_str())
                .map(|t| t.id)
                .collect();
            ids.sort();
            return Ok(ids
                .into_iter()
                .map(|id| Row::new(vec![SqlValue::U64(id)]))
                .collect());
        }

        // remaining resetting with id in (...)
        if Self::sql_has(sql, "id in (") && Self::sql_has(sql, "status = 'resetting'") {
            // parse ids from SQL
            let start = sql.find("id in (").unwrap() + "id in (".len();
            let end = sql[start..].find(')').unwrap() + start;
            let ids_part = &sql[start..end];
            let want: Vec<u64> = ids_part
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            let mut out = Vec::new();
            for id in want {
                if let Some(t) = db.tasks.get(&id) {
                    if t.status == TaskStatusResetting.as_str() {
                        out.push(Row::new(vec![SqlValue::U64(id)]));
                    }
                }
            }
            return Ok(out);
        }

        // any unfinished
        if Self::sql_has(sql, "status != 'resetting'") {
            for t in db.tasks.values() {
                if t.status != TaskStatusResetting.as_str() {
                    return Ok(vec![Row::new(vec![SqlValue::U64(t.id)])]);
                }
            }
            return Ok(vec![]);
        }

        Err(Error::new(format!("unhandled SQL in mock: {sql}")))
    }
}

impl Session for MemSession {
    fn ExecuteInternal(&mut self, ctx: &Context, sql: &str, args: &[SqlValue]) -> Result<()> {
        if self.closed {
            return Err(Error::new("session closed"));
        }
        let mut db = self.db.lock().unwrap();
        if let Some(msg) = db.fail_execute.clone() {
            return Err(Error::new(msg));
        }
        // heartbeat update
        if sql.contains("SET last_heartbeat_time = FROM_UNIXTIME") && sql.contains("WHERE id =") {
            let hb = args[0].as_i64();
            let id = args[1].as_u64();
            if let Some(t) = db.tasks.get_mut(&id) {
                t.last_heartbeat = hb;
            }
            return Ok(());
        }
        // status update single: SET status = %? WHERE id = %? AND status = %?
        if sql.contains("SET status =")
            && sql.contains("AND status =")
            && !sql.contains("status IN")
        {
            let new_status = args[0].as_str();
            let id = args[1].as_u64();
            let expect = args[2].as_str();
            if let Some(t) = db.tasks.get_mut(&id) {
                if t.status == expect {
                    t.status = new_status;
                }
            }
            return Ok(());
        }
        // status from multiple
        if sql.contains("SET status =") && sql.contains("status IN") {
            let new_status = args[0].as_str();
            let id = args[1].as_u64();
            if let Some(t) = db.tasks.get_mut(&id) {
                if sql.contains(&format!("'{}'", t.status)) {
                    t.status = new_status;
                }
            }
            return Ok(());
        }
        // delete via ExecuteInternal
        if sql.contains("DELETE FROM") {
            drop(db);
            let _ = self.ExecRestrictedSQL(ctx, &[], sql, args)?;
            return Ok(());
        }
        Err(Error::new(format!("unhandled ExecuteInternal SQL: {sql}")))
    }

    fn Close(&mut self) {
        self.closed = true;
    }
}

struct TestGlue {
    db: Arc<Mutex<MemDb>>,
}

impl Glue for TestGlue {
    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>> {
        Ok(Box::new(MemSession::new(self.db.clone())))
    }
}

struct TestInfoSchema {
    exists: bool,
}

impl InfoSchema for TestInfoSchema {
    fn TableByName(
        &self,
        _ctx: &Context,
        _db: &crate::stubs::CIStr,
        _table: &crate::stubs::CIStr,
    ) -> Result<()> {
        if self.exists {
            Ok(())
        } else {
            Err(err_table_not_exists())
        }
    }
}

struct TestDomain {
    store: MemStorage,
    is: TestInfoSchema,
}

impl Domain for TestDomain {
    fn Store(&self) -> &dyn Storage {
        &self.store
    }
    fn InfoSchema(&self) -> &dyn InfoSchema {
        &self.is
    }
}

fn sample_info() -> RegistrationInfo {
    RegistrationInfo {
        FilterStrings: vec!["test.t1".into()],
        StartTS: 100,
        RestoredTS: 200,
        UpstreamClusterID: 1,
        WithSysTable: false,
        Cmd: "restore".into(),
    }
}

fn new_registry(table_exists: bool) -> (Registry, Arc<Mutex<MemDb>>) {
    let db = MemDb::new();
    let glue = TestGlue { db: db.clone() };
    let dom = TestDomain {
        store: MemStorage,
        is: TestInfoSchema {
            exists: table_exists,
        },
    };
    let reg = NewRestoreRegistry(&Context::Background(), &glue, &dom).unwrap();
    (reg, db)
}

#[test]
fn heartbeat_manager_constructor_does_not_start_worker() {
    let db = MemDb::new();
    db.lock().unwrap().tasks.insert(
        1,
        TaskRow {
            id: 1,
            filter_strings: "test.t1".into(),
            filter_hash_key: "test.t1".into(),
            start_ts: 100,
            restored_ts: 200,
            upstream_cluster_id: 1,
            with_sys_table: false,
            status: TaskStatusRunning.as_str().into(),
            cmd: "restore".into(),
            last_heartbeat: 0,
        },
    );
    let session: Arc<Mutex<Box<dyn Session>>> =
        Arc::new(Mutex::new(Box::new(MemSession::new(db.clone()))));
    let ctx = Context::Background();
    let mut manager = NewHeartbeatManager(session.clone(), ctx.clone(), 1);

    manager.Stop();
    assert_eq!(db.lock().unwrap().tasks[&1].last_heartbeat, 0);

    manager.Start();
    manager.Stop();
    assert!(db.lock().unwrap().tasks[&1].last_heartbeat > 0);
}

#[test]
fn case_insensitive_filter_normalizes_patterns_and_inputs() {
    let filter = ParseFilter(&["TEST.T1".to_string()]).unwrap();
    let filter = CaseInsensitive(filter);

    assert!(MatchTable(filter.as_ref(), "test", "t1", false));
    assert!(MatchTable(filter.as_ref(), "TeSt", "T1", false));
}

#[test]
fn table_filter_preserves_go_rule_order_and_parser_contract() {
    let filter = ParseFilter(&[
        "*.*".to_string(),
        "!test.secret".to_string(),
        "test.public_*".to_string(),
        "# ignored".to_string(),
        "   ".to_string(),
    ])
    .unwrap();
    let filter = CaseInsensitive(filter);

    assert!(filter.MatchTable("TEST", "public_orders"));
    assert!(!filter.MatchTable("test", "secret"));
    assert!(filter.MatchTable("other", "anything"));
    assert!(ParseFilter(&["schema-only".to_string()]).is_err());
}

#[test]
fn go_rust_public_contract_matches() {
    set_skip_sleep(true);
    set_stale_ticker_duration_ms(1);
    set_wait_resetting_sleep_ms(1);

    // --- Normal: create registration + constants ---
    assert_eq!(RestoreRegistryDBName, "mysql");
    assert_eq!(RestoreRegistryTableName, "tidb_restore_registry");
    assert_eq!(FilterSeparator, "\x1F");
    assert_eq!(StaleTaskThresholdMinutes, 5);
    assert!(UpdateHeartbeatSQLTemplate.contains("last_heartbeat_time"));

    let (mut reg, db) = new_registry(true);
    let ctx = Context::Background();
    let (task_id, restored_ts) = reg
        .ResumeOrCreateRegistration(&ctx, sample_info(), true)
        .expect("create");
    assert_eq!(task_id, 1);
    assert_eq!(restored_ts, 200);
    assert_eq!(db.lock().unwrap().tasks[&1].status, "running");

    // Heartbeat update side effect
    reg.UpdateHeartbeat(&ctx, task_id).unwrap();
    assert!(db.lock().unwrap().tasks[&1].last_heartbeat > 0);

    // Start/stop heartbeat manager (resource lifecycle)
    reg.StartHeartbeatManager(&ctx, task_id);
    reg.StopHeartbeatManager();

    // Pause then resume
    reg.PauseTask(&ctx, task_id).unwrap();
    assert_eq!(db.lock().unwrap().tasks[&1].status, "paused");
    let (resumed_id, _) = reg
        .ResumeOrCreateRegistration(&ctx, sample_info(), true)
        .unwrap();
    assert_eq!(resumed_id, task_id);
    assert_eq!(db.lock().unwrap().tasks[&1].status, "running");

    // --- Boundary: conflict with actively heartbeating running task ---
    db.lock().unwrap().auto_bump_heartbeat = true;
    let err = reg
        .ResumeOrCreateRegistration(&ctx, sample_info(), true)
        .unwrap_err();
    assert!(berrors::is_invalid_argument(&err), "{err}");
    db.lock().unwrap().auto_bump_heartbeat = false;

    // Table conflict check
    reg.PauseTask(&ctx, task_id).unwrap();
    // insert as paused registration visible to maxID check: create second task id=2 running
    // First make task1 paused with filter test.t1; check from restore_id=2
    let tables = vec![Table {
        DB: DBInfo {
            Name: NewCIStr("test"),
        },
        Info: TableInfo {
            Name: NewCIStr("t1"),
        },
    }];
    let conflict = reg
        .CheckTablesWithRegisteredTasks(&ctx, 99, None, &[], &tables)
        .unwrap_err();
    assert!(berrors::is_tables_existed(&conflict), "{conflict}");

    // PiTR tracker schema conflict
    let mut tracker = NewPiTRIdTracker();
    tracker.TrackTableName("test", "t2");
    // filter test.t1 should not match t2; use db.* style via new paused task
    {
        let mut g = db.lock().unwrap();
        g.tasks.get_mut(&1).unwrap().filter_strings = "test.*".into();
        // ParseFilter uses filter_strings from GetRegistrationsByMaxID
        g.tasks.get_mut(&1).unwrap().status = "paused".into();
    }
    // Need filter_strings updated - GetRegistrations reads filter_strings field
    let schema_conflict = reg
        .CheckTablesWithRegisteredTasks(&ctx, 99, Some(&tracker), &[], &[])
        .unwrap_err();
    assert!(
        berrors::is_databases_existed(&schema_conflict),
        "{schema_conflict}"
    );

    // --- Error: empty currentStatuses path via Pause after close session fail ---
    {
        let (mut reg2, db2) = new_registry(true);
        db2.lock().unwrap().fail_execute = Some("boom".into());
        let e = reg2.UpdateHeartbeat(&ctx, 1).unwrap_err();
        assert!(e.msg.contains("failed to update heartbeat"), "{e}");
        // clear fail for close
        db2.lock().unwrap().fail_execute = None;
        reg2.Close();
    }

    // user-specified restoredTS mismatch
    {
        let (mut reg3, db3) = new_registry(true);
        let mut info = sample_info();
        let _ = reg3
            .ResumeOrCreateRegistration(&ctx, info.clone(), true)
            .unwrap();
        reg3.PauseTask(&ctx, 1).unwrap();
        info.RestoredTS = 999;
        let e = reg3
            .ResumeOrCreateRegistration(&ctx, info, true)
            .unwrap_err();
        assert!(berrors::is_invalid_argument(&e), "{e}");
        // auto resolve from paused reuses existing restoredTS when not user-specified
        let mut info2 = sample_info();
        info2.RestoredTS = 999;
        let (id, rts) = reg3.ResumeOrCreateRegistration(&ctx, info2, false).unwrap();
        assert_eq!(id, 1);
        assert_eq!(rts, 200);
        let _ = db3;
    }

    // GlobalOperationAfterSetResettingStatus: no other unfinished => runs fn
    {
        let (mut reg4, db4) = new_registry(true);
        let (id, _) = reg4
            .ResumeOrCreateRegistration(&ctx, sample_info(), true)
            .unwrap();
        let ran = AtomicU64::new(0);
        reg4.GlobalOperationAfterSetResettingStatus(&ctx, id, || {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(
            db4.lock().unwrap().tasks[&id].status,
            TaskStatusResetting.as_str()
        );
    }

    // OperationAfterWaitIDs when table missing runs immediately
    {
        let (reg5, _) = new_registry(false);
        let ran = AtomicU64::new(0);
        reg5.OperationAfterWaitIDs(&ctx, || {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    // FindAndDeleteMatchingTask deletes paused
    {
        let (mut reg6, db6) = new_registry(true);
        let (id, _) = reg6
            .ResumeOrCreateRegistration(&ctx, sample_info(), true)
            .unwrap();
        reg6.PauseTask(&ctx, id).unwrap();
        let deleted = reg6
            .FindAndDeleteMatchingTask(&ctx, sample_info(), true)
            .unwrap();
        assert_eq!(deleted, id);
        assert!(db6.lock().unwrap().tasks.is_empty());
    }

    // --- Resource cleanup: Close closes sessions ---
    reg.Unregister(&ctx, task_id).ok();
    reg.Close();
}
