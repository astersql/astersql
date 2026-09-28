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

// DDL worker 池模块。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE TABLE、ADD INDEX
// 等修改库表结构（schema）的语句。执行 DDL 作业（job）需要专门的
// worker（工作线程/协程），本模块提供一个简单的 worker 对象池：
// 调用方通过 `get` 借出一个空闲 worker，用完后通过 `put` 归还，
// 从而复用 worker 并限制并发执行 DDL 作业的数量。
//
// 池按作业类型区分：普通 DDL（如建表、改列元信息，代价小）与
// reorg 类 DDL（reorganization，指需要回填/重组大量数据的作业，
// 如加索引、改列类型，代价大），二者使用各自独立的池。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// DDL 作业类型，用于区分不同的 worker 池。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobType {
    /// 普通 DDL 作业：只修改元数据、无需搬运数据，执行开销小。
    General,
    /// reorg（数据重组）类 DDL 作业：需要扫描并回填表数据，
    /// 例如添加索引、修改列类型，执行时间长、开销大。
    Reorg,
}
/// DDL worker，代表一个可执行 DDL 作业的工作单元。
///
/// Rust 适配层保留 worker 标识与池类型，由池负责借出与归还。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Worker {
    /// worker 在池中的编号（0..size）。
    pub id: usize,
    /// 该 worker 所属池的作业类型。
    pub job_type: JobType,
}
/// DDL worker 对象池。
///
/// 内部状态由 `Arc<Mutex<..>>` 保护，克隆 `WorkerPool` 只会克隆
/// 指向同一共享状态的句柄，因此可以安全地在多线程间共享同一个池。
#[derive(Clone)]
pub struct WorkerPool {
    /// 池中所有 worker 的作业类型（整池同类型）。
    job_type: JobType,
    /// 池的可变状态，用互斥锁保证并发安全。
    state: Arc<Mutex<PoolState>>,
}
/// 池的内部可变状态。
#[derive(Debug, Default)]
struct PoolState {
    /// 池是否已关闭；关闭后不再借出 worker，归还的 worker 也会被丢弃。
    closed: bool,
    /// 当前空闲、可被借出的 worker 队列（先进先出）。
    available: VecDeque<Worker>,
    /// 已借出、尚未归还的 worker 数量。
    borrowed: usize,
}
impl WorkerPool {
    /// 创建指定作业类型、容量为 `size` 的 worker 池，
    /// 并预先生成编号 0..size 的全部 worker 放入空闲队列。
    pub fn new(job_type: JobType, size: usize) -> Self {
        let available = (0..size).map(|id| Worker { id, job_type }).collect();
        Self {
            job_type,
            state: Arc::new(Mutex::new(PoolState {
                available,
                ..PoolState::default()
            })),
        }
    }
    /// 从池中借出一个空闲 worker。
    ///
    /// - 池已关闭时返回 `Err`；
    /// - 池未关闭但暂无空闲 worker 时返回 `Ok(None)`（非阻塞，不等待）；
    /// - 借出成功时递增 `borrowed` 计数并返回 `Ok(Some(worker))`。
    pub fn get(&self) -> Result<Option<Worker>, String> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err("workerPool is closed".into());
        }
        let worker = state.available.pop_front();
        if worker.is_some() {
            state.borrowed += 1;
        }
        Ok(worker)
    }
    /// 归还一个 worker。
    ///
    /// 先递减借出计数（用 saturating_sub 防止下溢）；
    /// 若池已关闭则直接丢弃该 worker，否则放回空闲队列尾部。
    pub fn put(&self, worker: Worker) {
        let mut state = self.state.lock().unwrap();
        state.borrowed = state.borrowed.saturating_sub(1);
        if !state.closed {
            state.available.push_back(worker);
        }
    }
    /// 关闭池：标记 closed 并清空空闲队列。
    ///
    /// 幂等操作，重复调用无副作用；已借出的 worker 归还时会被丢弃。
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        state.available.clear();
    }
    /// 返回池的作业类型。
    pub fn job_type(&self) -> JobType {
        self.job_type
    }
    /// 返回当前空闲 worker 数量。
    pub fn available(&self) -> usize {
        self.state.lock().unwrap().available.len()
    }
    /// 返回当前已借出、尚未归还的 worker 数量。
    pub fn borrowed(&self) -> usize {
        self.state.lock().unwrap().borrowed
    }
}
