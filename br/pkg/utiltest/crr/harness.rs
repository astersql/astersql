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

//! CRR 本地测试线束：装配 PDSim、FlushSim、上游事件存储、工人与 checkpoint advancer。
//! 对应 Go utiltest/crr harness，用临时目录上的 LocalStorage 模拟上下游。
//! Tick 推进 advancer；Pull/Replicate 驱动事件复制；断言下游可恢复到目标 TSO。
//! 构造失败时关闭已创建存储，避免临时目录泄漏句柄。
//! Drop/Close 删除 base 目录，保证测试隔离。
//! FlushSim 写入经 Upstream 装饰器，故 flush 产物会进入复制事件流。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use astersql_br_pkg_stream::MetadataHelper;
use astersql_br_pkg_stream::stubs::backuppb::Metadata;
use astersql_br_pkg_stream_backupmetas::ParseName;
use astersql_br_pkg_streamhelper::{
    CheckpointAdvancer, EventType, NewCommandCheckpointAdvancer, StreamMeta,
};

// StreamMeta provides UploadV3GlobalCheckpointForTask on PDSim.
// PDSim 实现 StreamMeta，供 advancer 读写任务元数据。

use crate::crr_sim::{
    CRRUpstreamStorage, CRRWorker, NewCRRUpstreamStorage, NewCRRWorker, new_event_channel,
};
use crate::flush_sim::{FlushSim, NewFlushSimWithTestContext};
use crate::pd_sim::{NewPDSimWithTestContext, PDSim};
use crate::stubs::{Context, Error, LocalStorage, Result, Storage};
use crate::types::{RegionBoundary, TestContext, defaultTaskName};

/// TestHarness wires PDSim/FlushSim/CRR worker and one advancer together.
/// 聚合 CRR 单测依赖的仿真组件与本地存储。
pub struct TestHarness {
    // 未包装的上游本地存储，供 Close 释放。
    upstream_storage: Arc<dyn Storage>,
    // 下游本地存储句柄。
    downstream_storage: Arc<dyn Storage>,
    // 临时根目录，Close 时整树删除。
    _base_dir: PathBuf,

    /// 假 PD：region 布局、TSO 与全局检查点。
    pub PDSim: Arc<PDSim>,
    /// flush 模拟器，写入上游并记录 FlushRecord。
    pub FlushSim: FlushSim,
    /// 复制工人，消费上游事件写入下游。
    pub CRRWorker: CRRWorker,
    /// 带事件发射的上游装饰存储。
    pub Upstream: Arc<CRRUpstreamStorage>,
    /// 下游存储视图，供断言读取。
    pub Downstream: Arc<dyn Storage>,
    /// 检查点推进器，与 PDSim 任务监听对接。
    pub Advancer: CheckpointAdvancer,
}

/// 在系统临时目录创建线束，目录名含 seed 与 pid。
pub fn NewLocalTestHarnessWithTestContext(
    ctx: &Context,
    tc: &TestContext,
    boundaries: Vec<RegionBoundary>,
) -> Result<TestHarness> {
    static NEXT_HARNESS_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_HARNESS_ID.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!(
        "crr-harness-{}-{}-{id}",
        tc.Seed(),
        std::process::id()
    ));
    newLocalTestHarness(ctx, tc, base, boundaries)
}

// 创建上下游目录、PD/工人/advancer 并接线事件通道。
fn newLocalTestHarness(
    ctx: &Context,
    tc: &TestContext,
    baseDir: PathBuf,
    boundaries: Vec<RegionBoundary>,
) -> Result<TestHarness> {
    let upstreamDir = baseDir.join("upstream");
    // 上下游分目录，避免文件名冲突掩盖复制错误。
    let downstreamDir = baseDir.join("downstream");
    fs::create_dir_all(&upstreamDir).map_err(|e| {
        Error::new(format!(
            "create upstream dir {}: {e}",
            upstreamDir.display()
        ))
    })?;
    fs::create_dir_all(&downstreamDir).map_err(|e| {
        Error::new(format!(
            "create downstream dir {}: {e}",
            downstreamDir.display()
        ))
    })?;

    let upstreamStorage: Arc<dyn Storage> =
        Arc::new(LocalStorage::new(&upstreamDir).map_err(|e| {
            Error::new(format!(
                "create local upstream storage at {}: {e}",
                upstreamDir.display()
            ))
        })?);
    let downstreamStorage: Arc<dyn Storage> = match LocalStorage::new(&downstreamDir) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            // 下游创建失败：先关上游再返回。
            upstreamStorage.Close();
            return Err(Error::new(format!(
                "create local downstream storage at {}: {e}",
                downstreamDir.display()
            )));
        }
    };

    let pd = match NewPDSimWithTestContext(boundaries, defaultTaskName.to_string(), tc) {
        Ok(p) => p,
        Err(e) => {
            upstreamStorage.Close();
            downstreamStorage.Close();
            return Err(Error::new(format!("create pd sim: {e}")));
        }
    };

    let (tx, rx) = new_event_channel();
    // 有界事件通道连接上游装饰器与工人。
    let worker = NewCRRWorker(
        Arc::clone(&upstreamStorage),
        Arc::clone(&downstreamStorage),
        rx,
    );
    let upstream = Arc::new(NewCRRUpstreamStorage(Arc::clone(&upstreamStorage), tx));

    let advancer = NewCommandCheckpointAdvancer(pd.clone());
    start_task_listener(&advancer, pd.as_ref());
    // 先同步注入已有任务，再启动订阅处理。
    advancer.SpawnSubscriptionHandler();
    // 后台订阅后续任务变更事件。
    let _ = ctx;

    Ok(TestHarness {
        upstream_storage: upstreamStorage,
        downstream_storage: Arc::clone(&downstreamStorage),
        _base_dir: baseDir,
        FlushSim: NewFlushSimWithTestContext(
            Arc::clone(&pd),
            Arc::clone(&upstream) as Arc<dyn Storage>,
            tc,
        ),
        PDSim: pd,
        CRRWorker: worker,
        Upstream: upstream,
        Downstream: downstreamStorage,
        Advancer: advancer,
    })
}

/// Mirrors Go `StartTaskListener` for the Rust Env (synchronous Begin + SetTask).
/// 同步镜像 Go StartTaskListener：Begin 后处理 Add/Pause/Resume。
fn start_task_listener(advancer: &CheckpointAdvancer, env: &dyn StreamMeta) {
    let mut events = Vec::new();
    if env.Begin(&mut events).is_err() {
        // Begin 失败则静默跳过，保持与 Go 宽松启动一致。
        return;
    }
    for ev in events {
        match ev.Type {
            EventType::EventAdd => {
                // 注册任务信息与 key ranges。
                if let Some(info) = ev.Info {
                    advancer.SetTask(info, ev.Ranges);
                }
            }
            EventType::EventPause => {
                // 暂停推进。
                advancer.SetPaused(true);
            }
            EventType::EventResume => {
                // 恢复推进。
                advancer.SetPaused(false);
            }
            _ => {}
        }
    }
}

impl TestHarness {
    /// Tick drives one deterministic advancer state transition.
    /// 触发一次 advancer OnTick，返回当前全局检查点。
    pub fn Tick(&self, _ctx: &Context) -> Result<u64> {
        self.Advancer.OnTick().map_err(|e| Error::new(e))?;
        Ok(self.PDSim.GlobalCheckpoint())
    }

    /// PullMessages pulls pending replication events into worker local buffer.
    /// 委托工人拉取待复制事件。
    pub fn PullMessages(&mut self, limit: i32) -> i32 {
        self.CRRWorker.PullMessages(limit)
    }

    /// Replicate copies buffered events to downstream.
    /// 委托工人按最新优先复制缓冲事件。
    pub fn Replicate(&mut self, ctx: &Context, limit: i32) -> Result<i32> {
        self.CRRWorker.ReplicateBuffered(ctx, limit)
    }

    /// UploadGlobalCheckpoint uploads task global checkpoint in simulator metadata.
    /// 向假 PD 上传任务级全局检查点。
    pub fn UploadGlobalCheckpoint(&self, _ctx: &Context, checkpoint: u64) -> Result<()> {
        self.PDSim
            .UploadV3GlobalCheckpointForTask(defaultTaskName, checkpoint)
            .map_err(|e| Error::new(format!("upload global checkpoint {checkpoint}: {e}")))
    }

    /// AssertDownstreamCanRestoreTo validates that downstream can read every
    /// backupmeta and corresponding log files that were already part of a checkpoint
    /// transition to tso.
    /// 断言下游已具备恢复到 tso 所需的 meta 与 log 可读性。
    pub fn AssertDownstreamCanRestoreTo(&self, ctx: &Context, tso: u64) -> Result<()> {
        let globalCheckpoint = self.PDSim.GlobalCheckpoint();
        // 目标 TSO 不得超过当前全局检查点。
        if globalCheckpoint < tso {
            return Err(Error::new(format!(
                "global checkpoint {globalCheckpoint} is behind target {tso}"
            )));
        }

        let records = self.FlushSim.RecordsUpTo(tso);
        // 仅检查已纳入该检查点及之前的 flush。
        for record in records {
            assertReadableFile(ctx, self.Downstream.as_ref(), &record.MetadataPath)?;

            let baseName = std::path::Path::new(&record.MetadataPath)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let parsed = ParseName(baseName).map_err(|e| {
                Error::new(format!("parse backupmeta {}: {e}", record.MetadataPath))
            })?;
            if parsed.FlushTS != record.FlushTS {
                // 文件名标签与记录不一致视为损坏。
                return Err(Error::new(format!(
                    "backupmeta {} has flush ts {}, expected {}",
                    record.MetadataPath, parsed.FlushTS, record.FlushTS
                )));
            }

            let content = self
                .Downstream
                .ReadFile(ctx, &record.MetadataPath)
                .map_err(|e| Error::new(format!("read backupmeta {}: {e}", record.MetadataPath)))?;
            let meta = parseBackupMetadata(&content).map_err(|e| {
                Error::new(format!("parse backupmeta {}: {e}", record.MetadataPath))
            })?;

            for logPath in extractDataFilePaths(&meta) {
                // meta 引用的每个 log 都必须在下游可读。
                assertReadableFile(ctx, self.Downstream.as_ref(), &logPath).map_err(|e| {
                    Error::new(format!(
                        "backupmeta {} references unreadable log file: {e}",
                        record.MetadataPath
                    ))
                })?;
            }
        }
        Ok(())
    }

    /// Close releases harness storage resources.
    /// 关闭存储并删除临时目录。
    pub fn Close(&self) {
        self.upstream_storage.Close();
        self.downstream_storage.Close();
        let _ = fs::remove_dir_all(&self._base_dir);
    }
}

// RAII：离开作用域自动 Close。
impl Drop for TestHarness {
    fn drop(&mut self) {
        self.Close();
    }
}

// 读取失败则包装为「不可读」错误。
fn assertReadableFile(ctx: &Context, storage: &dyn Storage, name: &str) -> Result<()> {
    storage
        .ReadFile(ctx, name)
        .map(|_| ())
        .map_err(|e| Error::new(format!("{name} is not readable: {e}")))
}

// 从 FileGroups/DataFilesInfo 收集非空 Path。
fn extractDataFilePaths(meta: &Metadata) -> Vec<String> {
    let mut paths = Vec::with_capacity(meta.FileGroups.len());
    for group in &meta.FileGroups {
        if !group.Path.is_empty() {
            paths.push(group.Path.clone());
            continue;
        }
        for file in &group.DataFilesInfo {
            if !file.Path.is_empty() {
                paths.push(file.Path.clone());
            }
        }
    }
    paths
}

// 经 MetadataHelper 解析 backupmeta 字节。
fn parseBackupMetadata(raw: &[u8]) -> Result<Metadata> {
    MetadataHelper::ParseToMetadata(raw).map_err(|e| Error::new(e.to_string()))
}
