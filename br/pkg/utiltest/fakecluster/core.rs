// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! In-memory fake TiKV/PD cluster for BR streamhelper / utiltest harnesses.
//! Semantics match `br/pkg/utiltest/fakecluster/core.go`.
//! 内存假 TiKV/PD 集群：对齐 Go `fakecluster/core.go` 语义。
//! 提供 Region/Store/Cluster、flush 订阅流、checkpoint 推进与 GC safepoint。
//! FlushSimulator 控制 GetLastFlushTS 是否报 not flushed / epoch 不匹配。
//! trivialFlushStream 用 channel+Context 模拟 gRPC 订阅 Recv。
//! Cluster 支持 Split/Scatter/Transfer、TSO 分配与 RegionScan 重叠过滤。
//! 测试钩子 OnGetClient/OnClearCache/OnGetRegionCheckpoint 可注入失败。
//! LegacyRegionCheckpointRPCEnabled=0 时禁用旧 RPC，逼迫走 flush 订阅路径。

//! fakecluster 核心：为 BR 集成式单测构造可控的假集群拓扑。
//! 集中管理 store/region 状态，便于注入分裂与调度失败。
//! 供 utiltest 上层用例复用，避免每测重复搭脚手架。
//! 状态变更应可观测，方便断言客户端后续动作。
//! 对齐 Go fakecluster 意图，能力子集以实现测试为目标。
//! 符号索引补充 1：公开 API 的约束优先于内部实现细节。
//! 数据流补充 2：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 3：空输入、取消上下文、未知枚举值都应按 Go 方式处理。

use std::collections::HashMap;
// 原子字段保存 epoch/checkpoint/TSO。
use std::sync::atomic::{AtomicU64, Ordering};
// 有界通道模拟 flush 订阅流。
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
// Arc 共享 Region/Store；Mutex 保护可变集合。
use std::sync::{Arc, Mutex};
// 后台推送/等待辅助（若使用）。
use std::thread;
// Recv 超时轮询间隔。
use std::time::Duration;

// streamhelper 类型用于 RegionScan/Stores 返回值。
use astersql_br_pkg_streamhelper as streamhelper;
// 键区间重叠与比较。
use astersql_br_pkg_streamhelper_spans as spans;
// Scatter 随机选 store。
use rand::Rng;
use rand::seq::SliceRandom;
// debug/info 日志。
use tracing::{debug, info};

use crate::stubs::{
    Code, Context, Error, ErrorPb, FlushEvent, FlushNowRequest, FlushNowResponse, FlushResult,
    GetLastFlushTSOfRegionRequest, GetLastFlushTSOfRegionResponse, KeyRange, Lock,
    RegionCheckpoint, RegionIdentity, Result, SubscribeFlushEventRequest,
    SubscribeFlushEventResponse, codec, oracle, status_error,
};

/// FlushSimulator records whether flush simulation is enabled and the flushed epoch.
/// `FlushSimulator`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `FlushSimulator` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct FlushSimulator {
    /// 已成功 flush 的 epoch；0 表示尚未 flush。
    pub FlushedEpoch: AtomicU64,
    /// 是否启用 flush 仿真校验。
    pub Enabled: bool,
}

/// `FlushSimulator` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `FlushSimulator` 方法边界：非法参数应返回可分类错误而非 panic。
impl FlushSimulator {
    /// makeError returns an errorpb-style error when flush simulation fails the request.
    /// `makeError`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `makeError` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn makeError(&self, requestedEpoch: u64) -> Option<ErrorPb> {
        // 未启用仿真则不注入错误。
        if !self.Enabled {
            return None;
        }
        // 尚未 flush：返回 not flushed。
        if self.FlushedEpoch.load(Ordering::SeqCst) == 0 {
            return Some(ErrorPb {
                Message: "not flushed".to_string(),
            });
        }
        // epoch 不一致：返回 flushed epoch not match。
        if self.FlushedEpoch.load(Ordering::SeqCst) != requestedEpoch {
            return Some(ErrorPb {
                Message: "flushed epoch not match".to_string(),
            });
        }
        None
    }

    /// fork inherits Enabled only (not FlushedEpoch), matching Go.
    /// `fork`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `fork` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn fork(&self) -> FlushSimulator {
        FlushSimulator {
            FlushedEpoch: AtomicU64::new(0),
            Enabled: self.Enabled,
        }
    }
}

/// Region is a shared in-memory region (Go `*Region`).
/// `Region`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `Region` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct Region {
    /// 可变键范围。
    pub Range: Mutex<KeyRange>,
    /// 当前 leader store ID。
    pub Leader: AtomicU64,
    /// region epoch。
    pub Epoch: AtomicU64,
    /// region 稳定 ID。
    pub ID: u64,
    /// region checkpoint TS。
    pub Checkpoint: AtomicU64,
    /// 刷盘仿真器。
    pub FlushSim: FlushSimulator,
    /// 注入的事务锁列表。
    pub Locks: Mutex<Vec<Lock>>,
}

/// NewRegion clones start/end keys and initializes checkpoint.
/// `NewRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewRegion(
    id: u64,
    startKey: Vec<u8>,
    endKey: Vec<u8>,
    leader: u64,
    epoch: u64,
    checkpoint: u64,
    flushSimEnabled: bool,
) -> Arc<Region> {
    let r = Arc::new(Region {
        Range: Mutex::new(KeyRange {
            StartKey: startKey,
            EndKey: endKey,
        }),
        Leader: AtomicU64::new(leader),
        Epoch: AtomicU64::new(epoch),
        ID: id,
        Checkpoint: AtomicU64::new(checkpoint),
        FlushSim: FlushSimulator {
            FlushedEpoch: AtomicU64::new(0),
            Enabled: flushSimEnabled,
        },
        Locks: Mutex::new(Vec::new()),
    });
    r
}

/// `Region` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Region` 方法边界：非法参数应返回可分类错误而非 panic。
impl Region {
    /// SplitAt copies the right half into a new region and advances epochs.
    ///
    /// Go: `newRegion.Epoch = r.Epoch + 1`, then `r.Epoch++` and both flush sims fork
    /// (Enabled kept, FlushedEpoch reset to 0). After return both epochs equal old+1.
    /// `SplitAt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitAt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SplitAt(self: &Arc<Self>, newID: u64, key: &str) -> Arc<Region> {
        let key_bytes = key.as_bytes().to_vec();
        let end_key = {
            let mut range = self.Range.lock().unwrap();
            let end = range.EndKey.clone();
            range.EndKey = key_bytes.clone();
            end
        };
        let new_epoch = self.Epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let new_region = Arc::new(Region {
            Range: Mutex::new(KeyRange {
                StartKey: key_bytes,
                EndKey: end_key,
            }),
            Leader: AtomicU64::new(self.Leader.load(Ordering::SeqCst)),
            Epoch: AtomicU64::new(new_epoch),
            ID: newID,
            Checkpoint: AtomicU64::new(self.Checkpoint.load(Ordering::SeqCst)),
            FlushSim: self.FlushSim.fork(),
            Locks: Mutex::new(Vec::new()),
        });
        // 左段重置 FlushedEpoch，与 Go fork 语义一致。
        // r.FlushSim = r.FlushSim.fork()
        self.FlushSim.FlushedEpoch.store(0, Ordering::SeqCst);
        new_region
    }

    /// Flush marks the current epoch as flushed.
    /// `Flush`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Flush` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Flush(&self) {
        self.FlushSim
            .FlushedEpoch
            .store(self.Epoch.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    /// `start_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `start_key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn start_key(&self) -> Vec<u8> {
        self.Range.lock().unwrap().StartKey.clone()
    }

    /// `end_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `end_key` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn end_key(&self) -> Vec<u8> {
        self.Range.lock().unwrap().EndKey.clone()
    }
}

/// `Region` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Region` 方法边界：非法参数应返回可分类错误而非 panic。
impl std::fmt::Display for Region {
    /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `fmt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let range = self.Range.lock().unwrap();
        write!(
            f,
            "{}({}):[{}, {});{}L{}F{}",
            self.ID,
            self.Epoch.load(Ordering::SeqCst),
            hex::encode(&range.StartKey),
            hex::encode(&range.EndKey),
            self.Checkpoint.load(Ordering::SeqCst),
            self.Leader.load(Ordering::SeqCst),
            self.FlushSim.FlushedEpoch.load(Ordering::SeqCst)
        )
    }
}

// tiny hex helper without extra crate
/// `hex`：子模块，聚合相关桩类型与辅助函数。
mod hex {
    /// `encode`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `encode` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn encode(data: &[u8]) -> String {
        /// `HEX`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
        /// 调整阈值前确认是否影响重试次数或批大小语义。
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(data.len() * 2);
        for &b in data {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0xf) as usize] as char);
        }
        out
    }
}

/// trivialFlushStream wraps a response channel and context (Go gRPC stream stand-in).
/// `trivialFlushStream`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `trivialFlushStream` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct trivialFlushStream {
    c: Receiver<SubscribeFlushEventResponse>,
    cx: Context,
}

/// `trivialFlushStream` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `trivialFlushStream` 方法边界：非法参数应返回可分类错误而非 panic。
impl trivialFlushStream {
    /// Recv returns the next event, EOF when closed, or Canceled when ctx is done.
    /// `Recv`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Recv` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Recv(&self) -> Result<SubscribeFlushEventResponse> {
        loop {
            // 已取消：尽量排空通道，否则返回 Canceled。
            if self.cx.is_done() {
                match self.c.try_recv() {
                    Ok(item) => return Ok(item),
                    Err(TryRecvError::Empty) => {
                        let msg = self
                            .cx
                            .err_message()
                            .unwrap_or_else(|| "context canceled".to_string());
                        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                        return Err(status_error(Code::Canceled, msg).into());
                    }
                    Err(TryRecvError::Disconnected) => {
                        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                        return Err(Error::new("EOF"));
                    }
                }
            }
            // 10ms 轮询，兼顾取消响应与事件到达。
            match self.c.recv_timeout(Duration::from_millis(10)) {
                Ok(item) => return Ok(item),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(Error::new("EOF")),
            }
        }
    }

    /// `Header`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Header` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Header(&self) -> Result<HashMap<String, String>> {
        Ok(HashMap::new())
    }

    /// `Trailer`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Trailer` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Trailer(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    /// `CloseSend`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CloseSend` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CloseSend(&self) -> Result<()> {
        Ok(())
    }

    /// `Context`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Context` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Context(&self) -> Context {
        self.cx.clone()
    }

    /// `SendMsg`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SendMsg` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SendMsg(&self, _msg: ()) -> Result<()> {
        Ok(())
    }

    /// `RecvMsg`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RecvMsg` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn RecvMsg(&self, _msg: &mut ()) -> Result<()> {
        Ok(())
    }
}

/// `StoreClientState`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `StoreClientState` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct StoreClientState {
    // 是否支持 SubscribeFlushEvent。
    supports_sub: bool,
    // bootstrap 代数，切换支持时递增。
    bootstrap_at: u64,
    // sub_id 到发送端映射。
    subscribers: HashMap<u64, SyncSender<SubscribeFlushEventResponse>>,
    // 分配订阅 ID。
    next_sub_id: u64,
}

/// Store is one TiKV store view of regions plus flush subscription state.
/// `Store`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `Store` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct Store {
    pub ID: u64,
    /// 本 store 持有的 region。
    pub RegionMap: Mutex<HashMap<u64, Arc<Region>>>,
    // 订阅客户端状态。
    client: Mutex<StoreClientState>,
    /// 查询前置 hook，可注入错误。
    pub OnGetRegionCheckpoint:
        Mutex<Option<Box<dyn Fn(&GetLastFlushTSOfRegionRequest) -> Result<()> + Send + Sync>>>,
    /// 0 禁用旧 GetLastFlushTS RPC。
    pub LegacyRegionCheckpointRPCEnabled: AtomicU64, // 0/1 bool
    /// flush 任务名（如 drr）。
    pub FlushTaskName: Mutex<String>,
}

/// `Store` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Store` 方法边界：非法参数应返回可分类错误而非 panic。
impl Store {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `new` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn new(id: u64) -> Arc<Self> {
        Arc::new(Self {
            ID: id,
            RegionMap: Mutex::new(HashMap::new()),
            client: Mutex::new(StoreClientState {
                supports_sub: false,
                bootstrap_at: 0,
                subscribers: HashMap::new(),
                next_sub_id: 1,
            }),
            OnGetRegionCheckpoint: Mutex::new(None),
            LegacyRegionCheckpointRPCEnabled: AtomicU64::new(1),
            FlushTaskName: Mutex::new(String::new()),
        })
    }

    /// `BootstrapAt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `BootstrapAt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn BootstrapAt(&self) -> u64 {
        self.client.lock().unwrap().bootstrap_at
    }

    /// `SupportsSub`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SupportsSub` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SupportsSub(&self) -> bool {
        self.client.lock().unwrap().supports_sub
    }

    /// FlushNow immediately flushes and returns a successful FlushResult.
    /// `FlushNow`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FlushNow` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FlushNow(&self, _ctx: &Context, _in: &FlushNowRequest) -> Result<FlushNowResponse> {
        self.Flush();
        let task_name = {
            let name = self.FlushTaskName.lock().unwrap();
            if name.is_empty() {
                "Universe".to_string()
            } else {
                name.clone()
            }
        };
        Ok(FlushNowResponse {
            Results: vec![FlushResult {
                TaskName: task_name,
                Success: true,
            }],
        })
    }

    /// `GetID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetID(&self) -> u64 {
        self.ID
    }

    /// SubscribeFlushEvent registers a subscriber; unregisters when ctx is cancelled.
    /// `SubscribeFlushEvent`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SubscribeFlushEvent` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SubscribeFlushEvent(
        self: &Arc<Self>,
        ctx: Context,
        _in: &SubscribeFlushEventRequest,
    ) -> Result<trivialFlushStream> {
        let (rx, sub_id) = {
            let mut client = self.client.lock().unwrap();
            if !client.supports_sub {
                // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                return Err(status_error(Code::Unimplemented, "meow?").into());
            }
            let (tx, rx) = mpsc::sync_channel(1024);
            let id = client.next_sub_id;
            client.next_sub_id += 1;
            client.subscribers.insert(id, tx);
            (rx, id)
        };

        let store = Arc::clone(self);
        let ctx_watch = ctx.clone();
        thread::spawn(move || {
            ctx_watch.wait_cancelled();
            let mut client = store.client.lock().unwrap();
            client.subscribers.remove(&sub_id);
        });

        Ok(trivialFlushStream { c: rx, cx: ctx })
    }

    /// `SetSupportFlushSub`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetSupportFlushSub` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetSupportFlushSub(&self, b: bool) {
        let mut client = self.client.lock().unwrap();
        client.bootstrap_at += 1;
        client.supports_sub = b;
    }

    /// `SetGetRegionCheckpointHook`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetGetRegionCheckpointHook` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetGetRegionCheckpointHook(
        &self,
        hook: impl Fn(&GetLastFlushTSOfRegionRequest) -> Result<()> + Send + Sync + 'static,
    ) {
        *self.OnGetRegionCheckpoint.lock().unwrap() = Some(Box::new(hook));
    }

    /// GetLastFlushTSOfRegion returns per-region checkpoints or region errors.
    /// `GetLastFlushTSOfRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetLastFlushTSOfRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetLastFlushTSOfRegion(
        &self,
        _ctx: &Context,
        input: &GetLastFlushTSOfRegionRequest,
    ) -> Result<GetLastFlushTSOfRegionResponse> {
        // DRR 路径禁用 legacy RPC。
        if self.LegacyRegionCheckpointRPCEnabled.load(Ordering::SeqCst) == 0 {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(status_error(
                Code::Unimplemented,
                "GetLastFlushTSOfRegion is legacy and disabled in DRR harness",
            )
            .into());
        }
        if let Some(hook) = self.OnGetRegionCheckpoint.lock().unwrap().as_ref() {
            hook(input)?;
        }
        let mut resp = GetLastFlushTSOfRegionResponse {
            Checkpoints: Vec::new(),
        };
        let region_map = self.RegionMap.lock().unwrap();
        for r in &input.Regions {
            let region = region_map.get(&r.Id);
            // 非本 store leader：not found。
            if region.is_none() || region.unwrap().Leader.load(Ordering::SeqCst) != self.ID {
                resp.Checkpoints.push(RegionCheckpoint {
                    Err: Some(ErrorPb {
                        Message: "not found".to_string(),
                    }),
                    Region: Some(RegionIdentity {
                        Id: r.Id,
                        EpochVersion: r.EpochVersion,
                    }),
                    Checkpoint: 0,
                });
                continue;
            }
            let region = region.unwrap();
            if let Some(err) = region.FlushSim.makeError(r.EpochVersion) {
                resp.Checkpoints.push(RegionCheckpoint {
                    Err: Some(err),
                    Region: Some(RegionIdentity {
                        Id: region.ID,
                        EpochVersion: region.Epoch.load(Ordering::SeqCst),
                    }),
                    Checkpoint: 0,
                });
                continue;
            }
            // 请求 epoch 过期。
            if region.Epoch.load(Ordering::SeqCst) != r.EpochVersion {
                resp.Checkpoints.push(RegionCheckpoint {
                    Err: Some(ErrorPb {
                        Message: "epoch not match".to_string(),
                    }),
                    Region: Some(RegionIdentity {
                        Id: region.ID,
                        EpochVersion: region.Epoch.load(Ordering::SeqCst),
                    }),
                    Checkpoint: 0,
                });
                continue;
            }
            resp.Checkpoints.push(RegionCheckpoint {
                Err: None,
                Region: Some(RegionIdentity {
                    Id: region.ID,
                    EpochVersion: region.Epoch.load(Ordering::SeqCst),
                }),
                Checkpoint: region.Checkpoint.load(Ordering::SeqCst),
            });
        }
        debug!(
            target: "fakecluster",
            "Get last flush ts of region regions={} out={}",
            input.Regions.len(),
            resp.Checkpoints.len()
        );
        Ok(resp)
    }

    /// `emitFlushEvents`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `emitFlushEvents` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn emitFlushEvents(&self, resp: &SubscribeFlushEventResponse) {
        let client = self.client.lock().unwrap();
        for ch in client.subscribers.values() {
            match ch.try_send(resp.clone()) {
                // 满缓冲丢弃，避免阻塞 flush 路径。
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    /// FlushExcept flushes leader regions except those containing excluded keys.
    /// `FlushExcept`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FlushExcept` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FlushExcept(&self, keys: &[&str]) {
        let mut events = Vec::new();
        let region_map = self.RegionMap.lock().unwrap();
        'outer: for r in region_map.values() {
            // 非 leader 跳过。
            if r.Leader.load(Ordering::SeqCst) != self.ID {
                continue;
            }
            let range = r.Range.lock().unwrap();
            for key in keys {
                let key_bytes = key.as_bytes();
                if spans::CompareBytesExt(&range.StartKey, false, key_bytes, false) <= 0
                    && spans::CompareBytesExt(key_bytes, false, &range.EndKey, true) < 0
                {
                    // 键落在区间内：排除该 region。
                    continue 'outer;
                }
            }
            drop(range);
            r.Flush();
            events.push(FlushEvent {
                // 事件键使用 TiKV 编码形态。
                StartKey: codec::EncodeBytes(Vec::new(), &r.start_key()),
                EndKey: codec::EncodeBytes(Vec::new(), &r.end_key()),
                Checkpoint: r.Checkpoint.load(Ordering::SeqCst),
            });
        }
        drop(region_map);
        self.emitFlushEvents(&SubscribeFlushEventResponse { Events: events });
    }

    /// `Flush`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Flush` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Flush(&self) {
        self.FlushExcept(&[]);
    }

    /// Test/helper: number of active flush subscribers.
    /// `subscriber_count`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `subscriber_count` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn subscriber_count(&self) -> usize {
        self.client.lock().unwrap().subscribers.len()
    }
}

/// `Store` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Store` 方法边界：非法参数应返回可分类错误而非 panic。
impl std::fmt::Display for Store {
    /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `fmt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.ID)?;
        let region_map = self.RegionMap.lock().unwrap();
        for r in region_map.values() {
            write!(f, "{r} ")?;
        }
        Ok(())
    }
}

/// RegionState is a read-only snapshot of a region.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `RegionState`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `RegionState` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct RegionState {
    pub ID: u64,
    pub Epoch: u64,
    pub StoreID: u64,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub Checkpoint: u64,
}

/// `stateFromRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `stateFromRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn stateFromRegion(r: &Region) -> RegionState {
    let range = r.Range.lock().unwrap();
    RegionState {
        ID: r.ID,
        Epoch: r.Epoch.load(Ordering::SeqCst),
        StoreID: r.Leader.load(Ordering::SeqCst),
        StartKey: range.StartKey.clone(),
        EndKey: range.EndKey.clone(),
        Checkpoint: r.Checkpoint.load(Ordering::SeqCst),
    }
}

/// Cluster is the in-memory fake cluster.
/// `Cluster`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `Cluster` 生命周期：构造后是否可变、是否跨线程共享需明确。
pub struct Cluster {
    // 集群级粗锁，保护 TSO/分裂散射临界区。
    mu: Mutex<()>,
    /// 已分配 ID 高水位。
    pub IDAlloced: AtomicU64,
    /// store ID 到 Store。
    pub StoreMap: Mutex<HashMap<u64, Arc<Store>>>,
    /// 全局 region 列表。
    pub Regions: Mutex<Vec<Arc<Region>>>,
    /// 观测到的最大 TS（若使用）。
    pub MaxTS: AtomicU64,
    // GetLogBackupClient 钩子。
    OnGetClient: Mutex<Option<Box<dyn Fn(u64) -> Result<()> + Send + Sync>>>,
    // ClearCache 钩子。
    OnClearCache: Mutex<Option<Box<dyn Fn(u64) -> Result<()> + Send + Sync>>>,
    /// 服务级 GC safepoint。
    pub ServiceGCSafePoint: AtomicU64,
    /// 是否已设置 safepoint。
    pub ServiceGCSafePointSet: AtomicU64,
    /// UnblockGC 后置位。
    pub ServiceGCSafePointDeleted: AtomicU64,
    // 集群当前 TSO。
    CurrentTS: AtomicU64,
}

/// New creates an empty cluster.
/// `New`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `New` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn New() -> Cluster {
    Cluster {
        mu: Mutex::new(()),
        IDAlloced: AtomicU64::new(0),
        StoreMap: Mutex::new(HashMap::new()),
        Regions: Mutex::new(Vec::new()),
        MaxTS: AtomicU64::new(0),
        OnGetClient: Mutex::new(None),
        OnClearCache: Mutex::new(None),
        ServiceGCSafePoint: AtomicU64::new(0),
        ServiceGCSafePointSet: AtomicU64::new(0),
        ServiceGCSafePointDeleted: AtomicU64::new(0),
        CurrentTS: AtomicU64::new(0),
    }
}

/// NewBasicCluster creates `n` stores and one full-range initial region.
/// `NewBasicCluster`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `NewBasicCluster` 数据流：调用方准备输入，本函数产出可断言结果或错误。
pub fn NewBasicCluster(n: usize, simEnabled: bool) -> Cluster {
    let c = New();
    let mut stores = Vec::with_capacity(n);
    for _ in 0..n {
        let s = Store::new(c.AllocID());
        stores.push(s);
    }
    let initial = NewRegion(
        c.AllocID(),
        Vec::new(),
        Vec::new(),
        stores[0].ID,
        0,
        0,
        simEnabled,
    );
    // 初始 region 复制到最多前 3 个 store（对齐 Go）。
    for i in 0..3 {
        if i < stores.len() {
            stores[i]
                .RegionMap
                .lock()
                .unwrap()
                .insert(initial.ID, Arc::clone(&initial));
        }
    }
    {
        let mut map = c.StoreMap.lock().unwrap();
        for s in stores {
            map.insert(s.ID, s);
        }
    }
    c.Regions.lock().unwrap().push(initial);
    c
}

/// `Cluster` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Cluster` 方法边界：非法参数应返回可分类错误而非 panic。
impl Cluster {
    /// `AllocID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AllocID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AllocID(&self) -> u64 {
        self.IDAlloced.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// `EnsureStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `EnsureStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn EnsureStore(&self, id: u64, bootstrapAt: u64) -> Arc<Store> {
        let mut map = self.StoreMap.lock().unwrap();
        if let Some(s) = map.get(&id) {
            return Arc::clone(s);
        }
        let alloced = self.IDAlloced.load(Ordering::SeqCst);
        // 外部指定更大 ID 时抬高分配器。
        if id > alloced {
            self.IDAlloced.store(id, Ordering::SeqCst);
        }
        let store = Store::new(id);
        store.client.lock().unwrap().bootstrap_at = bootstrapAt;
        map.insert(id, Arc::clone(&store));
        store
    }

    /// `AddRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AddRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AddRegion(&self, region: Arc<Region>, peerStoreIDs: &[u64]) {
        if region.ID > self.IDAlloced.load(Ordering::SeqCst) {
            self.IDAlloced.store(region.ID, Ordering::SeqCst);
        }
        self.Regions.lock().unwrap().push(Arc::clone(&region));
        for &store_id in peerStoreIDs {
            let store = self.EnsureStore(store_id, 0);
            store
                .RegionMap
                .lock()
                .unwrap()
                .insert(region.ID, Arc::clone(&region));
        }
    }

    /// `SetOnGetClient`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub fn SetOnGetClient(&self, hook: impl Fn(u64) -> Result<()> + Send + Sync + 'static) {
        *self.OnGetClient.lock().unwrap() = Some(Box::new(hook));
    }

    /// `SetOnClearCache`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    /// 调整阈值前确认是否影响重试次数或批大小语义。
    pub fn SetOnClearCache(&self, hook: impl Fn(u64) -> Result<()> + Send + Sync + 'static) {
        *self.OnClearCache.lock().unwrap() = Some(Box::new(hook));
    }

    /// `StoreList`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `StoreList` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn StoreList(&self) -> Vec<Arc<Store>> {
        let map = self.StoreMap.lock().unwrap();
        let mut result: Vec<_> = map.values().cloned().collect();
        result.sort_by_key(|s| s.ID);
        result
    }

    /// `StoreIDs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `StoreIDs` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn StoreIDs(&self) -> Vec<u64> {
        let map = self.StoreMap.lock().unwrap();
        let mut result: Vec<_> = map.keys().copied().collect();
        result.sort();
        result
    }

    /// `RegionList`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RegionList` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn RegionList(&self) -> Vec<Arc<Region>> {
        let regions = self.Regions.lock().unwrap();
        let mut result = regions.clone();
        result.sort_by(|a, b| a.start_key().cmp(&b.start_key()));
        result
    }

    /// `CurrentTSO`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `CurrentTSO` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn CurrentTSO(&self) -> u64 {
        let _g = self.mu.lock().unwrap();
        self.CurrentTS.load(Ordering::SeqCst)
    }

    /// `SetCurrentTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetCurrentTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetCurrentTS(&self, ts: u64) {
        let _g = self.mu.lock().unwrap();
        self.CurrentTS.store(ts, Ordering::SeqCst);
    }

    /// `AllocTSO`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AllocTSO` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AllocTSO(&self) -> u64 {
        let _g = self.mu.lock().unwrap();
        let physical = oracle::ExtractPhysical(self.CurrentTS.load(Ordering::SeqCst)) + 1;
        let ts = oracle::ComposeTS(physical, 0);
        self.CurrentTS.store(ts, Ordering::SeqCst);
        ts
    }

    /// `BlockGCUntil`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `BlockGCUntil` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn BlockGCUntil(&self, _ctx: &Context, at: u64) -> Result<u64> {
        let _g = self.mu.lock().unwrap();
        let sp = self.ServiceGCSafePoint.load(Ordering::SeqCst);
        // safepoint 不可回退。
        if sp > at {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            return Err(Error::new(format!(
                "minimal safe point {sp} is greater than the target {at}"
            )));
        }
        self.ServiceGCSafePoint.store(at, Ordering::SeqCst);
        self.ServiceGCSafePointSet.store(1, Ordering::SeqCst);
        Ok(at)
    }

    /// `UnblockGC`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `UnblockGC` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn UnblockGC(&self, _ctx: &Context) -> Result<()> {
        let _g = self.mu.lock().unwrap();
        self.ServiceGCSafePointDeleted.store(1, Ordering::SeqCst);
        Ok(())
    }

    /// `FetchCurrentTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FetchCurrentTS` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FetchCurrentTS(&self, _ctx: &Context) -> Result<u64> {
        Ok(self.CurrentTSO())
    }

    /// `RegionScan`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RegionScan` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn RegionScan(
        &self,
        _ctx: &Context,
        key: &[u8],
        endKey: &[u8],
        limit: usize,
    ) -> Result<Vec<streamhelper::RegionWithLeader>> {
        let regions = self.RegionList();
        let mut result = Vec::with_capacity(limit);
        for region in regions {
            let range = region.Range.lock().unwrap();
            let span_a = spans::Span {
                StartKey: key.to_vec(),
                EndKey: endKey.to_vec(),
            };
            let span_b = spans::Span {
                StartKey: range.StartKey.clone(),
                EndKey: range.EndKey.clone(),
            };
            // 与查询区间重叠才纳入结果。
            let overlaps = spans::Overlaps(&span_a, &span_b);
            let start_cmp = range.StartKey.as_slice().cmp(key);
            if overlaps && result.len() < limit {
                result.push(streamhelper::RegionWithLeader {
                    Region: streamhelper::Region {
                        Id: region.ID,
                        StartKey: range.StartKey.clone(),
                        EndKey: range.EndKey.clone(),
                        RegionEpoch: streamhelper::RegionEpoch {
                            Version: region.Epoch.load(Ordering::SeqCst),
                            ConfVer: 0,
                        },
                    },
                    Leader: streamhelper::Peer {
                        Id: 0,
                        StoreId: region.Leader.load(Ordering::SeqCst),
                    },
                });
            // region 已越过查询起点且有序，可提前结束。
            } else if start_cmp == std::cmp::Ordering::Greater {
                break;
            }
        }
        Ok(result)
    }

    /// `GetLogBackupClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetLogBackupClient` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn GetLogBackupClient(&self, _ctx: &Context, storeID: u64) -> Result<Arc<Store>> {
        if let Some(hook) = self.OnGetClient.lock().unwrap().as_ref() {
            hook(storeID)?;
        }
        let map = self.StoreMap.lock().unwrap();
        map.get(&storeID)
            .cloned()
            .ok_or_else(|| Error::new(format!("the store {storeID} doesn't exist")))
    }

    /// `ClearCache`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ClearCache` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ClearCache(&self, _ctx: &Context, storeID: u64) -> Result<()> {
        if let Some(hook) = self.OnClearCache.lock().unwrap().as_ref() {
            hook(storeID)?;
        }
        Ok(())
    }

    /// `Stores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `Stores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn Stores(&self, _ctx: &Context) -> Result<Vec<streamhelper::Store>> {
        let map = self.StoreMap.lock().unwrap();
        let mut r = Vec::with_capacity(map.len());
        for (id, s) in map.iter() {
            r.push(streamhelper::Store {
                ID: *id,
                BootAt: s.BootstrapAt(),
            });
        }
        r.sort_by_key(|s| s.ID);
        Ok(r)
    }

    /// `FindRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FindRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FindRegionByID(&self, rid: u64) -> Option<Arc<Region>> {
        self.Regions
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.ID == rid)
            .cloned()
    }

    /// `LockRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `LockRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn LockRegion(&self, r: &Arc<Region>, locks: Vec<Lock>) -> Arc<Region> {
        *r.Locks.lock().unwrap() = locks;
        Arc::clone(r)
    }

    /// `FindRegionByKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FindRegionByKey` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FindRegionByKey(&self, key: &[u8]) -> Arc<Region> {
        for r in self.Regions.lock().unwrap().iter() {
            let range = r.Range.lock().unwrap();
            if key >= range.StartKey.as_slice()
                && (range.EndKey.is_empty() || key < range.EndKey.as_slice())
            {
                return Arc::clone(r);
            }
        }
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        panic!("inconsistent key space; key = {key:X?}");
    }

    /// `TransferRegionTo`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `TransferRegionTo` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn TransferRegionTo(&self, rid: u64, newPeers: &[u64]) {
        let r = self.FindRegionByID(rid);
        let map = self.StoreMap.lock().unwrap();
        for store in map.values() {
            let mut region_map = store.RegionMap.lock().unwrap();
            if newPeers.iter().any(|pid| *pid == store.ID) {
                if let Some(ref region) = r {
                    region_map.insert(rid, Arc::clone(region));
                }
            } else {
                region_map.remove(&rid);
            }
        }
    }

    /// `SetRegionLeader`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetRegionLeader` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetRegionLeader(&self, rid: u64, leader: u64) {
        if let Some(r) = self.FindRegionByID(rid) {
            r.Leader.store(leader, Ordering::SeqCst);
        }
    }

    /// `SetRegionCheckpoint`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetRegionCheckpoint` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SetRegionCheckpoint(&self, rid: u64, checkpoint: u64) {
        if let Some(r) = self.FindRegionByID(rid) {
            r.Checkpoint.store(checkpoint, Ordering::SeqCst);
        }
    }

    /// `BumpRegionEpoch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `BumpRegionEpoch` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn BumpRegionEpoch(&self, rid: u64) {
        if let Some(r) = self.FindRegionByID(rid) {
            r.Epoch.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// `SplitAt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitAt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SplitAt(&self, key: &str) {
        let k = key.as_bytes();
        let r = self.FindRegionByKey(k);
        let new_id = self.AllocID();
        let new_region = r.SplitAt(new_id, key);
        let map = self.StoreMap.lock().unwrap();
        for store in map.values() {
            let mut region_map = store.RegionMap.lock().unwrap();
            if region_map.contains_key(&r.ID) {
                region_map.insert(new_region.ID, Arc::clone(&new_region));
            }
        }
        drop(map);
        self.Regions.lock().unwrap().push(new_region);
    }

    /// `chooseStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `chooseStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn chooseStores(&self, n: usize) -> Vec<u64> {
        let map = self.StoreMap.lock().unwrap();
        let mut s: Vec<u64> = map.keys().copied().collect();
        // 随机打乱以模拟散射。
        s.shuffle(&mut rand::thread_rng());
        // Go returns s[:n], which panics when the cluster has too few stores.
        s[..n].to_vec()
    }

    /// `FindPeers`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FindPeers` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FindPeers(&self, rid: u64) -> Vec<u64> {
        let mut result = Vec::new();
        let map = self.StoreMap.lock().unwrap();
        for store in map.values() {
            if store.RegionMap.lock().unwrap().contains_key(&rid) {
                result.push(store.ID);
            }
        }
        result
    }

    /// `shuffleLeader`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `shuffleLeader` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn shuffleLeader(&self, rid: u64) {
        let mut peers = self.FindPeers(rid);
        peers.shuffle(&mut rand::thread_rng());
        if let Some(r) = self.FindRegionByID(rid) {
            r.Leader.store(peers[0], Ordering::SeqCst);
        }
    }

    /// `SplitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn SplitAndScatter(&self, keys: &[&str]) {
        let _g = self.mu.lock().unwrap();
        for key in keys {
            self.SplitAt(key);
        }
        let region_ids: Vec<u64> = self.Regions.lock().unwrap().iter().map(|r| r.ID).collect();
        for rid in region_ids {
            let chosen = self.chooseStores(3);
            self.TransferRegionTo(rid, &chosen);
            self.shuffleLeader(rid);
        }
    }

    /// `RemoveStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RemoveStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn RemoveStore(&self, id: u64) {
        let _g = self.mu.lock().unwrap();
        let store = {
            let map = self.StoreMap.lock().unwrap();
            map.get(&id).cloned()
        };
        let store = store.expect("RemoveStore called with an unknown store ID");
        let regions: Vec<Arc<Region>> = store.RegionMap.lock().unwrap().values().cloned().collect();
        for r in regions {
            if r.Leader.load(Ordering::SeqCst) == id {
                let ps = self.FindPeers(r.ID);
                self.UpdateRegion(r.ID, |region| {
                    for p in &ps {
                        if *p != region.Leader.load(Ordering::SeqCst) {
                            info!(
                                target: "fakecluster",
                                region = region.ID,
                                new_leader = *p,
                                old_leader = region.Leader.load(Ordering::SeqCst),
                                "remove store: transforming leader"
                            );
                            region.Leader.store(*p, Ordering::SeqCst);
                            break;
                        }
                    }
                });
            }
        }
        self.StoreMap.lock().unwrap().remove(&id);
    }

    /// `UpdateRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `UpdateRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn UpdateRegion<F>(&self, rid: u64, mutator: F)
    where
        F: FnOnce(&Region),
    {
        let r = self
            .FindRegionByID(rid)
            .expect("UpdateRegion called with an unknown region ID");
        mutator(&r);
    }

    /// `AdvanceCheckpoints`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AdvanceCheckpoints` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AdvanceCheckpoints(&self) -> u64 {
        let mut min_checkpoint = u64::MAX;
        let region_ids: Vec<u64> = self.Regions.lock().unwrap().iter().map(|r| r.ID).collect();
        let mut rng = rand::thread_rng();
        for rid in region_ids {
            self.UpdateRegion(rid, |r| {
                let delta = rng.gen_range(1u64..=256);
                let cp = r.Checkpoint.fetch_add(delta, Ordering::SeqCst) + delta;
                if cp < min_checkpoint {
                    min_checkpoint = cp;
                }
                r.FlushSim.FlushedEpoch.store(0, Ordering::SeqCst);
            });
        }
        info!(target: "fakecluster", to = min_checkpoint, "checkpoint updated");
        min_checkpoint
    }

    /// `AdvanceCheckpointBy`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AdvanceCheckpointBy` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AdvanceCheckpointBy(&self, duration: Duration) -> u64 {
        let mut min_checkpoint = u64::MAX;
        let region_ids: Vec<u64> = self.Regions.lock().unwrap().iter().map(|r| r.ID).collect();
        for rid in region_ids {
            self.UpdateRegion(rid, |r| {
                let new_time = oracle::add_duration(
                    oracle::GetTimeFromTS(r.Checkpoint.load(Ordering::SeqCst)),
                    duration,
                );
                let new_checkpoint = oracle::GoTimeToTS(new_time);
                r.Checkpoint.store(new_checkpoint, Ordering::SeqCst);
                if new_checkpoint < min_checkpoint {
                    min_checkpoint = new_checkpoint;
                }
                r.FlushSim.FlushedEpoch.store(0, Ordering::SeqCst);
            });
        }
        info!(target: "fakecluster", to = min_checkpoint, "checkpoint updated");
        min_checkpoint
    }

    /// `AdvanceClusterTimeBy`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `AdvanceClusterTimeBy` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn AdvanceClusterTimeBy(&self, duration: Duration) -> u64 {
        let new_time = oracle::GoTimeToTS(oracle::add_duration(
            oracle::GetTimeFromTS(self.CurrentTS.load(Ordering::SeqCst)),
            duration,
        ));
        self.CurrentTS.store(new_time, Ordering::SeqCst);
        new_time
    }

    /// `FlushAll`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FlushAll` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FlushAll(&self) {
        let map = self.StoreMap.lock().unwrap();
        for s in map.values() {
            s.Flush();
        }
    }

    /// `FlushAllExcept`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `FlushAllExcept` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn FlushAllExcept(&self, keys: &[&str]) {
        let map = self.StoreMap.lock().unwrap();
        for s in map.values() {
            s.FlushExcept(keys);
        }
    }

    /// `RegionIDs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RegionIDs` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn RegionIDs(&self) -> Vec<u64> {
        self.RegionList().into_iter().map(|r| r.ID).collect()
    }

    /// `RegionSnapshot`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RegionSnapshot` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn RegionSnapshot(&self, regionID: u64) -> (RegionState, bool) {
        match self.FindRegionByID(regionID) {
            Some(r) => (stateFromRegion(&r), true),
            None => (RegionState::default(), false),
        }
    }

    /// `RegionSnapshotsOnStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RegionSnapshotsOnStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn RegionSnapshotsOnStore(&self, storeID: u64) -> Result<Vec<RegionState>> {
        let map = self.StoreMap.lock().unwrap();
        let store = map
            .get(&storeID)
            .ok_or_else(|| Error::new(format!("store {storeID} not found")))?;
        let region_map = store.RegionMap.lock().unwrap();
        let mut result = Vec::with_capacity(region_map.len());
        for r in region_map.values() {
            result.push(stateFromRegion(r));
        }
        result.sort_by_key(|r| r.ID);
        Ok(result)
    }

    /// `ApplyCheckpointToStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ApplyCheckpointToStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn ApplyCheckpointToStore(
        &self,
        ctx: &Context,
        storeID: u64,
        checkpoint: u64,
    ) -> Result<Vec<RegionState>> {
        let store = {
            let map = self.StoreMap.lock().unwrap();
            map.get(&storeID)
                .cloned()
                .ok_or_else(|| Error::new(format!("store {storeID} not found")))?
        };
        let mut region_ids: Vec<u64> = store.RegionMap.lock().unwrap().keys().copied().collect();
        region_ids.sort();

        {
            let region_map = store.RegionMap.lock().unwrap();
            for region_id in &region_ids {
                let r = region_map.get(region_id).unwrap();
                let cur = r.Checkpoint.load(Ordering::SeqCst);
                if checkpoint <= cur {
                    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                    return Err(Error::new(format!(
                        "region {region_id} checkpoint {checkpoint} must be greater than current {cur}"
                    )));
                }
            }
        }

        let mut states = Vec::with_capacity(region_ids.len());
        let mut events = Vec::with_capacity(region_ids.len());
        {
            let region_map = store.RegionMap.lock().unwrap();
            for region_id in &region_ids {
                let r = region_map.get(region_id).unwrap();
                r.Checkpoint.store(checkpoint, Ordering::SeqCst);
                let state = stateFromRegion(r);
                events.push(FlushEvent {
                    StartKey: codec::EncodeBytes(Vec::new(), &state.StartKey),
                    EndKey: codec::EncodeBytes(Vec::new(), &state.EndKey),
                    Checkpoint: checkpoint,
                });
                states.push(state);
            }
        }
        if checkpoint > self.CurrentTS.load(Ordering::SeqCst) {
            self.CurrentTS.store(checkpoint, Ordering::SeqCst);
        }

        let resp = SubscribeFlushEventResponse { Events: events };
        let receivers: Vec<SyncSender<SubscribeFlushEventResponse>> = {
            let client = store.client.lock().unwrap();
            client.subscribers.values().cloned().collect()
        };
        for ch in receivers {
            // blocking send with cancel, matching Go select
            loop {
                if ctx.is_done() {
                    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
                    return Err(Error::new(format!(
                        "send flush events for store {storeID} to subscribers: {}",
                        ctx.err_message()
                            .unwrap_or_else(|| "context canceled".into())
                    )));
                }
                match ch.try_send(resp.clone()) {
                    Ok(()) => break,
                    Err(TrySendError::Full(_)) => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
        }
        Ok(states)
    }

    /// `NewTaskEvent`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NewTaskEvent` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    pub fn NewTaskEvent(&self, taskName: &str, startTS: u64) -> streamhelper::TaskEvent {
        streamhelper::TaskEvent {
            Type: streamhelper::EventType::EventAdd,
            Name: taskName.to_string(),
            Info: Some(streamhelper::StreamBackupTaskInfo {
                Name: taskName.to_string(),
                StartTs: startTS,
                EndTs: 0,
                TableFilter: Vec::new(),
                Storage: None,
            }),
            Ranges: vec![streamhelper::KeyRange::default()],
            Err: None,
        }
    }
}

/// `Cluster` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `Cluster` 方法边界：非法参数应返回可分类错误而非 panic。
impl std::fmt::Display for Cluster {
    /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `fmt` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, ">>> fake cluster <<<\nregions: ")?;
        for region in self.Regions.lock().unwrap().iter() {
            write!(f, "{region} ")?;
        }
        writeln!(f)?;
        for store in self.StoreMap.lock().unwrap().values() {
            writeln!(f, "{store}")?;
        }
        Ok(())
    }
}
