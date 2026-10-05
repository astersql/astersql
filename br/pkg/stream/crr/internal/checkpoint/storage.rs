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

//! 上游 backupmeta 扫描与加载，对齐 Go `storage.go`。
//! 职责：在 `v1/backupmeta` 前缀上增量 Walk、解析文件名/内容、抽取 data file 路径。
//! 约束：增量扫描依赖 StartAfter；URI scheme 仅允许 s3/file/gcs。
//! StartAfter 使用大写十六进制 flushTS + 最大 store 后缀，保证与真实 meta 名字典序衔接。
//! 名称与内容中的 store id 必须一致（名称为 0 时放宽），否则拒绝加载。
//! 解析失败停止 Walk：坏文件名不可静默跳过，避免安全点越过未识别对象。
//! load 阶段才读内容；扫描阶段只靠文件名过滤，降低对象存储读放大。

use std::sync::atomic::{AtomicBool, Ordering};

use astersql_br_pkg_stream_backupmetas::ParseName;
use serde::Deserialize;
use serde_json::Value;

use crate::calculator::{Calculator, Context, Error, UpstreamStorageReader, WalkOption};

/// meta 文件后缀；Walk 时非 `.meta` 直接跳过。
const META_SUFFIX: &str = ".meta";
/// StartAfter 尾缀：取最大 16 位十六进制 store id 再加 `~`，使同 flushTS 的全部 store 都被扫过。
const MAX_STORE_ID_SUFFIX: &str = "FFFFFFFFFFFFFFFF~";
/// 流式备份 meta 固定前缀，与 TiKV/BR 对象布局一致。
const STREAM_BACKUP_META_PREFIX: &str = "v1/backupmeta";

/// 仅从文件名解析出的 meta 条目，尚未读内容。
pub(crate) struct parsedMetaFile {
    /// 对象完整路径（含 v1/backupmeta 前缀）。
    pub path: String,
    /// 文件名中的 FlushTS，用于水位与跳过判定。
    pub flush_ts: u64,
    /// 文件名中的 StoreID；可能为 0，最终以内容为准。
    pub store_id: u64,
    pub empty: bool,
}

/// 已读 JSON 并解析出 data file 列表的 meta。
pub(crate) struct loadedMetaFile {
    pub path: String,
    pub flush_ts: u64,
    /// 经 resolve_store_id 校验后的权威 store id。
    pub store_id: u64,
    pub empty: bool,
    /// 下游需确认同步的日志/数据对象路径。
    pub data_file_paths: Vec<String>,
}

/// WalkDir 回调的轻量条目。
struct walkEntry {
    path: String,
    size: i64,
}

/// backupmeta JSON 中单文件信息；仅 Path 参与同步等待。
#[derive(Clone, Debug, Default, Deserialize)]
struct BackupDataFileInfo {
    #[serde(default)]
    Path: String,
}

/// 文件组：优先使用组级 Path，否则展开 DataFilesInfo。
#[derive(Clone, Debug, Default, Deserialize)]
struct BackupDataFileGroup {
    #[serde(default)]
    Path: String,
    #[serde(default)]
    DataFilesInfo: Vec<BackupDataFileInfo>,
}

/// backupmeta 主体；StoreId 兼容 snake_case 别名以对齐历史落盘格式。
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct BackupMetadata {
    #[serde(default, alias = "meta_version")]
    MetaVersion: Value,
    #[serde(default, alias = "store_id")]
    StoreId: i64,
    #[serde(default)]
    Files: Vec<BackupDataFileInfo>,
    #[serde(default)]
    FileGroups: Vec<BackupDataFileGroup>,
}

impl BackupMetadata {
    /// 与 Go GetStoreId 同名，便于对照。
    fn GetStoreId(&self) -> i64 {
        self.StoreId
    }
}

/// 包装常量，便于将来与 Go 导出函数签名对齐。
fn get_stream_backup_meta_prefix() -> &'static str {
    STREAM_BACKUP_META_PREFIX
}

/// 把回调式 WalkDir 适配为“yield 返回 false 即停止”，停止哨兵错误会被吞掉。
/// 真实存储错误仍向上返回，前缀加上 walk upstream backupmeta。
fn walk_dir_seq(
    ctx: &Context,
    storage: &dyn UpstreamStorageReader,
    opt: &WalkOption,
    mut yield_fn: impl FnMut(walkEntry) -> bool,
) -> Result<(), Error> {
    let stop = AtomicBool::new(false);
    let result = storage.WalkDir(ctx, opt, &mut |file_path, size| {
        // 已请求停止后仍可能被回调一次，直接返回哨兵错误打断底层遍历。
        if stop.load(Ordering::SeqCst) {
            return Err(Error::new("stop walk iteration"));
        }
        if !yield_fn(walkEntry {
            path: file_path.to_string(),
            size,
        }) {
            stop.store(true, Ordering::SeqCst);
            return Err(Error::new("stop walk iteration"));
        }
        Ok(())
    });

    match result {
        Ok(()) => Ok(()),
        // 主动停止不是失败；与 Go iterator break 语义对齐。
        Err(err) if err.message() == "stop walk iteration" => Ok(()),
        Err(err) => Err(Error::new(format!(
            "walk upstream backupmeta prefix: {err}"
        ))),
    }
}

impl Calculator {
    /// 返回本轮待处理 meta 迭代器；内部先物化 Vec，避免跨借用持有 Walk 状态。
    pub(crate) fn new_meta_file_iter(
        &self,
        ctx: &Context,
    ) -> impl Iterator<Item = Result<parsedMetaFile, Error>> + '_ {
        collect_meta_files(self, ctx).into_iter()
    }
}

/// 扫描上游 meta：应用 StartAfter、过滤已同步 flushTS、解析文件名。
/// 解析失败会作为 Err 元素推入结果并停止 Walk，调用方应短路返回。
fn collect_meta_files(calc: &Calculator, ctx: &Context) -> Vec<Result<parsedMetaFile, Error>> {
    let mut walk_opt = WalkOption {
        SubDir: get_stream_backup_meta_prefix().to_string(),
        ..Default::default()
    };
    // synced_ts=0 时 StartAfter 为空，表示全量列举。
    let start_after = meta_scan_start_after(calc.state.synced_ts);
    if !start_after.is_empty() {
        walk_opt.StartAfter = start_after;
    }

    // 闭包捕获副本，避免与 calc 可变借用纠缠。
    let synced_ts = calc.state.synced_ts;
    let mut collected = Vec::new();
    let walk_result = walk_dir_seq(ctx, &*calc.deps.Upstream, &walk_opt, |entry| {
        if !entry.path.ends_with(META_SUFFIX) {
            return true;
        }
        let base_name = path_base(&entry.path)
            .trim_end_matches(META_SUFFIX)
            .to_string();
        let parsed = match ParseName(&base_name) {
            Ok(parsed) => parsed,
            Err(err) => {
                collected.push(Err(Error::new(format!(
                    "parse backupmeta name {}: {err}",
                    entry.path
                ))));
                // 返回 false 触发停止；坏名不应被静默跳过。
                return false;
            }
        };
        // 全局 synced_ts 以下的 meta 在增量扫描中可忽略（StartAfter 可能仍漏扫边界项）。
        if parsed.FlushTS <= synced_ts {
            return true;
        }
        collected.push(Ok(parsedMetaFile {
            path: entry.path,
            flush_ts: parsed.FlushTS,
            store_id: parsed.StoreID,
            empty: parsed.IsEmpty(),
        }));
        true
    });
    // Walk 级错误追加到结果末尾，迭代器消费时与条目错误统一处理。
    if let Err(err) = walk_result {
        collected.push(Err(err));
    }
    collected
}

/// 读取并解析 meta JSON，校验 store id，抽取待同步 data 路径。
pub(crate) fn load_meta_file(
    ctx: &Context,
    storage: &dyn UpstreamStorageReader,
    meta_file: parsedMetaFile,
) -> Result<(loadedMetaFile, bool), Error> {
    if meta_file.empty {
        if meta_file.store_id == 0 {
            eprintln!(
                "ignore empty backupmeta with no store id in file name: path={}, flush-ts={}",
                meta_file.path, meta_file.flush_ts
            );
            return Ok((
                loadedMetaFile {
                    path: meta_file.path,
                    flush_ts: meta_file.flush_ts,
                    store_id: 0,
                    empty: true,
                    data_file_paths: Vec::new(),
                },
                true,
            ));
        }
        return Ok((
            loadedMetaFile {
                path: meta_file.path,
                flush_ts: meta_file.flush_ts,
                store_id: meta_file.store_id,
                empty: true,
                data_file_paths: Vec::new(),
            },
            false,
        ));
    }
    // 读失败包装路径信息，便于运维定位缺失对象。
    let meta_bytes = storage.ReadFile(ctx, &meta_file.path).map_err(|err| {
        Error::new(format!(
            "read upstream backupmeta {}: {err}",
            meta_file.path
        ))
    })?;

    let meta = parse_backup_metadata(&meta_bytes)
        .map_err(|err| Error::new(format!("parse backupmeta {}: {err}", meta_file.path)))?;

    // 以内容 StoreId 为准，文件名仅作交叉校验。
    let store_id = resolve_store_id(meta_file.store_id, meta.GetStoreId(), &meta_file.path)?;

    Ok((
        loadedMetaFile {
            path: meta_file.path,
            flush_ts: meta_file.flush_ts,
            store_id,
            empty: false,
            data_file_paths: extract_data_file_paths(&meta),
        },
        false,
    ))
}

/// JSON 反序列化；字段缺省走 Default，兼容精简落盘。
pub(crate) fn parse_backup_metadata(raw_meta: &[u8]) -> Result<BackupMetadata, Error> {
    let mut meta: BackupMetadata =
        serde_json::from_slice(raw_meta).map_err(|err| Error::new(err.to_string()))?;
    // Go MetadataHelper.ParseToMetadata 把 V1 顶层 Files 视为一个匿名 group。
    // 只在 FileGroups 为空时转换，避免覆盖已经是 V2 布局的元数据。
    if metadata_version_is_v1(&meta.MetaVersion) && meta.FileGroups.is_empty() {
        meta.FileGroups.push(BackupDataFileGroup {
            Path: String::new(),
            DataFilesInfo: meta.Files.clone(),
        });
    }
    Ok(meta)
}

/// serde 测试夹具使用枚举名，历史精简格式也可能直接写 protobuf 数值 1。
fn metadata_version_is_v1(version: &Value) -> bool {
    matches!(version, Value::String(name) if name == "V1" || name == "MetaVersion_V1")
        || matches!(version, Value::Number(number) if number.as_i64() == Some(1))
}

/// 内容 store id 必须为正；文件名 store id 非 0 时必须与内容一致。
fn resolve_store_id(
    name_store_id: u64,
    content_store_id: i64,
    meta_path: &str,
) -> Result<u64, Error> {
    if content_store_id <= 0 {
        return Err(Error::new(format!(
            "backupmeta {meta_path} contains invalid store id {content_store_id}"
        )));
    }
    let store_id = content_store_id as u64;
    // name_store_id==0 表示文件名未编码 store，仅信内容。
    if name_store_id != 0 && name_store_id != store_id {
        return Err(Error::new(format!(
            "backupmeta {meta_path} has mismatched store id between name ({name_store_id}) and content ({store_id})"
        )));
    }
    Ok(store_id)
}

/// 组级 Path 优先；否则展开各 DataFilesInfo.Path，空路径丢弃。
pub(crate) fn extract_data_file_paths(meta: &BackupMetadata) -> Vec<String> {
    let mut paths = Vec::with_capacity(meta.FileGroups.len());
    for group in &meta.FileGroups {
        if !group.Path.is_empty() {
            paths.push(group.Path.clone());
            // 组路径已覆盖时不再展开成员，避免重复 pending。
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

/// 校验上游 URI 是否支持 StartAfter 增量列举；不支持则计算器无法安全跳过旧 meta。
pub(crate) fn validate_incremental_meta_scan_storage(raw_uri: &str) -> Result<(), Error> {
    let scheme = match parse_scheme(raw_uri) {
        Ok(scheme) => scheme,
        Err(err) => {
            return Err(Error::new(format!(
                "parse upstream storage uri {raw_uri:?}: {err}"
            )));
        }
    };
    match scheme.as_str() {
        // 这些后端在 BR 栈中均支持 StartAfter/续列。
        "s3" | "oss" | "file" | "gcs" => Ok(()),
        "" => Err(Error::new(format!(
            "upstream storage uri {raw_uri:?} has empty scheme"
        ))),
        // 其它 scheme（如 hdfs）可能无法增量跳过，拒绝启动更安全。
        _ => Err(Error::new(format!(
            "crr checkpoint calculator requires StartAfter-capable upstream storage, got {raw_uri}"
        ))),
    }
}

/// 解析 scheme；`file://` 特判为 file，其它取 `://` 前缀，无 scheme 返回空串。
fn parse_scheme(raw_uri: &str) -> Result<String, Error> {
    if let Some(rest) = raw_uri.strip_prefix("file://") {
        let _ = rest;
        return Ok("file".to_string());
    }
    if let Some(idx) = raw_uri.find("://") {
        return Ok(raw_uri[..idx].to_string());
    }
    Ok(String::new())
}

/// 构造 Walk StartAfter：synced_ts==0 表示全量；否则大写 hex(ts)+MAX_STORE_ID_SUFFIX。
/// 大写与 backupmeta 文件名一致，避免大小写字典序导致漏扫（见 storage_internal_test）。
pub(crate) fn meta_scan_start_after(synced_ts: u64) -> String {
    if synced_ts == 0 {
        return String::new();
    }
    path_join(&[
        get_stream_backup_meta_prefix(),
        &format!("{synced_ts:016X}{MAX_STORE_ID_SUFFIX}"),
    ])
}

/// 取最后一段路径；无 `/` 时返回原串。
fn path_base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 拼接路径段并折叠多余 `/`，不引入前导斜杠。
fn path_join(parts: &[&str]) -> String {
    parts
        .iter()
        .flat_map(|part| part.split('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}
