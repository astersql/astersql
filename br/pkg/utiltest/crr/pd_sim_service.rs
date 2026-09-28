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

//! PDSim Env / streamhelper service methods (Go `pd_sim_service.go`).
//! 将 PDSim 适配为 streamhelper 所需的集群元数据与日志备份服务接口。
//! 实现 TiKVClusterMeta / LogBackupService / StreamMeta 等 trait，供 advancer 注入。
//! StoreClientAdapter 把 fakecluster::Store 包装成 LogBackupClient。
//! 锁解析相关方法刻意返回 unsupported，DRR harness 不覆盖该路径。
//! PauseTask 在 Rust 侧无实时 task 通道，仅校验任务名后成功返回。
//! 与 pd_sim.rs 分工：那边管状态，这边对接 streamhelper 接口面。

// Arc 包装 StoreClientAdapter，满足 LogBackupClient 对象安全要求。
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;

// streamhelper 生产 trait；本文件只做适配，不实现真实 gRPC。
use astersql_br_pkg_streamhelper::regioniter::{RegionWithLeader, Store, TiKVClusterMeta};
use astersql_br_pkg_streamhelper::{
    EventType, FlushEvent, GetLastFlushTSOfRegionRequest, GetLastFlushTSOfRegionResponse, KeyRange,
    LogBackupClient, LogBackupFlushIntervalGetter, LogBackupService, RegionCheckpoint, RegionError,
    RegionIdentity, RegionLockResolver, StreamBackupTaskInfo, StreamMeta, TaskEvent,
};
use astersql_br_pkg_streamhelper_config::{Config, DefaultCommandConfig};
use astersql_br_pkg_utiltest_fakecluster::{self as fakecluster, Context as FcContext};

use crate::pd_sim::PDSim;
use crate::stubs::{Context, Error, Result};

/// 集群元数据：Region 扫描、Store 列表、GC 阻塞与当前 TS。
impl TiKVClusterMeta for PDSim {
    /// 按键范围扫描 region；limit 负值按 0 处理，避免底层 panic。
    fn RegionScan(
        &self,
        key: &[u8],
        endKey: &[u8],
        limit: i32,
    ) -> std::result::Result<Vec<RegionWithLeader>, String> {
        self.cluster
            .RegionScan(&FcContext::background(), key, endKey, limit.max(0) as usize)
            .map_err(|e| e.to_string())
    }

    /// 返回全部 store 元信息，供 advancer 枚举刷盘目标。
    fn Stores(&self) -> std::result::Result<Vec<Store>, String> {
        self.cluster
            .Stores(&FcContext::background())
            .map_err(|e| e.to_string())
    }

    /// 阻塞 GC 直到给定 TS，防止 checkpoint 之前的数据被回收。
    fn BlockGCUntil(&self, at: u64) -> std::result::Result<u64, String> {
        self.cluster
            .BlockGCUntil(&FcContext::background(), at)
            .map_err(|e| e.to_string())
    }

    /// 解除 GC 阻塞，并把服务级 GC safepoint 清零。
    fn UnblockGC(&self) -> std::result::Result<(), String> {
        self.cluster
            .UnblockGC(&FcContext::background())
            .map_err(|e| e.to_string())?;
        self.cluster.ServiceGCSafePoint.store(0, Ordering::SeqCst);
        Ok(())
    }

    /// 拉取集群当前 TS，对齐 PD GetTS 语义。
    fn FetchCurrentTS(&self) -> std::result::Result<u64, String> {
        self.cluster
            .FetchCurrentTS(&FcContext::background())
            .map_err(|e| e.to_string())
    }
}

/// Adapter wrapping a fakecluster Store as streamhelper LogBackupClient.
/// 将假集群 Store 适配为 streamhelper 的 LogBackupClient。
struct StoreClientAdapter {
    /// 假集群中的单个 store 句柄，承载 flush TS 查询状态。
    store: Arc<fakecluster::Store>,
}

impl LogBackupClient for StoreClientAdapter {
    /// 查询各 region 最近 flush TS；错误映射为 NotLeader 形态供重试。
    fn GetLastFlushTSOfRegion(
        &self,
        req: &GetLastFlushTSOfRegionRequest,
    ) -> std::result::Result<GetLastFlushTSOfRegionResponse, String> {
        // streamhelper 请求字段 → fakecluster 请求字段的浅转换。
        let fc_req = fakecluster::GetLastFlushTSOfRegionRequest {
            Regions: req
                .Regions
                .iter()
                .map(|r| fakecluster::RegionIdentity {
                    Id: r.Id,
                    EpochVersion: r.EpochVersion,
                })
                .collect(),
        };
        let resp = self
            .store
            .GetLastFlushTSOfRegion(&FcContext::background(), &fc_req)
            .map_err(|e| e.to_string())?;
        Ok(GetLastFlushTSOfRegionResponse {
            Checkpoints: resp
                .Checkpoints
                .into_iter()
                .map(|c| {
                    let region = c.Region.unwrap_or_default();
                    RegionCheckpoint {
                        Region: RegionIdentity {
                            Id: region.Id,
                            EpochVersion: region.EpochVersion,
                        },
                        Checkpoint: c.Checkpoint,
                        Err: c.Err.map(|e| RegionError {
                            EpochNotMatch: e.Message == "epoch not match",
                            NotLeader: e.Message == "not found",
                        }),
                    }
                })
                .collect(),
        })
    }

    fn SubscribeFlushEvents(&self) -> std::result::Result<mpsc::Receiver<Vec<FlushEvent>>, String> {
        let (ctx, cancel) = FcContext::with_cancel();
        let stream = self
            .store
            .SubscribeFlushEvent(ctx, &fakecluster::SubscribeFlushEventRequest::default())
            .map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(response) = stream.Recv() {
                let events = response
                    .Events
                    .into_iter()
                    .filter_map(|event| {
                        let start = decode_tikv_key(&event.StartKey).ok()?;
                        let end = decode_tikv_key(&event.EndKey).ok()?;
                        Some(FlushEvent {
                            StartKey: start,
                            EndKey: end,
                            Checkpoint: event.Checkpoint,
                        })
                    })
                    .collect();
                if tx.send(events).is_err() {
                    break;
                }
            }
            cancel.cancel();
        });
        Ok(rx)
    }
}

/// Decode TiDB's ascending memcomparable byte encoding used on flush-event keys.
fn decode_tikv_key(encoded: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if encoded.is_empty() {
        return Ok(Vec::new());
    }
    const GROUP: usize = 8;
    const MARKER: u8 = 0xff;
    let mut input = encoded;
    let mut decoded = Vec::new();
    loop {
        if input.len() < GROUP + 1 {
            return Err("invalid encoded key length".into());
        }
        let (group, rest) = input.split_at(GROUP);
        let marker = rest[0];
        let pad = MARKER
            .checked_sub(marker)
            .ok_or_else(|| "invalid encoded key marker".to_string())? as usize;
        if pad > GROUP || group[GROUP - pad..].iter().any(|byte| *byte != 0) {
            return Err("invalid encoded key padding".into());
        }
        decoded.extend_from_slice(&group[..GROUP - pad]);
        input = &rest[1..];
        if pad != 0 {
            return Ok(decoded);
        }
    }
}

/// 按 store 获取日志备份客户端，并支持清缓存。
impl LogBackupService for PDSim {
    /// 返回包装后的 StoreClientAdapter；store 不存在则透传错误。
    fn GetLogBackupClient(
        &self,
        storeID: u64,
    ) -> std::result::Result<Arc<dyn LogBackupClient>, String> {
        let store = self
            .cluster
            .GetLogBackupClient(&FcContext::background(), storeID)
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(StoreClientAdapter { store }))
    }

    /// 清理指定 store 上的客户端缓存，迫使下次重建连接。
    fn ClearCache(&self, storeID: u64) -> std::result::Result<(), String> {
        self.cluster
            .ClearCache(&FcContext::background(), storeID)
            .map_err(|e| e.to_string())
    }
}

/// 刷盘间隔：复用默认命令配置的 resolve-lock 间隔作为测试占位。
impl LogBackupFlushIntervalGetter for PDSim {
    fn GetLogBackupFlushInterval(&self) -> std::result::Result<std::time::Duration, String> {
        Ok(DefaultCommandConfig().GetResolveLockInterval())
    }
}

/// StreamMeta：任务发现、全局 checkpoint 读写与暂停。
impl StreamMeta for PDSim {
    /// Begin 推送单个 EventAdd，携带当前任务名与起始 TS、全范围 KeyRange。
    fn Begin(&self, ch: &mut Vec<TaskEvent>) -> std::result::Result<(), String> {
        let state_task_name = self.task_name();
        let state_task_start = self.task_start();
        ch.push(TaskEvent {
            Type: EventType::EventAdd,
            Name: state_task_name.clone(),
            Info: Some(StreamBackupTaskInfo {
                Name: state_task_name,
                StartTs: state_task_start,
                ..Default::default()
            }),
            // 默认空 KeyRange 表示全表范围，与 Go 单任务 harness 一致。
            Ranges: vec![KeyRange::default()],
            Err: None,
        });
        Ok(())
    }

    /// 委托 pd_sim 内部上传，保留回滚/未知任务错误语义。
    fn UploadV3GlobalCheckpointForTask(
        &self,
        taskName: &str,
        checkpoint: u64,
    ) -> std::result::Result<(), String> {
        self.upload_v3_global_checkpoint(taskName, checkpoint)
    }

    /// 读取任务全局 checkpoint；任务名不匹配时返回 unknown task。
    fn GetGlobalCheckpointForTask(&self, taskName: &str) -> std::result::Result<u64, String> {
        self.get_global_checkpoint(taskName)
    }

    /// 清空任务全局 checkpoint，用于测试重置场景。
    fn ClearV3GlobalCheckpointForTask(&self, taskName: &str) -> std::result::Result<(), String> {
        self.clear_v3_global_checkpoint(taskName)
    }

    /// 仅校验任务名；Go 会向 taskCh 发 EventPause，Rust Env 无实时通道故忽略。
    fn PauseTask(&self, taskName: &str) -> std::result::Result<(), String> {
        if taskName != self.task_name() {
            return Err(format!("unknown task \"{taskName}\""));
        }
        // Go sends EventPause on taskCh when present; Rust Env has no live channel.
        // 保留 EventPause 符号引用，避免被优化掉并提示语义差异。
        let _ = EventType::EventPause;
        Ok(())
    }
}

/// 锁解析在 DRR harness 中未实现，统一返回 unsupported。
impl RegionLockResolver for PDSim {
    /// 范围锁解析占位：刻意失败，避免测试误以为已接通 TiKV。
    fn ResolveLocksForRange(
        &self,
        _maxVersion: u64,
        _startKey: &[u8],
        _endKey: &[u8],
    ) -> std::result::Result<(), String> {
        Err("lock resolving is unsupported in DRR harness".into())
    }
}

impl PDSim {
    /// 公开等待接口：阻塞至全局 checkpoint 超过 current。
    pub fn WaitGlobalCheckpointAdvance(
        &self,
        ctx: &Context,
        taskName: &str,
        current: u64,
    ) -> Result<()> {
        self.wait_global_checkpoint_advance(ctx, taskName, current)
    }

    /// Env 标识符，便于日志区分真实 PD 与仿真器。
    pub fn Identifier(&self) -> &'static str {
        "drr-pd-sim"
    }

    /// 不提供真实 tikv.Storage；调用即 panic，迫使测试避开锁解析路径。
    pub fn GetStore(&self) -> ! {
        panic!("PDSim does not provide tikv.Storage; lock resolving is unsupported in DRR harness")
    }

    /// 单 region 锁扫描占位：始终错误。
    pub fn ScanLocksInOneRegion(
        &self,
        _key: &[u8],
        _endKey: &[u8],
        _maxVersion: u64,
        _limit: u32,
    ) -> Result<()> {
        Err(Error::new("lock scanning is unsupported in DRR harness"))
    }

    /// 单 region 锁解析占位：始终错误。
    pub fn ResolveLocksInOneRegion(&self) -> Result<()> {
        Err(Error::new("lock resolving is unsupported in DRR harness"))
    }
}
