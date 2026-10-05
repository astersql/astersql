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

// 列统计 ANALYZE 下推执行器。
//
// 将列直方图/采样请求下推到 TiKV（DistSQL），处理 common handle 与
// 整型主键范围拆分，并生成可读的分析作业信息。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// 伪列 ID：表示额外附加的 row handle（行句柄）列。
pub const ExtraHandleID: i64 = -1;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列统计 ANALYZE 过程错误。
pub struct AnalyzeColumnError(pub String);

impl fmt::Display for AnalyzeColumnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AnalyzeColumnError {}

/// 列 ANALYZE 操作的 Result 别名。
pub type AnalyzeColumnResult<T = ()> = Result<T, AnalyzeColumnError>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列分析请求上下文（含 requestID）。
pub struct analyzeContext {
    pub requestID: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// DDL  schema 状态：Public / WriteOnly / DeleteOnly 等（在线 DDL 可见性阶段）。
pub enum schemaState {
    Public,
    WriteOnly,
    DeleteOnly,
    None,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引中的一列及其可选前缀长度。
pub struct indexColumn {
    pub offset: usize,
    /// `None` means a full-length index column; `Some` is a prefix length.
    pub prefixLength: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表上索引元信息（唯一性、主键、条件索引等）。
pub struct indexInfo {
    pub name: String,
    pub state: schemaState,
    pub unique: bool,
    pub primary: bool,
    pub columns: Vec<indexColumn>,
    pub condition: Option<String>,
}

impl indexInfo {
    /// 是否包含前缀索引列。
    pub fn hasPrefixIndex(&self) -> bool {
        self.columns
            .iter()
            .any(|column| column.prefixLength.is_some())
    }

    /// 是否为带条件的部分索引。
    pub fn hasCondition(&self) -> bool {
        self.condition.is_some()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 列元信息：ID、名称及变更/删除/临时标记。
pub struct columnInfo {
    pub id: i64,
    pub name: String,
    pub changing: bool,
    pub removing: bool,
    pub temporary: bool,
}

impl columnInfo {
    /// 列是否处于变更中。
    pub fn isChanging(&self) -> bool {
        self.changing
    }

    /// 列是否正在删除。
    pub fn isRemoving(&self) -> bool {
        self.removing
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表的索引与列集合快照。
pub struct tableInfo {
    pub indices: Vec<indexInfo>,
    pub columns: Vec<columnInfo>,
}

impl tableInfo {
    /// 非临时列数量。
    pub fn nonTemporaryColumnCount(&self) -> usize {
        let mut columns = self
            .columns
            .iter()
            .filter(|column| !column.temporary && !column.isRemoving())
            .map(|column| column.name.to_lowercase())
            .collect::<BTreeSet<_>>();
        for column in self
            .columns
            .iter()
            .filter(|column| !column.temporary && !column.isRemoving() && column.isChanging())
        {
            let changing_name = column.name.strip_prefix("_Col$_").unwrap_or(&column.name);
            let origin_name = changing_name
                .rsplit_once('_')
                .map_or(changing_name, |(origin, _)| origin);
            columns.remove(&origin_name.to_lowercase());
        }
        columns.len()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 行句柄（handle）列类型：整型主键或 common handle。
pub enum handleCols {
    Int,
    Common,
}

impl handleCols {
    /// 是否为整型 handle。
    pub fn isInt(self) -> bool {
        matches!(self, Self::Int)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TiKV 键范围 `[low, high)`，用于扫描切分。
pub struct keyRange {
    pub low: Vec<u8>,
    pub high: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
/// ANALYZE 选项：桶数、TopN、样本数、采样率。
pub enum analyzeOptionType {
    NumBuckets,
    NumTopN,
    NumSamples,
    SampleRate,
}

impl analyzeOptionType {
    /// 选项在作业文案中的英文标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::NumBuckets => "buckets",
            Self::NumTopN => "topn",
            Self::NumSamples => "samples",
            Self::SampleRate => "samplerate",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 下推到存储层的列采样/直方图收集请求参数。
pub struct columnAnalyzeRequest {
    pub sampleRate: f64,
    pub sampleSize: u64,
    pub bucketCount: u64,
    pub topNSize: u64,
    pub cmsDepth: u32,
    pub cmsWidth: u32,
    pub fmSketchSize: u32,
    pub collectExtendedStats: bool,
    pub collectNdv: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 完整分析请求：列请求 + 未解析的 opaque protobuf 字段。
pub struct analyzeRequest {
    pub columnRequest: columnAnalyzeRequest,
    /// Serialized protobuf fields not understood by this module are retained.
    pub opaqueFields: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// v2 已填充选项映射。
pub struct v2AnalyzeOptions {
    pub filledOptions: BTreeMap<analyzeOptionType, u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 附着在执行器上的分析元信息。
pub struct analyzeInfo {
    pub v2Options: Option<v2AnalyzeOptions>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 作业展示信息字符串容器。
pub struct analyzeJob {
    pub jobInfo: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 虚拟生成列求值所需的列 ID 列表。
pub struct virtualColumnSchema {
    pub columnIDs: Vec<i64>,
}

#[derive(Clone, Debug, Default)]
/// 带错误通知的 WaitGroup 计数包装（此处仅暴露 worker 计数）。
pub struct notifyErrorWaitGroupWrapper {
    pub workers: Arc<AtomicUsize>,
}

#[derive(Clone, Debug, Default)]
/// 简单 WaitGroup 计数包装。
pub struct waitGroupWrapper {
    pub workers: Arc<AtomicUsize>,
}

/// 语句级内存追踪：挂接与分离。
pub trait memoryTracker: Send + Sync {
    fn attach_to_statement(&self) -> AnalyzeColumnResult;
    fn detach(&self);
}

/// DistSQL/下推查询结果流，可关闭。
pub trait selectResult: Send {
    fn close(&mut self) -> AnalyzeColumnResult;
}

/// 持有可能拆成两段 key 范围的双结果流。
pub struct tableResultHandler {
    pub firstResult: Option<Box<dyn selectResult>>,
    pub secondResult: Option<Box<dyn selectResult>>,
}

impl tableResultHandler {
    /// 构造空结果处理器。
    pub fn new() -> Self {
        Self {
            firstResult: None,
            secondResult: None,
        }
    }

    /// 设置第一段（可选）与第二段结果流。
    pub fn set_results(
        &mut self,
        first: Option<Box<dyn selectResult>>,
        second: Box<dyn selectResult>,
    ) {
        self.firstResult = first;
        self.secondResult = Some(second);
    }
}

impl Default for tableResultHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 读隔离级别：RC 或快照隔离（对应 MVCC 可见版本选择）。
pub enum isolationLevel {
    ReadCommitted,
    SnapshotIsolation,
}

#[derive(Clone, Debug, PartialEq)]
/// 构建下推 ANALYZE 请求的规格：表 ID、范围、隔离与资源组等。
pub struct analyzeRequestSpec {
    pub physicalTableIDs: Vec<i64>,
    pub commonHandle: bool,
    pub ranges: Vec<keyRange>,
    pub analyzeRequest: analyzeRequest,
    pub isolationLevel: isolationLevel,
    pub startTimestamp: u64,
    pub keepOrder: bool,
    pub concurrency: usize,
    pub storeBatchSize: usize,
    pub allowBatchTaskDataMerge: bool,
    pub executeBatchTasksSerially: bool,
    pub resourceGroupTagger: Vec<u8>,
    pub resourceGroupName: String,
    pub explicitRequestSourceType: String,
    pub distSQLContextID: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 发送 DistSQL 时的传输侧参数。
pub struct analyzeTransportSpec {
    pub clientID: String,
    pub kvVariables: BTreeMap<String, String>,
    pub restrictedSQL: bool,
    pub distSQLContextID: u64,
}

/// 列分析运行时：内存 tracker、范围拆分、构请求与执行。
pub trait analyzeColumnRuntime: Send + Sync {
    fn new_memory_tracker(
        &self,
        plan_id: i64,
        byte_limit: i64,
    ) -> AnalyzeColumnResult<Arc<dyn memoryTracker>>;
    fn split_ranges_across_int64_boundary(
        &self,
        ranges: Vec<keyRange>,
        keep_order: bool,
        descending: bool,
        split_common_handle: bool,
    ) -> (Vec<keyRange>, Vec<keyRange>);
    fn build_request(
        &self,
        spec: analyzeRequestSpec,
        memory_tracker: &dyn memoryTracker,
    ) -> AnalyzeColumnResult<Vec<u8>>;
    fn analyze(
        &self,
        context: &analyzeContext,
        request: Vec<u8>,
        transport: analyzeTransportSpec,
    ) -> AnalyzeColumnResult<Box<dyn selectResult>>;
}

/// 列/索引分析执行器共享的基础字段。
pub struct baseAnalyzeExec {
    pub tableID: i64,
    pub concurrency: usize,
    pub analyzeStoreBatchSize: usize,
    pub analyzeRequest: analyzeRequest,
    pub options: BTreeMap<analyzeOptionType, u64>,
    pub job: Option<analyzeJob>,
    pub snapshot: u64,
    pub planID: i64,
    pub enableAnalyzeSnapshot: bool,
    pub resourceGroupTagger: Vec<u8>,
    pub resourceGroupName: String,
    pub explicitRequestSourceType: String,
    pub restrictedSQL: bool,
    pub clientID: String,
    pub kvVariables: BTreeMap<String, String>,
    pub distSQLContextID: u64,
    pub runtime: Arc<dyn analyzeColumnRuntime>,
}

// AnalyzeColumnsExec represents Analyze columns push down executor.
// AnalyzeColumnsExec 表示列统计下推执行器。
/// 列 ANALYZE 下推执行器：打开范围、构建响应并准备作业文案。
pub struct AnalyzeColumnsExec {
    pub baseAnalyzeExec: baseAnalyzeExec,
    pub tableInfo: tableInfo,
    pub colsInfo: Vec<columnInfo>,
    pub handleCols: Option<handleCols>,
    pub commonHandle: Option<indexInfo>,
    pub resultHandler: Option<tableResultHandler>,
    pub indexes: Vec<indexInfo>,
    pub analyzeInfo: analyzeInfo,
    pub samplingBuilderWg: notifyErrorWaitGroupWrapper,
    pub samplingMergeWg: waitGroupWrapper,
    pub schemaForVirtualColEval: virtualColumnSchema,
    pub baseCount: i64,
    pub baseModifyCnt: i64,
    pub samplingStatsConcurrency: usize,
    pub memTracker: Option<Arc<dyn memoryTracker>>,
}

/// 列是否被单列非前缀唯一索引覆盖（可用于优化采样）。
pub fn isColumnCoveredBySingleColUniqueIndex(table: &tableInfo, columnOffset: usize) -> bool {
    table.indices.iter().any(|index| {
        index.state == schemaState::Public
            && isSingleColNonPrefixUniqueIndex(index)
            && index.columns[0].offset == columnOffset
    })
}

/// 是否为 Public 状态的单列、非前缀、无条件唯一/主键索引。
pub fn isSingleColNonPrefixUniqueIndex(index: &indexInfo) -> bool {
    index.state == schemaState::Public
        && (index.unique || index.primary)
        && index.columns.len() == 1
        && !index.hasPrefixIndex()
        && !index.hasCondition()
}

impl AnalyzeColumnsExec {
    /// 安装内存 tracker，必要时跨 int64 边界拆分范围并拉取结果。
    pub fn open(&mut self, context: &analyzeContext, ranges: Vec<keyRange>) -> AnalyzeColumnResult {
        let tracker = self
            .baseAnalyzeExec
            .runtime
            .new_memory_tracker(self.baseAnalyzeExec.planID, -1)?;
        tracker.attach_to_statement()?;
        self.memTracker = Some(tracker);
        self.resultHandler = Some(tableResultHandler::new());
        let (first_ranges, second_ranges) = self
            .baseAnalyzeExec
            .runtime
            .split_ranges_across_int64_boundary(
                ranges,
                true,
                false,
                !hasPkHist(self.handleCols.as_ref()),
            );
        let first_result = self.buildResp(context, first_ranges)?;
        if second_ranges.is_empty() {
            self.resultHandler
                .as_mut()
                .expect("result handler initialized above")
                .set_results(None, first_result);
            return Ok(());
        }
        let second_result = self.buildResp(context, second_ranges)?;
        self.resultHandler
            .as_mut()
            .expect("result handler initialized above")
            .set_results(Some(first_result), second_result);
        Ok(())
    }

    /// 按快照开关选择隔离级别，构建并发送分析请求。
    pub fn buildResp(
        &self,
        context: &analyzeContext,
        ranges: Vec<keyRange>,
    ) -> AnalyzeColumnResult<Box<dyn selectResult>> {
        let common_handle = self.handleCols.is_some_and(|columns| !columns.isInt());
        let (start_timestamp, isolation_level) = if self.baseAnalyzeExec.enableAnalyzeSnapshot {
            (
                self.baseAnalyzeExec.snapshot,
                isolationLevel::SnapshotIsolation,
            )
        } else {
            (u64::MAX, isolationLevel::ReadCommitted)
        };
        let enable_store_batch = self.baseAnalyzeExec.analyzeStoreBatchSize > 0;
        let spec = analyzeRequestSpec {
            physicalTableIDs: vec![self.baseAnalyzeExec.tableID],
            commonHandle: common_handle,
            ranges,
            analyzeRequest: self.baseAnalyzeExec.analyzeRequest.clone(),
            isolationLevel: isolation_level,
            startTimestamp: start_timestamp,
            // Full-sampling Analyze restores handle order after collecting samples.
            keepOrder: false,
            concurrency: self.baseAnalyzeExec.concurrency,
            storeBatchSize: self.baseAnalyzeExec.analyzeStoreBatchSize,
            allowBatchTaskDataMerge: enable_store_batch,
            executeBatchTasksSerially: enable_store_batch,
            resourceGroupTagger: self.baseAnalyzeExec.resourceGroupTagger.clone(),
            resourceGroupName: self.baseAnalyzeExec.resourceGroupName.clone(),
            explicitRequestSourceType: self.baseAnalyzeExec.explicitRequestSourceType.clone(),
            distSQLContextID: self.baseAnalyzeExec.distSQLContextID,
        };
        let request = self.baseAnalyzeExec.runtime.build_request(
            spec,
            self.memTracker
                .as_deref()
                .expect("open installs the memory tracker before building a response"),
        )?;
        self.baseAnalyzeExec.runtime.analyze(
            context,
            request,
            analyzeTransportSpec {
                clientID: self.baseAnalyzeExec.clientID.clone(),
                kvVariables: self.baseAnalyzeExec.kvVariables.clone(),
                restrictedSQL: self.baseAnalyzeExec.restrictedSQL,
                distSQLContextID: self.baseAnalyzeExec.distSQLContextID,
            },
        )
    }
}

/// 整型主键 handle 存在时可单独收集 PK 直方图。
pub fn hasPkHist(handleColumns: Option<&handleCols>) -> bool {
    handleColumns.is_some_and(|columns| columns.isInt())
}

/// 向作业文案追加列列表或 “all columns”。
pub fn prepareColumns(executor: &AnalyzeColumnsExec, builder: &mut String) {
    let mut columns = executor.colsInfo.as_slice();
    if columns
        .last()
        .is_some_and(|column| column.id == ExtraHandleID)
    {
        columns = &columns[..columns.len() - 1];
    }
    if columns.is_empty() {
        return;
    }
    let filtered = columns
        .iter()
        .filter(|column| !column.isChanging() && !column.isRemoving())
        .collect::<Vec<_>>();
    if filtered.len() < executor.tableInfo.nonTemporaryColumnCount() {
        builder.push_str(if columns.len() > 1 {
            " columns "
        } else {
            " column "
        });
        for (index, column) in filtered.iter().enumerate() {
            if index > 0 {
                builder.push_str(", ");
            }
            builder.push_str(&column.name);
        }
    } else {
        builder.push_str(" all columns");
    }
}

/// 向作业文案追加索引列表或 “all indexes”。
pub fn prepareIndexes(executor: &AnalyzeColumnsExec, builder: &mut String) {
    if executor.indexes.is_empty() {
        return;
    }
    if executor.indexes.len() < executor.tableInfo.indices.len() {
        builder.push_str(if executor.indexes.len() > 1 {
            " indexes "
        } else {
            " index "
        });
        for (position, index) in executor.indexes.iter().enumerate() {
            if position > 0 {
                builder.push_str(", ");
            }
            builder.push_str(&index.name);
        }
    } else {
        builder.push_str(" all indexes");
    }
}

/// 组装 “analyze table … with …” 作业描述字符串。
pub fn prepareAnalyzeColumnsJobInfo(executor: Option<&mut AnalyzeColumnsExec>) {
    let Some(executor) = executor else {
        return;
    };
    let options = executor
        .analyzeInfo
        .v2Options
        .as_ref()
        .map_or(&executor.baseAnalyzeExec.options, |options| {
            &options.filledOptions
        });
    let sample_rate = executor
        .baseAnalyzeExec
        .analyzeRequest
        .columnRequest
        .sampleRate;
    let mut builder = String::new();
    if executor.baseAnalyzeExec.restrictedSQL {
        builder.push_str("auto ");
    }
    builder.push_str("analyze table");
    prepareIndexes(executor, &mut builder);
    if !executor.indexes.is_empty() && !executor.colsInfo.is_empty() {
        builder.push(',');
    }
    prepareColumns(executor, &mut builder);
    builder.push_str(" with ");
    let mut rendered = Vec::new();
    for option_type in [analyzeOptionType::NumBuckets, analyzeOptionType::NumTopN] {
        if let Some(value) = options.get(&option_type) {
            rendered.push(format!("{value} {}", option_type.label()));
        }
    }
    if options
        .get(&analyzeOptionType::NumSamples)
        .copied()
        .unwrap_or_default()
        != 0
    {
        if let Some(value) = options.get(&analyzeOptionType::NumSamples) {
            rendered.push(format!("{value} {}", analyzeOptionType::NumSamples.label()));
        }
    } else {
        rendered.push(format!("{sample_rate} samplerate"));
    }
    builder.push_str(&rendered.join(", "));
    if let Some(job) = executor.baseAnalyzeExec.job.as_mut() {
        job.jobInfo = builder;
    }
}
