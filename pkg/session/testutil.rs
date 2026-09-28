// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// 会话测试工具（testutil）：为集成测试提供 mock store / domain / session 抽象。
//
// 安装可注入的 [`TestRuntime`] 后，用例可通过 [`CreateStoreAndBootstrap`]、
// [`CreateSessionAndSetID`]、[`MustExec`] 等辅助函数搭建环境并执行 SQL，
// 而无需依赖真实 TiKV。另提供 [`RevertVersionAndVariables`] 用于回退 bootstrap 版本相关系统变量。

#![allow(dead_code, non_snake_case)]

use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use crate::{SessionError, SessionResult};

/// 测试侧可读的 bootstrap 版本（OnceLock 注入）。
pub static GetBootstrapVersion: OnceLock<i64> = OnceLock::new();
/// 当前 bootstrap 版本号注入点。
pub static CurrentBootstrapVersion: OnceLock<i64> = OnceLock::new();
/// 测试用 TiDB DDL 表版本注入点。
pub static TiDBDDLTableVersionForTest: OnceLock<i64> = OnceLock::new();

/// 测试用 KV store 抽象（可向下转型为具体 mock 实现）。
pub trait TestStore: Any + Send + Sync {
    /// 返回 `Any` 以便测试代码做类型断言。
    fn as_any(&self) -> &dyn Any;
}
/// 测试用 Domain 抽象（schema / 统计等域服务容器）。
pub trait TestDomain: Any + Send + Sync {
    /// 返回 `Any` 以便测试代码做类型断言。
    fn as_any(&self) -> &dyn Any;
}

/// 测试用结果集：列名、逐行拉取与关闭。
pub trait TestRecordSet: Send {
    /// 结果集列名。
    fn Columns(&self) -> &[String];
    /// 拉取下一行；`None` 表示结束。
    fn Next(&mut self) -> SessionResult<Option<Vec<String>>>;
    /// 关闭结果集并释放资源。
    fn Close(&mut self) -> SessionResult;
}

// Like Go's session.Session, a concrete session is bound to one connection and
// is not safe for concurrent use. The runtime itself remains Send + Sync.
/// 测试会话：绑定单连接，非并发安全（与 Go `session.Session` 一致）。
pub trait TestSession {
    /// 设置连接 ID（connection ID）。
    fn SetConnectionID(&self, connection_id: u64);
    /// 执行 SQL，返回零或多个结果集。
    fn Execute(&self, sql: &str) -> SessionResult<Vec<Box<dyn TestRecordSet>>>;
    /// 预编译语句，返回 statement ID。
    fn PrepareStmt(&self, sql: &str) -> SessionResult<u64>;
    /// 执行已预编译语句（可带绑定参数）。
    fn ExecutePreparedStmt(
        &self,
        statement_id: u64,
        arguments: &[String],
    ) -> SessionResult<Option<Box<dyn TestRecordSet>>>;
}

/// Internal test-driver tags preserving binary-protocol parameter types after
/// the narrow `TestSession` ABI converts them to strings.
pub const TYPED_PREPARED_NUMERIC_PREFIX: &str = "\0astersql:numeric:";
pub const TYPED_PREPARED_NULL: &str = "\0astersql:null";

/// 可注入的测试运行时：创建 mock store、bootstrap 会话与表达式转换等。
pub trait TestRuntime: Send + Sync {
    /// 测试场景下调高 GOMAXPROCS 语义对应项。
    fn SetMaxProcsForTest(&self);
    /// 是否处于 next-gen 部署模式。
    fn IsNextGen(&self) -> bool;
    /// 按 next-gen 要求更新全局配置。
    fn UpdateConfigForNextgen(&self);
    /// 创建内存 mock store。
    fn NewMockStore(&self) -> SessionResult<Arc<dyn TestStore>>;
    /// 对 store 执行 bootstrap，返回 Domain。
    fn BootstrapSession(&self, store: Arc<dyn TestStore>) -> SessionResult<Arc<dyn TestDomain>>;
    /// 基于 store 创建测试会话。
    fn CreateSession4Test(&self, store: Arc<dyn TestStore>) -> SessionResult<Arc<dyn TestSession>>;
    /// 将字符串参数转为可绑定表达式文本。
    fn ArgsToExpressions(&self, arguments: &[String]) -> Vec<String>;
}

/// 全局测试运行时单例。
static TEST_RUNTIME: OnceLock<Arc<dyn TestRuntime>> = OnceLock::new();
/// 为 CreateSessionAndSetID 生成递增连接 ID。
static SESSION_KIT_ID_GENERATOR: AtomicU64 = AtomicU64::new(0);

/// 安装测试运行时；重复安装返回错误。
pub fn InstallTestRuntime(runtime: Arc<dyn TestRuntime>) -> SessionResult {
    TEST_RUNTIME
        .set(runtime)
        .map_err(|_| SessionError::new("test runtime is already installed"))
}

/// 取得已安装的测试运行时；未安装则 panic。
fn runtime() -> &'static dyn TestRuntime {
    TEST_RUNTIME
        .get()
        .expect("test runtime must be installed before using session test utilities")
        .as_ref()
}

/// 创建 mock store 并完成 bootstrap，返回 `(store, domain)`。
pub fn CreateStoreAndBootstrap() -> SessionResult<(Arc<dyn TestStore>, Arc<dyn TestDomain>)> {
    let runtime = runtime();
    runtime.SetMaxProcsForTest();
    // next-gen 需先改配置再开 store，避免与默认路径行为不一致。
    if runtime.IsNextGen() {
        runtime.UpdateConfigForNextgen();
    }
    let store = runtime.NewMockStore()?;
    let domain = runtime.BootstrapSession(Arc::clone(&store))?;
    Ok((store, domain))
}

/// 创建测试会话并分配唯一连接 ID。
pub fn CreateSessionAndSetID(store: Arc<dyn TestStore>) -> SessionResult<Arc<dyn TestSession>> {
    let session = runtime().CreateSession4Test(store)?;
    let connection_id = SESSION_KIT_ID_GENERATOR.fetch_add(1, Ordering::AcqRel) + 1;
    session.SetConnectionID(connection_id);
    Ok(session)
}

/// 执行 SQL（或预编译路径）并断言成功；有结果集则关闭。
pub fn MustExec(session: &dyn TestSession, sql: &str, arguments: &[String]) {
    let record_set = exec(session, sql, arguments).expect("test SQL failed");
    if let Some(mut record_set) = record_set {
        record_set.Close().expect("close record set");
    }
}

/// 执行 SQL 并要求返回结果集，失败或无结果集则 panic。
pub fn MustExecToRecodeSet(
    session: &dyn TestSession,
    sql: &str,
    arguments: &[String],
) -> Box<dyn TestRecordSet> {
    exec(session, sql, arguments)
        .expect("test SQL failed")
        .expect("statement did not return a record set")
}

/// 无参数走 `Execute`，有参数走 prepare + execute；取首个结果集。
fn exec(
    session: &dyn TestSession,
    sql: &str,
    arguments: &[String],
) -> SessionResult<Option<Box<dyn TestRecordSet>>> {
    if arguments.is_empty() {
        let mut record_sets = session.Execute(sql)?;
        return Ok(if record_sets.is_empty() {
            None
        } else {
            Some(record_sets.remove(0))
        });
    }
    let statement_id = session.PrepareStmt(sql)?;
    let parameters = runtime().ArgsToExpressions(arguments);
    session.ExecutePreparedStmt(statement_id, &parameters)
}

/// 将 `mysql.tidb` 中的 `tidb_server_version` 回退到指定版本；旧版本额外关闭 dist task。
pub fn RevertVersionAndVariables(session: &dyn TestSession, version: i64) {
    MustExec(
        session,
        &format!(
            "update mysql.tidb set variable_value='{version}' where variable_name='tidb_server_version'"
        ),
        &[],
    );
    // 版本 ≤195 时 dist task 开关在旧逻辑中应为 off。
    if version <= 195 {
        MustExec(
            session,
            "update mysql.global_variables set variable_value='off' where variable_name='tidb_enable_dist_task'",
            &[],
        );
    }
}
