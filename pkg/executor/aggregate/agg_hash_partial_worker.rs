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

// Hash 聚合 Partial Worker：消费 child chunk、更新 partial result、按需 spill。
//
// Partial 阶段对输入行计算 group key、维护 AggMap；内存超限时通过
// `ParallelHashAggSpillHelper` 落盘。shuffle 阶段按 key 哈希分发给 Final Worker。

// HashAggPartialWorker 负责从 child chunk 计算 group key、更新 partial result、按 final worker 分片并在需要时 spill。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// HashAggPartialWorker indicates the partial workers of parallel hash agg execution,
// the number of the worker can be set by `tidb_hashagg_partial_concurrency`.
// HashAggPartialWorker 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct HashAggPartialWorker {
//     pub baseHashAggWorker: baseHashAggWorker,
//     pub idForTest: i32,
//     pub ctx: sessionctx::Context,
//     pub inputCh: channel::Channel</* Go: chan *chunk.Chunk */>,
//     pub outputChs: Vec<channel::Channel</* Go: chan aggfuncs.AggPartialResultMapper */>>,
//     pub globalOutputCh: channel::Channel</* Go: chan *AfFinalResult */>,
// Partial worker transmit the HashAggInput by this channel,
// so that the data fetcher could get the partial worker's HashAggInput
//     pub giveBackCh: channel::Channel</* Go: chan<- *HashAggInput */>,
//     pub partialResultsBuffer: Vec<Vec<aggfuncs::PartialResult>>,
//     pub partialResultNumInRow: i32,
// Length of this map is equal to the number of final workers
// All data in one AggPartialResultMapper are specifically sent to a target final worker.
// e.g. all data in partialResultsMap[3] should be sent to final worker 3.
//     pub partialResultsMap: Vec<aggfuncs::AggPartialResultMapper>,
//     pub partialResultsMapMem: atomic::Int64,
//     pub groupByItems: Vec<expression::Expression>,
//     pub groupKeyBuf: Vec<Vec<byte>>,
// chk stores the input data from child,
// and is reused by childExec and partial worker.
//     pub chk: Box<chunk::Chunk>,
//     pub isSpillPrepared: bool,
//     pub spillHelper: Box<parallelHashAggSpillHelper>,
//     pub tmpChksForSpill: Vec<Box<chunk::Chunk>>,
//     pub serializeHelpers: Box<aggfuncs::SerializeHelper>,
//     pub spilledChunksIO: Vec<Box<chunk::DataInDiskByChunks>>,
// It's useful when spill is triggered and the fetcher could know when partial workers finish their works.
//     pub inflightChunkSync: Box<sync::WaitGroup>,
//     pub fileNamePrefixForTest: String,
// }
// getChildInput 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggPartialWorker {
//     pub fn getChildInput(&mut self/* Go args:  */) /* Go returns: (*chunk.Chunk, bool) */ {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     select {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case <-w.finishCh:
//         return nil, false
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case chk, ok := <-w.inputCh:
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if !ok {
//             return nil, false
//         }
//         return chk, true
//     }
// }
// }
// */
use crate::agg_spill::ParallelHashAggSpillHelper;
use crate::agg_util::{AggMap, AggState, Aggregation, Chunk, Row, get_group_key};
use std::sync::Arc;

/// `twmb/murmur3.Sum32`: MurmurHash3 x86 32-bit with seed 0.
pub(crate) fn murmur3_sum32(input: &[u8]) -> u32 {
    let mut hash = 0_u32;
    let mut chunks = input.chunks_exact(4);
    for chunk in &mut chunks {
        let mut key = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        key = key.wrapping_mul(0xcc9e_2d51);
        key = key.rotate_left(15);
        key = key.wrapping_mul(0x1b87_3593);
        hash ^= key;
        hash = hash.rotate_left(13);
        hash = hash.wrapping_mul(5).wrapping_add(0xe654_6b64);
    }

    let tail = chunks.remainder();
    let mut key = 0_u32;
    match tail.len() {
        3 => {
            key ^= u32::from(tail[2]) << 16;
            key ^= u32::from(tail[1]) << 8;
            key ^= u32::from(tail[0]);
        }
        2 => {
            key ^= u32::from(tail[1]) << 8;
            key ^= u32::from(tail[0]);
        }
        1 => key ^= u32::from(tail[0]),
        _ => {}
    }
    if !tail.is_empty() {
        key = key.wrapping_mul(0xcc9e_2d51);
        key = key.rotate_left(15);
        key = key.wrapping_mul(0x1b87_3593);
        hash ^= key;
    }

    hash ^= input.len() as u32;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^ (hash >> 16)
}

/// Partial Worker 状态：分组列、聚合描述、可选 spill、内存估算与 AggMap。
pub struct HashAggPartialWorker {
    pub group_columns: Vec<usize>,
    pub aggregations: Arc<Vec<Aggregation>>,
    pub spill: Option<Arc<ParallelHashAggSpillHelper>>,
    map: AggMap,
    memory_usage: usize,
}

impl HashAggPartialWorker {
    /// 构造 Partial Worker，初始 map 为空。
    pub fn new(
        group_columns: Vec<usize>,
        aggregations: Arc<Vec<Aggregation>>,
        spill: Option<Arc<ParallelHashAggSpillHelper>>,
    ) -> Self {
        Self {
            group_columns,
            aggregations,
            spill,
            map: AggMap::new(),
            memory_usage: 0,
        }
    }

    /// 消费一个输入 chunk：编码 group key、更新各聚合态，必要时触发 spill。
    pub fn update_partial_result(&mut self, chunk: &Chunk) -> Result<(), String> {
        for row in chunk {
            let key = get_group_key(row, &self.group_columns)?;
            let group_row = self
                .group_columns
                .iter()
                .map(|index| {
                    row.get(*index)
                        .cloned()
                        .ok_or_else(|| format!("group column {index} out of range"))
                })
                .collect::<Result<Row, _>>()?;
            // 仅新 group 计入内存增量，避免重复累加已有槽位。
            let is_new = !self.map.contains_key(&key);
            let (_, states) = self.map.entry(key.clone()).or_insert_with(|| {
                (
                    group_row,
                    self.aggregations.iter().map(|_| AggState::new()).collect(),
                )
            });
            for (state, aggregation) in states.iter_mut().zip(self.aggregations.iter()) {
                state.update(aggregation, row)?;
            }
            if is_new {
                self.memory_usage = self
                    .memory_usage
                    .saturating_add(key.len() + states.len() * std::mem::size_of::<AggState>());
            }
        }
        if let Some(spill) = &self.spill {
            // 超限则交出当前 map 落盘，并清零内存计数。
            if spill.set_need_spill(self.memory_usage) {
                let data = std::mem::take(&mut self.map);
                spill.spill(data)?;
                self.memory_usage = 0;
            }
        }
        Ok(())
    }

    /// 按 group key 哈希把中间结果分片到各 Final Worker 对应的 map。
    pub fn shuffle_intermediate_data(&mut self, final_concurrency: usize) -> Vec<AggMap> {
        let mut outputs = (0..final_concurrency.max(1))
            .map(|_| AggMap::new())
            .collect::<Vec<_>>();
        for (key, value) in std::mem::take(&mut self.map) {
            let worker = murmur3_sum32(&key) as usize % outputs.len();
            outputs[worker].insert(key, value);
        }
        self.memory_usage = 0;
        outputs
    }

    /// 强制把残留 map 落盘（收尾阶段）；未配置 spill 则报错。
    pub fn spill_remaining(&mut self) -> Result<(), String> {
        if self.map.is_empty() {
            return Ok(());
        }
        match &self.spill {
            Some(spill) => {
                let data = std::mem::take(&mut self.map);
                spill.spill(data)?;
                self.memory_usage = 0;
                Ok(())
            }
            None => Err("spill helper is not configured".to_string()),
        }
    }
}
