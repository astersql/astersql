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

//! 日志备份 metadata 集合、截断安全点与 migration 管理。
//! 对应 Go `br/pkg/stream/stream_metas.go`：加载 meta 并计算 shiftTS、
//! 按时间窗口删除数据文件后回写 meta，以及 migrations 目录的合并/追加。
//! shiftTS 优先从带标签的 meta 文件名解析，失败则扫描 FileGroups。
//! Migration 合并按 Path 归并 MetaEdit；名称含序号与内容哈希。
//! 截断删除遵循先物理后 meta，降低悬挂引用窗口。
//! isInsane 防止把备份根或路径穿越当作可销毁前缀。
//! MergeAndMigrateTo 在写 BASE 后清理 id<=seq 的追加文件。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_br_pkg_stream_backupmetas::ShiftTSStatus;
use astersql_br_pkg_utils_consts::DefaultCF;

use crate::stubs::Storage;
use crate::stubs::backuppb::{
    DataFileGroup, DeleteSpansOfFile, MetaEdit, Metadata, Migration, MigrationVersion,
};
use crate::stubs::errors::Error;
use crate::stubs::errors::berrors;

// BASE migration 序号与文件名；meta 后缀用于剥路径。
const baseMigrationSN: i32 = 0;
const baseMigrationName: &str = "BASE";
const metaSuffix: &str = ".meta";
/// 当前支持的 migration 协议版本（M2）。
pub const SupportedMigVersion: MigrationVersion = MigrationVersion::M2;

/// 构造默认 M2 Migration，Creator 占位与 Go 一致。
pub fn NewMigration() -> Migration {
    Migration {
        Version: MigrationVersion::M2,
        Creator: "br;commit=unknown;branch=unknown".into(),
        ..Default::default()
    }
}

/// 截断安全点文件名（历史拼写 trancate 与 Go 保持一致）。
pub const TruncateSafePointFileName: &str = "v1_stream_trancate_safepoint.txt";

/// 已加载的 stream metadata 视图，支持截断删除与回写。
/// DryRun/BeforeDoWriteBack 供测试拦截写回。
pub struct StreamMetadataSet {
    /// 为 true 时上层可跳过真实删除（字段保留对齐 Go）。
    pub DryRun: bool,
    /// path → 摘要信息（MinTS 与各 FileGroup）。
    metadataInfos: HashMap<String, MetadataInfo>,
    /// 下载批次大小提示（与 Go 字段对齐）。
    pub MetadataDownloadBatchSize: u32,
    pub Helper: crate::stream_mgr::MetadataHelper,
    /// 回写前钩子：返回 true 表示跳过本次写/删。
    pub BeforeDoWriteBack: Option<Box<dyn FnMut(&str, &Metadata) -> bool + Send>>,
}

impl Default for StreamMetadataSet {
    fn default() -> Self {
        Self {
            DryRun: false,
            metadataInfos: HashMap::new(),
            MetadataDownloadBatchSize: 128,
            Helper: crate::stream_mgr::NewMetadataHelper(),
            BeforeDoWriteBack: None,
        }
    }
}

/// 单个 FileGroup 的轻量摘要，供截断遍历。
pub struct FileGroupInfo {
    pub MaxTS: u64,
    pub Length: u64,
    pub KVCount: i64,
}

/// 单份 meta 文件的摘要：全局 MinTS + groups。
pub struct MetadataInfo {
    pub MinTS: u64,
    pub FileGroupInfos: Vec<FileGroupInfo>,
}

impl StreamMetadataSet {
    /// 测试导出：只读访问内部 map。
    pub fn TEST_GetMetadataInfos(&self) -> &HashMap<String, MetadataInfo> {
        &self.metadataInfos
    }

    /// 加载 MinTs<=until 的 meta，并计算恢复所需的最小 shiftTS。
    /// 返回值取各文件算出的 shift 与 until 的较小者。
    pub fn LoadUntilAndCalculateShiftTS(
        &mut self,
        s: Arc<dyn Storage>,
        until: u64,
    ) -> Result<u64, Error> {
        let mut metas: HashMap<String, MetadataInfo> = HashMap::new();
        let mut shift_until_ts = until;
        crate::stream_mgr::FastUnmarshalMetaData(s, 0, until, |filename, raw| {
            let m = crate::stream_mgr::MetadataHelper::ParseToMetadataHard(&raw)?;
            // 只缓存窗口内可见的 meta 摘要。
            if m.MinTs <= until {
                let mut file_group_infos = Vec::with_capacity(m.FileGroups.len());
                for group in &m.FileGroups {
                    let mut kv_count = 0i64;
                    // 汇总 entries 作为 KVCount 近似。
                    for file in &group.DataFilesInfo {
                        kv_count += file.NumberOfEntries;
                    }
                    file_group_infos.push(FileGroupInfo {
                        MaxTS: group.MaxTs,
                        Length: group.Length,
                        KVCount: kv_count,
                    });
                }
                metas.insert(
                    filename.clone(),
                    MetadataInfo {
                        MinTS: m.MinTs,
                        FileGroupInfos: file_group_infos,
                    },
                );
            }
            // restoreTS=MAX：仅受 until/start 约束。
            let (ts, ok) = UpdateShiftTS(&filename, &m, until, u64::MAX);
            // 取更保守（更小）的 shift。
            if ok && ts < shift_until_ts {
                shift_until_ts = ts;
            }
            Ok(())
        })?;
        self.metadataInfos = metas;
        Ok(shift_until_ts)
    }

    /// 遍历 MaxTS < before 的 FileGroup；回调返回 true 则提前结束。
    pub fn IterateFilesFullyBefore<F>(&self, before: u64, mut f: F)
    where
        F: FnMut(&FileGroupInfo) -> bool,
    {
        for m in self.metadataInfos.values() {
            for d in &m.FileGroupInfos {
                // 未完全早于 before 的 group 跳过。
                if d.MaxTS >= before {
                    continue;
                }
                if f(d) {
                    return;
                }
            }
        }
    }

    /// 删除 MaxTs < from 的物理文件组，并回写/删除 meta。
    /// 返回未能确认删除的物理路径；警告聚合为错误当 not_deleted 空。
    pub fn RemoveDataFilesAndUpdateMetadataInBatch(
        &mut self,
        from: u64,
        st: Arc<dyn Storage>,
        mut update_fn: impl FnMut(i64),
    ) -> Result<Vec<String>, Error> {
        let mut not_deleted = Vec::new();
        let mut warnings: Vec<Error> = Vec::new();
        let paths: Vec<_> = self.metadataInfos.keys().cloned().collect();
        for path in paths {
            let Some(meta_info) = self.metadataInfos.get(&path) else {
                continue;
            };
            // meta 整体仍新于 from：无可删 group。
            if meta_info.MinTS >= from {
                continue;
            }
            let data = st.ReadFile(&path).map_err(Error::new)?;
            let mut meta = crate::stream_mgr::MetadataHelper::ParseToMetadataHard(&data)?;
            let mut delete_physical = Vec::new();
            for ds in &meta.FileGroups {
                if ds.MaxTs < from {
                    delete_physical.push(ds.Path.clone());
                }
            }
            let count = delete_physical.len() as i64;
            // 先删物理文件，再改 meta，降低悬挂引用窗口。
            for p in &delete_physical {
                if let Err(e) = st.DeleteFile(p) {
                    warnings.push(Error::new(e));
                }
            }
            // 从 meta 去掉已删 group。
            meta.FileGroups
                .retain(|g| !delete_physical.iter().any(|p| p == &g.Path));
            // 兼容 V1 扁平 Files 列表。
            meta.Files
                .retain(|f| !delete_physical.iter().any(|p| p == &f.Path));
            updateMetadataInternalStat(&mut meta);

            // 钩子可跳过写回（测试注入失败等）。
            let skip = if let Some(hook) = self.BeforeDoWriteBack.as_mut() {
                hook(&path, &meta)
            } else {
                false
            };
            if !skip {
                // 空 meta：删文件本身；钩子再通知一次空对象。
                if meta.FileGroups.is_empty() && meta.Files.is_empty() {
                    // 与 Go 删除路径钩子对齐。
                    if let Some(hook) = self.BeforeDoWriteBack.as_mut() {
                        let empty = Metadata::default();
                        hook(&path, &empty);
                    }
                    if let Err(e) = st.DeleteFile(&path) {
                        warnings.push(Error::new(e));
                        not_deleted.extend(delete_physical);
                    }
                } else {
                    let raw = crate::stream_mgr::MetadataHelper::Marshal(&mut meta)?;
                    if let Err(e) = st.WriteFile(&path, &raw) {
                        warnings.push(Error::new(e));
                        not_deleted.extend(delete_physical);
                    }
                }
            }
            update_fn(count);
        }
        // 有警告但物理删都成功：汇总报错。
        if !warnings.is_empty() && not_deleted.is_empty() {
            return Err(Error::new(
                warnings
                    .into_iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        Ok(not_deleted)
    }
}

/// 读取截断安全点文件；不存在返回 0。
pub fn GetTSFromFile(s: &dyn Storage, filename: &str) -> Result<u64, Error> {
    // 缺失视为尚未设置安全点。
    if !s.FileExists(filename).map_err(Error::new)? {
        return Ok(0);
    }
    let data = s.ReadFile(filename).map_err(Error::new)?;
    let s = std::str::from_utf8(&data).map_err(|e| Error::new(format!("{e}")))?;
    s.parse::<u64>().map_err(|_| {
        Error::new("failed to parse the truncate safepoint").Annotate("invalid meta file")
    })
}

/// 将安全点以十进制文本写入存储。
pub fn SetTSToFile(s: &dyn Storage, safepoint: u64, filename: &str) -> Result<(), Error> {
    s.WriteFile(filename, safepoint.to_string().as_bytes())
        .map_err(Error::new)
}

/// 从路径取 basename 去 `.meta` 后解析带标签文件名。
pub fn TryParseTaggedBackupMetaFileNameWrapper(
    filename: &str,
) -> Result<astersql_br_pkg_stream_backupmetas::ParsedName, String> {
    let base = filename
        .rsplit('/')
        .next()
        .unwrap_or(filename)
        // 去掉目录与 .meta 后缀再解析。
        .trim_end_matches(metaSuffix);
    astersql_br_pkg_stream_backupmetas::TryParseTaggedBackupMetaFileName(base)
}

/// 计算 shiftTS：优先文件名标签，否则回退 Metadata 扫描。
/// 返回 (ts, found)。
pub fn UpdateShiftTS(filename: &str, m: &Metadata, startTS: u64, restoreTS: u64) -> (u64, bool) {
    if let Ok(parsedName) = TryParseTaggedBackupMetaFileNameWrapper(filename) {
        let (ts, status) = parsedName.CalculateShiftTS(startTS, restoreTS);
        return match status {
            // 文件名直接给出有效 shift。
            ShiftTSStatus::ShiftTSFound => (ts, true),
            // 标签表明无需 shift。
            ShiftTSStatus::ShiftTSNotFound => (0, false),
            // 统计不可信时回退扫 meta。
            ShiftTSStatus::ShiftTSInvalidStats => UpdateShiftTSFromMetadata(m, startTS, restoreTS),
        };
    }
    UpdateShiftTSFromMetadata(m, startTS, restoreTS)
}

/// 在 WriteCF 文件的 MinBeginTsInDefaultCf 中取最小者作为 shift。
/// meta 与窗口不重叠时返回 (0, false)。
pub fn UpdateShiftTSFromMetadata(m: &Metadata, startTS: u64, restoreTS: u64) -> (u64, bool) {
    // 空或与 [start,restore] 无交集。
    if m.FileGroups.is_empty() || m.MinTs > restoreTS || m.MaxTs < startTS {
        return (0, false);
    }
    let mut minBeginTS = 0;
    let mut isExist = false;
    for ds in &m.FileGroups {
        for d in &ds.DataFilesInfo {
            // 只看 WriteCF 且带 Default 起始 Ts 的文件。
            if d.Cf == DefaultCF || d.MinBeginTsInDefaultCf == 0 {
                continue;
            }
            if d.MinTs > restoreTS || d.MaxTs < startTS {
                continue;
            }
            // 追踪窗口内最小 MinBeginTsInDefaultCf。
            if d.MinBeginTsInDefaultCf < minBeginTS || !isExist {
                isExist = true;
                minBeginTS = d.MinBeginTsInDefaultCf;
            }
        }
    }
    (minBeginTS, isExist)
}

/// 替换 FileGroups 并重算 Min/Max/ResolvedTs。
pub fn ReplaceMetadata(meta: &mut Metadata, filegroups: Vec<DataFileGroup>) {
    meta.FileGroups = filegroups;
    updateMetadataInternalStat(meta);
}

/// 根据 FileGroups 重算 meta 级时间戳统计；空则清零。
fn updateMetadataInternalStat(meta: &mut Metadata) {
    // 无 group：时间字段清零。
    if meta.FileGroups.is_empty() {
        meta.MinTs = 0;
        meta.MaxTs = 0;
        meta.ResolvedTs = 0;
        return;
    }
    meta.MinTs = meta.FileGroups[0].MinTs;
    meta.MaxTs = meta.FileGroups[0].MaxTs;
    meta.ResolvedTs = meta.FileGroups[0].MinResolvedTs;
    for group in &meta.FileGroups {
        if group.MinTs < meta.MinTs {
            meta.MinTs = group.MinTs;
        }
        if group.MaxTs > meta.MaxTs {
            meta.MaxTs = group.MaxTs;
        }
        if group.MinResolvedTs < meta.ResolvedTs {
            meta.ResolvedTs = group.MinResolvedTs;
        }
    }
}

/// 占位类型：表示不使用额外钩子。
pub struct NoHooks;

/// 合并迁移结果：新 BASE 与警告列表。
pub struct MigratedTo {
    pub NewBase: Migration,
    pub Warnings: Vec<Error>,
}

/// `MergeAndMigrateTo` 的返回包装。
pub struct MergeAndMigratedTo {
    pub Migrated: MigratedTo,
}

/// 已加载的 BASE + 按序号追加的 migrations。
pub struct Migrations {
    pub Base: Option<Migration>,
    pub Appended: Vec<(i32, Migration)>,
}

impl Migrations {
    /// 列出 BASE（若有）与全部 Appended。
    pub fn ListAll(&self) -> Vec<Migration> {
        let mut out = Vec::new();
        if let Some(b) = &self.Base {
            out.push(b.clone());
        }
        for (_, m) in &self.Appended {
            out.push(m.clone());
        }
        out
    }

    /// 合并到序号 seq（含）的默认实现。
    pub fn MergeTo(&self, seq: i32) -> Migration {
        self.MergeToBy(seq, MergeMigrations)
    }

    /// 可注入 merge 函数；从 BASE 起折叠 id<=seq 的追加项。
    pub fn MergeToBy(&self, seq: i32, merge: fn(&Migration, &Migration) -> Migration) -> Migration {
        // 无 BASE 时从空 Migration 起合并。
        let mut cur = self.Base.clone().unwrap_or_else(NewMigration);
        for (id, m) in &self.Appended {
            // Appended 已排序，超出即可停止。
            if *id > seq {
                break;
            }
            cur = merge(&cur, m);
        }
        cur
    }
}

/// migrations 目录操作扩展：加载、追加、加锁、合并写回 BASE。
pub struct MigrationExt {
    pub storage: Arc<dyn Storage>,
    pub prefix: String,
    pub skip_locking: bool,
    pub always_run_truncate: bool,
    pub phantom: Vec<Migration>,
    pub interactive: Option<Box<dyn Fn(&Migration) -> bool + Send>>,
}

/// 默认前缀 `v1/migrations`。
pub fn MigrationExtension(s: Arc<dyn Storage>) -> MigrationExt {
    MigrationExt {
        storage: s,
        prefix: "v1/migrations".into(),
        skip_locking: false,
        always_run_truncate: false,
        phantom: Vec::new(),
        interactive: None,
    }
}

/// 合并迁移前的一次性选项闭包。
pub type MergeAndMigrateToOpt = Box<dyn FnOnce(&mut MigrationExt)>;

/// 测试选项：跳过远程锁。
pub fn MMOptSkipLockingInTest() -> MergeAndMigrateToOpt {
    Box::new(|m| m.skip_locking = true)
}

/// 测试选项：总是执行 truncate 相关路径。
pub fn MMOptAlwaysRunTruncate() -> MergeAndMigrateToOpt {
    Box::new(|m| m.always_run_truncate = true)
}

/// 合并时附加内存中的 phantom migration（不落盘）。
pub fn MMOptAppendPhantomMigration(migs: Vec<Migration>) -> MergeAndMigrateToOpt {
    Box::new(move |m| m.phantom.extend(migs))
}

/// 写回前交互确认；返回 false 则 abort。
pub fn MMOptInteractiveCheck(
    f: impl Fn(&Migration) -> bool + Send + 'static,
) -> MergeAndMigrateToOpt {
    Box::new(move |m| m.interactive = Some(Box::new(f)))
}

/// 合并两次 migration：EditMeta 按 Path 归并，其余列表拼接，TruncatedTo 取 max。
pub fn MergeMigrations(m1: &Migration, m2: &Migration) -> Migration {
    let mut out = NewMigration();
    out.EditMeta = mergeMetaEdits(m1.GetEditMeta(), m2.GetEditMeta());
    out.Compactions.extend_from_slice(m1.GetCompactions());
    out.Compactions.extend_from_slice(m2.GetCompactions());
    // 截断点取较大，表示更激进的已截断进度。
    out.TruncatedTo = m1.GetTruncatedTo().max(m2.GetTruncatedTo());
    out.DestructPrefix.extend_from_slice(m1.GetDestructPrefix());
    out.DestructPrefix.extend_from_slice(m2.GetDestructPrefix());
    out.IngestedSstPaths
        .extend_from_slice(m1.GetIngestedSstPaths());
    out.IngestedSstPaths
        .extend_from_slice(m2.GetIngestedSstPaths());
    out
}

/// 同 Path 的删除列表合并；逻辑删除再走 span 合并。
fn mergeMetaEdits(s1: &[MetaEdit], s2: &[MetaEdit]) -> Vec<MetaEdit> {
    let mut edits: HashMap<String, MetaEdit> = HashMap::new();
    for edit in s1 {
        edits.insert(
            edit.GetPath().to_string(),
            MetaEdit {
                Path: edit.Path.clone(),
                DeletePhysicalFiles: edit.DeletePhysicalFiles.clone(),
                DeleteLogicalFiles: edit.DeleteLogicalFiles.clone(),
                // Go intentionally rebuilds left-hand edits from only the
                // deletion lists. DestructSelf is not carried across merges.
                DestructSelf: false,
            },
        );
    }
    for edit in s2 {
        let path = edit.GetPath().to_string();
        // 同 Path：追加物理删除并合并逻辑删除。
        if let Some(target) = edits.get_mut(&path) {
            target
                .DeletePhysicalFiles
                .extend_from_slice(edit.GetDeletePhysicalFiles());
            target.DeleteLogicalFiles =
                mergeDeleteLogicalFiles(&target.DeleteLogicalFiles, edit.GetDeleteLogicalFiles());
        } else {
            edits.insert(path, edit.clone());
        }
    }
    edits.into_values().collect()
}

/// 同文件 Path 的 span 列表追加合并。
fn mergeDeleteLogicalFiles(
    s1: &[DeleteSpansOfFile],
    s2: &[DeleteSpansOfFile],
) -> Vec<DeleteSpansOfFile> {
    let mut files: HashMap<String, DeleteSpansOfFile> = HashMap::new();
    for file in s1 {
        files.insert(
            file.GetPath().to_string(),
            DeleteSpansOfFile {
                Path: file.Path.clone(),
                Spans: file.Spans.clone(),
                WholeFileLength: file.WholeFileLength,
            },
        );
    }
    for file in s2 {
        let path = file.GetPath().to_string();
        // 同文件追加 span。
        if let Some(target) = files.get_mut(&path) {
            target.Spans.extend_from_slice(file.GetSpans());
        } else {
            files.insert(path, file.clone());
        }
    }
    files.into_values().collect()
}

/// MetaEdit 是否无任何删除/自毁动作。
pub fn isEmptyEdition(medit: &MetaEdit) -> bool {
    medit.DeletePhysicalFiles.is_empty()
        && medit.DeleteLogicalFiles.is_empty()
        && !medit.DestructSelf
}

/// 从 migration 文件名解析序号；BASE→0；前 8 字符为十进制。
pub fn migIdOf(s: &str) -> Result<i32, Error> {
    const migrationPrefixLen: usize = 8;
    // 特殊名 BASE。
    if s == baseMigrationName {
        return Ok(baseMigrationSN);
    }
    // 正常名至少 8 位序号前缀。
    if s.len() < migrationPrefixLen {
        return Err(Error::new(format!(
            "migration name {s} is too short, perhaps `migrations` dir corrupted"
        ))
        .Annotate(berrors::ErrUnknown));
    }
    s[..migrationPrefixLen].parse::<i32>().map_err(|err| {
        Error::new(format!(
            "migration name {s} is not a valid number, perhaps `migrations` dir corrupted: {err}"
        ))
        .Annotate(berrors::ErrUnknown)
    })
}

/// 危险/无意义前缀：空、`.`、`v1` 或 `..` 穿越。
pub fn isInsane(pfx: &str) -> bool {
    let rooted = pfx.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in pfx.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if !rooted => parts.push(part),
            ".." => {}
            _ => parts.push(part),
        }
    }
    let normalized = if parts.is_empty() {
        if rooted {
            "/".to_owned()
        } else {
            ".".to_owned()
        }
    } else if rooted {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    };
    matches!(normalized.as_str(), "" | "." | "/" | "/v1" | "v1") || pfx.starts_with("..")
}

/// metadata 是否无任何文件组/文件。
pub fn isEmptyMetadata(md: &Metadata) -> bool {
    md.FileGroups.is_empty() && md.Files.is_empty()
}

/// 对 migration 内容做异或哈希，用于文件名校验段。
pub fn hashMigration(m: &Migration) -> u64 {
    let mut crc64_res = 0u64;
    // 压缩产物哈希异或。
    for compaction in m.GetCompactions() {
        crc64_res ^= compaction.ArtifactsHash;
    }
    // 再异或各 MetaEdit。
    for meta_edit in m.GetEditMeta() {
        crc64_res ^= hashMetaEdit(meta_edit);
    }
    for ext_bkup in m.GetIngestedSstPaths() {
        crc64_res ^= crc64_iso(ext_bkup.as_bytes());
    }
    // 最后混入 TruncatedTo。
    crc64_res ^ m.GetTruncatedTo()
}

/// 对 MetaEdit 的物理/逻辑删除与 DestructSelf 做异或哈希。
pub fn hashMetaEdit(meta_edit: &MetaEdit) -> u64 {
    let mut res = 0u64;
    // 物理文件路径。
    for df in meta_edit.GetDeletePhysicalFiles() {
        res ^= crc64_iso(df.as_bytes());
    }
    for spans in meta_edit.GetDeleteLogicalFiles() {
        // path+offset+length 组成逻辑删除指纹。
        for span in spans.GetSpans() {
            let mut buf = spans.GetPath().as_bytes().to_vec();
            buf.extend_from_slice(&span.GetOffset().to_le_bytes());
            buf.extend_from_slice(&span.GetLength().to_le_bytes());
            res ^= crc64_iso(&buf);
        }
    }
    // DestructSelf 作为单字节标志参与哈希。
    let flag = if meta_edit.DestructSelf { [1u8] } else { [0u8] };
    res ^ crc64_iso(&flag)
}

fn crc64_iso(data: &[u8]) -> u64 {
    // Go hash/crc64.ISO uses the reflected ISO polynomial and complements
    // before and after updating. Keep this local to avoid changing Cargo.lock.
    const POLY: u64 = 0xd800_0000_0000_0000;
    let mut crc = u64::MAX;
    for byte in data {
        crc ^= u64::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `{sn:08}_{hash:016X}.mgrt` 命名，与 Go 一致。
pub fn nameOf(mig: &Migration, sn: i32) -> String {
    format!("{sn:08}_{:016X}.mgrt", hashMigration(mig))
}

impl MigrationExt {
    /// 读取 BASE；不存在返回 None。
    pub fn LoadBase(&self) -> Result<Option<Migration>, Error> {
        match self
            .storage
            .ReadFile(&format!("{}/{}", self.prefix, baseMigrationName))
        {
            Ok(data) => Migration::Unmarshal(&data).map(Some).map_err(Error::new),
            Err(_) => Ok(None),
        }
    }

    /// 链式携带操作上下文提示（当前为透传）。
    pub fn WithOperationContext(mut self, _hint: &str) -> Self {
        self
    }

    /// 加载 BASE 与全部追加 migration，按 id 排序。
    pub fn Load(&self) -> Result<Migrations, Error> {
        let persisted_base = self.LoadBase()?;
        let mut base = persisted_base.clone().unwrap_or_else(NewMigration);
        // Go folds the legacy truncation checkpoint into a persisted BASE so
        // migrations created before TruncatedTo existed remain compatible.
        if persisted_base.is_some() {
            base.TruncatedTo = base.TruncatedTo.max(GetTSFromFile(
                self.storage.as_ref(),
                TruncateSafePointFileName,
            )?);
        }
        let mut appended = Vec::new();
        for (path, _) in self.storage.ListFiles(&self.prefix).map_err(Error::new)? {
            let name = path.rsplit('/').next().unwrap_or(&path);
            // 跳过 BASE 与临时文件。
            if name == baseMigrationName || name == "BASE_TMP" {
                continue;
            }
            let id = migIdOf(name)?;
            let data = self.storage.ReadFile(&path).map_err(Error::new)?;
            let mig = Migration::Unmarshal(&data).map_err(Error::new)?;
            appended.push((id, mig));
        }
        // 保证 MergeTo 顺序折叠。
        appended.sort_by_key(|(id, _)| *id);
        Ok(Migrations {
            Base: Some(base),
            Appended: appended,
        })
    }

    /// 追加下一条 migration，返回新序号。
    pub fn AppendMigration(&self, mig: &Migration) -> Result<i32, Error> {
        let loaded = self.Load()?;
        // 下一序号：已有最大+1，或从 1 起。
        let next = loaded.Appended.last().map(|(id, _)| id + 1).unwrap_or(1);
        let name = nameOf(mig, next);
        let path = format!("{}/{}", self.prefix, name);
        let data = mig.Marshal().map_err(Error::new)?;
        self.storage.WriteFile(&path, &data).map_err(Error::new)?;
        Ok(next)
    }

    /// 测试桩：写 READ 锁标记文件。
    pub fn GetReadLock(&self, _hint: &str) -> Result<(), Error> {
        // 测试桩：在 v1/LOCK 下写 READ 标记。
        let path = format!("v1/LOCK/{}.READ.lock", uuidish());
        let meta = serde_json::json!({
            "OwnerID": "test",
            "LockType": "migrations",
            "TxnID": [1,2,3],
            "Hint": _hint,
        });
        self.storage
            .WriteFile(&path, meta.to_string().as_bytes())
            .map_err(Error::new)?;
        Ok(())
    }

    /// 测试桩：执行闭包后返回空 effects 列表。
    pub fn DryRun<F: FnOnce(MigrationExt)>(&self, f: F) -> Vec<(String, String)> {
        // 测试桩：执行后返回空 effects。
        let nested = MigrationExtension(self.storage.clone());
        f(nested);
        Vec::new()
    }

    /// 合并至 seq、应用 phantom/交互检查，写回 BASE 并删除已合并追加项。
    pub fn MergeAndMigrateTo(
        &mut self,
        seq: i32,
        opts: Vec<MergeAndMigrateToOpt>,
    ) -> Result<MergeAndMigratedTo, Error> {
        // 先应用选项（锁/phantom/交互）。
        for opt in opts {
            opt(self);
        }
        // Options are per invocation in Go; consume/reset their state here so
        // reusing a MigrationExt cannot replay phantom migrations or checks.
        let phantom = std::mem::take(&mut self.phantom);
        let interactive = self.interactive.take();
        self.skip_locking = false;
        self.always_run_truncate = false;
        let loaded = self.Load()?;
        let mut merged = loaded.MergeTo(seq);
        // 叠加入内存 phantom。
        for p in &phantom {
            merged = MergeMigrations(&merged, p);
        }
        // Go treats a declined interactive check as a non-fatal warning and
        // performs no storage mutation.
        if let Some(check) = &interactive {
            if !check(&merged) {
                return Ok(MergeAndMigratedTo {
                    Migrated: MigratedTo {
                        NewBase: NewMigration(),
                        Warnings: vec![Error::new("User aborted, nothing will happen")],
                    },
                });
            }
        }
        // Go writes BASE_TMP and renames it, avoiding a partially written BASE.
        let data = merged.Marshal().map_err(Error::new)?;
        let tmp_path = format!("{}/BASE_TMP", self.prefix);
        let base_path = format!("{}/{}", self.prefix, baseMigrationName);
        self.storage
            .WriteFile(&tmp_path, &data)
            .map_err(Error::new)?;
        self.storage
            .Rename(&tmp_path, &base_path)
            .map_err(Error::new)?;
        // 删除已合并的追加文件。
        for (path, _) in self.storage.ListFiles(&self.prefix).map_err(Error::new)? {
            let name = path.rsplit('/').next().unwrap_or(&path);
            if name == baseMigrationName {
                continue;
            }
            if let Ok(id) = migIdOf(name) {
                if id <= seq {
                    let _ = self.storage.DeleteFile(&path);
                }
            }
        }
        Ok(MergeAndMigratedTo {
            Migrated: MigratedTo {
                NewBase: merged,
                Warnings: Vec::new(),
            },
        })
    }
}

/// 用纳秒生成伪 UUID 十六进制串，供锁文件名。
fn uuidish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{ns:032x}")
}
