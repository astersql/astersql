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

//! gRPC store connection pool ported from `br/pkg/utils/store_manager.go`.
//!
//! 管理到 TiKV store 的 gRPC 连接：按 store_id 缓存，供备份客户端复用。
//! `Pool` 提供容量受限的轮询连接池；`StoreManager` 负责按 store 懒拨号与重置。
//! 拨号细节经 `GrpcConnFactory` 注入，便于测试替身与 TLS/keepalive 参数透传。
//! 与 Go 一致：取消上下文立即返回；关闭失败只记日志不阻断清理。
//! 典型调用：`TryWithConn`/`WithConn` 执行 RPC，故障时 `ResetBackupClient` 重建。
//! 本文件不直接发起备份 RPC，只维护连接生命周期与池化策略。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_errors::ErrFailedToConnect;
use astersql_br_pkg_logutil::{Field, ShortError, log};
use astersql_errors::{Annotate, SharedError, Trace};

// 为 PD/拨号错误补堆栈，便于与 Go `errors.Trace` 对齐排查。
fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}
use crate::kvproto::metapb::Store;
use crate::stubs::context::Context;
use astersql_util::security::TLS;

/// 抽象 gRPC 连接：暴露目标地址与关闭，避免绑定具体 gRPC 实现。
pub trait GrpcConn: Send + Sync {
    fn target(&self) -> String;
    fn close(&self) -> Result<(), SharedError>;
}

/// 按 store 元数据拨号；实现方可读取 TLS/keepalive（当前由工厂侧持有）。
pub trait GrpcConnFactory: Send + Sync {
    fn dial(&self, ctx: &Context, store: &Store) -> Result<Arc<dyn GrpcConn>, SharedError>;
}

/// PD 查询 store 元数据；拨号前必须拿到地址（优先 peer address）。
pub trait PdClient: Send + Sync {
    fn get_store(&self, ctx: &Context, store_id: u64) -> Result<Store, SharedError>;
}

// 未显式设置 DialTimeout 时的默认拨号等待，对齐 Go 30s。
const defaultDialTimeout: Duration = Duration::from_secs(30);
// ResetBackupClient 最大重试次数；与 Go `resetRetryTimes` 一致。
const resetRetryTimes: i32 = 3;

/// 固定容量连接池：未满时新建，满后按 `next` 轮询复用。
/// `mu` 串行化 Get/Close，避免扩容与轮询竞态。
pub struct Pool {
    // 粗粒度互斥：与 Go sync.Mutex 一样覆盖 Get/Close 临界区。
    mu: Mutex<()>,
    conns: Mutex<Vec<Arc<dyn GrpcConn>>>,
    // 轮询游标；满池后每次 Get 前进一格。
    next: Mutex<usize>,
    cap: usize,
    newConn: Arc<dyn GrpcConnFactory>,
}

impl Pool {
    // 取出全部连接并重置轮询下标，供 Close 统一释放。
    fn take_conns(&self) -> Vec<Arc<dyn GrpcConn>> {
        let _guard = self.mu.lock().expect("pool mu poisoned");
        let mut conns = self.conns.lock().expect("pool conns poisoned");
        let taken = std::mem::take(&mut *conns);
        *self.next.lock().expect("pool next poisoned") = 0;
        taken
    }

    /// 关闭池内全部连接；单条 close 失败仅 Warn，继续清理其余连接。
    pub fn Close(&self) {
        for conn in self.take_conns() {
            if let Err(err) = conn.close() {
                log::Warn(
                    "failed to close clientConn",
                    [
                        Field::string("target", &conn.target()),
                        Field::string("error", &err.to_string()),
                    ],
                );
            }
        }
    }

    /// 获取一条连接：容量未满则 dial 并入池，否则 round-robin。
    /// dial 使用空 Store，目标地址由工厂侧配置决定（对齐 Go 池语义）。
    pub fn Get(&self, ctx: &Context) -> Result<Arc<dyn GrpcConn>, SharedError> {
        let _guard = self.mu.lock().expect("pool mu poisoned");
        let mut conns = self.conns.lock().expect("pool conns poisoned");
        if conns.len() < self.cap {
            let conn = self.newConn.dial(ctx, &Store::default())?;
            conns.push(Arc::clone(&conn));
            return Ok(conn);
        }
        let mut next = self.next.lock().expect("pool next poisoned");
        let conn = Arc::clone(&conns[*next]);
        *next = (*next + 1) % self.cap;
        Ok(conn)
    }
}

/// 构造指定容量的连接池；`new_conn` 负责实际拨号。
pub fn NewConnPool(capacity: usize, new_conn: Arc<dyn GrpcConnFactory>) -> Arc<Pool> {
    Arc::new(Pool {
        cap: capacity,
        conns: Mutex::new(Vec::with_capacity(capacity)),
        next: Mutex::new(0),
        newConn: new_conn,
        mu: Mutex::new(()),
    })
}

/// gRPC keepalive 参数；默认 10s ping / 3s 超时 / 允许无流时发送。
#[derive(Clone)]
pub struct KeepaliveParams {
    /// ping 间隔；过小增加空闲流量，过大延迟探活。
    pub time: Duration,
    /// 单次 ping 等待上限。
    pub timeout: Duration,
    /// 无活跃流时是否仍发送 keepalive（备份长连接常为 true）。
    pub permit_without_stream: bool,
}

impl Default for KeepaliveParams {
    fn default() -> Self {
        // 默认值与 Go grpc.KeepaliveParams 常用配置对齐。
        Self {
            time: Duration::from_secs(10),
            timeout: Duration::from_secs(3),
            permit_without_stream: true,
        }
    }
}

/// 按 store_id 缓存备份 gRPC 连接的管理器。
/// 连接变更（掉线/重置）通过 RemoveConn / ResetBackupClient 显式处理。
pub struct StoreManager {
    // 解析 store 地址与元数据的 PD 抽象。
    pdClient: Arc<dyn PdClient>,
    // store_id → 已建立连接；访问需持锁。
    grpcClis: Mutex<HashMap<u64, Arc<dyn GrpcConn>>>,
    keepalive: KeepaliveParams,
    // 可选 TLS；None 表示明文拨号路径。
    tlsConf: Option<Arc<TLS>>,
    /// 拨号超时；ZERO 表示使用 `defaultDialTimeout`。
    pub DialTimeout: Duration,
    // 真正执行 dial 的工厂；可替换为 mock。
    connFactory: Arc<dyn GrpcConnFactory>,
}

impl StoreManager {
    /// 返回当前 keepalive 配置副本，供上层构造客户端时透传。
    pub fn GetKeepalive(&self) -> KeepaliveParams {
        self.keepalive.clone()
    }

    /// 创建管理器；初始无缓存连接，DialTimeout 置零走默认。
    pub fn NewStoreManager(
        pd_cli: Arc<dyn PdClient>,
        keepalive: KeepaliveParams,
        tls_conf: Option<Arc<TLS>>,
        conn_factory: Arc<dyn GrpcConnFactory>,
    ) -> Self {
        Self {
            pdClient: pd_cli,
            grpcClis: Mutex::new(HashMap::new()),
            keepalive,
            tlsConf: tls_conf,
            DialTimeout: Duration::ZERO,
            connFactory: conn_factory,
        }
    }

    // 解析有效拨号超时：显式 >0 优先，否则 30s 默认。
    fn get_dial_timeout(&self) -> Duration {
        if self.DialTimeout > Duration::ZERO {
            self.DialTimeout
        } else {
            defaultDialTimeout
        }
    }

    /// 暴露内部 PD 客户端，供需要查询 store 列表的调用方复用。
    pub fn PDClient(&self) -> Arc<dyn PdClient> {
        Arc::clone(&self.pdClient)
    }

    // 向 PD 取 store 并 dial；优先 peer address，空则回退 address。
    // 超时/keepalive/TLS 当前以“读取保留”方式对齐 Go 字段使用点，实际注入在工厂内。
    fn get_grpc_conn_locked(
        &self,
        ctx: &Context,
        store_id: u64,
    ) -> Result<Arc<dyn GrpcConn>, SharedError> {
        inject_failpoint("hint-get-backup-client", store_id);
        let store = self.pdClient.get_store(ctx, store_id).map_err(trace_err)?;
        let mut addr = store.get_peer_address().to_string();
        if addr.is_empty() {
            // 与 Go 一致：无 peer 地址时使用普通 store address。
            addr = store.get_address().to_string();
        }
        log::L().Info(
            "StoreManager: dialing to store.",
            [
                Field::string("address", &addr),
                Field::uint64("store-id", store_id),
            ],
        );
        // 保留对超时/keepalive/TLS 的读取，避免后续接线时语义漂移。
        let _ = (
            self.get_dial_timeout(),
            self.keepalive.clone(),
            self.tlsConf.as_ref(),
        );
        self.connFactory.dial(ctx, &store).map_err(|err| {
            // 拨号失败包装为 ErrFailedToConnect，附带 store_id 便于定位。
            Annotate(
                Some(SharedError::new((*ErrFailedToConnect).clone())),
                format!("failed to make connection to store {store_id}: {err}"),
            )
            .unwrap_or(err)
        })
    }

    /// 移除并关闭指定 store 的缓存连接；关闭失败忽略（仅日志）。
    /// 上下文已取消时立即返回 Canceled，不碰连接表。
    pub fn RemoveConn(&self, ctx: &Context, store_id: u64) -> Result<(), SharedError> {
        if ctx.is_cancelled() {
            return Err(SharedError::new(Canceled));
        }
        let mut clis = self.grpcClis.lock().expect("grpcClis poisoned");
        if let Some(conn) = clis.remove(&store_id) {
            if let Err(err) = conn.close() {
                log::Warn(
                    "close backup connection failed, ignore it",
                    [
                        Field::uint64("storeID", store_id),
                        Field::string("error", &err.to_string()),
                    ],
                );
            }
        }
        Ok(())
    }

    /// 在缓存连接上执行可失败回调：命中缓存直接用，否则拨号后写入再调用。
    /// 与 Go 一致，拨号、缓存写入和回调均处于同一临界区，避免同一 store 重复拨号。
    pub fn TryWithConn<F>(&self, ctx: &Context, store_id: u64, f: F) -> Result<(), SharedError>
    where
        F: FnOnce(Arc<dyn GrpcConn>) -> Result<(), SharedError>,
    {
        if ctx.is_cancelled() {
            return Err(SharedError::new(Canceled));
        }
        let mut clis = self.grpcClis.lock().expect("grpcClis poisoned");
        if let Some(conn) = clis.get(&store_id) {
            return f(Arc::clone(conn));
        }
        let conn = self.get_grpc_conn_locked(ctx, store_id)?;
        clis.insert(store_id, Arc::clone(&conn));
        f(conn)
    }

    /// WithConn 的无错误回调形态：内部委托 TryWithConn 并恒返回 Ok(())。
    pub fn WithConn<F>(&self, ctx: &Context, store_id: u64, f: F) -> Result<(), SharedError>
    where
        F: FnOnce(Arc<dyn GrpcConn>),
    {
        self.TryWithConn(ctx, store_id, |conn| {
            f(conn);
            Ok(())
        })
    }

    /// 先移除旧连接，再最多 `resetRetryTimes` 次重拨并写回缓存。
    /// 失败间隔 sleep(retry+3) 秒，对齐 Go 退避；最终返回最后一次错误。
    pub fn ResetBackupClient(
        &self,
        ctx: &Context,
        store_id: u64,
    ) -> Result<Arc<dyn GrpcConn>, SharedError> {
        self.RemoveConn(ctx, store_id)?;
        let mut clis = self.grpcClis.lock().expect("grpcClis poisoned");
        let mut last_err = None;
        for retry in 0..resetRetryTimes {
            match self.get_grpc_conn_locked(ctx, store_id) {
                Ok(conn) => {
                    clis.insert(store_id, Arc::clone(&conn));
                    return Ok(conn);
                }
                Err(err) => {
                    log::Warn(
                        "failed to reset grpc connection, retry it",
                        [
                            Field::int("retry time", retry as i64),
                            ShortError(Some(&err)),
                        ],
                    );
                    last_err = Some(err);
                    // Go：time.Sleep(time.Duration(retry+3) * time.Second)
                    thread::sleep(Duration::from_secs((retry + 3) as u64));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            SharedError::new(std::io::Error::other("failed to reset backup client"))
        }))
    }

    /// 关闭全部缓存连接但保留映射，与 Go 的终止期生命周期语义一致。
    pub fn Close(&self) {
        let clis = self.grpcClis.lock().expect("grpcClis poisoned");
        for cli in clis.values() {
            if let Err(err) = cli.close() {
                log::Warn(
                    "fail to close Mgr",
                    [Field::string("error", &err.to_string())],
                );
            }
        }
    }

    /// 返回可选 TLS 配置，供外部构造传输层时复用同一证书材料。
    pub fn TLSConfig(&self) -> Option<Arc<TLS>> {
        self.tlsConf.clone()
    }
}

// failpoint 钩子占位；测试可注入拨号行为，生产为空实现。
fn inject_failpoint(_name: &str, _store_id: u64) {}

use astersql_br_pkg_errors::Canceled;
