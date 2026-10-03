// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! DB/config helpers ported from `br/pkg/utils/db.go`.
//!
//! 通过 TiDB 受限 SQL 读取/写入 TiKV 相关配置，并维护日志备份任务计数。
//! 与 Go 版一致：读配置失败时回退默认值；写 GC ratio 失败则带注解上抛。
//! Region 切分参数供备份拆分 key range 时对齐集群实际配置。
//! 读路径容忍配置缺失；写路径失败必须可观测，避免静默留下错误 GC 阈值。
//! `logBackupTaskCount` 仅反映本进程任务，不跨节点同步。
//! SHOW CONFIG 固定取 Value 列（下标 3），与 Go Datum 下标保持一致。

use std::any::Any;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::stubs::{RestrictedSQLExecutor, ResultField};
use astersql_br_pkg_logutil::{Field, ShortError, log};
use astersql_errors::{Annotate, SharedError};
use astersql_parser_types::FieldType;
use bytesize::ByteSize;

/// TiDB 新排序规则开关在系统变量/元数据中的键名，需与 Go 字符串一致。
pub const TidbNewCollationEnabled: &str = "new_collation_enabled";

/// 进程内日志备份任务引用计数；>0 表示仍有日志备份在使用相关资源。
static logBackupTaskCount: AtomicI32 = AtomicI32::new(0);

/// 一次返回 Region 切分大小与 keys 上限，供备份调度复用。
pub fn GetRegionSplitInfo(ctx: &mut dyn RestrictedSQLExecutor) -> (u64, i64) {
    (GetSplitSize(ctx), GetSplitKeys(ctx))
}

/// 从 `SHOW CONFIG` 结果第 4 列取字段类型。
/// 列布局与 TiDB SHOW CONFIG 固定：Type/Instance/Name/Value。
fn field_type_at(fields: &[ResultField], idx: usize) -> &FieldType {
    &fields
        .get(idx)
        .expect("SHOW CONFIG result is missing the Value field")
        .column
        .as_ref()
        .expect("SHOW CONFIG Value field is missing column metadata")
        .FieldType
}

/// 读取 `coprocessor.region-split-size`；失败或空结果回退 96MiB。
/// 值按 ByteSize 解析（如 `10MB`），与 Go `units.RAMInBytes` 语义对齐。
/// 查询失败/空行/解析失败三条路径均回退默认，保证备份可继续。
pub fn GetSplitSize(ctx: &mut dyn RestrictedSQLExecutor) -> u64 {
    // 96MiB 与 TiKV 历史默认 region-split-size 对齐。
    const DEFAULT_SPLIT_SIZE: u64 = 96 * 1024 * 1024;
    let var_str = "show config where name = 'coprocessor.region-split-size' and type = 'tikv'";
    let (rows, fields) = match ctx.ExecRestrictedSQL(
        &Default::default(),
        Vec::new(),
        var_str,
        Vec::<Box<dyn Any>>::new(),
    ) {
        Ok(v) => v,
        Err(err) => {
            // 受限 SQL 执行失败：集群可能短暂不可用，降级继续。
            log::Warn(
                "failed to get split size, use default value",
                [ShortError(Some(&err))],
            );
            return DEFAULT_SPLIT_SIZE;
        }
    };
    if rows.is_empty() {
        // 未配置或过滤无匹配行时同样使用默认。
        return DEFAULT_SPLIT_SIZE;
    }
    // SHOW CONFIG 第 4 列为 value；与 Go 取 Datum 下标一致。
    let d = rows[0].GetDatum(3, field_type_at(&fields, 3));
    let split_size_str = match d.ToString() {
        Ok(v) => v,
        Err(err) => {
            // Datum 转字符串失败极少见，仍走默认避免中断。
            log::Warn(
                "failed to get split size, use default value",
                [ShortError(Some(&err))],
            );
            return DEFAULT_SPLIT_SIZE;
        }
    };
    match split_size_str.parse::<ByteSize>() {
        // 解析成功则使用集群真实切分大小。
        Ok(size) => size.as_u64(),
        Err(err) => {
            // 非法 size 字符串时同样降级，避免整次备份失败。
            log::Warn(
                "failed to get split size, use default value",
                [ShortError(Some(&err))],
            );
            DEFAULT_SPLIT_SIZE
        }
    }
}

/// 读取 `coprocessor.region-split-keys`；失败回退 960_000。
/// 与 GetSplitSize 相同的降级策略，避免切分参数半成功。
pub fn GetSplitKeys(ctx: &mut dyn RestrictedSQLExecutor) -> i64 {
    // 960_000 对应 Go 侧历史默认 region-split-keys。
    const DEFAULT_SPLIT_KEYS: i64 = 960_000;
    let var_str = "show config where name = 'coprocessor.region-split-keys' and type = 'tikv'";
    let (rows, fields) = match ctx.ExecRestrictedSQL(
        &Default::default(),
        Vec::new(),
        var_str,
        Vec::<Box<dyn Any>>::new(),
    ) {
        Ok(v) => v,
        Err(err) => {
            log::Warn(
                "failed to get split keys, use default value",
                [ShortError(Some(&err))],
            );
            return DEFAULT_SPLIT_KEYS;
        }
    };
    if rows.is_empty() {
        return DEFAULT_SPLIT_KEYS;
    }
    let d = rows[0].GetDatum(3, field_type_at(&fields, 3));
    let split_keys_str = match d.ToString() {
        Ok(v) => v,
        Err(err) => {
            log::Warn(
                "failed to get split keys, use default value",
                [ShortError(Some(&err))],
            );
            return DEFAULT_SPLIT_KEYS;
        }
    };
    match split_keys_str.parse::<i64>() {
        Ok(v) => v,
        Err(err) => {
            log::Warn(
                "failed to get split keys, use default value",
                [ShortError(Some(&err))],
            );
            DEFAULT_SPLIT_KEYS
        }
    }
}

/// 查询 `gc.ratio-threshold`；无行返回 `None`（与 Go 空结果语义一致）。
/// 与切分参数不同：此处失败上抛，由调用方决定是否禁用 GC。
pub fn GetGcRatio(ctx: &mut dyn RestrictedSQLExecutor) -> Result<Option<String>, SharedError> {
    let val_str = "show config where name = 'gc.ratio-threshold' and type = 'tikv'";
    let (rows, fields) = ctx
        .ExecRestrictedSQL(
            &Default::default(),
            Vec::new(),
            val_str,
            Vec::<Box<dyn Any>>::new(),
        )
        .map_err(|err| {
            SharedError::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                err.to_string(),
            ))
        })?;
    if rows.is_empty() {
        return Ok(None);
    }
    let d = rows[0].GetDatum(3, field_type_at(&fields, 3));
    Ok(Some(d.ToString().map_err(|err| SharedError::new(err))?))
}

/// 集群默认 GC ratio；备份前后恢复配置时常用。
pub const DefaultGcRatioVal: &str = "1.1";
/// 关闭 GC 触发用的哨兵值（`-1.0`），与 Go 常量一致。
pub const DisabledGcRatioVal: &str = "-1.0";

/// 设置 TiKV `gc.ratio-threshold`；失败时 Annotate 附带目标 ratio。
/// 备份窗口常先写成 DisabledGcRatioVal，结束后再恢复 DefaultGcRatioVal。
pub fn SetGcRatio(ctx: &mut dyn RestrictedSQLExecutor, ratio: &str) -> Result<(), SharedError> {
    ctx.ExecRestrictedSQL(
        &Default::default(),
        Vec::new(),
        "set config tikv `gc.ratio-threshold`=?",
        vec![Box::new(ratio.to_string()) as Box<dyn Any>],
    )
    .map_err(|err| {
        Annotate(
            Some(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                err.to_string(),
            ))),
            format!("failed to set config `gc.ratio-threshold`={ratio}"),
        )
        .unwrap()
    })?;
    // 配置变更影响 GC 行为，用 Warn 级别便于运维审计。
    log::Warn(
        "set config tikv gc.ratio-threshold",
        [Field::string("ratio", ratio)],
    );
    Ok(())
}

/// Restore temporarily limits RocksDB compaction concurrency to one job.
pub const RocksDBMaxBackgroundJobsForRestore: &str = "1";

pub fn GetRocksDBMaxBackgroundJobs(
    ctx: &mut dyn RestrictedSQLExecutor,
) -> Result<String, SharedError> {
    let (rows, fields) = ctx
        .ExecRestrictedSQL(
            &Default::default(),
            Vec::new(),
            "show config where name = 'rocksdb.max-background-jobs' and type = 'tikv'",
            Vec::new(),
        )
        .map_err(|err| SharedError::new(std::io::Error::other(err.to_string())))?;
    if rows.is_empty() {
        return Ok(String::new());
    }
    rows[0]
        .GetDatum(3, field_type_at(&fields, 3))
        .ToString()
        .map_err(SharedError::new)
}

pub fn SetRocksDBMaxBackgroundJobs(
    ctx: &mut dyn RestrictedSQLExecutor,
    jobs: &str,
) -> Result<(), SharedError> {
    ctx.ExecRestrictedSQL(
        &Default::default(),
        Vec::new(),
        "set config tikv `rocksdb.max-background-jobs`=?",
        vec![Box::new(jobs.to_string()) as Box<dyn Any>],
    )
    .map_err(|err| {
        Annotate(
            Some(SharedError::new(std::io::Error::other(err.to_string()))),
            format!("failed to set config `rocksdb.max-background-jobs`={jobs}"),
        )
        .unwrap()
    })?;
    log::Warn(
        "set config tikv rocksdb.max-background-jobs",
        [Field::string("jobs", jobs)],
    );
    Ok(())
}

/// 日志备份任务开始时 +1；计数为进程全局，多任务可叠加。
/// Relaxed 序足够：仅用于“是否存在任务”的粗粒度探测。
pub fn LogBackupTaskCountInc() {
    logBackupTaskCount.fetch_add(1, Ordering::Relaxed);
    log::L().Info(
        "inc log backup task",
        [Field::int(
            "count",
            logBackupTaskCount.load(Ordering::Relaxed) as i64,
        )],
    );
}

/// 日志备份任务结束时 -1；调用方需与 Inc 成对，避免假阳性占用。
/// 不在此做下溢保护，依赖上层生命周期配对。
pub fn LogBackupTaskCountDec() {
    logBackupTaskCount.fetch_sub(1, Ordering::Relaxed);
    log::L().Info(
        "dec log backup task",
        [Field::int(
            "count",
            logBackupTaskCount.load(Ordering::Relaxed) as i64,
        )],
    );
}

/// 是否仍有未结束的日志备份任务（计数 > 0）。
/// 供 GC/资源清理路径快速短路。
pub fn CheckLogBackupTaskExist() -> bool {
    logBackupTaskCount.load(Ordering::Relaxed) > 0
}

/// Go 版可结合 session 判断；当前 Rust 仅委托进程内计数。
/// `_ctx` 保留签名以便后续接入会话级探测而不改调用点。
pub fn IsLogBackupInUse<Ctx>(_ctx: &Ctx) -> bool {
    CheckLogBackupTaskExist()
}

/// 暴露新排序规则变量名，避免调用方硬编码字符串漂移。
pub fn GetTidbNewCollationEnabled() -> &'static str {
    TidbNewCollationEnabled
}
