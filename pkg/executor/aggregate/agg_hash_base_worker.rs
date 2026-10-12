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
