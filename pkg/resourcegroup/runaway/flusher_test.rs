// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// batchFlusher 测试，覆盖 add 达阈值刷新、mergeFn 聚合、显式 flush 和空 flush。
// testing/testify、mock、JWK/JWT、TiDB session、metrics、infoschema 等外部依赖均按 Go 调用形状保留。

// BatchFlusher 单元测试：阈值刷盘、merge 聚合与显式空 flush。
//
// 前半保留 Go 测试参考字符串；后半用 Rust 对照验证 add 达阈值触发 flush、
// mergeFn 累加 Repeats，以及 flushIfDue 按时间间隔刷出。

const _GO_FLUSHER_TEST_REFERENCE: &str = r###"
#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

// newTestBatchFlusher 对应 Go 的同名测试/辅助函数。保留 Go 辅助函数/方法的调用形状；时间/休眠边界按原测试断言保留；错误分支和错误文本按 Go 测试保留。
// Go 签名: func newTestBatchFlusher[K comparable, V any](
	threshold int,
	mergeFn func(map[K]V, K, V),
	flushFn func(map[K]V) error,
) *batchFlusher[K, V] {
pub fn new_test_batch_flusher() {
		return &batchFlusher[K, V]{
			name:                "test",
			buffer:              make(map[K]V, threshold),
			ticker:              time.NewTicker(time.Hour),
			threshold:           threshold,
			mergeFn:             mergeFn,
			flushFn:             flushFn,
			batchSizeObserver:   metrics.RunawayFlusherBatchSizeHistogram.WithLabelValues("test"),
			durationObserver:    metrics.RunawayFlusherDurationHistogram.WithLabelValues("test"),
			intervalObserver:    metrics.RunawayFlusherIntervalHistogram.WithLabelValues("test"),
			flushSuccessCounter: metrics.RunawayFlusherCounter.WithLabelValues("test", metrics.LblOK),
			flushErrorCounter:   metrics.RunawayFlusherCounter.WithLabelValues("test", metrics.LblError),
			addCounter:          metrics.RunawayFlusherAddCounter.WithLabelValues("test"),
		}
}

// TestBatchFlusherAdd 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestBatchFlusherAdd(t *testing.T) {
#[test]
pub fn test_batch_flusher_add() {
		re := require.New(t)

    // 原测试检查 atomic/CAS 可见性；保留 load/store/compare-and-swap 位置。
		var flushCount atomic.Int32
		flusher := newTestBatchFlusher(
			3,
			func(m map[string]int, k string, v int) { m[k] = v },
			func(m map[string]int) error { flushCount.Add(1); return nil },
		)
		re.Empty(flusher.buffer)

		flusher.add("a", 1)
		flusher.add("b", 2)
		re.Len(flusher.buffer, 2)
		re.Equal(int32(0), flushCount.Load())

		flusher.add("c", 3)
		re.Len(flusher.buffer, 0)
		re.Equal(int32(1), flushCount.Load())

		flusher.add("d", 4)
		re.Len(flusher.buffer, 1)
		re.Equal(int32(1), flushCount.Load())
}

// TestBatchFlusherMergeFn 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestBatchFlusherMergeFn(t *testing.T) {
#[test]
pub fn test_batch_flusher_merge_fn() {
		re := require.New(t)

		var lastBuffer map[string]*Record
		flusher := newTestBatchFlusher(
			10,
			func(m map[string]*Record, k string, v *Record) {
				if existing, ok := m[k]; ok {
					existing.Repeats++
				} else {
					m[k] = v
				}
			},
			func(m map[string]*Record) error { lastBuffer = m; return nil },
		)

		flusher.add("key1", &Record{SQLDigest: "d1", Repeats: 1})
		flusher.add("key1", &Record{SQLDigest: "d1", Repeats: 1})
		flusher.add("key1", &Record{SQLDigest: "d1", Repeats: 1})
		flusher.add("key2", &Record{SQLDigest: "d2", Repeats: 1})

		re.Len(flusher.buffer, 2)
		re.Equal(3, flusher.buffer["key1"].Repeats)
		re.Equal(1, flusher.buffer["key2"].Repeats)

		flusher.flush()
		re.Len(flusher.buffer, 0)
		re.Equal(3, lastBuffer["key1"].Repeats)
}

// TestBatchFlusherFlush 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestBatchFlusherFlush(t *testing.T) {
#[test]
pub fn test_batch_flusher_flush() {
		re := require.New(t)

    // 原测试检查 atomic/CAS 可见性；保留 load/store/compare-and-swap 位置。
		var flushCount atomic.Int32
		flusher := newTestBatchFlusher(
			100,
			func(m map[string]int, k string, v int) { m[k] = v },
			func(m map[string]int) error { flushCount.Add(1); return nil },
		)
		re.Empty(flusher.buffer)

		flusher.add("a", 1)
		re.Len(flusher.buffer, 1)
		re.Equal(int32(0), flushCount.Load())

		flusher.flush()
		re.Len(flusher.buffer, 0)
		re.Equal(int32(1), flushCount.Load())

		flusher.add("b", 2)
		re.Len(flusher.buffer, 1)
		re.Equal(int32(1), flushCount.Load())
}

// TestBatchFlusherFlushEmpty 对应 Go 的同名测试/辅助函数。保留 Go 测试函数和子测试断言顺序；testify 断言保留原期望文本；错误分支和错误文本按 Go 测试保留。
// Go 签名: func TestBatchFlusherFlushEmpty(t *testing.T) {
#[test]
pub fn test_batch_flusher_flush_empty() {
		re := require.New(t)

    // 原测试检查 atomic/CAS 可见性；保留 load/store/compare-and-swap 位置。
		var flushCount atomic.Int32
		flusher := newTestBatchFlusher(
			10,
			func(m map[string]int, k string, v int) { m[k] = v },
			func(m map[string]int) error { flushCount.Add(1); return nil },
		)

		flusher.flush()
		re.Equal(int32(0), flushCount.Load())

		flusher.add("a", 1)
		flusher.flush()
		re.Equal(int32(1), flushCount.Load())

		flusher.flush()
		re.Equal(int32(1), flushCount.Load())
}
"###;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::flusher::BatchFlusher;
use crate::record::Record;

/// 达阈值自动刷盘；空缓冲上的显式 flush 不增加成功计数。
#[test]
fn threshold_flush_and_explicit_empty_flush_match_go() {
    let batches = Arc::new(Mutex::new(Vec::<HashMap<String, i32>>::new()));
    let captured = batches.clone();
    let mut flusher = BatchFlusher::new(
        "test",
        Duration::from_secs(3600),
        3,
        |buffer, key, value| {
            buffer.insert(key, value);
        },
        move |buffer| {
            captured.lock().unwrap().push(buffer.clone());
            Ok(())
        },
    )
    .unwrap();
    flusher.add("a".into(), 1).unwrap();
    flusher.add("b".into(), 2).unwrap();
    assert_eq!(flusher.len(), 2);
    flusher.add("c".into(), 3).unwrap();
    assert!(flusher.is_empty());
    assert_eq!(flusher.successful_flushes, 1);
    flusher.flush().unwrap();
    assert_eq!(flusher.successful_flushes, 1);
    assert_eq!(batches.lock().unwrap()[0].len(), 3);
}

/// merge 累加同一键的 Repeats，并按 interval 到期刷出单批。
#[test]
fn merge_and_interval_flush_preserve_one_batch() {
    let flushed = Arc::new(Mutex::new(Vec::<HashMap<String, Record>>::new()));
    let captured = flushed.clone();
    let mut flusher = BatchFlusher::new(
        "records",
        Duration::from_millis(1),
        10,
        |buffer, key, value: Record| {
            if let Some(existing) = buffer.get_mut(&key) {
                existing.Repeats += value.Repeats;
            } else {
                buffer.insert(key, value);
            }
        },
        move |buffer| {
            captured.lock().unwrap().push(buffer.clone());
            Ok(())
        },
    )
    .unwrap();
    // 三次同键入队，Repeats 应变为 3。
    for _ in 0..3 {
        flusher
            .add(
                "key1".into(),
                Record {
                    SQLDigest: "d1".into(),
                    Repeats: 1,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    assert_eq!(flusher.buffer()["key1"].Repeats, 3);
    assert!(flusher.flushIfDue(Instant::now()).unwrap());
    assert_eq!(flushed.lock().unwrap()[0]["key1"].Repeats, 3);
}

/// Go 的 `flush` 只记录写出错误，不向 `add`/调用方传播；失败批次仍会被丢弃。
#[test]
fn flush_error_is_counted_and_swallowed_before_next_batch() {
    let attempts = Arc::new(Mutex::new(Vec::<HashMap<String, i32>>::new()));
    let captured = attempts.clone();
    let mut flusher = BatchFlusher::new(
        "test",
        Duration::from_secs(3600),
        1,
        |buffer, key, value| {
            buffer.insert(key, value);
        },
        move |buffer| {
            let mut attempts = captured.lock().unwrap();
            attempts.push(buffer.clone());
            if attempts.len() == 1 {
                Err(crate::Error::Storage("injected flush failure".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap();

    assert!(flusher.add("failed".into(), 1).is_ok());
    assert!(flusher.is_empty());
    assert_eq!(flusher.failed_flushes, 1);

    assert!(flusher.add("succeeded".into(), 2).is_ok());
    assert_eq!(flusher.successful_flushes, 1);
    let attempts = attempts.lock().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].get("failed"), Some(&1));
    assert_eq!(attempts[1].get("succeeded"), Some(&2));
    assert!(!attempts[1].contains_key("failed"));
}
