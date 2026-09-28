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

//! Checkpoint Runner 核心：异步聚合、周期刷盘、校验与加锁，对齐 Go `checkpoint.go`。
//!
//! 数据流：调用方 `Append`/`FlushChecksum` → 主循环合并到内存 meta/checksum →
//! tick 触发 flushMeta/flushChecksum/setLock → 刷盘循环经 `checkpointStorage` 落盘。
//! `Flusher` 缓存失败批次并按 retry tick 重试；`WaitForFinish` 可选择最终 flush。
//! 分片读写支持可选 cipher 加密与 SHA256 完整性校验；损坏分片跳过而非失败。
//! 本文件不绑定备份/恢复路径常量，路径由 backup/restore 适配层传入。
//!
//! 并发模型：主循环线程 + 刷盘线程；共享 `RunnerShared`（meta/checksum/err）。
//! 错误模型：首个致命错误写入 err + err_closed，后续 Append 立即失败。
//! 加密：cipher 为 None 时 Encrypt/Decrypt 明文透传；有 cipher 时带 IV。
//! 与 Go 差异：Rust 用 crossbeam 通道；锁粒度用 Mutex/RwLock 表达。
//! 加载侧容忍损坏分片，使部分写失败仍可恢复进度。
//! `ValueMarshaler` 由场景注入（backup/restore JSON 形态不同）。
//! `removeCheckpointData` 失败阈值防止对象存储偶发错误阻断清理。
//! tick/TTL 默认值须与 Go 同名 const 一致，否则锁窗口漂移。
//! GlobalTimer 外部注入（生产取 TSO，测试 MockTimer）；本模块不直接依赖 PD。
//! 主循环 done 前排空 append/checksum，对齐 Go 结束可见性。
//! 刷盘循环 select_timeout 便于响应 ctx 取消。
//! failpoint `failed-after-checkpoint-flushes` 测 incomplete 重试。
//! Checksum 对明文 SHA256；密文与 IV 写入 RangeGroupData。
//! 空 meta/checksum 批次不落盘，减少无意义分片。
//! 锁续期失败视为致命（多 BR 互斥失效）。
//! parse 时 JSON 损坏返回 Ok 跳过；校验失败的组 continue。
//! walk 只处理 `.cpt`；remove 清理 `.cpt`/`.meta`/`.lock`。
//! maxFailedFilesNum=16 与 Go 一致。
//! RangeGroup serde 字段名 `group-key`/`groups` 对齐 Go JSON tag。
//! DureTime 用 duration_ns 序列化，跨语言读写同一纳秒表示。
//! Runner 通道：append/checksum 无界，done/err 有界(1)。
//! metaCh/checksumMetaCh/lockCh 在 start 时 take，防双消费者。
//! WaitForFinish close 存储在 join 之后，避免刷盘仍写时关闭。
//! doFlush 跳过空 Group；整批 RangeGroupMetas 为空则不 WriteFile。
//! send_error_static 与实例 sendError 同幂等语义。
//! Flusher 从尾部重试，近似栈，最近失败优先恢复。
//! updateLock 失败 Annotate 文案与 Go 接近，便于运维检索。
//! loadCheckpointChecksum Walk 全部文件（不限后缀），损坏跳过。
//! saveCheckpointMetadata 不附加校验，依赖存储原子写语义。
//! CheckpointMessage.GroupKey 是归并键，不是 range 起点的别名（虽常相同）。
//! KeyType/ValueType 为 Rust 侧约束；Go 用近似类型集合表达。
//! Append/FlushChecksum 在 err_closed 后 Annotate 前缀区分调用来源。
//! ctx.Done 优先于通道错误，取消语义不被吞掉。
//! startCheckpointMainLoop 把 flush_done JoinHandle 存回共享位供 Wait 汇合。
//! lock_ticker 默认 4min，须小于 lockTimeToLive(5min)，否则锁会过期真空。
//! checksum tick 默认 5s，短于 flush 30s，因校验项更轻量更频繁。
//! retry 默认 3s，给对象存储瞬时错误恢复窗口。
//! incomplete 队列无上限；极端持续失败可能积压内存，与 Go 同风险。
//! parseCheckpointData 解密失败上抛（区别于校验失败 continue）。
//! walk 回调错误中断遍历并上抛，不吞掉存储读失败。
//! remove 先收集路径再删，规避 WalkDir 实现相关的并发修改问题。
//! AtomicI64 计失败文件数，预留并行删除扩展（当前串行）。
//! CipherIv 与密文成对存储，缺 IV 时 Decrypt 无法还原。
//! RangeGroupsEncriptedData 字段名保留 Go 拼写（Encripted）。
//! NowDureTime 记录相对耗时，加载时取最大作为 past 进度。
//! newCheckpointRunner 的 valueMarshaler 以 Arc<dyn Fn> 跨线程共享。
//! errCh 有界 1：只保留首错，避免阻塞发送方。
//! doneCh 有界 1：重复 WaitForFinish 不会再次投递。
//! Select 分支用 Option 包装可选 ticker，Stop 后不再注册。
//! 主循环 break 后统一 Stop 三 ticker，防止泄漏。
//! Flusher.doFlush 失败仍保留原 meta 克隆，供重试同一批次。
//! flushAllIncomplete* 忽略单次错误，尽力而为后依赖上层清理。
//! checkpointStorage.close 在 WaitForFinish 末尾调用，对齐 Go Close。
//! 本文件注释只描述契约与对齐点，不宣称云存储/PD 已完整接入。
//! 阅读建议：先看 CheckpointRunner 字段与 start 主循环，再看 Flusher/parse。
//! 备份/恢复路径常量分别在 backup.rs / restore.rs / log_restore.rs。
//! external_storage 实现 checkpointStorage；本文件只依赖 trait。
//! 测试通过 Start*ForTest 注入短 tick 与 MockTimer，不改默认常量语义。
//! Group 切片 extend 保持 Append 顺序，Walk 侧 Files 次序依赖此约定。
//! HashMap meta 的迭代顺序不保证，落盘分片顺序允许与 Go 不同。
//! 校验失败 continue 而非 Err，是为了脏分片不阻断整体恢复。
//! JSON 反序列化失败 Ok 跳过，兼容写到一半的对象。
//! Encrypt 返回 (buff, iv)；无 cipher 时 iv 可空。
//! Decrypt 需要同一 cipher 与 iv，否则内容损坏或错误。
//! Sha256::digest 结果与 Go crypto/sha256 字节序一致。
//! Uuid 文件名在 external_storage，本文件只组 CheckpointData 字节。
//! RangeType.Files 复用 stubs::File，字段默认由 serde default 填充。
//! ChecksumItem.TableID 为 i64，与 TiDB table id 同宽。
//! pastDureTime 单调抬升，用于选择“最新”进度视图。
//! remove 忽略非目标后缀，避免误删业务快照文件。
//! failedFilesCount 用 SeqCst，保证与阈值比较可见性。
//! Annotate/Annotatef 包装保持错误链，便于上层日志。
//! 主循环对 append/checksum recv Err 直接 break，表示发送端关闭。
//! flush_err_rx 可读到错误时结束，防止半开状态继续 Append。
//! lock 通道发送 `()`，仅作续期脉冲，不含时间戳（时间在存储层取）。
//! checksumMetaCh 与 metaCh 分离，避免大包互相阻塞。
//! RunnerShared.cipher 为 Option，全程只读共享。
//! valueMarshaler 失败会使整批 doFlush 失败并入 incomplete。
//! 本模块不启动异步 runtime；线程模型为 std::thread。
//! 与 storage/ticker/stubs 协作；循环依赖通过 trait 反转。
//! 公开 API 命名保持 Go 风格（驼峰）以降低对照成本。
//! 内部 `_shared`/`_internal` 后缀标记非导出辅助。
//! 修改默认 tick/TTL 必须同步更新测试里对锁窗口的假设。
//! 新增落盘字段时同时更新 parse 与 Go 侧兼容说明。
//! 错误字符串若被测试 `contains` 依赖，改动需同步测试。
//! 完成注释任务不代表生产路径已从桩切换到真实客户端。
//! 变更刷盘格式时需双端（写/读）与 Go 同步 bump 兼容策略。
//!
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Select, Sender, bounded, unbounded};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::stubs::{
    self, CipherInfo, Context, Decrypt, Encrypt, Error, NowDureTime, Result, Storage, WalkOption,
    duration_ns,
};
use crate::ticker::dispatcherTicker;

/// checkpoint 根目录名常量。
pub const CheckpointDir: &str = "checkpoints";

#[derive(Clone, Debug, Default)]
/// 三类落盘路径：data/checksum/lock。
pub struct flushPath {
    pub CheckpointDataDir: String,
    pub CheckpointChecksumDir: String,
    pub CheckpointLockPath: String,
}

/// checksum 成本上限（秒），对齐 Go。
pub const MaxChecksumTotalCost: f64 = 60.0;
/// 默认 meta 刷盘周期（30s）。
pub const defaultTickDurationForFlush: Duration = Duration::from_secs(30);
/// 默认 checksum 刷盘周期（5s）。
pub const defaultTickDurationForChecksum: Duration = Duration::from_secs(5);
/// 默认锁续期周期（4 分钟）。
pub const defaultTickDurationForLock: Duration = Duration::from_secs(4 * 60);
/// 失败分片重试间隔（3s）。
pub const defaultRetryDuration: Duration = Duration::from_secs(3);
/// 锁 TTL（5 分钟）。
pub const lockTimeToLive: Duration = Duration::from_secs(5 * 60);

// Mirrors Go's independent `failed-after-checkpoint-flushes-checksum`
// injection point. The in-crate test hook keeps checksum and data retries
// independently controllable, as they are in the Go implementation.
static FAILED_AFTER_CHECKPOINT_FLUSHES_CHECKSUM: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn set_failed_after_checkpoint_flushes_checksum_for_test(enabled: bool) {
    FAILED_AFTER_CHECKPOINT_FLUSHES_CHECKSUM.store(enabled, Ordering::SeqCst);
}

/// 分组键约束：Eq+Hash+Clone+Send+Sync。
pub trait KeyType: Eq + Hash + Clone + Send + Sync + 'static {}
impl<T> KeyType for T where T: Eq + Hash + Clone + Send + Sync + 'static {}

/// 分组值约束：Clone+Send+Sync。
pub trait ValueType: Clone + Send + Sync + 'static {}
impl<T> ValueType for T where T: Clone + Send + Sync + 'static {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 备份常用 value：起止键 + File 列表。
pub struct RangeType {
    #[serde(default, rename = "StartKey")]
    pub StartKey: Vec<u8>,
    #[serde(default, rename = "EndKey")]
    pub EndKey: Vec<u8>,
    #[serde(default, rename = "Files")]
    pub Files: Vec<stubs::File>,
}

#[derive(Clone, Debug)]
/// Append 消息：按 GroupKey 归并 Value。
pub struct CheckpointMessage<K: KeyType, V: ValueType> {
    pub GroupKey: K,
    pub Group: Vec<V>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// 落盘前分组，JSON 字段对齐 Go。
#[serde(bound(
    serialize = "K: Serialize + Default + PartialEq, V: Serialize",
    deserialize = "K: Deserialize<'de> + Default, V: Deserialize<'de>"
))]
pub struct RangeGroup<K, V> {
    #[serde(rename = "group-key", default, skip_serializing_if = "is_default")]
    pub GroupKey: K,
    #[serde(rename = "groups")]
    pub Group: Vec<V>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// 加密后单组：密文/校验/IV/Size。
pub struct RangeGroupData {
    #[serde(rename = "RangeGroupsEncriptedData")]
    pub RangeGroupsEncriptedData: Vec<u8>,
    #[serde(rename = "Checksum")]
    pub Checksum: Vec<u8>,
    #[serde(rename = "CipherIv")]
    pub CipherIv: Vec<u8>,
    #[serde(rename = "Size")]
    pub Size: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// data flush 信封：DureTime + 多组。
pub struct CheckpointData {
    #[serde(rename = "dure-time", with = "duration_ns")]
    pub DureTime: Duration,
    #[serde(rename = "range-group-metas")]
    pub RangeGroupMetas: Vec<RangeGroupData>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// 单表 checksum 项。
pub struct ChecksumItem {
    #[serde(rename = "table-id")]
    pub TableID: i64,
    #[serde(rename = "crc64-xor")]
    pub Crc64xor: u64,
    #[serde(rename = "total-kvs")]
    pub TotalKvs: u64,
    #[serde(rename = "total-bytes")]
    pub TotalBytes: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// checksum 项列表。
pub struct ChecksumItems {
    #[serde(rename = "checksum-items")]
    pub Items: Vec<ChecksumItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// checksum 分片信封。
pub struct ChecksumInfo {
    #[serde(rename = "content")]
    pub Content: Vec<u8>,
    #[serde(rename = "checksum")]
    pub Checksum: Vec<u8>,
    #[serde(rename = "dure-time", with = "duration_ns")]
    pub DureTime: Duration,
}

/// 桥接 stubs::GlobalTimer。
pub trait GlobalTimer: stubs::GlobalTimer {}
impl<T: stubs::GlobalTimer + ?Sized> GlobalTimer for T {}

/// 落盘与锁抽象。
pub trait checkpointStorage: Send + Sync {
    fn flushCheckpointData(&self, ctx: &Context, data: &[u8]) -> Result<()>;
    fn flushCheckpointChecksum(&self, ctx: &Context, data: &[u8]) -> Result<()>;
    fn initialLock(&self, ctx: &Context) -> Result<()>;
    fn updateLock(&self, ctx: &Context) -> Result<()>;
    fn close(&self);
}

type ValueMarshaler<K, V> = Arc<dyn Fn(&RangeGroup<K, V>) -> Result<Vec<u8>> + Send + Sync>;

/// 主循环/刷盘共享态与错误槽。
struct RunnerShared<K: KeyType, V: ValueType> {
    meta: Mutex<HashMap<K, RangeGroup<K, V>>>,
    checksum: Mutex<ChecksumItems>,
    valueMarshaler: ValueMarshaler<K, V>,
    checkpointStorage: Arc<dyn checkpointStorage>,
    cipher: Option<CipherInfo>,
    err: RwLock<Option<Error>>,
    err_closed: Mutex<bool>,
}

/// 对外 Runner：通道、共享态、线程句柄。
/// start 时 take 接收端；对外保留发送端。
pub struct CheckpointRunner<K: KeyType, V: ValueType> {
    shared: Arc<RunnerShared<K, V>>,
    appendCh: Sender<CheckpointMessage<K, V>>,
    appendRx: Receiver<CheckpointMessage<K, V>>,
    checksumCh: Sender<ChecksumItem>,
    checksumRx: Receiver<ChecksumItem>,
    doneCh: Sender<bool>,
    doneRx: Receiver<bool>,
    metaCh: Mutex<Option<Sender<HashMap<K, RangeGroup<K, V>>>>>,
    metaRx: Mutex<Option<Receiver<HashMap<K, RangeGroup<K, V>>>>>,
    checksumMetaCh: Mutex<Option<Sender<ChecksumItems>>>,
    checksumMetaRx: Mutex<Option<Receiver<ChecksumItems>>>,
    lockCh: Mutex<Option<Sender<()>>>,
    lockRx: Mutex<Option<Receiver<()>>>,
    errCh_tx: Sender<Error>,
    errCh_rx: Receiver<Error>,
    wg: Mutex<Vec<JoinHandle<()>>>,
    done_sent: Mutex<bool>,
}

/// 建通道与空共享态，尚未启动循环。
pub fn newCheckpointRunner<K, V, F>(
    checkpointStorage: Arc<dyn checkpointStorage>,
    cipher: Option<CipherInfo>,
    vm: F,
) -> CheckpointRunner<K, V>
where
    K: KeyType + Default + PartialEq + Serialize + for<'de> Deserialize<'de>,
    V: ValueType + Serialize + for<'de> Deserialize<'de>,
    F: Fn(&RangeGroup<K, V>) -> Result<Vec<u8>> + Send + Sync + 'static,
{
    let (appendCh, appendRx) = unbounded();
    let (checksumCh, checksumRx) = unbounded();
    let (doneCh, doneRx) = bounded(1);
    let (metaCh, metaRx) = unbounded();
    let (checksumMetaCh, checksumMetaRx) = unbounded();
    let (lockCh, lockRx) = unbounded();
    let (errCh_tx, errCh_rx) = bounded(1);
    CheckpointRunner {
        shared: Arc::new(RunnerShared {
            meta: Mutex::new(HashMap::new()),
            checksum: Mutex::new(ChecksumItems { Items: Vec::new() }),
            valueMarshaler: Arc::new(vm),
            checkpointStorage,
            cipher,
            err: RwLock::new(None),
            err_closed: Mutex::new(false),
        }),
        appendCh,
        appendRx,
        checksumCh,
        checksumRx,
        doneCh,
        doneRx,
        metaCh: Mutex::new(Some(metaCh)),
        metaRx: Mutex::new(Some(metaRx)),
        checksumMetaCh: Mutex::new(Some(checksumMetaCh)),
        checksumMetaRx: Mutex::new(Some(checksumMetaRx)),
        lockCh: Mutex::new(Some(lockCh)),
        lockRx: Mutex::new(Some(lockRx)),
        errCh_tx,
        errCh_rx,
        wg: Mutex::new(Vec::new()),
        done_sent: Mutex::new(false),
    }
}

impl<K, V> CheckpointRunner<K, V>
where
    K: KeyType + Default + PartialEq + Serialize + for<'de> Deserialize<'de>,
    V: ValueType + Serialize + for<'de> Deserialize<'de>,
{
    /// 按表字段构造 ChecksumItem 后投递。
    pub fn FlushChecksum(
        &self,
        ctx: &Context,
        tableID: i64,
        crc64xor: u64,
        totalKvs: u64,
        totalBytes: u64,
    ) -> Result<()> {
        self.FlushChecksumItem(
            ctx,
            ChecksumItem {
                TableID: tableID,
                Crc64xor: crc64xor,
                TotalKvs: totalKvs,
                TotalBytes: totalBytes,
            },
        )
    }

    /// 投递 checksum；取消/关闭时快速失败。
    /// 先 try_recv errCh，再查 err_closed。
    pub fn FlushChecksumItem(&self, ctx: &Context, checksumItem: ChecksumItem) -> Result<()> {
        if ctx.Done() {
            return Err(ctx
                .Err()
                .unwrap_or_else(|| Error::new("context cancelled"))
                .Annotatef("failed to append checkpoint checksum item"));
        }
        if let Ok(err) = self.errCh_rx.try_recv() {
            return Err(err);
        }
        if *self.shared.err_closed.lock().unwrap() {
            let err = self
                .shared
                .err
                .read()
                .unwrap()
                .clone()
                .unwrap_or_else(|| Error::new("checkpoint runner closed"));
            return Err(
                err.Annotate("[checkpoint] Checksum: failed to append checkpoint checksum item")
            );
        }
        self.checksumCh
            .send(checksumItem)
            .map_err(|_| Error::new("checksum channel closed"))
    }

    /// 投递消息；关闭/取消时快速失败。
    /// 无界通道，背压在内存 meta。
    pub fn Append(&self, ctx: &Context, message: CheckpointMessage<K, V>) -> Result<()> {
        if ctx.Done() {
            return Err(ctx
                .Err()
                .unwrap_or_else(|| Error::new("context cancelled"))
                .Annotatef("failed to append checkpoint message"));
        }
        if let Ok(err) = self.errCh_rx.try_recv() {
            return Err(err);
        }
        if *self.shared.err_closed.lock().unwrap() {
            let err = self
                .shared
                .err
                .read()
                .unwrap()
                .clone()
                .unwrap_or_else(|| Error::new("checkpoint runner closed"));
            return Err(err.Annotate("[checkpoint] Append: failed to append checkpoint message"));
        }
        self.appendCh
            .send(message)
            .map_err(|_| Error::new("append channel closed"))
    }

    /// 发 done、join 线程并 close 存储。
    /// done 只发送一次（done_sent）。
    pub fn WaitForFinish(&self, _ctx: &Context, flush: bool) {
        {
            let mut sent = self.done_sent.lock().unwrap();
            if !*sent {
                let _ = self.doneCh.send(flush);
                *sent = true;
            }
        }
        let handles: Vec<_> = self.wg.lock().unwrap().drain(..).collect();
        for h in handles {
            let _ = h.join();
        }
        self.shared.checkpointStorage.close();
    }

    /// take 内存 checksum 发到刷盘侧。
    fn flushChecksum_internal(
        shared: &RunnerShared<K, V>,
        checksumMetaCh: &Sender<ChecksumItems>,
        ctx: &Context,
        err_rx: &Receiver<Error>,
    ) -> Result<()> {
        let checksum = {
            let mut guard = shared.checksum.lock().unwrap();
            let items = std::mem::take(&mut guard.Items);
            ChecksumItems { Items: items }
        };
        if ctx.Done() {
            return Err(ctx.Err().unwrap_or_else(|| Error::new("context cancelled")));
        }
        if let Ok(err) = err_rx.try_recv() {
            return Err(err);
        }
        checksumMetaCh
            .send(checksum)
            .map_err(|_| Error::new("checksum meta channel closed"))
    }

    /// take 内存 meta 发到刷盘侧。
    fn flushMeta_internal(
        shared: &RunnerShared<K, V>,
        metaCh: &Sender<HashMap<K, RangeGroup<K, V>>>,
        ctx: &Context,
        err_rx: &Receiver<Error>,
    ) -> Result<()> {
        let meta = {
            let mut guard = shared.meta.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        if ctx.Done() {
            return Err(ctx.Err().unwrap_or_else(|| Error::new("context cancelled")));
        }
        if let Ok(err) = err_rx.try_recv() {
            return Err(err);
        }
        metaCh
            .send(meta)
            .map_err(|_| Error::new("meta channel closed"))
    }

    /// 向锁通道发续期信号。
    fn setLock_internal(
        lockCh: &Sender<()>,
        ctx: &Context,
        err_rx: &Receiver<Error>,
    ) -> Result<()> {
        if ctx.Done() {
            return Err(ctx.Err().unwrap_or_else(|| Error::new("context cancelled")));
        }
        if let Ok(err) = err_rx.try_recv() {
            return Err(err);
        }
        lockCh
            .send(())
            .map_err(|_| Error::new("lock channel closed"))
    }

    /// 幂等记录首个致命错误。
    fn sendError(&self, err: Error) {
        let mut closed = self.shared.err_closed.lock().unwrap();
        if *closed {
            return;
        }
        *self.shared.err.write().unwrap() = Some(err.clone());
        let _ = self.errCh_tx.try_send(err);
        *closed = true;
    }

    /// 主 select：合并消息并按 tick flush/checksum/lock。
    /// 接收端 take 一次；退出 Stop ticker。
    pub fn startCheckpointMainLoop(
        &self,
        ctx: Context,
        tickDurationForFlush: Duration,
        tickDurationForChecksum: Duration,
        tickDurationForLock: Duration,
        retryDuration: Duration,
    ) {
        let shared = self.shared.clone();
        let appendRx = self.appendRx.clone();
        let checksumRx = self.checksumRx.clone();
        let doneRx = self.doneRx.clone();
        let metaCh = self.metaCh.lock().unwrap().take().expect("metaCh");
        let metaRx = self.metaRx.lock().unwrap().take().expect("metaRx");
        let checksumMetaCh = self
            .checksumMetaCh
            .lock()
            .unwrap()
            .take()
            .expect("checksumMetaCh");
        let checksumMetaRx = self
            .checksumMetaRx
            .lock()
            .unwrap()
            .take()
            .expect("checksumMetaRx");
        let lockCh = self.lockCh.lock().unwrap().take().expect("lockCh");
        let lockRx = self.lockRx.lock().unwrap().take().expect("lockRx");
        let errCh_tx = self.errCh_tx.clone();
        let err_closed = self.shared.clone();

        let handle = thread::spawn(move || {
            let flush_done = Arc::new(Mutex::new(None));
            let flush_done2 = flush_done.clone();
            // 刷盘循环：失败入 incomplete。
            let flush_err_rx = startCheckpointFlushLoop(
                ctx.clone(),
                shared.clone(),
                metaRx,
                checksumMetaRx,
                lockRx,
                retryDuration,
                flush_done2,
            );
            // 三套 ticker，测试可缩短。
            let mut flush_ticker = dispatcherTicker(tickDurationForFlush);
            let mut checksum_ticker = dispatcherTicker(tickDurationForChecksum);
            let mut lock_ticker = dispatcherTicker(tickDurationForLock);

            loop {
                if ctx.Done() {
                    if let Some(err) = ctx.Err() {
                        send_error_static(&err_closed, &errCh_tx, err);
                    }
                    break;
                }

                // crossbeam Select 模拟 Go select。
                let mut sel = Select::new();
                let idx_lock = lock_ticker.Ch().map(|rx| sel.recv(rx));
                let idx_checksum = checksum_ticker.Ch().map(|rx| sel.recv(rx));
                let idx_flush = flush_ticker.Ch().map(|rx| sel.recv(rx));
                let idx_append = sel.recv(&appendRx);
                let idx_csum_item = sel.recv(&checksumRx);
                let idx_done = sel.recv(&doneRx);
                let idx_ferr = sel.recv(&flush_err_rx);

                let oper = sel.select();
                let index = oper.index();

                // 锁 tick：续期失败视为致命。
                if Some(index) == idx_lock {
                    let _ = oper.recv(lock_ticker.Ch().unwrap());
                    if let Err(err) = Self::setLock_internal(&lockCh, &ctx, &flush_err_rx) {
                        send_error_static(&err_closed, &errCh_tx, err);
                        break;
                    }
                // checksum tick：批量交出。
                } else if Some(index) == idx_checksum {
                    let _ = oper.recv(checksum_ticker.Ch().unwrap());
                    if let Err(err) =
                        Self::flushChecksum_internal(&shared, &checksumMetaCh, &ctx, &flush_err_rx)
                    {
                        send_error_static(&err_closed, &errCh_tx, err);
                        break;
                    }
                // meta tick：加密落盘。
                } else if Some(index) == idx_flush {
                    let _ = oper.recv(flush_ticker.Ch().unwrap());
                    if let Err(err) =
                        Self::flushMeta_internal(&shared, &metaCh, &ctx, &flush_err_rx)
                    {
                        send_error_static(&err_closed, &errCh_tx, err);
                        break;
                    }
                // Append：按 GroupKey 归并。
                } else if index == idx_append {
                    match oper.recv(&appendRx) {
                        Ok(msg) => {
                            let mut meta = shared.meta.lock().unwrap();
                            let groups =
                                meta.entry(msg.GroupKey.clone())
                                    .or_insert_with(|| RangeGroup {
                                        GroupKey: msg.GroupKey.clone(),
                                        Group: Vec::new(),
                                    });
                            groups.Group.extend(msg.Group);
                        }
                        Err(_) => break,
                    }
                // 单条 checksum 入内存。
                } else if index == idx_csum_item {
                    match oper.recv(&checksumRx) {
                        Ok(msg) => {
                            shared.checksum.lock().unwrap().Items.push(msg);
                        }
                        Err(_) => break,
                    }
                // done：先排空通道，再按需最终 flush。
                // 对齐 Go 结束可见性。
                } else if index == idx_done {
                    let flush = oper.recv(&doneRx).unwrap_or(false);
                    // Drain pending append/checksum messages first so WaitForFinish
                    // observes the same applied state as Go's unbuffered channels.
                    // 排空 append，含尾消息。
                    while let Ok(msg) = appendRx.try_recv() {
                        let mut meta = shared.meta.lock().unwrap();
                        let groups =
                            meta.entry(msg.GroupKey.clone())
                                .or_insert_with(|| RangeGroup {
                                    GroupKey: msg.GroupKey.clone(),
                                    Group: Vec::new(),
                                });
                        groups.Group.extend(msg.Group);
                    }
                    // 排空 checksum。
                    while let Ok(msg) = checksumRx.try_recv() {
                        shared.checksum.lock().unwrap().Items.push(msg);
                    }
                    if flush {
                        let _ = Self::flushMeta_internal(&shared, &metaCh, &ctx, &flush_err_rx);
                        let _ = Self::flushChecksum_internal(
                            &shared,
                            &checksumMetaCh,
                            &ctx,
                            &flush_err_rx,
                        );
                    }
                    // 关闭发送端触发刷盘 EOF。
                    drop(metaCh);
                    drop(checksumMetaCh);
                    drop(lockCh);
                    if let Some(h) = flush_done.lock().unwrap().take() {
                        let _ = h.join();
                    }
                    break;
                // 刷盘致命错误转发并结束。
                } else if index == idx_ferr {
                    if let Ok(err) = oper.recv(&flush_err_rx) {
                        send_error_static(&err_closed, &errCh_tx, err);
                    }
                    break;
                }
            }
            lock_ticker.Stop();
            flush_ticker.Stop();
            checksum_ticker.Stop();
        });
        self.wg.lock().unwrap().push(handle);
    }

    pub fn doChecksumFlush(&self, ctx: &Context, checksumItems: ChecksumItems) -> Result<()> {
        doChecksumFlush_shared(&self.shared, ctx, checksumItems)
    }

    pub fn doFlush(&self, ctx: &Context, meta: HashMap<K, RangeGroup<K, V>>) -> Result<()> {
        doFlush_shared(&self.shared, ctx, meta)
    }
}

// 静态 sendError，供线程闭包使用。
// 仅首个错误生效。
fn send_error_static<K: KeyType, V: ValueType>(
    shared: &Arc<RunnerShared<K, V>>,
    errCh_tx: &Sender<Error>,
    err: Error,
) {
    let mut closed = shared.err_closed.lock().unwrap();
    if *closed {
        return;
    }
    *shared.err.write().unwrap() = Some(err.clone());
    let _ = errCh_tx.try_send(err);
    *closed = true;
}

// Items→ChecksumInfo 落盘；空则 Ok。
fn doChecksumFlush_shared<K: KeyType, V: ValueType>(
    shared: &RunnerShared<K, V>,
    ctx: &Context,
    checksumItems: ChecksumItems,
) -> Result<()> {
    if checksumItems.Items.is_empty() {
        return Ok(());
    }
    let content = serde_json::to_vec(&checksumItems)?;
    let checksum = Sha256::digest(&content);
    let checksumInfo = ChecksumInfo {
        Content: content,
        Checksum: checksum.to_vec(),
        DureTime: NowDureTime(),
    };
    let data = serde_json::to_vec(&checksumInfo)?;
    shared
        .checkpointStorage
        .flushCheckpointChecksum(ctx, &data)?;
    if FAILED_AFTER_CHECKPOINT_FLUSHES_CHECKSUM.load(Ordering::SeqCst) {
        return Err(Error::new(
            "failpoint: failed after checkpoint flushes checksum",
        ));
    }
    Ok(())
}

// 序列化/加密各组后写 CheckpointData。
// 成功后可触发 failpoint。
fn doFlush_shared<K, V>(
    shared: &RunnerShared<K, V>,
    ctx: &Context,
    meta: HashMap<K, RangeGroup<K, V>>,
) -> Result<()>
where
    K: KeyType + Default + PartialEq + Serialize,
    V: ValueType + Serialize,
{
    if meta.is_empty() {
        return Ok(());
    }
    let mut checkpointData = CheckpointData {
        DureTime: NowDureTime(),
        RangeGroupMetas: Vec::with_capacity(meta.len()),
    };
    for group in meta.values() {
        // 空分组不写盘。
        if group.Group.is_empty() {
            continue;
        }
        let content = (shared.valueMarshaler)(group)?;
        let (encryptBuff, iv) = Encrypt(&content, shared.cipher.as_ref())?;
        let checksum = Sha256::digest(&content);
        checkpointData.RangeGroupMetas.push(RangeGroupData {
            RangeGroupsEncriptedData: encryptBuff,
            Checksum: checksum.to_vec(),
            Size: content.len(),
            CipherIv: iv,
        });
    }
    if !checkpointData.RangeGroupMetas.is_empty() {
        let data = serde_json::to_vec(&checkpointData)?;
        shared.checkpointStorage.flushCheckpointData(ctx, &data)?;
    }
    // Go: failpoint.Inject("failed-after-checkpoint-flushes", ...) after successful flush.
    // 对齐 Go：写成功后仍可失败。
    if crate::stubs::failpoint::failed_after_checkpoint_flushes() {
        return Err(Error::new("failpoint: failed after checkpoint flushes"));
    }
    Ok(())
}

// 失败暂存：retry 从尾恢复。
struct Flusher<K: KeyType, V: ValueType> {
    incompleteMetas: Vec<HashMap<K, RangeGroup<K, V>>>,
    incompleteChecksums: Vec<ChecksumItems>,
}

impl<K, V> Flusher<K, V>
where
    K: KeyType + Default + PartialEq + Serialize,
    V: ValueType + Serialize,
{
    fn new() -> Self {
        Self {
            incompleteMetas: Vec::new(),
            incompleteChecksums: Vec::new(),
        }
    }

    fn doFlush(
        &mut self,
        ctx: &Context,
        shared: &RunnerShared<K, V>,
        meta: HashMap<K, RangeGroup<K, V>>,
    ) {
        if let Err(_err) = doFlush_shared(shared, ctx, meta.clone()) {
            self.incompleteMetas.push(meta);
        }
    }

    fn doChecksumFlush(
        &mut self,
        ctx: &Context,
        shared: &RunnerShared<K, V>,
        checksums: ChecksumItems,
    ) {
        if let Err(_err) = doChecksumFlush_shared(shared, ctx, checksums.clone()) {
            self.incompleteChecksums.push(checksums);
        }
    }

    // 优先 meta，其次 checksum。
    fn flushOneIncomplete(&mut self, ctx: &Context, shared: &RunnerShared<K, V>) {
        if !self.incompleteMetas.is_empty() {
            let lastIdx = self.incompleteMetas.len() - 1;
            if doFlush_shared(shared, ctx, self.incompleteMetas[lastIdx].clone()).is_ok() {
                self.incompleteMetas.truncate(lastIdx);
            }
        } else if !self.incompleteChecksums.is_empty() {
            let lastIdx = self.incompleteChecksums.len() - 1;
            if doChecksumFlush_shared(shared, ctx, self.incompleteChecksums[lastIdx].clone())
                .is_ok()
            {
                self.incompleteChecksums.truncate(lastIdx);
            }
        }
    }

    // 关闭时尽力刷完 meta。
    fn flushAllIncompleteMeta(&mut self, ctx: &Context, shared: &RunnerShared<K, V>) {
        for meta in self.incompleteMetas.drain(..) {
            let _ = doFlush_shared(shared, ctx, meta);
        }
    }

    // 关闭时尽力刷完 checksum。
    fn flushAllIncompleteChecksum(&mut self, ctx: &Context, shared: &RunnerShared<K, V>) {
        for checksums in self.incompleteChecksums.drain(..) {
            let _ = doChecksumFlush_shared(shared, ctx, checksums);
        }
    }
}

// 刷盘 select：meta/checksum/lock/retry。
// 三者皆关才退出。
fn startCheckpointFlushLoop<K, V>(
    ctx: Context,
    shared: Arc<RunnerShared<K, V>>,
    metaRx: Receiver<HashMap<K, RangeGroup<K, V>>>,
    checksumMetaRx: Receiver<ChecksumItems>,
    lockRx: Receiver<()>,
    retryDuration: Duration,
    flush_done: Arc<Mutex<Option<JoinHandle<()>>>>,
) -> Receiver<Error>
where
    K: KeyType + Default + PartialEq + Serialize + for<'de> Deserialize<'de>,
    V: ValueType + Serialize + for<'de> Deserialize<'de>,
{
    let (err_tx, err_rx) = bounded(1);
    let handle = thread::spawn(move || {
        let mut flusher = Flusher::<K, V>::new();
        let mut retry_ticker = dispatcherTicker(retryDuration);
        let mut meta_open = true;
        let mut csum_open = true;
        let mut lock_open = true;
        loop {
            if ctx.Done() {
                if let Some(err) = ctx.Err() {
                    let _ = err_tx.try_send(err);
                }
                break;
            }
            // 上游均关，可退出。
            if !meta_open && !csum_open && !lock_open {
                break;
            }

            let mut sel = Select::new();
            let idx_meta = if meta_open {
                Some(sel.recv(&metaRx))
            } else {
                None
            };
            let idx_csum = if csum_open {
                Some(sel.recv(&checksumMetaRx))
            } else {
                None
            };
            let idx_lock = if lock_open {
                Some(sel.recv(&lockRx))
            } else {
                None
            };
            let idx_retry = retry_ticker.Ch().map(|rx| sel.recv(rx));

            let oper = match sel.select_timeout(Duration::from_millis(50)) {
                Ok(op) => op,
                Err(_) => continue,
            };
            let index = oper.index();
            if Some(index) == idx_meta {
                match oper.recv(&metaRx) {
                    // 失败入 incompleteMetas。
                    Ok(meta) => flusher.doFlush(&ctx, &shared, meta),
                    Err(_) => {
                        flusher.flushAllIncompleteMeta(&ctx, &shared);
                        meta_open = false;
                    }
                }
            } else if Some(index) == idx_csum {
                match oper.recv(&checksumMetaRx) {
                    // 失败入 incompleteChecksums。
                    Ok(checksums) => flusher.doChecksumFlush(&ctx, &shared, checksums),
                    Err(_) => {
                        flusher.flushAllIncompleteChecksum(&ctx, &shared);
                        csum_open = false;
                    }
                }
            } else if Some(index) == idx_lock {
                match oper.recv(&lockRx) {
                    Ok(()) => {
                        // 续期失败上报致命错误。
                        if let Err(err) = shared.checkpointStorage.updateLock(&ctx) {
                            let _ =
                                err_tx.try_send(err.Annotate("failed to update checkpoint lock."));
                            break;
                        }
                    }
                    Err(_) => {
                        lock_open = false;
                    }
                }
            } else if Some(index) == idx_retry {
                let _ = oper.recv(retry_ticker.Ch().unwrap());
                // retry tick 恢复一条。
                flusher.flushOneIncomplete(&ctx, &shared);
            }
        }
        retry_ticker.Stop();
    });
    *flush_done.lock().unwrap() = Some(handle);
    err_rx
}

/// 解析 data 分片并回调；损坏跳过。
/// 抬升 pastDureTime。
pub fn parseCheckpointData<K, V, F>(
    content: &[u8],
    pastDureTime: &mut Duration,
    cipher: Option<&CipherInfo>,
    mut fn_: F,
) -> Result<()>
where
    K: KeyType + Default + PartialEq + for<'de> Deserialize<'de>,
    V: ValueType + for<'de> Deserialize<'de>,
    F: FnMut(K, V) -> Result<()>,
{
    let checkpointData: CheckpointData = match serde_json::from_slice(content) {
        Ok(v) => v,
        Err(_) => {
            return Ok(());
        }
    };
    if checkpointData.DureTime > *pastDureTime {
        *pastDureTime = checkpointData.DureTime;
    }
    for meta in checkpointData.RangeGroupMetas {
        let decryptContent = Decrypt(&meta.RangeGroupsEncriptedData, cipher, &meta.CipherIv)?;
        let checksum = Sha256::digest(&decryptContent);
        if meta.Checksum.as_slice() != checksum.as_slice() {
            continue;
        }
        let group: RangeGroup<K, V> = serde_json::from_slice(&decryptContent)?;
        for g in group.Group {
            fn_(group.GroupKey.clone(), g)?;
        }
    }
    Ok(())
}

/// 遍历 `.cpt` 并 parse，返回最大耗时。
pub fn walkCheckpointFile<K, V, F>(
    ctx: &Context,
    s: &dyn Storage,
    cipher: Option<&CipherInfo>,
    subDir: &str,
    mut fn_: F,
) -> Result<Duration>
where
    K: KeyType + Default + PartialEq + for<'de> Deserialize<'de>,
    V: ValueType + for<'de> Deserialize<'de>,
    F: FnMut(K, V) -> Result<()>,
{
    let mut pastDureTime = Duration::ZERO;
    s.WalkDir(
        ctx,
        &WalkOption {
            SubDir: subDir.to_string(),
        },
        &mut |path, _size| {
            if path.ends_with(".cpt") {
                let content = s.ReadFile(ctx, path)?;
                parseCheckpointData::<K, V, _>(&content, &mut pastDureTime, cipher, &mut fn_)?;
            }
            Ok(())
        },
    )?;
    Ok(pastDureTime)
}

/// 读取反序列化单一 meta。
pub fn loadCheckpointMeta<T: for<'de> Deserialize<'de>>(
    ctx: &Context,
    s: &dyn Storage,
    path: &str,
    m: &mut T,
) -> Result<()> {
    let data = s.ReadFile(ctx, path)?;
    *m = serde_json::from_slice(&data)?;
    Ok(())
}

/// 解析 checksum 分片，按 TableID 覆盖写入。
pub fn parseCheckpointChecksum(
    data: &[u8],
    checkpointChecksum: &mut HashMap<i64, ChecksumItem>,
    pastDureTime: &mut Duration,
) -> Result<()> {
    let info: ChecksumInfo = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let checksum = Sha256::digest(&info.Content);
    if info.Checksum.as_slice() != checksum.as_slice() {
        return Ok(());
    }
    if info.DureTime > *pastDureTime {
        *pastDureTime = info.DureTime;
    }
    let items: ChecksumItems = serde_json::from_slice(&info.Content)?;
    for c in items.Items {
        checkpointChecksum.insert(c.TableID, c);
    }
    Ok(())
}

/// Walk 聚合 checksum，返回 map+耗时。
pub fn loadCheckpointChecksum(
    ctx: &Context,
    s: &dyn Storage,
    subDir: &str,
) -> Result<(HashMap<i64, ChecksumItem>, Duration)> {
    let mut pastDureTime = Duration::ZERO;
    let mut checkpointChecksum = HashMap::new();
    s.WalkDir(
        ctx,
        &WalkOption {
            SubDir: subDir.to_string(),
        },
        &mut |path, _size| {
            let data = s.ReadFile(ctx, path)?;
            parseCheckpointChecksum(&data, &mut checkpointChecksum, &mut pastDureTime)?;
            Ok(())
        },
    )?;
    Ok((checkpointChecksum, pastDureTime))
}

/// 序列化 meta 并 WriteFile。
pub fn saveCheckpointMetadata<T: Serialize>(
    ctx: &Context,
    s: &dyn Storage,
    meta: &T,
    path: &str,
) -> Result<()> {
    let data = serde_json::to_vec(meta)?;
    s.WriteFile(ctx, path, &data)
}

/// 删除 checkpoint 后缀文件；失败累计达 16 报错。
/// 先枚举再删。
pub fn removeCheckpointData(ctx: &Context, s: &dyn Storage, subDir: &str) -> Result<()> {
    let mut removedFileNames = Vec::with_capacity(1200);
    let mut removeCnt = 0;
    let mut removeSize: i64 = 0;
    s.WalkDir(
        ctx,
        &WalkOption {
            SubDir: subDir.to_string(),
        },
        &mut |path, size| {
            // 仅清理相关后缀。
            if !(path.ends_with(".cpt") || path.ends_with(".meta") || path.ends_with(".lock")) {
                return Ok(());
            }
            removedFileNames.push(path.to_string());
            removeCnt += 1;
            removeSize += size;
            Ok(())
        },
    )?;
    let _ = (removeCnt, removeSize);
    let maxFailedFilesNum: i64 = 16;
    let failedFilesCount = AtomicI64::new(0);
    let fatal = AtomicBool::new(false);
    let fatal_error = Mutex::new(None);
    let files = Mutex::new(removedFileNames.into_iter());
    let worker_count = 4;

    // Go uses util.NewWorkerPool(4) with an errgroup. Keep the same bounded
    // concurrency and stop taking new work once the 16th deletion fails.
    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    if fatal.load(Ordering::SeqCst) {
                        break;
                    }
                    let Some(name) = files.lock().unwrap().next() else {
                        break;
                    };
                    if let Err(err) = s.DeleteFile(ctx, &name) {
                        if failedFilesCount.fetch_add(1, Ordering::SeqCst) + 1 >= maxFailedFilesNum
                        {
                            let mut first = fatal_error.lock().unwrap();
                            if first.is_none() {
                                *first = Some(err.Annotate("failed to delete too many files"));
                            }
                            fatal.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            });
        }
    });

    if let Some(err) = fatal_error.lock().unwrap().take() {
        return Err(err);
    }
    Ok(())
}
