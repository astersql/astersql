// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DDL 模式变更（schema change）订阅与分发模块。
//
// 当 DDL（数据定义语言，如 CREATE/DROP TABLE）作业完成并发布模式变更事件后，
// 本模块由 DDL owner（集群中负责执行 DDL 的节点）侧的后台线程周期性拉取事件，
// 调用已注册的处理函数，并用位图标记各处理器的完成状态。
//
// 全部处理器都处理完同一条变更后，才会从持久化存储中删除该记录，避免重复投递丢失。
// 处理过程使用悲观事务（pessimistic transaction：先加锁再提交，冲突时立即失败）
// 保证「调用 handler + 更新 processedByFlag」的原子性。

use crate::{Error, SchemaChange, SchemaChangeEvent, Session, SessionPool, Store};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// 模式变更处理回调：接收会话与事件，返回错误表示本次未能处理（可稍后重试）。
pub type SchemaChangeHandler =
    Box<dyn FnMut(Session, &SchemaChangeEvent) -> Result<(), Error> + Send + 'static>;

/// 构造「尚未就绪、请稍后重试」错误；订阅循环遇到此错误会跳过该 handler 本轮，但不记入致命错误。
pub fn ErrNotReadyRetryLater() -> Error {
    Error::NotReadyRetryLater
}

/// 处理器标识；取值须落在 0..64，以便用 `u64` 位图记录「已被哪些 handler 处理」。
pub type HandlerID = i32;
/// 测试用 handler ID。
pub const TestHandlerID: HandlerID = 0;
/// 统计元数据（stats meta）同步用 handler ID。
pub const StatsMetaHandlerID: HandlerID = 1;
/// 优先级队列相关 handler ID。
pub const PriorityQueueHandlerID: HandlerID = 2;

/// 将 HandlerID 转为可读名称，便于日志与错误信息。
pub fn HandlerIDString(id: HandlerID) -> String {
    match id {
        TestHandlerID => "TestHandler".to_owned(),
        StatsMetaHandlerID => "StatsMetaHandler".to_owned(),
        _ => format!("HandlerID({id})"),
    }
}

/// 每次轮询从存储读取的最大变更条数（可运行时调整，至少按 1 处理）。
pub static ProcessEventsBatchSize: AtomicUsize = AtomicUsize::new(1024);
/// 慢 handler 日志阈值（当前实现保留常量，与 Go 侧语义对齐）。
pub const slowHandlerLogThreshold: Duration = Duration::from_secs(5);

/// Notifier 的共享内部状态：会话池、事件存储、已注册 handler 与停止标志。
struct NotifierInner {
    session_pool: SessionPool,
    store: Arc<dyn Store>,
    handlers: Mutex<BTreeMap<HandlerID, SchemaChangeHandler>>,
    /// 已成为 owner 时固化的「已注册 handler」位图，用于判断变更是否可删除。
    handlers_bitmap: AtomicU64,
    poll_interval: Duration,
    stop: AtomicBool,
    errors: Mutex<Vec<String>>,
}

/// DDL 模式变更通知器：在成为 owner 后启动轮询线程，向订阅方投递事件。
pub struct DDLNotifier {
    inner: Arc<NotifierInner>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// Owner 生命周期监听：竞选成为 / 卸任 DDL owner 时回调。
pub trait OwnerListener {
    /// 本节点成为 DDL owner。
    fn OnBecomeOwner(&self);
    /// 本节点卸任 DDL owner。
    fn OnRetireOwner(&self);
}

/// 创建通知器；初始为停止态，需在 `OnBecomeOwner` 后才会启动后台轮询。
pub fn NewDDLNotifier(
    session_pool: SessionPool,
    store: Arc<dyn Store>,
    poll_interval: Duration,
) -> DDLNotifier {
    DDLNotifier {
        inner: Arc::new(NotifierInner {
            session_pool,
            store,
            handlers: Mutex::new(BTreeMap::new()),
            handlers_bitmap: AtomicU64::new(0),
            poll_interval,
            stop: AtomicBool::new(true),
            errors: Mutex::new(Vec::new()),
        }),
        worker: Mutex::new(None),
    }
}

impl DDLNotifier {
    /// 注册 handler；同一 ID 只允许注册一次，非法 ID（不在 0..64）会 panic。
    pub fn RegisterHandler(&self, id: HandlerID, handler: SchemaChangeHandler) {
        assert!((0..64).contains(&id), "illegal HandlerID: {id}");
        let mut handlers = self.inner.handlers.lock().expect("handlers mutex poisoned");
        if handlers.contains_key(&id) {
            return;
        }
        handlers.insert(id, handler);
    }

    /// 同步执行一轮事件处理（测试或手动触发时使用）。
    pub fn ProcessEvents(&self) -> Result<(), Error> {
        process_events(&self.inner)
    }

    /// 停止后台轮询并等待工作线程退出。
    pub fn Stop(&self) {
        self.inner.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().expect("worker mutex poisoned").take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }

    /// 返回累计的处理错误字符串（不含 NotReadyRetryLater）。
    pub fn Errors(&self) -> Vec<String> {
        self.inner
            .errors
            .lock()
            .expect("errors mutex poisoned")
            .clone()
    }

    /// 转发到 `OwnerListener::OnBecomeOwner`。
    pub fn OnBecomeOwner(&self) {
        <Self as OwnerListener>::OnBecomeOwner(self);
    }
    /// 转发到 `OwnerListener::OnRetireOwner`。
    pub fn OnRetireOwner(&self) {
        <Self as OwnerListener>::OnRetireOwner(self);
    }
}

impl OwnerListener for DDLNotifier {
    fn OnBecomeOwner(&self) {
        let mut worker = self.worker.lock().expect("worker mutex poisoned");
        if worker.is_some() {
            return;
        }
        // 固化当前已注册 handler 的位图；后续删除事件时要求 processedByFlag 与之相等。
        let bitmap = self
            .inner
            .handlers
            .lock()
            .expect("handlers mutex poisoned")
            .keys()
            .fold(0_u64, |bitmap, id| bitmap | (1_u64 << (*id as u32)));
        self.inner.handlers_bitmap.store(bitmap, Ordering::Release);
        self.inner.stop.store(false, Ordering::Release);
        let inner = self.inner.clone();
        *worker = Some(thread::spawn(move || {
            loop {
                thread::park_timeout(inner.poll_interval);
                if inner.stop.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = process_events(&inner) {
                    inner
                        .errors
                        .lock()
                        .expect("errors mutex poisoned")
                        .push(error.to_string());
                }
            }
        }));
    }
    fn OnRetireOwner(&self) {
        self.Stop();
    }
}

impl Drop for DDLNotifier {
    fn drop(&mut self) {
        self.Stop();
    }
}

/// 分页列出存储中的变更，逐条投递给各 handler；全部处理完后删除记录。
fn process_events(inner: &NotifierInner) -> Result<(), Error> {
    let list_session = inner.session_pool.Get();
    let (mut result, close) = inner.store.List(list_session.clone());
    let process_session = inner.session_pool.Get();
    let outcome = (|| {
        // 本轮已失败的 handler 跳过后续变更，避免同一 handler 反复阻塞整批处理。
        let mut skip_handlers = HashSet::new();
        let batch_size = ProcessEventsBatchSize.load(Ordering::Acquire);
        let mut changes = vec![None; batch_size];
        loop {
            let count = result.Read(&mut changes)?;
            if count == 0 {
                break;
            }
            for change in changes.iter_mut().take(count).filter_map(Option::as_mut) {
                let handler_ids: Vec<HandlerID> = inner
                    .handlers
                    .lock()
                    .expect("handlers mutex poisoned")
                    .keys()
                    .copied()
                    .collect();
                for handler_id in handler_ids {
                    if skip_handlers.contains(&handler_id) {
                        continue;
                    }
                    let result = {
                        let mut handlers = inner.handlers.lock().expect("handlers mutex poisoned");
                        process_event_for_handler(
                            inner,
                            &process_session,
                            change,
                            handler_id,
                            handlers
                                .get_mut(&handler_id)
                                .expect("registered handler missing"),
                        )
                    };
                    if let Err(error) = result {
                        skip_handlers.insert(handler_id);
                        if !error.is_not_ready() {
                            inner.errors.lock().expect("errors mutex poisoned").push(format!("Error processing change ddlJobID={} subJobID={} handler={}: {error}", change.ddlJobID, change.subJobID, HandlerIDString(handler_id)));
                        }
                    }
                }
                // Go 测试模式下无 handler 时保留事件，生产模式则删除。
                let bitmap = inner.handlers_bitmap.load(Ordering::Acquire);
                if change.processedByFlag == bitmap && !(cfg!(test) && bitmap == 0) {
                    let delete_session = inner.session_pool.Get();
                    if let Err(error) = inner.store.DeleteAndCommit(
                        &delete_session,
                        change.ddlJobID,
                        change.subJobID,
                    ) {
                        inner
                            .errors
                            .lock()
                            .expect("errors mutex poisoned")
                            .push(format!(
                                "Error deleting change ddlJobID={} subJobID={}: {error}",
                                change.ddlJobID, change.subJobID
                            ));
                    }
                    inner.session_pool.Put(delete_session);
                }
            }
        }
        Ok(())
    })();
    close();
    inner.session_pool.Put(list_session);
    inner.session_pool.Put(process_session);
    outcome
}

/// 若该 handler 尚未处理过本变更：在悲观事务中执行回调并 CAS 更新 processedByFlag。
fn process_event_for_handler(
    inner: &NotifierInner,
    session: &Session,
    change: &mut SchemaChange,
    handler_id: HandlerID,
    handler: &mut SchemaChangeHandler,
) -> Result<(), Error> {
    let bit = 1_u64 << (handler_id as u32);
    if change.processedByFlag & bit != 0 {
        return Ok(());
    }
    let new_flag = change.processedByFlag | bit;
    session.BeginPessimistic()?;
    if let Err(error) = handler(session.clone(), &change.event) {
        session.Rollback();
        return Err(error);
    }
    // UpdateProcessed 带期望旧值，防止多 owner 短暂并存时互相覆盖。
    if let Err(error) = inner.store.UpdateProcessed(
        session,
        change.ddlJobID,
        change.subJobID,
        change.processedByFlag,
        new_flag,
    ) {
        session.Rollback();
        return Err(error);
    }
    if let Err(error) = session.Commit() {
        session.Rollback();
        return Err(error);
    }
    change.processedByFlag = new_flag;
    Ok(())
}
