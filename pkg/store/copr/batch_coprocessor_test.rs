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

// Batch Coprocessor 单元测试的 Go 参考源码占位文件。
//
// 全文以原始字符串保存 Go 侧均衡、一致性哈希、轮询调度、拓扑退避与存活 store
// 探测等测试，供迁移对照；当前无独立可执行的 Rust `#[test]`。

/// 嵌入的 Go 参考测试源码（Batch Cop 均衡与 TiFlash store 相关），不参与编译执行逻辑。
const GO_REFERENCE: &str = r################"

// testing/require/mockstore/testutils/failpoint/backoff/context/goleak 等外部依赖均按 Go 语义保留为占位调用，后续需接入 Rust 测试基础设施。
// Go imports（仅记录来源依赖，暂不接入 Rust crate）：
// - "context"
// - "math/rand"
// - "slices"
// - "sort"
// - "strconv"
// - "testing"
// - "time"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/failpoint"
// - "github.com/pingcap/kvproto/pkg/metapb"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/pingcap/tidb/pkg/store/driver/backoff"
// - "github.com/pingcap/tidb/pkg/util/logutil"
// - "github.com/pingcap/tidb/pkg/util/tiflash"
// - "github.com/stathat/consistent"
// - "github.com/stretchr/testify/require"
// - "github.com/tikv/client-go/v2/testutils"
// - "github.com/tikv/client-go/v2/tikv"
// - "github.com/tikv/client-go/v2/tikvrpc"
// - "go.uber.org/zap"

// StoreID: [1, storeCount]
// 构造 storeID 到 batchCopTask 的映射，保持 Go 中 StoreID 从 1 开始的约定。
pub fn buildStoreTaskMap(storeCount int) map[uint64]*batchCopTask {
	storeTasks := make(map[uint64]*batchCopTask)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for i := range storeCount {
		storeTasks[uint64(i+1)] = &batchCopTask{}
	}
	return storeTasks
}

// 构造带随机副本 store 的 RegionInfo fixture，保留 Go 对 region range 连续性的测试输入。
pub fn buildRegionInfos(storeCount, regionCount, replicaNum int) []RegionInfo {
	ss := make([]string, 0, regionCount)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for i := range regionCount {
		s := strconv.Itoa(i)
		ss = append(ss, s)
	}
	sort.Strings(ss)

	storeIDExist := func(storeID uint64, storeIDs []uint64) bool {
		return slices.Contains(storeIDs, storeID)
	}

	randomStores := func(storeCount, replicaNum int) []uint64 {
		var storeIDs []uint64
		for len(storeIDs) < replicaNum {
			t := uint64(rand.Intn(storeCount) + 1)
			// 条件分支保留 Go 的边界判断；Rust 接线时需确认 nil/空切片语义。
			if storeIDExist(t, storeIDs) {
				continue
			}
			storeIDs = append(storeIDs, t)
		}
		return storeIDs
	}

	var startKey string
	regionInfos := make([]RegionInfo, 0, len(ss))
	for i, s := range ss {
		var ri RegionInfo
		ri.Region = tikv.NewRegionVerID(uint64(i), 1, 1)
		ri.Meta = nil
		ri.AllStores = randomStores(storeCount, replicaNum)

		var keyRange kv.KeyRange
		if len(startKey) == 0 {
			keyRange.StartKey = nil
		} else {
			keyRange.StartKey = kv.Key(startKey)
		}
		keyRange.EndKey = kv.Key(s)
		ri.Ranges = NewKeyRanges([]kv.KeyRange{keyRange})
		regionInfos = append(regionInfos, ri)
		startKey = s
	}
	return regionInfos
}

// 统计 batchCopTask 中 regionInfos 的总数，函数名沿用 Go 原拼写。
pub fn calcReginCount(tasks []*batchCopTask) int {
	count := 0
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for _, task := range tasks {
		count += len(task.regionInfos)
	}
	return count
}

// TestBalanceBatchCopTaskWithContinuity 对应 Go 的同名测试：覆盖 batch cop 连续性均衡：大 region 集合要求 balance score 合理，小集合保持 nil 结果。
#[test]
pub fn TestBalanceBatchCopTaskWithContinuity(t *testing.T) {
	for replicaNum := 1; replicaNum < 6; replicaNum++ {
		storeCount := 10
		regionCount := 100000
		storeTasks := buildStoreTaskMap(storeCount)
		regionInfos := buildRegionInfos(storeCount, regionCount, replicaNum)
		tasks, score := balanceBatchCopTaskWithContinuity(storeTasks, regionInfos, 20)
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.True(t, isBalance(score))
		require.Equal(t, regionCount, calcReginCount(tasks))
	}

	{
		storeCount := 10
		regionCount := 100
		replicaNum := 2
		storeTasks := buildStoreTaskMap(storeCount)
		regionInfos := buildRegionInfos(storeCount, regionCount, replicaNum)
		tasks, _ := balanceBatchCopTaskWithContinuity(storeTasks, regionInfos, 20)
		require.True(t, tasks == nil)
	}
}

// TestBalanceBatchCopTaskWithEmptyTaskSet 对应 Go 的同名测试：覆盖 nil 与空 task set 的不同返回语义。
#[test]
pub fn TestBalanceBatchCopTaskWithEmptyTaskSet(t *testing.T) {
	{
		var nilTaskSet []*batchCopTask
		nilResult := balanceBatchCopTask(nil, nilTaskSet, false, 0, nil)
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.True(t, nilResult == nil)
	}

	{
		emptyTaskSet := make([]*batchCopTask, 0)
		emptyResult := balanceBatchCopTask(nil, emptyTaskSet, false, 0, nil)
		require.True(t, emptyResult != nil)
		require.True(t, len(emptyResult) == 0)
	}
}

// TestDeepCopyStoreTaskMap 对应 Go 的同名测试：验证 deep copy 后原 map 与副本的 regionInfos 不相互污染。
#[test]
pub fn TestDeepCopyStoreTaskMap(t *testing.T) {
	storeTasks1 := buildStoreTaskMap(10)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for _, task := range storeTasks1 {
		task.regionInfos = append(task.regionInfos, RegionInfo{})
	}

	storeTasks2 := deepCopyStoreTaskMap(storeTasks1, 0)
	for _, task := range storeTasks2 {
		task.regionInfos = append(task.regionInfos, RegionInfo{})
	}

	for _, task := range storeTasks1 {
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.Equal(t, 1, len(task.regionInfos))
	}

	for _, task := range storeTasks2 {
		require.Equal(t, 2, len(task.regionInfos))
	}
}

// Make sure no duplicated ip:addr.
// 生成随机 ip:port 字符串，保留 Go 测试对地址去重的 fixture 语义。
pub fn generateOneAddr() string {
	var ip string
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for i := range 4 {
		// 条件分支保留 Go 的边界判断；Rust 接线时需确认 nil/空切片语义。
		if i != 0 {
			ip += "."
		}
		ip += strconv.Itoa(rand.Intn(255))
	}
	return ip + ":" + strconv.Itoa(rand.Intn(65535))
}

// 循环生成不重复地址集合，Rust 接线时需要显式集合与随机源。
pub fn generateDifferentAddrs(num int) (res []string) {
	addrMap := make(map[string]struct{})
	for len(addrMap) < num {
		addr := generateOneAddr()
		// 条件分支保留 Go 的边界判断；Rust 接线时需确认 nil/空切片语义。
		if _, ok := addrMap[addr]; !ok {
			addrMap[addr] = struct{}{}
		}
	}
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for addr := range addrMap {
		res = append(res, addr)
	}
	return
}

// TestConsistentHash 对应 Go 的同名测试：验证一致性哈希在 compute node 打乱顺序后仍给 storage node 稳定分配。
#[test]
pub fn TestConsistentHash(t *testing.T) {
	allAddrs := generateDifferentAddrs(100)

	computeNodes := allAddrs[:30]
	storageNodes := allAddrs[30:]
	firstRoundMap := make(map[string]string)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for round := range 100 {
		hasher := consistent.New()
		rand.Shuffle(len(computeNodes), func(i, j int) {
			computeNodes[i], computeNodes[j] = computeNodes[j], computeNodes[i]
		})
		for _, computeNode := range computeNodes {
			hasher.Add(computeNode)
		}
		for _, storageNode := range storageNodes {
			computeNode, err := hasher.Get(storageNode)
			// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
			require.NoError(t, err)
			// 条件分支保留 Go 的边界判断；Rust 接线时需确认 nil/空切片语义。
			if round == 0 {
				firstRoundMap[storageNode] = computeNode
			} else {
				firstRoundAddr, ok := firstRoundMap[storageNode]
				require.True(t, ok)
				require.Equal(t, firstRoundAddr, computeNode)
			}
		}
	}
}

// TestDispatchPolicyRR 对应 Go 的同名测试：验证 TiFlash compute round-robin 分配的覆盖数和近似均匀性。
#[test]
pub fn TestDispatchPolicyRR(t *testing.T) {
	allAddrs := generateDifferentAddrs(100)
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for range 100 {
		regCnt := rand.Intn(10000)
		regIDs := make([]tikv.RegionVerID, 0, regCnt)
		for i := range regCnt {
			regIDs = append(regIDs, tikv.NewRegionVerID(uint64(i), 0, 0))
		}

		rpcCtxs, err := getTiFlashComputeRPCContextByRoundRobin(regIDs, allAddrs)
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.NoError(t, err)
		require.Equal(t, len(rpcCtxs), len(regIDs))
		checkMap := make(map[string]int, len(rpcCtxs))
		for _, c := range rpcCtxs {
			// 条件分支保留 Go 的边界判断；Rust 接线时需确认 nil/空切片语义。
			if v, ok := checkMap[c.Addr]; !ok {
				checkMap[c.Addr] = 1
			} else {
				checkMap[c.Addr] = v + 1
			}
		}
		actCnt := 0
		for _, v := range checkMap {
			actCnt += v
		}
		require.Equal(t, regCnt, actCnt)
		if len(regIDs) < len(allAddrs) {
			require.Equal(t, len(regIDs), len(checkMap))
			exp := -1
			for _, v := range checkMap {
				if exp == -1 {
					exp = v
				} else {
					require.Equal(t, exp, v)
				}
			}
		} else {
			// Using RR, it means region cnt for each tiflash_compute node should be almost same.
			minV := regCnt
			for _, v := range checkMap {
				if v < minV {
					minV = v
				}
			}
			for k, v := range checkMap {
				checkMap[k] = v - minV
			}
			for _, v := range checkMap {
				require.True(t, v == 0 || v == 1)
			}
		}
	}
}

// TestTopoFetcherBackoff 对应 Go 的同名测试：验证 topo fetch backoff 的耗时边界，保留 Go 对毫秒配置与秒级等待的断言。
#[test]
pub fn TestTopoFetcherBackoff(t *testing.T) {
	// context/backoff 控制取消、重试与超时语义；Rust 接线时需映射到异步上下文或取消令牌。
	fetchTopoBo := backoff.NewBackofferWithVars(context.Background(), fetchTopoMaxBackoff, nil)
	expectErr := errors.New("Cannot find proper topo from AutoScaler")
	var retryNum int
	// 时间相关断言保留 Go 的等待/耗时边界；不实际睡眠或计时。
	start := time.Now()
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for {
		retryNum++
		// 条件分支保留 Go 的边界判断；Rust 接线时需确认 nil/空切片语义。
		if err := fetchTopoBo.Backoff(tikv.BoTiFlashRPC(), expectErr); err != nil {
			break
		}
		logutil.BgLogger().Info("TestTopoFetcherBackoff", zap.Int("retryNum", retryNum))
	}
	dura := time.Since(start)
	// fetchTopoMaxBackoff is milliseconds.
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.GreaterOrEqual(t, dura, time.Duration(fetchTopoMaxBackoff*1000))
	require.GreaterOrEqual(t, dura, 30*time.Second)
	require.LessOrEqual(t, dura, 50*time.Second)
}

// TestGetAllUsedTiFlashStores 对应 Go 的同名测试：用 mock RegionCache 验证只筛出已使用 TiFlash store。
#[test]
pub fn TestGetAllUsedTiFlashStores(t *testing.T) {
	// mock TiKV/Store/region fixture 是外部测试 harness；这里保留构造顺序和 region/bucket 边界。
	mockClient, _, pdClient, err := testutils.NewMockTiKV("", nil)
	// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
	require.NoError(t, err)
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// Close 调用是测试资源释放点；当前不会真正打开连接或后台任务。
		pdClient.Close()
		err = mockClient.Close()
		require.NoError(t, err)
	}()

	pdCli := tikv.NewCodecPDClient(tikv.ModeTxn, pdClient)
	defer pdCli.Close()

	cache := NewRegionCache(tikv.NewRegionCache(pdCli))
	defer cache.Close()

	label1 := metapb.StoreLabel{Key: tikvrpc.EngineLabelKey, Value: tikvrpc.EngineLabelTiFlash}
	label2 := metapb.StoreLabel{Key: tikvrpc.EngineRoleLabelKey, Value: tikvrpc.EngineLabelTiFlashCompute}

	cache.SetRegionCacheStore(1, "192.168.1.1", "", tikvrpc.TiFlash, 1, []*metapb.StoreLabel{&label1, &label2})
	cache.SetRegionCacheStore(2, "192.168.1.2", "192.168.1.3", tikvrpc.TiFlash, 1, []*metapb.StoreLabel{&label1, &label2})
	cache.SetRegionCacheStore(3, "192.168.1.3", "192.168.1.2", tikvrpc.TiFlash, 1, []*metapb.StoreLabel{&label1, &label2})

	allUsedTiFlashStoresMap := make(map[uint64]struct{})
	allUsedTiFlashStoresMap[2] = struct{}{}
	allUsedTiFlashStoresMap[3] = struct{}{}
	allTiFlashStores := cache.RegionCache.GetTiFlashStores(tikv.LabelFilterNoTiFlashWriteNode)
	require.Equal(t, 3, len(allTiFlashStores))
	allUsedTiFlashStores := getAllUsedTiFlashStores(allTiFlashStores, allUsedTiFlashStoresMap)
	require.Equal(t, len(allUsedTiFlashStoresMap), len(allUsedTiFlashStores))
	// 循环结构按 Go 覆盖面保留，尤其是随机/表驱动/重试场景。
	for _, store := range allUsedTiFlashStores {
		_, ok := allUsedTiFlashStoresMap[store.StoreID()]
		require.True(t, ok)
	}
}

// BenchmarkBalanceBatchCopTaskWithContinuity 对应 Go benchmark，保留 StopTimer/StartTimer 与循环主体，当前不接 Rust benchmark。
// Rust stable 没有直接等价的 Go testing.B；这里保留 benchmark 入口形状。
pub fn BenchmarkBalanceBatchCopTaskWithContinuity(b *testing.B) {
	b.StopTimer()
	replicaNum := 3
	storeCount := 10
	regionCount := 200000
	storeTasks := buildStoreTaskMap(storeCount)
	regionInfos := buildRegionInfos(storeCount, regionCount, replicaNum)

	b.StartTimer()
	for i := 0; i < b.N; i++ {
		_, _ = balanceBatchCopTaskWithContinuity(storeTasks, regionInfos, 20)
	}
}

// TestAliveStoreSkipCheck 对应 Go 的同名测试：覆盖 closest/all replicas 策略下 alive store 检查能否跳过的组合矩阵。
#[test]
pub fn TestAliveStoreSkipCheck(t *testing.T) {
	// failpoint 用于注入测试分支；只保留注入点路径和启停顺序，不实际修改全局 failpoint。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/copr/mockNoAliveTiFlash", `return(false)`))
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/copr/mockNoAliveTiFlash"))
	}()

	usedTiFlashStoresMap := map[uint64]struct{}{
		1: {},
		2: {},
		3: {},
	}

	{
		// Non closest_replica; min replica num is 1.
		usedTiFlashStores := [][]uint64{
			{1, 2}, // region-1
			{2, 3}, // region-2
			{1},    // region-3
		}
		aliveStores := &aliveStoresBundle{
			storeIDsInAllZones: map[uint64]struct{}{
				1: {},
				2: {},
				3: {},
			},
			storeIDsInTiDBZone: map[uint64]struct{}{
				1: {},
			},
		}
		// 1, 2, 3 is alive, can skip check.
		require.True(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestAdaptive, 2, 1))
		require.True(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.AllReplicas, 2, 1))

		// 1, 2 is alive, cannot skip check.
		aliveStores.storeIDsInAllZones = map[uint64]struct{}{
			1: {},
			2: {},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestAdaptive, 2, 1))
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.AllReplicas, 2, 1))
	}

	{
		// Non closest_replica; min replica num is 2.
		usedTiFlashStores := [][]uint64{
			{1, 2}, // region-1
			{2, 3}, // region-2
			{1, 3}, // region-3
		}
		// 1, 2, 3 is alive, can skip check.
		aliveStores := &aliveStoresBundle{
			storeIDsInAllZones: map[uint64]struct{}{
				1: {},
				2: {},
				3: {},
			},
			storeIDsInTiDBZone: map[uint64]struct{}{
				1: {},
				2: {},
				3: {},
			},
		}
		require.True(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestAdaptive, 2, 2))
		require.True(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.AllReplicas, 2, 2))

		// 1, 2 is alive, can skip check.
		aliveStores.storeIDsInAllZones = map[uint64]struct{}{
			1: {},
			2: {},
		}
		require.True(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestAdaptive, 2, 2))
		require.True(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.AllReplicas, 2, 2))
	}

	{
		// closest_replica(always need check). min replica num is 1.
		usedTiFlashStores := [][]uint64{
			{1, 2}, // region-1
			{2, 3}, // region-2
			{1},    // region-3
		}
		// 1 is alive, cannot skip check.
		aliveStores := &aliveStoresBundle{
			storeIDsInAllZones: map[uint64]struct{}{
				1: {},
				2: {},
				3: {},
			},
			storeIDsInTiDBZone: map[uint64]struct{}{
				1: {},
			},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestReplicas, 2, 1))

		// 1, 2 is alive, cannot skip check.
		aliveStores.storeIDsInTiDBZone = map[uint64]struct{}{
			1: {},
			2: {},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestReplicas, 2, 1))

		// 1, 2, 3 is alive, can skip check.
		aliveStores.storeIDsInTiDBZone = map[uint64]struct{}{
			1: {},
			2: {},
			3: {},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestReplicas, 2, 1))
	}

	{
		// closest_replica. min replica num is 2.
		usedTiFlashStores := [][]uint64{
			{1, 2}, // region-1
			{2, 3}, // region-2
			{1, 3}, // region-3
		}

		// 1 is alive, cannot skip check.
		aliveStores := &aliveStoresBundle{
			storeIDsInAllZones: map[uint64]struct{}{
				1: {},
				2: {},
				3: {},
			},
			storeIDsInTiDBZone: map[uint64]struct{}{
				1: {},
			},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestReplicas, 2, 2))

		// 1, 2 is alive, cannot skip check.
		aliveStores.storeIDsInTiDBZone = map[uint64]struct{}{
			1: {},
			2: {},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestReplicas, 2, 2))

		// 1, 2, 3 is alive, can skip check.
		aliveStores.storeIDsInTiDBZone = map[uint64]struct{}{
			1: {},
			2: {},
			3: {},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestReplicas, 2, 2))

		// 1, 2 is alive, can skip check.
		usedTiFlashStores = [][]uint64{
			{1, 2}, // region-1
			{1, 2}, // region-2
			{1, 2}, // region-3
		}
		aliveStores.storeIDsInTiDBZone = map[uint64]struct{}{
			1: {},
			2: {},
		}
		require.False(t, canSkipCheckAliveStores(aliveStores, usedTiFlashStores, usedTiFlashStoresMap, tiflash.ClosestReplicas, 2, 2))
	}
}

// TestCheckAliveStore 对应 Go 的同名测试：覆盖 TiFlash alive store 检查返回 retry 与 invalid region 列表的不同场景。
#[test]
pub fn TestCheckAliveStore(t *testing.T) {
	// failpoint 用于注入测试分支；只保留注入点路径和启停顺序，不实际修改全局 failpoint。
	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/store/copr/mockNoAliveTiFlash", `return(false)`))
	// Go defer 表示作用域退出时资源收尾；Rust 接线时需要 Drop、scope guard 或显式 finally 等价处理。
	defer func() {
		// require 断言保留 Go 期望值顺序；当前未接入 Rust 断言宏。
		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/store/copr/mockNoAliveTiFlash"))
	}()
	aliveStores := &aliveStoresBundle{
		storeIDsInAllZones: map[uint64]struct{}{
			1: {},
			2: {},
			3: {},
		},
		storeIDsInTiDBZone: map[uint64]struct{}{
			1: {},
		},
	}

	usedTiFlashStoresMap := map[uint64]struct{}{
		1: {},
		2: {},
		3: {},
	}

	usedTiFlashStores := [][]uint64{
		{1, 2}, // region-1
		{2, 3}, // region-2
		{3},    // region-3
	}

	tasks := []*copTask{
		{
			region: tikv.NewRegionVerID(1, 1, 1),
		},
		{
			region: tikv.NewRegionVerID(2, 2, 2),
		},
		{
			region: tikv.NewRegionVerID(3, 3, 3),
		},
	}

	var minReplicaNum uint64
	var maxAllowedRemote int
	{
		// Test closest_replica. 2 remote region, 1 tidb zone region.
		maxAllowedRemote = 1
		needRetry, invalidRegions := checkAliveStore(aliveStores, usedTiFlashStores, usedTiFlashStoresMap,
			nil, tiflash.ClosestReplicas, 2, tasks, minReplicaNum, maxAllowedRemote)
		require.True(t, needRetry)
		require.Equal(t, 2, len(invalidRegions))
	}
	{
		// Test closest_replica. 2 remote region, 1 tidb zone region.
		maxAllowedRemote = 3
		needRetry, invalidRegions := checkAliveStore(aliveStores, usedTiFlashStores, usedTiFlashStoresMap,
			nil, tiflash.ClosestReplicas, 2, tasks, minReplicaNum, maxAllowedRemote)
		require.False(t, needRetry)
		require.Equal(t, 0, len(invalidRegions))
	}
	{
		// Test non closest_replica.
		needRetry, invalidRegions := checkAliveStore(aliveStores, usedTiFlashStores, usedTiFlashStoresMap,
			nil, tiflash.ClosestReplicas, 2, tasks, minReplicaNum, maxAllowedRemote)
		require.False(t, needRetry)
		require.Equal(t, 0, len(invalidRegions))
	}
	{
		// Test non closest_replica.
		aliveStores := &aliveStoresBundle{
			storeIDsInAllZones: map[uint64]struct{}{
				1: {},
				2: {},
			},
			storeIDsInTiDBZone: map[uint64]struct{}{
				1: {},
			},
		}
		needRetry, invalidRegions := checkAliveStore(aliveStores, usedTiFlashStores, usedTiFlashStoresMap,
			nil, tiflash.ClosestReplicas, 2, tasks, minReplicaNum, maxAllowedRemote)
		require.True(t, needRetry)
		require.Equal(t, 1, len(invalidRegions))
	}
	{
		aliveStores := &aliveStoresBundle{
			storeIDsInAllZones: map[uint64]struct{}{},
			storeIDsInTiDBZone: map[uint64]struct{}{},
		}
		needRetry, invalidRegions := checkAliveStore(aliveStores, usedTiFlashStores, usedTiFlashStoresMap,
			nil, tiflash.ClosestReplicas, 2, tasks, minReplicaNum, maxAllowedRemote)
		require.True(t, needRetry)
		require.Equal(t, 3, len(invalidRegions))
	}
}
"################;

use std::collections::{HashMap, HashSet};

use crate::batch_coprocessor::prefer_contiguous_tasks;
use crate::batch_request_sender::Store as RegionStore;
use crate::{
    AliveStoresBundle, Backoffer, BatchCopTask, BatchError, CommandType, KeyRange, KeyRanges,
    RegionInfo, RegionVerId, ReplicaReadPolicy, balance_batch_cop_task,
    balance_batch_cop_task_with_continuity, can_skip_check_alive_stores, check_alive_store,
    deep_copy_store_task_map, get_all_used_tiflash_stores,
    get_tiflash_compute_rpc_context_by_consistent_hash,
    get_tiflash_compute_rpc_context_by_round_robin, is_balance,
};

fn store(id: u64) -> RegionStore {
    RegionStore {
        id,
        address: format!("tiflash-{id}"),
        ..RegionStore::default()
    }
}

fn region(id: u64, stores: Vec<u64>) -> RegionInfo {
    RegionInfo {
        Region: RegionVerId::new(id, 1, 1),
        Ranges: KeyRanges::new(vec![KeyRange {
            start: id.to_be_bytes().to_vec(),
            end: id.saturating_add(1).to_be_bytes().to_vec(),
        }]),
        AllStores: stores,
        ..RegionInfo::default()
    }
}

#[test]
fn balance_batch_cop_task_with_continuity_matches_go() {
    for replica_count in 1..=5 {
        let stores: HashMap<_, _> = (0..10)
            .map(|id| {
                (
                    id,
                    BatchCopTask {
                        storeAddr: format!("tiflash-{id}"),
                        cmdType: CommandType::BatchCop,
                        ..BatchCopTask::default()
                    },
                )
            })
            .collect();
        let regions: Vec<_> = (0..100_000)
            .map(|id| {
                region(
                    id,
                    (0..replica_count)
                        .map(|offset| (id + offset) % 10)
                        .collect(),
                )
            })
            .collect();
        let (tasks, score) = balance_batch_cop_task_with_continuity(&stores, &regions, 20);
        let tasks = tasks.expect("large candidate set must use continuity balancing");
        assert!(
            is_balance(score),
            "replica count {replica_count}, score {score}"
        );
        assert_eq!(
            tasks.iter().map(BatchCopTask::region_count).sum::<usize>(),
            100_000
        );
    }
    let stores = HashMap::from([(1, BatchCopTask::default())]);
    assert!(
        balance_batch_cop_task_with_continuity(&stores, &[region(1, vec![1])], 20)
            .0
            .is_none()
    );
}

#[test]
fn balance_batch_cop_task_preserves_empty_task_set() {
    let result = balance_batch_cop_task(&[], Vec::new(), false, 0, Vec::new());
    assert!(result.is_empty());
}

#[test]
fn unbalanced_greedy_result_always_falls_back_to_contiguous_tasks_like_go() {
    let contiguous = vec![BatchCopTask {
        storeAddr: "contiguous".to_owned(),
        ..BatchCopTask::default()
    }];
    let selected = prefer_contiguous_tasks(80, Some(contiguous))
        .expect("Go selects an available continuity result whenever greedy is unbalanced");
    assert_eq!(selected[0].storeAddr, "contiguous");
}

#[test]
fn deep_copy_store_task_map_does_not_alias_regions() {
    let original = HashMap::from([(
        1,
        BatchCopTask {
            regionInfos: vec![region(1, vec![1])],
            ..BatchCopTask::default()
        },
    )]);
    let mut copied = deep_copy_store_task_map(&original, 10);
    copied
        .get_mut(&1)
        .expect("copied store")
        .regionInfos
        .push(region(2, vec![1]));
    assert_eq!(original[&1].regionInfos.len(), 1);
    assert_eq!(copied[&1].regionInfos.len(), 2);
}

#[test]
fn consistent_hash_is_independent_of_compute_node_order() {
    let ids: Vec<_> = (0..200).map(|id| RegionVerId::new(id, 1, 1)).collect();
    let stores = vec![
        "compute-0".to_owned(),
        "compute-1".to_owned(),
        "compute-2".to_owned(),
    ];
    let expected =
        get_tiflash_compute_rpc_context_by_consistent_hash(&ids, &stores).expect("dispatch");
    let mut reversed = stores.clone();
    reversed.reverse();
    let actual =
        get_tiflash_compute_rpc_context_by_consistent_hash(&ids, &reversed).expect("dispatch");
    assert_eq!(
        expected
            .iter()
            .map(|context| &context.address)
            .collect::<Vec<_>>(),
        actual
            .iter()
            .map(|context| &context.address)
            .collect::<Vec<_>>()
    );
    assert!(get_tiflash_compute_rpc_context_by_consistent_hash(&ids, &[]).is_err());
}

#[test]
fn round_robin_dispatch_is_complete_and_balanced() {
    let ids: Vec<_> = (0..1_003).map(|id| RegionVerId::new(id, 1, 1)).collect();
    let stores = vec![
        "compute-0".to_owned(),
        "compute-1".to_owned(),
        "compute-2".to_owned(),
    ];
    let contexts = get_tiflash_compute_rpc_context_by_round_robin(&ids, &stores).expect("dispatch");
    assert_eq!(contexts.len(), ids.len());
    let mut counts = HashMap::new();
    for context in contexts {
        *counts.entry(context.address).or_insert(0usize) += 1;
    }
    assert_eq!(counts.len(), stores.len());
    assert!(counts.values().max().expect("maximum") - counts.values().min().expect("minimum") <= 1);
}

#[test]
fn topology_backoff_exhausts_after_configured_attempts() {
    let mut backoffer = Backoffer::new(2);
    let error = BatchError::NoAliveStore("Cannot find proper topo from AutoScaler".to_owned());
    assert!(backoffer.backoff(&error).is_ok());
    assert!(backoffer.backoff(&error).is_ok());
    assert!(matches!(
        backoffer.backoff(&error),
        Err(BatchError::BackoffExhausted(_))
    ));
    assert_eq!(backoffer.history.len(), 3);
}

#[test]
fn get_all_used_tiflash_stores_filters_by_store_id() {
    let all = vec![store(1), store(2), store(3)];
    let used = HashSet::from([2, 3]);
    assert_eq!(
        get_all_used_tiflash_stores(&all, &used)
            .into_iter()
            .map(|store| store.id)
            .collect::<HashSet<_>>(),
        used
    );
}

#[test]
fn alive_store_skip_check_matches_policy_and_replica_count() {
    let used = HashSet::from([1, 2, 3]);
    let mut alive = AliveStoresBundle {
        store_ids_in_all_zones: HashSet::from([1, 2, 3]),
        store_ids_in_tidb_zone: HashSet::from([1]),
        ..AliveStoresBundle::default()
    };
    assert!(can_skip_check_alive_stores(
        &alive,
        &used,
        ReplicaReadPolicy::AllReplicas,
        1,
        false
    ));
    assert!(!can_skip_check_alive_stores(
        &alive,
        &used,
        ReplicaReadPolicy::ClosestReplicas,
        3,
        false
    ));
    alive.store_ids_in_all_zones.remove(&3);
    assert!(!can_skip_check_alive_stores(
        &alive,
        &used,
        ReplicaReadPolicy::AllReplicas,
        1,
        false
    ));
    assert!(can_skip_check_alive_stores(
        &alive,
        &used,
        ReplicaReadPolicy::AllReplicas,
        2,
        false
    ));
    assert!(can_skip_check_alive_stores(
        &alive,
        &used,
        ReplicaReadPolicy::ClosestReplicas,
        0,
        true
    ));
}

#[test]
fn check_alive_store_returns_retry_and_invalid_regions() {
    let ids = vec![
        RegionVerId::new(1, 1, 1),
        RegionVerId::new(2, 1, 1),
        RegionVerId::new(3, 1, 1),
    ];
    let per_region = vec![vec![1, 2], vec![2, 3], vec![3]];
    let used = HashSet::from([1, 2, 3]);
    let alive = AliveStoresBundle {
        store_ids_in_all_zones: HashSet::from([1, 2, 3]),
        store_ids_in_tidb_zone: HashSet::from([1]),
        ..AliveStoresBundle::default()
    };
    let (retry, invalid) = check_alive_store(
        &alive,
        &per_region,
        &used,
        ReplicaReadPolicy::ClosestReplicas,
        &ids,
        0,
        1,
        false,
    );
    assert!(retry);
    assert_eq!(invalid, vec![ids[1], ids[2]]);

    let (retry, invalid) = check_alive_store(
        &alive,
        &per_region,
        &used,
        ReplicaReadPolicy::ClosestReplicas,
        &ids,
        0,
        3,
        false,
    );
    assert!(!retry);
    assert!(invalid.is_empty());

    let partly_alive = AliveStoresBundle {
        store_ids_in_all_zones: HashSet::from([1, 2]),
        store_ids_in_tidb_zone: HashSet::from([1]),
        ..AliveStoresBundle::default()
    };
    let (retry, invalid) = check_alive_store(
        &partly_alive,
        &per_region,
        &used,
        ReplicaReadPolicy::ClosestReplicas,
        &ids,
        0,
        3,
        false,
    );
    assert!(retry);
    assert_eq!(invalid, vec![ids[2]]);

    let no_alive = AliveStoresBundle::default();
    let (retry, invalid) = check_alive_store(
        &no_alive,
        &per_region,
        &used,
        ReplicaReadPolicy::AllReplicas,
        &ids,
        0,
        3,
        false,
    );
    assert!(retry);
    assert_eq!(invalid, ids);
}
