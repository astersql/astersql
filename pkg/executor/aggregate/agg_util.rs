// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 聚合执行器通用工具：值类型、聚合态、group key 编码与运行时统计。
//
// 提供 Hash/Stream 聚合共享的 `Aggregation`/`AggState`/`get_group_key`，
// 以及 `HashAggRuntimeStats` 合并逻辑。Go 侧缓冲池与 spill Action 保留在注释块。

// 聚合执行器通用工具，保留 partial result/group key 缓冲池、group key 编码、运行时统计和 spill action 选择逻辑。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// Go const defaultPartialResultsBufferCap：保持原常量表达式。
// pub const defaultPartialResultsBufferCap: usize = 2048;
// Go const defaultGroupKeyCap：保持原常量表达式。
// pub const defaultGroupKeyCap: usize = 8;
// 包级 var 声明：保留 Go 初始化表达式，真实 Rust 静态生命周期后续再细化。
// var partialResultsBufferPool = sync.Pool{
//     New: func() any {
//         s := make([][]aggfuncs.PartialResult, 0, defaultPartialResultsBufferCap)
//         return &s
//     },
// }
// 包级 var 声明：保留 Go 初始化表达式，真实 Rust 静态生命周期后续再细化。
// var groupKeyPool = sync.Pool{
//     New: func() any {
//         s := make([][]byte, 0, defaultGroupKeyCap)
//         return &s
//     },
// }
// getBuffer 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn getBuffer() /* Go returns: (*[][]aggfuncs.PartialResult, *[][]byte) */ {
//     partialResultsBuffer := partialResultsBufferPool.Get().(*[][]aggfuncs.PartialResult)
//     *partialResultsBuffer = (*partialResultsBuffer)[:0]
//     groupKey := groupKeyPool.Get().(*[][]byte)
//     *groupKey = (*groupKey)[:0]
//     return partialResultsBuffer, groupKey
// }
// tryRecycleBuffer recycles small buffers only. This approach reduces the CPU pressure
// from memory allocation during high concurrency aggregation computations (like DDL's scheduled tasks),
// and also prevents the pool from holding too much memory and causing memory pressure.
// tryRecycleBuffer 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn tryRecycleBuffer(/* Go args: buf *[][]aggfuncs.PartialResult, groupKey *[][]byte */) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if cap(*buf) <= defaultPartialResultsBufferCap {
//         partialResultsBufferPool.Put(buf)
//     }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if cap(*groupKey) <= defaultGroupKeyCap {
//         groupKeyPool.Put(groupKey)
//     }
// }
// closeBaseExecutor 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn closeBaseExecutor(/* Go args: b *exec.BaseExecutor */) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if r := recover(); r != nil {
// Release the resource, but throw the panic again and let the top level handle it.
//         terror.Log(b.Close())
//         logutil.BgLogger().Warn("panic in Open(), close base executor and throw exception again")
//         panic(r)
//     }
// }
// recoveryHashAgg 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn recoveryHashAgg(/* Go args: output chan *AfFinalResult, r any */) {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//     err := util.GetRecoverError(r)
// channel 说明：这里对应 Go channel 发送/接收，不建立真实异步运行时。
//     output <- &AfFinalResult{err: err}
//     logutil.BgLogger().Warn("parallel hash aggregation panicked", zap.Error(err), zap.Stack("stack"))
// }
// getGroupKeyMemUsage 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn getGroupKeyMemUsage(/* Go args: groupKey [][]byte */) /* Go returns: int64 */ {
//     mem := int64(0)
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, key := range groupKey {
//         mem += int64(cap(key))
//     }
//     mem += aggfuncs.DefSliceSize * int64(cap(groupKey))
//     return mem
// }
// GetGroupKey evaluates the group items and args of aggregate functions.
// GetGroupKey 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn GetGroupKey(/* Go args: ctx sessionctx.Context, input *chunk.Chunk, groupKey [][]byte, groupByItems []expression.Expression */) /* Go returns: ([][]byte, error) */ {
//     numRows := input.NumRows()
//     avlGroupKeyLen := min(len(groupKey), numRows)
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for i := range avlGroupKeyLen {
//         groupKey[i] = groupKey[i][:0]
//     }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for i := avlGroupKeyLen; i < numRows; i++ {
//         groupKey = append(groupKey, make([]byte, 0, 10*len(groupByItems)))
//     }
//     errCtx := ctx.GetSessionVars().StmtCtx.ErrCtx()
//     exprCtx := ctx.GetExprCtx()
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, item := range groupByItems {
//         tp := item.GetType(ctx.GetExprCtx().GetEvalCtx())
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         buf, err := expression.GetColumn(tp.EvalType(), numRows)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//         if err != nil {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             return nil, err
//         }
// In strict sql mode like ‘STRICT_TRANS_TABLES’，can not insert an invalid enum value like 0.
// While in sql mode like '', can insert an invalid enum value like 0,
// then the enum value 0 will have the enum name '', which maybe conflict with user defined enum ''.
// Ref to issue #26885.
// This check is used to handle invalid enum name same with user defined enum name.
// Use enum value as groupKey instead of enum name.
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//         if item.GetType(ctx.GetExprCtx().GetEvalCtx()).GetType() == mysql.TypeEnum {
//             newTp := *tp
//             newTp.AddFlag(mysql.EnumSetAsIntFlag)
//             tp = &newTp
//         }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//         if err := expression.EvalExpr(exprCtx.GetEvalCtx(), ctx.GetSessionVars().EnableVectorizedExpression, item, tp.EvalType(), input, buf); err != nil {
//             expression.PutColumn(buf)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             return nil, err
//         }
// This check is used to avoid error during the execution of `EncodeDecimal`.
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//         if item.GetType(ctx.GetExprCtx().GetEvalCtx()).GetType() == mysql.TypeNewDecimal {
//             newTp := *tp
//             newTp.SetFlen(0)
//             tp = &newTp
//         }
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         groupKey, err = codec.HashGroupKey(ctx.GetSessionVars().StmtCtx.TimeZone(), input.NumRows(), buf, groupKey, tp)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         err = errCtx.HandleError(err)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//         if err != nil {
//             expression.PutColumn(buf)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             return nil, err
//         }
//         expression.PutColumn(buf)
//     }
// 返回说明：沿用 Go 的 nil/error 返回约定，Rust Result 形状仅作提示。
//     return groupKey[:numRows], nil
// }
// HashAggRuntimeStats record the HashAggExec runtime stat
// HashAggRuntimeStats 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct HashAggRuntimeStats {
//     pub PartialConcurrency: i32,
//     pub PartialWallTime: i64,
//     pub FinalConcurrency: i32,
//     pub FinalWallTime: i64,
//     pub PartialStats: Vec<Box<AggWorkerStat>>,
//     pub FinalStats: Vec<Box<AggWorkerStat>>,
// }
// workerString 对应 Go 方法：接收者为 `*HashAggRuntimeStats`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggRuntimeStats {
//     pub fn workerString(&mut self, /* Go args: buf *bytes.Buffer, prefix string, concurrency int, wallTime int64, workerStats []*AggWorkerStat */) {
//     var totalTime, totalWait, totalExec, totalTaskNum int64
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, w := range workerStats {
//         totalTime += w.WorkerTime
//         totalWait += w.WaitTime
//         totalExec += w.ExecTime
//         totalTaskNum += w.TaskNum
//     }
//     buf.WriteString(prefix)
//     fmt.Fprintf(buf, "_worker:{wall_time:%s, concurrency:%d, task_num:%d, tot_wait:%s, tot_exec:%s, tot_time:%s",
//         time.Duration(wallTime), concurrency, totalTaskNum, time.Duration(totalWait), time.Duration(totalExec), time.Duration(totalTime))
//     n := len(workerStats)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if n > 0 {
//         slices.SortFunc(workerStats, func(i, j *AggWorkerStat) int { return cmp.Compare(i.WorkerTime, j.WorkerTime) })
//         fmt.Fprintf(buf, ", max:%v, p95:%v",
//             time.Duration(workerStats[n-1].WorkerTime), time.Duration(workerStats[n*19/20].WorkerTime))
//     }
//     buf.WriteString("}")
// }
// }
// String implements the RuntimeStats interface.
// String 对应 Go 方法：接收者为 `e *HashAggRuntimeStats`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggRuntimeStats {
//     pub fn String(&mut self) /* Go returns: string */ {
//     buf := bytes.NewBuffer(make([]byte, 0, 64))
//     e.workerString(buf, "partial", e.PartialConcurrency, atomic.LoadInt64(&e.PartialWallTime), e.PartialStats)
//     buf.WriteString(", ")
//     e.workerString(buf, "final", e.FinalConcurrency, atomic.LoadInt64(&e.FinalWallTime), e.FinalStats)
//     return buf.String()
// }
// }
// Clone implements the RuntimeStats interface.
// Clone 对应 Go 方法：接收者为 `e *HashAggRuntimeStats`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggRuntimeStats {
//     pub fn Clone(&mut self) /* Go returns: execdetails.RuntimeStats */ {
//     newRs := &HashAggRuntimeStats{
//         PartialConcurrency: e.PartialConcurrency,
//         PartialWallTime:    atomic.LoadInt64(&e.PartialWallTime),
//         FinalConcurrency:   e.FinalConcurrency,
//         FinalWallTime:      atomic.LoadInt64(&e.FinalWallTime),
//         PartialStats:       make([]*AggWorkerStat, 0, e.PartialConcurrency),
//         FinalStats:         make([]*AggWorkerStat, 0, e.FinalConcurrency),
//     }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, s := range e.PartialStats {
//         newRs.PartialStats = append(newRs.PartialStats, s.Clone())
//     }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, s := range e.FinalStats {
//         newRs.FinalStats = append(newRs.FinalStats, s.Clone())
//     }
//     return newRs
// }
// }
// Merge implements the RuntimeStats interface.
// Merge 对应 Go 方法：接收者为 `e *HashAggRuntimeStats`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggRuntimeStats {
//     pub fn Merge(&mut self, /* Go args: other execdetails.RuntimeStats */) {
//     tmp, ok := other.(*HashAggRuntimeStats)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if !ok {
//         return
//     }
//     atomic.AddInt64(&e.PartialWallTime, atomic.LoadInt64(&tmp.PartialWallTime))
//     atomic.AddInt64(&e.FinalWallTime, atomic.LoadInt64(&tmp.FinalWallTime))
//     e.PartialStats = append(e.PartialStats, tmp.PartialStats...)
//     e.FinalStats = append(e.FinalStats, tmp.FinalStats...)
// }
// }
// Tp implements the RuntimeStats interface.
// Tp 对应 Go 方法：接收者为 `*HashAggRuntimeStats`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggRuntimeStats {
//     pub fn Tp(&mut self) /* Go returns: int */ {
//     return execdetails.TpHashAggRuntimeStat
// }
// }
// AggWorkerInfo contains the agg worker information.
// AggWorkerInfo 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct AggWorkerInfo {
//     pub Concurrency: i32,
//     pub WallTime: i64,
// }
// AggWorkerStat record the AggWorker runtime stat
// AggWorkerStat 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct AggWorkerStat {
//     pub TaskNum: i64,
//     pub WaitTime: i64,
//     pub ExecTime: i64,
//     pub WorkerTime: i64,
// }
// Clone implements the RuntimeStats interface.
// Clone 对应 Go 方法：接收者为 `w *AggWorkerStat`，保留原方法的控制流、错误返回和资源处理顺序。
// impl AggWorkerStat {
//     pub fn Clone(&mut self) /* Go returns: *AggWorkerStat */ {
//     return &AggWorkerStat{
//         TaskNum:    w.TaskNum,
//         WaitTime:   w.WaitTime,
//         ExecTime:   w.ExecTime,
//         WorkerTime: w.WorkerTime,
//     }
// }
// }
// actionSpillForUnparallel 对应 Go 方法：接收者为 `e *HashAggExec`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggExec {
//     pub fn actionSpillForUnparallel(&mut self) /* Go returns: memory.ActionOnExceed */ {
//     e.spillAction = &AggSpillDiskAction{
//         e: e,
//     }
//     return e.spillAction
// }
// }
// actionSpillForParallel 对应 Go 方法：接收者为 `e *HashAggExec`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggExec {
//     pub fn actionSpillForParallel(&mut self) /* Go returns: memory.ActionOnExceed */ {
//     e.parallelAggSpillAction = &ParallelAggSpillDiskAction{
//         e:           e,
//         spillHelper: e.spillHelper,
//     }
//     return e.parallelAggSpillAction
// }
// }
// ActionSpill returns an action for spilling intermediate data for hashAgg.
// ActionSpill 对应 Go 方法：接收者为 `e *HashAggExec`，保留原方法的控制流、错误返回和资源处理顺序。
// impl HashAggExec {
//     pub fn ActionSpill(&mut self) /* Go returns: memory.ActionOnExceed */ {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if e.IsUnparallelExec {
//         return e.actionSpillForUnparallel()
//     }
//     return e.actionSpillForParallel()
// }
// }
// failpointError 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn failpointError() /* Go returns: error */ {
//     var err error
//     failpoint.Inject("enableAggSpillIntest", func(val failpoint.Value) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//         if val.(bool) {
//             num := rand.Intn(1000)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//             if num < 3 {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//                 err = errors.Errorf("Random fail is triggered in ParallelAggSpillDiskAction")
//             }
//         }
//     })
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//     return err
// }
// updateWaitTime 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn updateWaitTime(/* Go args: stats *AggWorkerStat, startTime time.Time */) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if stats != nil {
//         stats.WaitTime += int64(time.Since(startTime))
//     }
// }
// updateWorkerTime 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn updateWorkerTime(/* Go args: stats *AggWorkerStat, startTime time.Time */) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if stats != nil {
//         stats.WorkerTime += int64(time.Since(startTime))
//     }
// }
// updateExecTime 对应 Go 函数：保留原函数的参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn updateExecTime(/* Go args: stats *AggWorkerStat, startTime time.Time */) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if stats != nil {
//         stats.ExecTime += int64(time.Since(startTime))
//         stats.TaskNum++
//     }
// }
// */
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
/// 聚合求值使用的简化单元格类型（Null/整数/浮点/文本/字节/布尔）。
pub enum Value {
    Null,
    Integer(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Bool(bool),
}
/// 一行：单元格向量。
pub type Row = Vec<Value>;
/// 一批行，对应执行器 chunk。
pub type Chunk = Vec<Row>;
/// group key → (分组列取值, 各聚合态) 的有序映射。
pub type AggMap = std::collections::BTreeMap<Vec<u8>, (Row, Vec<AggState>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 支持的聚合种类（COUNT/SUM/MIN/MAX/FIRST）。
pub enum AggKind {
    Count,
    Sum,
    Min,
    Max,
    First,
}
#[derive(Clone, Debug, PartialEq)]
/// 单个聚合描述：种类、输入列、是否 DISTINCT。
pub struct Aggregation {
    pub kind: AggKind,
    pub column: Option<usize>,
    pub distinct: bool,
}
impl Aggregation {
    /// 构造非 DISTINCT 聚合。
    pub fn new(kind: AggKind, column: Option<usize>) -> Self {
        Self {
            kind,
            column,
            distinct: false,
        }
    }

    /// 构造 DISTINCT 聚合（更新前先做去重）。
    pub fn new_distinct(kind: AggKind, column: Option<usize>) -> Self {
        Self {
            kind,
            column,
            distinct: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
/// 单个聚合的中间态：计数、数值累加、比较值与 DISTINCT 集合。
pub struct AggState {
    pub count: u64,
    pub number: Option<f64>,
    pub value: Option<Value>,
    distinct_values: std::collections::BTreeMap<Vec<u8>, Value>,
}

impl AggState {
    /// 创建空中间态。
    pub fn new() -> Self {
        Self {
            count: 0,
            number: None,
            value: None,
            distinct_values: std::collections::BTreeMap::new(),
        }
    }
    /// 用一行输入更新本聚合态；DISTINCT 时重复值直接跳过。
    pub fn update(&mut self, aggregation: &Aggregation, row: &Row) -> Result<(), String> {
        let value = aggregation
            .column
            .and_then(|index| row.get(index))
            .cloned()
            .unwrap_or(Value::Integer(1));
        // DISTINCT：用编码后的值作 key，已见过则不再 update_value。
        if aggregation.distinct {
            let mut key = Vec::new();
            encode_value(&value, &mut key);
            if self.distinct_values.insert(key, value.clone()).is_some() {
                return Ok(());
            }
        }
        self.update_value(aggregation.kind, value);
        Ok(())
    }

    /// 按聚合种类更新 count/number/value 字段。
    fn update_value(&mut self, kind: AggKind, value: Value) {
        match kind {
            AggKind::Count => {
                if !matches!(value, Value::Null) {
                    self.count += 1;
                }
            }
            AggKind::Sum => {
                if let Some(number) = as_number(&value) {
                    self.number = Some(self.number.unwrap_or(0.0) + number);
                }
            }
            AggKind::Min => {
                if !matches!(value, Value::Null)
                    && self
                        .value
                        .as_ref()
                        .is_none_or(|old| compare_values(&value, old).is_lt())
                {
                    self.value = Some(value);
                }
            }
            AggKind::Max => {
                if !matches!(value, Value::Null)
                    && self
                        .value
                        .as_ref()
                        .is_none_or(|old| compare_values(&value, old).is_gt())
                {
                    self.value = Some(value);
                }
            }
            AggKind::First => {
                if self.value.is_none() {
                    self.value = Some(value);
                }
            }
        }
    }
    /// 合并另一 partial 态（Final 阶段）；DISTINCT 需并入去重集合。
    pub fn merge(&mut self, aggregation: &Aggregation, other: &Self) {
        // DISTINCT merge：只并入尚未见过的值并同步 update_value。
        if aggregation.distinct {
            for (key, value) in &other.distinct_values {
                if !self.distinct_values.contains_key(key) {
                    self.distinct_values.insert(key.clone(), value.clone());
                    self.update_value(aggregation.kind, value.clone());
                }
            }
            return;
        }
        match aggregation.kind {
            AggKind::Count => self.count += other.count,
            AggKind::Sum => {
                if let Some(value) = other.number {
                    self.number = Some(self.number.unwrap_or(0.0) + value);
                }
            }
            AggKind::Min => {
                if let Some(value) = &other.value {
                    if self
                        .value
                        .as_ref()
                        .is_none_or(|old| compare_values(value, old).is_lt())
                    {
                        self.value = Some(value.clone());
                    }
                }
            }
            AggKind::Max => {
                if let Some(value) = &other.value {
                    if self
                        .value
                        .as_ref()
                        .is_none_or(|old| compare_values(value, old).is_gt())
                    {
                        self.value = Some(value.clone());
                    }
                }
            }
            AggKind::First => {
                if self.value.is_none() {
                    self.value = other.value.clone();
                }
            }
        }
    }
    /// 取出最终聚合值；无结果时返回 Null。
    pub fn result(&self, kind: AggKind) -> Value {
        match kind {
            AggKind::Count => Value::Integer(self.count.min(i64::MAX as u64) as i64),
            AggKind::Sum => self.number.map(Value::Float).unwrap_or(Value::Null),
            AggKind::Min | AggKind::Max | AggKind::First => {
                self.value.clone().unwrap_or(Value::Null)
            }
        }
    }
}

/// 按分组列顺序编码一行的 group key。
pub fn get_group_key(row: &Row, group_columns: &[usize]) -> Result<Vec<u8>, String> {
    let mut key = Vec::new();
    for column in group_columns {
        let value = row
            .get(*column)
            .ok_or_else(|| format!("group column {column} out of range"))?;
        encode_value(value, &mut key);
    }
    Ok(key)
}

/// 带类型标签的紧凑编码，保证不同 Value 变体 key 不冲突。
fn encode_value(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Null => output.push(0),
        Value::Integer(value) => {
            output.push(1);
            output.extend(value.to_le_bytes());
        }
        Value::Float(value) => {
            output.push(2);
            output.extend(value.to_bits().to_le_bytes());
        }
        Value::Text(value) => {
            output.push(3);
            output.extend((value.len() as u64).to_le_bytes());
            output.extend(value.as_bytes());
        }
        Value::Bytes(value) => {
            output.push(4);
            output.extend((value.len() as u64).to_le_bytes());
            output.extend(value);
        }
        Value::Bool(value) => {
            output.push(5);
            output.push(*value as u8);
        }
    }
}
/// 将整数/浮点转为 f64 供 SUM 累加；其它类型忽略。
fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(value) => Some(*value as f64),
        Value::Float(value) => Some(*value),
        _ => None,
    }
}
/// MIN/MAX 比较；同型直接比，跨型回退到 Debug 字符串序。
fn compare_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    match (left, right) {
        (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
        (Value::Float(left), Value::Float(right)) => left.total_cmp(right),
        (Value::Text(left), Value::Text(right)) => left.cmp(right),
        (Value::Bytes(left), Value::Bytes(right)) => left.cmp(right),
        (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
        _ => format!("{left:?}").cmp(&format!("{right:?}")),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 单个 Partial/Final worker 的耗时与任务计数。
pub struct AggWorkerStat {
    pub worker_time: Duration,
    pub wait_time: Duration,
    pub exec_time: Duration,
    pub task_count: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// HashAgg 运行时统计：partial/final worker 列表与 spill 次数。
pub struct HashAggRuntimeStats {
    pub partial: Vec<AggWorkerStat>,
    pub final_workers: Vec<AggWorkerStat>,
    pub spill_count: u64,
}
impl HashAggRuntimeStats {
    /// 合并另一份运行时统计（多轮执行或并行汇总）。
    pub fn merge(&mut self, other: &Self) {
        // Go 保留每个 worker 的独立样本，String 由完整样本集计算 max/p95。
        // 按下标相加会把不同执行轮次误认为同一 worker，并将 max/p95 翻倍。
        self.partial.extend_from_slice(&other.partial);
        self.final_workers.extend_from_slice(&other.final_workers);
        self.spill_count += other.spill_count;
    }
}
