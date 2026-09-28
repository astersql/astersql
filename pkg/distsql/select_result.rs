// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DistSQL SelectResult：协处理器（coprocessor）部分结果的迭代与聚合。
//
// DistSQL 将查询下推到 TiKV/TiFlash 的 coprocessor；本模块提供：
// - `SelectResult` / `SelectResultIter`：按行或原始字节拉取 partial result；
// - `serialSelectResults`：串行消费多个子结果；
// - `sortedSelectResults`：按 `ByItem` 对多路结果做归并有序输出；
// - `selectResultRuntimeStats`：汇总 cop 耗时、缓存命中与批处理计数。
//
// 文件前半为迁移历史草稿（块注释），后半为当前可解析的精简实现。

// coprocessor partial result 的 SelectResult 接口、排序/串行聚合读取、chunk 解码、
// intermediate outputs 迭代和 runtime stats 汇总；不会拉取真实 TiKV/TiFlash 响应或上报指标。

/* Mechanical draft retained for migration history.
use std::collections::HashMap;

// telemetry* 变量对应 Go 包级别指标别名，便于测试替换。
pub static telemetryBatchedQueryTaskCnt: metrics::Counter = metrics::TelemetryBatchedQueryTaskCnt;
pub static telemetryStoreBatchedCnt: metrics::Counter = metrics::TelemetryStoreBatchedCnt;
pub static telemetryStoreBatchedFallbackCnt: metrics::Counter = metrics::TelemetryStoreBatchedFallbackCnt;

// SelectResult is an iterator of coprocessor partial results.
pub trait SelectResult {
    fn NextRaw(&mut self, ctx: context::Context) -> Result<Vec<u8>, errors::Error>;
    fn Next(&mut self, ctx: context::Context, chk: *mut chunk::Chunk) -> Result<(), errors::Error>;
    fn IntoIter(
        &mut self,
        intermediateFieldTypes: Vec<Vec<*mut types::FieldType>>,
    ) -> Result<Box<dyn SelectResultIter>, errors::Error>;
    fn Close(&mut self) -> Result<(), errors::Error>;
}

// GetSelectResultConcurrency returns internal cop iterator concurrency if the implementation exposes it.
pub fn GetSelectResultConcurrency(sr: &dyn SelectResult) -> (i32, i32, bool) {
    let Some(r) = sr.downcast_ref::<selectResult>() else {
        return (0, 0, false);
    };
    let Some(ci) = r.resp.as_cop_info() else {
        return (0, 0, false);
    };
    let (concurrency, extraConcurrency) = ci.GetConcurrency();
    (concurrency, extraConcurrency, true)
}

// SelectResultRow indicates the row returned by SelectResultIter.
/// SelectResultIter 返回的一行及其所属 channel。
pub struct SelectResultRow {
    // ChannelIndex 表示该行所在 channel：小于 intermediate 数量表示中间结果，等于数量表示最终结果。
    pub ChannelIndex: i32,
    // Row is the actual data of this row.
    pub Row: chunk::Row,
}

// SelectResultIter iterates rows from SelectResult.
pub trait SelectResultIter {
    fn Next(&mut self, ctx: context::Context) -> Result<SelectResultRow, errors::Error>;
    fn Close(&mut self) -> Result<(), errors::Error>;
}

// chunkRowHeap 对应 Go heap.Interface 的包装，持有 sortedSelectResults。
pub struct chunkRowHeap {
    pub sortedSelectResults: *mut sortedSelectResults,
}

impl chunkRowHeap {
    pub fn Len(&self) -> usize {
        unsafe { (*self.sortedSelectResults).rowPtrs.len() }
    }

    pub fn Less(&self, i: usize, j: usize) -> bool {
        let ssr = unsafe { &mut *self.sortedSelectResults };
        let iPtr = ssr.rowPtrs[i];
        let jPtr = ssr.rowPtrs[j];
        ssr.lessRow(
            ssr.cachedChunks[iPtr.ChkIdx as usize].GetRow(iPtr.RowIdx as i32),
            ssr.cachedChunks[jPtr.ChkIdx as usize].GetRow(jPtr.RowIdx as i32),
        )
    }

    pub fn Swap(&mut self, i: usize, j: usize) {
        unsafe { (*self.sortedSelectResults).rowPtrs.swap(i, j) };
    }

    pub fn Push(&mut self, x: chunk::RowPtr) {
        unsafe { (*self.sortedSelectResults).rowPtrs.push(x) };
    }

    pub fn Pop(&mut self) -> chunk::RowPtr {
        unsafe { (*self.sortedSelectResults).rowPtrs.pop().unwrap() }
    }
}

// NewSortedSelectResults is only for partition table.
// If schema == nil, sort by first few columns.
/// 将多个 SelectResult 转为迭代器后构造有序归并结果（常用于分区表）。
pub fn NewSortedSelectResults(
    ectx: expression::EvalContext,
    selectResult: Vec<Box<dyn SelectResult>>,
    schema: Option<*mut expression::Schema>,
    byitems: Vec<*mut planner_util::ByItems>,
    memTracker: *mut memory::Tracker,
) -> Box<dyn SelectResult> {
    let mut s = Box::new(sortedSelectResults {
        schema,
        selectResult,
        byItems: byitems,
        memTracker,
        ..Default::default()
    });
    s.initCompareFuncs(ectx);
    s.buildKeyColumns();
    let raw = &mut *s as *mut sortedSelectResults;
    s.heap = Some(chunkRowHeap { sortedSelectResults: raw });
    s.cachedChunks = (0..s.selectResult.len()).map(|_| None).collect();
    s
}

// sortedSelectResults 将多个 SelectResult 按 byItems 做归并排序。
#[derive(Default)]
/// 多路归并排序的 SelectResult：各输入保持有序，按 ByItem 选出当前最小行。
pub struct sortedSelectResults {
    pub schema: Option<*mut expression::Schema>,
    pub selectResult: Vec<Box<dyn SelectResult>>,
    pub compareFuncs: Vec<chunk::CompareFunc>,
    pub byItems: Vec<*mut planner_util::ByItems>,
    pub keyColumns: Vec<i32>,

    pub cachedChunks: Vec<Option<*mut chunk::Chunk>>,
    pub rowPtrs: Vec<chunk::RowPtr>,
    pub heap: Option<chunkRowHeap>,

    pub memTracker: *mut memory::Tracker,
}

impl sortedSelectResults {
    // updateCachedChunk 从指定子结果读下一批 rows，并把第一行指针压入 heap。
    fn updateCachedChunk(&mut self, ctx: context::Context, idx: u32) -> Result<(), errors::Error> {
        let chk = self.cachedChunks[idx as usize].unwrap();
        let prevMemUsage = unsafe { (*chk).MemoryUsage() };
        self.selectResult[idx as usize].Next(ctx, chk)?;
        unsafe { (*self.memTracker).Consume((*chk).MemoryUsage() - prevMemUsage) };
        if unsafe { (*chk).NumRows() } == 0 {
            return Ok(());
        }
        self.heap.as_mut().unwrap().Push(chunk::RowPtr { ChkIdx: idx, RowIdx: 0 });
        Ok(())
    }

    // initCompareFuncs 根据 byItems 的表达式类型初始化比较函数。
    fn initCompareFuncs(&mut self, ectx: expression::EvalContext) {
        self.compareFuncs = Vec::with_capacity(self.byItems.len());
        for item in &self.byItems {
            let keyType = unsafe { (**item).Expr.GetType(ectx.clone()) };
            self.compareFuncs.push(chunk::GetCompareFunc(keyType));
        }
    }

    // buildKeyColumns 计算排序 key 在输出 schema 中的列位置。
    fn buildKeyColumns(&mut self) {
        self.keyColumns = Vec::with_capacity(self.byItems.len());
        for (i, by) in self.byItems.iter().enumerate() {
            let col = unsafe { (**by).Expr.downcast_ref::<expression::Column>().unwrap() };
            if self.schema.is_none() {
                self.keyColumns.push(i as i32);
            } else {
                self.keyColumns
                    .push(unsafe { (*self.schema.unwrap()).ColumnIndex(col) });
            }
        }
    }

    // lessRow 按每个 key column 比较两行，Desc 时反转比较结果。
    fn lessRow(&self, rowI: chunk::Row, rowJ: chunk::Row) -> bool {
        for (i, colIdx) in self.keyColumns.iter().enumerate() {
            let cmpFunc = self.compareFuncs[i];
            let mut cmp = cmpFunc(rowI, *colIdx, rowJ, *colIdx);
            if unsafe { (*self.byItems[i]).Desc } {
                cmp = -cmp;
            }
            if cmp < 0 {
                return true;
            } else if cmp > 0 {
                return false;
            }
        }
        false
    }
}

impl SelectResult for sortedSelectResults {
    fn NextRaw(&mut self, _ctx: context::Context) -> Result<Vec<u8>, errors::Error> {
        panic!("Not support NextRaw for sortedSelectResults");
    }

    fn Next(&mut self, ctx: context::Context, c: *mut chunk::Chunk) -> Result<(), errors::Error> {
        unsafe { (*c).Reset() };
        for i in 0..self.cachedChunks.len() {
            if self.cachedChunks[i].is_none() {
                let copy = unsafe { (*c).CopyConstruct() };
                unsafe { (*self.memTracker).Consume((*copy).MemoryUsage()) };
                self.cachedChunks[i] = Some(copy);
            }
        }

        if self.heap.as_ref().unwrap().Len() == 0 {
            for i in 0..self.cachedChunks.len() {
                self.updateCachedChunk(ctx.clone(), i as u32)?;
            }
        }

        while unsafe { (*c).NumRows() < (*c).RequiredRows() } {
            if self.heap.as_ref().unwrap().Len() == 0 {
                break;
            }
            let idx = self.heap.as_mut().unwrap().Pop();
            unsafe {
                (*c).AppendRow((*self.cachedChunks[idx.ChkIdx as usize].unwrap()).GetRow(idx.RowIdx as i32));
            }
            if unsafe { idx.RowIdx as i32 >= (*self.cachedChunks[idx.ChkIdx as usize].unwrap()).NumRows() - 1 } {
                self.updateCachedChunk(ctx.clone(), idx.ChkIdx)?;
            } else {
                self.heap.as_mut().unwrap().Push(chunk::RowPtr {
                    ChkIdx: idx.ChkIdx,
                    RowIdx: idx.RowIdx + 1,
                });
            }
        }
        Ok(())
    }

    fn IntoIter(&mut self, _intermediate: Vec<Vec<*mut types::FieldType>>) -> Result<Box<dyn SelectResultIter>, errors::Error> {
        Err(errors::New("not implemented"))
    }

    fn Close(&mut self) -> Result<(), errors::Error> {
        for (i, sr) in self.selectResult.iter_mut().enumerate() {
            sr.Close()?;
            let chk = self.cachedChunks[i].unwrap();
            unsafe {
                (*self.memTracker).Consume(-(*chk).MemoryUsage());
            }
            self.cachedChunks[i] = None;
        }
        Ok(())
    }
}

// NewSerialSelectResults creates a SelectResult which reads each SelectResult serially.
pub fn NewSerialSelectResults(selectResults: Vec<Box<dyn SelectResult>>) -> Box<dyn SelectResult> {
    Box::new(serialSelectResults { selectResults, cur: 0 })
}

// serialSelectResults reads each SelectResult serially.
/// 串行拼接多个 SelectResult：前一个耗尽后再读下一个。
pub struct serialSelectResults {
    pub selectResults: Vec<Box<dyn SelectResult>>,
    pub cur: usize,
}

impl SelectResult for serialSelectResults {
    // 当前子结果耗尽后推进到下一个 SelectResult。
    fn NextRaw(&mut self, ctx: context::Context) -> Result<Vec<u8>, errors::Error> {
        while self.cur < self.selectResults.len() {
            let resultSubset = self.selectResults[self.cur].NextRaw(ctx.clone())?;
            if !resultSubset.is_empty() {
                return Ok(resultSubset);
            }
            self.cur += 1;
        }
        Ok(Vec::new())
    }

    fn Next(&mut self, ctx: context::Context, chk: *mut chunk::Chunk) -> Result<(), errors::Error> {
        while self.cur < self.selectResults.len() {
            self.selectResults[self.cur].Next(ctx.clone(), chk)?;
            if unsafe { (*chk).NumRows() } > 0 {
                return Ok(());
            }
            self.cur += 1;
        }
        Ok(())
    }

    fn IntoIter(&mut self, _intermediate: Vec<Vec<*mut types::FieldType>>) -> Result<Box<dyn SelectResultIter>, errors::Error> {
        Err(errors::New("not implemented"))
    }

    fn Close(&mut self) -> Result<(), errors::Error> {
        let mut err: Option<errors::Error> = None;
        for r in &mut self.selectResults {
            if let Err(rerr) = r.Close() {
                err = Some(rerr);
            }
        }
        if let Some(e) = err { Err(e) } else { Ok(()) }
    }
}

// selectResult 是普通 DistSQL response 的核心迭代器状态。
#[derive(Default)]
pub struct selectResult {
    pub label: String,
    pub resp: kv::Response,

    pub rowLen: i32,
    pub fieldTypes: Vec<*mut types::FieldType>,
    pub intermediateOutputTypes: Vec<Vec<*mut types::FieldType>>,
    pub ctx: *mut dcontext::DistSQLContext,

    pub selectResp: Option<tipb::SelectResponse>,
    pub selectRespSize: i64,
    pub respChkIdx: usize,
    pub respChunkDecoder: Option<chunk::Decoder>,

    pub partialCount: i64,
    pub sqlType: String,

    pub copPlanIDs: Vec<i32>,
    pub rootPlanID: i32,

    pub storeType: kv::StoreType,

    pub fetchDuration: time::Duration,
    pub durationReported: bool,
    pub memTracker: *mut memory::Tracker,

    pub stats: Option<selectResultRuntimeStats>,
    pub distSQLConcurrency: i32,
    pub paging: bool,

    pub iter: Option<selectResultIter>,
}

impl selectResult {
    fn fetchResp(&mut self, ctx: context::Context) -> Result<(), errors::Error> {
        self.fetchRespWithIntermediateResults(ctx, Vec::new())
    }

    // fetchRespWithIntermediateResults 从 kv.Response 拉取下一个 partial result，并解码 tipb.SelectResponse。
    fn fetchRespWithIntermediateResults(
        &mut self,
        ctx: context::Context,
        intermediateOutputTypes: Vec<Vec<*mut types::FieldType>>,
    ) -> Result<(), errors::Error> {
        let defer_stats = true;
        for _ in 0.. {
            self.respChkIdx = 0;
            let startTime = time::Now();
            let (resultSubset, err) = self.resp.Next(ctx.clone());
            let duration = time::Since(startTime);
            self.fetchDuration += duration;
            if let Some(err) = err {
                // 错误路径仍合并 CopExecDetails，保证 tidb_keys_examined 与慢日志可见。
                if let Some(subset) = resultSubset {
                    if let Some(copStats) = subset.as_cop_runtime_stats().and_then(|s| s.GetCopRuntimeStats()) {
                        unsafe { (*self.ctx).ExecDetails.MergeCopExecDetails(&copStats.CopExecDetails, duration) };
                    }
                }
                return Err(errors::Trace(err));
            }
            if self.selectResp.is_some() {
                self.memConsume(-atomic::LoadInt64(&self.selectRespSize));
            }
            if resultSubset.is_none() {
                self.selectResp = None;
                atomic::StoreInt64(&mut self.selectRespSize, 0);
                if !self.durationReported {
                    // 最后一轮 fetch 上报总耗时；paging/common 标签沿用 Go 逻辑。
                    let mode = if self.paging { "paging" } else { "common" };
                    metrics::DistSQLQueryHistogram
                        .WithLabelValues(self.label.clone(), self.sqlType.clone(), mode)
                        .Observe(self.fetchDuration.Seconds());
                    self.durationReported = true;
                }
                return Ok(());
            }

            let mut select_resp = tipb::SelectResponse::default();
            select_resp.Unmarshal(resultSubset.as_ref().unwrap().GetData())?;
            let respSize = select_resp.Size() as i64;
            atomic::StoreInt64(&mut self.selectRespSize, respSize);
            self.memConsume(respSize);
            if let Some(err) = select_resp.Error.clone() {
                return Err(dbterror::ClassTiKV.Synthesize(terror::ErrCode(err.Code), err.Msg));
            }

            if select_resp.IntermediateOutputs.len() != intermediateOutputTypes.len() {
                return Err(errors::Errorf(format!(
                    "The length of intermediate output types {} mismatches the length of got intermediate outputs {}. If a response contains intermediate outputs, you should use the SelectResultIter to read the data.",
                    intermediateOutputTypes.len(),
                    select_resp.IntermediateOutputs.len(),
                )));
            }
            self.intermediateOutputTypes = intermediateOutputTypes.clone();

            unsafe { (*self.ctx).SQLKiller.HandleSignal()? };
            for warning in &select_resp.Warnings {
                unsafe {
                    (*self.ctx).AppendWarning(dbterror::ClassTiKV.Synthesize(
                        terror::ErrCode(warning.Code),
                        warning.Msg.clone(),
                    ));
                }
            }

            self.partialCount += 1;
            if let Some(copStats) = resultSubset.as_ref().unwrap().as_cop_runtime_stats().and_then(|s| s.GetCopRuntimeStats()) {
                self.selectResp = Some(select_resp);
                self.updateCopRuntimeStats(ctx.clone(), copStats, resultSubset.as_ref().unwrap().RespTime(), false)?;
                unsafe { (*self.ctx).ExecDetails.MergeCopExecDetails(&copStats.CopExecDetails, duration) };
            } else {
                self.selectResp = Some(select_resp);
            }

            if !self.selectResp.as_ref().unwrap().Chunks.is_empty() {
                break;
            }
            let intermediate = &self.selectResp.as_ref().unwrap().IntermediateOutputs;
            if intermediate.iter().any(|output| !output.Chunks.is_empty()) {
                return Ok(());
            }
        }

        if defer_stats {
            // Go 的 defer 在函数返回前更新 copr cache hit telemetry；这里保留语义位置。
            self.report_cache_hit_telemetry();
        }
        Ok(())
    }

    fn report_cache_hit_telemetry(&self) {
        if let Some(stats) = &self.stats {
            if unsafe { !(*self.ctx).InRestrictedSQL } && stats.copRespTime.Size() > 0 {
                let ratio = stats.calcCacheHit();
                if ratio >= 1.0 { telemetry::CurrentCoprCacheHitRatioGTE100Count.Inc(); }
                if ratio >= 0.8 { telemetry::CurrentCoprCacheHitRatioGTE80Count.Inc(); }
                if ratio >= 0.4 { telemetry::CurrentCoprCacheHitRatioGTE40Count.Inc(); }
                if ratio >= 0.2 { telemetry::CurrentCoprCacheHitRatioGTE20Count.Inc(); }
                if ratio >= 0.1 { telemetry::CurrentCoprCacheHitRatioGTE10Count.Inc(); }
                if ratio >= 0.01 { telemetry::CurrentCoprCacheHitRatioGTE1Count.Inc(); }
                if ratio >= 0.0 { telemetry::CurrentCoprCacheHitRatioGTE0Count.Inc(); }
            }
        }
    }

    // readFromDefault 按 rowLen 逐列 DecodeOne，直到 chunk 满或当前响应耗尽。
    fn readFromDefault(&mut self, ctx: context::Context, chk: *mut chunk::Chunk) -> Result<(), errors::Error> {
        while unsafe { !(*chk).IsFull() } {
            if self.respChkIdx == self.selectResp.as_ref().unwrap().Chunks.len() {
                self.fetchResp(ctx.clone())?;
                if self.selectResp.is_none() {
                    return Ok(());
                }
            }
            self.readRowsData(chk)?;
            if self.selectResp.as_ref().unwrap().Chunks[self.respChkIdx].RowsData.is_empty() {
                self.respChkIdx += 1;
            }
        }
        Ok(())
    }

    // readFromChunk 解码 chunk encoding；大 chunk 可直接复用响应内存提前返回。
    fn readFromChunk(&mut self, ctx: context::Context, chk: *mut chunk::Chunk) -> Result<(), errors::Error> {
        if self.respChunkDecoder.is_none() {
            self.respChunkDecoder = Some(chunk::NewDecoder(
                chunk::NewChunkWithCapacity(self.fieldTypes.clone(), 0),
                self.fieldTypes.clone(),
            ));
        }

        while unsafe { !(*chk).IsFull() } {
            if self.respChkIdx == self.selectResp.as_ref().unwrap().Chunks.len() {
                self.fetchResp(ctx.clone())?;
                if self.selectResp.is_none() {
                    return Ok(());
                }
            }
            let decoder = self.respChunkDecoder.as_mut().unwrap();
            if decoder.IsFinished() {
                decoder.Reset(self.selectResp.as_ref().unwrap().Chunks[self.respChkIdx].RowsData.clone());
            }
            if decoder.RemainedRows() > (unsafe { (*chk).RequiredRows() } as f64 * 0.8) as i32 {
                if unsafe { (*chk).NumRows() } > 0 {
                    return Ok(());
                }
                decoder.ReuseIntermChk(chk);
                self.respChkIdx += 1;
                return Ok(());
            }
            decoder.Decode(chk);
            if decoder.IsFinished() {
                self.respChkIdx += 1;
            }
        }
        Ok(())
    }

    // updateCopRuntimeStats 合并 cop runtime stats、TiFlash RU、CPU 时间和 executor summaries。
    fn updateCopRuntimeStats(
        &mut self,
        ctx: context::Context,
        copStats: *mut copr::CopRuntimeStats,
        respTime: time::Duration,
        forUnconsumedStats: bool,
    ) -> Result<(), errors::Error> {
        let callee = unsafe { (*copStats).CalleeAddress.clone() };
        if self.rootPlanID <= 0
            || unsafe { (*self.ctx).RuntimeStatsColl.is_null() }
            || (callee.is_empty()
                && unsafe { (*copStats).ReqStats.is_none() || (*copStats).ReqStats.as_ref().unwrap().GetRPCStatsCount() == 0 })
        {
            return Ok(());
        }
        if unsafe { (*copStats).ScanDetail.is_some() && (*copStats).ScanDetail.as_ref().unwrap().ProcessedKeys > 0 }
            || unsafe { (*copStats).TimeDetail.KvReadWallTime > time::Duration::ZERO }
        {
            let mut readKeys = 0_i64;
            let mut readSize = 0_f64;
            if unsafe { (*copStats).ScanDetail.is_some() } {
                readKeys = unsafe { (*copStats).ScanDetail.as_ref().unwrap().ProcessedKeys };
                readSize = unsafe { (*copStats).ScanDetail.as_ref().unwrap().ProcessedKeysSize as f64 };
            }
            let readTime = unsafe { (*copStats).TimeDetail.KvReadWallTime.Seconds() };
            tikvmetrics::ObserveReadSLI(readKeys as u64, readTime, readSize);
        }

        if self.stats.is_none() {
            let mut stats = selectResultRuntimeStats { distSQLConcurrency: self.distSQLConcurrency, ..Default::default() };
            if let Some(ci) = self.resp.as_cop_info() {
                let (conc, extraConc) = ci.GetConcurrency();
                stats.distSQLConcurrency = conc;
                stats.extraConcurrency = extraConc;
            }
            self.stats = Some(stats);
        }
        self.stats.as_mut().unwrap().mergeCopRuntimeStats(copStats, respTime);

        let hasExecutor = self
            .selectResp
            .as_ref()
            .unwrap()
            .GetExecutionSummaries()
            .iter()
            .any(|detail| detail.has_core_fields() && detail.ExecutorId.is_some());

        if self.storeType == kv::TiFlash {
            let ruv2Metrics = execdetails::RUV2MetricsFromContext(ctx.clone());
            if ruv2Metrics.is_none() || !ruv2Metrics.unwrap().Bypass() {
                if let Some(ruDetailsRaw) = ctx.Value(clientutil::RUDetailsCtxKey) {
                    execdetails::MergeTiFlashRUConsumption(
                        self.selectResp.as_ref().unwrap().GetExecutionSummaries(),
                        ruDetailsRaw.downcast_ref::<clientutil::RUDetails>().unwrap(),
                    )?;
                }
            }
        }
        if unsafe { (*copStats).TimeDetail.ProcessTime > time::Duration::ZERO } {
            unsafe { (*self.ctx).CPUUsage.MergeTikvCPUTime((*copStats).TimeDetail.ProcessTime) };
        }
        if hasExecutor {
            if !self.copPlanIDs.is_empty() {
                unsafe {
                    (*self.ctx).RuntimeStatsColl.RecordCopStats(
                        *self.copPlanIDs.last().unwrap(),
                        self.storeType,
                        (*copStats).ScanDetail,
                        (*copStats).TimeDetail,
                        None,
                    );
                }
            }
            recordExecutionSummariesForTiFlashTasks(
                unsafe { (*self.ctx).RuntimeStatsColl },
                self.selectResp.as_ref().unwrap().GetExecutionSummaries(),
                self.storeType,
                self.copPlanIDs.clone(),
            );
            let interZoneBytes = unsafe {
                (*self.ctx)
                    .RuntimeStatsColl
                    .GetStmtCopRuntimeStats()
                    .TiflashNetworkStats
                    .GetInterZoneTrafficBytes()
            };
            if interZoneBytes > 0 && unsafe { !(*self.ctx).RUConsumptionReporter.is_null() } {
                let consumption = rmpb::Consumption { ReadCrossAzTrafficBytes: interZoneBytes };
                unsafe { (*self.ctx).RUConsumptionReporter.ReportConsumption((*self.ctx).ResourceGroupName.clone(), consumption) };
            }
        } else {
            // cop task 场景需要 summaries 数量与 copPlanIDs 对齐；TiFlash streaming 空 summaries 属于设计允许。
            let summaries = self.selectResp.as_ref().unwrap().GetExecutionSummaries();
            if summaries.len() != self.copPlanIDs.len() {
                if !forUnconsumedStats && !(self.storeType == kv::TiFlash && summaries.is_empty()) {
                    logutil::Logger(ctx).Warn(
                        "invalid cop task execution summaries length",
                        zap::Int("expected", self.copPlanIDs.len() as i32),
                        zap::Int("received", summaries.len() as i32),
                    );
                }
                return Ok(());
            }
            for (i, detail) in summaries.iter().enumerate() {
                let summary = if detail.has_core_fields() { Some(detail) } else { None };
                let planID = self.copPlanIDs[i];
                if i == self.copPlanIDs.len() - 1 {
                    unsafe {
                        (*self.ctx).RuntimeStatsColl.RecordCopStats(
                            planID,
                            self.storeType,
                            (*copStats).ScanDetail,
                            (*copStats).TimeDetail,
                            summary,
                        );
                    }
                } else if summary.is_some() {
                    unsafe { (*self.ctx).RuntimeStatsColl.RecordOneCopTask(planID, self.storeType, summary.unwrap()) };
                }
            }
        }
        Ok(())
    }

    // readRowsData 使用 codec.Decoder 逐列填充 chunk，并把未消费的 RowsData 写回响应。
    fn readRowsData(&mut self, chk: *mut chunk::Chunk) -> Result<(), errors::Error> {
        let mut rowsData = self.selectResp.as_ref().unwrap().Chunks[self.respChkIdx].RowsData.clone();
        let mut decoder = codec::NewDecoder(chk, unsafe { (*self.ctx).Location });
        while unsafe { !(*chk).IsFull() } && !rowsData.is_empty() {
            for i in 0..self.rowLen {
                rowsData = decoder.DecodeOne(rowsData, i, self.fieldTypes[i as usize])?;
            }
        }
        self.selectResp.as_mut().unwrap().Chunks[self.respChkIdx].RowsData = rowsData;
        Ok(())
    }

    fn memConsume(&mut self, bytes: i64) {
        if !self.memTracker.is_null() {
            unsafe { (*self.memTracker).Consume(bytes) };
        }
    }

    // close 释放响应内存、收集未消费 runtime stats、注册 selectResultRuntimeStats，并关闭 kv.Response。
    fn close(&mut self) -> Result<(), errors::Error> {
        metrics::DistSQLPartialCountHistogram.Observe(self.partialCount as f64);
        let respSize = atomic::SwapInt64(&mut self.selectRespSize, 0);
        if respSize > 0 {
            self.memConsume(-respSize);
        }
        if !self.ctx.is_null() {
            if let Some(unconsumed) = self.resp.as_has_unconsumed_cop_runtime_stats() {
                for copStats in unconsumed.CollectUnconsumedCopRuntimeStats() {
                    let _ = self.updateCopRuntimeStats(context::Background(), copStats, time::Duration::ZERO, true);
                    unsafe { (*self.ctx).ExecDetails.MergeCopExecDetails(&(*copStats).CopExecDetails, time::Duration::ZERO) };
                }
            }
        }
        if self.stats.is_some() && !self.ctx.is_null() {
            if let Some(ci) = self.resp.as_cop_info() {
                let stats = self.stats.as_mut().unwrap();
                stats.buildTaskDuration = ci.GetBuildTaskElapsed();
                let (batched, fallback) = ci.GetStoreBatchInfo();
                if batched != 0 || fallback != 0 {
                    stats.storeBatchedNum = batched;
                    stats.storeBatchedFallbackNum = fallback;
                    telemetryStoreBatchedCnt.Add(stats.storeBatchedNum as f64);
                    telemetryStoreBatchedFallbackCnt.Add(stats.storeBatchedFallbackNum as f64);
                    telemetryBatchedQueryTaskCnt.Add(stats.copRespTime.Size() as f64);
                }
            }
            self.stats.as_mut().unwrap().fetchRspDuration = self.fetchDuration;
            unsafe { (*self.ctx).RuntimeStatsColl.RegisterStats(self.rootPlanID, self.stats.as_ref().unwrap()) };
        }
        self.resp.Close()
    }
}

impl SelectResult for selectResult {
    fn Next(&mut self, ctx: context::Context, chk: *mut chunk::Chunk) -> Result<(), errors::Error> {
        if self.iter.is_some() {
            return Err(errors::New("selectResult is invalid after IntoIter()"));
        }
        unsafe { (*chk).Reset() };
        if self.selectResp.is_none() || self.respChkIdx == self.selectResp.as_ref().unwrap().Chunks.len() {
            self.fetchResp(ctx.clone())?;
            if self.selectResp.is_none() {
                return Ok(());
            }
            failpoint::Inject("mockConsumeSelectRespSlow", |val| {
                time::Sleep(time::Duration::from_millis(val.as_i32() as u64));
            });
        }
        match self.selectResp.as_ref().unwrap().GetEncodeType() {
            tipb::EncodeType_TypeDefault => self.readFromDefault(ctx, chk),
            tipb::EncodeType_TypeChunk => self.readFromChunk(ctx, chk),
            encodeType => Err(errors::Errorf(format!("unsupported encode type:{:?}", encodeType))),
        }
    }

    fn IntoIter(
        &mut self,
        intermediateFieldTypes: Vec<Vec<*mut types::FieldType>>,
    ) -> Result<Box<dyn SelectResultIter>, errors::Error> {
        if self.iter.is_some() {
            return Err(errors::New("selectResult is invalid after IntoIter()"));
        }
        Ok(Box::new(newSelectResultIter(self, intermediateFieldTypes)))
    }

    // NextRaw returns the next raw partial result.
    fn NextRaw(&mut self, ctx: context::Context) -> Result<Vec<u8>, errors::Error> {
        failpoint::Inject("mockNextRawError", |val| {
            if val.as_bool() {
                failpoint::Return(Err(errors::New("mockNextRawError")));
            }
        });
        if self.iter.is_some() {
            return Err(errors::New("selectResult is invalid after IntoIter()"));
        }
        let (resultSubset, err) = self.resp.Next(ctx);
        self.partialCount += 1;
        if let Some(err) = err {
            return Err(err);
        }
        Ok(resultSubset.map(|s| s.GetData()).unwrap_or_default())
    }

    // Close closes selectResult.
    fn Close(&mut self) -> Result<(), errors::Error> {
        if self.iter.is_some() {
            return Err(errors::New("selectResult is invalid after IntoIter()"));
        }
        self.close()
    }
}

// FillDummySummariesForTiFlashTasks fills dummy execution summaries for mpp tasks which lack summaries.
/// 为尚未记录 execution summary 的 TiFlash/MPP plan id 填充占位列表。
/// MPP：Massively Parallel Processing，TiFlash 侧并行执行模型。
pub fn FillDummySummariesForTiFlashTasks(
    runtimeStatsColl: *mut execdetails::RuntimeStatsColl,
    storeType: kv::StoreType,
    allPlanIDs: Vec<i32>,
    recordedPlanIDs: HashMap<i32, i32>,
) {
    let num: u64 = 0;
    let dummySummary = tipb::ExecutorExecutionSummary {
        TimeProcessedNs: Some(num),
        NumProducedRows: Some(num),
        NumIterations: Some(num),
        ExecutorId: None,
    };
    for planID in allPlanIDs {
        if !recordedPlanIDs.contains_key(&planID) {
            unsafe { (*runtimeStatsColl).RecordOneCopTask(planID, storeType, &dummySummary) };
        }
    }
}

// recordExecutionSummariesForTiFlashTasks records mpp task execution summaries.
pub fn recordExecutionSummariesForTiFlashTasks(
    runtimeStatsColl: *mut execdetails::RuntimeStatsColl,
    executionSummaries: Vec<*mut tipb::ExecutorExecutionSummary>,
    storeType: kv::StoreType,
    allPlanIDs: Vec<i32>,
) {
    let mut recordedPlanIDs = HashMap::new();
    for detail in executionSummaries {
        if unsafe { (*detail).has_core_fields() } {
            let id = unsafe { (*runtimeStatsColl).RecordOneCopTask(-1, storeType, detail) };
            recordedPlanIDs.insert(id, 0);
        }
    }
    FillDummySummariesForTiFlashTasks(runtimeStatsColl, storeType, allPlanIDs, recordedPlanIDs);
}

// selRespChannelIter 读取 SelectResponse 的一个输出 channel。
pub struct selRespChannelIter {
    pub channel: i32,
    pub loc: *mut time::Location,
    pub rowLen: i32,
    pub fieldTypes: Vec<*mut types::FieldType>,
    pub encodeType: tipb::EncodeType,
    pub chkData: Vec<tipb::Chunk>,

    // reserveChkSize indicates the reserved size for each chunk. (Only for default encoding)
    pub reserveChkSize: i32,
    // curChkIdx indicates the index of the current chunk in chkData read currently.
    pub curChkIdx: usize,
    // chk buffers the rows read from the current response.
    pub chk: Option<*mut chunk::Chunk>,
    // offset indicates the read offset in iter.chk.
    pub chkOffset: i32,
}

// newSelRespChannelIter chooses intermediate output or final output by channel index.
pub fn newSelRespChannelIter(result: &selectResult, channel: i32) -> Result<selRespChannelIter, errors::Error> {
    intest::Assert(result.selectResp.is_some() && result.selectResp.as_ref().unwrap().IntermediateOutputs.len() == result.intermediateOutputTypes.len());
    let intermediateOutputs = &result.selectResp.as_ref().unwrap().IntermediateOutputs;
    let mut rowLen = 0;
    let mut fieldTypes = Vec::new();
    let encodeType;
    let chkData;
    if (channel as usize) < intermediateOutputs.len() {
        fieldTypes = result.intermediateOutputTypes[channel as usize].clone();
        rowLen = fieldTypes.len() as i32;
        encodeType = intermediateOutputs[channel as usize].GetEncodeType();
        chkData = intermediateOutputs[channel as usize].GetChunks();
    } else if channel as usize == intermediateOutputs.len() {
        rowLen = result.rowLen;
        fieldTypes = result.fieldTypes.clone();
        encodeType = result.selectResp.as_ref().unwrap().GetEncodeType();
        chkData = result.selectResp.as_ref().unwrap().GetChunks();
    } else {
        return Err(errors::Errorf(format!(
            "invalid channel {} for selectResp with {} intermediate outputs",
            channel,
            intermediateOutputs.len(),
        )));
    }
    Ok(selRespChannelIter {
        channel,
        loc: unsafe { (*result.ctx).Location },
        rowLen,
        fieldTypes,
        encodeType,
        chkData,
        reserveChkSize: vardef::DefInitChunkSize,
        curChkIdx: 0,
        chk: None,
        chkOffset: 0,
    })
}

impl selRespChannelIter {
    pub fn Channel(&self) -> i32 {
        self.channel
    }

    // Next 返回当前 channel 的下一行；耗尽时返回空 SelectResultRow。
    pub fn Next(&mut self) -> Result<SelectResultRow, errors::Error> {
        if self.chk.is_some() && self.chkOffset < unsafe { (*self.chk.unwrap()).NumRows() } {
            self.chkOffset += 1;
            return Ok(SelectResultRow {
                ChannelIndex: self.channel,
                Row: unsafe { (*self.chk.unwrap()).GetRow(self.chkOffset - 1) },
            });
        }

        self.nextChunk()?;
        if self.chk.is_none() {
            return Ok(SelectResultRow { ChannelIndex: 0, Row: chunk::Row::default() });
        }
        self.chkOffset = 1;
        Ok(SelectResultRow {
            ChannelIndex: self.channel,
            Row: unsafe { (*self.chk.unwrap()).GetRow(0) },
        })
    }

    // nextChunk 根据 default/chunk encoding 填充 iter.chk。
    fn nextChunk(&mut self) -> Result<(), errors::Error> {
        self.chk = None;
        while self.curChkIdx < self.chkData.len() {
            if self.chkData[self.curChkIdx].RowsData.is_empty() {
                self.curChkIdx += 1;
                continue;
            }
            match self.encodeType {
                tipb::EncodeType_TypeDefault => {
                    let (newChk, leftRowsData) = self.fillChunkFromDefault(self.chk, self.chkData[self.curChkIdx].RowsData.clone())?;
                    self.chk = Some(newChk);
                    self.chkData[self.curChkIdx].RowsData = leftRowsData;
                    if unsafe { (*newChk).NumRows() < (*newChk).RequiredRows() } {
                        continue;
                    }
                }
                tipb::EncodeType_TypeChunk => {
                    let chk = chunk::NewChunkWithCapacity(self.fieldTypes.clone(), 0);
                    chunk::NewDecoder(chk, self.fieldTypes.clone()).Reset(self.chkData[self.curChkIdx].RowsData.clone());
                    self.chkData[self.curChkIdx].RowsData.clear();
                    self.chk = Some(chk);
                }
                _ => return Err(errors::Errorf(format!("unsupported encode type: {:?}", self.encodeType))),
            }
            if unsafe { (*self.chk.unwrap()).NumRows() } > 0 {
                break;
            }
        }
        Ok(())
    }

    // fillChunkFromDefault 解码 default encoding rowsData 到 chunk，返回未消费的 bytes。
    fn fillChunkFromDefault(
        &mut self,
        chk: Option<*mut chunk::Chunk>,
        mut rowsData: Vec<u8>,
    ) -> Result<(*mut chunk::Chunk, Vec<u8>), errors::Error> {
        let chk = chk.unwrap_or_else(|| chunk::NewChunkWithCapacity(self.fieldTypes.clone(), self.reserveChkSize));
        let mut decoder = codec::NewDecoder(chk, self.loc);
        while !rowsData.is_empty() && unsafe { (*chk).NumRows() < (*chk).RequiredRows() } {
            for i in 0..self.rowLen {
                rowsData = decoder.DecodeOne(rowsData, i, self.fieldTypes[i as usize])?;
            }
        }
        Ok((chk, rowsData))
    }
}

// selectResultIter 读取主结果和 intermediate outputs。
pub struct selectResultIter {
    pub result: *mut selectResult,
    pub channels: Vec<selRespChannelIter>,
    pub intermediateOutputTypes: Vec<Vec<*mut types::FieldType>>,
}

pub fn newSelectResultIter(
    result: &mut selectResult,
    intermediateOutputTypes: Vec<Vec<*mut types::FieldType>>,
) -> selectResultIter {
    intest::Assert(result.iter.is_none());
    selectResultIter {
        result,
        channels: Vec::new(),
        intermediateOutputTypes,
    }
}

impl SelectResultIter for selectResultIter {
    // Next implements SelectResultIter: 优先读 channel index 大的最终结果，再读较小的中间结果。
    fn Next(&mut self, ctx: context::Context) -> Result<SelectResultRow, errors::Error> {
        loop {
            let r = unsafe { &mut *self.result };
            if r.selectResp.is_none() {
                r.fetchRespWithIntermediateResults(ctx.clone(), self.intermediateOutputTypes.clone())?;
                if r.selectResp.is_none() {
                    return Ok(SelectResultRow { ChannelIndex: 0, Row: chunk::Row::default() });
                }
                if self.channels.is_empty() {
                    self.channels = Vec::with_capacity(self.intermediateOutputTypes.len() + 1);
                }
                for i in 0..=self.intermediateOutputTypes.len() {
                    self.channels.push(newSelRespChannelIter(r, i as i32)?);
                }
            }

            while !self.channels.is_empty() {
                let lastPos = self.channels.len() - 1;
                let row = self.channels[lastPos].Next()?;
                if !row.Row.IsEmpty() {
                    return Ok(row);
                }
                self.channels.truncate(lastPos);
            }
            r.selectResp = None;
        }
    }

    // Close implements SelectResultIter.
    fn Close(&mut self) -> Result<(), errors::Error> {
        unsafe { (*self.result).close() }
    }
}

// CopRuntimeStats checks whether a result has cop runtime stats.
pub trait CopRuntimeStats {
    fn GetCopRuntimeStats(&self) -> *mut copr::CopRuntimeStats;
}

// selectResultRuntimeStats 汇总 cop response 时间、processed keys、backoff、RU/cache 和并发等信息。
#[derive(Default)]
/// SelectResult 运行时统计：响应数、告警、扫描 key、缓存命中与 store batch。
pub struct selectResultRuntimeStats {
    pub copRespTime: execdetails::Percentile<execdetails::Duration>,
    pub procKeys: execdetails::Percentile<execdetails::Int64>,
    pub backoffSleep: HashMap<String, time::Duration>,
    pub totalProcessTime: time::Duration,
    pub totalWaitTime: time::Duration,
    pub reqStat: Option<tikv::RegionRequestRuntimeStats>,
    pub distSQLConcurrency: i32,
    pub extraConcurrency: i32,
    pub CoprCacheHitNum: i64,
    pub storeBatchedNum: u64,
    pub storeBatchedFallbackNum: u64,
    pub buildTaskDuration: time::Duration,
    pub fetchRspDuration: time::Duration,
}

impl selectResultRuntimeStats {
    // mergeCopRuntimeStats 将一个 cop task 的耗时、keys、backoff、request stats 和 cache hit 合入累计值。
    pub fn mergeCopRuntimeStats(&mut self, copStats: *mut copr::CopRuntimeStats, respTime: time::Duration) {
        self.copRespTime.Add(execdetails::Duration(respTime));
        let mut procKeys = execdetails::Int64(0);
        if unsafe { (*copStats).ScanDetail.is_some() } {
            procKeys = execdetails::Int64(unsafe { (*copStats).ScanDetail.as_ref().unwrap().ProcessedKeys });
        }
        self.procKeys.Add(procKeys);
        if unsafe { !(*copStats).BackoffSleep.is_empty() } {
            for (k, v) in unsafe { &(*copStats).BackoffSleep } {
                *self.backoffSleep.entry(k.clone()).or_insert(time::Duration::ZERO) += *v;
            }
        }
        self.totalProcessTime += unsafe { (*copStats).TimeDetail.ProcessTime };
        self.totalWaitTime += unsafe { (*copStats).TimeDetail.WaitTime };
        if unsafe { (*copStats).ReqStats.is_some() } {
            if self.reqStat.is_none() {
                self.reqStat = unsafe { (*copStats).ReqStats.clone() };
            } else {
                self.reqStat.as_mut().unwrap().Merge(unsafe { (*copStats).ReqStats.as_ref().unwrap() });
            }
        }
        if unsafe { (*copStats).CoprCacheHit } {
            self.CoprCacheHitNum += 1;
        }
    }

    // Clone implements RuntimeStats Clone, 深拷贝 percentile/backoff/request stats。
    pub fn Clone(&self) -> Box<dyn execdetails::RuntimeStats> {
        let mut newRs = selectResultRuntimeStats {
            copRespTime: execdetails::Percentile::default(),
            procKeys: execdetails::Percentile::default(),
            backoffSleep: HashMap::with_capacity(self.backoffSleep.len()),
            reqStat: Some(tikv::NewRegionRequestRuntimeStats()),
            distSQLConcurrency: self.distSQLConcurrency,
            extraConcurrency: self.extraConcurrency,
            CoprCacheHitNum: self.CoprCacheHitNum,
            storeBatchedNum: self.storeBatchedNum,
            storeBatchedFallbackNum: self.storeBatchedFallbackNum,
            buildTaskDuration: self.buildTaskDuration,
            fetchRspDuration: self.fetchRspDuration,
            ..Default::default()
        };
        newRs.copRespTime.MergePercentile(&self.copRespTime);
        newRs.procKeys.MergePercentile(&self.procKeys);
        for (k, v) in &self.backoffSleep {
            *newRs.backoffSleep.entry(k.clone()).or_insert(time::Duration::ZERO) += *v;
        }
        newRs.totalProcessTime += self.totalProcessTime;
        newRs.totalWaitTime += self.totalWaitTime;
        newRs.reqStat = self.reqStat.as_ref().map(|s| s.Clone());
        Box::new(newRs)
    }

    // Merge implements RuntimeStats Merge.
    pub fn Merge(&mut self, rs: &dyn execdetails::RuntimeStats) {
        let Some(other) = rs.downcast_ref::<selectResultRuntimeStats>() else {
            return;
        };
        self.copRespTime.MergePercentile(&other.copRespTime);
        self.procKeys.MergePercentile(&other.procKeys);
        for (k, v) in &other.backoffSleep {
            *self.backoffSleep.entry(k.clone()).or_insert(time::Duration::ZERO) += *v;
        }
        self.totalProcessTime += other.totalProcessTime;
        self.totalWaitTime += other.totalWaitTime;
        self.reqStat.as_mut().unwrap().Merge(other.reqStat.as_ref().unwrap());
        self.CoprCacheHitNum += other.CoprCacheHitNum;
        if other.distSQLConcurrency > self.distSQLConcurrency {
            self.distSQLConcurrency = other.distSQLConcurrency;
        }
        if other.extraConcurrency > self.extraConcurrency {
            self.extraConcurrency = other.extraConcurrency;
        }
        self.storeBatchedNum += other.storeBatchedNum;
        self.storeBatchedFallbackNum += other.storeBatchedFallbackNum;
        self.buildTaskDuration += other.buildTaskDuration;
        self.fetchRspDuration += other.fetchRspDuration;
    }

    // String formats runtime stats exactly in the Go order: cop task, rpc info, backoff.
    pub fn String(&self) -> String {
        let mut buf = String::new();
        let reqStat = self.reqStat.as_ref();
        if self.copRespTime.Size() > 0 {
            let size = self.copRespTime.Size();
            if size == 1 {
                fmt::Fprintf(
                    &mut buf,
                    format!(
                        "cop_task: {{num: 1, max: {}, proc_keys: {}",
                        execdetails::FormatDuration(time::Duration(self.copRespTime.GetPercentile(0.0))),
                        self.procKeys.GetPercentile(0.0),
                    ),
                );
            } else {
                let vMax = self.copRespTime.GetMax();
                let vMin = self.copRespTime.GetMin();
                let vP95 = self.copRespTime.GetPercentile(0.95);
                let sum = self.copRespTime.Sum();
                let vAvg = time::Duration::from_nanos((sum / size as f64) as u64);
                let keyMax = self.procKeys.GetMax();
                let keyP95 = self.procKeys.GetPercentile(0.95);
                buf.push_str(&format!(
                    "cop_task: {{num: {}, max: {}, min: {}, avg: {}, p95: {}",
                    size,
                    execdetails::FormatDuration(time::Duration(vMax.GetFloat64() as i64)),
                    execdetails::FormatDuration(time::Duration(vMin.GetFloat64() as i64)),
                    execdetails::FormatDuration(vAvg),
                    execdetails::FormatDuration(time::Duration(vP95 as i64)),
                ));
                if keyMax > 0 {
                    buf.push_str(", max_proc_keys: ");
                    buf.push_str(&keyMax.to_string());
                    buf.push_str(", p95_proc_keys: ");
                    buf.push_str(&keyP95.to_string());
                }
            }
            if self.totalProcessTime > time::Duration::ZERO {
                buf.push_str(", tot_proc: ");
                buf.push_str(&execdetails::FormatDuration(self.totalProcessTime));
                if self.totalWaitTime > time::Duration::ZERO {
                    buf.push_str(", tot_wait: ");
                    buf.push_str(&execdetails::FormatDuration(self.totalWaitTime));
                }
            }
            if config::GetGlobalConfig().TiKVClient.CoprCache.CapacityMB > 0 {
                buf.push_str(&format!(", copr_cache_hit_ratio: {:.2}", self.calcCacheHit()));
            } else {
                buf.push_str(", copr_cache: disabled");
            }
            if self.buildTaskDuration > time::Duration::ZERO {
                buf.push_str(", build_task_duration: ");
                buf.push_str(&execdetails::FormatDuration(self.buildTaskDuration));
            }
            if self.distSQLConcurrency > 0 {
                buf.push_str(", max_distsql_concurrency: ");
                buf.push_str(&self.distSQLConcurrency.to_string());
            }
            if self.extraConcurrency > 0 {
                buf.push_str(", max_extra_concurrency: ");
                buf.push_str(&self.extraConcurrency.to_string());
            }
            if self.storeBatchedNum > 0 {
                buf.push_str(", store_batch_num: ");
                buf.push_str(&self.storeBatchedNum.to_string());
            }
            if self.storeBatchedFallbackNum > 0 {
                buf.push_str(", store_batch_fallback_num: ");
                buf.push_str(&self.storeBatchedFallbackNum.to_string());
            }
            buf.push('}');
            if self.fetchRspDuration > time::Duration::ZERO {
                buf.push_str(", fetch_resp_duration: ");
                buf.push_str(&execdetails::FormatDuration(self.fetchRspDuration));
            }
        }

        if let Some(req) = reqStat {
            let rpcStatsStr = req.String();
            if !rpcStatsStr.is_empty() {
                buf.push_str(", rpc_info:{");
                buf.push_str(&rpcStatsStr);
                buf.push('}');
            }
        }

        if !self.backoffSleep.is_empty() {
            buf.push_str(", backoff{");
            let mut idx = 0;
            for (k, d) in &self.backoffSleep {
                if idx > 0 {
                    buf.push_str(", ");
                }
                idx += 1;
                buf.push_str(&format!("{}: {}", k, execdetails::FormatDuration(*d)));
            }
            buf.push('}');
        }
        buf
    }

    // Tp implements RuntimeStats interface.
    pub fn Tp(&self) -> i32 {
        execdetails::TpSelectResultRuntimeStats
    }

    // calcCacheHit calculates copr cache hit ratio, treating store batch count as extra total tasks.
    /// 计算 copr cache 命中率；store batch 计入总任务数。
    pub fn calcCacheHit(&self) -> f64 {
        let hit = self.CoprCacheHitNum;
        let mut tot = self.copRespTime.Size() as i64;
        if self.storeBatchedNum > 0 {
            tot += self.storeBatchedNum as i64;
        }
        if tot == 0 {
            return 0.0;
        }
        hit as f64 / tot as f64
    }
}
*/

// ===== 当前实现：精简版 SelectResult（不依赖真实 TiKV 响应） =====

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::time::Duration;

use crate::{DistSqlError, DistSqlResult, ResponseSource, SelectResponse};

#[derive(Clone, Debug, PartialEq)]
/// 行内标量值，用于排序比较与结果缓冲。
pub enum Scalar {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
}
impl Scalar {
    /// 按类型比较两个标量；跨类型时退化为 Debug 字符串比较。
    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Null, _) => Ordering::Less,
            (_, Self::Null) => Ordering::Greater,
            (Self::Int(a), Self::Int(b)) => a.cmp(b),
            (Self::UInt(a), Self::UInt(b)) => a.cmp(b),
            (Self::Float(a), Self::Float(b)) => a.total_cmp(b),
            (Self::Bytes(a), Self::Bytes(b)) => a.cmp(b),
            (Self::String(a), Self::String(b)) => a.cmp(b),
            (a, b) => format!("{a:?}").cmp(&format!("{b:?}")),
        }
    }
}
/// 一行结果，由若干 Scalar 组成。
pub type Row = Vec<Scalar>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 排序键：列下标与是否降序。
pub struct ByItem {
    pub column: usize,
    pub descending: bool,
}
/// 协处理器部分结果迭代器：支持原始字节、行批量读取与转换为行迭代器。
pub trait SelectResult: Send {
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>>;
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()>;
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>>;
    fn Close(&mut self) -> DistSqlResult<()>;
    fn concurrency(&self) -> Option<(usize, usize)> {
        None
    }
}
/// 按行迭代 SelectResult，并携带 channel 下标（用于中间结果通道）。
pub trait SelectResultIter: Send {
    fn Next(&mut self) -> DistSqlResult<Option<SelectResultRow>>;
    fn Close(&mut self) -> DistSqlResult<()>;
}
#[derive(Clone, Debug, Default, PartialEq)]
/// SelectResultIter 返回的一行及其所属 channel。
pub struct SelectResultRow {
    pub row: Row,
    pub channel: usize,
}
/// 若实现暴露并发度，则返回 `(concurrency, extra_concurrency)`。
pub fn GetSelectResultConcurrency(result: &dyn SelectResult) -> Option<(usize, usize)> {
    result.concurrency()
}

/// 基于 `ResponseSource` 的通用 SelectResult：缓冲行与原始字节，并累计 runtime stats。
pub struct selectResult<S: ResponseSource> {
    source: Option<S>,
    buffered: VecDeque<Row>,
    raw: VecDeque<Vec<u8>>,
    closed: bool,
    concurrency: usize,
    extra_concurrency: usize,
    pub runtime_stats: selectResultRuntimeStats,
}
impl<S: ResponseSource + 'static> selectResult<S> {
    /// 用响应源与主并发度构造 selectResult。
    pub fn new(source: S, concurrency: usize) -> Self {
        Self {
            source: Some(source),
            buffered: VecDeque::new(),
            raw: VecDeque::new(),
            closed: false,
            concurrency,
            extra_concurrency: 0,
            runtime_stats: selectResultRuntimeStats::default(),
        }
    }
    /// 从 ResponseSource 拉取下一个 SelectResponse 并消费；无更多数据时关闭。
    fn fetchResp(&mut self) -> DistSqlResult<bool> {
        if self.closed {
            return Ok(false);
        }
        let Some(response) = self
            .source
            .as_mut()
            .expect("source exists until close")
            .next_response()?
        else {
            self.Close()?;
            return Ok(false);
        };
        self.consume_response(response)
    }
    /// 将响应中的行写入缓冲，并更新扫描 key / 告警等统计。
    fn consume_response(&mut self, response: SelectResponse) -> DistSqlResult<bool> {
        self.runtime_stats.response_count += 1;
        self.runtime_stats.scanned_keys += response.scanned_keys;
        self.runtime_stats.warning_count += response.warnings.len();
        if let Some(error) = response.error {
            self.Close()?;
            return Err(DistSqlError(error));
        }
        for row in response.rows {
            self.raw.push_back(row.join("\t").into_bytes());
            self.buffered
                .push_back(row.into_iter().map(Scalar::String).collect());
        }
        Ok(true)
    }
}
impl<S: ResponseSource + 'static> SelectResult for selectResult<S> {
    // 优先弹出已缓冲的 raw；缓冲空则继续 fetchResp。
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        loop {
            if let Some(raw) = self.raw.pop_front() {
                if !self.buffered.is_empty() {
                    self.buffered.pop_front();
                }
                return Ok(Some(raw));
            }
            if !self.fetchResp()? {
                return Ok(None);
            }
        }
    }
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()> {
        while rows.len() < capacity {
            if let Some(row) = self.buffered.pop_front() {
                if !self.raw.is_empty() {
                    self.raw.pop_front();
                }
                rows.push(row);
                continue;
            }
            if !self.fetchResp()? {
                break;
            }
        }
        Ok(())
    }
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>> {
        Ok(Box::new(selectResultIter {
            result: self,
            channel: 0,
        }))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        if !self.closed {
            self.closed = true;
            self.buffered.clear();
            self.raw.clear();
            if let Some(source) = self.source.as_mut() {
                source.close()?;
            }
        }
        Ok(())
    }
    fn concurrency(&self) -> Option<(usize, usize)> {
        Some((self.concurrency, self.extra_concurrency))
    }
}
impl<S: ResponseSource> Drop for selectResult<S> {
    fn drop(&mut self) {
        if !self.closed {
            if let Some(source) = self.source.as_mut() {
                let _ = source.close();
            }
        }
    }
}

/// 将 selectResult 包装为逐行 SelectResultIter。
struct selectResultIter<S: ResponseSource> {
    result: Box<selectResult<S>>,
    channel: usize,
}
impl<S: ResponseSource + 'static> SelectResultIter for selectResultIter<S> {
    fn Next(&mut self) -> DistSqlResult<Option<SelectResultRow>> {
        let mut rows = Vec::new();
        self.result.Next(&mut rows, 1)?;
        Ok(rows.pop().map(|row| SelectResultRow {
            row,
            channel: self.channel,
        }))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        self.result.Close()
    }
}

/// 串行拼接多个 SelectResult：前一个耗尽后再读下一个。
pub struct serialSelectResults {
    results: Vec<Box<dyn SelectResult>>,
    current: usize,
}
/// 构造串行聚合的 SelectResult。
pub fn NewSerialSelectResults(results: Vec<Box<dyn SelectResult>>) -> Box<dyn SelectResult> {
    Box::new(serialSelectResults {
        results,
        current: 0,
    })
}
impl SelectResult for serialSelectResults {
    // 当前子结果耗尽后推进到下一个 SelectResult。
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        while self.current < self.results.len() {
            if let Some(data) = self.results[self.current].NextRaw()? {
                return Ok(Some(data));
            }
            self.current += 1;
        }
        Ok(None)
    }
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()> {
        while self.current < self.results.len() && rows.len() < capacity {
            let before = rows.len();
            self.results[self.current].Next(rows, capacity)?;
            if rows.len() == before {
                self.current += 1;
            }
        }
        Ok(())
    }
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>> {
        Err(DistSqlError("not implemented".into()))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        let mut last_error = None;
        for result in &mut self.results {
            if let Err(error) = result.Close() {
                last_error = Some(error);
            }
        }
        last_error.map_or(Ok(()), Err)
    }
}

/// 通用适配：把任意 SelectResult 的 `Next` 暴露为 SelectResultIter。
struct StreamIter {
    stream: Box<dyn SelectResult>,
}
impl SelectResultIter for StreamIter {
    fn Next(&mut self) -> DistSqlResult<Option<SelectResultRow>> {
        let mut rows = Vec::new();
        self.stream.Next(&mut rows, 1)?;
        Ok(rows.pop().map(|row| SelectResultRow { row, channel: 0 }))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        self.stream.Close()
    }
}

/// 多路归并排序的 SelectResult：各输入保持有序，按 ByItem 选出当前最小行。
pub struct sortedSelectResults {
    inputs: Vec<Box<dyn SelectResultIter>>,
    heads: Vec<Option<Row>>,
    order: Vec<ByItem>,
    initialized: bool,
    closed: bool,
}
/// 将多个 SelectResult 转为迭代器后构造有序归并结果（常用于分区表）。
pub fn NewSortedSelectResults(
    results: Vec<Box<dyn SelectResult>>,
    order: Vec<ByItem>,
) -> DistSqlResult<Box<dyn SelectResult>> {
    let mut inputs = Vec::with_capacity(results.len());
    for result in results {
        inputs.push(result.IntoIter()?);
    }
    let heads = vec![None; inputs.len()];
    Ok(Box::new(sortedSelectResults {
        inputs,
        heads,
        order,
        initialized: false,
        closed: false,
    }))
}
impl sortedSelectResults {
    /// 按 ByItem 顺序比较两行；降序时反转比较结果。
    fn compare_rows(order: &[ByItem], left: &Row, right: &Row) -> Ordering {
        for by in order {
            let ordering = match (left.get(by.column), right.get(by.column)) {
                (Some(left), Some(right)) => left.compare(right),
                (None, None) => Ordering::Equal,
                (None, _) => Ordering::Less,
                (_, None) => Ordering::Greater,
            };
            let ordering = if by.descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    }
    /// 惰性初始化：为每个输入预取一行作为堆顶候选。
    fn initialize(&mut self) -> DistSqlResult<()> {
        if self.initialized {
            return Ok(());
        }
        for index in 0..self.inputs.len() {
            self.heads[index] = self.inputs[index].Next()?.map(|row| row.row);
        }
        self.initialized = true;
        Ok(())
    }
    /// 选出当前最小行，并从对应输入推进下一候选。
    // 在各路 heads 中按 order 选最小行，并推进该路下一候选。
    fn next_row(&mut self) -> DistSqlResult<Option<Row>> {
        self.initialize()?;
        let Some(index) = self
            .heads
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.as_ref().map(|row| (index, row)))
            .min_by(|left, right| Self::compare_rows(&self.order, left.1, right.1))
            .map(|entry| entry.0)
        else {
            return Ok(None);
        };
        let row = self.heads[index].take();
        self.heads[index] = self.inputs[index].Next()?.map(|row| row.row);
        Ok(row)
    }
}
impl SelectResult for sortedSelectResults {
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        Err(DistSqlError(
            "NextRaw is unsupported for sorted select results".into(),
        ))
    }
    fn Next(&mut self, rows: &mut Vec<Row>, capacity: usize) -> DistSqlResult<()> {
        while rows.len() < capacity {
            let Some(row) = self.next_row()? else {
                break;
            };
            rows.push(row);
        }
        Ok(())
    }
    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn SelectResultIter>> {
        Err(DistSqlError("not implemented".into()))
    }
    fn Close(&mut self) -> DistSqlResult<()> {
        if !self.closed {
            self.closed = true;
            for input in &mut self.inputs {
                input.Close()?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// SelectResult 运行时统计：响应数、告警、扫描 key、缓存命中与 store batch。
pub struct selectResultRuntimeStats {
    pub response_count: usize,
    pub warning_count: usize,
    pub scanned_keys: u64,
    pub cop_response_time: Duration,
    pub cop_cache_hit_num: u64,
    pub store_batched_num: u64,
    pub store_batched_fallback_num: u64,
}
impl selectResultRuntimeStats {
    /// 合并单次 cop 响应的耗时、缓存命中与 store batch 计数。
    pub fn mergeCopRuntimeStats(
        &mut self,
        response_time: Duration,
        cache_hit: bool,
        store_batched: u64,
        fallback: u64,
    ) {
        self.cop_response_time += response_time;
        self.cop_cache_hit_num += u64::from(cache_hit);
        self.store_batched_num += store_batched;
        self.store_batched_fallback_num += fallback;
    }
    /// 累加另一份 runtime stats。
    pub fn Merge(&mut self, other: &Self) {
        self.response_count += other.response_count;
        self.warning_count += other.warning_count;
        self.scanned_keys += other.scanned_keys;
        self.cop_response_time += other.cop_response_time;
        self.cop_cache_hit_num += other.cop_cache_hit_num;
        self.store_batched_num += other.store_batched_num;
        self.store_batched_fallback_num += other.store_batched_fallback_num;
    }
    /// 计算 copr cache 命中率；store batch 计入总任务数。
    pub fn calcCacheHit(&self) -> f64 {
        let total = self.response_count as u64 + self.store_batched_num;
        if total == 0 {
            0.0
        } else {
            self.cop_cache_hit_num as f64 / total as f64
        }
    }
}
impl fmt::Display for selectResultRuntimeStats {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cop_task: {}, response_time: {:?}",
            self.response_count, self.cop_response_time
        )?;
        if astersql_config::get_global_config()
            .tikv_client
            .copr_cache
            .capacity_mb
            > 0
        {
            write!(formatter, ", cache_hit_ratio: {:.2}", self.calcCacheHit())
        } else {
            write!(formatter, ", copr_cache: disabled")
        }
    }
}

/// 为尚未记录 execution summary 的 TiFlash/MPP plan id 填充占位列表。
/// MPP：Massively Parallel Processing，TiFlash 侧并行执行模型。
pub fn FillDummySummariesForTiFlashTasks(
    all_plan_ids: &[i32],
    recorded: &HashMap<i32, usize>,
) -> Vec<i32> {
    all_plan_ids
        .iter()
        .copied()
        .filter(|id| !recorded.contains_key(id))
        .collect()
}
