// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL 扫描任务：按主键范围分批查询过期行，并回调删除侧处理。
//
// 扫描 SQL 选择键列，条件为 TTL 列 `< expire_time`，可选游标续扫与 LIMIT。
// `TtlStatistics` 用原子计数跟踪总/成功/错误行，过高错误率时熔断终止。
// `ScanWorker` 同一时刻只持有一个进行中任务。

use std::sync::atomic::{AtomicU64, Ordering};

use crate::session::{Datum, PhysicalTable, Row, SessionError, WorkerSession};

/// 单次扫描 SQL 执行的最大重试次数。
pub const SCAN_TASK_EXECUTE_SQL_MAX_RETRY: usize = 5;

/// 扫描过程中的原子行计数统计。
#[derive(Debug, Default)]
pub struct TtlStatistics {
    total_rows: AtomicU64,
    success_rows: AtomicU64,
    error_rows: AtomicU64,
}

impl TtlStatistics {
    /// 累加扫描到的总行数。
    pub fn add_total(&self, count: usize) {
        self.total_rows.fetch_add(count as u64, Ordering::Relaxed);
    }
    /// 累加成功处理行数。
    pub fn add_success(&self, count: usize) {
        self.success_rows.fetch_add(count as u64, Ordering::Relaxed);
    }
    /// 累加失败行数。
    pub fn add_error(&self, count: usize) {
        self.error_rows.fetch_add(count as u64, Ordering::Relaxed);
    }
    /// 读取 (total, success, error) 快照。
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.total_rows.load(Ordering::Relaxed),
            self.success_rows.load(Ordering::Relaxed),
            self.error_rows.load(Ordering::Relaxed),
        )
    }
    /// 将三类计数清零。
    pub fn reset(&self) {
        self.total_rows.store(0, Ordering::Relaxed);
        self.success_rows.store(0, Ordering::Relaxed);
        self.error_rows.store(0, Ordering::Relaxed);
    }
    /// Restore a persisted scan task's counters after owner takeover.
    pub fn restore(&self, total: u64, success: u64, errors: u64) {
        self.total_rows.store(total, Ordering::Relaxed);
        self.success_rows.store(success, Ordering::Relaxed);
        self.error_rows.store(errors, Ordering::Relaxed);
    }
    /// 样本量严格超过 `sample_floor` 且错误率超过 `maximum` 时返回 true（熔断条件）。
    pub fn error_rate_too_high(&self, sample_floor: u64, maximum: f64) -> bool {
        let (total, _, errors) = self.snapshot();
        total > sample_floor && errors as f64 / total.max(1) as f64 > maximum
    }
}

/// 扫描任务终止原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskTerminateReason {
    /// 正常扫完（空批或末批不足 batch_size）。
    Finished,
    /// 外部取消。
    Canceled,
    /// 错误率超过阈值。
    ErrorRateExceeded,
    /// 表定义变更（元数据校验失败等）。
    TableChanged,
    /// 执行错误。
    Error,
    /// worker 缩容/停止；任务应回到 waiting 以便其它 worker 接管。
    WorkerStop,
}

/// 一次键范围扫描任务的描述。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TtlScanTask {
    /// 所属作业 ID。
    pub job_id: String,
    /// 作业内扫描子任务 ID。
    pub scan_id: i64,
    /// 目标物理表。
    pub table: PhysicalTable,
    /// 过期水位：TTL 列值早于此时间的行待清理。
    pub expire_time: u64,
    /// 扫描键范围下界（可选）。
    pub range_start: Option<Vec<Datum>>,
    /// 扫描键范围上界（可选）。
    pub range_end: Option<Vec<Datum>>,
    /// 每批 SELECT 的 LIMIT。
    pub batch_size: usize,
}

/// 扫描结束时返回的结果摘要。
#[derive(Clone, Debug)]
pub struct ScanResult {
    /// 作业 ID。
    pub job_id: String,
    /// 扫描子任务 ID。
    pub scan_id: i64,
    /// 终止原因。
    pub reason: TaskTerminateReason,
    /// 可选的会话/执行错误。
    pub error: Option<SessionError>,
    /// 已扫描行数。
    pub scanned_rows: u64,
}

impl TtlScanTask {
    /// 构造带游标续扫的 SELECT SQL 与参数列表。
    ///
    /// `cursor` 为上一批最后一行的键值；用于 `AND (keys) > (...)` 翻页。
    pub fn scan_sql(&self, cursor: Option<&[Datum]>) -> (String, Vec<Datum>) {
        let columns = self
            .table
            .key_columns
            .iter()
            .map(|column| format!("`{column}`"))
            .collect::<Vec<_>>()
            .join(",");
        let partition = self
            .table
            .partition_name
            .as_ref()
            .map(|name| format!(" PARTITION (`{}`)", name.replace('`', "``")))
            .unwrap_or_default();
        let mut sql = format!(
            "SELECT {columns} FROM `{}`.`{}`{partition} WHERE `{}` < FROM_UNIXTIME(%?)",
            self.table.schema, self.table.table, self.table.ttl_column
        );
        let mut args = vec![Datum::Unsigned(self.expire_time)];
        if let Some(range_start) = &self.range_start {
            if !range_start.is_empty() {
                let placeholders = std::iter::repeat_n("%?", range_start.len())
                    .collect::<Vec<_>>()
                    .join(",");
                sql.push_str(&format!(" AND ({columns}) >= ({placeholders})"));
                args.extend(range_start.iter().cloned());
            }
        }
        if let Some(range_end) = &self.range_end {
            if !range_end.is_empty() {
                let placeholders = std::iter::repeat_n("%?", range_end.len())
                    .collect::<Vec<_>>()
                    .join(",");
                sql.push_str(&format!(" AND ({columns}) < ({placeholders})"));
                args.extend(range_end.iter().cloned());
            }
        }
        if let Some(cursor) = cursor {
            let placeholders = std::iter::repeat_n("%?", cursor.len())
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(" AND ({columns}) > ({placeholders})"));
            args.extend_from_slice(cursor);
        }
        sql.push_str(&format!(
            " ORDER BY {columns} LIMIT {}",
            self.batch_size.max(1)
        ));
        (sql, args)
    }

    /// 循环分批扫描：检查取消与错误率，执行 SQL（可重试），回调删除，更新游标。
    pub fn execute(
        &self,
        session: &mut dyn WorkerSession,
        statistics: &TtlStatistics,
        emit_delete: impl FnMut(Vec<Row>) -> Result<(), SessionError>,
        canceled: impl Fn() -> bool,
    ) -> ScanResult {
        self.execute_with_checkpoint(session, statistics, None, emit_delete, |_| Ok(()), canceled)
    }

    /// Continue after a durable cursor and persist each fully dispatched
    /// batch. The checkpoint callback must succeed before the next page is
    /// scanned; a failed checkpoint leaves the previous cursor intact.
    pub fn execute_with_checkpoint(
        &self,
        session: &mut dyn WorkerSession,
        statistics: &TtlStatistics,
        mut cursor: Option<Row>,
        mut emit_delete: impl FnMut(Vec<Row>) -> Result<(), SessionError>,
        mut checkpoint: impl FnMut(&Row) -> Result<(), SessionError>,
        canceled: impl Fn() -> bool,
    ) -> ScanResult {
        let mut scanned = 0_u64;
        loop {
            if canceled() {
                return self.result(TaskTerminateReason::Canceled, None, scanned);
            }
            // 对齐 Go：样本量 ≥10000 且错误率 >40% 则熔断。
            if statistics.error_rate_too_high(10_000, 0.4) {
                return self.result(TaskTerminateReason::ErrorRateExceeded, None, scanned);
            }
            let (sql, args) = self.scan_sql(cursor.as_deref());
            let mut rows = None;
            let mut last_error = None;
            // Go permits the initial attempt plus the configured number of
            // retries.  A five-retry task therefore has six attempts.
            for _ in 0..=SCAN_TASK_EXECUTE_SQL_MAX_RETRY {
                let execution = session.execute(&sql, &args);
                // Go cancels the statement context and checks that context
                // before either retrying or dispatching the returned rows.
                // Preserve that statement-boundary ordering here as well.
                if canceled() {
                    return self.result(TaskTerminateReason::Canceled, None, scanned);
                }
                match execution {
                    Ok(result) => {
                        rows = Some(result);
                        break;
                    }
                    Err(
                        error @ (SessionError::NonRetryable(_)
                        | SessionError::TableChanged
                        | SessionError::TtlDisabled
                        | SessionError::ExpireIntervalChanged),
                    ) => {
                        return self.result(TaskTerminateReason::Error, Some(error), scanned);
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            let Some(rows) = rows else {
                return self.result(TaskTerminateReason::Error, last_error, scanned);
            };
            if rows.is_empty() {
                return self.result(TaskTerminateReason::Finished, None, scanned);
            }
            if let Err(error) = emit_delete(rows.clone()) {
                return self.result(TaskTerminateReason::Error, Some(error), scanned);
            }
            statistics.add_total(rows.len());
            scanned += rows.len() as u64;
            if let Some(last) = rows.last()
                && let Err(error) = checkpoint(last)
            {
                return self.result(TaskTerminateReason::Error, Some(error), scanned);
            }
            // Go increments TotalRows only after the delete task has been
            // dispatched successfully.  Rows rejected by a canceled/full
            // dispatch must not be counted as scanned work.
            cursor = rows.last().cloned();
            // 末批不足 batch_size 说明已扫完。
            if rows.len() < self.batch_size.max(1) {
                return self.result(TaskTerminateReason::Finished, None, scanned);
            }
        }
    }

    /// 组装 `ScanResult` 的内部辅助。
    fn result(
        &self,
        reason: TaskTerminateReason,
        error: Option<SessionError>,
        scanned_rows: u64,
    ) -> ScanResult {
        ScanResult {
            job_id: self.job_id.clone(),
            scan_id: self.scan_id,
            reason,
            error,
            scanned_rows,
        }
    }
}

/// 单任务扫描 worker：空闲时可 schedule，完成后通过 `poll_result` 取回结果。
#[derive(Clone, Debug, Default)]
pub struct ScanWorker {
    current: Option<TtlScanTask>,
    result: Option<ScanResult>,
}

impl ScanWorker {
    /// 当前无任务时可接受调度。
    pub fn could_schedule(&self) -> bool {
        self.current.is_none() && self.result.is_none()
    }
    /// 若空闲则接受任务并返回 true。
    pub fn schedule(&mut self, task: TtlScanTask) -> bool {
        if !self.could_schedule() {
            return false;
        }
        self.current = Some(task);
        true
    }
    /// 标记当前任务结束并缓存结果。
    pub fn finish(&mut self, result: ScanResult) {
        self.current = None;
        self.result = Some(result);
    }
    /// 取出并清空最近一次结果。
    pub fn poll_result(&mut self) -> Option<ScanResult> {
        self.result.take()
    }
    /// 查看当前进行中的任务（若有）。
    pub fn current_task(&self) -> Option<&TtlScanTask> {
        self.current.as_ref()
    }
}
