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

// 通用批量刷盘器（BatchFlusher）。
//
// 将键值对先合并进内存缓冲，达到条数阈值或时间间隔后调用 `flush_fn` 写出。
// Go 实现在每次 flush 尝试后都会丢弃当前批次，避免失败数据混入后续批次；
// 成功/失败计数分别累计，便于指标对齐。

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::{Error, Result};

/// 按阈值与时间窗口批量刷出键值缓冲的通用结构。
pub struct BatchFlusher<K, V> {
    /// 刷盘器名称，用于日志/指标标签。
    pub name: String,
    /// 待刷出的键值缓冲。
    buffer: HashMap<K, V>,
    /// 距上次成功刷盘后允许等待的最长时间。
    interval: Duration,
    /// 缓冲达到该条数时立即刷盘；必须为正。
    threshold: usize,
    /// 最近一次刷盘时刻；空缓冲时为 None。
    last_flush_time: Option<Instant>,
    /// 入队时的合并函数（可做聚合，如累加 Repeats）。
    merge_fn: Box<dyn Fn(&mut HashMap<K, V>, K, V) + Send + Sync>,
    /// 实际写出缓冲的回调；失败时仍会清空缓冲，且错误只计数不外传。
    flush_fn: Box<dyn Fn(&HashMap<K, V>) -> Result<()> + Send + Sync>,
    /// stop 之后拒绝继续 add。
    stopped: bool,
    /// 刷盘成功次数。
    pub successful_flushes: u64,
    /// 刷盘失败次数。
    pub failed_flushes: u64,
    /// 累计 add 次数（含合并前后）。
    pub added: u64,
}

impl<K: Eq + Hash, V> BatchFlusher<K, V> {
    /// 构造刷盘器；`threshold == 0` 时返回 `InvalidArgument`。
    pub fn new(
        name: impl Into<String>,
        interval: Duration,
        threshold: usize,
        merge_fn: impl Fn(&mut HashMap<K, V>, K, V) + Send + Sync + 'static,
        flush_fn: impl Fn(&HashMap<K, V>) -> Result<()> + Send + Sync + 'static,
    ) -> Result<Self> {
        if threshold == 0 {
            return Err(Error::InvalidArgument(
                "flush threshold must be positive".into(),
            ));
        }
        Ok(Self {
            name: name.into(),
            buffer: HashMap::with_capacity(threshold),
            interval,
            threshold,
            last_flush_time: None,
            merge_fn: Box::new(merge_fn),
            flush_fn: Box::new(flush_fn),
            stopped: false,
            successful_flushes: 0,
            failed_flushes: 0,
            added: 0,
        })
    }

    /// 合并一条键值；达阈值则立即刷盘。已 stop 时返回 `Closed`。
    pub fn add(&mut self, key: K, value: V) -> Result<()> {
        if self.stopped {
            return Err(Error::Closed);
        }
        self.added += 1;
        (self.merge_fn)(&mut self.buffer, key, value);
        if self.buffer.len() >= self.threshold {
            self.flush()?;
        }
        Ok(())
    }

    /// 若距上次刷盘已超过 `interval`（或首次有数据），则刷盘；返回是否到期。
    pub fn flushIfDue(&mut self, now: Instant) -> Result<bool> {
        // 首次有数据即视为到期；否则按 interval 判断。
        let due = self
            .last_flush_time
            .map_or(!self.buffer.is_empty(), |last| {
                now.duration_since(last) >= self.interval
            });
        if due {
            self.flush()?;
        }
        Ok(due)
    }

    /// 将当前缓冲交给 `flush_fn`；无论成败都清空缓冲并更新时间戳。
    ///
    /// 与 Go 一致，写出错误由 flusher 记录后吞掉，不影响调用方继续入队。
    pub fn flush(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            self.last_flush_time = None;
            return Ok(());
        }
        let result = (self.flush_fn)(&self.buffer);
        if result.is_ok() {
            self.successful_flushes += 1;
        } else {
            self.failed_flushes += 1;
        }
        // Go deliberately drops the current batch after every attempt; failed data is not
        // mixed into a later batch and metrics describe exactly one attempt.
        // 与 Go 一致：每次尝试后丢弃当前批次，失败数据不混入后续批次。
        self.buffer = HashMap::with_capacity(self.threshold);
        self.last_flush_time = Some(Instant::now());
        Ok(())
    }

    /// 最后刷一次并标记为已停止。
    pub fn stop(&mut self) -> Result<()> {
        let result = self.flush();
        self.stopped = true;
        result
    }

    /// 当前缓冲条数。
    pub fn len(&self) -> usize {
        self.buffer.len()
    }
    /// 缓冲是否为空。
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
    /// 只读访问当前缓冲。
    pub fn buffer(&self) -> &HashMap<K, V> {
        &self.buffer
    }
}
