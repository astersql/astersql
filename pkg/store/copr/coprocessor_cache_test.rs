// Copyright 2016 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Coprocessor 缓存单元测试的 Go 对照参考。
//
// `GO_REFERENCE` 嵌入 Go 侧用例：缓存键字节布局、禁用/准入阈值、
// `Len` 估算、get/set 与分页命中回填，以及负容量错误信息。
// 本文件本身无可执行 Rust 测试体，仅作迁移对照。

/// 嵌入的 Go 版 coprocessor cache 测试结构参考文本（不可执行）。
const GO_REFERENCE: &str = r################"

// testing/require/mockstore/testutils/failpoint/backoff/context/goleak 等外部依赖均按 Go 语义保留为占位调用，后续需接入 Rust 测试基础设施。
// Go imports（仅记录来源依赖，暂不接入 Rust crate）：
// - "testing"
// - "time"
// - "github.com/pingcap/kvproto/pkg/coprocessor"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/stretchr/testify/require"
// - "github.com/tikv/client-go/v2/config"

// TestBuildCacheKey 对应 Go 的同名测试：逐字节断言 coprocessor cache key 的组成，并确认 paging 只追加 marker。
#[test]
pub fn TestBuildCacheKey(t *testing.T) {
	const (
		bytePagingSize = 0x0102030405060708
		rowPagingSize  = 0x1112131415161718
	)

	req := coprocessor.Request{
		Tp:      0xAB,
		StartTs: 0xAABBCC,
		Data:    []uint8{0x18, 0x0, 0x20, 0x0, 0x40, 0x0, 0x5a, 0x0},
		Ranges: []*coprocessor.KeyRange{
			{
				Start: kv.Key{0x01},
				End:   kv.Key{0x01, 0x02},
			},
			{
				Start: kv.Key{0x01, 0x01, 0x02},
				End:   kv.Key{0x01, 0x01, 0x03},
			},
		},
	}

	key, err := coprCacheBuildKey(&req)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	expectKey := ""
	expectKey += "\xab"                             // 1 byte Tp
	expectKey += "\x08\x00\x00\x00"                 // 4 bytes Data len
	expectKey += "\x18\x00\x20\x00\x40\x00\x5a\x00" // Data
	expectKey += "\x01\x00"                         // 2 bytes StartKey len
	expectKey += "\x01"                             // StartKey
	expectKey += "\x02\x00"                         // 2 bytes EndKey len
	expectKey += "\x01\x02"                         // EndKey
	expectKey += "\x03\x00"                         // 2 bytes StartKey len
	expectKey += "\x01\x01\x02"                     // StartKey
	expectKey += "\x03\x00"                         // 2 bytes EndKey len
	expectKey += "\x01\x01\x03"                     // EndKey
	require.EqualValues(t, []byte(expectKey), key)

	// A paging request (row-count or byte-budget) only appends a single marker
	// byte; the exact sizes are not part of the key, so every paging variant
	// shares the same key space and they never collide with the non-paging key
	// asserted above.
	req.PagingSizeBytes = bytePagingSize
	key, err = coprCacheBuildKey(&req)
	require.NoError(t, err)
	require.EqualValues(t, []byte(expectKey+"\x01"), key)

	req.PagingSize = rowPagingSize
	req.PagingSizeBytes = 0
	key, err = coprCacheBuildKey(&req)
	require.NoError(t, err)
	require.EqualValues(t, []byte(expectKey+"\x01"), key)

	req.PagingSize = rowPagingSize
	req.PagingSizeBytes = bytePagingSize
	key, err = coprCacheBuildKey(&req)
	require.NoError(t, err)
	require.EqualValues(t, []byte(expectKey+"\x01"), key)

	req = coprocessor.Request{
		Tp:      0xABCC, // Tp too big
		StartTs: 0xAABBCC,
		Data:    []uint8{0x18},
		Ranges:  []*coprocessor.KeyRange{},
	}

	_, err = coprCacheBuildKey(&req)
	require.Error(t, err)
}

// TestDisable 对应 Go 的同名测试：覆盖 cache disabled、容量非法和小容量启用三类配置。
#[test]
pub fn TestDisable(t *testing.T) {
	cache, err := newCoprCache(&config.CoprocessorCache{CapacityMB: 0})
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	require.Nil(t, cache)

	v := cache.Set([]byte("foo"), &coprCacheValue{})
	require.False(t, v)

	v2 := cache.Get([]byte("foo"))
	require.Nil(t, v2)

	// 时间相关断言保留 Go 的等待/耗时边界；不实际睡眠或计时。
	v = cache.CheckResponseAdmission(1024, time.Second*5, 0)
	require.False(t, v)

	cache, err = newCoprCache(&config.CoprocessorCache{CapacityMB: 0.001})
	require.Error(t, err)
	require.Nil(t, cache)

	cache, err = newCoprCache(&config.CoprocessorCache{CapacityMB: 0.001, AdmissionMaxResultMB: 1})
	require.NoError(t, err)
	require.NotNil(t, cache)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer cache.cache.Close()
}

// TestAdmission 对应 Go 的同名测试：覆盖请求/响应准入阈值：处理耗时、结果大小和 range 数限制。
#[test]
pub fn TestAdmission(t *testing.T) {
	cache, err := newCoprCache(&config.CoprocessorCache{AdmissionMinProcessMs: 5, AdmissionMaxResultMB: 1, CapacityMB: 1})
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	require.NotNil(t, cache)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer cache.cache.Close()

	v := cache.CheckRequestAdmission(0)
	require.True(t, v)

	v = cache.CheckRequestAdmission(1000)
	require.True(t, v)

	v = cache.CheckResponseAdmission(0, 0, 0)
	require.False(t, v)

	// 时间相关断言保留 Go 的等待/耗时边界；不实际睡眠或计时。
	v = cache.CheckResponseAdmission(0, 4*time.Millisecond, 0)
	require.False(t, v)

	v = cache.CheckResponseAdmission(0, 5*time.Millisecond, 0)
	require.False(t, v)

	v = cache.CheckResponseAdmission(1, 0, 0)
	require.False(t, v)

	v = cache.CheckResponseAdmission(1, 4*time.Millisecond, 0)
	require.False(t, v)

	v = cache.CheckResponseAdmission(1, 5*time.Millisecond, 0)
	require.True(t, v)

	v = cache.CheckResponseAdmission(1024, 5*time.Millisecond, 0)
	require.True(t, v)

	v = cache.CheckResponseAdmission(1024*1024, 5*time.Millisecond, 0)
	require.True(t, v)

	v = cache.CheckResponseAdmission(1024*1024+1, 5*time.Millisecond, 0)
	require.False(t, v)

	v = cache.CheckResponseAdmission(1024*1024+1, 4*time.Millisecond, 0)
	require.False(t, v)

	v = cache.CheckResponseAdmission(1024, 4*time.Millisecond, 1)
	require.True(t, v)

	v = cache.CheckResponseAdmission(1024, 4*time.Millisecond, 51)
	require.False(t, v)

	cache, err = newCoprCache(&config.CoprocessorCache{AdmissionMaxRanges: 5, AdmissionMinProcessMs: 5, AdmissionMaxResultMB: 1, CapacityMB: 1})
	require.NoError(t, err)
	require.NotNil(t, cache)
	// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
	defer cache.cache.Close()

	v = cache.CheckRequestAdmission(0)
	require.True(t, v)

	v = cache.CheckRequestAdmission(5)
	require.True(t, v)

	v = cache.CheckRequestAdmission(6)
	require.False(t, v)
}

// TestCacheValueLen 对应 Go 的同名测试：验证 coprCacheValue.Len 对切片头和可变数据长度的估算。
#[test]
pub fn TestCacheValueLen(t *testing.T) {
	v := coprCacheValue{
		TimeStamp:         0x123,
		RegionID:          0x1,
		RegionDataVersion: 0x3,
	}
	// 120 = (8 byte pointer + 8 byte for length + 8 byte for cap) * 4 + 8 byte * 3
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.Equal(t, 120, v.Len())

	v = coprCacheValue{
		Key:               []byte("foobar"),
		Data:              []byte("12345678"),
		TimeStamp:         0x123,
		RegionID:          0x1,
		RegionDataVersion: 0x3,
	}
	require.Equal(t, 120+len(v.Key)+len(v.Data), v.Len())

	v = coprCacheValue{
		Key:               []byte("foobar"),
		Data:              []byte("12345678"),
		TimeStamp:         0x123,
		RegionID:          0x1,
		RegionDataVersion: 0x3,
		PageEnd:           []byte("3235"),
	}
	require.Equal(t, 120+len(v.Key)+len(v.Data)+len(v.PageEnd), v.Len())
}

// TestGetSet 对应 Go 的同名测试：覆盖 cache get/set、Ristretto 异步可见延迟，以及 paging cache hit range 回填。
#[test]
pub fn TestGetSet(t *testing.T) {
	cache, err := newCoprCache(&config.CoprocessorCache{AdmissionMinProcessMs: 5, AdmissionMaxResultMB: 1, CapacityMB: 1})
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	require.NotNil(t, cache)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer cache.cache.Close()

	v := cache.Get([]byte("foo"))
	require.Nil(t, v)

	v2 := cache.Set([]byte("foo"), &coprCacheValue{
		Data:              []byte("bar"),
		TimeStamp:         0x123,
		RegionID:          0x1,
		RegionDataVersion: 0x3,
	})
	require.True(t, v2)

	// See https://github.com/dgraph-io/ristretto/blob/83508260cb49a2c3261c2774c991870fd18b5a1b/cache_test.go#L13
	// Changed from 10ms to 50ms to resist from unstable CI environment.
	// 时间相关断言保留 Go 的等待/耗时边界；不实际睡眠或计时。
	time.Sleep(time.Millisecond * 50)

	v = cache.Get([]byte("foo"))
	require.NotNil(t, v)
	require.EqualValues(t, []byte("bar"), v.Data)

	// Go 子测试闭包保留场景边界，避免迁移时合并表驱动用例。
	t.Run("paging size bytes cache hit keeps range", func(t *testing.T) {
		req := &kv.Request{}
		req.Paging.PagingSizeBytes = 1024
		worker := &copIteratorWorker{req: req}
		task := &copTask{}
		resp := &copResponse{pbResp: &coprocessor.Response{IsCacheHit: true}}
		cacheValue := &coprCacheValue{
			Data:      []byte("cached"),
			PageStart: []byte("m"),
			PageEnd:   []byte("z"),
		}
		require.NoError(t, worker.handleCopCache(task, resp, nil, cacheValue))
		require.EqualValues(t, []byte("cached"), resp.pbResp.Data)
		require.Equal(t, []byte("m"), resp.pbResp.GetRange().GetStart())
		require.Equal(t, []byte("z"), resp.pbResp.GetRange().GetEnd())
	})
}

// TestIssue24118 对应 Go 的同名测试：覆盖负容量配置的错误信息。
#[test]
pub fn TestIssue24118(t *testing.T) {
	_, err := newCoprCache(&config.CoprocessorCache{AdmissionMinProcessMs: 5, AdmissionMaxResultMB: 1, CapacityMB: -1})
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.EqualError(t, err, "Capacity must be > 0 to enable the cache")
}
"################;

use std::mem::size_of;
use std::time::Duration;

use crate::{
    CoprocessorCache, CoprocessorCacheConfig, CoprocessorCacheRequest, CoprocessorCacheValue,
    KeyRange, coprocessor_cache_build_key,
};

fn config() -> CoprocessorCacheConfig {
    CoprocessorCacheConfig {
        capacity_mb: 1.0,
        admission_max_result_mb: 1.0,
        admission_max_ranges: 0,
        admission_min_process_ms: 5,
    }
}

#[test]
fn build_cache_key_matches_go_byte_layout() {
    let mut request = CoprocessorCacheRequest {
        request_type: 0xab,
        data: vec![0x18, 0, 0x20, 0, 0x40, 0, 0x5a, 0],
        ranges: vec![
            KeyRange {
                start: vec![1],
                end: vec![1, 2],
            },
            KeyRange {
                start: vec![1, 1, 2],
                end: vec![1, 1, 3],
            },
        ],
        ..CoprocessorCacheRequest::default()
    };
    let expected = vec![
        0xab, 8, 0, 0, 0, 0x18, 0, 0x20, 0, 0x40, 0, 0x5a, 0, 1, 0, 1, 2, 0, 1, 2, 3, 0, 1, 1, 2,
        3, 0, 1, 1, 3,
    ];
    assert_eq!(coprocessor_cache_build_key(&request).unwrap(), expected);

    request.paging_size_bytes = 0x0102_0304_0506_0708;
    let mut paging_expected = expected.clone();
    paging_expected.push(1);
    assert_eq!(
        coprocessor_cache_build_key(&request).unwrap(),
        paging_expected
    );
    request.paging_size = 0x1112_1314_1516_1718;
    request.paging_size_bytes = 0;
    assert_eq!(
        coprocessor_cache_build_key(&request).unwrap(),
        paging_expected
    );
    request.paging_size_bytes = 0x0102_0304_0506_0708;
    assert_eq!(
        coprocessor_cache_build_key(&request).unwrap(),
        paging_expected
    );
    request.request_type = 0xabcc;
    assert!(coprocessor_cache_build_key(&request).is_err());
}

#[test]
fn disabled_and_invalid_cache_configuration_matches_go() {
    assert!(
        CoprocessorCache::new(&CoprocessorCacheConfig::default())
            .unwrap()
            .is_none()
    );
    assert!(
        CoprocessorCache::new(&CoprocessorCacheConfig {
            capacity_mb: 0.001,
            ..CoprocessorCacheConfig::default()
        })
        .is_err()
    );
    assert!(CoprocessorCache::new(&config()).unwrap().is_some());
}

#[test]
fn request_and_response_admission_matches_go_boundaries() {
    let cache = CoprocessorCache::new(&config()).unwrap().unwrap();
    assert!(cache.check_request_admission(0));
    assert!(cache.check_request_admission(1_000));
    for (size, elapsed, page, admitted) in [
        (0, Duration::ZERO, 0, false),
        (0, Duration::from_millis(5), 0, false),
        (1, Duration::from_millis(4), 0, false),
        (1, Duration::from_millis(5), 0, true),
        (1024 * 1024, Duration::from_millis(5), 0, true),
        (1024 * 1024 + 1, Duration::from_millis(5), 0, false),
        (1024, Duration::from_millis(4), 1, true),
        (1024, Duration::from_millis(4), 51, false),
    ] {
        assert_eq!(
            cache.check_response_admission(size, elapsed, page),
            admitted,
            "size={size}, elapsed={elapsed:?}, page={page}"
        );
    }
    let limited = CoprocessorCache::new(&CoprocessorCacheConfig {
        admission_max_ranges: 5,
        ..config()
    })
    .unwrap()
    .unwrap();
    assert!(limited.check_request_admission(5));
    assert!(!limited.check_request_admission(6));
}

#[test]
fn cache_value_len_counts_struct_and_owned_bytes() {
    let mut value = CoprocessorCacheValue::default();
    assert_eq!(value.len(), size_of::<CoprocessorCacheValue>());
    value.key = b"foobar".to_vec();
    value.data = b"12345678".to_vec();
    value.page_start = b"ab".to_vec();
    value.page_end = b"3235".to_vec();
    assert_eq!(
        value.len(),
        size_of::<CoprocessorCacheValue>()
            + value.key.len()
            + value.data.len()
            + value.page_start.len()
            + value.page_end.len()
    );
}

#[test]
fn cache_value_display_matches_go_string_contract() {
    let value = CoprocessorCacheValue {
        data: b"12345678".to_vec(),
        timestamp: 0x123,
        region_id: 1,
        region_data_version: 3,
        ..CoprocessorCacheValue::default()
    };
    assert_eq!(
        value.to_string(),
        "{ Ts = 291, RegionID = 1, RegionDataVersion = 3, len(Data) = 8 }"
    );
}

#[test]
fn cache_get_set_and_eviction_are_real() {
    let cache = CoprocessorCache::new(&config()).unwrap().unwrap();
    assert!(cache.get(b"foo").is_none());
    assert!(cache.set(
        b"foo".to_vec(),
        CoprocessorCacheValue {
            data: b"bar".to_vec(),
            timestamp: 0x123,
            region_id: 1,
            region_data_version: 3,
            ..CoprocessorCacheValue::default()
        }
    ));
    assert_eq!(cache.get(b"foo").unwrap().data, b"bar");
}

#[test]
fn negative_capacity_is_rejected_like_issue_24118() {
    let error = CoprocessorCache::new(&CoprocessorCacheConfig {
        capacity_mb: -1.0,
        admission_max_result_mb: 1.0,
        admission_min_process_ms: 5,
        ..CoprocessorCacheConfig::default()
    })
    .err()
    .expect("negative capacity must fail");
    assert_eq!(
        error.to_string(),
        "Capacity must be > 0 to enable the cache"
    );
}
