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

// 资源管理器（ResourceManager）核心：注册 goroutine 池、启动 CPU 观测与周期调度。
//
// 对应 TiDB 的 `resourcemanager`：进程内单例持有分片池映射、调度器列表与 CPU 观察者；
// `Start` 后每 100ms 对已注册池执行一次调容（schedule），`Stop` 关闭后台循环。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;

use crate::cpu::{NewCPUObserver, Observer};
use crate::scheduler::{NewCPUScheduler, Scheduler};
use crate::util::{
    Component, GoroutinePool, NewShardPoolMap, PoolContainer, PoolMapError, ShardPoolMap,
};
use crate::wait_group_wrapper::WaitGroupWrapper;

// InstanceResourceManager is the process-local resource manager singleton.
/// 进程内资源管理器单例，供全局注册与调度使用。
pub static InstanceResourceManager: LazyLock<ResourceManager> = LazyLock::new(NewResourceManger);

// RandomName returns a fresh UUID, matching the Go test helper.
/// 生成随机 UUID 字符串，与 Go 测试辅助函数行为一致。
pub fn RandomName() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ResourceManager owns the registered pools, schedulers, CPU observer and
// background scheduling lifecycle for this TiDB instance.
/// 资源管理器外壳：通过 Arc 共享内部状态，可 Clone 跨线程使用。
#[derive(Clone)]
pub struct ResourceManager {
    /// 共享的内部实现。
    pub(crate) inner: Arc<ResourceManagerInner>,
}

/// 资源管理器内部状态：池映射、调度器、CPU 观察者与退出通道。
pub(crate) struct ResourceManagerInner {
    /// 按名称分片登记的 goroutine 池映射。
    pub(crate) poolMap: RwLock<Arc<ShardPoolMap>>,
    /// 已安装的调度器列表（默认含 CPUScheduler）。
    pub(crate) scheduler: Vec<Box<dyn Scheduler + Send + Sync>>,
    /// CPU 使用率观察者。
    cpuObserver: Arc<Mutex<Observer>>,
    /// 调度循环退出状态；Go 的 exitCh 一旦 Stop 关闭便不可重新打开。
    exitCh: Mutex<ExitState>,
    /// 等待后台调度协程结束的 WaitGroup 包装。
    wg: WaitGroupWrapper,
}

/// 记录当前调度循环发送端及 Go channel 的永久关闭状态。
struct ExitState {
    sender: Option<Sender<()>>,
    stopped: bool,
}

// NewResourceManger preserves the misspelled public Go constructor name.
/// 创建默认资源管理器（保留 Go 侧拼写错误的构造函数名）。
pub fn NewResourceManger() -> ResourceManager {
    ResourceManager::NewResourceManger()
}

impl ResourceManager {
    /// 使用默认 CPU 调度器构造资源管理器。
    pub fn NewResourceManger() -> Self {
        Self::new_with_schedulers(vec![Box::new(NewCPUScheduler())])
    }

    /// 注入自定义调度器列表的构造函数（测试隐藏接口）。
    #[doc(hidden)]
    pub fn new_with_schedulers(schedulers: Vec<Box<dyn Scheduler + Send + Sync>>) -> Self {
        Self {
            inner: Arc::new(ResourceManagerInner {
                poolMap: RwLock::new(Arc::new(NewShardPoolMap())),
                scheduler: schedulers,
                cpuObserver: Arc::new(Mutex::new(NewCPUObserver())),
                exitCh: Mutex::new(ExitState {
                    sender: None,
                    stopped: false,
                }),
                wg: WaitGroupWrapper::default(),
            }),
        }
    }

    // Start begins CPU observation and the 100ms scheduling loop.
    /// 启动 CPU 观测，并开启每 100ms 一次的调度循环。
    pub fn Start(&self) {
        let (exit, exit_receiver) = mpsc::channel();
        {
            let mut exit_state = self
                .inner
                .exitCh
                .lock()
                .expect("resource manager exit channel mutex poisoned");
            // Go 的 exitCh 在构造时创建；Stop 关闭后，后续 Start 的 worker
            // 会立即观察到关闭并退出，因此不得重新启用调度。
            if exit_state.stopped {
                return;
            }
            exit_state.sender = Some(exit);
        }

        self.inner
            .cpuObserver
            .lock()
            .expect("resource manager CPU observer mutex poisoned")
            .Start();

        let manager = self.clone();
        // 后台循环：超时则 schedule，收到退出信号或断开则返回。
        self.inner.wg.Run(move || {
            loop {
                match exit_receiver.recv_timeout(Duration::from_millis(100)) {
                    Err(RecvTimeoutError::Timeout) => manager.schedule(),
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
    }

    // Stop stops the observer, closes the scheduling loop and waits for all
    // resource-manager workers to exit.
    /// 停止 CPU 观测、关闭调度循环，并等待全部后台 worker 退出。
    pub fn Stop(&self) {
        self.inner
            .cpuObserver
            .lock()
            .expect("resource manager CPU observer mutex poisoned")
            .Stop();
        let exit = {
            let mut exit_state = self
                .inner
                .exitCh
                .lock()
                .expect("resource manager exit channel mutex poisoned");
            assert!(!exit_state.stopped, "close of closed resource manager");
            exit_state.stopped = true;
            exit_state.sender.take()
        };
        if let Some(exit) = exit {
            let _ = exit.send(());
        }
        self.inner.wg.Wait();
    }

    // Register adds a named pool to the resource manager.
    /// 按名称与组件类型注册一个 goroutine 池。
    pub fn Register(
        &self,
        pool: Arc<dyn GoroutinePool>,
        name: String,
        component: Component,
    ) -> Result<(), PoolMapError> {
        self.registerPool(
            name,
            PoolContainer {
                Pool: pool,
                Component: component,
            },
        )
    }

    /// 将池容器写入分片池映射。
    fn registerPool(&self, name: String, pool: PoolContainer) -> Result<(), PoolMapError> {
        self.inner
            .poolMap
            .read()
            .expect("resource manager pool map lock poisoned")
            .Add(name, pool)
    }

    // Unregister removes a named pool. Missing pools are ignored.
    /// 按名称注销池；不存在时忽略。
    pub fn Unregister<K>(&self, name: K)
    where
        K: AsRef<str>,
    {
        self.inner
            .poolMap
            .read()
            .expect("resource manager pool map lock poisoned")
            .Del(name);
    }

    // Reset replaces the pool map. It is intended for tests, as in Go.
    /// 用空的分片池映射替换当前映射（测试用）。
    pub fn Reset(&self) {
        *self
            .inner
            .poolMap
            .write()
            .expect("resource manager pool map lock poisoned") = Arc::new(NewShardPoolMap());
    }
}
