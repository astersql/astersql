// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 语句摘要（Statement Summary）系统表检索。
//
// Statement Summary 把相似 SQL（按 digest 归类）的执行次数、耗时、扫描行数等
// 聚合到内存或持久化存储，并通过 information_schema 中的
// `STATEMENTS_SUMMARY` / `_HISTORY` / `_EVICTED` 及对应 `CLUSTER_*` 表暴露。
//
// 本模块提供：
// - 表名分类谓词与时间范围转换；
// - [`RowsReader`]：按批拉取摘要行；
// - [`StmtSummaryRetriever`]：按 Legacy / Persistent / Dummy 模式组装结果。

#![allow(non_snake_case)]

/// 单次 retrieve 默认最多返回的行数。
pub const DEFAULT_RETRIEVE_COUNT: usize = 1024;

/// 当前窗口语句摘要表名。
pub const TABLE_STATEMENTS_SUMMARY: &str = "STATEMENTS_SUMMARY";
/// 历史窗口语句摘要表名。
pub const TABLE_STATEMENTS_SUMMARY_HISTORY: &str = "STATEMENTS_SUMMARY_HISTORY";
/// 因容量被驱逐的摘要条目表名。
pub const TABLE_STATEMENTS_SUMMARY_EVICTED: &str = "STATEMENTS_SUMMARY_EVICTED";
/// 累计语句统计表名。
pub const TABLE_TIDB_STATEMENTS_STATS: &str = "TIDB_STATEMENTS_STATS";
/// 集群视角的当前窗口摘要（带 instance 列）。
pub const CLUSTER_TABLE_STATEMENTS_SUMMARY: &str = "CLUSTER_STATEMENTS_SUMMARY";
/// 集群视角的历史摘要。
pub const CLUSTER_TABLE_STATEMENTS_SUMMARY_HISTORY: &str = "CLUSTER_STATEMENTS_SUMMARY_HISTORY";
/// 集群视角的驱逐摘要。
pub const CLUSTER_TABLE_STATEMENTS_SUMMARY_EVICTED: &str = "CLUSTER_STATEMENTS_SUMMARY_EVICTED";
/// 集群视角的累计统计。
pub const CLUSTER_TABLE_TIDB_STATEMENTS_STATS: &str = "CLUSTER_TIDB_STATEMENTS_STATS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 语句摘要查询使用的精确时间窗口（unix 秒）。
pub struct StmtTimeRange {
    pub begin: i64,
    pub end: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 优化器下推的粗粒度时间范围，随后转为 [`StmtTimeRange`]。
pub struct CoarseTimeRange {
    pub start_unix: i64,
    pub end_unix: i64,
}

/// 从计划侧提取的摘要过滤条件：digest 集合、时间范围、是否跳过请求。
pub struct StatementsSummaryExtractor<D> {
    pub digests: Option<D>,
    pub coarse_time_range: Option<CoarseTimeRange>,
    pub skip_request: bool,
}

/// 可关闭的行拉取器（用于历史表流式读取）。
pub trait RowsPuller<Row, E> {
    fn rows(&mut self) -> Result<Vec<Row>, E>;
    fn close(&mut self) -> Result<(), E>;
}

/// 缓冲行 + 可选 puller：先消耗缓冲，空了再向 puller 取。
pub struct RowsReader<Row, E> {
    pub puller: Option<Box<dyn RowsPuller<Row, E>>>,
    pub rows: Vec<Row>,
}

/// 仅内存缓冲、无后续 puller 的简易 reader。
pub fn newSimpleRowsReader<Row, E>(rows: Vec<Row>) -> RowsReader<Row, E> {
    RowsReader { puller: None, rows }
}

/// 带初始缓冲与后续 puller 的 reader（历史表：内存行 + 持久化历史）。
pub fn newRowsReader<Row, E>(
    rows: Vec<Row>,
    puller: Box<dyn RowsPuller<Row, E>>,
) -> RowsReader<Row, E> {
    RowsReader {
        puller: Some(puller),
        rows,
    }
}

impl<Row, E> RowsReader<Row, E> {
    /// 最多返回 `maximum_count` 行；不足时先 [`pull`] 补齐。
    pub fn read(&mut self, maximum_count: usize) -> Result<Vec<Row>, E> {
        self.pull()?;
        if maximum_count >= self.rows.len() {
            return Ok(std::mem::take(&mut self.rows));
        }
        // 拆出前缀页，剩余留在缓冲供下次读取。
        let remaining = self.rows.split_off(maximum_count);
        Ok(std::mem::replace(&mut self.rows, remaining))
    }

    /// 缓冲为空且仍有 puller 时拉取一批；空结果则关闭 puller。
    pub fn pull(&mut self) -> Result<(), E> {
        if self.puller.is_none() || !self.rows.is_empty() {
            return Ok(());
        }
        let rows = self.puller.as_mut().expect("checked above").rows()?;
        if !rows.is_empty() {
            self.rows = rows;
            return Ok(());
        }
        self.puller.as_mut().expect("checked above").close()?;
        self.puller = None;
        Ok(())
    }

    /// 关闭底层 puller（若存在）。
    pub fn close(&mut self) -> Result<(), E> {
        if let Some(puller) = &mut self.puller {
            puller.close()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 检索模式：跳过 / 仅内存 Legacy / 持久化 Persistent。
pub enum RetrieverMode {
    Dummy,
    Legacy,
    Persistent,
}

/// 运行时依赖抽象：权限、实例地址、Legacy/Persistent 数据源。
pub trait StatementSummaryRuntime {
    type Context;
    type Row;
    type Column: Clone;
    type Digests;
    type Error;

    fn persistent_enabled(&self) -> bool;
    fn digests_empty(&self, digests: &Self::Digests) -> bool;
    fn has_process_privilege(&self, context: &Self::Context) -> bool;
    fn process_privilege_denied(&self) -> Self::Error;
    /// Preserve the planner error when adapting it to the runtime error type.
    fn statement_summary_error(&self, error: astersql_errors::SharedError) -> Self::Error;
    fn instance_address(&self, context: &Self::Context) -> Result<String, Self::Error>;
    fn append_host_info(
        &mut self,
        context: &mut Self::Context,
        rows: Vec<Self::Row>,
    ) -> Result<Vec<Self::Row>, Self::Error>;
    fn adjust_columns(
        &self,
        rows: Vec<Self::Row>,
        columns: &[Self::Column],
        table_name: &str,
    ) -> Vec<Self::Row>;
    fn legacy_evicted_rows(&self) -> Vec<Self::Row>;
    fn legacy_summary_rows(
        &mut self,
        context: &mut Self::Context,
        table_name: &str,
        columns: &[Self::Column],
        digests: Option<&Self::Digests>,
        instance_address: String,
        process_privilege: bool,
    ) -> Result<Vec<Self::Row>, Self::Error>;
    fn persistent_evicted_row(&self) -> Option<Self::Row>;
    fn persistent_memory_rows(
        &mut self,
        context: &mut Self::Context,
        columns: &[Self::Column],
        digests: Option<&Self::Digests>,
        time_ranges: Option<&[StmtTimeRange]>,
        instance_address: String,
        process_privilege: bool,
    ) -> Result<Vec<Self::Row>, Self::Error>;
    #[allow(clippy::too_many_arguments)]
    fn persistent_history_puller(
        &mut self,
        context: &mut Self::Context,
        columns: &[Self::Column],
        digests: Option<&Self::Digests>,
        time_ranges: Option<&[StmtTimeRange]>,
        instance_address: String,
        process_privilege: bool,
    ) -> Result<Box<dyn RowsPuller<Self::Row, Self::Error>>, Self::Error>;
}

/// 语句摘要表检索器：持有模式、列投影、digest/时间过滤与行 reader。
pub struct StmtSummaryRetriever<R: StatementSummaryRuntime> {
    pub runtime: R,
    pub mode: RetrieverMode,
    pub table_name: String,
    pub columns: Vec<R::Column>,
    pub digests: Option<R::Digests>,
    pub time_ranges: Option<Vec<StmtTimeRange>>,
    pub rows_reader: Option<RowsReader<R::Row, R::Error>>,
}

/// 根据 extractor 与 runtime 能力选择 Dummy/Legacy/Persistent 并构造检索器。
pub fn buildStmtSummaryRetriever<R: StatementSummaryRuntime>(
    runtime: R,
    table_name: String,
    columns: Vec<R::Column>,
    extractor: Option<StatementsSummaryExtractor<R::Digests>>,
) -> StmtSummaryRetriever<R> {
    let extractor = extractor.unwrap_or(StatementsSummaryExtractor {
        digests: None,
        coarse_time_range: None,
        skip_request: false,
    });
    // 空 digest 集合视为无过滤。
    let digests = extractor
        .digests
        .filter(|digests| !runtime.digests_empty(digests));
    // skip_request → Dummy；否则按是否开启持久化选择模式。
    let mode = if extractor.skip_request {
        RetrieverMode::Dummy
    } else if runtime.persistent_enabled() {
        RetrieverMode::Persistent
    } else {
        RetrieverMode::Legacy
    };
    StmtSummaryRetriever {
        runtime,
        mode,
        table_name,
        columns,
        digests,
        time_ranges: buildTimeRanges(extractor.coarse_time_range),
        rows_reader: None,
    }
}

impl<R: StatementSummaryRuntime> StmtSummaryRetriever<R> {
    /// 拉取至多 [`DEFAULT_RETRIEVE_COUNT`] 行；Dummy 模式直接空结果。
    pub fn retrieve(&mut self, context: &mut R::Context) -> Result<Vec<R::Row>, R::Error> {
        if self.mode == RetrieverMode::Dummy {
            return Ok(Vec::new());
        }
        self.ensureRowsReader(context)?;
        self.rows_reader
            .as_mut()
            .expect("reader initialized")
            .read(DEFAULT_RETRIEVE_COUNT)
    }

    /// 关闭内部 rows_reader。
    pub fn close(&mut self) -> Result<(), R::Error> {
        if let Some(reader) = &mut self.rows_reader {
            reader.close()?;
        }
        Ok(())
    }

    /// 运行时统计占位（当前无额外 stats）。
    pub fn getRuntimeStats(&self) -> Option<()> {
        None
    }

    /// 惰性初始化 rows_reader：驱逐表与摘要表走不同路径。
    pub fn ensureRowsReader(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        if self.rows_reader.is_some() {
            return Ok(());
        }
        self.rows_reader = Some(if isEvictedTable(&self.table_name) {
            self.initEvictedRowsReader(context)?
        } else {
            self.initSummaryRowsReader(context)?
        });
        Ok(())
    }

    /// 初始化驱逐表 reader：校验 PROCESS 权限，集群表追加 host 信息。
    pub fn initEvictedRowsReader(
        &mut self,
        context: &mut R::Context,
    ) -> Result<RowsReader<R::Row, R::Error>, R::Error> {
        checkPrivilege(&self.runtime, context)?;
        let mut rows = match self.mode {
            RetrieverMode::Legacy => self.runtime.legacy_evicted_rows(),
            RetrieverMode::Persistent => {
                self.runtime.persistent_evicted_row().into_iter().collect()
            }
            RetrieverMode::Dummy => Vec::new(),
        };
        if isClusterTable(&self.table_name) {
            rows = self.runtime.append_host_info(context, rows)?;
        }
        Ok(newSimpleRowsReader(self.runtime.adjust_columns(
            rows,
            &self.columns,
            &self.table_name,
        )))
    }

    /// 初始化摘要表 reader：Legacy 一次取齐；Persistent 区分当前/历史表。
    pub fn initSummaryRowsReader(
        &mut self,
        context: &mut R::Context,
    ) -> Result<RowsReader<R::Row, R::Error>, R::Error> {
        if self.mode == RetrieverMode::Persistent && isCumulativeTable(&self.table_name) {
            let error = astersql_util_dbterror_plannererrors::ErrNotSupportedYet
                .GenWithStackByArgs(&[
                    "cumulative statement summary table with persistent mode (v2)".into(),
                ]);
            return Err(self.runtime.statement_summary_error(error));
        }
        let process_privilege = self.runtime.has_process_privilege(context);
        let instance_address = clusterTableInstanceAddr(&self.runtime, context, &self.table_name)?;
        if self.mode == RetrieverMode::Legacy {
            let rows = self.runtime.legacy_summary_rows(
                context,
                &self.table_name,
                &self.columns,
                self.digests.as_ref(),
                instance_address,
                process_privilege,
            )?;
            return Ok(newSimpleRowsReader(rows));
        }

        // Persistent：先取内存窗口行。
        let memory_rows = self.runtime.persistent_memory_rows(
            context,
            &self.columns,
            self.digests.as_ref(),
            self.time_ranges.as_deref(),
            instance_address.clone(),
            process_privilege,
        )?;
        if isCurrentTable(&self.table_name) {
            return Ok(newSimpleRowsReader(memory_rows));
        }
        // 历史表：内存行作首批，再挂持久化历史 puller。
        if isHistoryTable(&self.table_name) {
            let history = self.runtime.persistent_history_puller(
                context,
                &self.columns,
                self.digests.as_ref(),
                self.time_ranges.as_deref(),
                instance_address,
                process_privilege,
            )?;
            return Ok(newRowsReader(memory_rows, history));
        }
        Ok(newSimpleRowsReader(Vec::new()))
    }
}

/// 是否为 CLUSTER_* 语句摘要相关表。
pub fn isClusterTable(table: &str) -> bool {
    matches!(
        table,
        CLUSTER_TABLE_STATEMENTS_SUMMARY
            | CLUSTER_TABLE_STATEMENTS_SUMMARY_HISTORY
            | CLUSTER_TABLE_STATEMENTS_SUMMARY_EVICTED
            | CLUSTER_TABLE_TIDB_STATEMENTS_STATS
    )
}

/// 是否为累计统计表（非滑动窗口）。
pub fn isCumulativeTable(table: &str) -> bool {
    matches!(
        table,
        TABLE_TIDB_STATEMENTS_STATS | CLUSTER_TABLE_TIDB_STATEMENTS_STATS
    )
}

/// 是否为当前窗口摘要表。
pub fn isCurrentTable(table: &str) -> bool {
    matches!(
        table,
        TABLE_STATEMENTS_SUMMARY | CLUSTER_TABLE_STATEMENTS_SUMMARY
    )
}

/// 是否为历史窗口摘要表。
pub fn isHistoryTable(table: &str) -> bool {
    matches!(
        table,
        TABLE_STATEMENTS_SUMMARY_HISTORY | CLUSTER_TABLE_STATEMENTS_SUMMARY_HISTORY
    )
}

/// 是否为驱逐条目表。
pub fn isEvictedTable(table: &str) -> bool {
    matches!(
        table,
        TABLE_STATEMENTS_SUMMARY_EVICTED | CLUSTER_TABLE_STATEMENTS_SUMMARY_EVICTED
    )
}

/// 驱逐表查询要求 PROCESS 权限。
pub fn checkPrivilege<R: StatementSummaryRuntime>(
    runtime: &R,
    context: &R::Context,
) -> Result<(), R::Error> {
    if runtime.has_process_privilege(context) {
        Ok(())
    } else {
        Err(runtime.process_privilege_denied())
    }
}

/// 集群表返回本实例地址，本地表返回空串。
pub fn clusterTableInstanceAddr<R: StatementSummaryRuntime>(
    runtime: &R,
    context: &R::Context,
    table: &str,
) -> Result<String, R::Error> {
    if isClusterTable(table) {
        runtime.instance_address(context)
    } else {
        Ok(String::new())
    }
}

/// 将粗粒度时间范围转为单段 [`StmtTimeRange`] 列表。
pub fn buildTimeRanges(range: Option<CoarseTimeRange>) -> Option<Vec<StmtTimeRange>> {
    range.map(|range| {
        vec![StmtTimeRange {
            begin: range.start_unix,
            end: range.end_unix,
        }]
    })
}
