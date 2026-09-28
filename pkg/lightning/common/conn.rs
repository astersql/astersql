// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Lightning gRPC / 客户端连接池。
//
// 导入时需向多个 TiKV store 建立并发连接。本模块提供可关闭的 `ClientConn`、
// 固定容量轮询复用的 `ConnPool`，以及按 store ID 管理连接池的 `GRPCConns`。

use crate::{CommonError, Context};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 抽象客户端连接：记录目标地址与关闭状态。
#[derive(Debug)]
pub struct ClientConn {
    target: String,
    closed: AtomicBool,
}

impl ClientConn {
    /// 构造指向 `target` 的新连接句柄（尚未真正拨号时可作占位）。
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            closed: AtomicBool::new(false),
        }
    }

    /// 标记连接已关闭。
    pub fn Close(&self) -> Result<(), CommonError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }

    /// 返回连接目标地址。
    pub fn Target(&self) -> &str {
        &self.target
    }

    /// 查询连接是否已关闭。
    pub fn IsClosed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// 按上下文创建新 `ClientConn` 的工厂回调类型。
pub type ConnFactory = Arc<dyn Fn(&Context) -> Result<Arc<ClientConn>, CommonError> + Send + Sync>;

/// 固定容量连接池：未满时新建，满后按 `next` 轮询复用。
pub struct ConnPool {
    state: Mutex<ConnPoolState>,
    cap: usize,
    newConn: ConnFactory,
}

/// 连接池内部可变状态：已创建连接列表与轮询游标。
struct ConnPoolState {
    conns: Vec<Arc<ClientConn>>,
    next: usize,
}

impl ConnPool {
    /// 取出池中全部连接并重置轮询游标（转移所有权）。
    pub fn TakeConns(&self) -> Vec<Arc<ClientConn>> {
        let mut state = self.state.lock().expect("ConnPool mutex poisoned");
        state.next = 0;
        std::mem::take(&mut state.conns)
    }

    /// 关闭池中所有连接。
    pub fn Close(&self) {
        for conn in self.TakeConns() {
            let _ = conn.Close();
        }
    }

    /// 获取一条连接：容量未满则工厂新建并入池，否则轮询已有连接。
    fn get(&self, ctx: &Context) -> Result<Arc<ClientConn>, CommonError> {
        let mut state = self.state.lock().expect("ConnPool mutex poisoned");
        if state.conns.len() < self.cap {
            let conn = (self.newConn)(ctx)?;
            state.conns.push(Arc::clone(&conn));
            return Ok(conn);
        }
        let conn = Arc::clone(&state.conns[state.next]);
        state.next = (state.next + 1) % self.cap;
        Ok(conn)
    }
}

/// 创建容量为 `capacity` 的连接池。
///
/// 与 Go 的 `make(..., 0, capacity)` 一致，零容量可完成构造；首次取连接时会因池中
/// 没有可复用连接而失败。调用方应传入正容量。
pub fn NewConnPool(capacity: usize, newConn: ConnFactory) -> ConnPool {
    ConnPool {
        state: Mutex::new(ConnPoolState {
            conns: Vec::with_capacity(capacity),
            next: 0,
        }),
        cap: capacity,
        newConn,
    }
}

/// 按 TiKV store ID 管理多个 `ConnPool` 的 gRPC 连接表。
pub struct GRPCConns {
    conns: Mutex<HashMap<u64, Arc<ConnPool>>>,
}

impl GRPCConns {
    /// 关闭所有 store 下的连接池。
    pub fn Close(&self) {
        let pools: Vec<_> = self
            .conns
            .lock()
            .expect("GRPCConns mutex poisoned")
            .values()
            .cloned()
            .collect();
        for pool in pools {
            pool.Close();
        }
    }

    /// 获取指定 store 的一条 gRPC 连接；池不存在时按 `tcpConcurrency` 懒创建。
    pub fn GetGrpcConn(
        &self,
        ctx: &Context,
        storeID: u64,
        tcpConcurrency: usize,
        newConn: ConnFactory,
    ) -> Result<Arc<ClientConn>, CommonError> {
        // 先拿到/创建对应 store 的池，再在锁外从池中取连接，缩短临界区。
        let pool = {
            let mut conns = self.conns.lock().expect("GRPCConns mutex poisoned");
            Arc::clone(
                conns
                    .entry(storeID)
                    .or_insert_with(|| Arc::new(NewConnPool(tcpConcurrency, newConn))),
            )
        };
        pool.get(ctx)
    }
}

/// 创建空的 gRPC 连接表。
pub fn NewGRPCConns() -> GRPCConns {
    GRPCConns {
        conns: Mutex::new(HashMap::new()),
    }
}
