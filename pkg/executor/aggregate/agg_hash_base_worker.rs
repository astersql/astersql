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

// Hash 聚合 partial/final worker 的公共基座。
//
// Hash 聚合（Hash Aggregation）按 group key 哈希分桶后并行聚合；
// `BaseHashAggWorker` 持有聚合描述、chunk 大小上限与 finish 标志，
// 供 PartialWorker 与 FinalWorker 复用。

// baseHashAggWorker 保存 HashAggFinalWorker 与 HashAggPartialWorker 的公共字段，保留 finish channel、agg 函数、chunk 大小、统计和内存 tracker 的 Go 结构。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// baseHashAggWorker stores the common attributes of HashAggFinalWorker and HashAggPartialWorker.
// nolint:structcheck
// baseHashAggWorker 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct baseHashAggWorker {
//     pub finishCh: channel::Channel</* Go: <-chan struct{} */>,
//     pub aggFuncs: Vec<aggfuncs::AggFunc>,
//     pub maxChunkSize: i32,
//     pub stats: Box<AggWorkerStat>,
//     pub memTracker: Box<memory::Tracker>,
// }
// newBaseHashAggWorker 对应 Go 函数：保留原参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn newBaseHashAggWorker(/* Go args: finishCh <-chan struct{}, aggFuncs []aggfuncs.AggFunc, maxChunkSize int, memTrack *memory.Tracker */) /* Go returns: baseHashAggWorker */ {
//     baseWorker := baseHashAggWorker{
//         finishCh:     finishCh,
//         aggFuncs:     aggFuncs,
//         maxChunkSize: maxChunkSize,
//         memTracker:   memTrack,
//     }
//     return baseWorker
// }
// getPartialResultSliceLenConsiderByteAlign 对应 Go 方法：接收者为 `w *baseHashAggWorker`，保留控制流、错误返回和资源处理顺序。
// impl baseHashAggWorker {
//     pub fn getPartialResultSliceLenConsiderByteAlign(&mut self/* Go args:  */) /* Go returns: int */ {
//     length := len(w.aggFuncs)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if length == 1 {
//         return 1
//     }
//     return length + length&1
// }
// }
// checkFinishChClosed 对应 Go 方法：接收者为 `w *baseHashAggWorker`，保留控制流、错误返回和资源处理顺序。
// impl baseHashAggWorker {
//     pub fn checkFinishChClosed(&mut self/* Go args:  */) /* Go returns: bool */ {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     select {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case <-w.finishCh:
//         return true
//     default:
//     }
//     return false
// }
// }
// */
use crate::agg_util::Aggregation;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Partial/Final worker 共享字段：聚合列表、输出 chunk 上限、结束标志。
pub struct BaseHashAggWorker {
    pub aggregations: Arc<Vec<Aggregation>>,
    pub max_chunk_size: usize,
    finish: Arc<AtomicBool>,
}
impl BaseHashAggWorker {
    /// 构造基座；与 Go 一致，原样保存调用方给出的 `max_chunk_size`。
    pub fn new(
        finish: Arc<AtomicBool>,
        aggregations: Arc<Vec<Aggregation>>,
        max_chunk_size: usize,
    ) -> Self {
        Self {
            finish,
            aggregations,
            max_chunk_size,
        }
    }
    /// 按缓存行大小对齐 partial result 槽位数（与 Go `getPartialResultSliceLenConsiderByteAlign` 一致）。
    /// Align partial result slots to cache-line sized groups as the Go worker does.
    pub fn aligned_partial_result_len(&self) -> usize {
        let count = self.aggregations.len();
        if count == 1 {
            return 1;
        }
        count + (count & 1)
    }
    /// 查询 finish 标志：主线程关闭后 worker 应尽快退出。
    pub fn is_finished(&self) -> bool {
        self.finish.load(Ordering::Acquire)
    }
}
