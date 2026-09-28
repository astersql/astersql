// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 优先队列 DDL 事件处理（queue_ddl_handler）相关单元测试。
//
// 上方注释保留 Go 集成测试（加索引、截断/删表分区、DropSchema 与 running job 交互）；
// 可执行部分校验：队列未初始化时，开启 auto-analyze 返回可重试错误，关闭则静默忽略。

// TestKit、require、failpoint、DDL notifier、sessionctx、goroutine/channel 等外部依赖均按 Go 调用形状保留为后续接线点。
//
// enableAutoAnalyze enables auto-analyze for the test and restores it on cleanup.
// In tests, auto-analyze is disabled by default, so tests that need it enabled
// should call this helper at the beginning.
// enableAutoAnalyze 对应 Go 测试辅助函数：临时打开 auto analyze，并在测试清理阶段恢复全局变量。
// pub fn enableAutoAnalyze(t: &testing::T, tk: *testkit::TestKit) {
// 	bakRunAutoAnalyze := vardef.RunAutoAnalyze.Load()
// auto analyze 开关影响队列初始化和后台分析执行。
// 	tk.MustExec("set @@global.tidb_enable_auto_analyze = 1")
// 测试清理回调保留 Go 的资源收尾语义，确保全局变量在用例结束后恢复。
// 	t.Cleanup(func() {
// auto analyze 开关影响队列初始化和后台分析执行。
// 		tk.MustExec(fmt.Sprintf("set @@global.tidb_enable_auto_analyze = %t", bakRunAutoAnalyze))
// 	})
// }
//
// TestHandleDDLEventsWithRunningJobs 对应 Go 测试函数：保留 Test Handle D D L Events With Running Jobs 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestHandleDDLEventsWithRunningJobs(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	enableAutoAnalyze(t, tk)
// 	tk.MustExec("create table t1 (a int)")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("insert into t1 values (1)")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	ctx := context.Background()
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
// 	schema := ast.NewCIStr("test")
// 	tbl1, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t1"))
// 	require.NoError(t, err)
// 	tk.MustExec("analyze table t1")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(ctx))
//
// Check there are no running jobs.
// 	runningJobs := pq.GetRunningJobs()
// 	require.Len(t, runningJobs, 0)
// Check no jobs are in the queue.
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
//
// Insert 10 rows into t1.
// 	tk.MustExec("insert into t1 values (2), (3)")
// Flush the stats delta.
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Pop the t1 job.
// 	job1, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.Equal(t, tbl1.Meta().ID, job1.GetTableID())
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, _ := job1.ValidateAndPrepare(tk.Session())
// 	require.True(t, valid)
//
// Check if the running job is still in the queue.
// 	runningJobs = pq.GetRunningJobs()
// 	require.Len(t, runningJobs, 1)
//
// channel 用于跨 goroutine 同步完成信号，暂不替换为具体同步原语。
// 	down := make(chan struct{})
// 	fp := "github.com/pingcap/tidb/pkg/executor/mockStuckAnalyze"
// goroutine 表示并发执行路径；这里保留异步分析或关闭队列的原始时序。
// 	go func() {
// defer handles cleanup in Go.
// 		defer close(down)
// failpoint 用于模拟执行阻塞或异常路径，不实际启用外部注入点。
// 		require.NoError(t, failpoint.Enable(fp, "return(1)"))
// Analyze 会执行真实 ANALYZE 路径；这里只保留测试对统计结果的预期。
// 		require.NoError(t, job1.Analyze(handle, dom.SysProcTracker()))
// 	}()
//
// Create a new index on t1.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	tk.MustExec("alter table t1 add index idx (a)")
//
// Find the add index event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	addIndexEvent := statstestutil.FindEvent(handle.DDLEventCh(), model.ActionAddIndex)
//
// Handle the add index event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(handle, addIndexEvent)
// 	require.NoError(t, err)
//
// Handle the add index event in priority queue.
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(handle.SPool(), func(sctx sessionctx.Context) error {
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 		return pq.HandleDDLEvent(ctx, sctx, addIndexEvent)
// 	}, statsutil.FlagWrapTxn))
//
// Check the queue is empty.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
//
// Requeue the running jobs.
// must-retry 重入队逻辑影响失败任务是否重新进入优先队列。
// 	pq.RequeueMustRetryJobs()
//
// Still no jobs in the queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
//
// failpoint 用于模拟执行阻塞或异常路径，不实际启用外部注入点。
// 	require.NoError(t, failpoint.Disable(fp))
// Wait for the analyze job to finish.
// 	<-down
//
// Requeue the running jobs again.
// must-retry 重入队逻辑影响失败任务是否重新进入优先队列。
// 	pq.RequeueMustRetryJobs()
//
// Check the job is in the queue.
// 	job, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.Equal(t, tbl1.Meta().ID, job.GetTableID())
// 	require.True(t, job.HasNewlyAddedIndex())
// }
//
// TestTruncateTable 对应 Go 测试函数：保留 Test Truncate Table 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestTruncateTable(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(h)
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2),(6,2),(11,2),(16,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Truncate table.
// truncate 会替换表或分区 ID，队列中旧 job 应被移除。
// 	testKit.MustExec("truncate table t")
//
// Find the truncate table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	truncateTableEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionTruncateTable)
//
// Handle the truncate table event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, truncateTableEvent)
// 	require.NoError(t, err)
//
// 	sctx := testKit.Session().(sessionctx.Context)
// Handle the truncate table event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	require.NoError(t, pq.HandleDDLEvent(ctx, sctx, truncateTableEvent))
//
// The table is truncated, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestTruncatePartitionedTableWithStaticPartition 对应 Go 测试函数：保留 Test Truncate Partitioned Table With Static Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestTruncatePartitionedTableWithStaticPartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	testKit.MustExec("set global tidb_partition_prune_mode='static'")
// 	testTruncatePartitionedTable(t, do, testKit)
// }
//
// TestTruncatePartitionedTableWithDynamicPartition 对应 Go 测试函数：保留 Test Truncate Partitioned Table With Dynamic Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestTruncatePartitionedTableWithDynamicPartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	testKit.MustExec("set global tidb_partition_prune_mode='dynamic'")
// 	testTruncatePartitionedTable(t, do, testKit)
// }
//
// testTruncatePartitionedTable 对应 Go 辅助函数：按来源测试复用同一组 fixture、参数和断言路径。
// pub fn testTruncatePartitionedTable(
// 	t *testing.T,
// 	do *domain.Domain,
// 	testKit *testkit.TestKit,
// ) {
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range (c1) (partition p0 values less than (10), partition p1 values less than (20))")
// 	h := do.StatsHandle()
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2),(6,2),(11,2),(16,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
//
// Truncate table.
// truncate 会替换表或分区 ID，队列中旧 job 应被移除。
// 	testKit.MustExec("truncate table t")
//
// Find the truncate table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	truncateTableEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionTruncateTable)
//
// Handle the truncate table event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, truncateTableEvent)
// 	require.NoError(t, err)
//
// 	sctx := testKit.Session().(sessionctx.Context)
// Handle the truncate table event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	require.NoError(t, pq.HandleDDLEvent(ctx, sctx, truncateTableEvent))
//
// The table is truncated, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestDropTable 对应 Go 测试函数：保留 Test Drop Table 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestDropTable(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(h)
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2),(6,2),(11,2),(16,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Drop table.
// drop table 事件用于验证队列能清理不存在对象的 job。
// 	testKit.MustExec("drop table t")
//
// Find the drop table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	dropTableEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionDropTable)
//
// Handle the drop table event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, dropTableEvent)
// 	require.NoError(t, err)
//
// 	sctx := testKit.Session().(sessionctx.Context)
// Handle the drop table event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	require.NoError(t, pq.HandleDDLEvent(ctx, sctx, dropTableEvent))
//
// The table is dropped, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestDropPartitionedTableWithStaticPartition 对应 Go 测试函数：保留 Test Drop Partitioned Table With Static Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestDropPartitionedTableWithStaticPartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	testKit.MustExec("set global tidb_partition_prune_mode='static'")
// 	testDropPartitionedTable(t, do, testKit)
// }
//
// TestDropPartitionedTableWithDynamicPartition 对应 Go 测试函数：保留 Test Drop Partitioned Table With Dynamic Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestDropPartitionedTableWithDynamicPartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	testKit.MustExec("set global tidb_partition_prune_mode='dynamic'")
// 	testDropPartitionedTable(t, do, testKit)
// }
//
// testDropPartitionedTable 对应 Go 辅助函数：按来源测试复用同一组 fixture、参数和断言路径。
// pub fn testDropPartitionedTable(
// 	t *testing.T,
// 	do *domain.Domain,
// 	testKit *testkit.TestKit,
// ) {
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range (c1) (partition p0 values less than (10), partition p1 values less than (20))")
// 	h := do.StatsHandle()
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2),(6,2),(11,2),(16,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
//
// Drop table.
// drop table 事件用于验证队列能清理不存在对象的 job。
// 	testKit.MustExec("drop table t")
//
// Find the drop table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	dropTableEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionDropTable)
//
// Handle the drop table event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, dropTableEvent)
// 	require.NoError(t, err)
//
// 	sctx := testKit.Session().(sessionctx.Context)
// Handle the drop table event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	require.NoError(t, pq.HandleDDLEvent(ctx, sctx, dropTableEvent))
//
// The table is dropped, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestTruncateTablePartition 对应 Go 测试函数：保留 Test Truncate Table Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestTruncateTablePartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range (c1) (partition p0 values less than (10), partition p1 values less than (20))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Truncate table partition.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t truncate partition p0")
//
// Find the truncate table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	truncateTablePartitionEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionTruncateTablePartition)
//
// Handle the truncate table partition event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, truncateTablePartitionEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// Handle the truncate table partition event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, truncateTablePartitionEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// The table partition is truncated, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestDropTablePartition 对应 Go 测试函数：保留 Test Drop Table Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestDropTablePartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range (c1) (partition p0 values less than (10), partition p1 values less than (20))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Drop table partition.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t drop partition p0")
//
// Find the drop table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	dropTablePartitionEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionDropTablePartition)
//
// Handle the drop table partition event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, dropTablePartitionEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// Handle the drop table partition event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, dropTablePartitionEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// The table partition is dropped, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestExchangeTablePartition 对应 Go 测试函数：保留 Test Exchange Table Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestExchangeTablePartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t1 (c1 int, c2 int, index idx(c1, c2)) partition by range (c1) (partition p0 values less than (10), partition p1 values less than (20))")
// 	testKit.MustExec("create table t2 (c1 int, c2 int, index idx(c1, c2))")
// 	is := do.InfoSchema()
// 	tbl1, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t1"))
// 	require.NoError(t, err)
// 	tableInfo1 := tbl1.Meta()
// 	tbl2, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t2"))
// 	require.NoError(t, err)
// 	tableInfo2 := tbl2.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t1")
// 	testKit.MustExec("analyze table t2")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t1 values (1,2),(2,2),(3,3),(4,4)")
// 	testKit.MustExec("insert into t2 values (1,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo2.ID, job.GetTableID())
//
// Exchange table partition.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t1 exchange partition p0 with table t2")
//
// Find the exchange table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	exchangeTablePartitionEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionExchangeTablePartition)
//
// Handle the exchange table partition event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, exchangeTablePartitionEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// Handle the exchange table partition event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, exchangeTablePartitionEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// They are exchanged, the job should be updated to the exchanged table.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err = pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo1.ID, job.GetTableID(), "The job should be updated to the exchanged table")
// }
//
// TestReorganizeTablePartition 对应 Go 测试函数：保留 Test Reorganize Table Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestReorganizeTablePartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range (c1) (partition p0 values less than (10), partition p1 values less than (20))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Reorganize table partition.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t reorganize partition p0 into (partition p0 values less than (5), partition p2 values less than (10))")
//
// Find the reorganize table partition event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	reorganizeTablePartitionEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionReorganizePartition)
//
// Handle the reorganize table partition event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, reorganizeTablePartitionEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// Handle the reorganize table partition event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, reorganizeTablePartitionEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// The table partition is reorganized, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestAlterTablePartitioning 对应 Go 测试函数：保留 Test Alter Table Partitioning 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestAlterTablePartitioning(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Alter table partitioning.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t partition by range columns (c1) (partition p0 values less than (5), partition p1 values less than (10))")
//
// Find the alter table partitioning event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	alterTablePartitioningEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionAlterTablePartitioning)
//
// Handle the alter table partitioning event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, alterTablePartitioningEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// Handle the alter table partitioning event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, alterTablePartitioningEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// The table partitioning is altered, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestRemovePartitioning 对应 Go 测试函数：保留 Test Remove Partitioning 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestRemovePartitioning(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range columns (c1) (partition p0 values less than (5), partition p1 values less than (10))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Remove partitioning.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t remove partitioning")
//
// Find the remove partitioning event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	removePartitioningEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionRemovePartitioning)
//
// Handle the remove partitioning event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, removePartitioningEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// Handle the remove partitioning event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, removePartitioningEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// The table partitioning is removed, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestDropSchemaEventWithDynamicPartition 对应 Go 测试函数：保留 Test Drop Schema Event With Dynamic Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestDropSchemaEventWithDynamicPartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range columns (c1) (partition p0 values less than (5), partition p1 values less than (10))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Create a non-partitioned table.
// 	testKit.MustExec("create table t2 (c1 int, c2 int, index idx(c1, c2))")
// 	testKit.MustExec("analyze table t2")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t2 values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
// 	l, err := pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, l, 2)
//
// Drop schema.
// drop database 会使待分析对象消失，测试验证失败 job 不被无限重试。
// 	testKit.MustExec("drop database test")
//
// Find the drop schema event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	dropSchemaEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionDropSchema)
// 	require.NotNil(t, dropSchemaEvent)
//
// Handle the drop schema event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, dropSchemaEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, dropSchemaEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// The table should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// TestDropSchemaEventWithStaticPartition 对应 Go 测试函数：保留 Test Drop Schema Event With Static Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestDropSchemaEventWithStaticPartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	h := do.StatsHandle()
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range columns (c1) (partition p0 values less than (5), partition p1 values less than (10))")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(h)
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	testKit.MustExec("set global tidb_partition_prune_mode='static'")
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(6,6)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	l, err := pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, l, 2)
//
// Drop schema.
// drop database 会使待分析对象消失，测试验证失败 job 不被无限重试。
// 	testKit.MustExec("drop database test")
//
// Find the drop schema event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	dropSchemaEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionDropSchema)
// 	require.NotNil(t, dropSchemaEvent)
//
// Handle the drop schema event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, dropSchemaEvent)
// 	require.NoError(t, err)
//
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(ctx, sctx, dropSchemaEvent))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
//
// The table should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
// }
//
// 包级常量对应 Go const；时间和 failpoint 相关表达式保持原调用形状。
// pub const tiflashReplicaLease: _ = 600 * time.Millisecond;
//
// TestVectorIndexTriggerAutoAnalyze 对应 Go 测试函数：保留 Test Vector Index Trigger Auto Analyze 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestVectorIndexTriggerAutoAnalyze(t: &testing::T) {
// 	store := testkit.CreateMockStoreWithSchemaLease(t, tiflashReplicaLease, mockstore.WithMockTiFlash(2))
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
//
// 	tiflash := infosync.NewMockTiFlash()
// 	infosync.SetMockTiFlash(tiflash)
// defer handles cleanup in Go.
// 	defer func() {
// 		tiflash.Lock()
// 		tiflash.StatusServer.Close()
// 		tiflash.Unlock()
// 	}()
// 	dom := domain.GetDomain(tk.Session())
// 	h := dom.StatsHandle()
//
// failpoint 用于模拟执行阻塞或异常路径，不实际启用外部注入点。
// 	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/MockCheckColumnarIndexProcess", `return(1)`)
//
// 	tk.MustExec("create table t (a int, b vector, c vector(3), d vector(4));")
// 	tk.MustExec("analyze table t")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	tk.MustExec("alter table t set tiflash replica 1;")
// 	testkit.SetTiFlashReplica(t, dom, "test", "t")
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(context.Background()))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
//
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	tk.MustExec("alter table t add vector index vecIdx1((vec_cosine_distance(d))) USING HNSW;")
//
// 	addIndexEvent := statstestutil.FindEventWithTimeout(h.DDLEventCh(), model.ActionAddColumnarIndex, 1)
// No event is found
// 	require.Nil(t, addIndexEvent)
// }
//
// TestAddIndexTriggerAutoAnalyze 对应 Go 测试函数：保留 Test Add Index Trigger Auto Analyze 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestAddIndexTriggerAutoAnalyze(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("set @@global.tidb_analyze_version=2;")
// defer handles cleanup in Go.
// 	defer testKit.MustExec("set @@global.tidb_analyze_version=default;")
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range columns (c1) (partition p0 values less than (5), partition p1 values less than (10))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Add two indexes.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t add index idx1(c1)")
// 	testKit.MustExec("alter table t add index idx2(c2)")
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(context.Background()))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, _ := job.ValidateAndPrepare(testKit.Session())
// 	require.True(t, valid)
// 	require.NoError(t, job.Analyze(h, do.SysProcTracker()))
//
// Check the stats of the indexes.
// 	tableStats := h.GetPhysicalTableStats(tableInfo.ID, tableInfo)
// 	require.True(t, tableStats.GetIdx(1).IsAnalyzed())
// 	require.True(t, tableStats.GetIdx(2).IsAnalyzed())
// 	require.True(t, tableStats.GetIdx(3).IsAnalyzed())
// }
//
// TestAddIndexTriggerAutoAnalyzeWithStaticPartition 对应 Go 测试函数：保留 Test Add Index Trigger Auto Analyze With Static Partition 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestAddIndexTriggerAutoAnalyzeWithStaticPartition(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// Enable the static partition mode.
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	testKit.MustExec("set @@global.tidb_partition_prune_mode='static'")
// 	testKit.MustExec("set @@global.tidb_analyze_version=2;")
// defer handles cleanup in Go.
// 	defer testKit.MustExec("set @@global.tidb_analyze_version=default;")
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2)) partition by range columns (c1) (partition p0 values less than (5), partition p1 values less than (10))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	p0ID := tableInfo.GetPartitionInfo().Definitions[0].ID
// 	p1ID := tableInfo.GetPartitionInfo().Definitions[1].ID
// 	h := do.StatsHandle()
// Analyze table.
// 	testKit.MustExec("analyze table t")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(context.Background()))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
//
// Add two indexes.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t add index idx1(c1)")
// 	testKit.MustExec("alter table t add index idx2(c2)")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	addIndexEvent1 := statstestutil.FindEvent(h.DDLEventCh(), model.ActionAddIndex)
// 	require.NotNil(t, addIndexEvent1)
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(context.Background(), sctx, addIndexEvent1))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	addIndexEvent2 := statstestutil.FindEvent(h.DDLEventCh(), model.ActionAddIndex)
// 	require.NotNil(t, addIndexEvent2)
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(
// 		h.SPool(),
// 		func(sctx sessionctx.Context) error {
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 			require.NoError(t, pq.HandleDDLEvent(context.Background(), sctx, addIndexEvent2))
// 			return nil
// 		}, statsutil.FlagWrapTxn),
// 	)
// 	job, err := pq.Pop()
// 	require.NoError(t, err)
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, _ := job.ValidateAndPrepare(testKit.Session())
// 	require.True(t, valid)
// 	require.NoError(t, job.Analyze(h, do.SysProcTracker()))
// 	job, err = pq.Pop()
// 	require.NoError(t, err)
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, _ = job.ValidateAndPrepare(testKit.Session())
// 	require.True(t, valid)
// 	require.NoError(t, job.Analyze(h, do.SysProcTracker()))
//
// Check the stats of the indexes for each partition.
// 	tableStats := h.GetPhysicalTableStats(p0ID, tableInfo)
// 	require.True(t, tableStats.GetIdx(1).IsAnalyzed())
// 	require.True(t, tableStats.GetIdx(2).IsAnalyzed())
// 	require.True(t, tableStats.GetIdx(3).IsAnalyzed())
// 	tableStats = h.GetPhysicalTableStats(p1ID, tableInfo)
// 	require.True(t, tableStats.GetIdx(1).IsAnalyzed())
// 	require.True(t, tableStats.GetIdx(2).IsAnalyzed())
// 	require.True(t, tableStats.GetIdx(3).IsAnalyzed())
// }
//
// TestCreateIndexUnderDDLAnalyzeEnabled 对应 Go 测试函数：保留 Test Create Index Under D D L Analyze Enabled 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestCreateIndexUnderDDLAnalyzeEnabled(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int)")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(h)
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2),(6,2),(11,2),(16,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
//
// enable ddl analyze.
// 	testKit.MustExec("set @@tidb_stats_update_during_ddl = 1")
// create index.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t add index idx(c1, c2)")
//
// Find the create index event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	addIndexEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionAddIndex)
// 	tblInfo, idxInfo, analyzed := addIndexEvent.GetAddIndexInfo()
// 	require.Equal(t, tableInfo.ID, tblInfo.ID)
// 	require.Equal(t, analyzed, true)
// 	require.Equal(t, idxInfo[0].Name.L, "idx")
//
// Handle the add index event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, addIndexEvent)
// 	require.NoError(t, err)
//
// 	sctx := testKit.Session().(sessionctx.Context)
// Handle the add index event in priority queue.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	require.NoError(t, pq.HandleDDLEvent(ctx, sctx, addIndexEvent))
//
// The table is truncated, the job should be removed from the priority queue.
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.True(t, isEmpty)
//
// modify column.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t modify c1 varchar(10)")
//
// Find the modify column event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	modifyColumnEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionModifyColumn)
// 	tblInfo, columnInfo, analyzed := modifyColumnEvent.GetModifyColumnInfo()
// 	require.Equal(t, tableInfo.ID, tblInfo.ID)
// 	require.Equal(t, analyzed, true)
// 	require.Equal(t, columnInfo[0].Name.L, "c1")
// }
//
// TestTurnOffAutoAnalyzeAfterQueueInit 对应 Go 测试函数：保留 Test Turn Off Auto Analyze After Queue Init 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestTurnOffAutoAnalyzeAfterQueueInit(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2))")
// 	is := do.InfoSchema()
// 	tbl, err := is.TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))
// 	require.NoError(t, err)
// 	tableInfo := tbl.Meta()
// 	h := do.StatsHandle()
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(h)
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2),(6,2),(11,2),(16,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
// 	require.NoError(t, pq.Initialize(ctx))
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableInfo.ID, job.GetTableID())
//
// Add a new index on column c1.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t add index idx1(c1)")
//
// Find the add index event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	addIndexEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionAddIndex)
// 	require.NotNil(t, addIndexEvent)
//
// Disable the auto analyze.
// auto analyze 开关影响队列初始化和后台分析执行。
// 	testKit.MustExec("set @@global.tidb_enable_auto_analyze = 0;")
//
// Handle the add index event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(h, addIndexEvent)
// 	require.NoError(t, err)
//
// Handle the add index event in priority queue.
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(h.SPool(), func(sctx sessionctx.Context) error {
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 		return pq.HandleDDLEvent(ctx, sctx, addIndexEvent)
// 	}, statsutil.FlagWrapTxn))
//
// 	isEmpty, err = pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
// }
//
// TestTurnOffAutoAnalyzeBeforeQueueInit 对应 Go 测试函数：保留 Test Turn Off Auto Analyze Before Queue Init 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestTurnOffAutoAnalyzeBeforeQueueInit(t: &testing::T) {
// 	store, do := testkit.CreateMockStoreAndDomain(t)
// 	testKit := testkit.NewTestKit(t, store)
// 	testKit.MustExec("use test")
// 	enableAutoAnalyze(t, testKit)
// 	testKit.MustExec("create table t (c1 int, c2 int, index idx(c1, c2))")
// 	h := do.StatsHandle()
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(h)
// Insert some data.
// 	testKit.MustExec("insert into t values (1,2),(2,2),(6,2),(11,2),(16,2)")
// 	testKit.MustExec("flush stats_delta *.*")
// 	require.NoError(t, h.Update(context.Background(), do.InfoSchema()))
//
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// Disable the auto analyze.
// auto analyze 开关影响队列初始化和后台分析执行。
// 	testKit.MustExec("set @@global.tidb_enable_auto_analyze = 0;")
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(h)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	ctx := context.Background()
//
// Add a new index on column c1.
// DDL SQL 会触发 notifier/队列更新，是本测试的重要外部依赖。
// 	testKit.MustExec("alter table t add index idx1(c1)")
//
// Find the add index event.
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	addIndexEvent := statstestutil.FindEvent(h.DDLEventCh(), model.ActionAddIndex)
// 	require.NotNil(t, addIndexEvent)
//
// Handle the add index event.
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err := statstestutil.HandleDDLEventWithTxn(h, addIndexEvent)
// 	require.NoError(t, err)
//
// Handle the add index event in priority queue.
// CallWithSCtx 提供带事务/会话变量的 sessionctx，不实际打开会话。
// 	require.NoError(t, statsutil.CallWithSCtx(h.SPool(), func(sctx sessionctx.Context) error {
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 		return pq.HandleDDLEvent(ctx, sctx, addIndexEvent)
// 	}, statsutil.FlagWrapTxn))
//
// 	_, err = pq.IsEmptyForTest()
// 	require.ErrorContains(t, err, "priority queue not initialized")
// }
// */
use crate::{
    ERR_NOT_READY_RETRY_LATER, NewAnalysisPriorityQueue, QueueSource, SchemaChangeAction,
    SchemaChangeEvent,
};
use std::sync::Arc;

/// 空数据源，仅用于构造未初始化队列。
struct EmptySource;
impl QueueSource for EmptySource {
    fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn crate::AnalysisJob>>, String> {
        Ok(Vec::new())
    }
}

/// 未 Initialize 时：run_auto_analyze=true 返回 ERR_NOT_READY_RETRY_LATER，false 则 Ok。
#[test]
fn ddl_event_respects_auto_analyze_readiness_gate() {
    let queue = NewAnalysisPriorityQueue(Arc::new(EmptySource));
    let event = SchemaChangeEvent {
        action: Some(SchemaChangeAction::AddIndex),
        table_id: 7,
        ..SchemaChangeEvent::default()
    };
    assert_eq!(
        ERR_NOT_READY_RETRY_LATER,
        queue.HandleDDLEvent(true, &event).unwrap_err()
    );
    assert!(queue.HandleDDLEvent(false, &event).is_ok());
}

struct RecordingSource {
    recreated: Arc<std::sync::Mutex<Vec<i64>>>,
    fail_recreate: bool,
}

impl QueueSource for RecordingSource {
    fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn crate::AnalysisJob>>, String> {
        Ok(Vec::new())
    }

    fn recreate_job(&self, table_id: i64) -> Result<Option<Box<dyn crate::AnalysisJob>>, String> {
        self.recreated.lock().unwrap().push(table_id);
        if self.fail_recreate {
            Err("mock handleTaskOnce error".to_owned())
        } else {
            Ok(None)
        }
    }
}

#[test]
fn truncate_table_only_removes_dropped_physical_ids() {
    let recreated = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = NewAnalysisPriorityQueue(Arc::new(RecordingSource {
        recreated: Arc::clone(&recreated),
        fail_recreate: false,
    }));
    queue.RebuildWithoutLock(Vec::new()).unwrap();

    queue
        .HandleDDLEvent(
            true,
            &SchemaChangeEvent {
                action: Some(SchemaChangeAction::TruncateTable),
                table_id: 20,
                old_table_id: Some(10),
                affected_table_ids: vec![11, 12],
                added_index_analyzed: false,
            },
        )
        .unwrap();

    assert!(recreated.lock().unwrap().is_empty());
}

#[test]
fn initialized_dispatch_logs_and_swallows_handler_errors() {
    let recreated = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = NewAnalysisPriorityQueue(Arc::new(RecordingSource {
        recreated,
        fail_recreate: true,
    }));
    queue.RebuildWithoutLock(Vec::new()).unwrap();

    let result = queue.HandleDDLEvent(
        true,
        &SchemaChangeEvent {
            action: Some(SchemaChangeAction::AddIndex),
            table_id: 7,
            ..SchemaChangeEvent::default()
        },
    );

    assert_eq!(result, Ok(()));
}

#[test]
fn already_analyzed_added_index_is_ignored() {
    let recreated = Arc::new(std::sync::Mutex::new(Vec::new()));
    let queue = NewAnalysisPriorityQueue(Arc::new(RecordingSource {
        recreated: Arc::clone(&recreated),
        fail_recreate: false,
    }));
    queue.RebuildWithoutLock(Vec::new()).unwrap();

    queue
        .HandleDDLEvent(
            true,
            &SchemaChangeEvent {
                action: Some(SchemaChangeAction::AddIndex),
                table_id: 7,
                added_index_analyzed: true,
                ..SchemaChangeEvent::default()
            },
        )
        .unwrap();

    assert!(recreated.lock().unwrap().is_empty());
}

#[test]
fn partition_changes_recreate_only_the_surviving_global_table() {
    for action in [
        SchemaChangeAction::TruncateTablePartition,
        SchemaChangeAction::DropTablePartition,
        SchemaChangeAction::ReorganizePartition,
    ] {
        let recreated = Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue = NewAnalysisPriorityQueue(Arc::new(RecordingSource {
            recreated: Arc::clone(&recreated),
            fail_recreate: false,
        }));
        queue.RebuildWithoutLock(Vec::new()).unwrap();

        queue
            .HandleDDLEvent(
                true,
                &SchemaChangeEvent {
                    action: Some(action),
                    table_id: 100,
                    affected_table_ids: vec![101, 102],
                    ..SchemaChangeEvent::default()
                },
            )
            .unwrap();

        assert_eq!(*recreated.lock().unwrap(), vec![100], "action: {action:?}");
    }
}

fn job(table_id: i64) -> Box<dyn crate::AnalysisJob> {
    Box::new(crate::NewNonPartitionedTableAnalysisJob(
        table_id,
        std::collections::HashMap::new(),
        2,
        false,
        0.5,
        10.0,
        std::time::Duration::ZERO,
    ))
}

#[test]
fn destructive_ddl_actions_remove_every_obsolete_physical_job() {
    let cases = [
        (
            SchemaChangeAction::DropTable,
            SchemaChangeEvent {
                table_id: 10,
                affected_table_ids: vec![11, 12],
                ..SchemaChangeEvent::default()
            },
        ),
        (
            SchemaChangeAction::TruncateTablePartition,
            SchemaChangeEvent {
                table_id: 10,
                affected_table_ids: vec![11, 12],
                ..SchemaChangeEvent::default()
            },
        ),
        (
            SchemaChangeAction::ExchangeTablePartition,
            SchemaChangeEvent {
                table_id: 10,
                old_table_id: Some(13),
                affected_table_ids: vec![11],
                ..SchemaChangeEvent::default()
            },
        ),
        (
            SchemaChangeAction::AlterTablePartitioning,
            SchemaChangeEvent {
                table_id: 10,
                old_table_id: Some(13),
                ..SchemaChangeEvent::default()
            },
        ),
        (
            SchemaChangeAction::RemovePartitioning,
            SchemaChangeEvent {
                table_id: 10,
                old_table_id: Some(13),
                affected_table_ids: vec![11, 12],
                ..SchemaChangeEvent::default()
            },
        ),
        (
            SchemaChangeAction::DropSchema,
            SchemaChangeEvent {
                table_id: 10,
                affected_table_ids: vec![11, 12, 13],
                ..SchemaChangeEvent::default()
            },
        ),
    ];

    for (action, mut event) in cases {
        event.action = Some(action);
        let queue = NewAnalysisPriorityQueue(Arc::new(EmptySource));
        let mut jobs = vec![job(event.table_id), job(999)];
        if let Some(old_table_id) = event.old_table_id {
            jobs.push(job(old_table_id));
        }
        jobs.extend(event.affected_table_ids.iter().copied().map(job));
        queue.RebuildWithoutLock(jobs).unwrap();
        queue.HandleDDLEvent(true, &event).unwrap();

        let remaining = queue
            .Snapshot()
            .unwrap()
            .0
            .into_iter()
            .map(|job| job.TableID)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            remaining,
            std::collections::HashSet::from([999]),
            "{action:?}"
        );
    }
}
