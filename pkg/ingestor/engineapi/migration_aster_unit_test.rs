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

// `engineapi` 迁移单元测试：校验与 Go 行为对齐的冲突合并、导入数据迭代与引擎通道语义。
//
// 通过内存假实现（`VecIter` / `MemoryData` / `MockEngine`）覆盖 `IngestData`、`Engine`
// 等接口的关键路径，避免依赖真实存储或外部排序引擎。

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;

use crate::*;

/// 将普通消息包装为 `EngineError`，便于测试断言错误路径。
fn engine_error(message: &str) -> EngineError {
    std::io::Error::new(std::io::ErrorKind::Other, message).into()
}

/// 基于内存 KV 切片的前向迭代器，实现 `ForwardIter` 供单元测试使用。
struct VecIter {
    /// 有序（或测试给定顺序）的键值对列表。
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
    /// 当前位置；`None` 表示尚未定位或已释放。
    position: Option<usize>,
    /// `Close` 后置位，使 `Valid` 恒为 false。
    closed: bool,
}

impl ForwardIter for VecIter {
    /// 定位到首个键值对；空集合时返回 false。
    fn First(&mut self) -> bool {
        self.position = (!self.pairs.is_empty()).then_some(0);
        self.Valid()
    }

    /// 迭代器是否指向合法位置且未关闭。
    fn Valid(&self) -> bool {
        !self.closed && self.position.is_some_and(|index| index < self.pairs.len())
    }

    /// 前进一位并返回新位置是否仍有效。
    fn Next(&mut self) -> bool {
        if let Some(position) = &mut self.position {
            *position += 1;
        }
        self.Valid()
    }

    /// 当前键；调用前须保证 `Valid()`。
    fn Key(&self) -> &[u8] {
        &self.pairs[self.position.unwrap()].0
    }

    /// 当前值；调用前须保证 `Valid()`。
    fn Value(&self) -> &[u8] {
        &self.pairs[self.position.unwrap()].1
    }

    /// 关闭迭代器，后续 `Valid` 为 false。
    fn Close(&mut self) -> Result<(), EngineError> {
        self.closed = true;
        Ok(())
    }

    /// 内存实现无错误，恒返回 `None`。
    fn Error(&self) -> Option<&EngineError> {
        None
    }

    /// 释放内部缓冲并清空当前位置。
    fn ReleaseBuf(&mut self) {
        self.pairs.clear();
        self.position = None;
    }
}

/// 内存版 `IngestData`：持有 KV、时间戳与引用计数，供引擎导入路径测试。
struct MemoryData {
    pairs: Vec<(Vec<u8>, Vec<u8>)>,
    /// 提交时间戳（TS），对应导入批次的可见性版本。
    ts: u64,
    /// 引用计数，模拟下游持有期间的生命周期。
    refs: AtomicI64,
    /// `Finish` 累计导入字节数。
    finished_bytes: AtomicI64,
    /// `Finish` 累计导入条数。
    finished_count: AtomicI64,
}

impl IngestData for MemoryData {
    /// 在半开区间 `[lower, upper)` 内取首尾键；空界表示无界。
    fn GetFirstAndLastKey(
        &self,
        lower_bound: &[u8],
        upper_bound: &[u8],
    ) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), EngineError> {
        // 空上下界视为无限制，与 Go 半开区间语义一致。
        let in_range = |key: &[u8]| {
            (lower_bound.is_empty() || key >= lower_bound)
                && (upper_bound.is_empty() || key < upper_bound)
        };
        let mut keys = self
            .pairs
            .iter()
            .map(|pair| pair.0.as_slice())
            .filter(|key| in_range(key));
        let first = keys.next().map(ToOwned::to_owned);
        // 仅一个键时 last 回退为 first，避免二次遍历失败。
        let last = keys.last().map(ToOwned::to_owned).or_else(|| first.clone());
        Ok((first, last))
    }

    /// 按范围过滤后构造 `VecIter`；忽略上下文与缓冲池参数。
    fn NewIter(
        &self,
        _ctx: &Context,
        lower_bound: &[u8],
        upper_bound: &[u8],
        _buf_pool: &mut membuf::Pool,
    ) -> Box<dyn ForwardIter> {
        // 复制范围内 KV，使迭代器与底层数据解耦。
        let pairs = self
            .pairs
            .iter()
            .filter(|pair| {
                (lower_bound.is_empty() || pair.0.as_slice() >= lower_bound)
                    && (upper_bound.is_empty() || pair.0.as_slice() < upper_bound)
            })
            .cloned()
            .collect();
        Box::new(VecIter {
            pairs,
            position: None,
            closed: false,
        })
    }

    fn GetTS(&self) -> u64 {
        self.ts
    }

    fn IncRef(&self) {
        self.refs.fetch_add(1, Ordering::SeqCst);
    }

    fn DecRef(&self) {
        self.refs.fetch_sub(1, Ordering::SeqCst);
    }

    /// 累加下游报告的导入字节与条数。
    fn Finish(&self, total_bytes: i64, total_count: i64) {
        self.finished_bytes.fetch_add(total_bytes, Ordering::SeqCst);
        self.finished_count.fetch_add(total_count, Ordering::SeqCst);
    }
}

/// 假引擎：向通道推送固定批次，并暴露统计与冲突信息。
struct MockEngine {
    closed: bool,
    /// `LoadIngestData` 成功次数，用于观测调用。
    loaded: AtomicU64,
    conflict: Mutex<ConflictInfo>,
}

impl Engine for MockEngine {
    fn ID(&self) -> String {
        "engine-168".to_owned()
    }

    /// 向 `out_ch` 发送一份固定批次；上下文已取消时立即失败。
    fn LoadIngestData(
        &self,
        ctx: &Context,
        out_ch: &SyncSender<DataAndRanges>,
    ) -> Result<(), EngineError> {
        if ctx.is_cancelled() {
            return Err(engine_error("context cancelled"));
        }
        let data = MemoryData {
            pairs: vec![
                (b"a".to_vec(), b"1".to_vec()),
                (b"b".to_vec(), b"2".to_vec()),
            ],
            ts: 168,
            refs: AtomicI64::new(0),
            finished_bytes: AtomicI64::new(0),
            finished_count: AtomicI64::new(0),
        };
        // 半开区间 [a, c) 覆盖上述两个键。
        out_ch
            .send(DataAndRanges {
                Data: Box::new(data),
                SortedRanges: vec![Range {
                    Start: b"a".to_vec(),
                    End: b"c".to_vec(),
                }],
            })
            .map_err(|error| engine_error(&error.to_string()))?;
        self.loaded.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn KVStatistics(&self) -> (i64, i64) {
        (4, 2)
    }
    fn ImportedStatistics(&self) -> (i64, i64) {
        (2, 1)
    }
    fn ConflictInfo(&self) -> ConflictInfo {
        self.conflict.lock().unwrap().clone()
    }
    fn GetKeyRange(&self) -> Result<(Vec<u8>, Vec<u8>), EngineError> {
        Ok((b"a".to_vec(), b"c".to_vec()))
    }
    /// Region 分裂键（Region：TiKV 数据分片单位），测试用固定三点。
    fn GetRegionSplitKeys(&self) -> Result<Vec<Vec<u8>>, EngineError> {
        Ok(vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()])
    }
    fn Close(&mut self) -> Result<(), EngineError> {
        self.closed = true;
        Ok(())
    }
}

/// 构造含 key1..=key5 的标准 `MemoryData` 夹具。
fn memory_data() -> MemoryData {
    MemoryData {
        pairs: vec![
            (b"key1".to_vec(), b"value1".to_vec()),
            (b"key2".to_vec(), b"value2".to_vec()),
            (b"key3".to_vec(), b"value3".to_vec()),
            (b"key4".to_vec(), b"value4".to_vec()),
            (b"key5".to_vec(), b"value5".to_vec()),
        ],
        ts: 123,
        refs: AtomicI64::new(0),
        finished_bytes: AtomicI64::new(0),
        finished_count: AtomicI64::new(0),
    }
}

/// 校验 `ConflictInfo::Merge` 的 uint64 溢出回绕，以及重复键策略字符串与 Go 一致。
#[test]
fn migration_conflict_merge_and_duplicate_strings_match_go() {
    let mut info = ConflictInfo {
        Count: u64::MAX,
        Files: vec!["first.sst".into()],
    };
    info.Merge(&ConflictInfo {
        Count: 2,
        Files: vec!["second.sst".into(), "third.sst".into()],
    });
    assert_eq!(info.Count, 1, "Go uint64 addition wraps on overflow");
    assert_eq!(info.Files, ["first.sst", "second.sst", "third.sst"]);

    assert_eq!(OnDuplicateKeyIgnore.String(), "ignore");
    assert_eq!(OnDuplicateKeyRecord.String(), "record");
    assert_eq!(OnDuplicateKeyRemove.String(), "remove");
    assert_eq!(OnDuplicateKeyError.String(), "error");
    assert_eq!(OnDuplicateKey(99).String(), "unknown");
}

/// 校验范围首尾键、迭代、缓冲释放、引用计数与 Finish 累加与 Go 对齐。
#[test]
fn migration_ingest_data_range_iteration_ref_and_finish_match_go() {
    let data = memory_data();
    assert_eq!(data.GetTS(), 123);
    assert_eq!(
        data.GetFirstAndLastKey(b"key2", b"key5").unwrap(),
        (Some(b"key2".to_vec()), Some(b"key4".to_vec()))
    );
    assert_eq!(
        data.GetFirstAndLastKey(b"key25", b"key26").unwrap(),
        (None, None)
    );

    let mut pool = membuf::NewPool(vec![]);
    let mut iter = data.NewIter(
        &Context::background(),
        b"key2",
        b"key5",
        std::sync::Arc::get_mut(&mut pool).unwrap(),
    );
    assert!(iter.First());
    let mut seen = Vec::new();
    while iter.Valid() {
        seen.push((iter.Key().to_vec(), iter.Value().to_vec()));
        iter.Next();
    }
    assert_eq!(seen.len(), 3);
    assert!(iter.Error().is_none());
    iter.ReleaseBuf();
    assert!(!iter.Valid());
    iter.Close().unwrap();

    data.IncRef();
    data.IncRef();
    data.DecRef();
    data.DecRef();
    data.Finish(10, 1);
    data.Finish(20, 2);
    assert_eq!(data.refs.load(Ordering::SeqCst), 0);
    assert_eq!(data.finished_bytes.load(Ordering::SeqCst), 30);
    assert_eq!(data.finished_count.load(Ordering::SeqCst), 3);
}

/// 校验引擎通过 channel 投递批次、统计查询、取消上下文与 Close 行为。
#[test]
fn migration_engine_channel_context_statistics_and_ranges_match_go() {
    let mut engine = MockEngine {
        closed: false,
        loaded: AtomicU64::new(0),
        conflict: Mutex::new(ConflictInfo {
            Count: 1,
            Files: vec!["conflict.sst".into()],
        }),
    };
    let ctx = Context::background();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);

    engine.LoadIngestData(&ctx, &sender).unwrap();
    let batch = receiver.recv().unwrap();
    assert_eq!(batch.SortedRanges.len(), 1);
    assert_eq!(batch.SortedRanges[0].Start, b"a");
    assert_eq!(batch.SortedRanges[0].End, b"c");
    assert_eq!(batch.Data.GetTS(), 168);
    assert_eq!(engine.ID(), "engine-168");
    assert_eq!(engine.KVStatistics(), (4, 2));
    assert_eq!(engine.ImportedStatistics(), (2, 1));
    assert_eq!(engine.ConflictInfo().Count, 1);
    assert_eq!(
        engine.GetKeyRange().unwrap(),
        (b"a".to_vec(), b"c".to_vec())
    );
    assert_eq!(
        engine.GetRegionSplitKeys().unwrap(),
        vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
    );

    // 取消后 LoadIngestData 必须失败，且 Close 置位 closed。
    let cancelled = Context::background();
    cancelled.cancel();
    assert!(engine.LoadIngestData(&cancelled, &sender).is_err());
    engine.Close().unwrap();
    assert!(engine.closed);
}
