// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Coprocessor 结果缓存（coprocessor cache）。
//
// Coprocessor（协处理器）是 TiKV 侧就地执行的算子推送请求。本模块缓存合格的
// 响应，减少重复扫描；容量、准入（admission）与分页（paging）标记共同决定
// 是否入缓存。Region 为键空间分片，缓存项携带 region id / data version 以便校验。

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use crate::batch_request_sender::{BatchError, BatchResult, KeyRange};

/// Coprocessor 缓存配置：容量与准入阈值（结果大小、range 数、最短处理时间）。
#[derive(Clone, Debug, Default)]
pub struct CoprocessorCacheConfig {
    /// 缓存总容量，单位 MB；为 0 表示关闭缓存。
    pub capacity_mb: f64,
    /// 单条响应允许入缓存的最大结果大小，单位 MB。
    pub admission_max_result_mb: f64,
    /// 请求允许入缓存的最大 KeyRange 个数；0 表示不限制。
    pub admission_max_ranges: usize,
    /// 响应允许入缓存的最短处理耗时，单位毫秒。
    pub admission_min_process_ms: u64,
}

/// 构造缓存键所需的请求摘要：类型、数据、ranges 与分页参数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoprocessorCacheRequest {
    /// 请求类型（Tp），编码进缓存键的首字节。
    pub request_type: u64,
    /// 请求体二进制数据。
    pub data: Vec<u8>,
    /// 扫描的键区间列表（KeyRange：半开区间 [start, end)）。
    pub ranges: Vec<KeyRange>,
    /// 按行数分页的 page size；非零时在键末尾追加 paging 标记。
    pub paging_size: u64,
    /// 按字节预算分页的 page size；非零时同样只追加同一标记字节。
    pub paging_size_bytes: u64,
}

/// 缓存中保存的一条 coprocessor 响应及元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoprocessorCacheValue {
    /// 与查找键一致的缓存键副本。
    pub key: Vec<u8>,
    /// 缓存的响应数据载荷。
    pub data: Vec<u8>,
    /// 写入时的时间戳（用于版本/新鲜度语义）。
    pub timestamp: u64,
    /// 所属 Region（键空间分片）的 ID。
    pub region_id: u64,
    /// Region 数据版本（epoch 相关），用于判断缓存是否仍有效。
    pub region_data_version: u64,
    /// 分页命中时回填的页起始键。
    pub page_start: Vec<u8>,
    /// 分页命中时回填的页结束键。
    pub page_end: Vec<u8>,
}

impl CoprocessorCacheValue {
    /// 估算条目占用字节数（结构体大小 + 各切片长度），用于容量记账。
    pub fn len(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.key.len()
            + self.data.len()
            + self.page_start.len()
            + self.page_end.len()
    }

    /// 是否占用为 0（通常不会出现，仅满足习惯 API）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Display for CoprocessorCacheValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{{ Ts = {}, RegionID = {}, RegionDataVersion = {}, len(Data) = {} }}",
            self.timestamp,
            self.region_id,
            self.region_data_version,
            self.data.len()
        )
    }
}

/// 缓存内部可变状态：哈希表、FIFO 插入序、当前代价与驱逐计数。
#[derive(Default)]
struct CacheState {
    /// 键到缓存值的映射。
    entries: HashMap<Vec<u8>, CoprocessorCacheValue>,
    /// 按插入顺序记录的键队列，容量不足时从队头驱逐。
    insertion_order: VecDeque<Vec<u8>>,
    /// 当前已占用的字节代价总和。
    cost: usize,
    /// 累计驱逐次数。
    evictions: u64,
}

/// 线程安全的 Coprocessor 结果缓存本体。
pub struct CoprocessorCache {
    /// 受互斥锁保护的内部状态。
    state: Mutex<CacheState>,
    /// 容量上限（字节）。
    capacity: usize,
    /// 请求准入：最大 range 数；0 表示不限制。
    admission_max_ranges: usize,
    /// 响应准入：最大结果字节数。
    admission_max_size: usize,
    /// 响应准入：最短处理时间；分页后续页可放宽为 1/3。
    admission_min_process_time: Duration,
}

impl CoprocessorCache {
    /// 按配置创建缓存；`capacity_mb == 0` 返回 `Ok(None)` 表示禁用。
    pub fn new(config: &CoprocessorCacheConfig) -> BatchResult<Option<Self>> {
        // 容量为 0 表示功能关闭，与 Go 侧 CapacityMB=0 行为一致。
        if config.capacity_mb == 0.0 {
            return Ok(None);
        }
        let capacity = (config.capacity_mb * 1024.0 * 1024.0) as usize;
        // 非零但换算后不足 1 字节视为非法配置。
        if capacity == 0 {
            return Err(BatchError::OtherResponse(
                "Capacity must be > 0 to enable the cache".to_owned(),
            ));
        }
        let maximum = (config.admission_max_result_mb * 1024.0 * 1024.0) as usize;
        if maximum == 0 {
            return Err(BatchError::OtherResponse(
                "AdmissionMaxResultMB must be > 0 to enable the cache".to_owned(),
            ));
        }
        Ok(Some(Self {
            state: Mutex::new(CacheState::default()),
            capacity,
            admission_max_ranges: config.admission_max_ranges,
            admission_max_size: maximum,
            admission_min_process_time: Duration::from_millis(config.admission_min_process_ms),
        }))
    }

    /// 按键查找缓存项；键不一致时视为未命中。
    pub fn get(&self, key: &[u8]) -> Option<CoprocessorCacheValue> {
        self.state
            .lock()
            .expect("coprocessor cache lock poisoned")
            .entries
            .get(key)
            .filter(|value| value.key == key)
            .cloned()
    }

    /// 请求侧准入：range 数量是否不超过上限。
    pub fn check_request_admission(&self, ranges: usize) -> bool {
        self.admission_max_ranges == 0 || ranges <= self.admission_max_ranges
    }

    /// 响应侧准入：结果大小、处理耗时与分页任务序号共同判定。
    pub fn check_response_admission(
        &self,
        data_size: usize,
        process_time: Duration,
        paging_task_index: u32,
    ) -> bool {
        // 空结果、过大结果或分页序号过高直接拒绝。
        if data_size == 0 || data_size > self.admission_max_size || paging_task_index > 50 {
            return false;
        }
        // 分页后续页降低最短处理时间门槛（约为原阈值的 1/3）。
        let minimum = if paging_task_index > 0 {
            self.admission_min_process_time / 3
        } else {
            self.admission_min_process_time
        };
        process_time >= minimum
    }

    /// 写入缓存；单条过大则失败，否则按 FIFO 驱逐直至腾出空间。
    pub fn set(&self, key: Vec<u8>, mut value: CoprocessorCacheValue) -> bool {
        value.key.clone_from(&key);
        let cost = value.len();
        if cost > self.capacity {
            return false;
        }
        let mut state = self.state.lock().expect("coprocessor cache lock poisoned");
        // 覆盖同键旧值时先扣回其代价并从插入序中移除。
        if let Some(old) = state.entries.remove(&key) {
            state.cost = state.cost.saturating_sub(old.len());
            state.insertion_order.retain(|item| item != &key);
        }
        // 容量不足时按插入顺序从队头驱逐，直到能放下新条目。
        while state.cost + cost > self.capacity {
            let Some(old_key) = state.insertion_order.pop_front() else {
                break;
            };
            if let Some(old) = state.entries.remove(&old_key) {
                state.cost = state.cost.saturating_sub(old.len());
                state.evictions += 1;
            }
        }
        state.cost += cost;
        state.insertion_order.push_back(key.clone());
        state.entries.insert(key, value);
        true
    }

    /// 返回累计驱逐次数。
    pub fn evictions(&self) -> u64 {
        self.state
            .lock()
            .expect("coprocessor cache lock poisoned")
            .evictions
    }
}

/// 将请求摘要编码为缓存键：Tp、Data 长度与内容、各 range，以及可选 paging 标记。
pub fn coprocessor_cache_build_key(request: &CoprocessorCacheRequest) -> BatchResult<Vec<u8>> {
    let request_type = u8::try_from(request.request_type)
        .map_err(|_| BatchError::OtherResponse("Request Tp too big".to_owned()))?;
    let data_len = u32::try_from(request.data.len())
        .map_err(|_| BatchError::OtherResponse("Cache data too big".to_owned()))?;
    let mut key = Vec::with_capacity(
        5 + request.data.len()
            + request
                .ranges
                .iter()
                .map(|range| 4 + range.start.len() + range.end.len())
                .sum::<usize>()
            + usize::from(request.paging_size > 0 || request.paging_size_bytes > 0),
    );
    key.push(request_type);
    key.extend_from_slice(&data_len.to_le_bytes());
    key.extend_from_slice(&request.data);
    // 每个 KeyRange 以 start/end 长度前缀 + 内容顺序拼入键。
    for range in &request.ranges {
        let start_len = u16::try_from(range.start.len())
            .map_err(|_| BatchError::OtherResponse("Cache start key too big".to_owned()))?;
        let end_len = u16::try_from(range.end.len())
            .map_err(|_| BatchError::OtherResponse("Cache end key too big".to_owned()))?;
        key.extend_from_slice(&start_len.to_le_bytes());
        key.extend_from_slice(&range.start);
        key.extend_from_slice(&end_len.to_le_bytes());
        key.extend_from_slice(&range.end);
    }
    // 行数或字节预算任一启用分页时，只追加单一 marker 字节（不含具体 size）。
    if request.paging_size > 0 || request.paging_size_bytes > 0 {
        key.push(1);
    }
    Ok(key)
}

/// Go 风格类型别名：`coprCache` 对应 `CoprocessorCache`。
#[allow(non_camel_case_types)]
pub type coprCache = CoprocessorCache;
/// Go 风格类型别名：`coprCacheValue` 对应 `CoprocessorCacheValue`。
#[allow(non_camel_case_types)]
pub type coprCacheValue = CoprocessorCacheValue;
