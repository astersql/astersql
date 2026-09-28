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

// TTL 过期行删除任务与有限重试缓冲。
//
// `DeleteTask` 按主键 IN 列表与 TTL 列过期条件拼 DELETE；失败行进入
// `DeleteRetryBuffer`，在达到最大重试次数或缓冲满之前由 worker 再次投递。

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::scan::TtlStatistics;
use crate::session::{Datum, PhysicalTable, Row, SessionError, WorkerSession};

/// 单条删除任务允许的最大重试次数。
pub const DELETE_MAX_RETRY: usize = 3;
/// 重试缓冲中最多缓存的失败任务条数，防止内存无限增长。
pub const DELETE_RETRY_BUFFER_SIZE: usize = 128;
/// Go 版删除重试缓冲的默认轮询间隔。
pub const DELETE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Go 版 `tidb_ttl_delete_batch_size` 的默认值。
pub const DELETE_BATCH_SIZE: usize = 100;

/// 删除限流器：在真正执行 DELETE 前等待令牌，避免打满存储。
pub trait DeleteRateLimiter {
    /// 按本批行数申请删除令牌；失败时本批应视为未删除并进入重试。
    fn wait_delete_token(&mut self, rows: usize) -> Result<(), SessionError>;
}

/// 一次批量删除的输入：所属 job、目标物理表、待删行与过期时间戳。
#[derive(Clone, Debug)]
pub struct DeleteTask {
    /// 所属 TTL job 标识。
    pub job_id: String,
    /// 目标物理表（含 schema、主键列、TTL 列）。
    pub table: PhysicalTable,
    /// 待删除行，每行是主键列 Datum 序列。
    pub rows: Vec<Row>,
    /// TTL 过期阈值（Unix 时间），写入 DELETE 的过期条件。
    pub expire_time: u64,
    /// 与 Go `ttlDeleteTask.statistics` 一致，由原任务和重试任务共享。
    pub statistics: Arc<TtlStatistics>,
}

impl DeleteTask {
    /// 按主键列数与行数生成带占位符的 DELETE SQL（含 TTL 过期谓词）。
    pub fn delete_sql(&self, row_count: usize) -> String {
        let keys = self
            .table
            .key_columns
            .iter()
            .map(|column| format!("`{column}`"))
            .collect::<Vec<_>>()
            .join(",");
        let composite = self.table.key_columns.len() > 1;
        let key = if composite { format!("({keys})") } else { keys };
        let values = std::iter::repeat_n("%?", self.table.key_columns.len())
            .collect::<Vec<_>>()
            .join(",");
        let point = if composite {
            format!("({values})")
        } else {
            values
        };
        format!(
            "DELETE LOW_PRIORITY FROM `{}`.`{}` WHERE {key} IN ({}) AND `{}` < FROM_UNIXTIME(%?) LIMIT {row_count}",
            self.table.schema,
            self.table.table,
            std::iter::repeat_n(point, row_count)
                .collect::<Vec<_>>()
                .join(", "),
            self.table.ttl_column,
        )
    }

    /// 申请令牌后执行删除；成功返回空，失败或限流失败返回仍需重试的行。
    pub fn do_delete(
        &self,
        session: &mut dyn WorkerSession,
        limiter: &mut dyn DeleteRateLimiter,
    ) -> Vec<Row> {
        let mut retry_rows = Vec::new();
        for start in (0..self.rows.len()).step_by(DELETE_BATCH_SIZE) {
            let end = (start + DELETE_BATCH_SIZE).min(self.rows.len());
            let batch = &self.rows[start..end];
            // Go 会把当前批放入重试，但仍继续处理后续批次。
            if limiter.wait_delete_token(batch.len()).is_err() {
                retry_rows.extend(batch.iter().cloned());
                continue;
            }

            let task = DeleteTask {
                rows: batch.to_vec(),
                ..self.clone()
            };
            let sql = task.delete_sql(batch.len());
            // 参数顺序：各行主键列值 + 过期时间，与 Go SQL 子句顺序一致。
            let mut args = Vec::new();
            for row in batch {
                args.extend(row.iter().cloned());
            }
            args.push(Datum::Unsigned(self.expire_time));
            match session.execute(&sql, &args) {
                Ok(_) => self.statistics.add_success(batch.len()),
                Err(SessionError::NonRetryable(_)) => self.statistics.add_error(batch.len()),
                Err(_) => {
                    retry_rows.extend(batch.iter().cloned());
                }
            }
        }
        retry_rows
    }
}

/// 缓冲中的一条待重试项：原始任务、剩余行与已重试次数。
#[derive(Clone, Debug)]
struct RetryItem {
    task: DeleteTask,
    rows: Vec<Row>,
    retry_count: usize,
    in_time: Duration,
}

/// 有界 FIFO 重试队列：满或超过最大重试次数时丢弃并视为最终失败。
#[derive(Clone)]
pub struct DeleteRetryBuffer {
    items: VecDeque<RetryItem>,
    max_size: usize,
    max_retry: usize,
    retry_interval: Duration,
    get_time: Arc<dyn Fn() -> Duration + Send + Sync>,
    /// 被淘汰、达到重试上限或在 drain 时最终放弃的行数。
    discarded_rows: usize,
}

impl fmt::Debug for DeleteRetryBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeleteRetryBuffer")
            .field("items", &self.items)
            .field("max_size", &self.max_size)
            .field("max_retry", &self.max_retry)
            .field("retry_interval", &self.retry_interval)
            .field("discarded_rows", &self.discarded_rows)
            .finish_non_exhaustive()
    }
}

impl Default for DeleteRetryBuffer {
    fn default() -> Self {
        let start = std::time::Instant::now();
        Self::with_options(
            DELETE_RETRY_BUFFER_SIZE,
            DELETE_MAX_RETRY,
            DELETE_RETRY_INTERVAL,
            move || start.elapsed(),
        )
    }
}

impl DeleteRetryBuffer {
    /// 构造可注入时钟的重试缓冲，生产默认值与 Go `newTTLDelRetryBuffer` 一致。
    pub fn with_options(
        max_size: usize,
        max_retry: usize,
        retry_interval: Duration,
        get_time: impl Fn() -> Duration + Send + Sync + 'static,
    ) -> Self {
        Self {
            items: VecDeque::new(),
            max_size,
            max_retry,
            retry_interval,
            get_time: Arc::new(get_time),
            discarded_rows: 0,
        }
    }

    /// 当前重试轮询间隔。
    pub fn retry_interval(&self) -> Duration {
        self.retry_interval
    }
    /// 当前缓冲中的待重试项数量。
    pub fn len(&self) -> usize {
        self.items.len()
    }
    /// 返回因重试上限/缓冲满而最终放弃的行数。
    pub fn discarded_rows(&self) -> usize {
        self.discarded_rows
    }
    /// 记录一次删除结果：无剩余行不入队；否则以 retry_count=0 入队。
    pub fn record_task_result(&mut self, task: DeleteTask, rows: Vec<Row>) -> bool {
        if rows.is_empty() {
            return false;
        }
        self.record(task, rows, 0)
    }
    /// 在未超过重试次数时把失败行压入队尾；缓冲满时淘汰最旧项，
    /// 与 Go 的有界 FIFO 重试缓冲一致；返回当前项是否入队。
    fn record(&mut self, mut task: DeleteTask, rows: Vec<Row>, retry_count: usize) -> bool {
        if retry_count >= self.max_retry {
            self.discarded_rows += rows.len();
            task.statistics.add_error(rows.len());
            return false;
        }
        while !self.items.is_empty() && self.items.len() >= self.max_size {
            if let Some(evicted) = self.items.pop_front() {
                self.discarded_rows += evicted.rows.len();
                evicted.task.statistics.add_error(evicted.rows.len());
            }
        }
        task.rows = rows.clone();
        self.items.push_back(RetryItem {
            task,
            rows,
            retry_count,
            in_time: (self.get_time)(),
        });
        true
    }
    /// 对已到期项各重试一次，返回下一次应轮询的间隔。
    pub fn retry_all(&mut self, mut execute: impl FnMut(&DeleteTask) -> Vec<Row>) -> Duration {
        let count = self.items.len();
        for _ in 0..count {
            let Some(front) = self.items.front() else {
                break;
            };
            let elapsed = (self.get_time)().saturating_sub(front.in_time);
            if elapsed < self.retry_interval {
                return self.retry_interval - elapsed;
            }
            let Some(item) = self.items.pop_front() else {
                break;
            };
            let retry_rows = execute(&item.task);
            if !retry_rows.is_empty() {
                self.record(item.task, retry_rows, item.retry_count + 1);
            }
        }
        self.retry_interval
    }
    /// 清空缓冲，并把所有残留行按 Go 语义计为最终错误。
    pub fn drain(&mut self) {
        for item in self.items.drain(..) {
            item.task.statistics.add_error(item.rows.len());
            self.discarded_rows += item.rows.len();
        }
    }
}
