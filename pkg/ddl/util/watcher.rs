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

// etcd schema 路径的 Watcher 抽象与线程安全实现。
//
// `Rewatch` 会先清空旧 channel 再异步重新注册，以对齐 Go 侧
// “先关闭旧事件流再建立新监听”的顺序保证。

use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::{CancellationToken, DdlUtilError, EtcdClient, WatchChannel};

/// schema/版本路径的 etcd 监听接口。
pub trait Watcher: Send + Sync {
    /// 当前事件通道；未就绪或失败时为 `None`。
    fn WatchChan(&self) -> Option<WatchChannel>;
    /// 同步注册 watch；失败时清空 channel 并记录错误。
    fn Watch(&self, cancellation: &CancellationToken, etcd_client: &EtcdClient, path: &str);
    /// 清空旧 channel 后在后台线程重新 watch。
    fn Rewatch(&self, cancellation: CancellationToken, etcd_client: EtcdClient, path: String);
    /// 最近一次 watch/rewatch 错误。
    fn LastError(&self) -> Option<DdlUtilError>;
    /// 最近一次 rewatch 耗时。
    fn LastRewatchDuration(&self) -> Option<Duration>;
}

/// watcher 可变状态。
struct WatcherState {
    /// 当前事件通道。
    channel: Option<WatchChannel>,
    /// 最近错误。
    last_error: Option<DdlUtilError>,
    /// 最近 rewatch 耗时。
    last_rewatch_duration: Option<Duration>,
}

/// 全空默认状态。
impl Default for WatcherState {
    fn default() -> Self {
        Self {
            channel: None,
            last_error: None,
            last_rewatch_duration: None,
        }
    }
}

/// 线程安全的 etcd watcher。
/// Thread-safe etcd watcher. Rewatch clears the old channel before starting
/// asynchronous registration, matching the close-event ordering guarantee in Go.
#[derive(Clone, Default)]
pub struct watcher {
    /// 共享可变状态。
    state: Arc<RwLock<WatcherState>>,
}

/// 构造默认 watcher 的 trait 对象。
pub fn NewWatcher() -> Arc<dyn Watcher> {
    Arc::new(watcher::default())
}

impl Watcher for watcher {
    fn WatchChan(&self) -> Option<WatchChannel> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .channel
            .clone()
    }

    fn Watch(&self, cancellation: &CancellationToken, etcd_client: &EtcdClient, path: &str) {
        let result = cancellation.check().and_then(|_| etcd_client.watch(path));
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 成功则发布 channel；失败则清空并记录 last_error。
        match result {
            Ok(channel) => {
                state.channel = Some(channel);
                state.last_error = None;
            }
            Err(error) => {
                state.channel = None;
                state.last_error = Some(error);
            }
        }
    }

    fn Rewatch(&self, cancellation: CancellationToken, etcd_client: EtcdClient, path: String) {
        // 先释放旧 channel，保证消费者先看到关闭再收到新流。
        {
            let mut state = self
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.channel = None;
            state.last_error = None;
        }

        let state = Arc::clone(&self.state);
        // 后台完成 watch 并更新耗时与结果。
        thread::spawn(move || {
            let started = Instant::now();
            let result = cancellation.check().and_then(|_| etcd_client.watch(&path));
            let mut state = state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.last_rewatch_duration = Some(started.elapsed());
            match result {
                Ok(channel) => {
                    state.channel = Some(channel);
                    state.last_error = None;
                }
                Err(error) => {
                    state.channel = None;
                    state.last_error = Some(error);
                }
            }
        });
    }

    fn LastError(&self) -> Option<DdlUtilError> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_error
            .clone()
    }

    fn LastRewatchDuration(&self) -> Option<Duration> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_rewatch_duration
    }
}
