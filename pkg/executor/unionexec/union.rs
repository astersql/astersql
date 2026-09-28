// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Union 执行器（UnionExec）：并发合并多个子执行器的结果集。
//
// 对应 SQL `UNION`/`UNION ALL`。每个子计划（child）由独立 worker 拉取 Chunk
//（列式批处理块），经通道汇总给主线程；支持取消与错误传播。下方保留 Go 版
// 并发模型注释，其后为实现可运行的精简 Rust 版本。

// UnionExec 如何为每个 child 启动并发 resultPuller，并用资源池复用 chunk。
//
// Go var block: _ exec.Executor = &UnionExec{}。
// 这里用注释保留接口断言语义，表示 UnionExec 需要满足 exec.Executor。
//
// UnionExec pulls all it's children's result and returns to its parent directly.
// A "resultPuller" is started for every child to pull result from that child and push it to the "resultPool", the used
// "Chunk" is obtained from the corresponding "resourcePool". All resultPullers are running concurrently.
//	                          +----------------+
//	+---> resourcePool 1 ---> | resultPuller 1 |-----+
// | +----------------+ |
// | |
// | +----------------+ v
//	+---> resourcePool 2 ---> | resultPuller 2 |-----> resultPool ---+
// | +----------------+ ^ |
// | ...... | |
// | +----------------+ | |
// +---> resourcePool n ---> | resultPuller n |-----+ |
// | +----------------+ |
// | |
// | +-------------+ |
//	|--------------------------| main thread | <---------------------+
//	                           +-------------+
// UnionExec 对应 Go 的并发 union 执行器：多个 worker 拉取 child 结果，主线程从 resultPool 汇总。
// pub struct UnionExec {
//     pub base_executor: exec::BaseExecutor,
//     pub Concurrency: i32,
//     pub child_id_chan: chan::Sender<i32>,
//     pub stop_fetch_data: atomic::Value,
//     pub finished: chan::Sender<()>,
//     pub resource_pools: Vec<chan::Sender<*mut chunk::Chunk>>,
//     pub result_pool: chan::Sender<*mut unionWorkerResult>,
//     pub results: Vec<*mut chunk::Chunk>,
//     pub wg: sync::WaitGroup,
//     pub initialized: bool,
//     pub mu: unionExecMutexState,
//     pub child_in_flight_for_test: i32,
// }
//
// unionExecMutexState 对应 Go 中匿名 mu 结构体，保护已打开 child 的最大编号。
// pub struct unionExecMutexState {
//     pub mutex: *mut syncutil::Mutex,
//     pub max_opened_child_id: i32,
// }
//
// unionWorkerResult stores the result for a union worker.
// A "resultPuller" is started for every child to pull result from that child, unionWorkerResult is used to store that pulled result.
// "src" is used for Chunk reuse: after pulling result from "resultPool", main-thread must push a valid unused Chunk to "src" to
// enable the corresponding "resultPuller" continue to work.
// pub struct unionWorkerResult {
//     pub chk: *mut chunk::Chunk,
//     pub err: Option<error::Error>,
//     pub src: chan::Sender<*mut chunk::Chunk>,
// }
//
// impl UnionExec {
// waitAllFinished 等待所有 resultPuller 退出，然后关闭 resultPool 通知主线程无更多结果。
//     pub fn waitAllFinished(&mut self) {
//         self.wg.Wait();
//         close(self.result_pool.clone());
//     }
//
// Open implements the Executor Open interface.
// Open 只重置并发控制状态；child 在 resultPuller 中按需打开。
//     pub fn Open(&mut self, _ctx: context::Context) -> Result<(), error::Error> {
//         self.stop_fetch_data.Store(false);
//         self.initialized = false;
//         self.finished = make_chan();
//         self.mu.mutex = Box::into_raw(Box::new(syncutil::Mutex::default()));
//         self.mu.max_opened_child_id = -1;
//         Ok(())
//     }
//
// initialize 对应 Go 的首次 Next 初始化：创建结果池、资源池、child id 队列并启动 worker。
//     pub fn initialize(&mut self, ctx: context::Context) {
//         if self.Concurrency > self.ChildrenLen() {
//             self.Concurrency = self.ChildrenLen();
//         }
//         for _ in 0..self.Concurrency {
//             self.results.push(exec::NewFirstChunk(self.Children(0)));
//         }
//         self.result_pool = make_chan_with_capacity(self.Concurrency);
//         self.resource_pools = Vec::with_capacity(self.Concurrency as usize);
//         self.child_id_chan = make_chan_with_capacity(self.ChildrenLen());
//
//         for i in 0..self.Concurrency {
//             self.resource_pools.push(make_chan_with_capacity(1));
//             self.resource_pools[i as usize].send(self.results[i as usize]);
//             self.wg.Add(1);
// Go 这里启动 goroutine；保留异步边界，不实际绑定线程模型。
//             go!(self.resultPuller(ctx.clone(), i));
//         }
//         for i in 0..self.ChildrenLen() {
//             self.child_id_chan.send(i);
//         }
//         close(self.child_id_chan.clone());
//         go!(self.waitAllFinished());
//     }
//
// resultPuller 是每个 worker 的主循环：打开 child、复用 chunk 拉数据并发送给主线程。
//     pub fn resultPuller(&mut self, ctx: context::Context, worker_id: i32) {
//         let mut result = unionWorkerResult {
//             err: None,
//             chk: std::ptr::null_mut(),
//             src: self.resource_pools[worker_id as usize].clone(),
//         };
//
// Go defer 中含 recover：panic 会转为错误结果，并设置 stopFetchData 阻止其它 worker 继续拉取。
//         defer! {
//             if let Some(r) = recover() {
//                 logutil::Logger(ctx.clone()).Warn("resultPuller panicked", zap::Any("recover", r), zap::Stack("stack"));
//                 result.err = Some(util::GetRecoverError(r));
//                 self.stop_fetch_data.Store(true);
//                 let _ = self.sendResult(&mut result);
//             }
//             self.wg.Done();
//         }
//
//         failpoint::Inject("pauseUnionExecResultPuller", || {});
//         for child_id in self.child_id_chan.iter() {
//             self.mu.mutex.Lock();
//             if child_id > self.mu.max_opened_child_id {
//                 self.mu.max_opened_child_id = child_id;
//             }
//             self.mu.mutex.Unlock();
//
//             if let Err(err) = exec::Open(ctx.clone(), self.Children(child_id)) {
//                 result.err = Some(err);
//                 self.stop_fetch_data.Store(true);
//                 if !self.sendResult(&mut result) {
//                     return;
//                 }
//             }
//
//             failpoint::Inject("issue21441", || {
//                 atomic::AddInt32(&mut self.child_in_flight_for_test, 1);
//             });
//
//             loop {
//                 if self.stop_fetch_data.Load().as_bool() {
//                     return;
//                 }
//                 select! {
//                     recv(self.finished) => return,
//                     recv(self.resource_pools[worker_id as usize]) -> chk => {
//                         result.chk = chk;
//                     }
//                 }
//
//                 result.err = exec::Next(ctx.clone(), self.Children(child_id), result.chk).err();
//                 if result.err.is_none() && unsafe { (*result.chk).NumRows() } == 0 {
//                     self.resource_pools[worker_id as usize].send(result.chk);
//                     break;
//                 }
//                 failpoint::Inject("issue21441", || {
//                     if atomic::LoadInt32(&self.child_in_flight_for_test) as i32 > self.Concurrency {
//                         panic!("the count of child in flight is larger than e.concurrency unexpectedly");
//                     }
//                 });
//                 if !self.sendResult(&mut result) {
//                     return;
//                 }
//                 if result.err.is_some() {
//                     self.stop_fetch_data.Store(true);
//                     return;
//                 }
//             }
//             failpoint::Inject("issue21441", || {
//                 atomic::AddInt32(&mut self.child_in_flight_for_test, -1);
//             });
//         }
//     }
//
// sendResult 对应 Go 的 select：如果 Close 已关闭 finished，就不再阻塞发送结果。
//     pub fn sendResult(&mut self, result: &mut unionWorkerResult) -> bool {
//         select! {
//             recv(self.finished) => false,
//             send(self.result_pool, result) => true,
//         }
//     }
//
// Next implements the Executor Next interface.
// Next 首次调用时启动 worker；之后从 resultPool 取一个 worker 结果，交换列并归还 chunk。
//     pub fn Next(&mut self, ctx: context::Context, req: &mut chunk::Chunk) -> Result<(), error::Error> {
//         req.GrowAndReset(self.MaxChunkSize());
//         if !self.initialized {
//             self.initialize(ctx);
//             self.initialized = true;
//         }
//         let (result, ok) = self.result_pool.recv_with_ok();
//         if !ok {
//             return Ok(());
//         }
//         if let Some(err) = result.err {
//             return Err(errors::Trace(err));
//         }
//
//         if unsafe { (*result.chk).NumCols() } != req.NumCols() {
//             return Err(errors::Errorf(format!(
//                 "Internal error: UnionExec chunk column count mismatch, req: {}, result: {}",
//                 req.NumCols(),
//                 unsafe { (*result.chk).NumCols() },
//             )));
//         }
//         req.SwapColumns(result.chk);
//         result.src.send(result.chk);
//         Ok(())
//     }
//
// Close implements the Executor Close interface.
// Close 先通知 worker 停止并等待退出，再清空池子，最后关闭已经打开过的 child。
//     pub fn Close(&mut self) -> Result<(), error::Error> {
//         if !self.finished.is_nil() {
//             self.stop_fetch_data.Store(true);
//             close(self.finished.clone());
//             self.wg.Wait();
//         }
//         self.results.clear();
//         if !self.result_pool.is_nil() {
//             channel::Clear(self.result_pool.clone());
//         }
//         self.resource_pools.clear();
//         if !self.child_id_chan.is_nil() {
//             channel::Clear(self.child_id_chan.clone());
//         }
//
// We do not need to acquire the e.mu.Lock since all the resultPuller can be
// promised to exit when reaching here (e.childIDChan been closed).
//         let mut first_err: Option<error::Error> = None;
//         for i in 0..=self.mu.max_opened_child_id {
//             if let Err(err) = exec::Close(self.Children(i)) {
//                 if first_err.is_none() {
//                     first_err = Some(err);
//                 }
//             }
//         }
//         match first_err {
//             Some(err) => Err(err),
//             None => Ok(()),
//         }
//     }
// }
// */
// ---------- 可运行的精简实现：线程 + 通道汇总子执行器结果 ----------

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread;
use std::thread::JoinHandle;

/// 简化 Chunk：外层为行，内层为字符串列值。
pub type Chunk = Vec<Vec<String>>;
/// 子执行器接口：Open → 反复 Next → Close。
pub trait Executor: Send {
    fn open(&mut self) -> Result<(), String>;
    fn next(&mut self) -> Result<Option<Chunk>, String>;
    fn close(&mut self) -> Result<(), String>;
}

/// worker 发回主线程的一次结果：数据块或错误。
struct WorkerResult {
    chunk: Option<Chunk>,
    error: Option<String>,
}

type Child = Box<dyn Executor>;
type ChildSlots = Arc<Mutex<Vec<Option<Child>>>>;

fn send_result(
    sender: &mpsc::SyncSender<WorkerResult>,
    result: WorkerResult,
    cancel: &AtomicBool,
    force: bool,
) -> bool {
    let mut result = Some(result);
    loop {
        if !force && cancel.load(Ordering::Acquire) {
            return false;
        }
        match sender.try_send(result.take().expect("result must be present")) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
            Err(mpsc::TrySendError::Full(value)) => {
                result = Some(value);
                thread::yield_now();
            }
        }
    }
}

fn child_call<T>(call: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(result) => result,
        Err(_) => Err("union worker panicked".to_string()),
    }
}

/// 并发 Union 执行器：持有子执行器、结果接收端与取消标志。
pub struct UnionExec {
    children: Vec<Option<Child>>,
    receiver: Option<mpsc::Receiver<WorkerResult>>,
    joins: Vec<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    opened: bool,
    concurrency: usize,
    child_slots: Option<ChildSlots>,
    max_opened_child_id: Arc<Mutex<isize>>,
}

impl UnionExec {
    /// 用子执行器列表构造尚未打开的 UnionExec。
    pub fn new(children: Vec<Box<dyn Executor>>) -> Self {
        let concurrency = children.len();
        Self {
            children: children.into_iter().map(Some).collect(),
            receiver: None,
            joins: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            opened: false,
            concurrency,
            child_slots: None,
            max_opened_child_id: Arc::new(Mutex::new(-1)),
        }
    }

    /// 构造一个使用指定 worker 数量的 UnionExec。
    pub fn with_concurrency(children: Vec<Box<dyn Executor>>, concurrency: usize) -> Self {
        let mut union = Self::new(children);
        union.concurrency = concurrency;
        union
    }

    /// 当前 worker 数量上限。
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// 打开执行器：仅标记已打开并清除取消标志（延迟到 Next 时启动 worker）。
    pub fn open(&mut self) -> Result<(), String> {
        if self.opened {
            return Err("union executor is already open".to_string());
        }
        self.cancel.store(false, Ordering::Release);
        *self
            .max_opened_child_id
            .lock()
            .expect("union child state mutex poisoned") = -1;
        self.opened = true;
        Ok(())
    }

    fn restore_children(&mut self) {
        let Some(slots) = self.child_slots.take() else {
            return;
        };
        let mut slots = slots.lock().expect("union child slots mutex poisoned");
        self.children = slots.drain(..).collect();
    }

    /// 首次 Next 时启动 worker 池，经 sync_channel 回传 child 结果。
    fn initialize(&mut self) -> Result<(), String> {
        if self.receiver.is_some() {
            return Ok(());
        }
        if self.children.iter().any(Option::is_none) {
            return Err("union executor children are unavailable".to_string());
        }
        let child_count = self.children.len();
        let child_slots: ChildSlots = Arc::new(Mutex::new(std::mem::take(&mut self.children)));
        let child_ids = Arc::new(Mutex::new((0..child_count).collect::<VecDeque<usize>>()));
        let (sender, receiver) = mpsc::sync_channel(self.concurrency.max(1));
        let worker_count = self.concurrency.min(child_count);
        for _ in 0..worker_count {
            let sender = sender.clone();
            let cancel = self.cancel.clone();
            let child_ids = child_ids.clone();
            let child_slots = child_slots.clone();
            let max_opened_child_id = self.max_opened_child_id.clone();
            // 每个 worker 依次处理 child；child 在 Union Close 中统一关闭。
            self.joins.push(thread::spawn(move || {
                loop {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let child_id = child_ids
                        .lock()
                        .expect("union child id mutex poisoned")
                        .pop_front();
                    let Some(child_id) = child_id else {
                        break;
                    };
                    {
                        let mut max_opened_child_id = max_opened_child_id
                            .lock()
                            .expect("union child state mutex poisoned");
                        *max_opened_child_id = (*max_opened_child_id).max(child_id as isize);
                    }
                    let mut child = child_slots
                        .lock()
                        .expect("union child slots mutex poisoned")[child_id]
                        .take()
                        .expect("union child must be available");

                    let opened = match child_call(|| child.open()) {
                        Ok(()) => true,
                        Err(error) => {
                            cancel.store(true, Ordering::Release);
                            let _ = send_result(
                                &sender,
                                WorkerResult {
                                    chunk: None,
                                    error: Some(error),
                                },
                                &cancel,
                                true,
                            );
                            false
                        }
                    };
                    if opened {
                        while !cancel.load(Ordering::Acquire) {
                            match child_call(|| child.next()) {
                                Ok(Some(chunk)) => {
                                    if !send_result(
                                        &sender,
                                        WorkerResult {
                                            chunk: Some(chunk),
                                            error: None,
                                        },
                                        &cancel,
                                        false,
                                    ) {
                                        break;
                                    }
                                }
                                Ok(None) => break,
                                Err(error) => {
                                    cancel.store(true, Ordering::Release);
                                    let _ = send_result(
                                        &sender,
                                        WorkerResult {
                                            chunk: None,
                                            error: Some(error),
                                        },
                                        &cancel,
                                        true,
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    child_slots
                        .lock()
                        .expect("union child slots mutex poisoned")[child_id] = Some(child);
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                }
            }));
        }
        drop(sender);
        self.child_slots = Some(child_slots);
        self.receiver = Some(receiver);
        Ok(())
    }
    /// 拉取下一批结果；遇错误则设置取消标志停止其余 worker。
    pub fn next(&mut self) -> Result<Option<Chunk>, String> {
        if !self.opened {
            return Err("union executor is not open".to_string());
        }
        self.initialize()?;
        match self
            .receiver
            .as_ref()
            .expect("union receiver initialized")
            .recv()
        {
            Ok(WorkerResult {
                chunk: Some(chunk), ..
            }) => Ok(Some(chunk)),
            Ok(WorkerResult {
                error: Some(error), ..
            }) => {
                self.cancel.store(true, Ordering::Release);
                Err(error)
            }
            Ok(_) | Err(_) => Ok(None),
        }
    }
    /// 关闭：取消 worker、丢弃接收端、join 全部线程。
    pub fn close(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        self.receiver.take();
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
        self.restore_children();
        let max_opened_child_id = *self
            .max_opened_child_id
            .lock()
            .expect("union child state mutex poisoned");
        let mut first_error = None;
        for child in self
            .children
            .iter_mut()
            .take((max_opened_child_id + 1).max(0) as usize)
        {
            let Some(child) = child.as_mut() else {
                continue;
            };
            if let Err(error) = child_call(|| child.close()) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        self.opened = false;
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// 析构时确保 Close，避免 worker 泄漏。
impl Drop for UnionExec {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
