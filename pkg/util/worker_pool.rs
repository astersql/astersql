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

// 有界工作线程池。
//
// 预创建固定数量的 Worker 令牌，经 channel 借出/回收以限制并发；
// 支持普通线程提交与挂到 `ErrorGroupWithRecover` 的可恢复任务。

use std::sync::Arc;
use std::thread;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};

use crate::wait_group_wrapper::ErrorGroupWithRecover;

/// 可克隆的工作池句柄，内部共享令牌 channel。
#[derive(Clone)]
pub struct WorkerPool {
    inner: Arc<WorkerPoolInner>,
}

/// 池容量、令牌收发端与调试用名称。
struct WorkerPoolInner {
    limit: usize,
    workers_tx: Sender<Worker>,
    workers_rx: Receiver<Worker>,
    name: String,
}

/// 工作令牌，携带唯一 ID 供任务侧识别。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Worker {
    /// Worker 编号（从 1 起）。
    pub ID: u64,
}

/// 创建容量为 `limit` 的池，并预填充 ID 为 1..=limit 的令牌。
pub fn NewWorkerPool(limit: usize, name: String) -> WorkerPool {
    let (workers_tx, workers_rx) = bounded(limit);
    for id in 1..=limit {
        workers_tx
            .send(Worker { ID: id as u64 })
            .expect("worker pool initialization failed");
    }
    WorkerPool {
        inner: Arc::new(WorkerPoolInner {
            limit,
            workers_tx,
            workers_rx,
            name,
        }),
    }
}

/// RAII 守卫：任务结束（含 panic）时自动把 Worker 还回池中。
struct RecycleGuard {
    pool: WorkerPool,
    worker: Option<Worker>,
}

impl Drop for RecycleGuard {
    fn drop(&mut self) {
        self.pool
            .RecycleWorker(self.worker.take().expect("worker guard used twice"));
    }
}

impl WorkerPool {
    /// 当前空闲（可立即借出）的 Worker 数量。
    pub fn IdleCount(&self) -> usize {
        self.inner.workers_rx.len()
    }

    /// 池容量上限。
    pub fn Limit(&self) -> usize {
        self.inner.limit
    }

    /// 借出一个 Worker，在新线程中执行闭包，结束后自动回收。
    pub fn Apply<F>(&self, function: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let worker = self.ApplyWorker();
        let pool = self.clone();
        thread::spawn(move || {
            let _guard = RecycleGuard {
                pool,
                worker: Some(worker),
            };
            function();
        });
    }

    /// 同 `Apply`，但把 Worker ID 传给闭包。
    pub fn ApplyWithID<F>(&self, function: F)
    where
        F: FnOnce(u64) + Send + 'static,
    {
        let worker = self.ApplyWorker();
        let id = worker.ID;
        let pool = self.clone();
        thread::spawn(move || {
            let _guard = RecycleGuard {
                pool,
                worker: Some(worker),
            };
            function(id);
        });
    }

    /// 在 ErrorGroup 上提交任务：借 Worker，执行后回收；错误由 group 汇总。
    pub fn ApplyOnErrorGroup<F>(&self, group: &ErrorGroupWithRecover, function: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let worker = self.ApplyWorker();
        let pool = self.clone();
        group.Go(move || {
            let _guard = RecycleGuard {
                pool,
                worker: Some(worker),
            };
            function()
        });
    }

    /// 同 `ApplyOnErrorGroup`，闭包额外接收 Worker ID。
    pub fn ApplyWithIDInErrorGroup<F>(&self, group: &ErrorGroupWithRecover, function: F)
    where
        F: FnOnce(u64) -> Result<()> + Send + 'static,
    {
        let worker = self.ApplyWorker();
        let id = worker.ID;
        let pool = self.clone();
        group.Go(move || {
            let _guard = RecycleGuard {
                pool,
                worker: Some(worker),
            };
            function(id)
        });
    }

    /// 非阻塞尝试取令牌；池空时阻塞等待，channel 断开则 panic。
    pub fn ApplyWorker(&self) -> Worker {
        match self.inner.workers_rx.try_recv() {
            Ok(worker) => worker,
            Err(TryRecvError::Empty) => {
                log::debug!("wait for workers; pool={}", self.inner.name);
                self.inner
                    .workers_rx
                    .recv()
                    .expect("worker pool channel disconnected")
            }
            Err(TryRecvError::Disconnected) => panic!("worker pool channel disconnected"),
        }
    }

    /// 将 Worker 令牌还回池中供复用。
    pub fn RecycleWorker(&self, worker: Worker) {
        self.inner
            .workers_tx
            .send(worker)
            .expect("worker pool channel disconnected");
    }

    /// 是否存在至少一个空闲 Worker。
    pub fn HasWorker(&self) -> bool {
        self.IdleCount() > 0
    }
}
