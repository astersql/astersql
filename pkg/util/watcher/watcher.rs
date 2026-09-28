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

// 轮询式文件系统监视器。
//
// 周期性对比已监视路径的元信息快照，推断 Create/Remove/Modify/Chmod/Rename/Move，
// 经无缓冲 channel 投递事件；常用于感知 binlog 等文件滚动与变更。

use super::event::{Chmod, Create, Event, FileInfo, Modify, Move, Remove, Rename};
use crossbeam_channel::{Receiver, Sender, bounded, select, tick};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Watcher 生命周期与 I/O 错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatcherError {
    /// 已启动，禁止重复 Start。
    Started,
    /// 已关闭，禁止再 Add/Start。
    Closed,
    /// 底层文件系统错误（带可读消息）。
    Io(String),
}

impl fmt::Display for WatcherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started => f.write_str("watcher already started"),
            Self::Closed => f.write_str("watcher already closed"),
            Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for WatcherError {}

/// 已启动错误常量（与 Go `ErrWatcherStarted` 对应）。
pub const ErrWatcherStarted: WatcherError = WatcherError::Started;
/// 已关闭错误常量（与 Go `ErrWatcherClosed` 对应）。
pub const ErrWatcherClosed: WatcherError = WatcherError::Closed;

/// 内部监视状态：用户添加的根路径集合 + 当前文件快照。
#[derive(Default)]
struct State {
    names: HashSet<PathBuf>,
    files: HashMap<PathBuf, FileInfo>,
}

/// Watcher 共享内部状态：事件/错误/关闭通道与工作线程句柄。
struct Inner {
    event_tx: Mutex<Option<Sender<Event>>>,
    error_tx: Mutex<Option<Sender<WatcherError>>>,
    close_tx: Sender<()>,
    close_rx: Receiver<()>,
    closed: AtomicBool,
    running: AtomicI32,
    operation: Mutex<()>,
    state: Mutex<State>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// 对外 Watcher：提供 Events/Errors 接收端与 Add/Remove/Start/Close。
pub struct Watcher {
    /// 文件变更事件接收端（无缓冲，与发送端同步）。
    pub Events: Receiver<Event>,
    /// 监视过程中的错误接收端。
    pub Errors: Receiver<WatcherError>,
    inner: Arc<Inner>,
}

/// 构造未启动的 Watcher，初始化事件/错误/关闭通道。
pub fn NewWatcher() -> Watcher {
    let (event_tx, Events) = bounded(0);
    let (error_tx, Errors) = bounded(0);
    let (close_tx, close_rx) = bounded(1);
    Watcher {
        Events,
        Errors,
        inner: Arc::new(Inner {
            event_tx: Mutex::new(Some(event_tx)),
            error_tx: Mutex::new(Some(error_tx)),
            close_tx,
            close_rx,
            closed: AtomicBool::new(false),
            running: AtomicI32::new(0),
            operation: Mutex::new(()),
            state: Mutex::new(State::default()),
            worker: Mutex::new(None),
        }),
    }
}

impl Watcher {
    /// 以给定轮询间隔启动后台监视线程；已运行或已关闭时返回错误。
    pub fn Start(&self, duration: Duration) -> Result<(), WatcherError> {
        // CAS 保证仅一个线程能成功置 running。
        if self
            .inner
            .running
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(ErrWatcherStarted);
        }
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(ErrWatcherClosed);
        }

        let inner = Arc::clone(&self.inner);
        *self.inner.worker.lock().unwrap() = Some(thread::spawn(move || do_watch(inner, duration)));
        Ok(())
    }

    /// 停止监视、清空状态；未运行时直接返回（可幂等调用）。
    pub fn Close(&self) {
        if self
            .inner
            .running
            .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        self.inner.closed.store(true, Ordering::SeqCst);
        let _ = self.inner.close_tx.try_send(());
        if let Some(worker) = self.inner.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        // 与 Go close(w.Events)/close(w.Errors) 一致：释放最后的发送端，
        // 使仍由调用方持有的 Receiver 立即观察到断开。
        self.inner.event_tx.lock().unwrap().take();
        self.inner.error_tx.lock().unwrap().take();
        // 持有 operation 锁后清空快照，避免与轮询竞态。
        let _operation = self.inner.operation.lock().unwrap();
        *self.inner.state.lock().unwrap() = State::default();
    }

    /// 添加监视路径（文件或目录一层子项），关闭后失败。
    pub fn Add(&self, name: impl AsRef<Path>) -> Result<(), WatcherError> {
        let _operation = self.inner.operation.lock().unwrap();
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(ErrWatcherClosed);
        }
        let name = name.as_ref().to_path_buf();
        let listed = listForName(&name)?;
        let mut state = self.inner.state.lock().unwrap();
        state.names.insert(name);
        state.files.extend(listed);
        Ok(())
    }

    /// 移除监视路径及其（若为目录）直接子项跟踪。
    pub fn Remove(&self, name: impl AsRef<Path>) -> Result<(), WatcherError> {
        let _operation = self.inner.operation.lock().unwrap();
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(ErrWatcherClosed);
        }
        do_remove(&self.inner, name.as_ref());
        Ok(())
    }
}

/// 从状态中删除路径；目录时一并去掉父路径匹配的直接子项。
fn do_remove(inner: &Inner, name: &Path) {
    let mut state = inner.state.lock().unwrap();
    state.names.remove(name);
    let Some(info) = state.files.remove(name) else {
        return;
    };
    if info.IsDir() {
        state.files.retain(|path, _| path.parent() != Some(name));
    }
}

/// 后台循环：按 ticker 间隔列举并 diff，关闭信号到达时退出。
fn do_watch(inner: Arc<Inner>, duration: Duration) {
    let ticker = tick(duration);
    loop {
        select! {
            recv(inner.close_rx) -> _ => return,
            recv(ticker) -> _ => {
                let _operation = inner.operation.lock().unwrap();
                let current = list_for_all(&inner);
                if !poll_events(&inner, &current) {
                    return;
                }
                inner.state.lock().unwrap().files = current;
            }
        }
    }
}

/// 发送事件；若已关闭则返回 false 以终止轮询。
fn send_event(inner: &Inner, event: Event) -> bool {
    let Some(event_tx) = inner.event_tx.lock().unwrap().as_ref().cloned() else {
        return false;
    };
    select! {
        recv(inner.close_rx) -> _ => false,
        send(event_tx, event) -> result => result.is_ok(),
    }
}

/// 对比新旧快照，发送 Modify/Chmod，再配对 rename/move，最后 Create/Remove。
fn poll_events(inner: &Inner, current: &HashMap<PathBuf, FileInfo>) -> bool {
    let previous = inner.state.lock().unwrap().files.clone();
    let mut creates = HashMap::new();
    let mut removes = HashMap::new();

    // 旧有、新无 → 候选 Remove。
    for (path, info) in &previous {
        if !current.contains_key(path) {
            removes.insert(path.clone(), info.clone());
        }
    }
    for (path, current_info) in current {
        let Some(previous_info) = previous.get(path) else {
            creates.insert(path.clone(), current_info.clone());
            continue;
        };
        // 修改时间或大小变化 → Modify。
        if (previous_info.ModTime() != current_info.ModTime()
            || previous_info.Size() != current_info.Size())
            && !send_event(inner, event(path, Modify, current_info))
        {
            return false;
        }
        // 权限变化 → Chmod。
        if previous_info.Mode() != current_info.Mode()
            && !send_event(inner, event(path, Chmod, current_info))
        {
            return false;
        }
    }

    // 同一 inode 在不同路径上消失/出现 → Rename（同父）或 Move（异父）。
    for (removed_path, removed_info) in removes.clone() {
        for (created_path, created_info) in creates.clone() {
            if removed_info.same_file(&created_info) {
                let op = if removed_path.parent() == created_path.parent() {
                    Rename
                } else {
                    Move
                };
                removes.remove(&removed_path);
                creates.remove(&created_path);
                if !send_event(inner, event(&removed_path, op, &removed_info)) {
                    return false;
                }
                break;
            }
        }
    }
    for (path, info) in creates {
        if !send_event(inner, event(&path, Create, &info)) {
            return false;
        }
    }
    for (path, info) in removes {
        if !send_event(inner, event(&path, Remove, &info)) {
            return false;
        }
    }
    true
}

/// 组装一条 Event。
fn event(path: &Path, op: u32, info: &FileInfo) -> Event {
    Event {
        Path: path.to_path_buf(),
        Op: op,
        FileInfo: info.clone(),
    }
}

/// 列举所有已注册根路径下的当前文件快照；根消失时自动 Remove 并上报错误。
fn list_for_all(inner: &Inner) -> HashMap<PathBuf, FileInfo> {
    let names: Vec<PathBuf> = inner.state.lock().unwrap().names.iter().cloned().collect();
    let mut all = HashMap::new();
    for name in names {
        match listForName(&name) {
            Ok(files) => all.extend(files),
            Err(error) => {
                // 路径不存在时停止跟踪该根。
                if matches!(&error, WatcherError::Io(message) if message.contains("not found")) {
                    do_remove(inner, &name);
                }
                let Some(error_tx) = inner.error_tx.lock().unwrap().as_ref().cloned() else {
                    return HashMap::new();
                };
                select! {
                    recv(inner.close_rx) -> _ => return HashMap::new(),
                    send(error_tx, error) -> _ => {}
                }
            }
        }
    }
    all
}

/// 列举路径自身；若为目录则再包含其直接子项（非递归）。
pub fn listForName(name: impl AsRef<Path>) -> Result<HashMap<PathBuf, FileInfo>, WatcherError> {
    let name = name.as_ref();
    let metadata = fs::metadata(name).map_err(|error| io_error("name", name, error))?;
    let is_dir = metadata.is_dir();
    let mut listed = HashMap::from([(name.to_path_buf(), FileInfo::new(metadata))]);
    if !is_dir {
        return Ok(listed);
    }
    let entries = fs::read_dir(name).map_err(|error| io_error("directory", name, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error("directory", name, error))?;
        let metadata = entry
            .metadata()
            .map_err(|error| io_error("directory", name, error))?;
        listed.insert(entry.path(), FileInfo::new(metadata));
    }
    Ok(listed)
}

/// 将 I/O 错误包装为带路径上下文的 `WatcherError::Io`。
fn io_error(kind: &str, name: &Path, error: io::Error) -> WatcherError {
    WatcherError::Io(format!("{kind} {}: {error}", name.display()))
}
