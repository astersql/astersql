// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 任务管理器：登记、分片存放并发任务的元数据与通道。
//
// 对应 Go `poolmanager` 中的 TaskManager：按固定分片数（shard）哈希 taskID，
// 降低不同任务之间的锁竞争；每个任务持有任务队列与退出信号通道，供资源管理器
// 在 Overclock/Downclock 时调整 worker 并发度。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};

/// 任务元数据分片数，固定为 8，与 Go 实现一致。
const shard: usize = 8;

/// 由任务 ID 取模得到分片下标。
fn getShardID(id: u64) -> usize {
    (id % shard as u64) as usize
}

/// 可发送到任务通道的一次性闭包任务（对应 Go 的 `func()`）。
pub type Task = Box<dyn FnOnce() + Send + 'static>;

/// 无界任务通道：发送可关闭的任务闭包，接收端加锁保护。
#[derive(Clone)]
pub struct TaskChannel {
    /// 任务发送端。
    sender: Sender<Task>,
    /// MPMC 任务接收端，可与退出通道共同阻塞选择。
    receiver: Receiver<Task>,
    /// 丢弃唯一发送端会关闭此广播通道并唤醒全部 worker。
    close_sender: Arc<Mutex<Option<Sender<()>>>>,
    close_receiver: Receiver<()>,
    /// 串行化 send 与 close，避免关闭检查和实际发送之间的竞态。
    gate: Arc<Mutex<()>>,
    /// 通道是否已标记关闭（关闭后拒绝 send）。
    closed: Arc<AtomicBool>,
}

impl TaskChannel {
    /// 创建一对新的任务收发通道。
    pub fn new() -> Self {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let (close_sender, close_receiver) = crossbeam_channel::bounded(0);
        Self {
            sender,
            receiver,
            close_sender: Arc::new(Mutex::new(Some(close_sender))),
            close_receiver,
            gate: Arc::new(Mutex::new(())),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 发送任务；若已关闭则返回 SendError。
    pub fn send(&self, task: Task) -> Result<(), crossbeam_channel::SendError<Task>> {
        let _gate = self.gate.lock().unwrap();
        if self.is_closed() {
            return Err(crossbeam_channel::SendError(task));
        }
        self.sender.send(task)
    }

    /// 标记通道关闭（不丢弃已在队列中的任务）。
    pub fn close(&self) {
        let _gate = self.gate.lock().unwrap();
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.close_sender.lock().unwrap().take();
    }

    /// 查询通道是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 非阻塞尝试接收一个任务；空队列返回 Ok(None)。
    pub fn try_recv(&self) -> Result<Option<Task>, crossbeam_channel::TryRecvError> {
        match self.receiver.try_recv() {
            Ok(task) => Ok(Some(task)),
            Err(crossbeam_channel::TryRecvError::Empty) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// 阻塞接收一个任务。
    pub fn recv(&self) -> Result<Task, crossbeam_channel::RecvError> {
        crossbeam_channel::select_biased! {
            recv(self.receiver) -> task => task,
            recv(self.close_receiver) -> _ => self.receiver.try_recv().map_err(|_| crossbeam_channel::RecvError),
        }
    }

    pub fn data_receiver(&self) -> Receiver<Task> {
        self.receiver.clone()
    }

    pub fn close_receiver(&self) -> Receiver<()> {
        self.close_receiver.clone()
    }
}

impl Default for TaskChannel {
    fn default() -> Self {
        Self::new()
    }
}

/// 有界信号通道：用于通知任务退出（对应 Go 的 exit channel）。
#[derive(Clone)]
pub struct SignalChannel {
    /// 同步发送端（带容量）。
    sender: Sender<()>,
    /// 接收端。
    receiver: Receiver<()>,
    /// 是否已关闭。
    closed: Arc<AtomicBool>,
}

impl SignalChannel {
    /// 创建容量为 `capacity` 的有界信号通道。
    pub fn bounded(capacity: usize) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(capacity);
        Self {
            sender,
            receiver,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 非阻塞发送一次退出信号。
    pub fn try_send(&self) -> Result<(), crossbeam_channel::TrySendError<()>> {
        self.sender.try_send(())
    }

    /// 非阻塞尝试接收信号；空则 Ok(false)。
    pub fn try_recv(&self) -> Result<bool, crossbeam_channel::TryRecvError> {
        match self.receiver.try_recv() {
            Ok(()) => Ok(true),
            Err(crossbeam_channel::TryRecvError::Empty) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// 查询信号通道是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn receiver(&self) -> Receiver<()> {
        self.receiver.clone()
    }
}

/// `Meta` 的同一阻塞选择结果，对应 Go `select` 的三个可观察分支。
pub enum MetaEvent {
    Task(Task),
    Exit,
    Closed,
}

/// 单个已注册任务的元数据：创建时间、通道、并发计数等。
#[derive(Clone)]
pub struct Meta {
    /// 任务创建时间戳，用于选择最老/最新任务做调容。
    createTS: Instant,
    /// 退出信号通道（Downclock 时向其发送暂停信号）。
    exitCh: SignalChannel,
    /// 任务队列通道。
    taskCh: TaskChannel,
    /// 任务唯一 ID。
    taskID: u64,
    /// 当前正在运行的 worker 数（原子计数）。
    running: Arc<AtomicI32>,
    /// 注册时的初始并发度（调容上下限参考）。
    initialConcurrency: i32,
}

impl Meta {
    /// 构造任务元数据（保留 Go 风格大写构造函数名）。
    pub fn NewMeta(
        taskID: u64,
        exitCh: SignalChannel,
        taskCh: TaskChannel,
        concurrency: i32,
    ) -> Self {
        Self {
            createTS: Instant::now(),
            exitCh,
            taskCh,
            taskID,
            running: Arc::new(AtomicI32::new(0)),
            initialConcurrency: concurrency,
        }
    }

    /// Rust 风格构造：自动创建容量为 1 的退出通道。
    pub fn new(task_id: u64, task_ch: TaskChannel, concurrency: i32) -> Self {
        Self::NewMeta(task_id, SignalChannel::bounded(1), task_ch, concurrency)
    }

    /// 返回任务 ID。
    pub fn TaskID(&self) -> u64 {
        self.taskID
    }

    /// 运行中任务数加一。
    pub fn IncTask(&self) {
        self.running.fetch_add(1, Ordering::SeqCst);
    }

    /// 运行中任务数减一。
    pub fn DecTask(&self) {
        self.running.fetch_sub(1, Ordering::SeqCst);
    }

    /// 克隆任务通道句柄。
    pub fn GetTaskCh(&self) -> TaskChannel {
        self.taskCh.clone()
    }

    /// 克隆退出信号通道句柄。
    pub fn GetExitCh(&self) -> SignalChannel {
        self.exitCh.clone()
    }

    /// 同时阻塞等待任务、降频退出或任务通道关闭；多个就绪分支随机公平选择。
    pub fn recv_event(&self) -> MetaEvent {
        let tasks = self.taskCh.data_receiver();
        let closed = self.taskCh.close_receiver();
        let exit = self.exitCh.receiver();
        crossbeam_channel::select! {
            recv(tasks) -> task => task.map(MetaEvent::Task).unwrap_or(MetaEvent::Closed),
            recv(exit) -> _ => MetaEvent::Exit,
            recv(closed) -> _ => tasks.try_recv().map(MetaEvent::Task).unwrap_or(MetaEvent::Closed),
        }
    }
}

/// 单个分片内的任务状态容器（RwLock 保护的 ID→Meta 映射）。
pub struct TaskStatusContainer {
    /// 本分片内的任务元数据表。
    stats: RwLock<HashMap<u64, Meta>>,
}

/// 全局任务管理器：按分片存放任务，并记录原始并发度上限。
pub struct TaskManager {
    /// 固定长度的分片数组。
    task: Vec<TaskStatusContainer>,
    /// 管理器创建时配置的原始并发度。
    concurrency: i32,
}

impl TaskManager {
    /// 按给定并发度创建 TaskManager，并初始化全部空分片。
    pub fn NewTaskManager(c: i32) -> Self {
        let task = (0..shard)
            .map(|_| TaskStatusContainer {
                stats: RwLock::new(HashMap::new()),
            })
            .collect();
        Self {
            task,
            concurrency: c,
        }
    }

    /// Rust 风格构造别名。
    pub fn new(c: i32) -> Self {
        Self::NewTaskManager(c)
    }

    /// 将任务元数据登记到对应分片。
    pub fn RegisterTask(&self, task: Meta) {
        let id = getShardID(task.taskID);
        self.task[id]
            .stats
            .write()
            .unwrap()
            .insert(task.taskID, task);
    }

    /// Rust 风格登记别名。
    pub fn register_task(&self, task: Meta) {
        self.RegisterTask(task);
    }

    /// 按 taskID 从对应分片删除任务。
    pub fn DeleteTask(&self, taskID: u64) {
        let shardID = getShardID(taskID);
        self.task[shardID].stats.write().unwrap().remove(&taskID);
    }

    /// Rust 风格删除别名。
    pub fn delete_task(&self, task_id: u64) {
        self.DeleteTask(task_id);
    }

    /// 返回创建时配置的原始并发度。
    pub fn GetOriginConcurrency(&self) -> i32 {
        self.concurrency
    }

    /// Rust 风格查询原始并发度别名。
    pub fn get_origin_concurrency(&self) -> i32 {
        self.GetOriginConcurrency()
    }
}
