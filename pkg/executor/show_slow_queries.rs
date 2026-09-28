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

// `SHOW SLOW` / 慢查询展示执行器。
//
// 慢查询（Slow Query）指执行耗时超过阈值的语句记录，常用于性能诊断。
// 本模块在 Open 时一次性拉取结果，Next 时按 Chunk（列式结果批次）分页写出。

#![allow(non_snake_case)]

use astersql_types::datum::{Duration, Time};
use astersql_util_chunk::Chunk;

#[derive(Clone)]
/// 单条慢查询记录：SQL、起止时间、耗时、连接与事务等元数据。
pub struct SlowQueryInfo {
    pub sql: String,
    pub start: Time,
    pub duration: Duration,
    pub detail: String,
    pub success: bool,
    pub connection_id: u64,
    pub transaction_ts: u64,
    pub user: String,
    pub database: String,
    pub table_ids: String,
    pub index_names: String,
    pub internal: bool,
    pub digest: String,
    pub session_alias: String,
}

/// 慢查询数据源边界：打开、按请求拉取、以及最大 Chunk 行数。
pub trait ShowSlowSource {
    type Request;
    type Error;

    /// 打开底层慢日志/存储源。
    fn open(&mut self) -> Result<(), Self::Error>;
    /// 按 SHOW SLOW 请求参数返回匹配的慢查询列表。
    fn show_slow_query(&mut self, request: &Self::Request) -> Vec<SlowQueryInfo>;
    /// 单次 Next 可写入的最大行数。
    fn max_chunk_size(&self) -> usize;
}

/// 慢查询 SHOW 执行器：缓存全量结果并用 cursor 分页输出。
pub struct ShowSlowExec<S: ShowSlowSource> {
    pub source: S,
    pub show_slow: S::Request,
    pub result: Vec<SlowQueryInfo>,
    pub cursor: usize,
}

impl<S: ShowSlowSource> ShowSlowExec<S> {
    /// 打开数据源并物化全部慢查询到 `result`。
    pub fn Open<C>(&mut self, _ctx: C) -> Result<(), S::Error> {
        self.source.open()?;
        self.result = self.source.show_slow_query(&self.show_slow);
        Ok(())
    }

    /// 将剩余结果写入 Chunk，直至填满或耗尽。
    pub fn Next<C>(&mut self, _ctx: C, req: &mut Chunk) -> Result<(), S::Error> {
        // 按列顺序追加：SQL、开始时间、耗时、详情、成功标志等。
        req.Reset();
        while self.cursor < self.result.len() && req.NumRows() < self.source.max_chunk_size() {
            let slow = &self.result[self.cursor];
            req.AppendString(0, &slow.sql);
            req.AppendTime(1, slow.start);
            req.AppendDuration(2, slow.duration);
            req.AppendString(3, &slow.detail);
            req.AppendInt64(4, i64::from(slow.success));
            req.AppendUint64(5, slow.connection_id);
            req.AppendUint64(6, slow.transaction_ts);
            req.AppendString(7, &slow.user);
            req.AppendString(8, &slow.database);
            req.AppendString(9, &slow.table_ids);
            req.AppendString(10, &slow.index_names);
            req.AppendInt64(11, i64::from(slow.internal));
            req.AppendString(12, &slow.digest);
            req.AppendString(13, &slow.session_alias);
            self.cursor += 1;
        }
        Ok(())
    }
}
