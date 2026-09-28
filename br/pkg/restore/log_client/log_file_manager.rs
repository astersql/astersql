// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Log file manager types and pure filter/read helpers, matching `log_file_manager.go`.
//! 日志文件管理器：在给定 StartTS/RestoreTS/shiftStartTS 下过滤并遍历备份元数据与数据文件。
//! 本文件侧重类型与纯过滤/迭代组装；对象存储 walk 在无桩环境用 injected_metas 替代。
//! TS 过滤对 WriteCF/DefaultCF 使用不同下界（startTS vs shiftStartTS），与 Go 一致。
//! MetaVersion>1 时文件 Path 取物理 group 路径，兼容合并后的元数据布局。
//! 迭代器组合依赖 `astersql_br_pkg_utils_iter`，保持惰性求值以降低内存峰值。
//! Stats 使用 Relaxed 原子累加，允许并发 Filter 路径轻微计数偏差换吞吐。
//! getKeyTS 拒绝短于 8 字节的键，错误信息带 hex 便于对照备份脏数据。
//! LoadMigrations/Subcompactions 在 walk 桩上直接从注入切片构造迭代器。
//! Create 后必须调用 loadShiftTS，保证 DefaultCF 下界与迁移构建器同步。
//! FilterDataFiles 丢弃 IsMeta，FilterMetaFiles 只收集 IsMeta，职责互斥。
//! MetaDataGroupName 取首 group 路径，与检查点 groupKey 编码约定对齐。
//! Close 只关 helper；Storage Arc 由调用方持有生命周期。
//! EncryptionManager 字段在 Init 中保留，当前纯过滤路径未直接使用。
//! metadataDownloadBatchSize 供上层下载批大小，本文件过滤逻辑不读取它。
//! TruncateTS/SST 相关 import 供同 crate 扩展点与 Go 文件对齐保留。
//! HashMap import 同理：迁移/扩展路径可能引用，机械翻译阶段保留。
//! 与 Go 对照时优先核对 ShouldFilterOutByTsStatic 三分支。
//! LogDataFileInfo::Default 便于测试快速构造，生产路径应填齐关键字段。
//! GroupIndex/FileIndex 携带 Enumerate 下标，供 OffsetIn* 字段赋值。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use astersql_br_pkg_restore_utils::TruncateTS;
use astersql_br_pkg_utils_iter::{
    CollectAll, ConcatAll, Enumerate, Fail, FilterOut, FlatMap, FromSlice, Indexed, Map, TryNextor,
};

use crate::migration::{WithMigrations, WithMigrationsBuilder};
use crate::ssts::{CompactedSSTs, CopiedSST, SSTs};
use crate::stubs::backuppb::{
    DataFileGroup, DataFileInfo, File, FileType, IngestedSSTs, LogFileSubcompaction,
    LogFileSubcompactionMeta, Metadata, Migration, RewrittenTableID,
};
use crate::stubs::berrors;
use crate::stubs::consts;
use crate::stubs::encryption;
use crate::stubs::kv_entry::Entry;
use crate::stubs::storeapi::Storage;
use crate::stubs::stream::MetadataHelper;
use crate::stubs::{Context, Error, Result, log};
use sha2::{Digest, Sha256};

// 与 Go 类型别名对齐，缩短迭代器签名。
pub type Meta = Metadata;
pub type Log = DataFileInfo;
pub type GroupIndex = Indexed<DataFileGroup>;
pub type FileIndex = Indexed<DataFileInfo>;
pub type GroupIndexIter = Box<dyn TryNextor<GroupIndex>>;
pub type FileIndexIter = Box<dyn TryNextor<FileIndex>>;
// 对外 DML 文件流类型。
pub type LogIter = Box<dyn TryNextor<LogDataFileInfo>>;
pub type MetaNameIter = Box<dyn TryNextor<MetaName>>;
pub type MetaGroupIter = Box<dyn TryNextor<DDLMetaGroup>>;
pub type SubCompactionIter = Box<dyn TryNextor<LogFileSubcompaction>>;
pub type SSTIter = Box<dyn TryNextor<Box<dyn SSTs + Send>>>;

/// 元数据内容 + 存储路径名，供遍历与缓存键使用。
#[derive(Clone, Debug)]
pub struct MetaName {
    pub meta: Meta,
    pub name: String,
}

/// 展平后的日志数据文件视图，附带检查点定位所需的三级偏移。
#[derive(Clone, Debug, Default)]
pub struct LogDataFileInfo {
    // 物理或逻辑路径（视 MetaVersion）。
    pub Path: String,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub Cf: String,
    pub RangeOffset: u64,
    pub Length: u64,
    pub RangeLength: u64,
    pub NumberOfEntries: i64,
    pub TableId: i64,
    pub IsMeta: bool,
    pub Type: crate::stubs::backuppb::FileType,
    pub CompressionType: i32,
    pub Sha256: Vec<u8>,
    pub FileEncryptionInfo: Option<crate::stubs::encryptionpb::FileEncryptionInfo>,
    // 文件覆盖的时间戳闭区间。
    pub MinTs: u64,
    pub MaxTs: u64,
    // 下列三字段供 SkipMap/检查点定位，不来自 protobuf 原始字段。
    pub MetaDataGroupName: String,
    pub OffsetInMetaGroup: i32,
    pub OffsetInMergedGroup: i32,
}

impl LogDataFileInfo {
    /// 从 DataFileInfo 填充，并附上 meta/group/file 偏移。
    pub fn from_data_file(
        f: &DataFileInfo,
        meta_group: &str,
        group_off: i32,
        file_off: i32,
    ) -> Self {
        Self {
            Path: f.Path.clone(),
            StartKey: f.StartKey.clone(),
            EndKey: f.EndKey.clone(),
            Cf: f.Cf.clone(),
            RangeOffset: f.RangeOffset,
            Length: f.Length,
            RangeLength: f.RangeLength,
            NumberOfEntries: f.NumberOfEntries,
            TableId: f.TableId,
            IsMeta: f.IsMeta,
            Type: f.Type,
            CompressionType: f.CompressionType,
            Sha256: f.Sha256.clone(),
            FileEncryptionInfo: f.FileEncryptionInfo.clone(),
            MinTs: f.MinTs,
            MaxTs: f.MaxTs,
            MetaDataGroupName: meta_group.to_string(),
            OffsetInMetaGroup: group_off,
            OffsetInMergedGroup: file_off,
        }
    }

    // 返回校验和副本，避免外部可变借用内部缓冲。
    pub fn GetSha256(&self) -> Vec<u8> {
        self.Sha256.clone()
    }
}

// AppliedFile：供 rewrite/拆分工具按 Start/EndKey 处理。
impl astersql_br_pkg_restore_utils::AppliedFile for LogDataFileInfo {
    fn GetStartKey(&self) -> Vec<u8> {
        self.StartKey.clone()
    }
    fn GetEndKey(&self) -> Vec<u8> {
        self.EndKey.clone()
    }
}

/// 并发安全的文件统计，FilterMetaFiles 路径可原子累加。
#[derive(Default)]
pub struct LogFilesStatistic {
    // 条目/文件数/字节：跨线程累加。
    pub NumEntries: AtomicI64,
    pub NumFiles: AtomicU64,
    pub Size: AtomicU64,
}

/// 同一物理路径下的 DDL/meta 文件集合。
#[derive(Clone, Debug)]
pub struct DDLMetaGroup {
    pub Path: String,
    pub FileMetas: Vec<DataFileInfo>,
}

/// CreateLogFileManager 的入参打包，对应 Go 结构体初始化字段。
pub struct LogFileManagerInit {
    // 恢复窗口 [StartTS, RestoreTS]。
    pub StartTS: u64,
    pub RestoreTS: u64,
    pub Storage: Arc<dyn Storage>,
    pub MigrationsBuilder: WithMigrationsBuilder,
    pub Migrations: WithMigrations,
    pub MetadataDownloadBatchSize: u32,
    pub EncryptionManager: Option<encryption::Manager>,
}

/// 运行时管理器：持有 TS 窗口、存储、迁移与可选统计。
pub struct LogFileManager {
    // shiftStartTS 可能小于 startTS（schema 前移）。
    pub startTS: u64,
    pub restoreTS: u64,
    pub shiftStartTS: u64,
    pub storage: Arc<dyn Storage>,
    pub helper: MetadataHelper,
    pub withMigrationBuilder: WithMigrationsBuilder,
    pub withMigrations: WithMigrations,
    pub metadataDownloadBatchSize: u32,
    pub Stats: Option<Arc<LogFilesStatistic>>,
    /// Injected metas for tests / non-walk environments.
    /// 测试或无 walk 环境注入的元数据列表，替代对象存储枚举。
    pub injected_metas: Vec<MetaName>,
}

/// 构造管理器并加载 shiftStartTS（无 walk 时回退为 startTS）。
pub fn CreateLogFileManager(_ctx: &Context, init: LogFileManagerInit) -> Result<LogFileManager> {
    let mut fm = LogFileManager {
        startTS: init.StartTS,
        restoreTS: init.RestoreTS,
        // 先用 startTS 占位，loadShiftTS 可能再调整。
        shiftStartTS: init.StartTS,
        storage: init.Storage,
        helper: MetadataHelper,
        withMigrationBuilder: init.MigrationsBuilder,
        withMigrations: init.Migrations,
        metadataDownloadBatchSize: init.MetadataDownloadBatchSize,
        Stats: None,
        injected_metas: Vec::new(),
    };
    fm.loadShiftTS()?;
    Ok(fm)
}

impl LogFileManager {
    /// 用迁移列表重建 WithMigrations 视图。
    pub fn BuildMigrations(&mut self, migs: &[Migration]) {
        self.withMigrations = self.withMigrationBuilder.Build(migs);
    }

    /// 返回当前 shiftStartTS（DefaultCF 过滤下界）。
    pub fn ShiftTS(&self) -> u64 {
        self.shiftStartTS
    }

    /// 校验保留最新 MVCC 压缩覆盖是否满足恢复窗口。
    pub fn ValidateRetainLatestMVCCCompactionCoverage(&self, migs: &[Migration]) -> Result<()> {
        self.withMigrationBuilder
            .ValidateRetainLatestMVCCCompactionCoverage(migs)
    }

    fn loadShiftTS(&mut self) -> Result<()> {
        // Without object-store walk, default shiftStartTS = startTS (Go fallback when not found).
        // 无对象存储 walk 时与 Go “找不到则回退”一致。
        self.shiftStartTS = self.startTS;
        self.withMigrationBuilder.SetShiftStartTS(self.shiftStartTS);
        Ok(())
    }

    /// 注入测试元数据；生产路径应由 walk 填充。
    pub fn SetInjectedMetas(&mut self, metas: Vec<MetaName>) {
        self.injected_metas = metas;
    }

    /// 按 TS 窗口过滤 injected_metas：restore < MinTs 或 MaxTs < shift 则剔除。
    pub fn streamingMeta(&self, _ctx: &Context) -> Result<MetaNameIter> {
        let shift = self.shiftStartTS;
        let restore = self.restoreTS;
        let it = FromSlice(self.injected_metas.clone());
        Ok(FilterOut(it, move |metaname: &MetaName| {
            // FilterOut 谓词为 true 表示丢弃。
            restore < metaname.meta.MinTs || metaname.meta.MaxTs < shift
        }))
    }

    /// 展开 DML 数据文件迭代器：套迁移 → 物理/逻辑层 → TS/IsMeta 过滤。
    pub fn FilterDataFiles(&self, m: MetaNameIter) -> LogIter {
        let startTS = self.startTS;
        let restoreTS = self.restoreTS;
        let shiftStartTS = self.shiftStartTS;
        let ms = self.withMigrations.Metas(m);
        FlatMap(ms, move |m: crate::migration::MetaWithMigrations| {
            let meta = m.meta.clone();
            let meta_version = meta.MetaVersion;
            let groups = meta.FileGroups.clone();
            // first_path 用作 MetaDataGroupName（检查点键），取首 group 路径。
            let first_path = groups.first().map(|g| g.Path.clone()).unwrap_or_default();
            let gs = m.Physicals(Enumerate(FromSlice(groups)));
            FlatMap(gs, move |gim: crate::migration::PhysicalWithMigrations| {
                let physical_path = gim.physical.Item.Path.clone();
                let physical_path2 = physical_path.clone();
                let group_index = gim.physical.Index;
                let first_path = first_path.clone();
                let files = gim.physical.Item.DataFilesInfo.clone();
                let fs = FilterOut(
                    gim.Logicals(Enumerate(FromSlice(files))),
                    move |di: &FileIndex| {
                        let mut item = di.Item.clone();
                        // v2+ 元数据：逻辑文件 Path 覆盖为物理路径。
                        if meta_version > 1 {
                            item.Path = physical_path.clone();
                        }
                        // 过滤 meta 文件与 TS 窗口外文件。
                        item.IsMeta
                            || ShouldFilterOutByTsStatic(&item, restoreTS, startTS, shiftStartTS)
                    },
                );
                Map(fs, move |di: FileIndex| {
                    let mut item = di.Item;
                    if meta_version > 1 {
                        item.Path = physical_path2.clone();
                    }
                    LogDataFileInfo::from_data_file(&item, &first_path, group_index, di.Index)
                })
            })
        })
    }

    /// 实例方法包装静态 TS 过滤。
    pub fn ShouldFilterOutByTs(&self, d: &DataFileInfo) -> bool {
        ShouldFilterOutByTsStatic(d, self.restoreTS, self.startTS, self.shiftStartTS)
    }

    /// 加载 DDL 文件列表并初始化 helper 缓存条目。
    pub fn LoadDDLFiles(&self, ctx: &Context) -> Result<Vec<Log>> {
        let m = self.streamingMeta(ctx)?;
        let mg = self.FilterMetaFiles(m);
        self.collectDDLFilesAndPrepareCache(ctx, mg)
    }

    /// 加载 DML 文件惰性迭代器（不立即物化）。
    pub fn LoadDMLFiles(&self, ctx: &Context) -> Result<LogIter> {
        let m = self.streamingMeta(ctx)?;
        Ok(self.FilterDataFiles(m))
    }

    fn collectDDLFilesAndPrepareCache(
        &self,
        ctx: &Context,
        files: MetaGroupIter,
    ) -> Result<Vec<Log>> {
        log::Info("start to collect all ddl files");
        let mut files = files;
        let iter_ctx = astersql_br_pkg_utils_iter::Context::background();
        // 物化全部 DDL 组；任一步失败则带注解返回。
        let fs = CollectAll(&iter_ctx, &mut *files);
        if let Some(err) = fs.Err {
            return Err(Error::Annotatef(
                Error::new(err),
                "failed to collect from files",
            ));
        }
        let mut dataFileInfos = Vec::new();
        for g in fs.Item.unwrap_or_default() {
            // 缓存条目数=可读 meta KV 文件数，供后续并行读取配额。
            self.helper
                .InitCacheEntry(&g.Path, countReadableMetaKVFiles(&g.FileMetas));
            dataFileInfos.extend(g.FileMetas);
        }
        Ok(dataFileInfos)
    }

    /// 过滤出 meta 文件并按 group 聚合；可选累加 Stats。
    pub fn FilterMetaFiles(&self, ms: MetaNameIter) -> MetaGroupIter {
        let startTS = self.startTS;
        let restoreTS = self.restoreTS;
        let shiftStartTS = self.shiftStartTS;
        let stats = self.Stats.clone();
        FlatMap(ms, move |m: MetaName| {
            let meta_version = m.meta.MetaVersion;
            let stats = stats.clone();
            Map(FromSlice(m.meta.FileGroups), move |g: DataFileGroup| {
                let path = g.Path.clone();
                let mut metas = Vec::new();
                for mut d in g.DataFilesInfo {
                    if meta_version > 1 {
                        d.Path = path.clone();
                    }
                    if ShouldFilterOutByTsStatic(&d, restoreTS, startTS, shiftStartTS) {
                        continue;
                    }
                    // 统计包含通过 TS 过滤的全部文件，不仅 meta。
                    if let Some(ref st) = stats {
                        st.NumEntries
                            .fetch_add(d.NumberOfEntries, Ordering::Relaxed);
                        st.NumFiles.fetch_add(1, Ordering::Relaxed);
                        st.Size.fetch_add(d.Length, Ordering::Relaxed);
                    }
                    if d.IsMeta {
                        metas.push(d);
                    }
                }
                DDLMetaGroup {
                    Path: path,
                    FileMetas: metas,
                }
            })
        })
    }

    /// Fetch compacted SST groups produced by log compaction.
    pub fn GetCompactionIter(&self, ctx: &Context) -> SSTIter {
        Map(
            self.withMigrations.Compactions(ctx, self.storage.as_ref()),
            |compaction| Box::new(CompactedSSTs::new(compaction)) as Box<dyn SSTs + Send>,
        )
    }

    /// Flatten ingested SST groups while preserving their rewrite metadata.
    pub fn GetIngestedSSTs(&self, ctx: &Context) -> SSTIter {
        FlatMap(
            self.withMigrations.IngestedSSTs(ctx, self.storage.as_ref()),
            |group: IngestedSSTs| {
                let rewritten = group.Rewritten.unwrap_or_default();
                Map(FromSlice(group.Files), move |file: File| {
                    Box::new(CopiedSST::new(Some(file), rewritten.clone())) as Box<dyn SSTs + Send>
                })
            },
        )
    }

    /// Count all KV pairs represented by compacted and ingested SST files.
    pub fn CountExtraSSTTotalKVs(&self, ctx: &Context) -> Result<i64> {
        let mut count = 0i64;
        let mut groups = ConcatAll(vec![self.GetCompactionIter(ctx), self.GetIngestedSSTs(ctx)]);
        let iter_ctx = astersql_br_pkg_utils_iter::Context::background();
        loop {
            let next = groups.TryNext(&iter_ctx);
            if let Some(err) = next.Err {
                return Err(Error::new(err));
            }
            if next.Finished {
                break;
            }
            if let Some(group) = next.Item {
                for file in group.GetSSTs() {
                    count = count.saturating_add(file.TotalKvs as i64);
                }
            }
        }
        Ok(count)
    }

    /// Load, verify and split meta KV entries exactly as the Go restore path does.
    pub fn ReadFilteredEntriesFromFiles(
        &self,
        ctx: &Context,
        file: &DataFileInfo,
        filter_ts: u64,
    ) -> Result<(Vec<KvEntryWithTS>, Vec<KvEntryWithTS>)> {
        let buff = self.read_file_slice(ctx, file)?;
        let checksum = Sha256::digest(&buff);
        if checksum.as_slice() != file.Sha256.as_slice() {
            return Err(berrors::ErrInvalidArgument(format!(
                "checksum mismatch expect {}, got {}",
                hex::encode(&file.Sha256),
                hex::encode(checksum)
            )));
        }

        let mut kv_entries = Vec::new();
        let mut filtered_out = Vec::new();
        let mut dedup_map: HashMap<Vec<u8>, KvEntryWithTS> = HashMap::new();
        let mut dedup_order = Vec::new();
        let mut pos = 0usize;

        while pos < buff.len() {
            let (key, value, consumed) = decode_kv_entry(&buff[pos..])?;
            pos += consumed;
            if !is_db_or_ddl_job_history_key(&key) {
                continue;
            }
            let ts = getKeyTS(&key)?;
            if ts > self.restoreTS
                || (file.Cf == consts::WriteCF && ts < self.startTS)
                || (file.Cf == consts::DefaultCF && ts < self.shiftStartTS)
                || value.is_empty()
            {
                continue;
            }
            if file.Cf == consts::WriteCF {
                if value.len() < 9 {
                    return Err(berrors::ErrInvalidArgument(format!(
                        "invalid input value, len:{}",
                        value.len()
                    )));
                }
                match value[0] {
                    b'L' | b'R' => continue,
                    b'P' | b'D' => {}
                    other => {
                        return Err(berrors::ErrInvalidArgument(format!(
                            "invalid write type:{}",
                            other as char
                        )));
                    }
                }
            }
            if is_meta_ddl_job_history_key(&key) {
                if file.Cf != consts::WriteCF {
                    append_entry(
                        &mut kv_entries,
                        &mut filtered_out,
                        key,
                        value,
                        ts,
                        filter_ts,
                    );
                }
                continue;
            }
            if is_meta_auto_id_key(&key) {
                let logical = TruncateTS(&key).unwrap_or_else(|| key.clone());
                match dedup_map.get(&logical) {
                    Some(existing) if existing.Ts >= ts => {}
                    Some(_) => {
                        dedup_map.insert(
                            logical,
                            KvEntryWithTS {
                                E: Entry {
                                    Key: key,
                                    Value: value,
                                },
                                Ts: ts,
                            },
                        );
                    }
                    None => {
                        dedup_order.push(logical.clone());
                        dedup_map.insert(
                            logical,
                            KvEntryWithTS {
                                E: Entry {
                                    Key: key,
                                    Value: value,
                                },
                                Ts: ts,
                            },
                        );
                    }
                }
                continue;
            }
            append_entry(
                &mut kv_entries,
                &mut filtered_out,
                key,
                value,
                ts,
                filter_ts,
            );
        }
        for logical in dedup_order {
            if let Some(entry) = dedup_map.remove(&logical) {
                append_entry(
                    &mut kv_entries,
                    &mut filtered_out,
                    entry.E.Key,
                    entry.E.Value,
                    entry.Ts,
                    filter_ts,
                );
            }
        }
        Ok((kv_entries, filtered_out))
    }

    fn read_file_slice(&self, ctx: &Context, file: &DataFileInfo) -> Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(helper) = crate::export_test::helper_for(&self.storage) {
            helper.bump_active();
            let data = helper.Data.lock().unwrap();
            let start = file.RangeOffset as usize;
            let end = start.saturating_add(file.RangeLength as usize);
            if end > data.len() {
                drop(data);
                helper.drop_active();
                return Err(Error::new("stream data out of range"));
            }
            let out = data[start..end].to_vec();
            drop(data);
            helper.wait_gate();
            helper.drop_active();
            return Ok(out);
        }
        let data = self.storage.ReadFile(ctx, &file.Path)?;
        let start = file.RangeOffset as usize;
        let end = start.saturating_add(file.RangeLength as usize);
        if end > data.len() {
            return Err(Error::new("stream data out of range"));
        }
        Ok(data[start..end].to_vec())
    }

    /// 关闭底层 MetadataHelper 资源。
    pub fn Close(&self) {
        self.helper.Close();
    }
}

fn append_entry(
    before: &mut Vec<KvEntryWithTS>,
    after: &mut Vec<KvEntryWithTS>,
    key: Vec<u8>,
    value: Vec<u8>,
    ts: u64,
    filter_ts: u64,
) {
    let entry = KvEntryWithTS {
        E: Entry {
            Key: key,
            Value: value,
        },
        Ts: ts,
    };
    if ts < filter_ts {
        before.push(entry)
    } else {
        after.push(entry)
    }
}

fn decode_kv_entry(buff: &[u8]) -> Result<(Vec<u8>, Vec<u8>, usize)> {
    if buff.len() < 8 {
        return Err(Error::new("invalid buff"));
    }
    let key_len = u32::from_le_bytes(buff[..4].try_into().unwrap()) as usize;
    let value_len_offset = 4usize.saturating_add(key_len);
    if value_len_offset.saturating_add(4) > buff.len() {
        return Err(Error::new("invalid buff"));
    }
    let value_len = u32::from_le_bytes(
        buff[value_len_offset..value_len_offset + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    let end = value_len_offset.saturating_add(4).saturating_add(value_len);
    if end > buff.len() {
        return Err(Error::new("invalid buff"));
    }
    Ok((
        buff[4..value_len_offset].to_vec(),
        buff[value_len_offset + 4..end].to_vec(),
        end,
    ))
}

fn is_db_or_ddl_job_history_key(key: &[u8]) -> bool {
    key.starts_with(b"mD")
}
fn is_meta_ddl_job_history_key(key: &[u8]) -> bool {
    key.starts_with(b"mDDLJobH")
}
fn is_meta_auto_id_key(key: &[u8]) -> bool {
    key.windows(4)
        .any(|w| matches!(w, b"IID:" | b"TID:" | b"SID:"))
        || key.windows(5).any(|w| w == b"TARID")
}

/// 静态 TS 过滤：MinTs>restore，或 WriteCF MaxTs<start，或 DefaultCF MaxTs<shift。
pub fn ShouldFilterOutByTsStatic(
    d: &DataFileInfo,
    restoreTS: u64,
    startTS: u64,
    shiftStartTS: u64,
) -> bool {
    d.MinTs > restoreTS
        || (d.Cf == consts::WriteCF && d.MaxTs < startTS)
        || (d.Cf == consts::DefaultCF && d.MaxTs < shiftStartTS)
}

/// 统计可读取的 meta KV 文件数，规则与 Go `shouldReadMetaKVFile` 一致。
pub fn countReadableMetaKVFiles(files: &[DataFileInfo]) -> i32 {
    files.iter().filter(|f| shouldReadMetaKVFile(f)).count() as i32
}

/// Write CF 的 Put/Delete 都可读；Default CF 的 Delete 必须跳过。
pub fn shouldReadMetaKVFile(file: &DataFileInfo) -> bool {
    if file.Cf == consts::WriteCF {
        return true;
    }
    if file.Type == FileType::Delete {
        return false;
    }
    file.Cf == consts::DefaultCF
}

/// 带提交时间戳的 KV 条目，供过滤/去重逻辑使用。
#[derive(Clone, Debug)]
pub struct KvEntryWithTS {
    pub E: Entry,
    pub Ts: u64,
}

/// 从键尾 8 字节解码降序时间戳（DecodeUintDesc：按位取反大端）。
pub fn getKeyTS(key: &[u8]) -> Result<u64> {
    if key.len() < 8 {
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument("key too short"),
            format!(
                "the length of key is smaller than 8, key:{}",
                hex::encode(key)
            ),
        ));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&key[key.len() - 8..]);
    // DecodeUintDesc: bitwise NOT of big-endian.
    // 与 Go codec.DecodeUintDesc 一致。
    let ts = !u64::from_be_bytes(buf);
    Ok(ts)
}

/// Load subcompaction metadata below `prefix` and apply the Go TS window filter.
pub fn Subcompactions(
    ctx: &Context,
    prefix: &str,
    storage: &dyn Storage,
    shiftStartTS: u64,
    restoredTS: u64,
) -> SubCompactionIter {
    #[derive(serde::Deserialize)]
    struct StoredMeta {
        TableId: i64,
        InputMinTs: u64,
        InputMaxTs: u64,
    }
    #[derive(serde::Deserialize)]
    struct StoredSubcompaction {
        Meta: StoredMeta,
        SstOutputs: Vec<File>,
    }
    #[derive(serde::Deserialize)]
    struct StoredSubcompactions {
        Subcompactions: Vec<StoredSubcompaction>,
    }
    let names = match storage.WalkDir(ctx, prefix) {
        Ok(names) => names,
        Err(err) => return Fail(err.to_string()),
    };
    let mut subs = Vec::new();
    for name in names {
        let bytes = match storage.ReadFile(ctx, &name) {
            Ok(bytes) => bytes,
            Err(err) => return Fail(err.to_string()),
        };
        let group: StoredSubcompactions = match serde_json::from_slice(&bytes) {
            Ok(group) => group,
            Err(err) => return Fail(format!("failed to decode subcompactions {name}: {err}")),
        };
        subs.extend(group.Subcompactions.into_iter().filter_map(|subc| {
            if subc.Meta.InputMaxTs < shiftStartTS || subc.Meta.InputMinTs > restoredTS {
                return None;
            }
            Some(LogFileSubcompaction {
                Meta: LogFileSubcompactionMeta {
                    TableId: subc.Meta.TableId,
                },
                SstOutputs: subc.SstOutputs,
            })
        }));
    }
    FromSlice(subs)
}

// MapFilter 适配：谓词第二项 true 表示过滤掉。
fn MapFilterFromSlice<T: Send + 'static>(
    items: Vec<T>,
    mut f: impl FnMut(T) -> (T, bool) + Send + 'static,
) -> Box<dyn TryNextor<T>> {
    astersql_br_pkg_utils_iter::MapFilter(FromSlice(items), move |x| f(x))
}

/// 从注入的迁移列表构造迭代器（无存储 walk）。
pub fn LoadMigrations(
    _ctx: &Context,
    _s: &dyn Storage,
    migs: Vec<Migration>,
) -> Box<dyn TryNextor<Migration>> {
    FromSlice(migs)
}
