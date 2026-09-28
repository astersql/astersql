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

// 会话资源池：按容量复用可关闭的会话资源。
//
// 对应 Go pools；`Get` 优先取空闲资源，否则工厂新建；`Put` 在未满且未关闭时归还，
// 否则关闭。`Destroy` 直接关闭且不归还。

use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};

/// 可池化资源：可关闭，并能向下转型为具体类型。
/// Rust counterpart of `pools.Resource`.
pub trait Resource: Any + Send + Sync {
    /// 释放底层资源。
    fn close(&self);
    /// 供调用方 downcast 到具体资源类型。
    fn as_any(&self) -> &dyn Any;
}

/// 池中资源的共享句柄。
pub type PooledResource = Arc<dyn Resource>;
/// 池空时创建新资源的工厂函数。
pub type Factory = Arc<dyn Fn() -> Result<PooledResource> + Send + Sync>;
/// Get/Put/Destroy 可选钩子回调。
pub type ResourceCallback = Arc<dyn Fn(&PooledResource) + Send + Sync>;

/// 会话池接口：获取、归还与关闭。
pub trait SessionPool: Send + Sync {
    /// 取出一个资源；池已关闭则返回错误。
    fn Get(&self) -> Result<PooledResource>;
    /// 归还资源；已关闭或已满则关闭该资源。
    fn Put(&self, resource: PooledResource);
    /// 关闭池并关闭所有空闲资源。
    fn Close(&self);
}

/// 可销毁资源的会话池扩展。
pub trait DestroyableSessionPool: SessionPool {
    /// 销毁资源（回调后 close），不放回池中。
    fn Destroy(&self, resource: PooledResource);
}

/// 池内部可变状态：空闲队列与关闭标记。
struct PoolState {
    /// 空闲资源队列。
    resources: VecDeque<PooledResource>,
    /// 池是否已关闭。
    closed: bool,
}

/// 具体池实现：容量、工厂、状态与可选回调。
struct Pool {
    capacity: usize,
    factory: Factory,
    /// 受互斥锁保护的可变状态。
    state: Mutex<PoolState>,
    get_callback: Option<ResourceCallback>,
    put_callback: Option<ResourceCallback>,
    destroy_callback: Option<ResourceCallback>,
}

/// 创建可销毁的会话池。
pub fn NewSessionPool(
    capacity: usize,
    factory: Factory,
    get_callback: Option<ResourceCallback>,
    put_callback: Option<ResourceCallback>,
    destroy_callback: Option<ResourceCallback>,
) -> Arc<dyn DestroyableSessionPool> {
    Arc::new(Pool {
        capacity,
        factory,
        state: Mutex::new(PoolState {
            resources: VecDeque::with_capacity(capacity),
            closed: false,
        }),
        get_callback,
        put_callback,
        destroy_callback,
    })
}

impl SessionPool for Pool {
    fn Get(&self) -> Result<PooledResource> {
        // 先在锁内检查关闭态并尝试弹出空闲资源。
        let resource = {
            let mut state = self.state.lock().expect("session pool mutex poisoned");
            if state.closed {
                return Err(anyhow!("session pool closed"));
            }
            state.resources.pop_front()
        };

        // 无空闲资源则调用工厂新建。
        let resource = match resource {
            Some(resource) => resource,
            None => (self.factory)()?,
        };

        // failpoint：测试注入 Get 错误。
        fail::fail_point!("mockSessionPoolReturnError", |_| {
            return Err(anyhow!("mockSessionPoolReturnError"));
        });

        if let Some(callback) = &self.get_callback {
            callback(&resource);
        }
        Ok(resource)
    }

    fn Put(&self, resource: PooledResource) {
        if let Some(callback) = &self.put_callback {
            callback(&resource);
        }

        let mut state = self.state.lock().expect("session pool mutex poisoned");
        // 已关闭或已满：关闭资源而非入队。
        if state.closed || state.resources.len() >= self.capacity {
            drop(state);
            resource.close();
            return;
        }
        state.resources.push_back(resource);
    }

    fn Close(&self) {
        // 标记关闭并取出全部空闲资源，锁外逐个 close。
        let resources = {
            let mut state = self.state.lock().expect("session pool mutex poisoned");
            if state.closed {
                return;
            }
            state.closed = true;
            state.resources.drain(..).collect::<Vec<_>>()
        };
        for resource in resources {
            resource.close();
        }
    }
}

impl DestroyableSessionPool for Pool {
    fn Destroy(&self, resource: PooledResource) {
        if let Some(callback) = &self.destroy_callback {
            callback(&resource);
        }
        resource.close();
    }
}
