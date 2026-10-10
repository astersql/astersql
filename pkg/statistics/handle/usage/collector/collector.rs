// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 统计使用量全局/会话增量收集器。
//
// 对齐 Go 侧 collector：全局 worker 从普通通道与高优先级通道接收会话增量（delta），
// 经 `merge_fn` 合并；会话侧优先非阻塞 `SendDelta`，超时后走阻塞的 `SendDeltaSync`。
// Close 时丢弃 close sender、join worker，并在退出前 flush 残留消息。

#![allow(non_camel_case_types, non_snake_case)]

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, select, select_biased};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 会话非阻塞发送超时阈值：超过后改走同步高优先级发送。默认 5 分钟。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// 普通/高优先级有界通道容量，与 Go 默认对齐。
pub const DEFAULT_CHANNEL_SIZE: usize = 10;

/// 全局收集器：派生会话句柄、启动合并 worker、关闭并回收。
pub trait GlobalCollector<T> {
    /// 为当前会话派生一个可发送增量的会话收集器。
    fn SpawnSession(&self) -> sessionCollector<T>;
    /// 关闭全局收集器并等待 worker 退出。
    fn Close(&self);
    /// 启动后台合并 worker（幂等：已关闭则直接返回）。
    fn StartWorker(&self);
}

/// 会话收集器：向全局通道投递增量数据。
pub trait SessionCollector<T> {
    /// 非阻塞尝试发送；通道满或失败返回 false。超时后降级为同步发送。
    fn SendDelta(&mut self, data: T) -> bool;
    /// 尝试发送并在通道拒绝时归还原始增量。
    fn TrySendDelta(&mut self, data: T) -> Result<(), T>;
    /// 阻塞发送到高优先级通道；关闭时返回 false。
    fn SendDeltaSync(&mut self, data: T) -> bool;
}

/// 全局收集器实现：双通道、close 信号、worker 列表与合并回调。
pub struct globalCollector<T> {
    /// 将收到的增量合并进全局状态的回调。
    merge_fn: Arc<dyn Fn(T) + Send + Sync>,
    /// 普通优先级数据发送端。
    data_sender: Sender<T>,
    /// 普通优先级数据接收端（worker 侧）。
    data_receiver: Receiver<T>,
    /// 高优先级数据发送端（同步路径）。
    high_priority_sender: Sender<T>,
    /// 高优先级数据接收端（worker 优先消费）。
    high_priority_receiver: Receiver<T>,
    /// close 信号发送端；`take` 后通道关闭以唤醒等待方。
    close_sender: Mutex<Option<Sender<()>>>,
    /// close 信号接收端（会话与 worker 共享）。
    close_receiver: Receiver<()>,
    /// 已启动的 worker 线程句柄。
    workers: Mutex<Vec<JoinHandle<()>>>,
    /// 确保 Close 只执行一次。
    close_once: Once,
    /// 是否已关闭（阻止再启 worker）。
    closed: AtomicBool,
    /// 会话非阻塞发送的超时阈值。
    timeout: Duration,
}

/// 会话侧收集器：记录上次成功发送时间，并持有两个发送端与 close 接收端。
pub struct sessionCollector<T> {
    /// 上次成功投递时间，用于判断是否超时改走同步路径。
    last_update: Instant,
    /// 普通通道发送端。
    data_sender: Sender<T>,
    /// 高优先级通道发送端。
    high_priority_sender: Sender<T>,
    /// 关闭信号接收端；同步发送时与 send 竞态。
    close_receiver: Receiver<()>,
    /// 非阻塞路径超时阈值。
    timeout: Duration,
}

/// 构造全局收集器：创建有界数据通道、同步 close 通道，默认超时与容量。
pub fn NewGlobalCollector<T: Send + 'static>(
    merge_fn: impl Fn(T) + Send + Sync + 'static,
) -> globalCollector<T> {
    let (data_sender, data_receiver) = bounded(DEFAULT_CHANNEL_SIZE);
    let (high_priority_sender, high_priority_receiver) = bounded(DEFAULT_CHANNEL_SIZE);
    let (close_sender, close_receiver) = bounded(0);
    globalCollector {
        merge_fn: Arc::new(merge_fn),
        data_sender,
        data_receiver,
        high_priority_sender,
        high_priority_receiver,
        close_sender: Mutex::new(Some(close_sender)),
        close_receiver,
        workers: Mutex::new(Vec::new()),
        close_once: Once::new(),
        closed: AtomicBool::new(false),
        timeout: DEFAULT_TIMEOUT,
    }
}

impl<T: Send + 'static> globalCollector<T> {
    /// 克隆通道句柄，派生绑定本全局收集器的会话收集器。
    pub fn SpawnSession(&self) -> sessionCollector<T> {
        sessionCollector {
            last_update: Instant::now(),
            data_sender: self.data_sender.clone(),
            high_priority_sender: self.high_priority_sender.clone(),
            close_receiver: self.close_receiver.clone(),
            timeout: self.timeout,
        }
    }

    /// 启动一个合并 worker：优先消费高优先级队列，再阻塞等待普通数据或关闭。
    pub fn StartWorker(&self) {
        let mut workers = self.workers.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            return;
        }

        let merge_fn = Arc::clone(&self.merge_fn);
        let data_receiver = self.data_receiver.clone();
        let high_priority_receiver = self.high_priority_receiver.clone();
        let close_receiver = self.close_receiver.clone();
        let worker = thread::spawn(move || {
            loop {
                // Match Go's nested select: prefer already queued high-priority data,
                // then block on either data channel or closure.
                // 对齐 Go 嵌套 select：先取已排队的高优先级数据，再阻塞等待普通数据或关闭。
                select_biased! {
                    recv(high_priority_receiver) -> message => match message {
                        Ok(data) => merge_fn(data),
                        Err(_) => break,
                    },
                    recv(close_receiver) -> _ => break,
                    default => {
                        select! {
                            recv(data_receiver) -> message => match message {
                                Ok(data) => merge_fn(data),
                                Err(_) => break,
                            },
                            recv(high_priority_receiver) -> message => match message {
                                Ok(data) => merge_fn(data),
                                Err(_) => break,
                            },
                            recv(close_receiver) -> _ => break,
                        }
                    }
                }
            }

            // 退出前排空两通道残留，避免 Close 丢未合并增量
            flush(&merge_fn, &high_priority_receiver, &data_receiver);
        });
        workers.push(worker);
    }

    /// 标记关闭、丢弃 close sender，并 join 全部 worker。
    pub fn Close(&self) {
        self.close_once.call_once(|| {
            self.closed.store(true, Ordering::Release);
            // take 掉 sender 使 close_receiver 断开，唤醒阻塞中的同步发送
            self.close_sender.lock().unwrap().take();
            for worker in self.workers.lock().unwrap().drain(..) {
                worker.join().expect("collector worker panicked");
            }
        });
    }
}

/// 非阻塞排空高优先级与普通通道，将残留增量交给 merge_fn。
fn flush<T>(
    merge_fn: &Arc<dyn Fn(T) + Send + Sync>,
    high_priority_receiver: &Receiver<T>,
    data_receiver: &Receiver<T>,
) {
    loop {
        match high_priority_receiver.try_recv() {
            Ok(data) => {
                merge_fn(data);
                continue;
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
        }
        match data_receiver.try_recv() {
            Ok(data) => merge_fn(data),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        }
    }
}

impl<T> sessionCollector<T> {
    /// 未超时则 try_send 普通通道；超时或需保证送达时走 SendDeltaSync。
    pub fn SendDelta(&mut self, data: T) -> bool {
        self.TrySendDelta(data).is_ok()
    }

    /// 非阻塞尝试发送；失败时把所有权归还调用者，避免为了重试而共享或克隆增量。
    pub fn TrySendDelta(&mut self, data: T) -> Result<(), T> {
        if self.last_update.elapsed() > self.timeout {
            return self.send_delta_sync_recover(data);
        }

        match self.data_sender.try_send(data) {
            Ok(()) => {
                self.last_update = Instant::now();
                Ok(())
            }
            Err(crossbeam_channel::TrySendError::Full(data))
            | Err(crossbeam_channel::TrySendError::Disconnected(data)) => Err(data),
        }
    }

    /// 阻塞写入高优先级通道；若先收到关闭信号则失败返回 false。
    pub fn SendDeltaSync(&mut self, data: T) -> bool {
        self.send_delta_sync_recover(data).is_ok()
    }

    /// 同步发送的所有权保留形式；关闭或断连时归还未发送的数据。
    fn send_delta_sync_recover(&mut self, data: T) -> Result<(), T> {
        select! {
            send(self.high_priority_sender, data) -> result => {
                match result {
                    Ok(()) => {
                        self.last_update = Instant::now();
                        Ok(())
                    }
                    Err(error) => Err(error.0),
                }
            },
            recv(self.close_receiver) -> _ => Err(data),
        }
    }
}

impl<T: Send + 'static> GlobalCollector<T> for globalCollector<T> {
    fn SpawnSession(&self) -> sessionCollector<T> {
        globalCollector::SpawnSession(self)
    }

    fn Close(&self) {
        globalCollector::Close(self);
    }

    fn StartWorker(&self) {
        globalCollector::StartWorker(self);
    }
}

impl<T> SessionCollector<T> for sessionCollector<T> {
    fn SendDelta(&mut self, data: T) -> bool {
        sessionCollector::SendDelta(self, data)
    }

    fn TrySendDelta(&mut self, data: T) -> Result<(), T> {
        sessionCollector::TrySendDelta(self, data)
    }

    fn SendDeltaSync(&mut self, data: T) -> bool {
        sessionCollector::SendDeltaSync(self, data)
    }
}
