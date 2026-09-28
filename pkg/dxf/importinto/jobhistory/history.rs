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

// IMPORT INTO 历史作业信息查询与展示格式化。
//
// 从 `tidb_global_task_history` / `tidb_background_subtask_history` 聚合
// 任务元数据、KV 体积、分步骤耗时与吞吐（每核/整体），供历史作业展示。

use std::collections::HashMap;

use crate::{Context, Error, errors, injectfailpoint, proto, storage, taskkey};

/// IMPORT INTO 历史作业各阶段耗时记录。
/// Duration records elapsed time of an IMPORT INTO history job.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[allow(non_snake_case)]
pub struct Duration {
    #[serde(rename = "total")]
    /// 全流程总耗时。
    pub Total: String,
    #[serde(rename = "encode")]
    /// 编码排序阶段耗时。
    pub Encode: String,
    #[serde(rename = "merge_sort")]
    /// 归并排序阶段耗时。
    pub MergeSort: String,
    #[serde(rename = "ingest")]
    /// 写入/ingest 阶段耗时。
    pub Ingest: String,
    #[serde(rename = "collect_conflicts")]
    /// 冲突收集阶段耗时。
    pub CollectConflicts: String,
    #[serde(rename = "resolve_conflicts")]
    /// 冲突解决阶段耗时。
    pub ResolveConflicts: String,
    #[serde(rename = "post_process")]
    /// 后处理阶段耗时。
    pub PostProcess: String,
}

/// 单条 IMPORT INTO 历史作业的详细展示信息。
/// Info contains detailed information for one IMPORT INTO history job.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[allow(non_snake_case)]
pub struct Info {
    #[serde(rename = "job_id")]
    /// 导入作业 ID。
    pub JobID: i64,
    #[serde(rename = "keyspace")]
    /// 作业所属 keyspace。
    pub Keyspace: String,
    #[serde(rename = "task_id")]
    /// 历史任务 ID。
    pub TaskID: i64,
    #[serde(rename = "state")]
    /// 任务状态字符串。
    pub State: String,
    #[serde(rename = "concurrency")]
    /// 任务并发度。
    pub Concurrency: i32,
    #[serde(rename = "max_node_count")]
    /// 最大参与节点数。
    pub MaxNodeCount: i32,
    #[serde(rename = "distsql_scan_concurrency")]
    /// DistSQL 扫描并发。
    pub DistSQLScanConcurrency: i32,
    #[serde(rename = "index_count")]
    /// 目标表索引个数。
    pub IndexCount: i32,
    #[serde(rename = "column_count")]
    /// 目标表列个数。
    pub ColumnCount: i32,
    #[serde(rename = "file_size")]
    /// 源文件总大小（人类可读）。
    pub FileSize: String,
    #[serde(rename = "data_kv_size")]
    /// 数据 KV 体积（人类可读）。
    pub DataKVSize: String,
    #[serde(rename = "index_kv_size")]
    /// 索引 KV 体积（人类可读）。
    pub IndexKVSize: String,
    #[serde(rename = "per_core_speed")]
    /// 每核每小时吞吐。
    pub PerCoreSpeed: String,
    #[serde(rename = "overall_speed")]
    /// 整体每小时吞吐。
    pub OverallSpeed: String,
    #[serde(rename = "row_count")]
    /// 导入行数。
    pub RowCount: i64,
    #[serde(rename = "row_length")]
    /// 平均行长（文件字节/行数）。
    pub RowLength: i64,
    #[serde(rename = "duration")]
    /// 各阶段耗时。
    pub Duration: Duration,
}

/// 仅从历史表查询 IMPORT INTO 作业信息；无匹配任务时返回 ErrTaskNotFound。
/// GetFromHistory returns IMPORT INTO job info from history tables only.
/// It returns ErrTaskNotFound when no matching history task exists.
#[allow(non_snake_case)]
pub fn GetFromHistory(
    ctx: Context,
    mgr: &storage::TaskManager,
    keyspace: &str,
    jobID: i64,
) -> Result<Info, Error> {
    // 注入随机失败点，便于测试错误路径。
    injectfailpoint::DXFRandomErrorWithOnePercent()?;

    let taskKey = taskkey::ForJobInKeyspace(keyspace.to_owned(), jobID);
    // 子任务历史表的 task_key 列存的是任务 ID，而非 task-key 字符串。
    // mysql.tidb_background_subtask_history.task_key stores task ID, not the task-key string.
    let rows = mgr.ExecuteSQLWithNewSession(
        ctx.clone(),
        r#"
            select
                t.id,
                t.state,
                t.concurrency,
                t.max_node_count,
                cast(json_extract(cast(cast(t.meta as char) as json), '$.Plan.DistSQLScanConcurrency') as signed) as distsql_scan_concurrency,
                cast(json_length(json_extract(cast(cast(t.meta as char) as json), '$.Plan.DesiredTableInfo.index_info')) as signed) as index_count,
                cast(json_length(json_extract(cast(cast(t.meta as char) as json), '$.Plan.DesiredTableInfo.cols')) as signed) as column_count,
                cast(json_extract(cast(cast(t.meta as char) as json), '$.Plan.TotalFileSize') as signed) as file_size_bytes,
                cast(json_extract(cast(cast(t.meta as char) as json), '$.Summary."row-count"') as signed) as row_count
            from mysql.tidb_global_task_history t
            where t.task_key = %? and t.type = %?"#,
        vec![taskKey.into(), proto::ImportInto.into()],
    )?;
    // 历史中无该 job 则标注 ErrTaskNotFound。
    if rows.is_empty() {
        return Err(errors::Annotatef(
            storage::ErrTaskNotFound,
            format!(
                "import-into job {} in keyspace {} not found in history",
                jobID, keyspace
            ),
        ));
    }

    let row = &rows[0];
    let mut info = Info {
        JobID: jobID,
        Keyspace: keyspace.to_owned(),
        TaskID: row.GetInt64(0),
        State: row.GetString(1),
        Concurrency: row.GetInt64(2) as i32,
        MaxNodeCount: row.GetInt64(3) as i32,
        ..Info::default()
    };
    if !row.IsNull(4) {
        info.DistSQLScanConcurrency = row.GetInt64(4) as i32;
    }
    if !row.IsNull(5) {
        info.IndexCount = row.GetInt64(5) as i32;
    }
    if !row.IsNull(6) {
        info.ColumnCount = row.GetInt64(6) as i32;
    }

    // 文件总字节用于平均行长与吞吐计算。
    let mut totalFileBytes: i64 = 0;
    if !row.IsNull(7) {
        totalFileBytes = row.GetInt64(7);
        info.FileSize = formatBytes(totalFileBytes);
    }
    if !row.IsNull(8) {
        info.RowCount = row.GetInt64(8);
    }
    // 平均行长 = 文件字节 / 行数（四舍五入）。
    if info.RowCount > 0 {
        info.RowLength = (totalFileBytes as f64 / info.RowCount as f64).round() as i64;
    }

    // 按 step + kv_group 聚合子任务字节与起止时间。
    let stepRows = mgr.ExecuteSQLWithNewSession(
        ctx,
        r#"
            select
                step,
                json_unquote(json_extract(cast(meta as char), '$."kv-group"')) as kv_group,
                cast(sum(cast(json_extract(summary, '$.bytes') as signed)) as signed) as bytes,
                min(case when start_time > 0 then start_time else null end) as min_start_time,
                max(case when state_update_time > 0 then state_update_time else null end) as max_state_update_time
            from mysql.tidb_background_subtask_history
            where task_key = %?
            group by step, kv_group"#,
        vec![info.TaskID.into()],
    )?;

    let mut dataKVSizeBytes: i64 = 0;
    let mut indexKVSizeBytes: i64 = 0;
    let mut hasDataKVSize = false;
    let mut hasIndexKVSize = false;
    let mut minStartTime: i64 = 0;
    let mut maxUpdateTime: i64 = 0;
    let mut hasTotalDuration = false;
    let mut stepDurations: HashMap<proto::Step, [i64; 2]> = HashMap::new();

    // 遍历聚合行：累计 data/index KV 体积，并维护全局与分步骤时间窗。
    for stepRow in stepRows {
        let step = stepRow.GetInt64(0) as proto::Step;
        let mut kvGroup = String::new();
        if !stepRow.IsNull(1) {
            kvGroup = stepRow.GetString(1);
        }
        if step == proto::ImportStepWriteAndIngest && !stepRow.IsNull(2) {
            // Go：kv_group=data 记为数据 KV，其余组记为索引 KV。
            // Go classifies kv_group=data as data KV and every other group as index KV.
            if kvGroup == "data" {
                dataKVSizeBytes += stepRow.GetInt64(2);
                hasDataKVSize = true;
            } else {
                indexKVSizeBytes += stepRow.GetInt64(2);
                hasIndexKVSize = true;
            }
        }

        // 缺开始或更新时间则无法计入耗时。
        if stepRow.IsNull(3) || stepRow.IsNull(4) {
            continue;
        }
        let startTime = stepRow.GetInt64(3);
        let updateTime = stepRow.GetInt64(4);
        if !hasTotalDuration {
            minStartTime = startTime;
            maxUpdateTime = updateTime;
            hasTotalDuration = true;
        } else {
            minStartTime = minStartTime.min(startTime);
            maxUpdateTime = maxUpdateTime.max(updateTime);
        }

        if let Some(existing) = stepDurations.get_mut(&step) {
            existing[0] = existing[0].min(startTime);
            existing[1] = existing[1].max(updateTime);
        } else {
            stepDurations.insert(step, [startTime, updateTime]);
        }
    }

    if hasDataKVSize {
        info.DataKVSize = formatBytes(dataKVSizeBytes);
    }
    if hasIndexKVSize {
        info.IndexKVSize = formatBytes(indexKVSizeBytes);
    }

    // 总耗时取所有子任务 [min_start, max_update] 跨度。
    let mut totalDurationSeconds: i64 = 0;
    if hasTotalDuration {
        totalDurationSeconds = (maxUpdateTime - minStartTime).max(0);
        info.Duration.Total = formatDuration(totalDurationSeconds);
    }
    info.PerCoreSpeed = formatBytesPerCoreHour(
        totalFileBytes,
        totalDurationSeconds,
        info.MaxNodeCount,
        info.Concurrency,
    );
    info.OverallSpeed = formatBytesPerHour(totalFileBytes, totalDurationSeconds);

    // 将各步骤时间窗映射到 Duration 字段。
    for (step, bounds) in stepDurations {
        let duration = formatDuration((bounds[1] - bounds[0]).max(0));
        match step {
            proto::ImportStepEncodeAndSort => info.Duration.Encode = duration,
            proto::ImportStepMergeSort => info.Duration.MergeSort = duration,
            proto::ImportStepWriteAndIngest => info.Duration.Ingest = duration,
            proto::ImportStepCollectConflicts => info.Duration.CollectConflicts = duration,
            proto::ImportStepConflictResolution => info.Duration.ResolveConflicts = duration,
            proto::ImportStepPostProcess => info.Duration.PostProcess = duration,
            _ => {}
        }
    }

    Ok(info)
}

/// 按 Go `time.Duration.String` 风格格式化整秒（如 `1h2m3s`）。
/// Formats whole seconds with Go time.Duration.String semantics.
#[allow(non_snake_case)]
pub fn formatDuration(seconds: i64) -> String {
    if seconds < 0 {
        return String::new();
    }
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        return format!("{hours}h{minutes}m{seconds}s");
    }
    if minutes > 0 {
        return format!("{minutes}m{seconds}s");
    }
    format!("{seconds}s")
}

/// 二进制单位字节格式化（对齐 docker/go-units BytesSize）。
fn formatBytesValue(mut size: f64) -> String {
    // 使用 KiB 进制与约四位有效数字格式。
    // docker/go-units BytesSize uses binary units and %.4g formatting.
    const UNITS: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let base = bytesize::KIB as f64;
    let mut unit = 0;
    while size >= base && unit < UNITS.len() - 1 {
        size /= base;
        unit += 1;
    }
    format!("{}{}", formatFourSignificantDigits(size), UNITS[unit])
}

#[allow(non_snake_case)]
/// 约四位有效数字：过大/过小用科学计数，否则定点并去尾零。
fn formatFourSignificantDigits(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }

    let exponent = value.abs().log10().floor() as i32;
    if exponent < -4 || exponent >= 4 {
        let formatted = format!("{value:.3e}");
        let (mantissa, exponent) = formatted
            .split_once('e')
            .expect("Rust scientific formatting always contains an exponent");
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let exponent: i32 = exponent
            .parse()
            .expect("Rust scientific formatting always emits an integer exponent");
        return format!("{mantissa}e{exponent:+03}");
    }

    let decimal_places = (3 - exponent).max(0) as usize;
    let formatted = format!("{value:.decimal_places$}");
    if formatted.contains('.') {
        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    } else {
        formatted
    }
}

/// 将字节整数格式化为人类可读大小；负值返回空串。
/// Formats bytes with docker/go-units BytesSize semantics.
#[allow(non_snake_case)]
pub fn formatBytes(size: i64) -> String {
    if size < 0 {
        return String::new();
    }
    formatBytesValue(size as f64)
}

/// 格式化整体吞吐（字节/小时）。
/// Formats total throughput per hour.
#[allow(non_snake_case)]
pub fn formatBytesPerHour(totalBytes: i64, durationSeconds: i64) -> String {
    if totalBytes < 0 || durationSeconds <= 0 {
        return String::new();
    }
    let bytesPerHour = totalBytes as f64 * 3600.0 / durationSeconds as f64;
    format!("{}/hour", formatBytesValue(bytesPerHour))
}

/// 格式化每核吞吐（字节/核/小时）；节点数与并发均须为正。
/// Formats throughput per core-hour.
#[allow(non_snake_case)]
pub fn formatBytesPerCoreHour(
    totalBytes: i64,
    durationSeconds: i64,
    maxNodeCount: i32,
    taskConcurrency: i32,
) -> String {
    if totalBytes < 0 || durationSeconds <= 0 || maxNodeCount <= 0 || taskConcurrency <= 0 {
        return String::new();
    }
    let cores = f64::from(maxNodeCount) * f64::from(taskConcurrency);
    let bytesPerCoreHour = totalBytes as f64 * 3600.0 / durationSeconds as f64 / cores;
    format!("{}/core/hour", formatBytesValue(bytesPerCoreHour))
}
