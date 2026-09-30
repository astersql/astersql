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

// Coprocessor 任务构建与切分相关测试的 Go 对照参考。
//
// `GO_REFERENCE` 覆盖：KeyRange 单调排序、无/有 bucket（Region 内子分片）时的
// 任务切分、按 Location 拆分、rebuild、paging（分页）、retry/remain 计算、
// small-task 并发与 store batch 等。Region 为键空间分片。本文件无可执行 Rust 测试体。

/// 嵌入的 Go 版 coprocessor 测试结构参考文本（不可执行）。
const GO_REFERENCE: &str = r################"

// testing/require/mockstore/testutils/failpoint/backoff/context/goleak 等外部依赖均按 Go 语义保留为占位调用，后续需接入 Rust 测试基础设施。
// Go imports（仅记录来源依赖，暂不接入 Rust crate）：
// - "context"
// - "sync/atomic"
// - "testing"
// - "time"
// - "github.com/pingcap/kvproto/pkg/coprocessor"
// - "github.com/pingcap/kvproto/pkg/errorpb"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/pingcap/tidb/pkg/store/driver/backoff"
// - "github.com/pingcap/tidb/pkg/util/paging"
// - "github.com/pingcap/tidb/pkg/util/trxevents"
// - "github.com/stretchr/testify/require"
// - "github.com/tikv/client-go/v2/testutils"
// - "github.com/tikv/client-go/v2/tikv"

// 封装 buildCopTasks 的测试入口，固定 respChan=true 并透传 req/cache/event callback。
pub fn buildTestCopTasks(bo *Backoffer, cache *RegionCache, ranges *KeyRanges, req *kv.Request, eventCb trxevents.EventCallback) ([]*copTask, error) {
	return buildCopTasks(bo, ranges, &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		eventCb:  eventCb,
		respChan: true,
	})
}

// TestEnsureMonotonicKeyRanges 对应 Go 的同名测试：验证乱序 key range 会被重新排序，已排序输入不会改变。
#[test]
pub fn TestEnsureMonotonicKeyRanges(t *testing.T) {
	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	ctx := context.Background()
	ranges := NewKeyRanges([]kv.KeyRange{
		{StartKey: []byte("b"), EndKey: []byte("d")},
		{StartKey: []byte("a"), EndKey: []byte("b")},
	})
	reordered := ensureMonotonicKeyRanges(ctx, ranges)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.True(t, reordered)
	require.Equal(t, "a", string(ranges.At(0).StartKey))
	require.Equal(t, "b", string(ranges.At(0).EndKey))
	require.Equal(t, "b", string(ranges.At(1).StartKey))

	sortedRanges := NewKeyRanges([]kv.KeyRange{
		{StartKey: []byte("a"), EndKey: []byte("b")},
		{StartKey: []byte("b"), EndKey: []byte("c")},
	})
	reordered = ensureMonotonicKeyRanges(ctx, sortedRanges)
	require.False(t, reordered)
}

// TestBuildTasksWithoutBuckets 对应 Go 的同名测试：在无 bucket 元数据时验证 range 到 region 的任务切分。
#[test]
pub fn TestBuildTasksWithoutBuckets(t *testing.T) {
	// nil --- 'g' --- 'n' --- 't' --- nil
	// <- 0 -> <- 1 -> <- 2 -> <- 3 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	_, regionIDs, _ := testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	req := &kv.Request{}
	flashReq := &kv.Request{}
	flashReq.StoreType = kv.TiFlash
	tasks, err := buildTestCopTasks(bo, cache, buildCopRanges("a", "c"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "c")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "c"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "c")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("g", "n"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[1], 0, "g", "n")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("g", "n"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[1], 0, "g", "n")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("m", "n"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[1], 0, "m", "n")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("m", "n"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[1], 0, "m", "n")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "k"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 2)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "g")
	taskEqual(t, tasks[1], regionIDs[1], 0, "g", "k")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "k"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 2)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "g")
	taskEqual(t, tasks[1], regionIDs[1], 0, "g", "k")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "x"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 4)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "g")
	taskEqual(t, tasks[1], regionIDs[1], 0, "g", "n")
	taskEqual(t, tasks[2], regionIDs[2], 0, "n", "t")
	taskEqual(t, tasks[3], regionIDs[3], 0, "t", "x")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "x"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 4)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "g")
	taskEqual(t, tasks[1], regionIDs[1], 0, "g", "n")
	taskEqual(t, tasks[2], regionIDs[2], 0, "n", "t")
	taskEqual(t, tasks[3], regionIDs[3], 0, "t", "x")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "b", "b", "c"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "b", "b", "c")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "b", "b", "c"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "b", "b", "c")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "b", "e", "f"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "b", "e", "f")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "b", "e", "f"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "b", "e", "f")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("g", "n", "o", "p"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 2)
	taskEqual(t, tasks[0], regionIDs[1], 0, "g", "n")
	taskEqual(t, tasks[1], regionIDs[2], 0, "o", "p")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("g", "n", "o", "p"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 2)
	taskEqual(t, tasks[0], regionIDs[1], 0, "g", "n")
	taskEqual(t, tasks[1], regionIDs[2], 0, "o", "p")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("h", "k", "m", "p"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 2)
	taskEqual(t, tasks[0], regionIDs[1], 0, "h", "k", "m", "n")
	taskEqual(t, tasks[1], regionIDs[2], 0, "n", "p")

	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("h", "k", "m", "p"), flashReq, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 2)
	taskEqual(t, tasks[0], regionIDs[1], 0, "h", "k", "m", "n")
	taskEqual(t, tasks[1], regionIDs[2], 0, "n", "p")
}

// TestBuildTasksByBuckets 对应 Go 的同名测试：在 bucket 元数据存在时验证单 bucket、多 bucket、跨 bucket range 的任务切分。
#[test]
pub fn TestBuildTasksByBuckets(t *testing.T) {
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	// region: nil------------------n-----------x-----------nil
	// buckets: nil----c----g----k---n----t------x-----------nil
	_, regionIDs, _ := testutils.BootstrapWithMultiRegions(cluster, []byte("n"), []byte("x"))
	cluster.SplitRegionBuckets(regionIDs[0], [][]byte{{}, {'c'}, {'g'}, {'k'}, {'n'}}, regionIDs[0])
	cluster.SplitRegionBuckets(regionIDs[1], [][]byte{{'n'}, {'t'}, {'x'}}, regionIDs[1])
	cluster.SplitRegionBuckets(regionIDs[2], [][]byte{{'x'}, {}}, regionIDs[2])
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	// one range per bucket
	// region: nil------------------n-----------x-----------nil
	// buckets: nil----c----g----k---n----t------x-----------nil
	// range&task: a-b c-d h-i k---n o-p u--x-----------nil
	req := &kv.Request{}
	regionRanges := []struct {
		regionID uint64
		ranges   []string
	}{
		{regionIDs[0], []string{"a", "b", "c", "d", "h", "i", "k", "n"}},
		{regionIDs[1], []string{"o", "p", "u", "x"}},
		{regionIDs[2], []string{"x", ""}},
	}
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for _, regionRange := range regionRanges {
		regionID, ranges := regionRange.regionID, regionRange.ranges
		tasks, err := buildTestCopTasks(bo, cache, buildCopRanges(ranges...), req, nil)
		require.NoError(t, err)
		require.Len(t, tasks, len(ranges)/2)
		for i, task := range tasks {
			taskEqual(t, task, regionID, regionID, ranges[2*i], ranges[2*i+1])
		}
	}

	// one request multiple regions
	allRanges := []string{}
	for _, regionRange := range regionRanges {
		allRanges = append(allRanges, regionRange.ranges...)
	}
	tasks, err := buildTestCopTasks(bo, cache, buildCopRanges(allRanges...), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, len(allRanges)/2)
	taskIdx := 0
	for _, regionRange := range regionRanges {
		regionID, ranges := regionRange.regionID, regionRange.ranges
		for i := 0; i < len(ranges); i += 2 {
			taskEqual(t, tasks[taskIdx], regionID, regionID, ranges[i], ranges[i+1])
			taskIdx++
		}
	}

	// several ranges per bucket
	// region: nil---------------------------n-----------x-----------nil
	// buckets: nil-----c-------g-------k-----n----t------x-----------nil
	// ranges: nil-a b-c d-e f-g h-i j-k-l m-n
	// tasks: nil-a b-c
	//                    d-e f-g
	//                            h-i j-k
	//                                  k-l m-n
	keyRanges := []string{
		"", "a", "b", "c",
		"d", "e", "f", "g",
		"h", "i", "j", "k",
		"k", "l", "m", "n",
	}
	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges(keyRanges...), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, len(keyRanges)/4)
	for i, task := range tasks {
		taskEqual(t, task, regionIDs[0], regionIDs[0], keyRanges[4*i], keyRanges[4*i+1], keyRanges[4*i+2], keyRanges[4*i+3])
	}

	// cross bucket ranges
	// buckets: nil-----c-------g---------k---n----t------x-----------nil
	// ranges: nil-------d e---h i---j
	// tasks: nil-----c
	// c-d e-g
	//                          g-h i---j
	keyRanges = []string{
		"", "d", "e", "h", "i", "j",
	}
	expectedTaskRanges := [][]string{
		{"", "c"},
		{"c", "d", "e", "g"},
		{"g", "h", "i", "j"},
	}
	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges(keyRanges...), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, len(expectedTaskRanges))
	for i, task := range tasks {
		taskEqual(t, task, regionIDs[0], regionIDs[0], expectedTaskRanges[i]...)
	}

	// cross several buckets ranges
	// region: n ----------------------------- x
	// buckets: n -- q -- r -- t -- u -- v -- x
	// ranges: n--o p--q s ------------ w
	// tasks: n--o p--q
	//                             s--t
	//								  t -- u
	//									   u -- v
	//											v--w
	expectedTaskRanges = [][]string{
		{"n", "o", "p", "q"},
		{"s", "t"},
		{"t", "u"},
		{"u", "v"},
		{"v", "w"},
	}
	cluster.SplitRegionBuckets(regionIDs[1], [][]byte{{'n'}, {'q'}, {'r'}, {'t'}, {'u'}, {'v'}, {'x'}}, regionIDs[1])
	cache = NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()
	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("n", "o", "p", "q", "s", "w"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, len(expectedTaskRanges))
	for i, task := range tasks {
		taskEqual(t, task, regionIDs[1], regionIDs[1], expectedTaskRanges[i]...)
	}

	// out of range buckets
	// region: n------------------x
	// buckets: q---s---u
	// ranges: n-o p ----s t---v w-x
	// tasks: n-o p-q
	//                 q--s
	//                      t-u
	//                        u-v w-x
	expectedTaskRanges = [][]string{
		{"n", "o", "p", "q"},
		{"q", "s"},
		{"t", "u"},
		{"u", "v", "w", "x"},
	}
	cluster.SplitRegionBuckets(regionIDs[1], [][]byte{{'q'}, {'s'}, {'u'}}, regionIDs[1])
	cache = NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()
	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("n", "o", "p", "s", "t", "v", "w", "x"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, len(expectedTaskRanges))
	for i, task := range tasks {
		taskEqual(t, task, regionIDs[1], regionIDs[1], expectedTaskRanges[i]...)
	}

	// out of range buckets
	// region: n------------x
	// buckets: g-------t---------z
	// ranges: o-p u-w
	// tasks: o-p
	//                   u-w
	expectedTaskRanges = [][]string{
		{"o", "p"},
		{"u", "w"},
	}
	cluster.SplitRegionBuckets(regionIDs[1], [][]byte{{'g'}, {'t'}, {'z'}}, regionIDs[1])
	cache = NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()
	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("o", "p", "u", "w"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, len(expectedTaskRanges))
	for i, task := range tasks {
		taskEqual(t, task, regionIDs[1], regionIDs[1], expectedTaskRanges[i]...)
	}

	// cover the whole region
	// region: n--------------x
	// buckets: n -- q -- r -- x
	// ranges: n--------------x
	// tasks: o -- q
	//                 q -- r
	//						r -- x
	expectedTaskRanges = [][]string{
		{"n", "q"},
		{"q", "r"},
		{"r", "x"},
	}
	cluster.SplitRegionBuckets(regionIDs[1], [][]byte{{'n'}, {'q'}, {'r'}, {'x'}}, regionIDs[1])
	cache = NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()
	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("n", "x"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, len(expectedTaskRanges))
	for i, task := range tasks {
		taskEqual(t, task, regionIDs[1], regionIDs[1], expectedTaskRanges[i]...)
	}
}

// TestSplitKeyRangesByLocationsWithoutBuckets 对应 Go 的同名测试：验证没有 bucket 时 SplitKeyRangesByLocations 的 region 边界切分和 limit。
#[test]
pub fn TestSplitKeyRangesByLocationsWithoutBuckets(t *testing.T) {
	// nil --- 'g' --- 'n' --- 't' --- nil
	// <- 0 -> <- 1 -> <- 2 -> <- 3 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	locRanges, err := cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("a", "c")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 1)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "a", "c")

	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("a", "c")), 0, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 0)

	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("h", "y")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 3)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "h", "n")
	rangeEqual(t, locRanges[1].Ranges.ToRanges(), "n", "t")
	rangeEqual(t, locRanges[2].Ranges.ToRanges(), "t", "y")

	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("h", "n")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 1)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "h", "n")

	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("s", "s")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 1)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "s", "s")

	// min --> max
	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("a", "z")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 4)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "a", "g")
	rangeEqual(t, locRanges[1].Ranges.ToRanges(), "g", "n")
	rangeEqual(t, locRanges[2].Ranges.ToRanges(), "n", "t")
	rangeEqual(t, locRanges[3].Ranges.ToRanges(), "t", "z")

	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("a", "z")), 3, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 3)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "a", "g")
	rangeEqual(t, locRanges[1].Ranges.ToRanges(), "g", "n")
	rangeEqual(t, locRanges[2].Ranges.ToRanges(), "n", "t")

	// many range
	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 4)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "a", "b", "c", "d", "e", "f", "f", "g")
	rangeEqual(t, locRanges[1].Ranges.ToRanges(), "g", "h", "i", "j", "k", "l", "m", "n")
	rangeEqual(t, locRanges[2].Ranges.ToRanges(), "o", "p", "q", "r", "s", "t")
	rangeEqual(t, locRanges[3].Ranges.ToRanges(), "u", "v", "w", "x", "y", "z")

	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("a", "b", "b", "h", "h", "m", "n", "t", "v", "w")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 4)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "a", "b", "b", "g")
	rangeEqual(t, locRanges[1].Ranges.ToRanges(), "g", "h", "h", "m", "n")
	rangeEqual(t, locRanges[2].Ranges.ToRanges(), "n", "t")
	rangeEqual(t, locRanges[3].Ranges.ToRanges(), "v", "w")

	locRanges, err = cache.SplitKeyRangesByLocations(bo, NewKeyRanges(BuildKeyRanges("a", "b", "v", "w")), UnspecifiedLimit, false, false)
	require.NoError(t, err)
	require.Len(t, locRanges, 2)
	rangeEqual(t, locRanges[0].Ranges.ToRanges(), "a", "b")
	rangeEqual(t, locRanges[1].Ranges.ToRanges(), "v", "w")
}

// TestSplitKeyRanges 对应 Go 的同名测试：验证 SplitKeyRanges 在 region 边界、空 range 和多 range 输入下的结果。
#[test]
pub fn TestSplitKeyRanges(t *testing.T) {
	// nil --- 'g' --- 'n' --- 't' --- nil
	// <- 0 -> <- 1 -> <- 2 -> <- 3 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	ranges, err := cache.SplitRegionRanges(bo, BuildKeyRanges("a", "c"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 1)
	rangeEqual(t, ranges, "a", "c")

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("a", "c"), 0)
	require.NoError(t, err)
	require.Len(t, ranges, 0)

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("h", "y"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 3)
	rangeEqual(t, ranges, "h", "n", "n", "t", "t", "y")

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("s", "z"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 2)
	rangeEqual(t, ranges, "s", "t", "t", "z")

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("s", "s"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 1)
	rangeEqual(t, ranges, "s", "s")

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("t", "t"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 1)
	rangeEqual(t, ranges, "t", "t")

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("t", "u"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 1)
	rangeEqual(t, ranges, "t", "u")

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("u", "z"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 1)
	rangeEqual(t, ranges, "u", "z")

	// min --> max
	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("a", "z"), UnspecifiedLimit)
	require.NoError(t, err)
	require.Len(t, ranges, 4)
	rangeEqual(t, ranges, "a", "g", "g", "n", "n", "t", "t", "z")

	ranges, err = cache.SplitRegionRanges(bo, BuildKeyRanges("a", "z"), 3)
	require.NoError(t, err)
	require.Len(t, ranges, 3)
	rangeEqual(t, ranges, "a", "g", "g", "n", "n", "t")
}

// TestRebuild 对应 Go 的同名测试：验证 region epoch 变化后 rebuild cop task 的 region 与 peer 信息刷新。
#[test]
pub fn TestRebuild(t *testing.T) {
	// nil --- 'm' --- nil
	// <- 0 -> <- 1 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	storeID, regionIDs, peerIDs := testutils.BootstrapWithMultiRegions(cluster, []byte("m"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()
	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()
	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	req := &kv.Request{}
	tasks, err := buildTestCopTasks(bo, cache, buildCopRanges("a", "z"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 2)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "m")
	taskEqual(t, tasks[1], regionIDs[1], 0, "m", "z")

	// nil -- 'm' -- 'q' -- nil
	// <- 0 -> <--1-> <-2-->
	regionIDs = append(regionIDs, cluster.AllocID())
	peerIDs = append(peerIDs, cluster.AllocID())
	cluster.Split(regionIDs[1], regionIDs[2], []byte("q"), []uint64{peerIDs[2]}, storeID)
	cache.InvalidateCachedRegion(tasks[1].region)

	req.Desc = true
	tasks, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "z"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 3)
	taskEqual(t, tasks[2], regionIDs[0], 0, "a", "m")
	taskEqual(t, tasks[1], regionIDs[1], 0, "m", "q")
	taskEqual(t, tasks[0], regionIDs[2], 0, "q", "z")
}

// 把成对字符串转换为 KeyRanges，保留 Go 测试里 compact range fixture 写法。
pub fn buildCopRanges(keys ...string) *KeyRanges {
	return NewKeyRanges(BuildKeyRanges(keys...))
}

// 断言 copTask 的 region、bucket version 和 range 列表，与 Go helper 的检查顺序一致。
pub fn taskEqual(t *testing.T, task *copTask, regionID, bucketsVer uint64, keys ...string) {
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.Equal(t, task.region.GetID(), regionID)
	require.Equal(t, task.bucketsVer, bucketsVer)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for i := range task.ranges.Len() {
		r := task.ranges.At(i)
		require.Equal(t, string(r.StartKey), keys[2*i])
		require.Equal(t, string(r.EndKey), keys[2*i+1])
	}
}

// 断言 kv.KeyRange 列表与字符串边界一一对应。
pub fn rangeEqual(t *testing.T, ranges []kv.KeyRange, keys ...string) {
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for i := range ranges {
		r := ranges[i]
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.Equal(t, string(r.StartKey), keys[2*i])
		require.Equal(t, string(r.EndKey), keys[2*i+1])
	}
}

// TestBuildPagingTasks 对应 Go 的同名测试：验证 paging 请求会构造带 paging 标记和最小 page size 的任务。
#[test]
pub fn TestBuildPagingTasks(t *testing.T) {
	// nil --- 'g' --- 'n' --- 't' --- nil
	// <- 0 -> <- 1 -> <- 2 -> <- 3 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	_, regionIDs, _ := testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	req := &kv.Request{}
	req.Paging.Enable = true
	req.Paging.MinPagingSize = paging.MinPagingSize
	tasks, err := buildTestCopTasks(bo, cache, buildCopRanges("a", "c"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "c")
	require.True(t, tasks[0].paging)
	require.Equal(t, tasks[0].pagingSize, paging.MinPagingSize)
}

// TestBuildCopTaskDoesNotCancelBoCtx 对应 Go 的同名测试：确保 buildCopTasks 内部派生 context 不会取消传入 backoffer context。
#[test]
pub fn TestBuildCopTaskDoesNotCancelBoCtx(t *testing.T) {
	// nil --- 'g' --- 'n' --- 't' --- nil
	// <- 0 -> <- 1 -> <- 2 -> <- 3 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	req := &kv.Request{}
	req.Paging.Enable = true
	req.Paging.MinPagingSize = paging.MinPagingSize
	req.MaxExecutionTime = 10000
	_, err = buildTestCopTasks(bo, cache, buildCopRanges("a", "c"), req, nil)
	require.NoError(t, err)
	contextDone := false
	select {
	case <-bo.GetCtx().Done():
		contextDone = true
	default:
	}
	require.False(t, contextDone, "buildCopTasks should not cancel bo context")
}

// TestBuildPagingTasksDisablePagingForSmallLimit 对应 Go 的同名测试：验证小 limit 场景禁用 paging 并清空 paging size。
#[test]
pub fn TestBuildPagingTasksDisablePagingForSmallLimit(t *testing.T) {
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()
	_, regionIDs, _ := testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))

	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	req := &kv.Request{}
	req.Paging.Enable = true
	req.Paging.MinPagingSize = paging.MinPagingSize
	req.LimitSize = 1
	tasks, err := buildTestCopTasks(bo, cache, buildCopRanges("a", "c"), req, nil)
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "c")
	require.False(t, tasks[0].paging)
	require.Equal(t, tasks[0].pagingSize, uint64(0))
}

// TestBuildCopTasksWithPagingSizeBytes 对应 Go 的同名测试：验证 byte-budget paging 不设置 row-count paging 标记，但仍扩大响应 channel。
#[test]
pub fn TestBuildCopTasksWithPagingSizeBytes(t *testing.T) {
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()
	_, regionIDs, _ := testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))

	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()
	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()
	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	// A byte budget lives on the request and is the single source of truth.
	req := &kv.Request{KeepOrder: true}
	req.Paging.PagingSizeBytes = uint64(4 * 1024 * 1024)
	tasks, err := buildCopTasks(bo, buildCopRanges("a", "c"), &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		respChan: true,
	})
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	taskEqual(t, tasks[0], regionIDs[0], 0, "a", "c")
	// A byte budget alone must not turn on row-count paging at the task level,
	// but it still enlarges the response channel like row-count paging does.
	require.False(t, tasks[0].paging)
	require.Equal(t, uint64(0), tasks[0].pagingSize)
	require.Equal(t, 18, cap(tasks[0].respChan))

	// Row-count paging with a tiny limit downgrades independently; the byte
	// budget on the request is untouched by that downgrade.
	req.Paging.Enable = true
	req.Paging.MinPagingSize = paging.MinPagingSize
	req.LimitSize = 1
	tasks, err = buildCopTasks(bo, buildCopRanges("a", "c"), &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		respChan: true,
	})
	require.NoError(t, err)
	require.Len(t, tasks, 1)
	require.False(t, tasks[0].paging)
	require.Equal(t, uint64(0), tasks[0].pagingSize)
	require.Equal(t, uint64(4*1024*1024), req.Paging.PagingSizeBytes)
	require.Equal(t, 18, cap(tasks[0].respChan))
}

// 把 kv.KeyRange 转为 coprocessor.KeyRange，保留 paging/retry 测试的辅助转换。
pub fn toCopRange(r kv.KeyRange) *coprocessor.KeyRange {
	coprRange := coprocessor.KeyRange{}
	coprRange.Start = r.StartKey
	coprRange.End = r.EndKey
	return &coprRange
}

// 展开 KeyRanges 为普通 kv.KeyRange 切片，用于 remain/retry 断言。
pub fn toRange(r *KeyRanges) []kv.KeyRange {
	ranges := make([]kv.KeyRange, 0, r.Len())
	// 条件分支保留 Go 的边界判断；Rust 接线时需确认 nil/空切片语义。
	if r.first != nil {
		ranges = append(ranges, *r.first)
	}
	ranges = append(ranges, r.mid...)
	if r.last != nil {
		ranges = append(ranges, *r.last)
	}
	return ranges
}

// TestCalculateRetry 对应 Go 的同名测试：覆盖 retry range 计算：已完成、不重叠、部分重叠和未完成 range。
#[test]
pub fn TestCalculateRetry(t *testing.T) {
	worker := copIteratorWorker{}

	// split in one range
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("b", "c")[0]
		retry := worker.calculateRetry(NewKeyRanges(ranges), toCopRange(split), false)
		rangeEqual(t, toRange(retry), "b", "c", "e", "g")
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("e", "f")[0]
		retry := worker.calculateRetry(NewKeyRanges(ranges), toCopRange(split), true)
		rangeEqual(t, toRange(retry), "a", "c", "e", "f")
	}

	// across ranges
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("b", "f")[0]
		retry := worker.calculateRetry(NewKeyRanges(ranges), toCopRange(split), false)
		rangeEqual(t, toRange(retry), "b", "c", "e", "g")
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("b", "f")[0]
		retry := worker.calculateRetry(NewKeyRanges(ranges), toCopRange(split), true)
		rangeEqual(t, toRange(retry), "a", "c", "e", "f")
	}

	// exhaust the ranges
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("a", "g")[0]
		retry := worker.calculateRetry(NewKeyRanges(ranges), toCopRange(split), false)
		rangeEqual(t, toRange(retry), "a", "c", "e", "g")
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("a", "g")[0]
		retry := worker.calculateRetry(NewKeyRanges(ranges), toCopRange(split), true)
		rangeEqual(t, toRange(retry), "a", "c", "e", "g")
	}

	// nil range
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		retry := worker.calculateRetry(NewKeyRanges(ranges), nil, false)
		rangeEqual(t, toRange(retry), "a", "c", "e", "g")
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		retry := worker.calculateRetry(NewKeyRanges(ranges), nil, true)
		rangeEqual(t, toRange(retry), "a", "c", "e", "g")
	}
}

// TestCalculateRemain 对应 Go 的同名测试：覆盖 remain range 计算，确保结果按原 KeyRanges 结构裁剪。
#[test]
pub fn TestCalculateRemain(t *testing.T) {
	worker := copIteratorWorker{}

	// split in one range
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("a", "b")[0]
		remain := worker.calculateRemain(NewKeyRanges(ranges), toCopRange(split), false)
		rangeEqual(t, toRange(remain), "b", "c", "e", "g")
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("f", "g")[0]
		remain := worker.calculateRemain(NewKeyRanges(ranges), toCopRange(split), true)
		rangeEqual(t, toRange(remain), "a", "c", "e", "f")
	}

	// across ranges
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("a", "f")[0]
		remain := worker.calculateRemain(NewKeyRanges(ranges), toCopRange(split), false)
		rangeEqual(t, toRange(remain), "f", "g")
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("b", "g")[0]
		remain := worker.calculateRemain(NewKeyRanges(ranges), toCopRange(split), true)
		rangeEqual(t, toRange(remain), "a", "b")
	}

	// exhaust the ranges
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("a", "g")[0]
		remain := worker.calculateRemain(NewKeyRanges(ranges), toCopRange(split), false)
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.Equal(t, remain.Len(), 0)
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		split := BuildKeyRanges("a", "g")[0]
		remain := worker.calculateRemain(NewKeyRanges(ranges), toCopRange(split), true)
		require.Equal(t, remain.Len(), 0)
	}

	// nil range
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		remain := worker.calculateRemain(NewKeyRanges(ranges), nil, false)
		rangeEqual(t, toRange(remain), "a", "c", "e", "g")
	}
	{
		ranges := BuildKeyRanges("a", "c", "e", "g")
		remain := worker.calculateRemain(NewKeyRanges(ranges), nil, true)
		rangeEqual(t, toRange(remain), "a", "c", "e", "g")
	}
}

// TestBasicSmallTaskConc 对应 Go 的同名测试：验证 small task 判定边界以及 per-core 并发下限。
#[test]
pub fn TestBasicSmallTaskConc(t *testing.T) {
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.False(t, isSmallTask(&copTask{RowCountHint: -1}))
	require.False(t, isSmallTask(&copTask{RowCountHint: 0}))
	require.True(t, isSmallTask(&copTask{RowCountHint: 1}))
	require.True(t, isSmallTask(&copTask{RowCountHint: 6}))
	require.True(t, isSmallTask(&copTask{RowCountHint: CopSmallTaskRow}))
	require.False(t, isSmallTask(&copTask{RowCountHint: CopSmallTaskRow + 1}))
	_, conc := smallTaskConcurrency([]*copTask{}, 16)
	require.GreaterOrEqual(t, conc, 0)
}

// TestBuildCopTasksWithRowCountHint 对应 Go 的同名测试：验证 row count hint 会写入 task 并影响 small task concurrency。
#[test]
pub fn TestBuildCopTasksWithRowCountHint(t *testing.T) {
	// nil --- 'g' --- 'n' --- 't' --- nil
	// <- 0 -> <- 1 -> <- 2 -> <- 3 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()
	_, _, _ = testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()
	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)
	req := &kv.Request{}
	ranges := buildCopRanges("a", "c", "d", "e", "h", "x", "y", "z")
	tasks, err := buildCopTasks(bo, ranges, &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		rowHints: []int{1, 1, 3, CopSmallTaskRow},
	})
	require.Nil(t, err)
	require.Equal(t, len(tasks), 4)
	// task[0] ["a"-"c", "d"-"e"]
	require.Equal(t, tasks[0].RowCountHint, 2)
	// task[1] ["h"-"n"]
	require.Equal(t, tasks[1].RowCountHint, 3)
	// task[2] ["n"-"t"]
	require.Equal(t, tasks[2].RowCountHint, 3)
	// task[3] ["t"-"x", "y"-"z"]
	require.Equal(t, tasks[3].RowCountHint, 3+CopSmallTaskRow)
	_, conc := smallTaskConcurrency(tasks, 16)
	require.Equal(t, conc, 1)

	ranges = buildCopRanges("a", "c", "d", "e", "h", "x", "y", "z")
	tasks, err = buildCopTasks(bo, ranges, &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		rowHints: []int{1, 1, 3, 3},
	})
	require.Nil(t, err)
	require.Equal(t, len(tasks), 4)
	// task[0] ["a"-"c", "d"-"e"]
	require.Equal(t, tasks[0].RowCountHint, 2)
	// task[1] ["h"-"n"]
	require.Equal(t, tasks[1].RowCountHint, 3)
	// task[2] ["n"-"t"]
	require.Equal(t, tasks[2].RowCountHint, 3)
	// task[3] ["t"-"x", "y"-"z"]
	require.Equal(t, tasks[3].RowCountHint, 6)
	_, conc = smallTaskConcurrency(tasks, 16)
	require.Equal(t, conc, 2)

	// cross-region long range
	ranges = buildCopRanges("a", "z")
	tasks, err = buildCopTasks(bo, ranges, &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		rowHints: []int{10},
	})
	require.Nil(t, err)
	require.Equal(t, len(tasks), 4)
	// task[0] ["a"-"g"]
	require.Equal(t, tasks[0].RowCountHint, 10)
	// task[1] ["g"-"n"]
	require.Equal(t, tasks[1].RowCountHint, 10)
	// task[2] ["n"-"t"]
	require.Equal(t, tasks[2].RowCountHint, 10)
	// task[3] ["t"-"z"]
	require.Equal(t, tasks[3].RowCountHint, 10)
}

// TestSmallTaskConcurrencyLimit 对应 Go 的同名测试：验证 small task concurrency limit 受 CPU 与任务数共同约束。
#[test]
pub fn TestSmallTaskConcurrencyLimit(t *testing.T) {
	smallTaskCount := 1000
	tasks := make([]*copTask, 0, smallTaskCount)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for range smallTaskCount {
		tasks = append(tasks, &copTask{
			RowCountHint: 1,
		})
	}
	count, conc := smallTaskConcurrency(tasks, 1)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.Equal(t, smallConcPerCore, conc)
	require.Equal(t, smallTaskCount, count)
	// also handle 0 value.
	count, conc = smallTaskConcurrency(tasks, 0)
	require.Equal(t, smallConcPerCore, conc)
	require.Equal(t, smallTaskCount, count)
}

// TestBatchStoreCoprOnlySendToLeader 对应 Go 的同名测试：验证 batch store cop 只发 leader 且 busy threshold 按请求保留。
#[test]
pub fn TestBatchStoreCoprOnlySendToLeader(t *testing.T) {
	// nil --- 'g' --- 'n' --- 't' --- nil
	// <- 0 -> <- 1 -> <- 2 -> <- 3 ->
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()
	_, _, _ = testutils.BootstrapWithMultiRegions(cluster, []byte("g"), []byte("n"), []byte("t"))
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()
	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)
	req := &kv.Request{
		StoreBatchSize:     3,
		// 时间相关断言保留 Go 的等待/耗时边界；不实际睡眠或计时。
		StoreBusyThreshold: time.Second,
	}
	ranges := buildCopRanges("a", "c", "d", "e", "h", "x", "y", "z")
	tasks, err := buildCopTasks(bo, ranges, &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		rowHints: []int{1, 1, 3, 3},
	})
	require.Len(t, tasks, 1)
	require.Zero(t, tasks[0].busyThreshold)
	batched := tasks[0].batchTaskList
	require.Len(t, batched, 3)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for _, task := range batched {
		require.Zero(t, task.task.busyThreshold)
	}

	req = &kv.Request{
		StoreBatchSize:     0,
		StoreBusyThreshold: time.Second,
	}
	tasks, err = buildCopTasks(bo, ranges, &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		rowHints: []int{1, 1, 3, 3},
	})
	require.Len(t, tasks, 4)
	for _, task := range tasks {
		require.Equal(t, task.busyThreshold, time.Second)
	}
}

// TestStoreBatchTasksPreserveChildBucketsVersion 对应 Go 的同名测试：验证 store batch 转 PB 后子任务 bucket version 不丢失。
#[test]
pub fn TestStoreBatchTasksPreserveChildBucketsVersion(t *testing.T) {
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	_, regionIDs, _ := testutils.BootstrapWithMultiRegions(cluster, []byte("n"), []byte("x"))
	cluster.SplitRegionBuckets(regionIDs[0], [][]byte{{}, {'n'}}, 101)
	cluster.SplitRegionBuckets(regionIDs[1], [][]byte{{'n'}, {'x'}}, 202)
	cluster.SplitRegionBuckets(regionIDs[2], [][]byte{{'x'}, {}}, 303)
	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()
	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)

	req := &kv.Request{StoreBatchSize: 3}
	tasks, err := buildCopTasks(bo, buildCopRanges("a", "b", "o", "p", "y", "z"), &buildCopTaskOpt{
		req:      req,
		cache:    cache,
		rowHints: []int{1, 1, 1},
	})
	require.NoError(t, err)
	require.Len(t, tasks, 1)

	pbTasks := tasks[0].ToPBBatchTasks()
	require.Len(t, pbTasks, 2)
	versionByRegion := make(map[uint64]uint64, len(pbTasks))
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for _, pbTask := range pbTasks {
		versionByRegion[pbTask.GetRegionId()] = pbTask.GetBucketsVersion()
	}
	require.Equal(t, map[uint64]uint64{
		regionIDs[1]: 202,
		regionIDs[2]: 303,
	}, versionByRegion)
}

// TestHandleBatchCopResponseUpdatesChildBucketsOnVersionNotMatch 对应 Go 的同名测试：验证 batch cop response 遇 bucket version mismatch 后刷新 child bucket 元数据。
#[test]
pub fn TestHandleBatchCopResponseUpdatesChildBucketsOnVersionNotMatch(t *testing.T) {
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, cluster, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	_, regionIDs, _ := testutils.BootstrapWithMultiRegions(cluster, []byte("m"))
	cluster.SplitRegionBuckets(regionIDs[0], [][]byte{{}, {'m'}}, 7)
	cluster.SplitRegionBuckets(regionIDs[1], [][]byte{{'m'}, {'n'}, {}}, 11)

	tikvStore, err := tikv.NewTestTiKVStore(mockClient, pdClient, nil, nil, 0)
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		require.NoError(t, tikvStore.Close())
	}()
	copStore, err := NewStore(tikvStore, nil)
	require.NoError(t, err)
	defer copStore.Close()

	cache := copStore.GetRegionCache()
	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	bo := backoff.NewBackofferWithVars(context.Background(), 3000, nil)
	req := &kv.Request{}

	parentTasks, err := buildCopTasks(bo, buildCopRanges("a", "b"), &buildCopTaskOpt{
		req:   req,
		cache: cache,
	})
	require.NoError(t, err)
	require.Len(t, parentTasks, 1)
	childTasks, err := buildCopTasks(bo, buildCopRanges("n", "o"), &buildCopTaskOpt{
		req:   req,
		cache: cache,
	})
	require.NoError(t, err)
	require.Len(t, childTasks, 1)
	parentTask := parentTasks[0]
	childTask := childTasks[0]
	require.Equal(t, uint64(7), parentTask.bucketsVer)
	require.Equal(t, uint64(11), childTask.bucketsVer)

	childTask.taskID = 1
	// atomic 变量用于跨回调观察计数或取消状态；Rust 接线时需使用原子类型保持内存语义。
	var storeBatchedNum atomic.Uint64
	var storeBatchedFallbackNum atomic.Uint64
	worker := &copIteratorWorker{
		store:                   copStore,
		req:                     req,
		storeBatchedNum:         &storeBatchedNum,
		storeBatchedFallbackNum: &storeBatchedFallbackNum,
	}
	parentRPCCtx := &tikv.RPCContext{
		Region:        parentTask.region,
		BucketVersion: parentTask.bucketsVer,
	}
	bucketKeys := [][]byte{[]byte("m"), []byte("n"), {}}
	resp := &coprocessor.Response{
		BatchResponses: []*coprocessor.StoreBatchTaskResponse{
			{
				TaskId: childTask.taskID,
				RegionError: &errorpb.Error{
					BucketVersionNotMatch: &errorpb.BucketVersionNotMatch{
						Version: 99,
						Keys:    bucketKeys,
					},
				},
			},
		},
	}

	_, remains, err := worker.handleBatchCopResponse(bo, parentRPCCtx, resp, map[uint64]*batchedCopTask{
		childTask.taskID: {task: childTask},
	})
	require.NoError(t, err)
	require.NotEmpty(t, remains)

	loc, err := cache.LocateKey(bo.TiKVBackoffer(), []byte("n"))
	require.NoError(t, err)
	require.Equal(t, regionIDs[1], loc.Region.GetID())
	require.Equal(t, uint64(99), loc.Buckets.GetVersion())
	require.Equal(t, bucketKeys, loc.Buckets.GetKeys())
}
"################;

use protobuf::Message;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    Backoffer, BatchError, BatchResult, BatchedCopTask, BuildCopTaskOptions, CopBackend,
    CopProtocolResponse, CopRUInterceptor, CopRequest, CopTask, CopTaskWorker, CopWireRequest,
    KeyRange, KeyRanges, LocatedKeyRanges, PagingOptions, PartitionKeyRanges, Peer, RegionVerId,
    ReplicaReadType, RequestType, RunawayAction, RunawayChecker, StoreType, build_cop_tasks,
    calculate_remain, calculate_retry, check_store_batch_coprocessor, ensure_monotonic_key_ranges,
    is_small_task, small_task_concurrency,
};

#[derive(Default)]
struct TestBackend {
    locations: Mutex<Vec<LocatedKeyRanges>>,
    response: Mutex<CopProtocolResponse>,
    wires: Mutex<Vec<CopWireRequest>>,
    invalidated: Mutex<Vec<RegionVerId>>,
    bucket_updates: Mutex<Vec<(RegionVerId, u64, u64)>>,
    allow_batch: AtomicBool,
    transport_error: Mutex<Option<String>>,
    missing_region_once: AtomicBool,
    resolved_lock_calls: Mutex<Vec<Vec<u8>>>,
}

impl TestBackend {
    fn with_locations(locations: Vec<LocatedKeyRanges>) -> Arc<Self> {
        Arc::new(Self {
            locations: Mutex::new(locations),
            ..Self::default()
        })
    }
}

impl CopBackend for TestBackend {
    fn split_key_ranges(
        &self,
        _ranges: &KeyRanges,
        _skip_buckets: bool,
    ) -> BatchResult<Vec<LocatedKeyRanges>> {
        Ok(self.locations.lock().unwrap().clone())
    }

    fn build_batch_task(
        &self,
        task: &CopTask,
        _replica_read: ReplicaReadType,
    ) -> BatchResult<Option<BatchedCopTask>> {
        Ok(self
            .allow_batch
            .load(Ordering::Acquire)
            .then(|| BatchedCopTask {
                task: Box::new(task.clone()),
                store_id: 1,
                peer: Some(Peer { id: 1, store_id: 1 }),
                load_based_replica_retry: false,
            }))
    }

    fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>> {
        Ok(vec![(1, "tidb-1".to_owned()), (2, "tidb-2".to_owned())])
    }

    fn send(&self, _task: &CopTask, request: &CopWireRequest) -> BatchResult<CopProtocolResponse> {
        self.wires.lock().unwrap().push(request.clone());
        if self.missing_region_once.swap(false, Ordering::AcqRel) {
            return Err(BatchError::MissingRegion(RegionVerId::new(1, 1, 1)));
        }
        if let Some(error) = self.transport_error.lock().unwrap().take() {
            return Err(BatchError::Transport(error));
        }
        Ok(self.response.lock().unwrap().clone())
    }

    fn invalidate_region(&self, region: RegionVerId) {
        self.invalidated.lock().unwrap().push(region);
    }

    fn update_buckets(&self, region: RegionVerId, old_version: u64, new_version: u64) {
        self.bucket_updates
            .lock()
            .unwrap()
            .push((region, old_version, new_version));
    }

    fn resolve_lock(&self, lock: &[u8], _start_ts: u64) -> BatchResult<()> {
        self.resolved_lock_calls.lock().unwrap().push(lock.to_vec());
        Ok(())
    }

    fn check_visibility(&self, _start_ts: u64) -> BatchResult<()> {
        Ok(())
    }
}

fn key_range(start: &str, end: &str) -> KeyRange {
    KeyRange {
        start: start.as_bytes().to_vec(),
        end: end.as_bytes().to_vec(),
    }
}

fn location(id: u64, bucket_version: u64, ranges: Vec<KeyRange>) -> LocatedKeyRanges {
    LocatedKeyRanges {
        region: RegionVerId::new(id, 1, 1),
        location_start: ranges
            .first()
            .map(|range| range.start.clone())
            .unwrap_or_default(),
        location_end: ranges
            .last()
            .map(|range| range.end.clone())
            .unwrap_or_default(),
        ranges: KeyRanges::new(ranges),
        bucket_version,
        store_address: format!("store-{id}"),
        store_id: id,
        ..LocatedKeyRanges::default()
    }
}

fn request(ranges: Vec<KeyRange>) -> CopRequest {
    CopRequest {
        key_ranges: vec![PartitionKeyRanges {
            ranges,
            row_hints: Vec::new(),
        }],
        concurrency: 8,
        ..CopRequest::default()
    }
}

fn worker(backend: Arc<TestBackend>, request: CopRequest) -> CopTaskWorker {
    CopTaskWorker::new(
        backend,
        Arc::new(request),
        None,
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU32::new(0)),
    )
}

#[test]
fn ensure_monotonic_key_ranges_sorts_only_invalid_input() {
    let mut ranges = KeyRanges::new(vec![
        key_range("m", "z"),
        key_range("a", "b"),
        key_range("b", "m"),
    ]);
    assert!(ensure_monotonic_key_ranges(&mut ranges));
    assert_eq!(
        ranges.to_ranges(),
        vec![
            key_range("a", "b"),
            key_range("b", "m"),
            key_range("m", "z")
        ]
    );
    assert!(!ensure_monotonic_key_ranges(&mut ranges));
}

#[test]
fn build_tasks_without_buckets_preserves_region_boundaries() {
    let backend = TestBackend::with_locations(vec![
        location(1, 0, vec![key_range("a", "m")]),
        location(2, 0, vec![key_range("m", "z")]),
    ]);
    let req = request(vec![key_range("a", "z")]);
    let tasks = build_cop_tasks(
        backend.as_ref(),
        &req,
        KeyRanges::new(vec![key_range("a", "z")]),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].region.id, 1);
    assert_eq!(tasks[1].region.id, 2);
}

#[test]
fn build_tasks_by_buckets_carries_bucket_version() {
    let backend = TestBackend::with_locations(vec![
        location(1, 7, vec![key_range("a", "f")]),
        location(1, 7, vec![key_range("f", "m")]),
    ]);
    let req = request(vec![key_range("a", "m")]);
    let tasks = build_cop_tasks(
        backend.as_ref(),
        &req,
        KeyRanges::new(vec![key_range("a", "m")]),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().all(|task| task.bucket_version == 7));
}

#[test]
fn split_key_ranges_by_locations_without_buckets_keeps_all_ranges() {
    let backend = TestBackend::with_locations(vec![
        location(1, 0, vec![key_range("a", "b"), key_range("c", "d")]),
        location(2, 0, vec![key_range("x", "z")]),
    ]);
    let input = vec![
        key_range("a", "b"),
        key_range("c", "d"),
        key_range("x", "z"),
    ];
    let tasks = build_cop_tasks(
        backend.as_ref(),
        &request(input.clone()),
        KeyRanges::new(input.clone()),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert_eq!(
        tasks
            .iter()
            .flat_map(|task| task.ranges.to_ranges())
            .collect::<Vec<_>>(),
        input
    );
}

#[test]
fn split_key_ranges_matches_go_half_open_boundaries() {
    let ranges = KeyRanges::new(vec![key_range("a", "m"), key_range("m", "z")]);
    let (left, right) = ranges.split(b"f");
    assert_eq!(left.to_ranges(), vec![key_range("a", "f")]);
    assert_eq!(
        right.to_ranges(),
        vec![key_range("f", "m"), key_range("m", "z")]
    );
}

#[test]
fn rebuild_after_region_error_returns_fresh_tasks() {
    let backend = TestBackend::with_locations(vec![location(2, 0, vec![key_range("a", "z")])]);
    backend.response.lock().unwrap().region_error = Some("epoch not match".to_owned());
    let task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "z")]),
        ..CopTask::default()
    };
    let mut backoffer = Backoffer::new(3);
    let result = worker(backend, request(vec![key_range("a", "z")]))
        .handle_task_once(&mut backoffer, task)
        .unwrap();
    assert_eq!(result.remains.len(), 1);
    assert_eq!(result.remains[0].region.id, 2);
    assert_eq!(backoffer.history.len(), 1);
}

#[test]
fn build_paging_tasks_grows_size_per_location() {
    let backend = TestBackend::with_locations(vec![
        location(1, 0, vec![key_range("a", "b")]),
        location(2, 0, vec![key_range("b", "c")]),
        location(3, 0, vec![key_range("c", "d")]),
    ]);
    let mut req = request(vec![
        key_range("a", "b"),
        key_range("b", "c"),
        key_range("c", "d"),
    ]);
    req.paging = PagingOptions {
        enabled: true,
        minimum_size: 8,
        maximum_size: 32,
        size_bytes: 0,
    };
    let tasks = build_cop_tasks(
        backend.as_ref(),
        &req,
        KeyRanges::new(req.key_ranges[0].ranges.clone()),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.paging_size)
            .collect::<Vec<_>>(),
        vec![8, 8, 8]
    );
    assert!(tasks.iter().all(|task| task.paging));
}

#[test]
fn build_cop_tasks_does_not_cancel_backoffer_context() {
    let backend = TestBackend::with_locations(vec![location(1, 0, vec![key_range("a", "z")])]);
    let token = Backoffer::new(1).cancellation();
    let _ = build_cop_tasks(
        backend.as_ref(),
        &request(vec![key_range("a", "z")]),
        KeyRanges::new(vec![key_range("a", "z")]),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert!(!token.is_cancelled());
}

#[test]
fn paging_is_disabled_when_limit_is_smaller_than_page() {
    let backend = TestBackend::with_locations(vec![location(1, 0, vec![key_range("a", "z")])]);
    let mut req = request(vec![key_range("a", "z")]);
    req.limit_size = 7;
    req.paging = PagingOptions {
        enabled: true,
        minimum_size: 8,
        maximum_size: 32,
        size_bytes: 0,
    };
    let tasks = build_cop_tasks(
        backend.as_ref(),
        &req,
        KeyRanges::new(vec![key_range("a", "z")]),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert!(!tasks[0].paging);
    assert_eq!(tasks[0].paging_size, 0);
}

#[test]
fn paging_size_bytes_is_forwarded_to_wire_request() {
    let backend = TestBackend::with_locations(vec![location(1, 0, vec![key_range("a", "z")])]);
    let mut req = request(vec![key_range("a", "z")]);
    req.paging.size_bytes = 4096;
    let task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "z")]),
        ..CopTask::default()
    };
    worker(backend.clone(), req)
        .handle_task_once(&mut Backoffer::new(1), task)
        .unwrap();
    assert_eq!(backend.wires.lock().unwrap()[0].paging_size_bytes, 4096);
}

#[test]
fn calculate_retry_matches_ascending_and_descending_go_cases() {
    let ranges = KeyRanges::new(vec![key_range("a", "m"), key_range("m", "z")]);
    let split = key_range("f", "t");
    assert_eq!(
        calculate_retry(&ranges, Some(&split), false).to_ranges(),
        vec![key_range("f", "m"), key_range("m", "z")]
    );
    assert_eq!(
        calculate_retry(&ranges, Some(&split), true).to_ranges(),
        vec![key_range("a", "m"), key_range("m", "t")]
    );
    assert_eq!(calculate_retry(&ranges, None, false), ranges);
}

#[test]
fn calculate_remain_matches_ascending_and_descending_go_cases() {
    let ranges = KeyRanges::new(vec![key_range("a", "m"), key_range("m", "z")]);
    let split = key_range("f", "t");
    assert_eq!(
        calculate_remain(&ranges, Some(&split), false).to_ranges(),
        vec![key_range("t", "z")]
    );
    assert_eq!(
        calculate_remain(&ranges, Some(&split), true).to_ranges(),
        vec![key_range("a", "f")]
    );
    assert_eq!(calculate_remain(&ranges, None, false), ranges);
}

#[test]
fn basic_small_task_concurrency_classifies_hints() {
    let tasks = vec![
        CopTask {
            row_count_hint: 1,
            ..CopTask::default()
        },
        CopTask {
            row_count_hint: 32,
            ..CopTask::default()
        },
        CopTask {
            row_count_hint: 33,
            ..CopTask::default()
        },
    ];
    assert!(is_small_task(&tasks[0]));
    assert!(is_small_task(&tasks[1]));
    assert!(!is_small_task(&tasks[2]));
    let (count, concurrency) = small_task_concurrency(&tasks, 4);
    assert_eq!(count, 2);
    assert!(concurrency <= 80);
}

#[test]
fn build_cop_tasks_aggregates_row_count_hints() {
    let backend = TestBackend::with_locations(vec![location(
        1,
        0,
        vec![key_range("a", "b"), key_range("b", "c")],
    )]);
    let input = vec![key_range("a", "b"), key_range("b", "c")];
    let tasks = build_cop_tasks(
        backend.as_ref(),
        &request(input.clone()),
        KeyRanges::new(input),
        BuildCopTaskOptions {
            row_hints: vec![10, 20],
            ..BuildCopTaskOptions::default()
        },
    )
    .unwrap();
    assert_eq!(tasks[0].row_count_hint, 30);
}

#[test]
fn small_task_concurrency_respects_cpu_limit() {
    let tasks = (0..10_000)
        .map(|_| CopTask {
            row_count_hint: 1,
            ..CopTask::default()
        })
        .collect::<Vec<_>>();
    assert_eq!(small_task_concurrency(&tasks, 1).1, 20);
    assert_eq!(small_task_concurrency(&tasks, 4).1, 80);
}

#[test]
fn store_batch_coprocessor_only_accepts_leader_requests() {
    let mut req = request(vec![key_range("a", "z")]);
    assert!(check_store_batch_coprocessor(&req));
    req.replica_read = ReplicaReadType::Follower;
    assert!(!check_store_batch_coprocessor(&req));
    req.replica_read = ReplicaReadType::Leader;
    req.keep_order = true;
    assert!(!check_store_batch_coprocessor(&req));
    req.keep_order = false;
    req.request_type = RequestType::Analyze;
    assert!(!check_store_batch_coprocessor(&req));
    req.request_type = RequestType::Dag;
    req.store_type = StoreType::TiFlash;
    assert!(!check_store_batch_coprocessor(&req));
}

#[test]
fn go_merge_48_analyze_batch_requires_explicit_merge_contract() {
    let mut req = request(vec![key_range("a", "z")]);
    req.request_type = RequestType::Analyze;
    req.store_batch_size = 3;
    req.request_source.internal = true;
    assert!(!check_store_batch_coprocessor(&req));
    req.allow_batch_task_data_merge = true;
    req.execute_batch_tasks_serially = true;
    assert!(check_store_batch_coprocessor(&req));
}

#[test]
fn go_merge_48_unhinted_merge_batches_and_forwards_wire_flags() {
    let backend = TestBackend::with_locations(vec![
        location(1, 0, vec![key_range("a", "b")]),
        location(2, 0, vec![key_range("b", "c")]),
        location(3, 0, vec![key_range("c", "d")]),
    ]);
    backend.allow_batch.store(true, Ordering::Release);
    let mut req = request(vec![key_range("a", "d")]);
    req.request_type = RequestType::Analyze;
    req.request_source.internal = true;
    req.store_batch_size = 3;
    let ranges = KeyRanges::new(vec![key_range("a", "d")]);
    let legacy = build_cop_tasks(
        backend.as_ref(),
        &req,
        ranges.clone(),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert_eq!(legacy.len(), 3);
    req.allow_batch_task_data_merge = true;
    req.execute_batch_tasks_serially = true;
    let merged = build_cop_tasks(
        backend.as_ref(),
        &req,
        ranges,
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].batch_task_list.len(), 2);
    worker(backend.clone(), req)
        .handle_task_once(&mut Backoffer::new(1), merged[0].clone())
        .unwrap();
    let wires = backend.wires.lock().unwrap();
    assert!(wires[0].allow_batch_task_data_merge);
    assert!(wires[0].execute_batch_tasks_serially);
}

#[test]
fn go_merge_48_query_limiter_is_per_store_and_overrides_request_limiter() {
    let backend = TestBackend::with_locations(vec![location(1, 0, vec![key_range("a", "b")])]);
    let mut req = request(vec![key_range("a", "b")]);
    req.copr_request_limiter = astersql_kv::NewCoprRequestLimiter(1);
    req.query_cop_store_limiter = astersql_kv::NewQueryCopStoreLimiter(1);
    worker(backend.clone(), req)
        .handle_task_once(
            &mut Backoffer::new(1),
            CopTask {
                region: RegionVerId::new(1, 1, 1),
                ranges: KeyRanges::new(vec![key_range("a", "b")]),
                ..CopTask::default()
            },
        )
        .unwrap();
    let admission = backend.wires.lock().unwrap()[0]
        .attempt_limiter
        .clone()
        .unwrap();
    let first = admission.acquire(1).unwrap().unwrap();
    let other_store = admission.acquire(2).unwrap().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
    let second = admission.clone();
    let join = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let _permit = second.acquire(1).unwrap().unwrap();
        acquired_tx.send(()).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(
        acquired_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err()
    );
    drop(first);
    acquired_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    drop(other_store);
    join.join().unwrap();
    let waits = admission.wait_stats();
    assert!(!waits.is_zero());
    assert!(waits.max_time <= waits.total_time);
}

#[test]
fn go_merge_48_request_limiter_spans_stores() {
    let backend = TestBackend::with_locations(vec![location(1, 0, vec![key_range("a", "b")])]);
    let mut req = request(vec![key_range("a", "b")]);
    req.copr_request_limiter = astersql_kv::NewCoprRequestLimiter(1);
    worker(backend.clone(), req)
        .handle_task_once(
            &mut Backoffer::new(1),
            CopTask {
                region: RegionVerId::new(1, 1, 1),
                ranges: KeyRanges::new(vec![key_range("a", "b")]),
                ..CopTask::default()
            },
        )
        .unwrap();
    let admission = backend.wires.lock().unwrap()[0]
        .attempt_limiter
        .clone()
        .unwrap();
    let first = admission.acquire(1).unwrap().unwrap();
    let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
    let second = admission.clone();
    let join = std::thread::spawn(move || {
        let _permit = second.acquire(2).unwrap().unwrap();
        acquired_tx.send(()).unwrap();
    });
    assert!(
        acquired_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err()
    );
    drop(first);
    acquired_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    join.join().unwrap();
    assert!(!admission.wait_stats().is_zero());
}

#[test]
fn go_merge_48_merged_child_ack_does_not_emit_empty_response() {
    let backend = TestBackend::with_locations(Vec::new());
    let child = CopTask {
        task_id: 2,
        region: RegionVerId::new(2, 1, 1),
        ranges: KeyRanges::new(vec![key_range("b", "c")]),
        ..CopTask::default()
    };
    let mut parent = CopTask {
        task_id: 1,
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "b")]),
        ..CopTask::default()
    };
    parent.batch_task_list.insert(
        2,
        BatchedCopTask {
            task: Box::new(child),
            store_id: 1,
            peer: Some(Peer { id: 1, store_id: 1 }),
            load_based_replica_retry: false,
        },
    );
    backend.response.lock().unwrap().batch_responses.insert(
        2,
        CopProtocolResponse {
            data_merged_into_response: true,
            read_bytes: 17,
            ..CopProtocolResponse::default()
        },
    );
    let worker = worker(backend, request(vec![key_range("a", "c")]));
    let result = worker
        .handle_task_once(&mut Backoffer::new(1), parent)
        .unwrap();
    assert!(result.batch_responses.is_empty());
    assert!(result.remains.is_empty());
    assert_eq!(worker.runtime_stats().len(), 1);
}

#[test]
fn go_merge_48_pre_dispatch_miss_rebuilds_all_batch_ranges_once() {
    let backend = TestBackend::with_locations(vec![
        location(1, 0, vec![key_range("a", "b")]),
        location(2, 0, vec![key_range("b", "c")]),
    ]);
    backend.allow_batch.store(true, Ordering::Release);
    backend.missing_region_once.store(true, Ordering::Release);
    let mut req = request(vec![key_range("a", "c")]);
    req.allow_batch_task_data_merge = true;
    req.store_batch_size = 2;
    let mut tasks = build_cop_tasks(
        backend.as_ref(),
        &req,
        KeyRanges::new(vec![key_range("a", "c")]),
        BuildCopTaskOptions::default(),
    )
    .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].batch_task_list.len(), 1);
    req.store_batch_size = 0;
    let result = worker(backend, req)
        .handle_task_once(&mut Backoffer::new(3), tasks.remove(0))
        .unwrap();
    let mut ranges = result
        .remains
        .into_iter()
        .flat_map(|task| {
            let mut ranges = task.ranges.to_ranges();
            for child in task.batch_task_list.values() {
                ranges.extend(child.task.ranges.to_ranges());
            }
            ranges
        })
        .collect::<Vec<_>>();
    ranges.sort_by(|a, b| a.start.cmp(&b.start));
    assert_eq!(ranges, vec![key_range("a", "b"), key_range("b", "c")]);
}

#[test]
fn go_merge_48_paging_ema_precharge_updates_only_for_continuation() {
    let backend = TestBackend::with_locations(Vec::new());
    let mut req = request(vec![key_range("a", "c")]);
    req.paging.size_bytes = 4096;
    let worker = worker(backend.clone(), req);
    let task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "c")]),
        ..CopTask::default()
    };
    *backend.response.lock().unwrap() = CopProtocolResponse {
        range: Some(key_range("a", "b")),
        read_bytes: 123,
        ..CopProtocolResponse::default()
    };
    worker
        .handle_task_once(&mut Backoffer::new(1), task.clone())
        .unwrap();
    assert_eq!(backend.wires.lock().unwrap()[0].predicted_read_bytes, 4096);
    *backend.response.lock().unwrap() = CopProtocolResponse {
        read_bytes: 999,
        ..CopProtocolResponse::default()
    };
    worker
        .handle_task_once(&mut Backoffer::new(1), task)
        .unwrap();
    assert_eq!(backend.wires.lock().unwrap()[1].predicted_read_bytes, 123);
}

fn go_merge_48_lock(txn_id: u64) -> Vec<u8> {
    let mut lock = kvproto::kvrpcpb::LockInfo::new();
    lock.set_lock_version(txn_id);
    lock.write_to_bytes().unwrap()
}

#[test]
fn go_merge_48_lock_hints_back_off_once_and_follow_the_sent_request() {
    let backend = TestBackend::with_locations(Vec::new());
    let mut req = request(vec![key_range("a", "b")]);
    req.resolved_locks = vec![42];
    req.committed_locks = vec![44];
    let worker = worker(backend.clone(), req);
    let task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "b")]),
        ..CopTask::default()
    };
    *backend.response.lock().unwrap() = CopProtocolResponse {
        locked: Some(go_merge_48_lock(42)),
        ..CopProtocolResponse::default()
    };
    let mut backoffer = Backoffer::new(3);
    worker
        .handle_task_once(&mut backoffer, task.clone())
        .unwrap();
    assert_eq!(backoffer.history.len(), 1);
    let wires = backend.wires.lock().unwrap();
    assert_eq!(wires[0].resolved_locks, vec![42]);
    assert_eq!(wires[0].committed_locks, vec![44]);
    drop(wires);
    *backend.response.lock().unwrap() = CopProtocolResponse {
        locked: Some(go_merge_48_lock(43)),
        ..CopProtocolResponse::default()
    };
    worker
        .handle_task_once(&mut backoffer, task.clone())
        .unwrap();
    assert_eq!(
        backoffer.history.len(),
        1,
        "unhinted lock leaves backoff available"
    );
    worker.handle_task_once(&mut backoffer, task).unwrap();
    assert_eq!(
        backoffer.history.len(),
        2,
        "resolved ID is in the next request"
    );
    assert_eq!(
        backend.wires.lock().unwrap()[2].resolved_locks,
        vec![42, 43]
    );
}

#[test]
fn go_merge_48_batch_child_locks_share_one_hint_backoff() {
    let backend = TestBackend::with_locations(Vec::new());
    let mut req = request(vec![key_range("a", "c")]);
    req.resolved_locks = vec![42];
    let mut parent = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "b")]),
        ..CopTask::default()
    };
    for id in [2, 3] {
        parent.batch_task_list.insert(
            id,
            BatchedCopTask {
                task: Box::new(CopTask {
                    task_id: id,
                    region: RegionVerId::new(id, 1, 1),
                    ranges: KeyRanges::new(vec![key_range("b", "c")]),
                    ..CopTask::default()
                }),
                store_id: 1,
                peer: Some(Peer { id: 1, store_id: 1 }),
                load_based_replica_retry: false,
            },
        );
        backend.response.lock().unwrap().batch_responses.insert(
            id,
            CopProtocolResponse {
                locked: Some(go_merge_48_lock(42)),
                ..CopProtocolResponse::default()
            },
        );
    }
    let mut backoffer = Backoffer::new(3);
    let result = worker(backend.clone(), req)
        .handle_task_once(&mut backoffer, parent)
        .unwrap();
    assert_eq!(backoffer.history.len(), 1);
    assert_eq!(result.remains.len(), 2);
    assert!(result.batch_responses.is_empty());
    assert_eq!(backend.resolved_lock_calls.lock().unwrap().len(), 2);
}

#[test]
fn go_merge_48_store_batch_metrics_count_failed_inputs_once() {
    let backend = TestBackend::with_locations(vec![location(3, 0, vec![key_range("b", "c")])]);
    let mut task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "b")]),
        ..CopTask::default()
    };
    for id in [2, 3] {
        task.batch_task_list.insert(
            id,
            BatchedCopTask {
                task: Box::new(CopTask {
                    task_id: id,
                    region: RegionVerId::new(id, 1, 1),
                    ranges: KeyRanges::new(vec![key_range("b", "c")]),
                    ..CopTask::default()
                }),
                store_id: 1,
                peer: Some(Peer { id: 1, store_id: 1 }),
                load_based_replica_retry: false,
            },
        );
    }
    *backend.response.lock().unwrap() = CopProtocolResponse {
        batch_responses: [
            (
                2,
                CopProtocolResponse {
                    data: b"success".to_vec(),
                    ..CopProtocolResponse::default()
                },
            ),
            (
                3,
                CopProtocolResponse {
                    region_error: Some("region split".to_owned()),
                    ..CopProtocolResponse::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        batch_region_errors: [3].into_iter().collect(),
        ..CopProtocolResponse::default()
    };
    let worker = worker(backend, request(vec![key_range("a", "c")]));
    let result = worker
        .handle_task_once(&mut Backoffer::new(1), task)
        .unwrap();
    assert_eq!(result.batch_responses.len(), 1);
    assert_eq!(result.remains.len(), 1);
    assert_eq!(worker.store_batch_stats(), (1, 1));
}

#[test]
fn go_merge_48_child_retry_fanout_does_not_underflow_batch_metrics() {
    let backend = TestBackend::with_locations(vec![
        location(3, 0, vec![key_range("b", "c")]),
        location(4, 0, vec![key_range("c", "d")]),
    ]);
    let mut task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "b")]),
        ..CopTask::default()
    };
    task.batch_task_list.insert(
        2,
        BatchedCopTask {
            task: Box::new(CopTask {
                task_id: 2,
                region: RegionVerId::new(2, 1, 1),
                ranges: KeyRanges::new(vec![key_range("b", "d")]),
                ..CopTask::default()
            }),
            store_id: 1,
            peer: Some(Peer { id: 1, store_id: 1 }),
            load_based_replica_retry: false,
        },
    );
    *backend.response.lock().unwrap() = CopProtocolResponse {
        batch_responses: [(
            2,
            CopProtocolResponse {
                region_error: Some("region split".to_owned()),
                ..CopProtocolResponse::default()
            },
        )]
        .into_iter()
        .collect(),
        batch_region_errors: [2].into_iter().collect(),
        ..CopProtocolResponse::default()
    };
    let worker = worker(backend, request(vec![key_range("a", "d")]));
    let result = worker
        .handle_task_once(&mut Backoffer::new(3), task)
        .unwrap();
    assert_eq!(result.remains.len(), 2);
    assert_eq!(worker.store_batch_stats(), (0, 1));
}

#[test]
fn go_merge_48_batch_child_errors_are_not_delivered_as_data() {
    let backend = TestBackend::with_locations(Vec::new());
    let mut task = CopTask {
        region: RegionVerId::new(1, 1, 1),
        ranges: KeyRanges::new(vec![key_range("a", "b")]),
        ..CopTask::default()
    };
    task.batch_task_list.insert(
        2,
        BatchedCopTask {
            task: Box::new(CopTask {
                task_id: 2,
                region: RegionVerId::new(2, 1, 1),
                ranges: KeyRanges::new(vec![key_range("b", "c")]),
                ..CopTask::default()
            }),
            store_id: 1,
            peer: Some(Peer { id: 1, store_id: 1 }),
            load_based_replica_retry: false,
        },
    );
    backend.response.lock().unwrap().batch_responses.insert(
        2,
        CopProtocolResponse {
            other_error: "write conflict".to_owned(),
            ..CopProtocolResponse::default()
        },
    );
    let worker = worker(backend.clone(), request(vec![key_range("a", "c")]));
    let error = worker
        .handle_task_once(&mut Backoffer::new(1), task.clone())
        .unwrap_err();
    assert!(error.to_string().contains("write conflict"));
    backend.response.lock().unwrap().batch_responses.clear();
    backend
        .response
        .lock()
        .unwrap()
        .batch_responses
        .insert(99, CopProtocolResponse::default());
    let error = worker
        .handle_task_once(&mut Backoffer::new(1), task)
        .unwrap_err();
    assert!(error.to_string().contains("task id 99 not found"));
}

#[test]
fn store_batch_tasks_preserve_child_bucket_versions() {
    let child = CopTask {
        task_id: 7,
        region: RegionVerId::new(7, 1, 1),
        bucket_version: 11,
        ranges: KeyRanges::new(vec![key_range("a", "z")]),
        ..CopTask::default()
    };
    let mut parent = CopTask::default();
    parent.batch_task_list.insert(
        child.task_id,
        BatchedCopTask {
            task: Box::new(child),
            store_id: 1,
            peer: None,
            load_based_replica_retry: false,
        },
    );
    let wire = parent.to_pb_batch_tasks();
    assert_eq!(wire.len(), 1);
    assert_eq!(wire[0].bucket_version, 11);
}

#[test]
fn batch_response_updates_child_bucket_versions() {
    let backend = TestBackend::with_locations(Vec::new());
    let child = CopTask {
        task_id: 7,
        region: RegionVerId::new(7, 1, 1),
        bucket_version: 2,
        ranges: KeyRanges::new(vec![key_range("a", "z")]),
        ..CopTask::default()
    };
    let mut task = CopTask::default();
    task.batch_task_list.insert(
        child.task_id,
        BatchedCopTask {
            task: Box::new(child.clone()),
            store_id: 1,
            peer: None,
            load_based_replica_retry: false,
        },
    );
    backend.response.lock().unwrap().batch_responses.insert(
        child.task_id,
        CopProtocolResponse {
            latest_bucket_version: 5,
            data: b"ok".to_vec(),
            ..CopProtocolResponse::default()
        },
    );
    worker(backend.clone(), request(Vec::new()))
        .handle_task_once(&mut Backoffer::new(1), task)
        .unwrap();
    assert!(
        backend
            .bucket_updates
            .lock()
            .unwrap()
            .contains(&(child.region, 2, 5))
    );
}

#[derive(Debug, Default)]
struct RecordedRunawayKeys(Mutex<Vec<u64>>);

impl RunawayChecker for RecordedRunawayKeys {
    fn check_thresholds(
        &self,
        _ru: Option<&crate::CopRUDetails>,
        processed_keys: u64,
        _error: Option<&BatchError>,
    ) -> BatchResult<()> {
        self.0.lock().unwrap().push(processed_keys);
        Ok(())
    }

    fn check_action(&self) -> RunawayAction {
        RunawayAction::None
    }
}

#[test]
fn batch_response_checks_runaway_threshold_for_parent_and_child_scan_details() {
    let backend = TestBackend::with_locations(Vec::new());
    let checker = Arc::new(RecordedRunawayKeys::default());
    let child = CopTask {
        task_id: 7,
        ranges: KeyRanges::new(vec![key_range("a", "z")]),
        ..CopTask::default()
    };
    let mut task = CopTask::default();
    task.batch_task_list.insert(
        child.task_id,
        BatchedCopTask {
            task: Box::new(child),
            store_id: 1,
            peer: None,
            load_based_replica_retry: false,
        },
    );
    let mut response = backend.response.lock().unwrap();
    response.scanned_keys = 3;
    response.batch_responses.insert(
        7,
        CopProtocolResponse {
            scanned_keys: 5,
            ..CopProtocolResponse::default()
        },
    );
    drop(response);
    let mut request = request(Vec::new());
    request.runaway_checker = Some(checker.clone());
    worker(backend, request)
        .handle_task_once(&mut Backoffer::new(1), task)
        .unwrap();
    assert_eq!(*checker.0.lock().unwrap(), vec![3, 5]);
}

#[derive(Debug)]
struct TwoPhaseRUInterceptor;

impl CopRUInterceptor for TwoPhaseRUInterceptor {
    fn on_request_wait(
        &self,
        _task: &CopTask,
        _wire: &CopWireRequest,
    ) -> BatchResult<crate::CopRUDetails> {
        Ok(crate::CopRUDetails {
            read_ru: 1.0,
            write_ru: 0.0,
        })
    }
    fn on_response_wait(
        &self,
        _task: &CopTask,
        _wire: &CopWireRequest,
        _response: &CopProtocolResponse,
    ) -> BatchResult<crate::CopRUDetails> {
        Ok(crate::CopRUDetails {
            read_ru: 3.0,
            write_ru: 0.0,
        })
    }
}

#[derive(Debug, Default)]
struct RecordedRunawayRU(Mutex<Vec<f64>>);

impl RunawayChecker for RecordedRunawayRU {
    fn check_thresholds(
        &self,
        ru: Option<&crate::CopRUDetails>,
        _keys: u64,
        _error: Option<&BatchError>,
    ) -> BatchResult<()> {
        let read = ru.map_or(0.0, |details| details.read_ru);
        self.0.lock().unwrap().push(read);
        if read >= 4.0 {
            return Err(BatchError::OtherResponse("runaway RU threshold".into()));
        }
        Ok(())
    }
    fn check_action(&self) -> RunawayAction {
        RunawayAction::Kill
    }
}

#[derive(Debug, Default)]
struct RecordedAllRU(Mutex<Vec<f64>>);

impl RunawayChecker for RecordedAllRU {
    fn check_thresholds(
        &self,
        ru: Option<&crate::CopRUDetails>,
        _keys: u64,
        _error: Option<&BatchError>,
    ) -> BatchResult<()> {
        self.0
            .lock()
            .unwrap()
            .push(ru.map_or(0.0, |details| details.read_ru));
        Ok(())
    }

    fn check_action(&self) -> RunawayAction {
        RunawayAction::Kill
    }
}

#[derive(Debug, Default)]
struct RecordedRUError(Mutex<Vec<(f64, Option<String>)>>);

impl RunawayChecker for RecordedRUError {
    fn check_thresholds(
        &self,
        ru: Option<&crate::CopRUDetails>,
        _keys: u64,
        error: Option<&BatchError>,
    ) -> BatchResult<()> {
        self.0.lock().unwrap().push((
            ru.map_or(0.0, |details| details.read_ru),
            error.map(ToString::to_string),
        ));
        Ok(())
    }

    fn check_action(&self) -> RunawayAction {
        RunawayAction::Kill
    }
}

#[test]
fn resource_control_request_and_response_consumption_reaches_runaway_ru_threshold() {
    let backend = TestBackend::with_locations(Vec::new());
    let checker = Arc::new(RecordedRunawayRU::default());
    let mut request = request(Vec::new());
    request.resource_group_name = "rg".into();
    request.runaway_checker = Some(checker.clone());
    request.resource_control_interceptor = Some(Arc::new(TwoPhaseRUInterceptor));
    let error = worker(backend, request)
        .handle_task_once(&mut Backoffer::new(1), CopTask::default())
        .unwrap_err();
    assert!(error.to_string().contains("runaway RU threshold"));
    assert_eq!(*checker.0.lock().unwrap(), vec![4.0]);
}

#[test]
fn resource_control_ru_is_counted_once_and_checked_for_parent_and_child() {
    let backend = TestBackend::with_locations(Vec::new());
    let checker = Arc::new(RecordedAllRU::default());
    let child = CopTask {
        task_id: 7,
        ..CopTask::default()
    };
    let mut task = CopTask::default();
    task.batch_task_list.insert(
        child.task_id,
        BatchedCopTask {
            task: Box::new(child),
            store_id: 1,
            peer: None,
            load_based_replica_retry: false,
        },
    );
    backend.response.lock().unwrap().batch_responses.insert(
        7,
        CopProtocolResponse {
            data: b"child".to_vec(),
            ..CopProtocolResponse::default()
        },
    );
    let mut request = request(Vec::new());
    request.resource_group_name = "rg".into();
    request.runaway_checker = Some(checker.clone());
    request.resource_control_interceptor = Some(Arc::new(TwoPhaseRUInterceptor));

    worker(backend, request)
        .handle_task_once(&mut Backoffer::new(1), task)
        .unwrap();

    assert_eq!(*checker.0.lock().unwrap(), vec![4.0, 4.0]);
}

#[test]
fn request_wait_ru_and_original_transport_error_reach_runaway_checker() {
    let backend = TestBackend::with_locations(Vec::new());
    *backend.transport_error.lock().unwrap() = Some("rpc unavailable".to_owned());
    let checker = Arc::new(RecordedRUError::default());
    let mut request = request(Vec::new());
    request.resource_group_name = "rg".into();
    request.runaway_checker = Some(checker.clone());
    request.resource_control_interceptor = Some(Arc::new(TwoPhaseRUInterceptor));

    let error = worker(backend, request)
        .handle_task_once(&mut Backoffer::new(1), CopTask::default())
        .unwrap_err();

    assert_eq!(error.to_string(), "transport error: rpc unavailable");
    assert_eq!(
        *checker.0.lock().unwrap(),
        vec![(1.0, Some("transport error: rpc unavailable".to_owned()))]
    );
}
