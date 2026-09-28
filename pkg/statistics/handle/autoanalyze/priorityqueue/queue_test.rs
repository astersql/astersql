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

// 分析优先队列（AnalysisPriorityQueue）主体单元测试。
//
// 上方大段注释保留 Go 集成测试语义（初始化、Pop、DML 变更刷新、并发 Close 等）；
// 可执行部分用空 QueueSource 校验未初始化错误、空重建与幂等 Close。

// TestKit、require、failpoint、DDL notifier、sessionctx、goroutine/channel 等外部依赖均按 Go 调用形状保留为后续接线点。
//
// TestCallAPIBeforeInitialize 对应 Go 测试函数：保留 Test Call A P I Before Initialize 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestCallAPIBeforeInitialize(t: &testing::T) {
// 	_, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
//
// 	t.Run("IsEmpty", func(t *testing.T) {
// 		isEmpty, err := pq.IsEmptyForTest()
// 		require.Error(t, err)
// 		require.False(t, isEmpty)
// 	})
//
// 	t.Run("Pop", func(t *testing.T) {
// 		poppedJob, err := pq.Pop()
// 		require.Error(t, err)
// 		require.Nil(t, poppedJob)
// 	})
//
// 	t.Run("GetAllJobs", func(t *testing.T) {
// 		jobs := pq.GetRunningJobs()
// 		require.Len(t, jobs, 0)
// 	})
//
// 	t.Run("Peek", func(t *testing.T) {
// 		job, err := pq.PeekForTest()
// 		require.Error(t, err)
// 		require.Nil(t, job)
// 	})
// }
//
// TestAnalysisPriorityQueue 对应 Go 测试函数：保留 Test Analysis Priority Queue 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestAnalysisPriorityQueue(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	handle := dom.StatsHandle()
// 	tk.MustExec("create table t1 (a int)")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("create table t2 (a int)")
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("insert into t1 values (1)")
// 	tk.MustExec("insert into t2 values (1)")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	ctx := context.Background()
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
//
// 	t.Run("Initialize", func(t *testing.T) {
// With timeout context
// 		cancellableContext, cancel := context.WithTimeout(ctx, 100*time.Second)
// Cancel the context to test the error handling
// 		cancel()
// 		err := pq.Initialize(cancellableContext)
// 		require.ErrorIs(t, err, context.Canceled)
//
// 		err = pq.Initialize(ctx)
// 		require.NoError(t, err)
// 		require.True(t, pq.IsInitialized())
//
// Test double initialization
// 		err = pq.Initialize(ctx)
// 		require.NoError(t, err)
// 	})
//
// 	t.Run("IsEmpty And Pop", func(t *testing.T) {
// 		isEmpty, err := pq.IsEmptyForTest()
// 		require.NoError(t, err)
// 		require.False(t, isEmpty)
//
// 		poppedJob, err := pq.Pop()
// 		require.NoError(t, err)
// 		require.NotNil(t, poppedJob)
//
// 		poppedJob, err = pq.Pop()
// 		require.NoError(t, err)
// 		require.NotNil(t, poppedJob)
//
// 		isEmpty, err = pq.IsEmptyForTest()
// 		require.NoError(t, err)
// 		require.True(t, isEmpty)
//
// 		runningJobs := pq.GetRunningJobs()
// 		require.Len(t, runningJobs, 2)
// 	})
// }
//
// TestRefreshLastAnalysisDuration 对应 Go 测试函数：保留 Test Refresh Last Analysis Duration 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestRefreshLastAnalysisDuration(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	tk.MustExec("create table t1 (a int)")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("create table t2 (a int)")
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("insert into t1 values (1)")
// 	tk.MustExec("insert into t2 values (1)")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	ctx := context.Background()
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(ctx))
//
// Check current jobs
// 	isEmpty, err := pq.IsEmptyForTest()
// 	require.NoError(t, err)
// 	require.False(t, isEmpty)
//
// Analyze the tables
// 	tk.MustExec("analyze table t1")
// 	tk.MustExec("analyze table t2")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Call RefreshLastAnalysisDuration
// 	pq.RefreshLastAnalysisDuration()
//
// Check if the jobs' last analysis durations and weights have been updated
// 	updatedJob1, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.NotZero(t, updatedJob1.GetWeight())
// 	require.NotZero(t, updatedJob1.GetIndicators().LastAnalysisDuration)
// 	require.NotEqual(t, time.Minute*3, updatedJob1.GetIndicators().LastAnalysisDuration)
//
// 	updatedJob2, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.NotZero(t, updatedJob2.GetWeight())
// 	require.NotZero(t, updatedJob2.GetIndicators().LastAnalysisDuration)
// 	require.NotEqual(t, time.Minute*3, updatedJob2.GetIndicators().LastAnalysisDuration)
//
// Check running jobs
// 	runningJobs := pq.GetRunningJobs()
// 	require.Len(t, runningJobs, 2)
// }
//
// testProcessDMLChanges 对应 Go 辅助函数：按来源测试复用同一组 fixture、参数和断言路径。
// pub fn testProcessDMLChanges(t: &testing::T, partitioned: bool) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	ctx := context.Background()
// 	if partitioned {
// 		tk.MustExec("use test")
// 		tk.MustExec("create table t1 (a int) partition by range (a) (partition p0 values less than (10), partition p1 values less than (20))")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 		statstestutil.HandleNextDDLEventWithTxn(handle)
// 		tk.MustExec("create table t2 (a int) partition by range (a) (partition p0 values less than (10), partition p1 values less than (20))")
// 		statstestutil.HandleNextDDLEventWithTxn(handle)
// Because we don't handle the DDL events in unit tests by default,
// we need to use this way to make sure the stats record for the global table is created.
// Insert some rows into the tables.
// 		tk.MustExec("insert into t1 values (11)")
// 		tk.MustExec("insert into t2 values (12)")
// 		tk.MustExec("flush stats_delta *.*")
// Analyze the tables.
// 		tk.MustExec("analyze table t1")
// 		tk.MustExec("analyze table t2")
// 		require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
// 	} else {
// 		tk.MustExec("use test")
// 		tk.MustExec("create table t1 (a int)")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 		statstestutil.HandleNextDDLEventWithTxn(handle)
// 		tk.MustExec("create table t2 (a int)")
// 		statstestutil.HandleNextDDLEventWithTxn(handle)
// 	}
// 	tk.MustExec("insert into t1 values (1)")
// 	tk.MustExec("insert into t2 values (1), (2)")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
// 	schema := ast.NewCIStr("test")
// 	tbl1, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t1"))
// 	require.NoError(t, err)
// 	tbl2, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t2"))
// 	require.NoError(t, err)
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(ctx))
//
// Check current jobs.
// 	job1, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.Equal(t, tbl1.Meta().ID, job1.GetTableID())
// 	job2, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.Equal(t, tbl2.Meta().ID, job2.GetTableID())
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, _ := job1.ValidateAndPrepare(tk.Session())
// 	require.True(t, valid)
// 	valid, _ = job2.ValidateAndPrepare(tk.Session())
// 	require.True(t, valid)
// Analyze 会执行真实 ANALYZE 路径；这里只保留测试对统计结果的预期。
// 	require.NoError(t, job1.Analyze(handle, dom.SysProcTracker()))
// 	require.NoError(t, job2.Analyze(handle, dom.SysProcTracker()))
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Insert 9 rows into t1.
// 	tk.MustExec("insert into t1 values (3), (4), (5), (6), (7), (8), (9), (10), (11)")
// Insert 1 row into t2.
// 	tk.MustExec("insert into t2 values (3)")
//
// Dump the stats to kv.
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Check if the jobs have been updated.
// 	updatedJob1, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.NotZero(t, updatedJob1.GetWeight())
// 	require.Equal(t, tbl1.Meta().ID, updatedJob1.GetTableID())
//
// Update 15 times on t2.
// 	for i := 0; i < 15; i++ {
// 		tk.MustExec("update t2 set a = a + 1 where a = " + strconv.Itoa(i+3))
// 	}
//
// Dump the stats to kv.
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Check if the jobs have been updated.
// 	updatedJob2, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.NotZero(t, updatedJob2.GetWeight())
// 	require.Equal(t, tbl2.Meta().ID, updatedJob2.GetTableID(), "t2 should have higher weight due to smaller table size and more changes")
// }
//
// TestProcessDMLChanges 对应 Go 测试函数：保留 Test Process D M L Changes 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestProcessDMLChanges(t: &testing::T) {
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	testProcessDMLChanges(t, false)
// }
//
// TestProcessDMLChangesPartitioned 对应 Go 测试函数：保留 Test Process D M L Changes Partitioned 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestProcessDMLChangesPartitioned(t: &testing::T) {
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	testProcessDMLChanges(t, true)
// }
//
// TestProcessDMLChangesWithRunningJobs 对应 Go 测试函数：保留 Test Process D M L Changes With Running Jobs 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestProcessDMLChangesWithRunningJobs(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	tk.MustExec("create table t1 (a int)")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("create table t2 (a int)")
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("insert into t1 values (1)")
// 	tk.MustExec("insert into t2 values (1)")
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
// 	tbl2, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t2"))
// 	require.NoError(t, err)
// 	tk.MustExec("analyze table t1")
// 	tk.MustExec("analyze table t2")
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
// Insert 2 rows into t2.
// 	tk.MustExec("insert into t2 values (2), (3)")
// Dump the stats to kv.
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
//
// Check if the running job is still in the queue.
// 	runningJobs = pq.GetRunningJobs()
// 	require.Len(t, runningJobs, 1)
//
// 	job2, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.NotZero(t, job2.GetWeight())
// 	require.Equal(t, tbl2.Meta().ID, job2.GetTableID(), "t1 should not be in the queue since it's a running job")
//
// Analyze the job.
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	valid, _ := job1.ValidateAndPrepare(tk.Session())
// 	require.True(t, valid)
// Analyze 会执行真实 ANALYZE 路径；这里只保留测试对统计结果的预期。
// 	require.NoError(t, job1.Analyze(handle, dom.SysProcTracker()))
//
// Add more rows to t1.
// 	tk.MustExec("insert into t1 values (4), (5), (6), (7), (8), (9), (10), (11), (12), (13)")
// Dump the stats to kv.
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Check if the jobs have been updated.
// 	job1, err = pq.Pop()
// 	require.NoError(t, err)
// 	require.NotZero(t, job1.GetWeight())
// 	require.Equal(t, tbl1.Meta().ID, job1.GetTableID(), "t1 has been removed from running jobs and should be in the queue")
// }
//
// TestRequeueMustRetryJobs 对应 Go 测试函数：保留 Test Requeue Must Retry Jobs 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestRequeueMustRetryJobs(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("create database example_schema")
// 	tk.MustExec("use example_schema")
// 	tk.MustExec("create table example_table (a int)")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	initJobs(tk)
// 	insertMultipleFinishedJobs(tk, "example_table", "")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// Insert the failed job.
// Just failed.
// 	now := tk.MustQuery("select now()").Rows()[0][0].(string)
// 	insertFailedJobWithStartTime(tk, "example_schema", "example_table", "", now)
//
// Insert some rows.
// 	tk.MustExec("insert into example_table values (11), (12), (13), (14), (15), (16), (17), (18), (19)")
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(context.Background(), dom.InfoSchema()))
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(context.Background()))
//
// 	job, err := pq.Pop()
// 	require.NoError(t, err)
// 	require.NotNil(t, job)
// 	sctx := tk.Session().(sessionctx.Context)
// ValidateAndPrepare 同时做元数据解析和失败重试检查，断言需保留返回值含义。
// 	ok, _ := job.ValidateAndPrepare(sctx)
// 	require.False(t, ok)
//
// Insert more rows.
// 	tk.MustExec("insert into example_table values (20), (21), (22), (23), (24), (25), (26), (27), (28), (29)")
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(context.Background(), dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
// 	l, err := pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, 0, l)
//
// Requeue the failed jobs.
// must-retry 重入队逻辑影响失败任务是否重新进入优先队列。
// 	pq.RequeueMustRetryJobs()
// 	l, err = pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, 1, l)
// }
//
// TestProcessDMLChangesWithLockedTables 对应 Go 测试函数：保留 Test Process D M L Changes With Locked Tables 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestProcessDMLChangesWithLockedTables(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	tk.MustExec("create table t1 (a int)")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("create table t2 (a int)")
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("insert into t1 values (1)")
// 	tk.MustExec("insert into t2 values (1)")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	ctx := context.Background()
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(context.Background()))
//
// 	schema := ast.NewCIStr("test")
// 	tbl1, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t1"))
// 	require.NoError(t, err)
// 	tbl2, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t2"))
// 	require.NoError(t, err)
//
// Check current jobs. Tables with the same priority do not have a stable heap order.
// 	snapshot, err := pq.Snapshot()
// 	require.NoError(t, err)
// 	currentJobIDs := make([]int64, 0, len(snapshot.CurrentJobs))
// 	for _, job := range snapshot.CurrentJobs {
// 		currentJobIDs = append(currentJobIDs, job.TableID)
// 	}
// 	require.ElementsMatch(t, []int64{tbl1.Meta().ID, tbl2.Meta().ID}, currentJobIDs)
//
// Lock t1.
// lock stats 会让队列跳过被锁定表或分区。
// 	tk.MustExec("lock stats t1")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Check if the jobs have been updated.
// 	l, err := pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, 1, l)
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tbl2.Meta().ID, job.GetTableID())
//
// Unlock t1.
// lock stats 会让队列跳过被锁定表或分区。
// 	tk.MustExec("unlock stats t1")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Check if the jobs have been updated.
// 	l, err = pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, 2, l)
// }
//
// TestProcessDMLChangesWithLockedPartitionsAndDynamicPruneMode 对应 Go 测试函数：保留 Test Process D M L Changes With Locked Partitions And Dynamic Prune Mode 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestProcessDMLChangesWithLockedPartitionsAndDynamicPruneMode(t: &testing::T) {
// 	ctx := context.Background()
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	tk.MustExec("create table t1 (a int) partition by range (a) (partition p0 values less than (10), partition p1 values less than (20))")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("insert into t1 values (1)")
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
// 	tk.MustExec("analyze table t1")
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	tk.MustExec("set global tidb_partition_prune_mode = 'dynamic'")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// Insert more rows into partition p0.
// 	tk.MustExec("insert into t1 partition (p0) values (2), (3), (4), (5), (6), (7), (8), (9)")
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(context.Background()))
//
// 	schema := ast.NewCIStr("test")
// 	tbl, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t1"))
// 	require.NoError(t, err)
//
// Check current jobs.
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	tableID := tbl.Meta().ID
// 	require.Equal(t, tableID, job.GetTableID())
//
// Lock the whole table.
// lock stats 会让队列跳过被锁定表或分区。
// 	tk.MustExec("lock stats t1")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// No jobs should be in the queue.
// 	l, err := pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, 0, l)
//
// Unlock the whole table.
// lock stats 会让队列跳过被锁定表或分区。
// 	tk.MustExec("unlock stats t1")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Check if the jobs have been updated.
// 	job, err = pq.PeekForTest()
// 	require.NoError(t, err)
// 	require.Equal(t, tableID, job.GetTableID())
// }
//
// TestProcessDMLChangesWithLockedPartitionsAndStaticPruneMode 对应 Go 测试函数：保留 Test Process D M L Changes With Locked Partitions And Static Prune Mode 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestProcessDMLChangesWithLockedPartitionsAndStaticPruneMode(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
// 	tk.MustExec("create table t1 (a int) partition by range (a) (partition p0 values less than (10), partition p1 values less than (20))")
// 测试手动消费 DDL event，以模拟 domain/统计模块的事务处理。
// 	statstestutil.HandleNextDDLEventWithTxn(handle)
// 	tk.MustExec("insert into t1 values (1)")
// 分区裁剪模式决定静态/动态分区 job 的粒度。
// 	tk.MustExec("set global tidb_partition_prune_mode = 'static'")
// 	statistics.AutoAnalyzeMinCnt = 0
// defer handles cleanup in Go.
// 	defer func() {
// 		statistics.AutoAnalyzeMinCnt = 1000
// 	}()
//
// 	ctx := context.Background()
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
// 	tk.MustExec("analyze table t1")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
// 	schema := ast.NewCIStr("test")
// 	tbl, err := dom.InfoSchema().TableByName(ctx, schema, ast.NewCIStr("t1"))
// 	require.NoError(t, err)
//
// Insert more rows into partition p0.
// 	tk.MustExec("insert into t1 partition (p0) values (2), (3), (4), (5), (6), (7), (8), (9)")
// 	tk.MustExec("flush stats_delta *.*")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(ctx))
//
// Check current jobs.
// 	job, err := pq.PeekForTest()
// 	require.NoError(t, err)
// 	pid := tbl.Meta().Partition.Definitions[0].ID
// 	require.Equal(t, pid, job.GetTableID())
//
// Lock partition p0.
// lock stats 会让队列跳过被锁定表或分区。
// 	tk.MustExec("lock stats t1 partition p0")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// No jobs should be in the queue.
// 	l, err := pq.Len()
// 	require.NoError(t, err)
// 	require.Equal(t, 0, l)
//
// Unlock partition p0.
// lock stats 会让队列跳过被锁定表或分区。
// 	tk.MustExec("unlock stats t1 partition (p0)")
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Process the DML changes.
// ProcessDMLChanges 根据统计 delta 刷新队列权重，迁移时保持调用位置。
// 	pq.ProcessDMLChanges()
//
// Check if the jobs have been updated.
// 	job, err = pq.PeekForTest()
// 	require.NoError(t, err)
// 	pid = tbl.Meta().Partition.Definitions[0].ID
// 	require.Equal(t, pid, job.GetTableID())
// }
//
// TestPQCanBeClosedAndReInitialized 对应 Go 测试函数：保留 Test P Q Can Be Closed And Re Initialized 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestPQCanBeClosedAndReInitialized(t: &testing::T) {
// 	_, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(context.Background()))
//
// Close the priority queue.
// 	pq.Close()
//
// Check if the priority queue is closed.
// 	require.False(t, pq.IsInitialized())
//
// Re-initialize the priority queue.
// 	require.NoError(t, pq.Initialize(context.Background()))
//
// Check if the priority queue is initialized.
// 	require.True(t, pq.IsInitialized())
// }
//
// TestPQHandlesTableDeletionGracefully 对应 Go 测试函数：保留 Test P Q Handles Table Deletion Gracefully 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestPQHandlesTableDeletionGracefully(t: &testing::T) {
// 	store, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
//
// 	tk := testkit.NewTestKit(t, store)
// 	tk.MustExec("use test")
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
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// defer handles cleanup in Go.
// 	defer pq.Close()
// 	require.NoError(t, pq.Initialize(ctx))
//
// Check the priority queue is not empty.
// 	l, err := pq.Len()
// 	require.NoError(t, err)
// 	require.NotEqual(t, 0, l)
//
// 	tbl, err := dom.InfoSchema().TableByName(ctx, ast.NewCIStr("test"), ast.NewCIStr("t1"))
// 	require.NoError(t, err)
//
// Drop the table and mock the table stats is removed from the cache.
// drop table 事件用于验证队列能清理不存在对象的 job。
// 	tk.MustExec("drop table t1")
// 从 DDL event channel 中选择指定事件，保持 Go 测试对事件类型的依赖。
// 	deleteEvent := statstestutil.FindEvent(handle.DDLEventCh(), model.ActionDropTable)
// 	require.NotNil(t, deleteEvent)
// DDL event 处理是队列变更的关键外部输入，保留事件流转顺序。
// 	err = statstestutil.HandleDDLEventWithTxn(handle, deleteEvent)
// 	require.NoError(t, err)
// 	require.NoError(t, handle.Update(ctx, dom.InfoSchema()))
//
// Make sure handle.Get() returns false.
// 	_, ok := handle.Get(tbl.Meta().ID)
// 	require.False(t, ok)
//
// 	require.NotPanics(t, func() {
// 		pq.RefreshLastAnalysisDuration()
// 	})
// }
//
// TestConcurrentCloseAndBackgroundOperations 对应 Go 测试函数：保留 Test Concurrent Close And Background Operations 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestConcurrentCloseAndBackgroundOperations(t: &testing::T) {
// Enable the failpoint to simulate long-running operations
// failpoint 用于模拟执行阻塞或异常路径，不实际启用外部注入点。
// 	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/statistics/handle/autoanalyze/priorityqueue/tryBlockCloseAnalysisPriorityQueue", "return(true)")
// 	_, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
//
// 	ctx := context.Background()
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// 	require.NoError(t, pq.Initialize(ctx))
//
// Use a channel to signal when Close() completes
// channel 用于跨 goroutine 同步完成信号，暂不替换为具体同步原语。
// 	closeDone := make(chan struct{})
// goroutine 表示并发执行路径；这里保留异步分析或关闭队列的原始时序。
// 	go func() {
// 		pq.Close()
// 		close(closeDone)
// 	}()
//
// Wait for Close() to complete with a timeout
// select 分支保留 Go 的并发等待和超时语义。
// 	select {
// 	case <-closeDone:
// Success - Close() completed without deadlock
// 		require.False(t, pq.IsInitialized(), "Queue should not be initialized after Close()")
// 超时分支用于防止并发测试死锁，仅记录等待上限。
// 	case <-time.After(6 * time.Second):
// 		t.Fatal("Close() timed out during concurrent operations - likely deadlock detected!")
// 	}
// }
//
// TestConcurrentClose 对应 Go 测试函数：保留 Test Concurrent Close 的 TestKit、DDL 事件、队列状态和 require 断言流程。
// #[test] 迁移提示：原 Go 测试依赖 testing.T、require 和 TestKit，此处暂保留带 t 参数的签名。
// pub fn TestConcurrentClose(t: &testing::T) {
// 	_, dom := testkit.CreateMockStoreAndDomain(t)
// 	handle := dom.StatsHandle()
//
// 	ctx := context.Background()
// 	pq := priorityqueue.NewAnalysisPriorityQueue(handle)
// 	require.NoError(t, pq.Initialize(ctx))
// 	require.True(t, pq.IsInitialized())
//
// 	const numGoroutines = 20
// 	var wg sync.WaitGroup
// 	for i := 0; i < numGoroutines; i++ {
// 		wg.Go(
// 			func() {
// defer handles cleanup in Go.
// 				defer func() {
// Ensure no panics occur during concurrent Close()
// 					if r := recover(); r != nil {
// 						t.Errorf("Close() panicked: %v", r)
// 					}
// 				}()
// 				pq.Close()
// 			},
// 		)
// 	}
//
// channel 用于跨 goroutine 同步完成信号，暂不替换为具体同步原语。
// 	done := make(chan struct{})
// goroutine 表示并发执行路径；这里保留异步分析或关闭队列的原始时序。
// 	go func() {
// 		wg.Wait()
// 		close(done)
// 	}()
//
// select 分支保留 Go 的并发等待和超时语义。
// 	select {
// 	case <-done:
// 超时分支用于防止并发测试死锁，仅记录等待上限。
// 	case <-time.After(5 * time.Second):
// 		t.Fatal("Concurrent Close() calls timed out - likely deadlock detected!")
// 	}
//
// Verify the queue is properly closed
// 	require.False(t, pq.IsInitialized(), "Queue should not be initialized after Close()")
// }
// */
use crate::{NOT_INITIALIZED_ERR_MSG, NewAnalysisPriorityQueue, QueueSource};
use std::sync::Arc;

/// 空数据源：不产生任何分析作业，用于隔离队列状态机测试。
struct EmptySource;
impl QueueSource for EmptySource {
    fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn crate::AnalysisJob>>, String> {
        Ok(Vec::new())
    }
}

/// 校验 Initialize 前 API 报错、空重建成功、Close 可重复调用。
#[test]
fn queue_initializes_empty_rebuild_and_closes_idempotently() {
    let queue = NewAnalysisPriorityQueue(Arc::new(EmptySource));
    assert!(!queue.IsInitialized());
    // 未初始化时 Peek 应返回 NOT_INITIALIZED_ERR_MSG。
    assert!(
        queue
            .PeekForTest()
            .unwrap_err()
            .contains(NOT_INITIALIZED_ERR_MSG)
    );
    queue.Initialize().unwrap();
    assert!(queue.IsInitialized());
    assert!(queue.IsEmptyForTest().unwrap());
    // Close 幂等：第二次调用不应 panic。
    queue.Close();
    queue.Close();
    assert!(!queue.IsInitialized());

    // Go allows the queue to be initialized again after Close resets its sync fields.
    queue.Initialize().unwrap();
    assert!(queue.IsInitialized());
    queue.Close();
}

#[test]
fn rebuild_before_initialize_matches_go_not_initialized_error() {
    let queue = NewAnalysisPriorityQueue(Arc::new(EmptySource));
    assert_eq!(queue.Rebuild().unwrap_err(), NOT_INITIALIZED_ERR_MSG);
    assert!(!queue.IsInitialized());
    queue.Close();
}

#[test]
fn requeue_consumes_missing_retry_entry_like_go() {
    use crate::{AnalysisJob, AnalysisRuntime, NewNonPartitionedTableAnalysisJob, TableMetadata};
    use std::collections::HashMap;
    use std::time::Duration;

    fn job() -> Box<dyn AnalysisJob> {
        Box::new(NewNonPartitionedTableAnalysisJob(
            11,
            HashMap::new(),
            2,
            false,
            0.5,
            100.0,
            Duration::ZERO,
        ))
    }

    struct MissingSource;
    impl QueueSource for MissingSource {
        fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn AnalysisJob>>, String> {
            Ok(vec![job()])
        }
    }

    struct FailedRuntime;
    impl AnalysisRuntime for FailedRuntime {
        fn table_by_id(&self, _: i64) -> Option<TableMetadata> {
            None
        }
        fn last_failed_analysis_duration(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<Duration>, String> {
            Ok(None)
        }
        fn average_analysis_duration(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<Duration>, String> {
            Ok(None)
        }
        fn execute_analyze(&self, _: &str, _: &[String], _: i32, _: bool) -> Result<bool, String> {
            Ok(false)
        }
    }

    let queue = NewAnalysisPriorityQueue(Arc::new(MissingSource));
    queue.Initialize().unwrap();
    let mut failed = queue.Pop().unwrap();
    failed.Analyze(&FailedRuntime).unwrap();
    assert!(queue.Snapshot().unwrap().2.contains(&11));

    queue.RequeueMustRetryJobs().unwrap();
    assert!(!queue.Snapshot().unwrap().2.contains(&11));
    queue.Close();
}

#[test]
fn refresh_updates_only_duration_and_recalculates_weight() {
    use crate::{AnalysisJob, Indicators, NewNonPartitionedTableAnalysisJob};
    use std::collections::HashMap;
    use std::time::Duration;

    fn job() -> Box<dyn AnalysisJob> {
        Box::new(NewNonPartitionedTableAnalysisJob(
            23,
            HashMap::new(),
            2,
            false,
            0.5,
            100.0,
            Duration::ZERO,
        ))
    }

    struct RefreshSource;
    impl QueueSource for RefreshSource {
        fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn AnalysisJob>>, String> {
            Ok(vec![job()])
        }
        fn refreshed_indicators(&self, id: i64) -> Result<Option<Indicators>, String> {
            assert_eq!(id, 23);
            Ok(Some(Indicators {
                // These two values must not replace the existing job's indicators.
                ChangePercentage: 0.9,
                TableSize: 1.0,
                LastAnalysisDuration: Duration::from_secs(3_600).into(),
            }))
        }
    }

    let queue = NewAnalysisPriorityQueue(Arc::new(RefreshSource));
    queue.Initialize().unwrap();
    let old_weight = queue.PeekForTest().unwrap().Weight;
    queue.RefreshLastAnalysisDuration().unwrap();
    let refreshed = queue.Pop().unwrap();
    let indicators = refreshed.GetIndicators();
    assert_eq!(indicators.ChangePercentage, 0.5);
    assert_eq!(indicators.TableSize, 100.0);
    assert_eq!(indicators.LastAnalysisDuration, Duration::from_secs(3_600));
    assert_ne!(refreshed.GetWeight().to_string(), old_weight);
    queue.Close();
}

#[test]
fn complete_job_hooks_clear_running_and_requeue_failures() {
    use crate::{AnalysisJob, AnalysisRuntime, NewNonPartitionedTableAnalysisJob, TableMetadata};
    use std::collections::HashMap;
    use std::time::Duration;
    fn job() -> Box<dyn AnalysisJob> {
        Box::new(NewNonPartitionedTableAnalysisJob(
            7,
            HashMap::new(),
            2,
            false,
            0.5,
            100.0,
            Duration::ZERO,
        ))
    }
    struct Source;
    impl QueueSource for Source {
        fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn AnalysisJob>>, String> {
            Ok(vec![job()])
        }
        fn recreate_job(&self, id: i64) -> Result<Option<Box<dyn AnalysisJob>>, String> {
            assert_eq!(id, 7);
            Ok(Some(job()))
        }
    }
    struct Runtime(bool);
    impl AnalysisRuntime for Runtime {
        fn table_by_id(&self, _: i64) -> Option<TableMetadata> {
            None
        }
        fn last_failed_analysis_duration(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<Duration>, String> {
            Ok(None)
        }
        fn average_analysis_duration(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<Option<Duration>, String> {
            Ok(None)
        }
        fn execute_analyze(&self, _: &str, _: &[String], _: i32, _: bool) -> Result<bool, String> {
            Ok(self.0)
        }
    }
    let queue = NewAnalysisPriorityQueue(Arc::new(Source));
    queue.Initialize().unwrap();
    let mut failed = queue.Pop().unwrap();
    assert!(queue.GetRunningJobs().contains(&7));
    failed.Analyze(&Runtime(false)).unwrap();
    assert!(queue.GetRunningJobs().is_empty());
    queue.RequeueMustRetryJobs().unwrap();
    let mut succeeded = queue.Pop().unwrap();
    assert_eq!(succeeded.GetTableID(), 7);
    succeeded.Analyze(&Runtime(true)).unwrap();
    assert!(queue.GetRunningJobs().is_empty());
    queue.RequeueMustRetryJobs().unwrap();
    assert!(queue.IsEmptyForTest().unwrap());
    queue.Close();
    // The callback retains only Weak queue state and stays safe after queue drop.
    drop(queue);
    succeeded.Analyze(&Runtime(true)).unwrap();
}
