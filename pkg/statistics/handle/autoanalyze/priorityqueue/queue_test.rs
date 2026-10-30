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

#[test]
fn port_da76b94491_rebuild_consumes_stream_without_batch_collection() {
    use crate::{AnalysisJob, NewAnalysisPriorityQueue, QueueSource};
    use std::sync::Arc;
    struct Streaming;
    impl QueueSource for Streaming {
        fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn AnalysisJob>>, String> {
            Err("batch collection must not be used".into())
        }
        fn visit_analysis_jobs(
            &self,
            _visit: &mut dyn FnMut(Box<dyn AnalysisJob>) -> Result<(), String>,
        ) -> Result<(), String> {
            Ok(())
        }
    }
    let queue = NewAnalysisPriorityQueue(Arc::new(Streaming));
    assert_eq!(queue.Initialize(), Ok(()));
    assert_eq!(queue.Rebuild(), Ok(()));
    queue.Close();
}

// Fixtures provide metadata/stat snapshots; all filtering, job construction,
// heap mutation and hooks under test are the actual queue implementation.
struct PortDa76Source {
    metadata: astersql_meta::Mutator,
    locked: std::sync::Mutex<std::collections::HashSet<i64>>,
    static_prune: bool,
    failure: std::sync::Mutex<Option<&'static str>>,
    reads: std::sync::Mutex<Vec<i64>>,
}
impl crate::PartitionStatsProvider for std::sync::Arc<PortDa76Source> {
    fn get_non_pseudo_physical_table_stats(&self, id: i64) -> Option<crate::TableStats> {
        self.reads.lock().unwrap().push(id);
        if id == 8 || id == 9 {
            return None;
        }
        Some(crate::TableStats {
            realtime_count: 10,
            column_count: 1,
            eligible: true,
            ..Default::default()
        })
    }
}
impl crate::AnalysisTableSource for std::sync::Arc<PortDa76Source> {
    fn parameters(&self) -> Result<crate::AnalysisScanParameters, String> {
        if *self.failure.lock().unwrap() == Some("parameters") {
            return Err("parameters".into());
        }
        Ok(crate::AnalysisScanParameters {
            factory: crate::NewAnalysisJobFactory(
                crate::SessionContext { analyze_version: 2 },
                0.5,
                0,
            ),
            locked_tables: self.locked.lock().unwrap().clone(),
            static_prune: self.static_prune,
        })
    }
    fn visit_tables(
        &self,
        visit: &mut dyn FnMut(crate::AnalysisScanTable) -> Result<(), String>,
    ) -> Result<(), String> {
        // Real metadata JSON/hash iteration drives the callback, not a list of jobs.
        self.metadata
            .iter_tables(1, |table| {
                let id = table.id;
                if let Some(error @ ("cancelled" | "scan error")) = *self.failure.lock().unwrap() {
                    return Err(astersql_meta::errors::new(error));
                }
                visit(crate::AnalysisScanTable {
                    info: crate::TableInfo {
                        id,
                        ..Default::default()
                    },
                    schema_name: match id {
                        4 => None,
                        5 => Some("INFORMATION_SCHEMA".into()),
                        6 => Some("mysql".into()),
                        _ => Some("test".into()),
                    },
                    is_view: id == 7,
                    partitions: (id == 3 || id == 8).then(|| {
                        vec![
                            crate::PartitionDefinition {
                                id: id * 10,
                                name: "p0".into(),
                            },
                            crate::PartitionDefinition {
                                id: id * 10 + 1,
                                name: "p1".into(),
                            },
                        ]
                    }),
                })
                .map_err(astersql_meta::errors::new)?;
                // An eligible unlocked first table must be built before scanning t2.
                if id == 1 && !self.locked.lock().unwrap().contains(&1) {
                    assert!(
                        self.reads.lock().unwrap().contains(&1),
                        "jobs were deferred until scan completion"
                    );
                }
                Ok(())
            })
            .map_err(|error| error.to_string())
    }
}
fn port_da76_source(static_prune: bool) -> std::sync::Arc<PortDa76Source> {
    let mut metadata =
        astersql_meta::new_mutator(astersql_meta::kv::Transaction::default(), Vec::new());
    metadata
        .create_database(&astersql_meta::model::DbInfo {
            id: 1,
            name: astersql_meta::ast::CiString::new("test"),
            ..Default::default()
        })
        .unwrap();
    for id in 1..=9 {
        metadata
            .create_table_or_view(
                1,
                &astersql_meta::model::TableInfo {
                    id,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    std::sync::Arc::new(PortDa76Source {
        metadata,
        locked: std::sync::Mutex::new([1, 3, 80, 81].into_iter().collect()),
        static_prune,
        failure: std::sync::Mutex::new(None),
        reads: std::sync::Mutex::new(Vec::new()),
    })
}
fn port_da76_ids(queue: &crate::AnalysisPriorityQueue) -> Vec<i64> {
    let mut ids = queue
        .Snapshot()
        .unwrap()
        .0
        .into_iter()
        .map(|job| job.TableID)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}
#[test]
fn port_da76b94491_rebuild_locked_tables() {
    let source = port_da76_source(false);
    let queue = crate::NewAnalysisPriorityQueue(std::sync::Arc::new(crate::TableScanQueueSource(
        source.clone(),
    )));
    queue.Initialize().unwrap();
    assert_eq!(port_da76_ids(&queue), [2]);
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2]);
    let entries = astersql_statistics_handle_logutil::StatsLogger().entries();
    let scan = entries
        .iter()
        .rev()
        .find(|entry| entry.message == "Fetched all tables")
        .unwrap();
    assert!(
        scan.fields
            .contains(&astersql_statistics_handle_logutil::log::LogField::U64(
                "tableCount".into(),
                6
            ))
    );
    // Unlock t1: a subsequent rebuild must see the new lock snapshot.
    source.locked.lock().unwrap().remove(&1);
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [1, 2]);
    // Missing schema, system schemas, views and locked tables never fetch stats.
    assert!(
        !source
            .reads
            .lock()
            .unwrap()
            .iter()
            .any(|id| [3, 4, 5, 6, 7, 30, 31, 80, 81].contains(id))
    );
    queue.Close();
}
#[test]
fn port_da76b94491_rebuild_dynamic_locked_partitions() {
    let source = port_da76_source(false);
    let queue = crate::NewAnalysisPriorityQueue(std::sync::Arc::new(crate::TableScanQueueSource(
        source.clone(),
    )));
    queue.Initialize().unwrap();
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2]);
    source.locked.lock().unwrap().remove(&3);
    source.locked.lock().unwrap().insert(30);
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2, 3]);
    let mut found = false;
    while queue.Len().unwrap() > 0 {
        let job = queue.Pop().unwrap();
        if job.GetTableID() == 3 {
            let dynamic = job
                .as_any()
                .downcast_ref::<crate::DynamicPartitionedTableAnalysisJob>()
                .unwrap();
            assert_eq!(
                dynamic.PartitionIDs.keys().copied().collect::<Vec<_>>(),
                [31]
            );
            found = true;
        }
    }
    assert!(found);
    source.locked.lock().unwrap().insert(3);
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2]);
    queue.Close();
}
#[test]
fn port_da76b94491_rebuild_static_locked_partitions() {
    let source = port_da76_source(true);
    let queue = crate::NewAnalysisPriorityQueue(std::sync::Arc::new(crate::TableScanQueueSource(
        source.clone(),
    )));
    queue.Initialize().unwrap();
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2]);
    source.locked.lock().unwrap().remove(&80);
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2, 80]);
    source.locked.lock().unwrap().remove(&81);
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2, 80, 81]);
    source.locked.lock().unwrap().insert(8);
    queue.Rebuild().unwrap();
    assert_eq!(port_da76_ids(&queue), [2]);
    queue.Close();
}
#[test]
fn port_da76b94491_scan_errors_and_cancellation_propagate() {
    for error in ["parameters", "cancelled", "scan error"] {
        let source = port_da76_source(false);
        *source.failure.lock().unwrap() = Some(error);
        let queue = crate::NewAnalysisPriorityQueue(std::sync::Arc::new(
            crate::TableScanQueueSource(source.clone()),
        ));
        assert_eq!(queue.Initialize(), Err(error.into()));
        assert!(!queue.IsInitialized());
        *source.failure.lock().unwrap() = None;
        queue.Initialize().unwrap();
        *source.failure.lock().unwrap() = Some(error);
        assert_eq!(queue.Rebuild(), Err(error.into()));
        queue.Close();
    }
    let source = port_da76_source(false);
    let adapter = crate::TableScanQueueSource(source);
    let mut emitted = 0;
    assert_eq!(
        crate::QueueSource::visit_analysis_jobs(&adapter, &mut |_| {
            emitted += 1;
            Err("push failed".into())
        }),
        Err("push failed".into())
    );
    assert_eq!(emitted, 1, "callback error must stop scanning immediately");
}

#[test]
fn port_da76b94491_partial_scan_error_keeps_only_emitted_jobs() {
    use crate::QueueSource;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Partial {
        source: crate::TableScanQueueSource<Arc<PortDa76Source>>,
        fail: AtomicBool,
    }
    impl QueueSource for Partial {
        fn build_analysis_jobs(&self) -> Result<Vec<Box<dyn crate::AnalysisJob>>, String> {
            unreachable!("streaming queue must not collect")
        }
        fn visit_analysis_jobs(
            &self,
            visit: &mut dyn FnMut(Box<dyn crate::AnalysisJob>) -> Result<(), String>,
        ) -> Result<(), String> {
            self.source.visit_analysis_jobs(&mut |job| {
                visit(job)?;
                if self.fail.load(Ordering::Relaxed) {
                    Err("scan stopped".into())
                } else {
                    Ok(())
                }
            })
        }
    }
    let data = port_da76_source(false);
    data.locked.lock().unwrap().remove(&1);
    let source = Arc::new(Partial {
        source: crate::TableScanQueueSource(data),
        fail: AtomicBool::new(true),
    });
    let queue = crate::NewAnalysisPriorityQueue(source.clone());
    assert_eq!(queue.Initialize(), Err("scan stopped".into()));
    assert!(!queue.IsInitialized());
    source.fail.store(false, Ordering::Relaxed);
    queue.Initialize().unwrap();
    assert_eq!(port_da76_ids(&queue), [1, 2]);
    source.fail.store(true, Ordering::Relaxed);
    assert_eq!(queue.Rebuild(), Err("scan stopped".into()));
    assert_eq!(port_da76_ids(&queue), [1]);
    queue.Close();
}
