// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// http://www.apache.org/licenses/LICENSE-2.0
// Copyright 2013 The ql Authors. All rights reserved.

// 会话核心 TiDB 侧逻辑：Domain 映射、SQL 解析封装与语句收尾。
//
// - [`domainMap`]：按 store UUID 缓存 Domain，支持 etcd / schema filter 创建与关闭回调清理。
// - [`Parse`]：解析 SQL，并把警告回写到会话。
// - [`finishStmt`] / [`autoCommitAfterStmt`] / [`checkStmtLimit`]：语句结束后的
//   事务提交/回滚、自动提交（autocommit）、悲观死锁处理与语句数限制。
// - [`StmtHistory`] / [`GetRows4Test`]：可重试语句历史与测试用结果集抽取。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use crate::{SessionError, SessionResult};

/// store 选项键：标记该 store 是否已完成 bootstrap。
pub const StoreBootstrappedKey: &str = "bootstrap";
/// 自动提交前检查连接存活的最短语句耗时阈值。
pub const minConnectionAliveCheckBeforeCommitDuration: Duration = Duration::from_secs(1);
/// `SELECT ... FOR UPDATE` 不可重试时的错误文案。
pub const ErrForUpdateCantRetry: &str = "for update cannot retry";

/// 存储运行时：提供 UUID 与选项清理能力。
pub trait StorageRuntime: Send + Sync {
    /// 返回 store 唯一标识，用作 Domain 缓存键。
    fn UUID(&self) -> String;
    /// 清除指定 store 选项（如 bootstrap 标记）。
    fn ClearOption(&self, key: &str);
}

/// Domain 运行时：初始化、关闭与关闭回调。
pub trait DomainRuntime: Send + Sync {
    /// 初始化 Domain（加载 schema、启动后台任务等）。
    fn Init(&self) -> SessionResult;
    /// 关闭 Domain 并触发 OnClose。
    fn Close(&self);
    /// 注册关闭时从 domap 移除自身的回调。
    fn SetOnClose(&self, callback: Box<dyn Fn() + Send + Sync>);
}

/// Domain 工厂：创建带 etcd / schema filter 的 Domain。
pub trait DomainFactory: Send + Sync {
    fn NewDomainWithEtcdClient(
        &self,
        store: Arc<dyn StorageRuntime>,
        etcd_client: Option<String>,
        schema_filter: Option<String>,
        server_info_options: &[astersql_domain_serverinfo::SyncerOption],
    ) -> Arc<dyn DomainRuntime>;
    fn LogInitFailure(&self, store_uuid: &str, error: &SessionError);
}

/// 全局 Domain 表：按 store UUID 复用已初始化 Domain，失败可重试。
pub struct domainMap {
    domains: Arc<Mutex<HashMap<String, Arc<dyn DomainRuntime>>>>,
    factory: Arc<dyn DomainFactory>,
    max_retries: usize,
}

impl domainMap {
    /// 构造 Domain 映射；`max_retries` 至少为 1。
    pub fn new(factory: Arc<dyn DomainFactory>, max_retries: usize) -> Self {
        Self {
            domains: Arc::new(Mutex::new(HashMap::new())),
            factory,
            max_retries: max_retries.max(1),
        }
    }

    /// 按 store 获取 Domain；`store` 为 None 时返回任意已有 Domain。
    pub fn Get(
        &self,
        store: Option<Arc<dyn StorageRuntime>>,
    ) -> SessionResult<Arc<dyn DomainRuntime>> {
        self.getWithEtcdClient(store, None, None, &[])
    }

    /// 带 schema filter 获取或创建 Domain。
    pub fn GetOrCreateWithFilter(
        &self,
        store: Arc<dyn StorageRuntime>,
        filter: String,
    ) -> SessionResult<Arc<dyn DomainRuntime>> {
        self.getWithEtcdClient(Some(store), None, Some(filter), &[])
    }

    /// The temporary system-variable Domain never owns a serving status endpoint.
    pub fn getDomainForGlobalVarInit(
        &self,
        store: Arc<dyn StorageRuntime>,
    ) -> SessionResult<Arc<dyn DomainRuntime>> {
        self.getWithEtcdClient(
            Some(store),
            None,
            Some("systemDBFilter".into()),
            &[astersql_domain_serverinfo::SyncerOption::WithoutStatusEndpointClaim],
        )
    }

    /// 核心查找/创建逻辑：命中缓存则返回，否则工厂建 Domain 并 Init，失败重试。
    fn getWithEtcdClient(
        &self,
        store: Option<Arc<dyn StorageRuntime>>,
        etcd_client: Option<String>,
        schema_filter: Option<String>,
        server_info_options: &[astersql_domain_serverinfo::SyncerOption],
    ) -> SessionResult<Arc<dyn DomainRuntime>> {
        let mut domains = self.domains.lock().expect("domain map lock poisoned");
        let Some(store) = store else {
            return domains
                .values()
                .next()
                .cloned()
                .ok_or_else(|| SessionError::new("can not find available domain for a nil store"));
        };
        let key = store.UUID();
        if let Some(domain) = domains.get(&key) {
            return Ok(Arc::clone(domain));
        }

        // 最多 max_retries 次创建+Init；成功则注册 OnClose 以便从 map 摘除。
        let mut last_error = None;
        for _ in 0..self.max_retries {
            let domain = self.factory.NewDomainWithEtcdClient(
                Arc::clone(&store),
                etcd_client.clone(),
                schema_filter.clone(),
                server_info_options,
            );
            match domain.Init() {
                Ok(()) => {
                    let weak_domains: Weak<Mutex<HashMap<String, Arc<dyn DomainRuntime>>>> =
                        Arc::downgrade(&self.domains);
                    let close_key = key.clone();
                    domain.SetOnClose(Box::new(move || {
                        if let Some(domains) = weak_domains.upgrade() {
                            domains
                                .lock()
                                .expect("domain map lock poisoned")
                                .remove(&close_key);
                        }
                    }));
                    domains.insert(key, Arc::clone(&domain));
                    return Ok(domain);
                }
                Err(error) => {
                    domain.Close();
                    self.factory.LogInitFailure(&store.UUID(), &error);
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| SessionError::new("domain initialization failed")))
    }

    /// 按 store UUID 从映射中删除 Domain。
    pub fn Delete(&self, store: &dyn StorageRuntime) {
        self.domains
            .lock()
            .expect("domain map lock poisoned")
            .remove(&store.UUID());
    }
}

/// 进程级 Domain 映射单例。
static DOMAP: OnceLock<domainMap> = OnceLock::new();
/// 统计信息 lease 秒数；测试可置为 -1 禁用。
static STATS_LEASE_SECONDS: AtomicI64 = AtomicI64::new(0);

/// 安装全局 Domain 映射；重复安装报错。
pub fn InstallDomainMap(map: domainMap) -> SessionResult {
    DOMAP
        .set(map)
        .map_err(|_| SessionError::new("domain map is already installed"))
}

/// 测试辅助：从 domap 删除 store，并清除 bootstrap 选项。
pub fn ResetStoreForWithTiKVTest(store: Arc<dyn StorageRuntime>) {
    if let Some(map) = DOMAP.get() {
        map.Delete(store.as_ref());
    }
    store.ClearOption(StoreBootstrappedKey);
}

/// 测试辅助：将 stats lease 置为 -1，禁用统计租约。
pub fn DisableStats4Test() {
    STATS_LEASE_SECONDS.store(-1, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 语句种类，用于收尾路径区分 INSERT/UPDATE/DELETE/COMMIT 等。
pub enum StatementKind {
    Other,
    Insert,
    Update,
    Delete,
    Commit,
    LoadDataLocal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 解析后的语句：原文、种类与是否只读。
pub struct ParsedStatement {
    pub text: String,
    pub kind: StatementKind,
    pub read_only: bool,
}

/// 解析运行时：解析 SQL 并追加警告。
pub trait ParseRuntime {
    fn ParseSQL(&mut self, source: &str) -> SessionResult<(Vec<ParsedStatement>, Vec<String>)>;
    fn AppendWarning(&mut self, warning: String);

    /// 返回解析器在错误结果之外产生的警告。
    ///
    /// Go parser 的返回值可同时携带 warnings 与 error；默认实现保持现有
    /// Rust runtime 兼容，需要该语义的适配器应覆写此方法。
    fn TakeWarningsAfterError(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// 解析 SQL：成功则把警告写入 runtime，失败原样返回错误。
pub fn Parse(runtime: &mut dyn ParseRuntime, src: &str) -> SessionResult<Vec<ParsedStatement>> {
    let parsed = runtime.ParseSQL(src);
    match parsed {
        Ok((statements, warnings)) => {
            for warning in warnings {
                runtime.AppendWarning(warning);
            }
            Ok(statements)
        }
        Err(error) => {
            for warning in runtime.TakeWarningsAfterError() {
                runtime.AppendWarning(warning);
            }
            Err(error)
        }
    }
}

/// 已编译语句运行时视图：只读性与种类。
pub trait StatementRuntime: Send + Sync {
    fn IsReadOnly(&self) -> bool;
    fn Kind(&self) -> StatementKind;
}

/// 语句收尾所需的会话能力（事务状态、提交/回滚、历史与限制等）。
pub trait FinishSessionRuntime {
    fn InTxn(&self) -> bool;
    fn IsAutocommit(&self) -> bool;
    fn BatchCommit(&self) -> bool;
    fn CouldRetry(&self) -> bool;
    fn DisableRetry(&mut self);
    fn StatementStartTime(&self) -> Option<Instant>;
    fn CheckConnectionAlive(&mut self) -> SessionResult;
    fn TxnValid(&self) -> bool;
    fn TxnPending(&self) -> bool;
    fn TxnIsPessimistic(&self) -> bool;
    fn TxnRequestSourceInternal(&self) -> bool;
    fn StmtCommit(&mut self);
    fn StmtRollback(&mut self, pessimistic_retry: bool);
    fn CommitTxn(&mut self) -> SessionResult;
    fn RollbackTxn(&mut self);
    fn ChangeTxnToInvalid(&mut self);
    fn IsDeadlock(&self, error: &SessionError) -> bool;
    fn ObserveAbortTxn(&mut self, pessimistic: bool, internal: bool);
    fn History(&mut self) -> &mut StmtHistory;
    fn StatementCountLimit(&self) -> usize;
    fn NewTxn(&mut self) -> SessionResult;
    fn SetInTxn(&mut self, in_transaction: bool);
    fn PreviousStatement(&self) -> String;
}

/// 记录中止事务耗时指标（区分悲观/内部来源）。
fn recordAbortTxnDuration(session: &mut dyn FinishSessionRuntime, internal: bool) {
    session.ObserveAbortTxn(session.TxnIsPessimistic(), internal);
}

/// 语句执行收尾：可选连接存活检查、写入历史、StmtCommit/Rollback，再自动提交并检查语句上限。
pub fn finishStmt(
    session: &mut dyn FinishSessionRuntime,
    mut meets_error: Option<SessionError>,
    statement: Arc<dyn StatementRuntime>,
) -> SessionResult {
    // 非只读且可能重试：LOAD DATA LOCAL 禁用重试，其余写入 StmtHistory。
    let read_only = statement.IsReadOnly();
    if !read_only
        && meets_error.is_none()
        && shouldCheckConnectionAliveBeforeCommit(session, statement.as_ref())
    {
        meets_error = session.CheckConnectionAlive().err();
    }
    if !read_only {
        if meets_error.is_none() && session.CouldRetry() {
            if isLoadDataLocal(statement.as_ref()) {
                session.DisableRetry();
            } else {
                session.History().Add(Arc::clone(&statement));
            }
        }
        if session.TxnValid() {
            if meets_error.is_some() {
                session.StmtRollback(false);
            } else {
                session.StmtCommit();
            }
        }
    }
    let result = autoCommitAfterStmt(session, meets_error, statement.as_ref());
    if session.TxnPending() {
        session.ChangeTxnToInvalid();
    }
    result?;
    checkStmtLimit(session, true)
}

/// 是否为 `LOAD DATA LOCAL` 语句。
fn isLoadDataLocal(statement: &dyn StatementRuntime) -> bool {
    statement.Kind() == StatementKind::LoadDataLocal
}

/// 自动提交且语句已跑够时长的 DML，在提交前探测连接是否仍存活。
fn shouldCheckConnectionAliveBeforeCommit(
    session: &dyn FinishSessionRuntime,
    statement: &dyn StatementRuntime,
) -> bool {
    if !session.IsAutocommit() || session.InTxn() {
        return false;
    }
    if session
        .StatementStartTime()
        .is_some_and(|start| start.elapsed() < minConnectionAliveCheckBeforeCommitDuration)
    {
        return false;
    }
    matches!(
        statement.Kind(),
        StatementKind::Insert | StatementKind::Update | StatementKind::Delete
    )
}

/// 语句后的自动提交/回滚：无显式事务则 Commit；出错时按悲观死锁等规则 Rollback。
fn autoCommitAfterStmt(
    session: &mut dyn FinishSessionRuntime,
    meets_error: Option<SessionError>,
    statement: &dyn StatementRuntime,
) -> SessionResult {
    let internal = session.TxnRequestSourceInternal();
    if let Some(error) = meets_error {
        if !session.InTxn() {
            session.RollbackTxn();
            recordAbortTxnDuration(session, internal);
        } else if session.TxnValid() && session.TxnIsPessimistic() && session.IsDeadlock(&error) {
            session.RollbackTxn();
            recordAbortTxnDuration(session, internal);
        }
        return Err(error);
    }
    if !session.InTxn() {
        return session.CommitTxn().map_err(|error| {
            if statement.Kind() == StatementKind::Commit {
                SessionError::new(format!(
                    "{error}; previous statement: {}",
                    session.PreviousStatement()
                ))
            } else {
                error
            }
        });
    }
    Ok(())
}

/// 检查事务内语句数是否超限；超限且未开 batch commit 则回滚，否则在 finish 时开新事务。
pub fn checkStmtLimit(session: &mut dyn FinishSessionRuntime, is_finish: bool) -> SessionResult {
    let mut statement_count = session.History().Count();
    if !is_finish {
        statement_count += 1;
    }
    if statement_count <= session.StatementCountLimit() {
        return Ok(());
    }
    if !session.BatchCommit() {
        session.RollbackTxn();
        return Err(SessionError::new(format!(
            "statement count {statement_count} exceeds the transaction limitation, transaction has been rollback, autocommit = {}",
            session.IsAutocommit()
        )));
    }
    if !is_finish {
        return Ok(());
    }
    let result = session.NewTxn();
    session.SetInTxn(true);
    result
}

/// 惰性初始化并返回语句历史。
pub fn GetHistory(history: &mut Option<StmtHistory>) -> &mut StmtHistory {
    history.get_or_insert_with(StmtHistory::new)
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单元格值：NULL 或文本。
pub enum CellValue {
    Null,
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一行结果：由若干 [`CellValue`] 组成。
pub struct Row {
    pub cells: Vec<CellValue>,
}

/// 结果集运行时：按 chunk 拉取与关闭。
pub trait RecordSetRuntime {
    fn Next(&mut self, reused_chunk: &mut Vec<Row>) -> SessionResult;
    fn Close(&mut self) -> SessionResult;
}

/// 测试辅助：耗尽结果集，收集全部行。
pub fn GetRows4Test(record_set: Option<&mut dyn RecordSetRuntime>) -> SessionResult<Vec<Row>> {
    let Some(record_set) = record_set else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    let mut reused_chunk = Vec::new();
    loop {
        reused_chunk.clear();
        record_set.Next(&mut reused_chunk)?;
        if reused_chunk.is_empty() {
            break;
        }
        rows.extend(reused_chunk.iter().cloned());
    }
    Ok(rows)
}

/// 将结果集转为二维字符串表（NULL 显示为 `<nil>`），并关闭结果集。
pub fn ResultSetToStringSlice(
    record_set: &mut dyn RecordSetRuntime,
) -> SessionResult<Vec<Vec<String>>> {
    let rows = GetRows4Test(Some(record_set))?;
    record_set.Close()?;
    Ok(rows
        .into_iter()
        .map(|row| {
            row.cells
                .into_iter()
                .map(|cell| match cell {
                    CellValue::Null => "<nil>".to_owned(),
                    CellValue::Text(value) => value,
                })
                .collect()
        })
        .collect())
}

/// 可重试事务内已成功语句的历史，用于乐观重试回放。
pub struct StmtHistory {
    history: Vec<Arc<dyn StatementRuntime>>,
}

impl Default for StmtHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl StmtHistory {
    /// 创建空历史。
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    /// 追加一条可重试语句。
    pub fn Add(&mut self, statement: Arc<dyn StatementRuntime>) {
        self.history.push(statement);
    }

    /// 历史中的语句条数。
    pub fn Count(&self) -> usize {
        self.history.len()
    }
}
