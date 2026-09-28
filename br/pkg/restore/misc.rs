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

//! Misc restore helpers matching `misc.go`.
//!
//! 本模块对齐 Go `br/pkg/restore/misc.go`，汇集恢复流程的杂项辅助：
//! - 日志恢复表 ID 黑名单文件：命名解析、protobuf 编解码、校验和、遍历/截断；
//! - `CheckTableTrackerContainsTableIDsFromBlocklistFiles`：快照与日志恢复冲突检测；
//! - schema/TS 工具：`GetTableSchema`、`AssertUserDBsEmpty`、`GetTS`/`GetTSWithRetry`；
//! - `HasRestoreIDColumn`：探测 `mysql.tidb_pitr_id_map` 是否含 restore_id；
//! - `regionScanner`：带缓存的 region 定位，供重叠 SST 批分组；
//! - `GroupOverlappedBackupFileSetsIter`：按键范围与 region 边界合并重叠备份文件集。
//!
//! 数据流（黑名单）：Marshal→对象存储；Walk→filter→unmarshal→checksum 校验→业务回调。
//! 数据流（批分组）：BackupFileSet→算键范围→排序→扫描 region→合并重叠→回调批次。
//! 手工 protobuf 编解码仅服务本文件消息，字段号需与 Go gogo 定义一致。
//! 短文件名解析失败返回 (0,0,false)，避免 Go 切片越界式 panic。
//!
//! region 缓存命中时 drain 前缀，保持「当前 key 落在 cache[0]」不变量。
//! 重叠 SST 必须同 TableID 与 RewriteRules，否则直接错误，防止静默错合并。
//! GetTSWithRetry 使用 WithRetryAggressive，失败时优先返回业务 get_ts_err。
//! HasRestoreIDColumn 在表不存在时返回 false，而非向上抛错。
//! proto_marshal/unmarshal 字段号固定，改动需同步 Go `.proto` / gogo 生成物。
//! Walk 过滤器与内容过滤器语义一致，但输入分别来自文件名与文件体。
//! Truncate 的 until_ts 比较对象是 RestoreCommitTs，不是 RestoreStartTs。
//! GroupOverlapped 排序主键为 startKey，次键 endKey，保证扫描顺序稳定。
//! IsKeyRangeInOneRegion 用 end_key < region.EndKey（严格小于）对齐 Go 半开区间。
//! 空库名 test 被特殊豁免，其它空用户库仍算「不 fresh」。
//! 黑名单文件名十六进制段固定宽度 16，与 Go `fmt.Sprintf("%016X")` 对齐。
//! SHA256 输入字节序为小端 TS + 表/库 ID，任一字段重排都会导致 checksum 失败。
//! fastWalk 使用 8 worker 并行 unmarshal，首个 ErrorGroup 错误后停止投递新任务。
//! CheckTableTracker 错误文案含建议 BackupTS/RestoreTS 边界，供运维重选备份窗口。
//! AssertUserDBsEmpty 列举上限后追加 "..."，避免错误信息无限膨胀。
//! GetTS 将 PD physical/logical 合成 oracle TSO，供备份/恢复统一时间轴。
//! locateRegionFromCache 在 key 越过缓存尾 EndKey 时强制 remote 刷新。
//! GroupOverlapped 间隙跨 Region 时先 flush batch，再开启新合并窗口。
//! getKeyRangeForBackupFileSet 对空 Start/End 做 unwrap_or_default，与 Go nil 键一致。
//! proto3 零值字段不编码；Decode 侧依赖 Default 零值恢复。
//! packed/non-packed repeated 双路径兼容历史写入器与 gogo 默认 packed。
//! 未知 wire type 跳过或报错，防止前向兼容字段破坏解析。
//! 本文件不持有对象存储连接生命周期；调用方保证 Walk/Delete 期间 Storage 有效。
//! FineGrained/CoarseGrained 仅作字符串标记，拆分策略由上层 switcher/restorer 解释。
//! PiTRIdTracker 冲突检测把分区 ID 与表 ID 同等对待，对齐 Go ContainsPartitionId。
//! Truncate 两阶段（收集路径→串行 Delete）规避 `&dyn Storage` 跨线程借用。
//! ComposeTS 失败不会在本模块重试；重试入口仅 `GetTSWithRetry`。
//! regionScanner.cache_size 默认 64，与 Go 批分组扫描上限一致。
//! BackupFileSetWithKeyRange 是批分组中间结构，不对外稳定导出。
//! 日志字段尽量复用 Go 原文字符串，便于跨语言 grep 对照。
//! 黑名单文件名形如 `R{commit:016X}_S{start:016X}.meta`，缺段或非十六进制则解析失败。
//! protobuf 字段含 RestoreCommitTs/TableIds/Checksum/DbIds/RewriteTs/RestoreStartTs。
//! Checksum 为内容 SHA256，Unmarshal 时与重算值比对防篡改。
//! AssertUserDBsEmpty 跳过系统库与名为 test 的空库。
//! GetTableSchema 走 infoschema，找不到表返回错误而非空壳。
//! GetTS 封装 PD TSO；GetTSWithRetry 在瞬时失败时 Aggressive 退避。
//! regionScanner 缓存按 StartKey 升序，查找时丢弃已越过的前缀。
//! GroupOverlappedBackupFileSetsIter 回调式输出：合并完成后 flush batch。
//! 批合并时同一批共享 RewriteRules，调用方勿并发改写。
//! Walk 的 filter 可提前跳过无关 TS 窗口文件以省 IO。
//! Truncate 删除 RestoreCommitTs ≤ until_ts 的文件，保留更新窗口。
//! Marshal 先算 checksum 再序列化，避免半写损坏。
//! SST 重叠判定基于 rewrite 后 raw key，而非用户可见 SQL 键。
//! 空 BackupFileSet 列表直接成功返回，不触发 region 扫描。
//! 表/分区 ID 已在 tracker 中时冲突检测必须报错，避免静默跳过。
//! base64/sha2 仅用于黑名单校验展示，不涉及备份数据加密。
//! 本文件不启动真实集群；PD/存储依赖由调用方注入。
//! 若与相邻 Go 字段号不一致，优先以 Go gogo 生成物为准回修。
//! 用户库 emptiness 检查用于 fresh cluster 恢复前置条件。
//! restore_id 列探测兼容旧集群升级路径，缺列则走兼容分支。
//! 错误注解尽量带 path/TS，便于运维定位坏黑名单对象。

use std::cmp::Ordering;
use std::path::Path;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_restore_utils::stubs::backuppb;
use astersql_br_pkg_restore_utils::{GetRewriteRawKeys, RewriteRules};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::restorer::{BackupFileSet, BatchBackupFileSet};
use crate::stubs::{
    CIStr, ComposeTS, Context, Domain, Error, ErrorGroup, IsMemOrSysDB, NewWorkerPool, PdClient,
    PiTRIdTracker, RegionInfo, Result, ScanRegionsWithRetry, SplitClient, Storage, TableInfo,
    WalkOption, WithRetryAggressive, berrors, log,
};

/// 拆分粒度字符串别名，取值见 Fine/Coarse 常量。
pub type Granularity = String;
/// 细粒度拆分标记，对齐 Go `FineGrained`。
pub const FineGrained: &str = "fine-grained";
/// 粗粒度拆分标记，对齐 Go `CoarseGrained`。
pub const CoarseGrained: &str = "coarse-grained";

/// 黑名单文件目录前缀（对象存储 SubDir）。
pub const logRestoreTableIDBlocklistFilePrefix: &str = "v1/log_restore_tables_blocklists";
/// Exported alias used by Go `export_test.go`.
/// 导出别名，供 export_test / 外部 crate 引用。
pub const LogRestoreTableIDBlocklistFilePrefix: &str = logRestoreTableIDBlocklistFilePrefix;

/// `AssertUserDBsEmpty` 最多列出的用户表数，超出追加 "..."。
const maxUserTablesNum: usize = 10;

/// 日志恢复表/库 ID 黑名单文件内容；Checksum 为字段的 SHA256。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogRestoreTableIDsBlocklistFile {
    /// 日志恢复提交 TS（文件名 R 段）。
    pub RestoreCommitTs: u64,
    /// 日志恢复起始/目标 TS（文件名 S 段）。
    pub RestoreStartTs: u64,
    /// rewrite 相关 TS，截断清理时回调使用。
    pub RewriteTs: u64,
    /// 被日志恢复占用的表 ID 列表。
    pub TableIds: Vec<i64>,
    /// 被日志恢复占用的库 ID 列表。
    pub DbIds: Vec<i64>,
    /// 完整性校验摘要；Unmarshal 时必须匹配重算值。
    pub Checksum: Vec<u8>,
}

impl LogRestoreTableIDsBlocklistFile {
    /// 清空为 Default，对齐 proto.Reset。
    pub fn Reset(&mut self) {
        *self = Self::default();
    }

    /// 调试字符串；不含 checksum，避免日志过长。
    pub fn String(&self) -> String {
        format!(
            "restore_commit_ts:{} restore_start_ts:{} rewrite_ts:{} table_ids:{:?} db_ids:{:?}",
            self.RestoreCommitTs, self.RestoreStartTs, self.RewriteTs, self.TableIds, self.DbIds
        )
    }

    /// `prefix/R{commit:016X}_S{start:016X}.meta`，与解析函数互逆。
    fn filename(&self) -> String {
        format!(
            "{}/R{:016X}_S{:016X}.meta",
            logRestoreTableIDBlocklistFilePrefix, self.RestoreCommitTs, self.RestoreStartTs
        )
    }

    /// 按小端序拼接 TS 与 ID 列表后做 SHA256；顺序必须与 Go 一致。
    fn checksumLogRestoreTableIDsBlocklistFile(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.RestoreCommitTs.to_le_bytes());
        hasher.update(self.RestoreStartTs.to_le_bytes());
        hasher.update(self.RewriteTs.to_le_bytes());
        for table_id in &self.TableIds {
            hasher.update((*table_id as u64).to_le_bytes());
        }
        for db_id in &self.DbIds {
            hasher.update((*db_id as u64).to_le_bytes());
        }
        hasher.finalize().to_vec()
    }

    /// Marshal 前写入 Checksum 字段。
    fn setChecksumLogRestoreTableIDsBlocklistFile(&mut self) {
        self.Checksum = self.checksumLogRestoreTableIDsBlocklistFile();
    }
}

/// 从文件名解析 (commitTS, startTS, ok)；失败三元组为 (0,0,false)。
pub fn parseLogRestoreTableIDsBlocklistFileName(filename: &str) -> (u64, u64, bool) {
    // 允许传入带目录的路径，只取最后一段文件名。
    let filename = Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    if !filename.ends_with(".meta") {
        return (0, 0, false);
    }
    if filename.is_empty() || !filename.as_bytes().starts_with(b"R") {
        return (0, 0, false);
    }
    // Go slices filename[1:17] / [19:35]; reject short names instead of panicking.
    // Rust 额外拒绝短名，避免切片 panic（Go 侧依赖固定长度）。
    if filename.len() < 35 {
        return (0, 0, false);
    }
    let Ok(restore_commit_ts) = u64::from_str_radix(&filename[1..17], 16) else {
        log::Warn("failed to parse log restore table IDs blocklist file name");
        return (0, 0, false);
    };
    // 中间必须是 "_S" 分隔符。
    if &filename[17..19] != "_S" {
        return (0, 0, false);
    }
    let Ok(restore_start_ts) = u64::from_str_radix(&filename[19..35], 16) else {
        log::Warn("failed to parse log restore table IDs blocklist file name");
        return (0, 0, false);
    };
    (restore_commit_ts, restore_start_ts, true)
}

/// Exported alias matching Go `export_test.go`.
/// 导出解析函数，供 export_test 与跨 crate 测试使用。
pub fn ParseLogRestoreTableIDsBlocklistFileName(filename: &str) -> (u64, u64, bool) {
    parseLogRestoreTableIDsBlocklistFileName(filename)
}

/// 构造黑名单文件：算 checksum → 返回 (相对路径, protobuf 字节)。
pub fn MarshalLogRestoreTableIDsBlocklistFile(
    restore_commit_ts: u64,
    restore_start_ts: u64,
    rewrite_ts: u64,
    table_ids: Vec<i64>,
    db_ids: Vec<i64>,
) -> Result<(String, Vec<u8>)> {
    let mut blocklist_file = LogRestoreTableIDsBlocklistFile {
        RestoreCommitTs: restore_commit_ts,
        RestoreStartTs: restore_start_ts,
        RewriteTs: rewrite_ts,
        TableIds: table_ids,
        DbIds: db_ids,
        Checksum: Vec::new(),
    };
    blocklist_file.setChecksumLogRestoreTableIDsBlocklistFile();
    let filename = blocklist_file.filename();
    let data = proto_marshal(&blocklist_file)?;
    Ok((filename, data))
}

/// 反序列化并校验 checksum；不匹配视为文件损坏。
pub fn unmarshalLogRestoreTableIDsBlocklistFile(
    data: &[u8],
) -> Result<LogRestoreTableIDsBlocklistFile> {
    let blocklist_file = proto_unmarshal(data)?;
    let calculated = blocklist_file.checksumLogRestoreTableIDsBlocklistFile();
    if calculated != blocklist_file.Checksum {
        // 错误文案含 base64，便于与 Go 侧日志对照。
        return Err(Error::new(format!(
            "checksum mismatch (calculated checksum is {} but the recorded checksum is {}), the log restore table IDs blocklist file may be corrupted",
            base64::engine::general_purpose::STANDARD.encode(&calculated),
            base64::engine::general_purpose::STANDARD.encode(&blocklist_file.Checksum),
        )));
    }
    Ok(blocklist_file)
}

/// Exported alias matching Go `export_test.go`.
/// 导出反序列化入口，行为与包内 `unmarshalLogRestoreTableIDsBlocklistFile` 相同。
pub fn UnmarshalLogRestoreTableIDsBlocklistFile(
    data: &[u8],
) -> Result<LogRestoreTableIDsBlocklistFile> {
    unmarshalLogRestoreTableIDsBlocklistFile(data)
}

/// 遍历黑名单目录：WalkDir 收集路径 → 主线程 ReadFile → 8 协程 unmarshal 并回调。
///
/// `filter_out_fn` 在 Walk 与 unmarshal 后各过滤一次；Go 在 worker 内 ReadFile，
/// Rust 主线程预读以便 job 持有字节且 `Storage` 仍可在 Delete 阶段复用。
fn fastWalkLogRestoreTableIDsBlocklistFile(
    ctx: &Context,
    s: &dyn Storage,
    filter_out_fn: impl Fn(u64, u64) -> bool + Send + Sync + 'static,
    execution_fn: impl Fn(&Context, &str, u64, u64, u64, Vec<i64>, Vec<i64>) -> Result<()>
    + Send
    + Sync
    + 'static,
) -> Result<()> {
    let mut filenames = Vec::new();
    s.WalkDir(
        ctx,
        &WalkOption {
            SubDir: logRestoreTableIDBlocklistFilePrefix.to_string(),
        },
        &mut |path, _size| {
            let (restore_commit_ts, restore_start_ts, parsed) =
                parseLogRestoreTableIDsBlocklistFileName(path);
            // Walk 阶段按 TS 预过滤，减少后续 ReadFile 次数。
            if parsed && filter_out_fn(restore_commit_ts, restore_start_ts) {
                return Ok(());
            }
            filenames.push(path.to_string());
            Ok(())
        },
    )?;

    // Read files on the caller thread so worker jobs own the bytes (Go captures storage by ref).
    // 主线程读完再投递 worker，避免闭包捕获 `&dyn Storage` 的生命周期问题。
    let mut file_data = Vec::with_capacity(filenames.len());
    for filename in filenames {
        let data = s.ReadFile(ctx, &filename).map_err(Error::Trace)?;
        file_data.push((filename, data));
    }

    let worker_pool = NewWorkerPool(8, "walk dir log restore table IDs blocklist files");
    let (eg, ectx) = ErrorGroup::with_context(ctx);
    let filter_out_fn = Arc::new(filter_out_fn);
    let execution_fn = Arc::new(execution_fn);
    for (filename, data) in file_data {
        if ectx.Err().is_some() {
            // 与 Go errgroup 一致：首个错误后停止调度新 job。
            break;
        }
        let filter_out_fn = filter_out_fn.clone();
        let execution_fn = execution_fn.clone();
        let ectx = ectx.clone();
        worker_pool.ApplyOnErrorGroup(&eg, move || {
            let blocklist_file =
                unmarshalLogRestoreTableIDsBlocklistFile(&data).map_err(Error::Trace)?;
            if filter_out_fn(
                blocklist_file.RestoreCommitTs,
                blocklist_file.RestoreStartTs,
            ) {
                return Ok(());
            }
            execution_fn(
                &ectx,
                &filename,
                blocklist_file.RestoreCommitTs,
                blocklist_file.RestoreStartTs,
                blocklist_file.RewriteTs,
                blocklist_file.TableIds,
                blocklist_file.DbIds,
            )
            .map_err(Error::Trace)
        });
    }
    eg.Wait().map_err(Error::Trace)
}

/// 校验 PiTR tracker 是否与历史 log restore 黑名单冲突（Go 同名函数）。
///
/// 跳过 commit 晚于 snapshot `start_ts` 或 restore 目标早于黑名单 `restore_start_ts` 的文件；
/// 表/分区 ID 已在 tracker 中则报错；库 ID 冲突同理；`clean_error` 在通过时用 rewrite_ts 清错。
pub fn CheckTableTrackerContainsTableIDsFromBlocklistFiles(
    ctx: &Context,
    s: &dyn Storage,
    tracker: &PiTRIdTracker,
    start_ts: u64,
    restored_ts: u64,
    table_name_by_table_id: impl Fn(i64) -> String + Send + Sync + 'static,
    db_name_by_db_id: impl Fn(i64) -> String + Send + Sync + 'static,
    check_table_id_lost: impl Fn(i64) -> bool + Send + Sync + 'static,
    check_db_id_lost: impl Fn(i64) -> bool + Send + Sync + 'static,
    clean_error: impl Fn(u64) + Send + Sync + 'static,
) -> Result<()> {
    let tracker = tracker.clone();
    fastWalkLogRestoreTableIDsBlocklistFile(
        ctx,
        s,
        move |restore_commit_ts, restore_start_ts| {
            // 本次 snapshot 起点已覆盖该 log restore，或 restore 目标早于黑名单起点 → 忽略。
            start_ts >= restore_commit_ts || restored_ts < restore_start_ts
        },
        move |_ctx, _filename, restore_commit_ts, restore_start_ts, rewrite_ts, table_ids, db_ids| {
            for table_id in table_ids {
                if tracker.ContainsTableId(table_id) || tracker.ContainsPartitionId(table_id) {
                    // 表在 snapshot 备份后又做过 log restore，与当前还原窗口互斥。
                    return Err(Error::new(format!(
                        "cannot restore the table(Id={}, name={} at {}) because it is log restored(at {}) after snapshot backup(at {}). \
Please respecify the filter that does not contain the table or replace with a newer snapshot backup(BackupTS > {}) \
or older restored ts(RestoreTS < {}).",
                        table_id,
                        table_name_by_table_id(table_id),
                        restored_ts,
                        restore_commit_ts,
                        start_ts,
                        restore_commit_ts,
                        restore_start_ts
                    )));
                }
                if check_table_id_lost(table_id) {
                    // meta 可能未纳入 log backup，仅告警不阻断（Go 同逻辑）。
                    log::Warn("the table is lost in the log backup storage, so that it can not be restored.");
                }
            }
            for db_id in db_ids {
                if tracker.ContainsDB(db_id) {
                    // 库在 snapshot 前已被 log restore 创建/占用。
                    return Err(Error::new(format!(
                        "cannot restore the database(Id={}, name {} at {}) because it is log restored(at {}) before snapshot backup(at {}). \
Please respecify the filter that does not contain the database or replace with a newer snapshot backup.",
                        db_id,
                        db_name_by_db_id(db_id),
                        restored_ts,
                        restore_commit_ts,
                        start_ts
                    )));
                }
                if check_db_id_lost(db_id) {
                    log::Warn(
                        "the database is lost in the log backup storage, so that it can not be restored.",
                    );
                }
            }
            // 该黑名单对应的 rewrite 错误可被上层清除。
            clean_error(rewrite_ts);
            Ok(())
        },
    )
    .map_err(Error::Trace)
}

/// 删除 commit TS ≤ `until_ts` 的黑名单文件（Go `TruncateLogRestoreTableIDsBlocklistFiles`）。
///
/// Go 在 worker 内直接 DeleteFile；Rust 分两阶段：fastWalk 收集路径再串行删除，
/// 因 Delete 需 `&dyn Storage` 且 fastWalk 已改为预读字节。
pub fn TruncateLogRestoreTableIDsBlocklistFiles(
    ctx: &Context,
    s: &dyn Storage,
    until_ts: u64,
) -> Result<()> {
    // Collect then delete: DeleteFile needs Storage; read phase already happened in fastWalk,
    // so pass owned delete via Mem-friendly path — re-read Walk and delete by name using a
    // second pass that captures storage paths only (delete runs after unmarshal filter).
    // filter：保留 restore_commit_ts > until_ts 的文件，其余进入删除列表。
    let mut to_delete = Arc::new(Mutex::new(Vec::<String>::new()));
    let to_delete_job = to_delete.clone();
    fastWalkLogRestoreTableIDsBlocklistFile(
        ctx,
        s,
        move |restore_commit_ts, _restore_target_ts| until_ts < restore_commit_ts,
        move |_ctx, filename, _, _, _, _, _| {
            to_delete_job.lock().unwrap().push(filename.to_string());
            Ok(())
        },
    )
    .map_err(Error::Trace)?;
    let paths = std::mem::take(&mut *to_delete.lock().unwrap());
    for filename in paths {
        s.DeleteFile(ctx, &filename).map_err(Error::Trace)?;
    }
    Ok(())
}

/// 唯一标识一张用户表（库名 + 表名），用于 filter / 日志展示。
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct UniqueTableName {
    pub DB: String,
    pub Table: String,
}

/// 将布尔开关转为 SQL 风格 `"ON"` / `"OFF"` 字符串（Go `TransferBoolToValue`）。
pub fn TransferBoolToValue(enable: bool) -> &'static str {
    if enable { "ON" } else { "OFF" }
}

/// 从 TiDB InfoSchema 读取表元数据（Go `GetTableSchema`）。
pub fn GetTableSchema(dom: &dyn Domain, db_name: &CIStr, table_name: &CIStr) -> Result<TableInfo> {
    dom.InfoSchema()
        .TableByName(db_name, table_name)
        .map_err(Error::Trace)
}

/// 断言集群无用户库表，供“空集群还原”前置检查（Go `AssertUserDBsEmpty`）。
///
/// 跳过内存/系统库；空 `test` 库不算用户库；最多列举 `maxUserTablesNum` 项后截断。
pub fn AssertUserDBsEmpty(dom: &dyn Domain) -> Result<()> {
    let databases = dom.InfoSchema().AllSchemas();
    let m = dom.MetaReader();
    let mut user_tables: Vec<String> = Vec::with_capacity(maxUserTablesNum + 1);
    let mut append_tables = |db_name: &str, table_name: &str| -> bool {
        if user_tables.len() >= maxUserTablesNum {
            user_tables.push("...".into());
            return true;
        }
        user_tables.push(format!("{db_name}.{table_name}"));
        false
    };

    'listdbs: for db in databases {
        let db_name = db.Name.L.clone();
        if IsMemOrSysDB(&db_name) {
            continue;
        }
        let tables = m.ListSimpleTables(db.ID).map_err(|err| {
            Error::Annotatef(
                err,
                format!("failed to iterator tables of database[id={}]", db.ID),
            )
        })?;
        if tables.is_empty() {
            // 全新集群会建空 test 库，不计入用户库（Go LISTDBS 同分支）。
            if db_name != "test" && append_tables(&db.Name.O, "") {
                break 'listdbs;
            }
            continue;
        }
        for table in tables {
            if append_tables(&db.Name.O, &table.Name.O) {
                break 'listdbs;
            }
        }
    }
    if !user_tables.is_empty() {
        return Err(Error::Annotate(
            berrors::ErrRestoreNotFreshCluster("user db/tables"),
            format!("user db/tables: {}", user_tables.join(", ")),
        ));
    }
    Ok(())
}

/// 向 PD 获取物理/逻辑 TS 并合成 TSO（Go `GetTS` / `oracle.ComposeTS`）。
pub fn GetTS(ctx: &Context, pd_client: &dyn PdClient) -> Result<u64> {
    let (p, l) = pd_client.GetTS(ctx).map_err(Error::Trace)?;
    Ok(ComposeTS(p, l))
}

/// 带 Aggressive backoff 重试的 GetTS（Go `GetTSWithRetry` + `WithRetry`）。
///
/// 保留最后一次 GetTS 原始错误以便 Trace；Go 侧 failpoint 在 Rust 桩中未复刻。
pub fn GetTSWithRetry(ctx: &Context, pd_client: &dyn PdClient) -> Result<u64> {
    let mut start_ts = 0u64;
    let mut get_ts_err: Option<Error> = None;
    let mut retry = 0u32;

    let err = WithRetryAggressive(ctx, || match GetTS(ctx, pd_client) {
        Ok(ts) => {
            start_ts = ts;
            get_ts_err = None;
            Ok(())
        }
        Err(err) => {
            retry += 1;
            log::Warn("failed to get TS, retry it");
            get_ts_err = Some(err.clone());
            Err(err)
        }
    });

    if let Err(ref e) = err {
        log::Error("failed to get TS");
        let _ = e;
    }
    // 成功返回 start_ts；失败优先包装最后一次 GetTS 错误。
    match err {
        Ok(()) => Ok(start_ts),
        Err(e) => Err(Error::Trace(get_ts_err.unwrap_or(e))),
    }
}

/// 检测 `mysql.tidb_pitr_id_map` 是否含 `restore_id` 列（版本/迁移探测）。
pub fn HasRestoreIDColumn(dom: &dyn Domain) -> bool {
    let table = match GetTableSchema(dom, &CIStr::new("mysql"), &CIStr::new("tidb_pitr_id_map")) {
        Ok(t) => t,
        Err(_) => return false,
    };
    table.Columns.iter().any(|col| col.Name.L == "restore_id")
}

/// 带 Region 列表缓存的扫描器，对齐 Go `regionScanner`。
///
/// `region_cache` 按 StartKey 有序；`cache_size` 为单次 ScanRegions 上限。
pub struct regionScanner {
    region_client: ArcSplitClient,
    region_cache: Vec<RegionInfo>,
    cache_size: i32,
}

/// Thin Arc wrapper so `regionScanner` can own a SplitClient.
/// 将 `Arc<dyn SplitClient>` 包装为 `SplitClient`，便于 struct 持有共享客户端。
pub struct ArcSplitClient(pub std::sync::Arc<dyn SplitClient>);

impl SplitClient for ArcSplitClient {
    fn ScanRegions(
        &self,
        ctx: &Context,
        key: &[u8],
        end_key: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionInfo>> {
        self.0.ScanRegions(ctx, key, end_key, limit)
    }
}

/// 构造 Region 扫描器；`cache_size` 传给 `ScanRegionsWithRetry`（批分组默认 64）。
pub fn NewRegionScanner(
    region_client: std::sync::Arc<dyn SplitClient>,
    cache_size: i32,
) -> regionScanner {
    regionScanner {
        region_client: ArcSplitClient(region_client),
        region_cache: Vec::new(),
        cache_size,
    }
}

impl regionScanner {
    /// 远程 ScanRegions 并整批替换缓存，返回覆盖 `key` 的首个 Region。
    pub fn locateRegionFromRemote(&mut self, ctx: &Context, key: &[u8]) -> Result<RegionInfo> {
        let region_infos =
            ScanRegionsWithRetry(ctx, &self.region_client, key, &[], self.cache_size)
                .map_err(Error::Trace)?;
        if region_infos.is_empty() {
            return Err(Error::new("no region found"));
        }
        self.region_cache = region_infos;
        Ok(self.region_cache[0].clone())
    }

    /// 优先查缓存；key 超出末 Region 或未命中则回退 remote（Go `locateRegionFromCache`）。
    pub fn locateRegionFromCache(&mut self, ctx: &Context, key: &[u8]) -> Result<RegionInfo> {
        if self.region_cache.is_empty() {
            return self.locateRegionFromRemote(ctx, key);
        }
        let last_end = self
            .region_cache
            .last()
            .and_then(|r| r.Region.as_ref())
            .map(|r| r.EndKey.clone())
            .unwrap_or_default();
        if !last_end.is_empty() && key >= last_end.as_slice() {
            // key 已越过缓存尾 Region，需重新拉取。
            return self.locateRegionFromRemote(ctx, key);
        }
        let found = self.region_cache.iter().position(|region_info| {
            let region = match &region_info.Region {
                Some(r) => r,
                None => return false,
            };
            region.StartKey.as_slice() <= key
                && (region.EndKey.is_empty() || region.EndKey.as_slice() > key)
        });
        let Some(i) = found else {
            return self.locateRegionFromRemote(ctx, key);
        };
        // 丢弃已扫描过的前缀，保留命中 Region 及其后继。
        self.region_cache.drain(0..i);
        Ok(self.region_cache[0].clone())
    }

    /// 判断 `[start_key, end_key)` 是否落在同一 Region 内（EndKey 空表示无上界）。
    pub fn IsKeyRangeInOneRegion(
        &mut self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
    ) -> Result<bool> {
        let region_info = self.locateRegionFromCache(ctx, start_key)?;
        let region = region_info
            .Region
            .as_ref()
            .ok_or_else(|| Error::new("region missing"))?;
        Ok(region.EndKey.is_empty() || end_key < region.EndKey.as_slice())
    }
}

/// BackupFileSet 及其在 TiKV 上的 rewrite 后键范围。
pub struct BackupFileSetWithKeyRange {
    pub backupFileSet: BackupFileSet,
    pub startKey: Vec<u8>,
    pub endKey: Vec<u8>,
}

/// 将重叠 SST 的 BackupFileSet 合并并按 Region 边界切 batch 回调（Go 同名函数）。
///
/// 流程：算各 set 键范围 → 按 start/end 排序 → 扫描 region → 非重叠处开新 set，
/// 重叠处合并 SST；`[lastEndKey, startKey)` 跨 Region 时 flush 当前 batch。
pub fn GroupOverlappedBackupFileSetsIter(
    ctx: &Context,
    region_client: std::sync::Arc<dyn SplitClient>,
    backup_file_sets: Vec<BackupFileSet>,
    mut fn_: impl FnMut(BatchBackupFileSet),
) -> Result<()> {
    let mut with_ranges = Vec::with_capacity(backup_file_sets.len());
    for backup_file_set in backup_file_sets {
        let (start_key, end_key) = getKeyRangeForBackupFileSet(&backup_file_set)?;
        with_ranges.push(BackupFileSetWithKeyRange {
            backupFileSet: backup_file_set,
            startKey: start_key,
            endKey: end_key,
        });
    }
    with_ranges.sort_by(|a, b| match a.startKey.cmp(&b.startKey) {
        Ordering::Equal => a.endKey.cmp(&b.endKey),
        other => other,
    });

    let mut region_scanner = NewRegionScanner(region_client, 64);
    let mut this_backup_file_set: Option<BackupFileSet> = None;
    let mut this_batch: BatchBackupFileSet = Vec::new();
    let mut last_end_key: Vec<u8> = Vec::new();

    for file in with_ranges {
        if last_end_key.as_slice() < file.startKey.as_slice() {
            // 与当前合并窗口不重叠：收尾旧 set，开启新 set。
            if let Some(current) = this_backup_file_set.take() {
                this_batch.push(current);
            }
            this_backup_file_set = Some(BackupFileSet {
                TableID: file.backupFileSet.TableID,
                SSTFiles: file.backupFileSet.SSTFiles.clone(),
                RewriteRules: file.backupFileSet.RewriteRules.clone(),
            });
            let in_one_region =
                region_scanner.IsKeyRangeInOneRegion(ctx, &last_end_key, &file.startKey)?;
            if !in_one_region && !this_batch.is_empty() {
                // 间隙跨 Region：先输出已累积 batch，再续扫。
                log::Info("generating one batch.");
                fn_(std::mem::take(&mut this_batch));
            }
            last_end_key = file.endKey;
        } else {
            // 键范围重叠：合并 SST，TableID 与 RewriteRules 必须一致。
            let current = this_backup_file_set
                .as_mut()
                .ok_or_else(|| Error::new("overlapped backup file set missing current set"))?;
            current
                .SSTFiles
                .extend(file.backupFileSet.SSTFiles.iter().cloned());
            let rules_equal = match (&current.RewriteRules, &file.backupFileSet.RewriteRules) {
                (Some(a), Some(b)) => a.Equal(b),
                (None, None) => true,
                _ => false,
            };
            if current.TableID != file.backupFileSet.TableID || !rules_equal {
                log::Error("the overlapped SST must have the same table id and rewrite rules");
                return Err(Error::new(format!(
                    "the overlapped SST must have the same table id({}<>{}) and rewrite rules",
                    current.TableID, file.backupFileSet.TableID
                )));
            }
            if last_end_key.as_slice() < file.endKey.as_slice() {
                // 扩大当前窗口上界。
                last_end_key = file.endKey;
            }
        }
    }
    if let Some(current) = this_backup_file_set {
        this_batch.push(current);
    }
    if !this_batch.is_empty() {
        // 输出最后一个 batch（Go 循环结束后同样 flush）。
        log::Info("generating one batch.");
        fn_(this_batch);
    }
    Ok(())
}

/// 汇总单个 BackupFileSet 内所有 SST 的 rewrite 键范围 min/max（Go 同名）。
fn getKeyRangeForBackupFileSet(backup_file_set: &BackupFileSet) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut start_key = Vec::new();
    let mut end_key = Vec::new();
    for f in &backup_file_set.SSTFiles {
        let (start, end) = GetRewriteRawKeys(f, backup_file_set.RewriteRules.as_ref())
            .map_err(|e| Error::new(format!("{e}")))?;
        let start = start.unwrap_or_default();
        let end = end.unwrap_or_default();
        if start_key.is_empty() || start.as_slice() < start_key.as_slice() {
            start_key = start;
        }
        if end_key.is_empty() || end_key.as_slice() < end.as_slice() {
            end_key = end;
        }
    }
    Ok((start_key, end_key))
}

// --- minimal protobuf codec for LogRestoreTableIDsBlocklistFile ---
// 手工 protobuf 编解码，字段号与 Go gogo 代码生成一致，避免引入完整 proto 依赖。

/// 编码 protobuf varint（7 位一组，高位 continuation bit）。
fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// 写入 field tag：`(field_number << 3) | wire_type`。
fn encode_key(field: u32, wire: u8, out: &mut Vec<u8>) {
    encode_varint(((field as u64) << 3) | (wire as u64), out);
}

/// varint 字段；值为 0 时省略（proto3 默认不编码）。
fn encode_varint_field(field: u32, v: u64, out: &mut Vec<u8>) {
    if v == 0 {
        return;
    }
    encode_key(field, 0, out);
    encode_varint(v, out);
}

/// length-delimited 字节字段（wire type 2）。
fn encode_bytes_field(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    encode_key(field, 2, out);
    encode_varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

/// packed repeated int64：先拼 packed payload 再作为 bytes 字段写出。
fn encode_packed_i64(field: u32, values: &[i64], out: &mut Vec<u8>) {
    if values.is_empty() {
        return;
    }
    let mut packed = Vec::new();
    for v in values {
        encode_varint(*v as u64, &mut packed);
    }
    encode_key(field, 2, out);
    encode_varint(packed.len() as u64, out);
    out.extend_from_slice(&packed);
}

/// 序列化黑名单消息；字段 1/3/4/5/6/7 对应 Go struct tag。
/// 注意字段 2 历史未使用，刻意跳号以保持与 Go gogo 编号一致。
fn proto_marshal(m: &LogRestoreTableIDsBlocklistFile) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // field 1: RestoreCommitTs（varint）
    encode_varint_field(1, m.RestoreCommitTs, &mut out);
    // field 3: TableIds（packed repeated int64）
    encode_packed_i64(3, &m.TableIds, &mut out);
    // field 4: Checksum（bytes）
    encode_bytes_field(4, &m.Checksum, &mut out);
    // field 5: DbIds（packed repeated int64）
    encode_packed_i64(5, &m.DbIds, &mut out);
    // field 6/7: RewriteTs / RestoreStartTs
    encode_varint_field(6, m.RewriteTs, &mut out);
    encode_varint_field(7, m.RestoreStartTs, &mut out);
    Ok(out)
}

/// 解码 varint；遇截断或 shift 溢出返回错误。
fn decode_varint(data: &[u8], idx: &mut usize) -> Result<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if *idx >= data.len() {
            return Err(Error::new("truncated varint"));
        }
        let b = data[*idx];
        *idx += 1;
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift > 63 {
            return Err(Error::new("varint overflow"));
        }
    }
}

/// 解析 packed repeated int64 的 payload。
fn decode_packed_i64(buf: &[u8]) -> Result<Vec<i64>> {
    let mut idx = 0;
    let mut out = Vec::new();
    while idx < buf.len() {
        let v = decode_varint(buf, &mut idx)?;
        out.push(v as i64);
    }
    Ok(out)
}

/// 反序列化黑名单；兼容 packed 与非 packed repeated，未知字段按 wire type 跳过。
fn proto_unmarshal(data: &[u8]) -> Result<LogRestoreTableIDsBlocklistFile> {
    let mut m = LogRestoreTableIDsBlocklistFile::default();
    let mut idx = 0;
    while idx < data.len() {
        let tag = decode_varint(data, &mut idx)?;
        let field = (tag >> 3) as u32;
        let wire = (tag & 0x7) as u8;
        match (field, wire) {
            (1, 0) => m.RestoreCommitTs = decode_varint(data, &mut idx)?,
            (6, 0) => m.RewriteTs = decode_varint(data, &mut idx)?,
            (7, 0) => m.RestoreStartTs = decode_varint(data, &mut idx)?,
            (3, 2) => {
                // packed table_ids：先读长度再解析 payload。
                let len = decode_varint(data, &mut idx)? as usize;
                if idx + len > data.len() {
                    return Err(Error::new("truncated packed table_ids"));
                }
                m.TableIds.extend(decode_packed_i64(&data[idx..idx + len])?);
                idx += len;
            }
            (3, 0) => {
                // non-packed repeated
                // 旧格式或非 packed 编码的单个 table_id 元素。
                m.TableIds.push(decode_varint(data, &mut idx)? as i64);
            }
            (5, 2) => {
                // packed db_ids，语义同 table_ids。
                let len = decode_varint(data, &mut idx)? as usize;
                if idx + len > data.len() {
                    return Err(Error::new("truncated packed db_ids"));
                }
                m.DbIds.extend(decode_packed_i64(&data[idx..idx + len])?);
                idx += len;
            }
            (5, 0) => {
                // 非 packed 单个 db_id。
                m.DbIds.push(decode_varint(data, &mut idx)? as i64);
            }
            (4, 2) => {
                // checksum 原始字节原样保留，后续与重算值比较。
                let len = decode_varint(data, &mut idx)? as usize;
                if idx + len > data.len() {
                    return Err(Error::new("truncated checksum"));
                }
                m.Checksum = data[idx..idx + len].to_vec();
                idx += len;
            }
            (_, 0) => {
                // 未知 varint 字段：读掉值继续。
                let _ = decode_varint(data, &mut idx)?;
            }
            (_, 1) => {
                // fixed64
                idx += 8;
            }
            (_, 2) => {
                // length-delimited：跳过 payload。
                let len = decode_varint(data, &mut idx)? as usize;
                idx += len;
            }
            (_, 5) => {
                // fixed32
                idx += 4;
            }
            _ => return Err(Error::new(format!("unsupported wire type {wire}"))),
        }
    }
    Ok(m)
}
