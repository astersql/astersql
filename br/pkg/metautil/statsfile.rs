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

//! Statistics backup/restore file helpers ported from `br/pkg/metautil/statsfile.go`.
//!
//! 本文件负责统计信息（stats）在备份存储中的落盘、内联与恢复加载。
//! 对应 Go `br/pkg/metautil/statsfile.go`：写入侧聚合 JSONTable 到 StatsFile，
//! 按阈值刷盘并生成 StatsFileIndex；读取侧并发下载、校验、解密后投递加载任务。
//! 注释重点说明刷盘阈值、内联短路、校验失败语义以及 rewriteIDMap 与分区物理 ID 的对齐。
//! 与 metafile 的加密/哈希辅助共用，保证 stats 文件与 schema 索引引用一致。
//! 当前 Rust 侧通过 stubs 的 JSON 序列化代替真实 protobuf，行为契约仍以 Go 为准。
//!
//! 关键符号索引：
//! - `maxStatsJsonTableSize` / `inlineSize`：可被测试临时改写的包级阈值。
//! - `StatsWriter`：BackupStats 累积 + BackupStatsDone 收尾刷盘。
//! - `RestoreStats`：下载与并发加载的编排入口，对齐 Go errgroup。
//! - `downloadStats` / `downloadOneStatsFile`：校验、解密、ID rewrite 与任务投递。
//! 阅读时优先核对：何时内联、何时写远端、checksum 作用在明文还是密文、rewrite 缺失如何失败。
//! JSONTable 的全部字段（含直方图载荷、分区与历史标记）必须完整往返。
//! 写入路径错误一律经 Trace 包装；校验失败走 ErrInvalidMetaFile，rewrite 失败走 ErrRestoreInvalidRewrite。
//! 取消语义：ctx 取消后不再派发新下载任务，已在飞的 worker 可提前结束 send。
//! 对象存储写入使用加密后的密文；恢复时必须用 index 中的 cipher_iv 解密后再验哈希。
//! 分区表场景下多个 physicalID 可能落在同一 StatsFile 的不同 block，靠 rewrite map 逐块重映射。
//! 测试通过缩小阈值验证强制刷盘与内联关闭后的远端读写闭环。
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use crate::stubs::{
    JSONTable, PartitionStatisticLoadTask, StatsReadWriter, StatsTypesJSONTable,
    kvproto::brpb::{CipherInfo, StatsBlock, StatsFile, StatsFileIndex},
    protobuf::Message,
};
use astersql_br_pkg_errors::{ErrInvalidMetaFile, ErrRestoreInvalidRewrite};
use astersql_br_pkg_utils::encryption::Decrypt;
use astersql_errors::{ErrorArg, SharedError, Trace};
use astersql_meta_model::TableInfo;
use astersql_objstore_storeapi::{Context, Storage};
use astersql_util::wait_group_wrapper::WaitGroupWrapper;
use astersql_util::worker_pool::NewWorkerPool;

use crate::metafile::{Encrypt, hex_encode, sha256_bytes};

// Mirrors the Go package-level knobs; tests shrink them to force flush paths.
/// 单次 stats 缓冲超过该阈值即刷盘，默认 32MiB；测试可压到 1 强制走写文件路径。
pub static maxStatsJsonTableSize: AtomicUsize = AtomicUsize::new(32 * 1024 * 1024); // 32 MiB
/// 首个且体积小于该阈值的 stats 可内联进 index，避免额外对象存储读写。
pub static inlineSize: AtomicUsize = AtomicUsize::new(8 * 1024); // 8 KiB

/// 统一把底层错误包进 Trace，对齐 Go `errors.Trace`。
fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

/// Keeps the Go 9-digit zero-padded naming rule for stats files.
/// 文件名必须与 Go 的 `%09d` 规则一致，否则 restore 无法按 index.name 找回对象。
pub fn getStatsFileName(physicalID: i64) -> String {
    format!("backupmeta.schema.stats.{:09}", physicalID)
}

/// Serializes the JSON stats payload with the same field names Go emits.
/// 字段名使用 snake_case，与 Go `encoding/json` 标签一致，避免跨语言反序列化失败。
pub fn marshalStatsJSONTable(jsonTable: &JSONTable) -> Result<Vec<u8>, SharedError> {
    serde_json::to_vec(jsonTable).map_err(|err| trace_err(SharedError::new(err)))
}

/// Parses the JSON stats payload back into the shared stats table shape.
/// 完整还原 Go JSONTable 的全部字段，避免统计直方图、分区或历史标记丢失。
pub fn unmarshalStatsJSONTable(bytes: &[u8]) -> Result<StatsTypesJSONTable, SharedError> {
    serde_json::from_slice(bytes).map_err(|err| trace_err(SharedError::new(err)))
}

// A lightweight function wrapper to dump the statistic
/// 轻量 stats 写入器：在内存聚合 StatsBlock，按阈值刷到对象存储并产出索引。
pub struct StatsWriter {
    storage: Arc<dyn Storage + Send + Sync>,
    cipher: Option<CipherInfo>,

    // final stats file indexes
    /// 最终写入 schema 的索引列表，可能含内联数据或远端文件引用。
    statsFileIndexes: Vec<StatsFileIndex>,

    // temporary variables, clear after each flush
    /// 当前缓冲累计字节数，用于与 maxStatsJsonTableSize 比较。
    totalSize: usize,
    /// 尚未刷盘的 StatsFile protobuf（Rust 侧为 JSON 桩）。
    statsFile: StatsFile,
}

/// 构造空写入器；cipher 为 None 时 Encrypt 走明文路径。
pub fn newStatsWriter(
    storage: Arc<dyn Storage + Send + Sync>,
    cipher: Option<CipherInfo>,
) -> StatsWriter {
    StatsWriter {
        storage,
        cipher,
        statsFileIndexes: Vec::new(),
        totalSize: 0,
        statsFile: StatsFile::new(),
    }
}

impl StatsWriter {
    // flush temporary and clear []byte to make it garbage collected as soon as possible
    /// 序列化当前缓冲并立刻清空，便于尽快释放大块 JSON 字节。
    fn flushTemporary(&mut self) -> Result<Vec<u8>, SharedError> {
        let marshaled = self
            .statsFile
            .write_to_bytes()
            .map_err(|err| trace_err(SharedError::new(err)));
        self.clearTemporary();
        marshaled
    }

    /// 重置缓冲状态，对齐 Go 在 flush 后重建空 StatsFile。
    fn clearTemporary(&mut self) {
        // clear the temporary variables
        self.totalSize = 0;
        self.statsFile = StatsFile::new();
    }

    /// 把缓冲写成内联 index 或加密远端文件；成功后缓冲已清空。
    fn writeStatsFileAndClear(
        &mut self,
        ctx: &Context,
        physicalID: i64,
    ) -> Result<(), SharedError> {
        let fileName = getStatsFileName(physicalID);
        let content = self.flushTemporary()?;

        // 仅当还没有任何 index 且体积足够小时才内联，避免后续大文件被错误内联。
        if self.statsFileIndexes.is_empty() && content.len() < inlineSize.load(Ordering::SeqCst) {
            let mut index = StatsFileIndex::new();
            index.set_inline_data(content);
            self.statsFileIndexes.push(index);
            return Ok(());
        }

        // 先算明文校验和，再加密；index 同时记录密文长度与原文长度。
        let checksum = sha256_bytes(&content);
        let sizeOri = content.len() as u64;
        let (encryptedContent, iv) = Encrypt(content, self.cipher.as_ref())?;

        self.storage
            .WriteFile(ctx, &fileName, &encryptedContent)
            .map_err(|err| trace_err(SharedError::new(std::io::Error::other(err.to_string()))))?;

        // 远端文件 index 必须带 name/sha256/iv，恢复时才能定位并验真。
        let mut index = StatsFileIndex::new();
        index.set_name(fileName);
        index.set_sha256(checksum);
        index.set_size_enc(encryptedContent.len() as u64);
        index.set_size_ori(sizeOri);
        index.set_cipher_iv(iv);
        self.statsFileIndexes.push(index);
        Ok(())
    }

    /// 追加一张表/分区的 stats；超过阈值时以当前 physicalID 命名刷盘。
    pub fn BackupStats(
        &mut self,
        ctx: &Context,
        jsonTable: Option<&JSONTable>,
        physicalID: i64,
    ) -> Result<(), SharedError> {
        // Go 对 nil JSONTable 直接返回，避免生成空 block。
        let Some(jsonTable) = jsonTable else {
            return Ok(());
        };

        let statsBytes = marshalStatsJSONTable(jsonTable)?;
        // totalSize 按序列化后字节计，与 Go len(statsBytes) 一致。
        self.totalSize += statsBytes.len();
        let mut block = StatsBlock::new();
        // physical_id 写入 block，恢复时靠它查 rewrite 映射。
        block.set_physical_id(physicalID);
        block.set_json_table(statsBytes);
        self.statsFile.mut_blocks().push(block);

        // check whether need to flush
        // 严格大于阈值才刷，等于阈值仍继续累积。
        if self.totalSize > maxStatsJsonTableSize.load(Ordering::SeqCst) {
            self.writeStatsFileAndClear(ctx, physicalID)?;
        }
        Ok(())
    }

    /// 收尾刷出残余缓冲，并返回完整的 StatsFileIndex 列表供 schema 引用。
    pub fn BackupStatsDone(&mut self, ctx: &Context) -> Result<Vec<StatsFileIndex>, SharedError> {
        if self.totalSize == 0 || self.statsFile.get_blocks().is_empty() {
            return Ok(self.statsFileIndexes.clone());
        }

        // Go 用首个 block 的 physicalID 命名最后一段文件。
        let physicalID = self.statsFile.get_blocks()[0].get_physical_id();
        self.writeStatsFileAndClear(ctx, physicalID)?;
        Ok(self.statsFileIndexes.clone())
    }
}

/// RestoreStats runs the download producer and the concurrent JSON loader just
/// like the Go errgroup pair, propagating the first error from either side.
/// 下载线程与 LoadStatsFromJSONConcurrently 并行；任一侧首错都会向上传播。
pub fn RestoreStats(
    ctx: &Context,
    storage: Arc<dyn Storage + Send + Sync>,
    cipher: Option<CipherInfo>,
    statsHandler: &dyn StatsReadWriter,
    newTableInfo: &TableInfo,
    statsFileIndexes: Vec<StatsFileIndex>,
    rewriteIDMap: HashMap<i64, i64>,
) -> Result<(), SharedError> {
    // 有界通道容量 8，与 Go 侧缓冲一致，背压下载速度。
    let (taskTx, taskRx) = mpsc::sync_channel::<PartitionStatisticLoadTask>(8);
    let download_ctx = ctx.clone();
    let download_handle = thread::spawn(move || {
        downloadStats(
            &download_ctx,
            storage,
            cipher,
            statsFileIndexes,
            rewriteIDMap,
            taskTx,
        )
    });

    // NOTICE: skip updating cache after load stats from json
    // concurrency=0 表示让 handler 自行决定；对齐 Go 跳过 cache 更新的约定。
    let load_result = statsHandler
        .LoadStatsFromJSONConcurrently(newTableInfo, taskRx, 0)
        .map_err(|err| trace_err(SharedError::new(err)));
    let download_result = download_handle.join().unwrap_or_else(|_| {
        Err(SharedError::new(std::io::Error::other(
            "downloadStats worker panicked",
        )))
    });
    // 先报告下载错误再报告加载错误，保持与 errgroup 先失败优先的可观察性。
    download_result?;
    load_result
}

/// Downloads, decrypts and verifies stats files on a bounded worker pool and
/// pushes rewritten partition tasks to `taskCh`. The sender is dropped before
/// returning, mirroring Go's `defer close(taskCh)`.
/// 有界 worker pool 并发拉取各 StatsFileIndex；首错写入共享槽位后停止调度新任务。
pub fn downloadStats(
    ctx: &Context,
    storage: Arc<dyn Storage + Send + Sync>,
    cipher: Option<CipherInfo>,
    statsFileIndexes: Vec<StatsFileIndex>,
    rewriteIDMap: HashMap<i64, i64>,
    taskCh: mpsc::SyncSender<PartitionStatisticLoadTask>,
) -> Result<(), SharedError> {
    // 固定 4 个 worker，与 Go downloadWorkerpool 规模一致。
    let downloadWorkerpool = NewWorkerPool(4, "download stats for each partition".to_string());
    let wg = WaitGroupWrapper::default();
    // 只保留第一个错误，后续失败不覆盖，便于稳定断言。
    let first_err: Arc<std::sync::Mutex<Option<SharedError>>> =
        Arc::new(std::sync::Mutex::new(None));
    let shared_map = Arc::new(rewriteIDMap);
    let shared_cipher = Arc::new(cipher);

    for statsFile in statsFileIndexes {
        // 取消或已有错误时停止派发，但仍会 Wait 已启动 worker。
        if ctx.is_cancelled() || first_err.lock().expect("stats first err lock").is_some() {
            break;
        }
        let ctx = ctx.clone();
        let storage = storage.clone();
        let cipher = shared_cipher.clone();
        let rewriteIDMap = shared_map.clone();
        let taskCh = taskCh.clone();
        let first_err = first_err.clone();
        let wg_pool = downloadWorkerpool.clone();
        wg.Run(move || {
            let worker = wg_pool.ApplyWorker();
            let result =
                downloadOneStatsFile(&ctx, &storage, &cipher, &statsFile, &rewriteIDMap, &taskCh);
            wg_pool.RecycleWorker(worker);
            if let Err(err) = result {
                let mut guard = first_err.lock().expect("stats first err lock");
                if guard.is_none() {
                    *guard = Some(err);
                }
            }
        });
    }

    // Drop the local sender so the receiver terminates once workers finish.
    // 对齐 Go `defer close(taskCh)`：本地 sender drop 后接收端才能 EOF。
    drop(taskCh);
    wg.Wait();
    let mut guard = first_err.lock().expect("stats first err lock");
    match guard.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// 处理单个 StatsFileIndex：内联直读或远端解密校验，再按 rewrite 映射投递加载任务。
fn downloadOneStatsFile(
    ctx: &Context,
    storage: &Arc<dyn Storage + Send + Sync>,
    cipher: &Arc<Option<CipherInfo>>,
    statsFile: &StatsFileIndex,
    rewriteIDMap: &HashMap<i64, i64>,
    taskCh: &mpsc::SyncSender<PartitionStatisticLoadTask>,
) -> Result<(), SharedError> {
    // 内联数据跳过对象存储与加解密，但仍走同一解析/rewrite 路径。
    let statsContent: Vec<u8> = if !statsFile.get_inline_data().is_empty() {
        statsFile.get_inline_data().to_vec()
    } else {
        let content = storage
            .ReadFile(ctx, statsFile.get_name())
            .map_err(|err| trace_err(SharedError::new(std::io::Error::other(err.to_string()))))?;
        let decryptContent = Decrypt(content, cipher.as_ref().as_ref(), statsFile.get_cipher_iv())?;

        // 校验的是明文内容哈希，与写入侧 sha256_bytes(content) 对称。
        let checksum = sha256_bytes(&decryptContent);
        if statsFile.get_sha256() != checksum.as_slice() {
            return Err(
                ErrInvalidMetaFile.GenWithStackByArgs(&[ErrorArg::String(format!(
                    "checksum mismatch expect {}, got {}",
                    hex_encode(statsFile.get_sha256()),
                    hex_encode(&checksum)
                ))]),
            );
        }
        decryptContent
    };

    let mut statsFileBlocks = crate::stubs::protobuf::parse_from_bytes::<StatsFile>(&statsContent)
        .map_err(SharedError::new)?;

    for block in statsFileBlocks.take_blocks().into_iter() {
        let mut block = block;
        // restore 必须能把旧 physical id 映射到新表；缺失规则视为不可恢复错误。
        let Some(physicalId) = rewriteIDMap.get(&block.get_physical_id()).copied() else {
            return Err(
                ErrRestoreInvalidRewrite.GenWithStackByArgs(&[ErrorArg::String(format!(
                    "not rewrite rule matched, old physical id: {}",
                    block.get_physical_id()
                ))]),
            );
        };
        let jsonTable = unmarshalStatsJSONTable(block.get_json_table())?;
        // reset the block.JsonTable to nil to make it garbage collected as soon as possible
        // 提前 take 掉大块 JSON，降低峰值内存，对齐 Go 置 nil 意图。
        block.take_json_table();

        if ctx.is_cancelled() {
            return Ok(());
        }
        // 接收端已关闭时静默退出，避免在取消路径上再报 channel 错误。
        if taskCh
            .send(PartitionStatisticLoadTask {
                PhysicalID: physicalId,
                JSONTable: Some(Box::new(jsonTable)),
            })
            .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}
