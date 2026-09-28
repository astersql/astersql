// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 表采样执行器单元测试。
//
// 用内存 `SampleRuntime` 模拟 Region 边界、分区与并发扫描，覆盖基础采样、
// 多 Region 切分、无切分整表、分区降序、chunk 容量上限与 keyspace 并发扫描。
#![allow(non_snake_case)]

use crate::sample::{
    KeyRange, SampleKv, TableRegionSampler, TableSampleExecutor, TableSampleRuntime,
    newTableRegionSampler, scanFirstKVForEachRange, sortRanges, splitIntoMultiRanges,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 测试用采样运行时：可注入边界、分区、空结果与扫描延迟。
#[derive(Clone)]
struct SampleRuntime {
    /// Region 边界键；`None` 表示不切分。
    boundaries: Option<Vec<Vec<u8>>>,
    /// 分区键范围；空表示非分区表。
    partitions: Vec<KeyRange>,
    /// 并发扫描线程数。
    concurrency: usize,
    empty_starts: Vec<Vec<u8>>,
    delayed_starts: Vec<Vec<u8>>,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl Default for SampleRuntime {
    fn default() -> Self {
        Self {
            boundaries: None,
            partitions: Vec::new(),
            concurrency: 4,
            empty_starts: Vec::new(),
            delayed_starts: Vec::new(),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// 将测试桩接到 `TableSampleRuntime`：扫首 KV 时直接返回区间端点键。
impl TableSampleRuntime for SampleRuntime {
    type Context = ();
    type Request = Vec<(Vec<u8>, Vec<u8>)>;
    type Handle = Vec<u8>;
    type Value = Vec<u8>;
    type Row = (Vec<u8>, Vec<u8>);
    type Column = ();
    type DecodeColumnMap = ();
    type Error = String;

    fn reset_request(&self, request: &mut Self::Request) {
        request.clear();
    }
    fn required_rows(&self, request: &Self::Request) -> usize {
        2usize.saturating_sub(request.len())
    }
    fn request_is_full(&self, request: &Self::Request) -> bool {
        request.len() >= 2
    }
    fn append_row(&self, request: &mut Self::Request, row: Self::Row) {
        self.events.lock().unwrap().push("append");
        request.push(row);
    }
    fn table_key_range(&self, physical_table_id: i64) -> KeyRange {
        KeyRange {
            start_key: format!("t{physical_table_id}:0").into_bytes(),
            end_key: format!("t{physical_table_id}:z").into_bytes(),
        }
    }
    fn partition_key_ranges(&self) -> Vec<KeyRange> {
        self.partitions.clone()
    }
    fn region_boundaries(&mut self, _: &KeyRange) -> Result<Option<Vec<Vec<u8>>>, Self::Error> {
        Ok(self.boundaries.clone())
    }
    fn no_regions_error(&self) -> Self::Error {
        "table has no regions".into()
    }
    fn executor_concurrency(&self) -> usize {
        self.concurrency
    }
    fn scan_first_kv(
        &mut self,
        range: &KeyRange,
        _: u64,
    ) -> Result<Option<SampleKv<Self::Handle, Self::Value>>, Self::Error> {
        if self.delayed_starts.contains(&range.start_key) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if self.empty_starts.contains(&range.start_key) {
            return Ok(None);
        }
        let key = range.start_key.clone();
        Ok(Some(SampleKv {
            handle: key.clone(),
            value: key,
        }))
    }
    fn build_sample_columns(
        &mut self,
    ) -> Result<(Vec<Self::Column>, Self::DecodeColumnMap), Self::Error> {
        Ok((vec![()], ()))
    }
    fn decode_row(
        &mut self,
        handle: Self::Handle,
        value: Self::Value,
        _: &[Self::Column],
        _: &Self::DecodeColumnMap,
    ) -> Result<Self::Row, Self::Error> {
        self.events.lock().unwrap().push("decode");
        Ok((handle, value))
    }
    fn reset_row_map(&mut self) {
        self.events.lock().unwrap().push("reset");
    }
}

/// 多 Region 下分多轮 Next，直到区间耗尽。
#[test]
fn TestTableSampleBasic() {
    let runtime = SampleRuntime {
        boundaries: Some(vec![b"t1:3".to_vec(), b"t1:6".to_vec()]),
        ..Default::default()
    };
    let mut executor = TableSampleExecutor {
        sampler: newTableRegionSampler(runtime, 42, 1, false),
    };
    let mut rows = Vec::new();
    executor.Open(&mut ()).unwrap();
    executor.Next(&mut (), &mut rows).unwrap();
    assert_eq!(rows.len(), 2);
    executor.Next(&mut (), &mut rows).unwrap();
    assert_eq!(rows.len(), 1);
    executor.Next(&mut (), &mut rows).unwrap();
    assert!(rows.is_empty());
    executor.Close().unwrap();
}

/// 边界乱序/重复时切分结果仍正确去重排序。
#[test]
fn TestTableSampleMultiRegions() {
    let mut runtime = SampleRuntime {
        boundaries: Some(vec![b"d".to_vec(), b"b".to_vec(), b"d".to_vec()]),
        ..Default::default()
    };
    let ranges = splitIntoMultiRanges(
        &mut runtime,
        KeyRange {
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
        },
    )
    .unwrap();
    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[1].start_key, b"b");
    assert_eq!(ranges[1].end_key, b"d");
}

/// 无 Region 边界时整表保持单一区间。
#[test]
fn TestTableSampleNoSplitTable() {
    let mut sampler = TableRegionSampler {
        runtime: SampleRuntime::default(),
        start_timestamp: 1,
        physical_table_id: 8,
        descending: false,
        ranges: None,
    };
    let ranges = sampler.splitTableRanges().unwrap();
    assert_eq!(ranges, vec![sampler.runtime.table_key_range(8)]);
}

/// 分区表降序初始化后区间按 start_key 逆序排列。
#[test]
fn TestTableSamplePlan() {
    let runtime = SampleRuntime {
        partitions: vec![
            KeyRange {
                start_key: b"p0".to_vec(),
                end_key: b"p1".to_vec(),
            },
            KeyRange {
                start_key: b"p1".to_vec(),
                end_key: b"p2".to_vec(),
            },
        ],
        ..Default::default()
    };
    let mut sampler = newTableRegionSampler(runtime, 9, 99, true);
    sampler.initRanges().unwrap();
    let ranges = sampler.ranges.unwrap();
    assert_eq!(ranges[0].start_key, b"p1");
    assert_eq!(ranges[1].start_key, b"p0");
}

/// 写入受 request 容量限制（此处满 2 行即停）。
#[test]
fn TestMaxChunkSize() {
    let runtime = SampleRuntime::default();
    let ranges = (0..10)
        .map(|index| KeyRange {
            start_key: vec![index],
            end_key: vec![index + 1],
        })
        .collect();
    let mut sampler = newTableRegionSampler(runtime, 1, 1, false);
    let mut request = Vec::new();
    sampler.writeChunkFromRanges(ranges, &mut request).unwrap();
    assert_eq!(request.len(), 2);
}

/// 多区间并发扫描后按 handle 排序校验覆盖性。
#[test]
fn TestKeyspaceSample() {
    let runtime = SampleRuntime {
        concurrency: 3,
        ..Default::default()
    };
    let mut ranges = vec![
        KeyRange {
            start_key: b"keyspace-2".to_vec(),
            end_key: b"keyspace-3".to_vec(),
        },
        KeyRange {
            start_key: b"keyspace-1".to_vec(),
            end_key: b"keyspace-2".to_vec(),
        },
    ];
    sortRanges(&mut ranges, false);
    let samples = scanFirstKVForEachRange(runtime, ranges, 10).unwrap();
    let mut keys = samples
        .into_iter()
        .map(|sample| sample.handle)
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, vec![b"keyspace-1".to_vec(), b"keyspace-2".to_vec()]);
}

/// Go consumes the sorted range prefix; sampling must not reshuffle regions.
#[test]
fn pick_ranges_preserves_sorted_prefix() {
    let runtime = SampleRuntime::default();
    let mut sampler = newTableRegionSampler(runtime, 1, 1, false);
    sampler.ranges = Some(
        [b"a", b"b", b"c"]
            .into_iter()
            .map(|start| KeyRange {
                start_key: start.to_vec(),
                end_key: vec![start[0] + 1],
            })
            .collect(),
    );

    let picked = sampler.pickRanges(2);

    assert_eq!(
        picked
            .into_iter()
            .map(|range| range.start_key)
            .collect::<Vec<_>>(),
        vec![b"a".to_vec(), b"b".to_vec()]
    );
}

/// Empty regions are skipped and later ranges are consumed in the same Next call.
#[test]
fn write_chunk_refills_after_empty_region() {
    let runtime = SampleRuntime {
        boundaries: Some(vec![b"t1:3".to_vec(), b"t1:6".to_vec()]),
        empty_starts: vec![b"t1:0".to_vec()],
        ..Default::default()
    };
    let mut sampler = newTableRegionSampler(runtime, 1, 1, false);
    let mut request = Vec::new();

    sampler.writeChunk(&mut request).unwrap();

    assert_eq!(request.len(), 2);
    assert!(sampler.finished());
}

/// The syncer consumes results in range order even when later workers finish first.
#[test]
fn concurrent_scan_preserves_range_order() {
    let runtime = SampleRuntime {
        concurrency: 2,
        delayed_starts: vec![b"a".to_vec()],
        ..Default::default()
    };
    let ranges = vec![
        KeyRange {
            start_key: b"a".to_vec(),
            end_key: b"b".to_vec(),
        },
        KeyRange {
            start_key: b"b".to_vec(),
            end_key: b"c".to_vec(),
        },
    ];

    let samples = scanFirstKVForEachRange(runtime, ranges, 1).unwrap();

    assert_eq!(
        samples
            .into_iter()
            .map(|sample| sample.handle)
            .collect::<Vec<_>>(),
        vec![b"a".to_vec(), b"b".to_vec()]
    );
}

/// Go clears the reusable row map only after the decoded row is appended.
#[test]
fn row_map_is_reset_after_append() {
    let runtime = SampleRuntime::default();
    let events = Arc::clone(&runtime.events);
    let mut sampler = newTableRegionSampler(runtime, 1, 1, false);
    let mut request = Vec::new();

    sampler
        .writeChunkFromRanges(
            vec![KeyRange {
                start_key: b"a".to_vec(),
                end_key: b"b".to_vec(),
            }],
            &mut request,
        )
        .unwrap();

    assert_eq!(*events.lock().unwrap(), vec!["decode", "append", "reset"]);
}

/// Descending changes range order, not the ascending scan inside each range.
#[test]
fn descending_sampling_scans_each_range_forward() {
    let runtime = SampleRuntime {
        partitions: vec![
            KeyRange {
                start_key: b"a".to_vec(),
                end_key: b"b".to_vec(),
            },
            KeyRange {
                start_key: b"b".to_vec(),
                end_key: b"c".to_vec(),
            },
        ],
        ..Default::default()
    };
    let mut sampler = newTableRegionSampler(runtime, 1, 1, true);
    let mut request = Vec::new();

    sampler.writeChunk(&mut request).unwrap();

    assert_eq!(
        request
            .into_iter()
            .map(|(handle, _)| handle)
            .collect::<Vec<_>>(),
        vec![b"b".to_vec(), b"a".to_vec()]
    );
}
