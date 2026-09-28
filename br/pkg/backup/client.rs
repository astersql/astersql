// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Backup client matching `br/pkg/backup/client.go`.
//!
//! 本模块对齐 Go `br/pkg/backup/client.go`，是 BR 全量/表级备份的**编排层**：
//! - 管理外部存储、checkpoint、加密与 GC safepoint 相关配置；
//! - 根据表过滤与元数据构造备份 key range 与 schema；
//! - 通过 `MainBackupLoop` / `RunLoop` 在所有存活 TiKV 上多轮发送备份请求；
//! - 消费 store 响应、解析锁冲突、推进 `ProgressRangeTree`，并在完成后汇总 checksum。
//!
//! 数据流概览：
//! `BackupRanges` → 建进度树 → 观察 PD store 变更 → `RunLoop` 多轮：
//!   取未完成 ranges → 按 label 选 store → `SendAsync`/`startBackup` →
//!   `CollectStoreBackupsAsync` 汇聚 → `OnBackupResponse` 落盘/记锁 →
//!   必要时 `ResolveLocksForRead` 后进入下一轮。
//!
//! Rust 侧用 `mpsc` + 轮询模拟 Go 的 `reflect.Select`；部分进度回调与树清理
//! 依赖边界使用内存实现，对外错误类型、checkpoint 配置哈希校验、锁文件语义
//! 仍需与 Go 保持一致。连接管理由 `ClientMgr` 注入，本文件不直接建链。
//!
//! 关键约束：checkpoint 哈希不一致必须失败；锁文件存在则拒绝绑定存储；
//! RunLoop 在上下文取消时尽快退出并等待在途发送收尾。
//! 进度树清空是成功完成的充分条件；部分失败通过重试策略消化。
//! 本文件中的 channel 轮询是实现细节，对外契约以错误类型与落盘结果为准。

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::limit::{NewResourceMemoryLimiter, ResourceConcurrentLimiter};
use crate::schema::{NewBackupSchemas, Schemas};
use crate::store::{
    BackupRetryPolicy, BackupSender, ObserveStoreChangesAsync, ResponseAndStore, startBackup,
};
use crate::stubs::backuppb::{self, BackupClient, BackupRequest, CipherInfo, PlacementPolicy};
use crate::stubs::berrors;
use crate::stubs::checkpoint::{self, CheckpointMetadataForBackup, CheckpointRunner};
use crate::stubs::conn;
use crate::stubs::ddl;
use crate::stubs::distsql;
use crate::stubs::filter::Filter;
use crate::stubs::gc;
use crate::stubs::glue::{self, Glue};
use crate::stubs::kvrpcpb;
use crate::stubs::meta;
use crate::stubs::metadef;
use crate::stubs::metapb;
use crate::stubs::metautil::{self, ChecksumStats, MetaPayload, MetaWriter};
use crate::stubs::model::{self, Job};
use crate::stubs::objstore;
use crate::stubs::oracle;
use crate::stubs::rtree::{self, FreeListG, KeyRange, ProgressRange, ProgressRangeTree};
use crate::stubs::storeapi;
use crate::stubs::summary;
use crate::stubs::tablecodec;
use crate::stubs::txnlock::{self, Lock};
use crate::stubs::utils::{self, ErrorContext, NewErrorContext};
use crate::stubs::version;
use crate::stubs::{
    Context, Error, ExternalStorage, PdClient, Result, Storage, Version, should_skip_round_sleep,
};

/// MaxResolveLocksbackupOffSleepMs mirrors Go constant (10 minutes).
///
/// 每轮解析事务锁时 Backoffer 允许的最长休眠（毫秒）。
/// Go 注释：每轮最多 sleep 10 分钟，避免锁解析长时间占满备份循环。
pub const MaxResolveLocksbackupOffSleepMs: u64 = 600_000;

/// IncompleteRangesUpdateInterval mirrors Go constant.
///
/// 主循环内刷新 `BackupReq.SubRanges`（未完成区间）的默认间隔。
/// 过短会增加进度树扫描开销，过长会让新完成区间不能及时从请求中剔除。
pub const IncompleteRangesUpdateInterval: Duration = Duration::from_secs(15);

/// RangesSentThreshold mirrors Go constant.
///
/// 发送区间规模的经验阈值（与 Go 同值）；用于观测/限流相关决策，
/// 防止单轮请求携带过多 sub-range 导致编解码与内存压力过大。
pub const RangesSentThreshold: u64 = 30_000_000;

/// ClientMgr manages connections needed by backup.
///
/// 备份所需连接的抽象：按 store 取/重置 Backup gRPC 客户端，以及 PD、
/// TiDB Storage、GC Manager、LockResolver。生产由 conn 层实现；测试可注入桩。
pub trait ClientMgr: Send + Sync {
    /// 获取（或复用）指向 `storeID` 的 Backup 客户端。
    fn GetBackupClient(&self, ctx: &Context, storeID: u64) -> Result<Arc<dyn BackupClient>>;
    /// 丢弃旧连接并重建客户端；`RunLoop` 在拓扑变化或非锁错误后常置 `reset=true`。
    fn ResetBackupClient(&self, ctx: &Context, storeID: u64) -> Result<Arc<dyn BackupClient>>;
    /// 返回 PD 客户端，用于 TS、store 列表与拓扑观察。
    fn GetPDClient(&self) -> Arc<dyn PdClient>;
    /// 返回 TiDB/KV Storage，用于读元数据与构造 range。
    fn GetStorage(&self) -> Arc<dyn Storage>;
    /// GC safepoint 管理器；`GetTS` 成功后会校验 backupTS 未被 GC。
    fn GetGCManager(&self) -> Arc<dyn gc::Manager>;
    /// 事务锁解析器；响应含 `Locked` 时在轮末 `ResolveLocksForRead`。
    fn GetLockResolver(&self) -> Arc<dyn txnlock::LockResolver>;
    /// 释放管理器持有的连接与后台资源。
    fn Close(&self);
}

/// ProgressUnit mirrors Go type.
///
/// 进度回调的计量单位字符串，与 CLI/summary 展示对齐。
pub type ProgressUnit = &'static str;
/// 一个逻辑 range 完成时上报（较粗粒度）。
pub const UnitRange: ProgressUnit = "range";
/// 一个 region/响应分片完成时上报（`OnBackupResponse` 成功路径常用）。
pub const UnitRegion: ProgressUnit = "region";

/// MainBackupLoop mirrors Go struct.
///
/// 单次 `BackupRanges` 的运行时上下文：请求模板、进度树、重试通知通道、
/// 限流器与取客户端回调。`state_rx` 为 Rust 侧对 Go `StateNotifier` 的接收端拆分
/// （Go 用同一 chan 双向语义，这里 Sender/Receiver 分离以便 `RunLoop` 独占接收）。
pub struct MainBackupLoop {
    /// 向各 store 异步发送备份请求的实现（通常为 `MainBackupSender`）。
    pub BackupSender: Box<dyn BackupSender>,
    /// 本轮备份请求模板；每轮会用未完成区间覆盖 `SubRanges`。
    pub BackupReq: BackupRequest,
    /// 每个 store 上的发送并发度，传入 `startBackup` 做 range 切分。
    pub Concurrency: u32,
    /// 全局未完成/已完成区间树，跨轮次共享以推进进度。
    pub GlobalProgressTree: Arc<ProgressRangeTree>,
    /// 副本读标签过滤；非空时只向匹配 label 的 store 发请求。
    pub ReplicaReadLabel: HashMap<String, String>,
    /// 向主循环投递重试策略（单 store 或全量）的发送端。
    pub StateNotifier: Sender<BackupRetryPolicy>,
    /// 重试策略接收端；`RunLoop` 启动时 `take` 走，避免多消费者。
    pub state_rx: Option<Receiver<BackupRetryPolicy>>,
    /// 限制同时 marshal/在途请求规模，对齐 Go `ResourceConcurrentLimiter`。
    pub Limiter: Arc<ResourceConcurrentLimiter>,
    /// 进度单位回调（range/region），供 UI/summary 累加。
    pub ProgressCallBack: Box<dyn Fn(ProgressUnit) + Send + Sync>,
    /// 按 store 获取客户端；`reset=true` 时走 `ResetBackupClient`。
    pub GetBackupClientCallBack:
        Box<dyn Fn(&Context, u64, bool) -> Result<Arc<dyn BackupClient>> + Send + Sync>,
}

/// MainBackupSender mirrors Go `MainBackupSender`.
///
/// 默认发送实现：后台线程调用 `startBackup`；非取消错误则通知对该 store 重试，
/// 最后向 `respCh` 发送 `None` 作为关闭标记（对应 Go `close(respCh)`）。
pub struct MainBackupSender;

impl BackupSender for MainBackupSender {
    fn SendAsync(
        &self,
        ctx: Context,
        round: u64,
        storeID: u64,
        limiter: Arc<ResourceConcurrentLimiter>,
        request: BackupRequest,
        concurrency: u32,
        cli: Arc<dyn BackupClient>,
        respCh: Sender<Option<ResponseAndStore>>,
        StateNotifier: Sender<BackupRetryPolicy>,
    ) {
        // 对齐 Go：仅两类失败——gRPC（内部已重试）与外部取消；其余触发单 store 重试。
        // 取消路径不投递重试策略，避免取消后仍打热 store。
        thread::spawn(move || {
            let result = startBackup(
                &ctx,
                storeID,
                limiter,
                request,
                cli,
                concurrency,
                respCh.clone(),
            );
            if let Err(err) = result {
                // Go 用 errors.Cause(err)==context.Canceled；此处用消息/`Done` 近似判定。
                // Go 用 errors.Cause；此处消息/`Done` 近似。
                let canceled = err.msg.contains("context canceled") || ctx.Done();
                if canceled {
                    // store backup cancelled —— 不重试，等待上层取消收尾
                    // 有意留空分支，语义与 Go 注释一致。
                } else {
                    let _ = (round, storeID);
                    // 仍存活时通知主循环仅重发该 store（BackupRetryPolicy::One）。
                    if !ctx.Done() {
                        let _ = StateNotifier.send(BackupRetryPolicy {
                            One: storeID,
                            All: false,
                        });
                    }
                }
            }
            // 关闭标记：汇聚侧收到 None/断连即减少剩余 producer（对齐 close(chan)）。
            // 必须发送，否则 Collect 可能永久等待。
            let _ = respCh.send(None); // close marker
        });
    }
}

impl MainBackupLoop {
    /// CollectStoreBackupsAsync mirrors Go method: multiplex store channels into globalCh.
    ///
    /// 将各 store 的响应通道多路复用到 `globalCh`。Go 使用 `reflect.Select` 阻塞等待；
    /// Rust 用非阻塞 `try_recv` + 短 sleep 轮询，语义上仍是：有数据则转发，
    /// 通道关闭/`None` 则减少剩余 producer，全部退出后向全局通道发关闭标记。
    pub fn CollectStoreBackupsAsync(
        &self,
        ctx: Context,
        round: u64,
        storeBackupChs: HashMap<u64, Arc<Mutex<Receiver<Option<ResponseAndStore>>>>>,
        globalCh: Sender<Option<ResponseAndStore>>,
    ) {
        thread::spawn(move || {
            let totalProducers = storeBackupChs.len();
            let mut receivers: Vec<Arc<Mutex<Receiver<Option<ResponseAndStore>>>>> =
                storeBackupChs.into_values().collect();
            let mut remaining = receivers.len();
            let mut allProducersExited = false;
            // 只要还有 store 生产者存活就继续汇聚；取消时尽快结束并关闭全局通道。
            while remaining > 0 {
                if ctx.Done() {
                    let _ = globalCh.send(None);
                    return;
                }
                let mut progressed = false;
                let mut i = 0;
                while i < receivers.len() {
                    let received = receivers[i].lock().unwrap().try_recv();
                    match received {
                        Ok(Some(v)) => {
                            // 转发前再查取消，避免向已取消的 handleLoop 堆积响应。
                            if ctx.Done() {
                                let _ = globalCh.send(None);
                                return;
                            }
                            let _ = globalCh.send(Some(v));
                            progressed = true;
                            i += 1;
                        }
                        // None/断连：该 store 本轮结束（对齐 Go select 收到 closed chan）。
                        Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                            receivers.remove(i);
                            remaining -= 1;
                            progressed = true;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            i += 1;
                        }
                    }
                }
                // 无进展时让出 CPU，避免忙等；间隔刻意短以降低汇聚延迟。
                if !progressed {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            allProducersExited = true;
            let _ = (allProducersExited, totalProducers, round);
            // 全部 producer 退出：关闭全局通道，驱动 handleLoop 进入锁解析/下一轮。
            let _ = globalCh.send(None);
        });
    }
}

/// Client instructs TiKV how to do a backup.
///
/// 备份客户端核心状态：连接管理、外部存储与后端描述、API 版本、加密、
/// checkpoint 元数据/运行器、GC TTL，以及是否按表解码 physicalID、是否跳过 checksum。
/// 字段在 `SetStorage*` / `StartCheckpointRunner` / `BackupRanges` 路径中逐步填充。
/// 备份客户端状态：存储、加密、checkpoint、GC TTL、API 版本等。
/// 方法按配置→建 range→BackupRanges→收尾组织；连接一律经 ClientMgr。
pub struct Client {
    /// 连接与组件管理器（PD/BackupClient/GC/锁解析）。
    // 字段 `mgr`：Client 状态的一部分，语义对齐 Go。
    mgr: Arc<dyn ClientMgr>,
    /// 集群 ID，构造时从 PD 读取，写入备份元数据以防止跨集群误用。
    // 字段 `clusterID`：Client 状态的一部分，语义对齐 Go。
    clusterID: u64,
    /// 外部对象存储句柄；未 `SetStorage` 前为 None。
    // 字段 `storage`：Client 状态的一部分，语义对齐 Go。
    storage: Option<Arc<dyn ExternalStorage>>,
    /// 与 `storage` 对应的后端描述（本地/S3 等），供元数据序列化。
    // 字段 `backend`：Client 状态的一部分，语义对齐 Go。
    backend: Option<backuppb::StorageBackend>,
    /// 从成功响应中学习到的 KV API 版本（V1/V1TTL/V2）。
    // 字段 `apiVersion`：Client 状态的一部分，语义对齐 Go。
    apiVersion: kvrpcpb::APIVersion,
    /// 可选备份文件加密参数，checkpoint 读写时透传。
    // 字段 `cipher`：Client 状态的一部分，语义对齐 Go。
    cipher: Option<CipherInfo>,
    /// 若目标目录已有 checkpoint 元数据则加载；用于续跑与配置哈希校验。
    // 字段 `checkpointMeta`：Client 状态的一部分，语义对齐 Go。
    checkpointMeta: Option<CheckpointMetadataForBackup>,
    /// 运行中的 checkpoint 追加器；成功响应会 `AppendForBackup`。
    // 字段 `checkpointRunner`：Client 状态的一部分，语义对齐 Go。
    checkpointRunner: Option<Arc<CheckpointRunner>>,
    /// BR GC safepoint TTL；`<=0` 时在 `SetGCTTL` 回落到默认值。
    // 字段 `gcTTL`：Client 状态的一部分，语义对齐 Go。
    gcTTL: i64,
    /// true 表示表级备份：进度树节点用表 ID 作为 physicalID。
    // 字段 `tableRange`：Client 状态的一部分，语义对齐 Go。
    tableRange: bool,
    /// true 时跳过 checksum 累加（加快备份，牺牲完整性校验）。
    // 字段 `skipChecksum`：Client 状态的一部分，语义对齐 Go。
    skipChecksum: bool,
}

/// NewBackupClient mirrors Go constructor.
///
/// 创建默认（非表级）备份客户端：从 PD 取 clusterID，其余字段置空/默认，
/// 调用方需再 `SetStorage*`、`SetGCTTL`、可选 checkpoint 后才能 `BackupRanges`。
/// 构造全量备份客户端：注入 ClientMgr，初始化默认 GC TTL 等字段。
pub fn NewBackupClient(ctx: &Context, mgr: Arc<dyn ClientMgr>) -> Client {
    let pdClient = mgr.GetPDClient();
    let clusterID = pdClient.GetClusterID(ctx);
    Client {
        clusterID,
        mgr,
        storage: None,
        backend: None,
        apiVersion: kvrpcpb::APIVersion::V1,
        cipher: None,
        checkpointMeta: None,
        checkpointRunner: None,
        gcTTL: 0,
        tableRange: false,
        skipChecksum: false,
    }
}

/// NewTableBackupClient mirrors Go constructor.
///
/// 表级备份客户端：在 `NewBackupClient` 基础上将 `tableRange=true`，
/// 使 `getProgressRange` 从 StartKey 解码 table ID 写入进度树。
/// 表级备份客户端入口；与全量共享 Client 结构，后续由调用方收窄 range。
pub fn NewTableBackupClient(ctx: &Context, mgr: Arc<dyn ClientMgr>) -> Client {
    let mut client = NewBackupClient(ctx, mgr);
    client.tableRange = true;
    client
}

impl Client {
    /// 设置备份文件加密参数；后续 checkpoint 读写与 SST 加密依赖此配置。
    /// 设置备份加密信息，后续写文件/元数据时透传。
    pub fn SetCipher(&mut self, cipher: CipherInfo) {
        self.cipher = Some(cipher);
    }

    /// 是否跳过备份过程中的 checksum 累加（对齐 Go `SetSkipChecksum`）。
    /// 跳过校验和时仅影响收尾汇总，不改变备份数据本身。
    pub fn SetSkipChecksum(&mut self, skipChecksum: bool) {
        self.skipChecksum = skipChecksum;
    }

    /// Test helper to inject checkpoint metadata (parity / unit tests).
    ///
    /// 仅测试注入 checkpoint 元数据，绕过真实外部存储加载路径。
    /// 测试注入 checkpoint 元数据，绕过真实存储读取。
    pub fn set_checkpoint_meta_for_test(&mut self, meta: CheckpointMetadataForBackup) {
        self.checkpointMeta = Some(meta);
    }

    /// 从 PD 取当前 TSO 并合成为 backup 可用的单一时间戳。
    /// 从 PD 取当前 TS，供未显式指定 backupTS 时使用。
    pub fn GetCurrentTS(&self, ctx: &Context) -> Result<u64> {
        let (p, l) = self.mgr.GetPDClient().GetTS(ctx)?;
        Ok(oracle::ComposeTS(p, l))
    }

    /// 解析本次备份使用的 TS：优先 checkpoint 中的 `BackupTS`；
    /// 否则若调用方指定 `ts>0` 则校验不晚于当前 TSO；否则取当前 TSO，
    /// 并可按 `duration`（timeago）回拨物理时间。最终必须通过 GC safepoint 检查。
    /// 解析 backupTS：显式 ts 优先，否则按 duration 回退；成功后校验 GC safepoint。
    pub fn GetTS(&self, ctx: &Context, duration: Duration, ts: u64) -> Result<u64> {
        // 续跑：强制使用首次备份写入 checkpoint 的 BackupTS，保证数据版本一致。
        if let Some(meta) = &self.checkpointMeta {
            return Ok(meta.BackupTS);
        }
        let backupTS = if ts > 0 {
            let (p, l) = self.mgr.GetPDClient().GetTS(ctx)?;
            let currentTS = oracle::ComposeTS(p, l);
            // 未来时间戳无意义且可能越过 GC/可见性边界。
            if ts > currentTS {
                return Err(Error::Annotate(
                    berrors::ErrInvalidArgument(),
                    format!(
                        "backup timestamp {ts} must not be later than current timestamp {currentTS}"
                    ),
                ));
            }
            ts
        } else {
            let (p, l) = self.mgr.GetPDClient().GetTS(ctx)?;
            let mut backupTS = oracle::ComposeTS(p, l);
            if duration.is_zero() {
                // keep backupTS —— timeago=0 表示「就用当前 TSO」
            } else if duration.as_secs_f64() < 0.0 {
                // 对齐 Go：负数 timeago 直接拒绝。
                // Duration can't be negative in Rust std; treat as invalid if somehow zero-neg via check above
                return Err(Error::Annotate(
                    berrors::ErrInvalidArgument(),
                    "negative timeago is not allowed",
                ));
            } else {
                // 从当前物理时间减去 timeago，再与原逻辑时钟分量重组合。
                let backupTime = oracle::GetTimeFromTS(backupTS);
                let backupAgo = backupTime.checked_sub(duration).ok_or_else(|| {
                    Error::Annotate(
                        berrors::ErrInvalidArgument(),
                        "backup ts overflow please choose a smaller timeago",
                    )
                })?;
                // `SystemTime` permits pre-epoch values on some platforms. Such values cannot
                // form a valid Unix TSO and Go rejects them as a timeago overflow.
                backupAgo
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| {
                        Error::Annotate(
                            berrors::ErrInvalidArgument(),
                            "backup ts overflow please choose a smaller timeago",
                        )
                    })?;
                let composed = oracle::ComposeTS(oracle::GetPhysical(backupAgo), l);
                // 下溢/回绕保护：回拨结果不应「大于」当前合成 TS。
                if backupTS < composed {
                    return Err(Error::Annotate(
                        berrors::ErrInvalidArgument(),
                        "backup ts overflow please choose a smaller timeago",
                    ));
                }
                backupTS = composed;
            }
            backupTS
        };

        // 确保 backupTS 仍在 GC safepoint 之后，否则历史版本可能已被回收。
        gc::CheckGCSafePoint(ctx, self.mgr.GetGCManager().as_ref(), backupTS)?;
        Ok(backupTS)
    }

    /// 写入备份目录锁文件，提示其他备份任务勿复用同一路径（对齐 Go `SetLockFile`）。
    /// 在外部存储写入锁文件，防止并发备份占用同一路径。
    pub fn SetLockFile(&self, ctx: &Context) -> Result<()> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| Error::new("storage not set"))?;
        storage.WriteFile(
            ctx,
            metautil::LockFile,
            b"DO NOT DELETE\nThis file exists to remind other backup jobs won't use this path",
        )
    }

    /// 返回注册 GC safepoint 使用的服务 ID；续跑时复用 checkpoint 中的 ID。
    /// 返回注册到 GC Manager 的 safepoint 标识。
    pub fn GetSafePointID(&self) -> String {
        if let Some(meta) = &self.checkpointMeta {
            return meta.GCServiceId.clone();
        }
        gc::MakeSafePointID()
    }

    /// 设置 GC safepoint TTL；非正数回落到 `DefaultBRGCSafePointTTL`。
    /// 设置 GC TTL；非法值回落到默认，对齐 Go。
    pub fn SetGCTTL(&mut self, mut ttl: i64) {
        if ttl <= 0 {
            ttl = gc::DefaultBRGCSafePointTTL;
        }
        self.gcTTL = ttl;
    }

    /// 读取当前配置的 GC TTL。
    /// 读取当前 GC TTL。
    pub fn GetGCTTL(&self) -> i64 {
        self.gcTTL
    }

    /// 返回已设置的存储后端描述（若有）。
    /// 外部存储后端描述（协议缓冲形态）。
    pub fn GetStorageBackend(&self) -> Option<&backuppb::StorageBackend> {
        self.backend.as_ref()
    }

    /// 返回外部存储句柄的克隆（若已 `SetStorage`）。
    /// 已绑定的外部存储句柄。
    pub fn GetStorage(&self) -> Option<Arc<dyn ExternalStorage>> {
        self.storage.clone()
    }

    /// 设置存储并检查目录未被完整备份占用：
    /// - 若已有 `MetaFile`，认为路径上已有完成备份，拒绝覆盖；
    /// - 若仅有 checkpoint 元数据，则加载并校验锁文件/SST 一致性以支持续跑。
    /// 绑定存储并检查未被其他备份锁定；失败则拒绝继续。
    pub fn SetStorageAndCheckNotInUse(
        &mut self,
        ctx: &Context,
        backend: backuppb::StorageBackend,
        opts: &storeapi::Options,
    ) -> Result<()> {
        self.SetStorage(ctx, backend, opts)?;
        self.CheckStorageNotInUse(ctx)
    }

    /// 检查当前已绑定目录是否可用于新备份或 checkpoint 续跑。
    pub(crate) fn CheckStorageNotInUse(&mut self, ctx: &Context) -> Result<()> {
        let storage = self.storage.as_ref().unwrap();
        let exist = storage.FileExists(ctx, metautil::MetaFile)?;
        if exist {
            return Err(Error::Annotate(
                berrors::ErrInvalidArgument(),
                format!(
                    "backup meta file exists in {}/{}, there may be some backup files in the path already, please specify a correct backup directory!",
                    storage.URI(),
                    metautil::MetaFile
                ),
            ));
        }
        let exist = storage.FileExists(ctx, checkpoint::CheckpointMetaPathForBackup)?;
        if exist {
            // checkpoint 元数据证明这是可续跑目录；Go 明确允许已有 lock/SST。
            self.checkpointMeta = Some(checkpoint::LoadCheckpointMetadata(ctx, storage.as_ref())?);
        } else {
            // 没有 checkpoint 时，lock 与 SST 并存表示目录被不完整任务占用。
            CheckBackupStorageIsLocked(ctx, storage.as_ref())?;
        }
        Ok(())
    }

    /// 校验续跑时配置哈希与 checkpoint 记录一致，防止参数漂移导致数据不完整。
    /// 校验 checkpoint 配置哈希与当前任务一致，防止跨配置续跑。
    pub fn CheckCheckpoint(&self, hash: &[u8]) -> Result<()> {
        if let Some(meta) = &self.checkpointMeta {
            if meta.ConfigHash != hash {
                let uri = self.storage.as_ref().map(|s| s.URI()).unwrap_or_default();
                return Err(Error::Annotate(
                    berrors::ErrInvalidArgument(),
                    format!(
                        "failed to backup to {uri}, because the checkpoint mode is used, but the hashs of the configs are not the same. Please check the config"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 返回正在运行的 checkpoint runner（若已启动）。
    /// 若已启动则返回 checkpoint runner。
    pub fn GetCheckpointRunner(&self) -> Option<Arc<CheckpointRunner>> {
        self.checkpointRunner.clone()
    }

    /// 启动 checkpoint runner：首次写入元数据；续跑则标记需要加载已完成区间图。
    /// `cfgHash`/`backupTS`/`safePointID` 写入元数据，供中断后恢复。
    /// 启动 checkpoint 后台 runner，持久化已完成区间。
    pub fn StartCheckpointRunner(
        &mut self,
        ctx: &Context,
        cfgHash: Vec<u8>,
        backupTS: u64,
        safePointID: String,
        _progressCallBack: impl Fn(ProgressUnit),
    ) -> Result<()> {
        // 首次启用 checkpoint：持久化 GCServiceId/ConfigHash/BackupTS。
        if self.checkpointMeta.is_none() {
            let checkpointMeta = CheckpointMetadataForBackup {
                GCServiceId: safePointID,
                ConfigHash: cfgHash,
                BackupTS: backupTS,
                CheckpointChecksum: None,
                LoadCheckpointDataMap: false,
            };
            let storage = self
                .storage
                .as_ref()
                .ok_or_else(|| Error::new("storage not set"))?;
            checkpoint::SaveCheckpointMetadata(ctx, storage.as_ref(), &checkpointMeta)?;
            self.checkpointMeta = Some(checkpointMeta);
        } else if let Some(meta) = &mut self.checkpointMeta {
            // 续跑：已有元数据，标记后续 BuildProgressRangeTree 需回放已完成区间。
            meta.LoadCheckpointDataMap = true;
        }
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| Error::new("storage not set"))?;
        // 无论首次或续跑，最终都启动异步 runner 以追加完成区间。
        self.checkpointRunner = Some(checkpoint::StartCheckpointRunnerForBackup(
            ctx,
            storage.as_ref(),
            self.cipher.as_ref(),
            self.mgr.GetPDClient().as_ref(),
        )?);
        Ok(())
    }

    /// 等待 checkpoint runner 刷盘结束；`flush=true` 时强制落盘。
    /// 等待 runner 结束；flush 控制是否强制刷盘。
    pub fn WaitForFinishCheckpoint(&self, ctx: &Context, flush: bool) {
        if let Some(runner) = &self.checkpointRunner {
            runner.WaitForFinish(ctx, flush);
        }
    }

    /// 将原始 KeyRange 包装为进度树节点。
    /// 表级备份时从 StartKey 解码 table ID 作为 physicalID，便于按表汇总 checksum。
    /// 将 KeyRange 包装为进度树节点。
    fn getProgressRange(&self, r: KeyRange, _sharedFreeListG: &FreeListG) -> ProgressRange {
        let mut physicalID = 0i64;
        // 非表级备份 physicalID 保持 0，与 Go 全量备份行为一致。
        if self.tableRange {
            physicalID = tablecodec::DecodeTableID(&r.StartKey);
        }
        ProgressRange {
            Res: rtree::NewRangeTreeWithFreeListG(physicalID, _sharedFreeListG),
            Origin: r,
        }
    }

    /// 绑定外部存储后端并创建 `ExternalStorage` 句柄（不做占用检查）。
    /// 仅绑定存储，不做占用检查（调用方需自行保证）。
    pub fn SetStorage(
        &mut self,
        ctx: &Context,
        backend: backuppb::StorageBackend,
        opts: &storeapi::Options,
    ) -> Result<()> {
        self.backend = Some(backend.clone());
        self.storage = Some(objstore::New(ctx, &backend, opts)?);
        Ok(())
    }

    /// 返回构造时从 PD 读取的集群 ID。
    /// PD 集群 ID，写入备份元数据。
    pub fn GetClusterID(&self) -> u64 {
        self.clusterID
    }

    /// 返回当前观测到的 KV API 版本。
    /// KV API 版本。
    pub fn GetApiVersion(&self) -> kvrpcpb::APIVersion {
        self.apiVersion
    }

    /// 由成功备份响应更新 API 版本（V1/V1TTL/V2）。
    /// 覆盖 API 版本（测试或兼容路径）。
    pub fn SetApiVersion(&mut self, v: kvrpcpb::APIVersion) {
        self.apiVersion = v;
    }

    /// 构造备份 key ranges、Schemas 与 PlacementPolicy 列表。
    /// 若存在 checkpoint checksum，注入 Schemas 以便续跑时合并校验和。
    /// 根据过滤与元数据构造备份 ranges 与 schemas。
    pub fn BuildBackupRangeAndSchema(
        &mut self,
        storage: &dyn Storage,
        tableFilter: &dyn Filter,
        backupTS: u64,
        isFullBackup: bool,
        meta_reader: &dyn meta::Reader,
    ) -> Result<(Vec<KeyRange>, Option<Schemas>, Vec<PlacementPolicy>)> {
        let (ranges, mut schemas, policies) = BuildBackupRangeAndInitSchema(
            storage,
            tableFilter,
            backupTS,
            isFullBackup,
            meta_reader,
        )?;
        if let Some(meta) = &self.checkpointMeta {
            if let Some(schemas_ref) = schemas.as_mut() {
                if let Some(cs) = &meta.CheckpointChecksum {
                    // 续跑：把历史 checksum 交给 Schemas，避免重复计算已完成部分。
                    schemas_ref.SetCheckpointChecksum(cs.clone());
                }
            }
        }
        Ok((ranges, schemas, policies))
    }

    /// 根据待备份 ranges 构建全局进度树；若 checkpoint 要求加载数据图，
    /// 则回放已完成区间文件、更新 checksum，并回调 `UnitRegion` 进度。
    /// 用待备份 ranges 初始化全局进度树。
    pub fn BuildProgressRangeTree(
        &self,
        ctx: &Context,
        ranges: Vec<KeyRange>,
        metaWriter: Option<Arc<dyn MetaWriter>>,
        progressCallBack: Arc<dyn Fn(ProgressUnit) + Send + Sync>,
    ) -> Result<ProgressRangeTree> {
        let progressRangeTree = rtree::NewProgressRangeTree(metaWriter.clone(), self.skipChecksum);
        // 与 Go btree FreeList 共享节点池，降低大量 Insert 时的分配开销。
        let sharedFreeListG = FreeListG::new(10240);
        for r in ranges {
            progressRangeTree.Insert(self.getProgressRange(r, &sharedFreeListG))?;
        }
        let completed_callback = progressCallBack.clone();
        progressRangeTree.SetCallBack(move || completed_callback(UnitRange));

        if let Some(meta) = &self.checkpointMeta {
            // 从外部存储遍历 checkpoint 文件，把已完成区间强制写回进度树。
            if meta.LoadCheckpointDataMap {
                let storage = self
                    .storage
                    .as_ref()
                    .ok_or_else(|| Error::new("storage not set"))?;
                let pastDureTime = checkpoint::WalkCheckpointFileForBackup(
                    ctx,
                    storage.as_ref(),
                    self.cipher.as_ref(),
                    |_name, rg| {
                        // 仅当区间仍属于某 Origin 时才回放，避免越界写进度。
                        if let Some(pr) =
                            progressRangeTree.FindContained(&rg.StartKey, &rg.EndKey)?
                        {
                            if pr
                                .Res
                                .PutForce(rg.StartKey.clone(), rg.EndKey.clone(), None, false)
                            {
                                // 把历史文件摘要并入 metaWriter，并按需更新 checksum。
                                let (crc, kvs, bytes) = utils::SummaryFiles(&rg.Files);
                                if let Some(mw) = &metaWriter {
                                    mw.Send(
                                        MetaPayload::Files(rg.Files.clone()),
                                        metautil::AppendDataFile,
                                    )?;
                                }
                                if !self.skipChecksum {
                                    progressRangeTree.UpdateChecksum(
                                        pr.Res.PhysicalID,
                                        crc,
                                        kvs,
                                        bytes,
                                    );
                                }
                                progressCallBack(UnitRegion);
                            }
                        }
                        Ok(())
                    },
                )?;
                // 把任务开始时间回拨，使 summary 耗时包含上次中断前的工作量。
                summary::AdjustStartTimeToEarlierTime(pastDureTime);
            }
        }
        Ok(progressRangeTree)
    }

    /// 备份入口：建进度树、观察 store 变更、组装 `MainBackupLoop` 并 `RunLoop`。
    /// 结束后若仍有未完成区间则报错；成功返回按 physicalID 汇总的 checksum 图。
    /// 备份主入口：建树→观察 store→RunLoop→汇总 checksum。
    /// 数据流对齐 Go BackupRanges；错误向上包装为 Result。
    pub fn BackupRanges(
        &mut self,
        ctx: &Context,
        ranges: Vec<KeyRange>,
        request: BackupRequest,
        concurrency: u32,
        rangeLimit: isize,
        replicaReadLabel: HashMap<String, String>,
        metaWriter: Option<Arc<dyn MetaWriter>>,
        progressCallBack: impl Fn(ProgressUnit) + Send + Sync + 'static,
    ) -> Result<HashMap<i64, ChecksumStats>> {
        let init = Instant::now();
        let progressCallBack: Arc<dyn Fn(ProgressUnit) + Send + Sync> = Arc::new(progressCallBack);
        let globalProgressTree = Arc::new(self.BuildProgressRangeTree(
            ctx,
            ranges,
            metaWriter,
            progressCallBack.clone(),
        )?);

        let (state_tx, state_rx) = mpsc::channel::<BackupRetryPolicy>();
        // 拓扑变化时向 StateNotifier 投递 All/单 store 重试策略。
        ObserveStoreChangesAsync(ctx.clone(), state_tx.clone(), self.mgr.GetPDClient());

        let mgr = self.mgr.clone();
        let mut mainBackupLoop = MainBackupLoop {
            BackupSender: Box::new(MainBackupSender),
            BackupReq: request,
            Concurrency: concurrency,
            GlobalProgressTree: globalProgressTree.clone(),
            ReplicaReadLabel: replicaReadLabel,
            StateNotifier: state_tx,
            state_rx: Some(state_rx),
            Limiter: Arc::new(NewResourceMemoryLimiter(rangeLimit)),
            ProgressCallBack: Box::new(move |unit| progressCallBack(unit)),
            // reset 时强制重建 gRPC，避免沿用故障连接。
            GetBackupClientCallBack: Box::new(move |ctx, storeID, reset| {
                if reset {
                    mgr.ResetBackupClient(ctx, storeID)
                } else {
                    mgr.GetBackupClient(ctx, storeID)
                }
            }),
        };

        self.RunLoop(ctx, &mut mainBackupLoop)?;
        // 与 Go 一致：循环返回后进度树仍非空视为备份未完成。
        if globalProgressTree.Len() > 0 {
            return Err(Error::new(
                "backup ranges done but some ranges are in complete",
            ));
        }
        let _ = init;
        Ok(globalProgressTree.GetChecksumMap())
    }

    /// 列出参与备份的 TiKV store；跳过 TiFlash。
    /// 若配置了副本读标签，则只保留标签匹配的 store，否则报错。
    /// 列出存活 store，并按 ReplicaReadLabel 过滤。
    fn getBackupStores(
        &self,
        ctx: &Context,
        replicaReadLabel: &HashMap<String, String>,
    ) -> Result<Vec<metapb::Store>> {
        let allStores = conn::GetAllTiKVStoresWithRetry(
            ctx,
            self.mgr.GetPDClient().as_ref(),
            conn::util::SkipTiFlash,
        )?;
        // 无标签：所有存活 TiKV 都参与（后续 RunLoop 仍会再查 liveness）。
        if replicaReadLabel.is_empty() {
            return Ok(allStores);
        }
        let mut targetStores = Vec::new();
        for store in allStores {
            for label in &store.Labels {
                if let Some(val) = replicaReadLabel.get(&label.Key) {
                    if val == &label.Value {
                        targetStores.push(store.clone());
                        break;
                    }
                }
            }
        }
        // 标签过滤后为空属于配置错误，避免静默备份到错误拓扑。
        if targetStores.is_empty() {
            return Err(Error::new(format!(
                "no store matches replica read label: {replicaReadLabel:?}"
            )));
        }
        Ok(targetStores)
    }

    /// 处理单个 store 备份响应：成功则记 checkpoint、写入进度树并学习 API 版本；
    /// 若为锁冲突则返回 `Lock` 供轮末集中解析；不可恢复错误则 GiveUp。
    /// 处理单条 store 响应：落盘文件、更新进度、收集锁或错误。
    pub fn OnBackupResponse(
        &mut self,
        _ctx: &Context,
        r: Option<&ResponseAndStore>,
        errContext: &ErrorContext,
        globalProgressTree: &ProgressRangeTree,
    ) -> Result<Option<Lock>> {
        // 汇聚侧关闭标记（None）直接忽略。
        let r = match r {
            Some(r) => r,
            None => return Ok(None),
        };
        let resp = r.GetResponse();
        let storeID = r.GetStoreID();
        // 成功路径：定位所属 ProgressRange，追加 checkpoint 并 Put 文件列表。
        if resp.GetError().is_none() {
            let pr = globalProgressTree.FindContained(&resp.StartKey, &resp.EndKey)?;
            if let Some(pr) = pr {
                // 有 runner 时先持久化完成区间，崩溃后可跳过重复备份。
                if let Some(runner) = &self.checkpointRunner {
                    checkpoint::AppendForBackup(
                        _ctx,
                        runner.as_ref(),
                        &resp.StartKey,
                        &resp.EndKey,
                        &resp.Files,
                    )?;
                }
                // 将响应覆盖的 key 区间与生成的 SST 文件登记进进度树。
                pr.Res.Put(
                    resp.StartKey.clone(),
                    resp.EndKey.clone(),
                    resp.Files.clone(),
                );
                // 响应中的 ApiVersion 数值映射到本地枚举（与 kvproto 约定一致）。
                self.SetApiVersion(match resp.ApiVersion {
                    1 => kvrpcpb::APIVersion::V1TTL,
                    2 => kvrpcpb::APIVersion::V2,
                    _ => kvrpcpb::APIVersion::V1,
                });
            }
        } else {
            let errPb = resp.GetError().unwrap();
            // 优先识别 key locked，交给上层 ResolveLocks，而非立即 GiveUp。
            if let backuppb::ErrorDetail::KvError { KvError: kv } = &errPb.Detail {
                if let Some(lockErr) = &kv.Locked {
                    return Ok(Some(txnlock::NewLock(lockErr)));
                }
            }
            // 按错误策略分类：可重试则吞掉，GiveUp 则包装为 KVStorage 错误返回。
            let res = utils::HandleBackupError(errPb, storeID, errContext);
            if res.Strategy == utils::Strategy::GiveUp {
                return Err(Error::Annotate(
                    berrors::ErrKVStorage(),
                    format!(
                        "error happen in store {storeID}: {}, {}",
                        res.Reason, errPb.Msg
                    ),
                ));
            }
        }
        Ok(None)
    }

    /// RunLoop mirrors Go `Client.RunLoop`.
    ///
    /// 无限轮次备份主循环（对齐 Go 注释）：
    /// - 每轮向所有存活 store 发送当前未完成 SubRanges；
    /// - gRPC 断开/单 store 失败 → 重发该 store；
    /// - 新 store 加入/重启/断开 → 可能全量重发（`All`）；
    /// - 锁错误则轮末解析后下一轮可不 reset 连接。
    /// 理想情况一轮完成；集群状态变化或 KV 错误会触发多轮。
    /// 多轮发送/汇聚主循环：直到进度树清空或上下文取消。
    /// 每轮：刷新 SubRanges→选 store→SendAsync→Collect→OnBackupResponse→解析锁。
    pub fn RunLoop(&mut self, ctx: &Context, loop_: &mut MainBackupLoop) -> Result<()> {
        let mut round = 0u64;
        let mut reset = true;
        // 独占接管重试通知接收端（BackupRanges 里放进 Option）。
        let state_rx = loop_
            .state_rx
            .take()
            .ok_or_else(|| Error::new("state notifier receiver missing"))?;

        // mainLoop：每轮重建 store 连接与汇聚；handleLoop 消费全局响应直到本轮结束。
        'mainLoop: loop {
            round += 1;
            // 防止异常拓扑触发过密轮次；测试可通过 should_skip_round_sleep 跳过。
            if !should_skip_round_sleep() {
                thread::sleep(Duration::from_millis(200));
            }
            // 每轮新建错误上下文，限制同类错误日志刷屏。
            let errContext = NewErrorContext("MainBackupLoop", 10);

            if ctx.Done() {
                return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
            }

            // 无未完成区间 ⇒ 备份完成。
            let incomplete = loop_.GlobalProgressTree.GetIncompleteRanges()?;
            if incomplete.is_empty() {
                return Ok(());
            }
            // 用剩余区间覆盖请求，避免重复备份已完成部分。
            loop_.BackupReq.SubRanges = incomplete;

            // mainCtx 控制本轮发送；handleCtx 控制响应处理；任一取消可打断本轮。
            let (mainCtx, mainCancel) = Context::WithCancel(ctx);
            let (mut handleCtx, mut handleCancel) = Context::WithCancel(ctx);

            // 取 store 失败通常可重试（此前已连过 PD），继续下一轮。
            let allStores = match self.getBackupStores(&mainCtx, &loop_.ReplicaReadLabel) {
                Ok(s) => s,
                Err(_) => {
                    mainCancel.cancel();
                    handleCancel.cancel();
                    reset = true;
                    continue 'mainLoop;
                }
            };

            let (global_tx, mut global_rx) = mpsc::channel::<Option<ResponseAndStore>>();
            let mut storeBackupResultChMap: HashMap<
                u64,
                Arc<Mutex<Receiver<Option<ResponseAndStore>>>>,
            > = HashMap::new();

            for store in &allStores {
                // 不存活的 store 本轮跳过，等待后续轮次或拓扑恢复。
                if utils::CheckStoreLiveness(store).is_err() {
                    continue;
                }
                let storeID = store.GetId();
                let cli = match (loop_.GetBackupClientCallBack)(&mainCtx, storeID, reset) {
                    Ok(c) => c,
                    Err(_) => {
                        mainCancel.cancel();
                        handleCancel.cancel();
                        reset = true;
                        continue 'mainLoop;
                    }
                };
                let (tx, rx) = mpsc::channel();
                storeBackupResultChMap.insert(storeID, Arc::new(Mutex::new(rx)));
                // 异步向该 store 发送；响应进入 store 私有 channel。
                loop_.BackupSender.SendAsync(
                    mainCtx.clone(),
                    round,
                    storeID,
                    loop_.Limiter.clone(),
                    loop_.BackupReq.clone(),
                    loop_.Concurrency,
                    cli,
                    tx,
                    loop_.StateNotifier.clone(),
                );
            }

            // 多路复用各 store 通道到 global_rx，供 handleLoop 统一消费。
            loop_.CollectStoreBackupsAsync(
                handleCtx.clone(),
                round,
                storeBackupResultChMap.clone(),
                global_tx,
            );

            // 本轮累积的锁，在全局通道关闭后一次性 ResolveLocksForRead。
            let mut allTxnLocks: Vec<Lock> = Vec::new();
            let mut last_incomplete_update = Instant::now();

            'handleLoop: loop {
                if ctx.Done() {
                    handleCancel.cancel();
                    mainCancel.cancel();
                    return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
                }

                // 周期性刷新 SubRanges，让发送侧尽快感知新完成的区间。
                // Periodic incomplete range update.
                if last_incomplete_update.elapsed() >= IncompleteRangesUpdateInterval
                    || should_skip_round_sleep()
                {
                    let startUpdate = Instant::now();
                    loop_.BackupReq.SubRanges = loop_.GlobalProgressTree.GetIncompleteRanges()?;
                    let elapsed = startUpdate.elapsed();
                    last_incomplete_update = Instant::now();
                    let _ = elapsed;
                    // 测试加速：伪造「刚更新过」，避免同一轮反复刷新。
                    if should_skip_round_sleep() {
                        // In tests, only update once per handle iteration when skipping sleeps.
                        last_incomplete_update = Instant::now() + IncompleteRangesUpdateInterval;
                    }
                }

                // 非阻塞读取重试策略，避免卡住响应处理。
                // Non-blocking state notifications.
                match state_rx.try_recv() {
                    Ok(storeBackupInfo) => {
                        // 集群级变更：取消本轮，reset 后全量重来。
                        if storeBackupInfo.All {
                            handleCancel.cancel();
                            mainCancel.cancel();
                            reset = true;
                            continue 'mainLoop;
                        }
                        // 单 store 失败：尝试只对该 store 重建连接；简化路径仍可能整轮重启。
                        if storeBackupInfo.One != 0 {
                            let storeID = storeBackupInfo.One;
                            let store = match self.mgr.GetPDClient().GetStore(&mainCtx, storeID) {
                                Ok(s) => s,
                                Err(_) => {
                                    handleCancel.cancel();
                                    mainCancel.cancel();
                                    reset = true;
                                    continue 'mainLoop;
                                }
                            };
                            if utils::CheckStoreLiveness(&store).is_err() {
                                reset = true;
                                continue 'mainLoop;
                            }
                            let cli =
                                match (loop_.GetBackupClientCallBack)(&mainCtx, storeID, reset) {
                                    Ok(c) => c,
                                    Err(_) => {
                                        handleCancel.cancel();
                                        mainCancel.cancel();
                                        reset = true;
                                        continue 'mainLoop;
                                    }
                                };
                            handleCancel.call();
                            let (tx, rx) = mpsc::channel();
                            storeBackupResultChMap.insert(storeID, Arc::new(Mutex::new(rx)));
                            let (global_tx2, global_rx2) =
                                mpsc::channel::<Option<ResponseAndStore>>();
                            loop_.BackupSender.SendAsync(
                                mainCtx.clone(),
                                round,
                                storeID,
                                loop_.Limiter.clone(),
                                loop_.BackupReq.clone(),
                                loop_.Concurrency,
                                cli,
                                tx,
                                loop_.StateNotifier.clone(),
                            );
                            let (handleCtx2, handleCancel2) = Context::WithCancel(&mainCtx);
                            handleCtx = handleCtx2;
                            handleCancel = handleCancel2;
                            loop_.CollectStoreBackupsAsync(
                                handleCtx.clone(),
                                round,
                                storeBackupResultChMap.clone(),
                                global_tx2,
                            );
                            global_rx = global_rx2;
                            continue 'handleLoop;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {}
                }

                match global_rx.recv_timeout(Duration::from_millis(10)) {
                    // 全局通道关闭：若有锁则解析并写入 Resolved/CommittedLocks，然后结束 handleLoop。
                    Ok(None) => {
                        if !allTxnLocks.is_empty() {
                            let bo = utils::AdaptTiKVBackoffer(
                                &handleCtx,
                                MaxResolveLocksbackupOffSleepMs,
                                berrors::ErrUnknown(),
                            );
                            // 用备份 EndVersion 解析读路径锁；成功后下一轮可不 reset。
                            match self.mgr.GetLockResolver().ResolveLocksForRead(
                                bo.Inner(),
                                loop_.BackupReq.EndVersion,
                                &allTxnLocks,
                                true,
                            ) {
                                Ok((_a, ignoreLocks, accessLocks)) => {
                                    if let Some(c) = loop_.BackupReq.Context.as_mut() {
                                        c.ResolvedLocks.extend(ignoreLocks);
                                        c.CommittedLocks.extend(accessLocks);
                                    }
                                }
                                Err(_) => {}
                            }
                            // 锁已解析：保留连接，下一轮继续未完成区间。
                            reset = false;
                        }
                        // 本轮响应流结束；回到 mainLoop 重算 incomplete 洞位。
                        break 'handleLoop;
                    }
                    // 正常响应：把响应子区间写入进度树，下轮重算剩余洞位。
                    Ok(Some(respAndStore)) => {
                        let lock = self.OnBackupResponse(
                            &handleCtx,
                            Some(&respAndStore),
                            &errContext,
                            loop_.GlobalProgressTree.as_ref(),
                        )?;
                        if let Some(lock) = lock {
                            allTxnLocks.push(lock);
                        }
                        (loop_.ProgressCallBack)(UnitRegion);
                    }
                    // 短超时轮询，以便穿插检查取消与状态通知。
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if mainCtx.Done() || handleCtx.Done() {
                            break 'handleLoop;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break 'handleLoop;
                    }
                }
            }
            // handleLoop 结束后自然落入 mainLoop 顶部，重新评估未完成区间。
            // Continue main loop to re-check incomplete ranges.
            let _ = Mutex::new(());
        }
    }
}

/// CheckBackupStorageIsLocked mirrors Go function.
///
/// 若存在锁文件，则遍历目录：发现 `.sst` 却无完整 checkpoint/元数据语义时拒绝，
/// 防止把残留半成品目录当成可续跑的 checkpoint 目录。
/// 若存储已有锁文件则报错，阻止并发写入。
pub fn CheckBackupStorageIsLocked(ctx: &Context, s: &dyn ExternalStorage) -> Result<()> {
    let exist = s.FileExists(ctx, metautil::LockFile)?;
    if exist {
        return s.WalkDir(ctx, &storeapi::WalkOption {}, &mut |path, _size| {
            // 锁+SST 并存但无 checkpoint 元数据 ⇒ 脏目录，要求用户换路径。
            if path.ends_with(".sst") {
                return Err(Error::Annotate(
                    berrors::ErrInvalidArgument(),
                    format!(
                        "backup lock file and sst file exist in {}/{}, there are some backup files in the path already, but hasn't checkpoint metadata, please specify a correct backup directory!",
                        s.URI(),
                        metautil::LockFile
                    ),
                ));
            }
            Ok(())
        });
    }
    Ok(())
}

/// BuildBackupRangeAndInitSchema mirrors Go function.
/// `meta_reader` is injected because Storage snapshots are stubbed locally.
///
/// 按表过滤器扫描库表，生成编码后的 KeyRange，并统计 schemas 数量。
/// 全量备份额外序列化 PlacementPolicy；内存库/模板系统库跳过。
/// 表版本超过 `CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION` 时直接失败。
/// `meta_reader` is injected because Storage snapshots are stubbed locally.
/// 构建 range 并初始化 schema 集合的便捷入口。
pub fn BuildBackupRangeAndInitSchema(
    storage: &dyn Storage,
    tableFilter: &dyn Filter,
    backupTS: u64,
    isFullBackup: bool,
    m: &dyn meta::Reader,
) -> Result<(Vec<KeyRange>, Option<Schemas>, Vec<PlacementPolicy>)> {
    let _ = (storage.GetSnapshot(Version::New(backupTS)), backupTS);

    let mut policies = Vec::new();
    // 全量备份才导出 placement policy，增量场景由调用方决定是否需要。
    if isFullBackup {
        let policyList = m.ListPolicies()?;
        for policyInfo in policyList {
            let p = serde_json::to_vec(&policyInfo)
                .map_err(|e| Error::Trace(Error::new(e.to_string())))?;
            policies.push(PlacementPolicy { Info: p });
        }
    }

    let mut ranges = Vec::new();
    let mut schemasNum = 0isize;
    let dbs = m.ListDatabases()?;

    for dbInfo in &dbs {
        // 过滤：用户未匹配的 schema、内存库、模板系统库均不进入备份集合。
        if !tableFilter.MatchSchema(&dbInfo.Name.O)
            || metadef::IsMemDB(&dbInfo.Name.L)
            || utils::IsTemplateSysDB(&dbInfo.Name)
        {
            continue;
        }
        let mut hasTable = false;
        m.IterTables(dbInfo.ID, &mut |tableInfo| {
            // 新版本表结构字段本 BR 不认识，提示升级 br。
            if tableInfo.Version > version::CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION {
                return Err(Error::new(format!(
                    "backup doesn't not support table {} with version {}, maybe try a new version of br",
                    tableInfo.Name.String(),
                    tableInfo.Version,
                )));
            }
            // 表级过滤：不匹配则跳过，不增加 schemasNum。
            if !tableFilter.MatchTable(&dbInfo.Name.O, &tableInfo.Name.O) {
                return Ok(());
            }
            schemasNum += 1;
            hasTable = true;
            // 将表/索引拆成 key ranges，再经 codec 编码进备份空间。
            let tableRanges = distsql::BuildTableRanges(tableInfo)?;
            for r in tableRanges {
                let (startKey, endKey) = storage
                    .GetCodec()
                    .EncodeRange(&r.StartKey, &r.EndKey);
                ranges.push(KeyRange {
                    StartKey: startKey,
                    EndKey: endKey,
                });
            }
            Ok(())
        })?;
        // 库匹配但无表：仍计 1 个 schema 槽位，对齐 Go 空库也要备份库信息。
        if !hasTable {
            schemasNum += 1;
        }
    }

    // 过滤器筛空：返回空 ranges 与 None schemas，而不是空 Schemas 对象。
    if schemasNum == 0 {
        return Ok((Vec::new(), None, Vec::new()));
    }

    // Go 在 Schemas.BackupSchemas 时重读同一 backupTS 快照。Rust 注入的 Reader
    // 是借用对象，无法放入 `'static` iterFunc，因此先从该不可变快照
    // 完整物化 DB/table 对，并把成功或错误一起延迟到 iterFunc 观察。
    let mut preparedSchemas = Vec::new();
    let preparedResult = BuildBackupSchemas(
        storage,
        tableFilter,
        backupTS,
        isFullBackup,
        m,
        &mut |dbInfo, tableInfo| {
            preparedSchemas.push((dbInfo.clone(), tableInfo.cloned()));
        },
    )
    .map(|_| preparedSchemas);
    let preparedResult = Arc::new(preparedResult);
    let schemas = NewBackupSchemas(
        Arc::new(
            move |_storage: &dyn Storage, fn_| match preparedResult.as_ref() {
                Ok(entries) => {
                    for (dbInfo, tableInfo) in entries {
                        fn_(dbInfo, tableInfo.as_ref());
                    }
                    Ok(())
                }
                Err(err) => Err(err.clone()),
            },
        ),
        schemasNum,
    );
    Ok((ranges, Some(schemas), policies))
}

/// BuildBackupSchemas mirrors Go function.
///
/// 遍历匹配的库表，填充 AutoInc/AutoRand、清理非 public 索引与 placement（非全量），
/// 并通过回调 `fn_` 交给上层写入备份元数据。空库仍回调一次 `(db, None)`。
/// 从库表元数据填充 BackupSchemas。
pub fn BuildBackupSchemas(
    _storage: &dyn Storage,
    tableFilter: &dyn Filter,
    _backupTS: u64,
    isFullBackup: bool,
    m: &dyn meta::Reader,
    fn_: &mut dyn FnMut(&model::DBInfo, Option<&model::TableInfo>),
) -> Result<()> {
    let dbs = m.ListDatabases()?;
    for mut dbInfo in dbs {
        if !tableFilter.MatchSchema(&dbInfo.Name.O)
            || metadef::IsMemDB(&dbInfo.Name.L)
            || utils::IsTemplateSysDB(&dbInfo.Name)
        {
            continue;
        }
        if !isFullBackup {
            // 非全量：去掉库级 placement 引用，减小增量元数据耦合。
            dbInfo.PlacementPolicyRef = None;
        }
        let mut hasTable = false;
        let db_for_cb = dbInfo.clone();
        m.IterTables(dbInfo.ID, &mut |tableInfo| {
            if !tableFilter.MatchTable(&db_for_cb.Name.O, &tableInfo.Name.O) {
                return Ok(());
            }
            let autoIDAccess = m.GetAutoIDAccessors(db_for_cb.ID, tableInfo.ID);
            let mut globalAutoID = 0i64;
            let mut err: Option<Error> = None;
            // 按表类型选择 AutoID 来源：sequence / 分离自增 / 普通 RowID。
            if tableInfo.IsSequence() {
                match autoIDAccess.SequenceValue().Get() {
                    Ok(v) => globalAutoID = v,
                    Err(e) => err = Some(e),
                }
                // 视图或不需要 AutoID 的表：跳过分配。
            } else if tableInfo.IsView() || !utils::NeedAutoID(tableInfo) {
                // no auto ID
            } else if tableInfo.SepAutoInc() {
                match autoIDAccess.IncrementID(tableInfo.Version).Get() {
                    Ok(v) => globalAutoID = v,
                    Err(e) => err = Some(e),
                }
                match autoIDAccess.RowID().Get() {
                    Ok(rowID) => tableInfo.AutoIncIDExtra = rowID + 1,
                    Err(err1) => {
                        if globalAutoID == 0 {
                            return Err(Error::Trace(err1));
                        }
                        let _ = err1;
                    }
                }
            } else {
                match autoIDAccess.RowID().Get() {
                    Ok(v) => globalAutoID = v,
                    Err(e) => err = Some(e),
                }
            }
            if let Some(e) = err {
                return Err(Error::Trace(e));
            }
            // 与 TiDB 惯例一致：备份元数据记录「下一个」可用 ID。
            tableInfo.AutoIncID = globalAutoID + 1;
            if !isFullBackup {
                // 非全量清除表级 placement，避免还原时错误应用旧策略。
                tableInfo.ClearPlacement();
            }
            tableInfo.TableCacheStatusType = model::TableCacheStatusDisable;
            if tableInfo.ContainsAutoRandomBits() {
                let globalAutoRandID = autoIDAccess.RandomID().Get()?;
                tableInfo.AutoRandID = globalAutoRandID + 1;
            }
            // 只保留 StatePublic 索引，与可备份/可还原的稳定结构对齐。
            // remove all non-public indices
            tableInfo
                .Indices
                .retain(|index| index.State == model::StatePublic);
            fn_(&db_for_cb, Some(tableInfo));
            hasTable = true;
            Ok(())
        })?;
        if !hasTable {
            // 空库也要让调用方感知该 DB，以便写入库级元数据。
            fn_(&dbInfo, None);
        }
    }
    Ok(())
}

/// skipUnsupportedDDLJob mirrors Go function.
///
/// 增量备份写 DDL 历史时跳过 placement/attributes 类任务：
/// 这些变更不通过当前 DDL 作业通道完整还原，或由其他机制处理。
/// 过滤备份历史 DDL 时跳过不支持的 job 类型。
pub fn skipUnsupportedDDLJob(job: &Job) -> bool {
    matches!(
        job.Type,
        model::ActionCreatePlacementPolicy
            | model::ActionAlterPlacementPolicy
            | model::ActionDropPlacementPolicy
            | model::ActionAlterTablePartitionPlacement
            | model::ActionModifySchemaDefaultPlacement
            | model::ActionAlterTablePlacement
            | model::ActionAlterTableAttributes
            | model::ActionAlterTablePartitionAttributes
    )
}

/// WriteBackupDDLJobs mirrors Go function.
///
/// 收集 `[lastBackupTS, backupTS]` 窗口内可备份的 DDL 作业并写入 metaWriter。
/// 先扫当前 session 全部作业，再向后翻 history iterator；按 SchemaVersion 过滤，
/// 并清除 job 内嵌 DB/Table 的 placement 信息。最终按 job ID 排序后追加。
/// 将 DDL 历史写入备份存储，供恢复侧回放。
pub fn WriteBackupDDLJobs(
    metaWriter: &dyn MetaWriter,
    g: &dyn Glue,
    store: &dyn Storage,
    lastBackupTS: u64,
    backupTS: u64,
    needDomain: bool,
    lastSnapMeta: &dyn meta::Reader,
    snapMeta: &dyn meta::Reader,
    newestMeta: &dyn meta::Reader,
) -> Result<()> {
    let _ = store.GetSnapshot(Version::New(backupTS));
    let _ = store.GetSnapshot(Version::New(lastBackupTS));
    let lastSchemaVersion = lastSnapMeta.GetSchemaVersionWithNonEmptyDiff()?;
    let backupSchemaVersion = snapMeta.GetSchemaVersionWithNonEmptyDiff()?;
    let version = store.CurrentVersion(crate::stubs::GlobalTxnScope)?;
    let _ = version;
    let _ = newestMeta;

    // 过滤函数：跳过不支持类型；SchemaVersion 过旧则提前结束翻页；
    // 仅保留 Done/Synced 且版本落在 (last, backup] 的作业。
    let appendJobsFn = |jobs: &[Job]| -> (Vec<Job>, bool) {
        let mut appendJobs = Vec::with_capacity(jobs.len());
        for job in jobs {
            // placement/attributes 类作业不写入增量 DDL 流。
            if skipUnsupportedDDLJob(job) {
                continue;
            }
            if let Some(binlog) = &job.BinlogInfo {
                // 版本已不新于上次备份：历史翻页可结束（返回 finished=true）。
                if binlog.SchemaVersion <= lastSchemaVersion {
                    return (appendJobs, true);
                }
                // 只备份已完成且落在版本窗口内的作业，避免未完成 DDL 污染元数据。
                if (job.State == model::JobStateDone || job.State == model::JobStateSynced)
                    && binlog.SchemaVersion > lastSchemaVersion
                    && binlog.SchemaVersion <= backupSchemaVersion
                {
                    let mut job = job.clone();
                    if let Some(db) = job.BinlogInfo.as_mut().and_then(|b| b.DBInfo.as_mut()) {
                        db.PlacementPolicyRef = None;
                    }
                    if let Some(t) = job.BinlogInfo.as_mut().and_then(|b| b.TableInfo.as_mut()) {
                        t.ClearPlacement();
                    }
                    appendJobs.push(job);
                }
            }
        }
        (appendJobs, false)
    };

    let mut allJobs = Vec::new();
    // 一次性 session 拉取当前 DDL 作业列表（是否需要 domain 由调用方决定）。
    g.UseOneShotSession(store, !needDomain, &mut |se| {
        allJobs = ddl::GetAllDDLJobs(&Context::Background(), se.GetSessionCtx())?;
        Ok(())
    })?;

    let (filtered, _) = appendJobsFn(&allJobs);
    allJobs = filtered;

    // 继续向历史翻页，补齐 session 列表可能未覆盖的旧作业。
    let mut historyJobsIter = ddl::GetLastHistoryDDLJobsIterator(newestMeta)?;
    let mut cacheJobs = Vec::new();
    loop {
        cacheJobs = historyJobsIter.GetLastJobs(ddl::DefNumHistoryJobs, cacheJobs)?;
        if cacheJobs.is_empty() {
            break;
        }
        let (jobs, finished) = appendJobsFn(&cacheJobs);
        allJobs.extend(jobs);
        if finished {
            break;
        }
    }

    // 按 ID 排序保证还原侧重放顺序稳定。
    allJobs.sort_by_key(|j| j.ID);
    for job in allJobs {
        let jobBytes =
            serde_json::to_vec(&job).map_err(|e| Error::Trace(Error::new(e.to_string())))?;
        // 以 JSON 字节写入 DDL 追加通道，格式对齐 Go metautil。
        metaWriter.Send(MetaPayload::Bytes(jobBytes), metautil::AppendDDL)?;
    }
    Ok(())
}

// 重导出 Progress，保持与 glue 包对外表面一致（部分调用方从 backup::client 取）。
// silence unused glue import for Progress re-exports
pub use glue::Progress;
