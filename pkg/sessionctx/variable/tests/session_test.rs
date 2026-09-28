// Copyright 2015 PingCAP, Inc.
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

// 会话变量与慢日志相关的 Go 迁移测试骨架。
//
// 主体保留在 `GO_REFERENCE` 字符串中的 Go 测试流程；文末若干用例已落地为可编译 Rust 测试。

const GO_REFERENCE: &str = r################"

// 主要测试函数、辅助函数、用例表、断言和资源收尾顺序均按源文件排列，便于后续人工逐段接入 Rust 测试框架。

// TestSetSystemVariable 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 对应 Go 同名测试：设置系统变量的成功/失败用例表。
#[test]
pub fn TestSetSystemVariable(t: &testing::T) {
    let mut v = variable::NewSessionVars(None);
    v.GlobalVarsAccessor = variable::NewMockGlobalAccessor4Tests();
    v.TimeZone = time.UTC;
    // Go 互斥锁保护共享 session/global 状态；Rust 实现时应显式处理所有权和锁生命周期。
    let mut mtx = new(sync.Mutex);

    let mut testCases = vec![/* []struct */ {
        key   string;
        value string;
        err   bool;
    }{
        {vardef::TxnIsolation, "SERIALIZABLE", true},
        {vardef::TimeZone, "xyz", true},
        {vardef::TiDBOptAggPushDown, "1", false},
        {vardef::TiDBOptDeriveTopN, "1", false},
        {vardef::TiDBOptDistinctAggPushDown, "1", false},
        {vardef::TiDBMemQuotaQuery, "1024", false},
        {vardef::TiDBMemQuotaApplyCache, "1024", false},
        {vardef::TiDBEnableStmtSummary, "1", true}, // now global only
        {vardef::TiDBEnableRowLevelChecksum, "1", true},
    }

    for tc in testCases {
        t.Run(tc.key, |t: &testing::T| {
            mtx.Lock();
            // 系统变量设置可能访问 global accessor 并返回校验错误，保留错误分支。
            let mut err = v.SetSystemVar(tc.key, tc.value);
            mtx.Unlock();
            if tc.err {
                require::Error(t, err);
            } else {
                require::NoError(t, err);
            }
        })
    }
}

// TestSession 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 对应 Go 同名测试：语句上下文行计数与 LastInsertID、重试重置。
#[test]
pub fn TestSession(t: &testing::T) {
    let mut ctx = mock::NewContext();

    let mut ss = ctx.GetSessionVars().StmtCtx;
    require::NotNil(t, ss);

    // For AffectedRows
    ss.AddAffectedRows(1);
    require::Equal(t, uint64(1), ss.AffectedRows());
    ss.AddAffectedRows(1);
    require::Equal(t, uint64(2), ss.AffectedRows());

    // For RecordRows
    ss.AddRecordRows(1);
    require::Equal(t, uint64(1), ss.RecordRows());
    ss.AddRecordRows(1);
    require::Equal(t, uint64(2), ss.RecordRows());

    // For FoundRows
    ss.AddFoundRows(1);
    require::Equal(t, uint64(1), ss.FoundRows());
    ss.AddFoundRows(1);
    require::Equal(t, uint64(2), ss.FoundRows());

    // For UpdatedRows
    ss.AddUpdatedRows(1);
    require::Equal(t, uint64(1), ss.UpdatedRows());
    ss.AddUpdatedRows(1);
    require::Equal(t, uint64(2), ss.UpdatedRows());

    // For TouchedRows
    ss.AddTouchedRows(1);
    require::Equal(t, uint64(1), ss.TouchedRows());
    ss.AddTouchedRows(1);
    require::Equal(t, uint64(2), ss.TouchedRows());

    // For CopiedRows
    ss.AddCopiedRows(1);
    require::Equal(t, uint64(1), ss.CopiedRows());
    ss.AddCopiedRows(1);
    require::Equal(t, uint64(2), ss.CopiedRows());

    // For last insert id
    ctx.GetSessionVars().SetLastInsertID(1);
    require::Equal(t, uint64(1), ctx.GetSessionVars().StmtCtx.LastInsertID);

    ss.ResetForRetry();
    require::Equal(t, uint64(0), ss.AffectedRows());
    require::Equal(t, uint64(0), ss.FoundRows());
    require::Equal(t, uint64(0), ss.UpdatedRows());
    require::Equal(t, uint64(0), ss.RecordRows());
    require::Equal(t, uint64(0), ss.TouchedRows());
    require::Equal(t, uint64(0), ss.CopiedRows());
    require::Equal(t, uint16(0), ss.WarningCount());
}

// TestSlowLogFormat 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 对应 Go 同名测试：慢日志格式化字段与输出内容。
#[test]
pub fn TestSlowLogFormat(t: &testing::T) {
    let mut store = testkit::CreateMockStore(t);
    let mut tk = testkit::NewTestKit(t, store);
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("use test");
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("create table t (id int primary key, v int)");
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("insert into t values (1,1), (2,2)");
    let mut seVar = tk.Session().GetSessionVars();
    require::NotNil(t, seVar);

    seVar.User = auth::UserIdentity{Username: "root", Hostname: "192.168.0.1"}
    seVar.ConnectionInfo = variable::ConnectionInfo{ClientIP: "192.168.0.1"}
    seVar.ConnectionID = 1;
    seVar.SessionAlias = "aliasabc";
    // the output of the logged CurrentDB should be 'test', should be to lower cased.
    seVar.SetCurrentDB("TeST");
    seVar.InRestrictedSQL = true;
    seVar.StmtCtx.WaitLockLeaseTime = 1;
    let mut txnTS = uint64(406649736972468225);
    let mut costTime = time.Second;
    let mut execDetail = execdetails::ExecDetails{
        RequestCount: 2,
        CopExecDetails: execdetails::CopExecDetails{
            BackoffTime: time.Millisecond,
            ScanDetail: util::ScanDetail{
                ProcessedKeys: 20001,
                TotalKeys:     10000,
            },
            TimeDetail: util::TimeDetail{
                ProcessTime: time.Second * time.Duration(2),
                WaitTime:    time.Minute,
            },
        },
    }
    let mut usedStats1 = stmtctx::UsedStatsInfoForTable{
        Name:                  "t1",
        TblInfo:               None,
        Version:               123,
        RealtimeCount:         1000,
        ModifyCount:           0,
        ColumnStatsLoadStatus: map_literal! {/* map[int64]string */ 2: "allEvicted", 3: "onlyCmsEvicted"},
        IndexStatsLoadStatus:  map_literal! {/* map[int64]string */ 1: "allLoaded", 2: "allLoaded"},
    }
    let mut usedStats2 = stmtctx::UsedStatsInfoForTable{
        Name:                  "t2",
        TblInfo:               None,
        Version:               0,
        RealtimeCount:         10000,
        ModifyCount:           0,
        ColumnStatsLoadStatus: map_literal! {/* map[int64]string */ 2: "unInitialized"},
    }

    let mut processTimeStats = execdetails::TaskTimeStats{
        AvgTime:    time.Second,
        P90Time:    time.Second * 2,
        MaxAddress: "10.6.131.78",
        MaxTime:    time.Second * 3,
    }
    let mut waitTimeStats = execdetails::TaskTimeStats{
        AvgTime:    time.Millisecond * 10,
        P90Time:    time.Millisecond * 20,
        MaxTime:    time.Millisecond * 30,
        MaxAddress: "10.6.131.79",
    }
    let mut copTasks = execdetails::CopTasksDetails{
        NumCopTasks:         10,
        ProcessTimeStats:    processTimeStats,
        WaitTimeStats:       waitTimeStats,
        BackoffTimeStatsMap: make(map[string]execdetails::TaskTimeStats),
        TotBackoffTimes:     make(map[string]int),
    }

    let mut backoffs = vec![/* []string */ "rpcTiKV", "rpcPD", "regionMiss"}
    for backoff in backoffs {
        copTasks.BackoffTimeStatsMap[backoff] = execdetails::TaskTimeStats{
            MaxTime:    time.Millisecond * 200,
            MaxAddress: "127.0.0.1",
            AvgTime:    time.Millisecond * 200,
            P90Time:    time.Millisecond * 200,
            TotTime:    time.Millisecond * 200,
        }
        copTasks.TotBackoffTimes[backoff] = 200;
    }

    let mut memMax: /* Go type */ int64 = 2333;
    let mut diskMax: /* Go type */ int64 = 6666;
    let mut resultFields = `# Txn_start_ts: 406649736972468225;
# Keyspace_name: keyspace_a
# Keyspace_ID: 1
# User@Host: root[root] @ 192.168.0.1 [192.168.0.1]
# Conn_ID: 1
# Session_alias: aliasabc
# Exec_retry_time: 5.1 Exec_retry_count: 3
# Query_time: 1
# Parse_time: 0.00000001
# Compile_time: 0.00000001
# Rewrite_time: 0.000000003 Preproc_subqueries: 2 Preproc_subqueries_time: 0.000000002
# Optimize_time: 0.00000001 Opt_logical: 0.00000001 Opt_physical: 0.00000001 Opt_binding_match: 0.00000001 Opt_stats_sync_wait: 0.00000001 Opt_stats_derive: 0.00000001
# Wait_TS: 0.000000003
# Process_time: 2 Wait_time: 60 Backoff_time: 0.001 Request_count: 2 Process_keys: 20001 Total_keys: 10000
# DB: test
# Index_names: [t1:a,t2:b]
# Is_internal: true
# Digest: e5796985ccafe2f71126ed6c0ac939ffa015a8c0744a24b7aee6d587103fd2f7
# Stats: t1:stats_meta_version=123[realtime_count=1000;modify_count=0][ID 1:allLoaded,ID 2:allLoaded][ID 2:allEvicted,ID 3:onlyCmsEvicted],t2:stats_meta_version=pseudo[realtime_count=10000;modify_count=0]
# Num_cop_tasks: 10
# Cop_proc_avg: 1 Cop_proc_p90: 2 Cop_proc_max: 3 Cop_proc_addr: 10.6.131.78
# Cop_wait_avg: 0.01 Cop_wait_p90: 0.02 Cop_wait_max: 0.03 Cop_wait_addr: 10.6.131.79
# Cop_backoff_regionMiss_total_times: 200 Cop_backoff_regionMiss_total_time: 0.2 Cop_backoff_regionMiss_max_time: 0.2 Cop_backoff_regionMiss_max_addr: 127.0.0.1 Cop_backoff_regionMiss_avg_time: 0.2 Cop_backoff_regionMiss_p90_time: 0.2
# Cop_backoff_rpcPD_total_times: 200 Cop_backoff_rpcPD_total_time: 0.2 Cop_backoff_rpcPD_max_time: 0.2 Cop_backoff_rpcPD_max_addr: 127.0.0.1 Cop_backoff_rpcPD_avg_time: 0.2 Cop_backoff_rpcPD_p90_time: 0.2
# Cop_backoff_rpcTiKV_total_times: 200 Cop_backoff_rpcTiKV_total_time: 0.2 Cop_backoff_rpcTiKV_max_time: 0.2 Cop_backoff_rpcTiKV_max_addr: 127.0.0.1 Cop_backoff_rpcTiKV_avg_time: 0.2 Cop_backoff_rpcTiKV_p90_time: 0.2
# Mem_max: 2333
# Mem_arbitration: 0.000054321
# Disk_max: 6666
# Prepared: true
# Plan_from_cache: true
# Plan_from_binding: true
# Has_more_results: true
# KV_total: 10
# PD_total: 11
# Backoff_total: 12
# Unpacked_bytes_sent_tikv_total: 0
# Unpacked_bytes_received_tikv_total: 0
# Unpacked_bytes_sent_tikv_cross_zone: 0
# Unpacked_bytes_received_tikv_cross_zone: 0
# Unpacked_bytes_sent_tiflash_total: 0
# Unpacked_bytes_received_tiflash_total: 0
# Unpacked_bytes_sent_tiflash_cross_zone: 0
# Unpacked_bytes_received_tiflash_cross_zone: 0
# Write_sql_response_total: 1
# Result_rows: 12345
# Succ: true
# IsExplicitTxn: true
# IsSyncStatsFailed: false
# IsWriteCacheTable: true
# Resource_group: rg1
# Request_unit_read: 50
# Request_unit_write: 100.56
# Time_queued_by_rc: 0.134
# Storage_from_kv: true
# Storage_from_mpp: false`
    let mut sql = "select * from t;";
    let mut _, digest = parser.NormalizeDigest(sql);
    let mut tikvExecDetail = util::ExecDetails{
        WaitKVRespDuration: (10 * time.Second).Nanoseconds(),
        WaitPDRespDuration: (11 * time.Second).Nanoseconds(),
        BackoffDuration:    (12 * time.Second).Nanoseconds(),
    }
    let mut ruDetails = util::NewRUDetailsWith(50.0, 100.56, 134*time.Millisecond);
    seVar.DurationParse = time.Duration(10);
    seVar.DurationCompile = time.Duration(10);
    seVar.DurationOptimizer.Total = time.Duration(10);
    seVar.DurationOptimizer.BindingMatch = time.Duration(10);
    seVar.DurationOptimizer.StatsSyncWait = time.Duration(10);
    seVar.DurationOptimizer.LogicalOpt = time.Duration(10);
    seVar.DurationOptimizer.PhysicalOpt = time.Duration(10);
    seVar.DurationOptimizer.StatsDerive = time.Duration(10);
    seVar.DurationOptimizer.TiFlashInfoFetch = time.Duration(10);
    seVar.DurationWaitTS = time.Duration(3);
    let mut logItems = variable::SlowQueryLogItems{
        TxnTS:             txnTS,
        KeyspaceName:      "keyspace_a",
        KeyspaceID:        1,
        SQL:               sql,
        Digest:            digest.String(),
        TimeTotal:         costTime,
        IndexNames:        "[t1:a,t2:b]",
        CopTasks:          copTasks,
        ExecDetail:        execDetail,
        MemMax:            memMax,
        DiskMax:           diskMax,
        Prepared:          true,
        PlanFromCache:     true,
        PlanFromBinding:   true,
        HasMoreResults:    true,
        KVExecDetail:      &tikvExecDetail,
        WriteSQLRespTotal: 1 * time.Second,
        ResultRows:        12345,
        Succ:              true,
        RewriteInfo: variable::RewritePhaseInfo{
            DurationRewrite:            3,
            DurationPreprocessSubQuery: 2,
            PreprocessSubQueries:       2,
        },
        ExecRetryCount:    3,
        ExecRetryTime:     5*time.Second + time.Millisecond*100,
        IsExplicitTxn:     true,
        IsWriteCacheTable: true,
        UsedStats:         stmtctx::UsedStatsInfo{},
        ResourceGroupName: "rg1",
        RUDetails:         ruDetails,
        StorageKV:         true,
        StorageMPP:        false,
        MemArbitration:    time.Duration(54321).Seconds(),
    }
    logItems.UsedStats.RecordUsedInfo(1, usedStats1);
    logItems.UsedStats.RecordUsedInfo(2, usedStats2);
    seVar.CurrentDBChanged = false;
    let mut logString = seVar.SlowLogFormat(logItems);
    require::Equal(t, resultFields+"\n"+sql, logString);
    require::NotContains(t, logString, variable::SlowLogSessionConnectAttrs);

    seVar.CurrentDBChanged = true;
    logString = seVar.SlowLogFormat(logItems);
    require::Equal(t, resultFields+"\n"+"use test;\n"+sql, logString);
    require::False(t, seVar.CurrentDBChanged);

    // Verify SessionConnectAttrs serialization.
    logItems.SessionConnectAttrs = map_literal! {/* map[string]string */ ;
        "_client_name": "libmysql",
        "_os":          "Linux",
        "app_name":     "test_svc",
    }
    logString = seVar.SlowLogFormat(logItems);
    // json.Encoder sorts map keys, so the output is deterministic.
    let mut expectedAttrsLine = `# Session_connect_attrs: {"_client_name":"libmysql","_os":"Linux","app_name":"test_svc"}`;
    require::Contains(t, logString, expectedAttrsLine);
    seVar.EnableRedactLog = vardef::On;
    logString = seVar.SlowLogFormat(logItems);
    require::Contains(t, logString, expectedAttrsLine);
    seVar.EnableRedactLog = vardef::Off;
    // Session_connect_attrs should appear after Storage_from_mpp, before Prev_stmt, and before the SQL.
    let mut attrsIdx = strings.Index(logString, "Session_connect_attrs");
    let mut mppIdx = strings.Index(logString, variable::SlowLogStorageFromMPP);
    let mut prevStmtIdx = strings.Index(logString, variable::SlowLogPrevStmt);
    let mut sqlIdx = strings.Index(logString, sql);
    require::Greater(t, attrsIdx, 0);
    require::Greater(t, mppIdx, 0);
    require::Greater(t, attrsIdx, mppIdx, "Session_connect_attrs should appear after Storage_from_mpp");
    if prevStmtIdx > 0 {
        require::Less(t, attrsIdx, prevStmtIdx, "Session_connect_attrs should appear before Prev_stmt");
    }
    require::Less(t, attrsIdx, sqlIdx, "Session_connect_attrs should appear before the SQL statement");

    // Verify reserved truncation metadata key is serialized as expected.
    logItems.SessionConnectAttrs = map_literal! {/* map[string]string */ ;
        "_truncated": "4",
        "app_name":   "test_svc",
    }
    logString = seVar.SlowLogFormat(logItems);
    require::Contains(t, logString, `# Session_connect_attrs: {"_truncated":"4","app_name":"test_svc"}`);
    // Restore for subsequent assertions.
    logItems.SessionConnectAttrs = None;

    let mut restore = config::RestoreFunc();
    // Go defer 用于恢复全局配置或释放资源；Rust 接线时需要改成 guard/drop 语义。
    defer restore();
    config::UpdateGlobal(|conf: &mut config::Config| {
        conf.KeyspaceObservability = config::KeyspaceObservability{
            Fields: vec![/* []config::KeyspaceObservabilityField */ {
                Source:       "meta_a",
                SlowLogField: "Keyspace_meta_slow_a",
            }},
        }
        require::NoError(t, conf.ResolveKeyspaceObservability(map_literal! {/* map[string]string */ "meta_a": "value_a"}));
    })
    logString = seVar.SlowLogFormat(logItems);
    require::Equal(t, resultFields+"\n"+"# Keyspace_meta_slow_a: value_a\n"+sql, logString);

    // test PrepareSlowLogItemsForRules and CompleteSlowLogItemsForRules
    // 慢日志规则在 Go 中写入 session vars 后参与匹配；这里只描述规则装配过程。
    seVar.SlowLogRules = slowlogrule::NewSessionSlowLogRules(slowlogrule::SlowLogRules{
        Fields: map_literal! {/* map[string]struct */ }{
            strings::ToLower(variable::SlowLogDBStr):          {},
            strings::ToLower(variable::SlowLogSucc):           {},
            strings::ToLower(execdetails::ProcessTimeStr):     {},
            strings::ToLower(variable::SlowLogResourceGroup):  {},
            strings::ToLower(variable::SlowLogExecRetryCount): {},
        },
    })
    // 执行明细聚合依赖 Go 结构体和内部锁，保留 Reset/Merge/Setter 的调用顺序。
    seVar.StmtCtx.SyncExecDetails.Reset();
    // 执行明细聚合依赖 Go 结构体和内部锁，保留 Reset/Merge/Setter 的调用顺序。
    seVar.StmtCtx.SyncExecDetails.MergeCopExecDetails(&execDetail.CopExecDetails, 0);
    // Make RequestCount to be 2.
    // 执行明细聚合依赖 Go 结构体和内部锁，保留 Reset/Merge/Setter 的调用顺序。
    seVar.StmtCtx.SyncExecDetails.MergeCopExecDetails(execdetails::CopExecDetails{}, 0);
    seVar.StmtCtx.ExecRetryCount = logItems.ExecRetryCount;
    seVar.StmtCtx.ResourceGroupName = logItems.ResourceGroupName;
    let mut ctx = context::WithValue(context::Background(), execdetails::StmtExecDetailKey,
        execdetails::StmtExecDetails{WriteSQLRespDuration: logItems.WriteSQLRespTotal});
    seVar.RUV2Metrics = execdetails::NewRUV2Metrics();
    seVar.RUV2Metrics.AddResultChunkCells(11);
    seVar.RUV2Metrics.AddPlanCnt(2);
    // 慢日志规则在 Go 中写入 session vars 后参与匹配；这里只描述规则装配过程。
    let mut actual = executor.PrepareSlowLogItemsForRules(ctx, vardef::GlobalSlowLogRules.Load(), seVar);
    let mut childCtx = context::WithValue(ctx, util::ExecDetailsKey, &tikvExecDetail);
    executor.CompleteSlowLogItemsForRules(childCtx, seVar, actual);
    let mut stmt, err = parser::New().ParseOneStmt(sql, "", "");
    require::NoError(t, err);
    // make StmtCtx.OriginalSQL is the same as sql
    seVar.StmtCtx.OriginalSQL = sql;
    seVar.StmtCtx.ResetSQLDigest(sql);
    *seVar.StmtCtx.IndexNames.lock().unwrap() = vec![/* []string */ "t1:a", "t2:b"}
    seVar.TxnCtx.IsExplicit = logItems.IsExplicitTxn;
    seVar.FoundInPlanCache = logItems.PlanFromCache;
    seVar.FoundInBinding = logItems.PlanFromBinding;
    seVar.RewritePhaseInfo = logItems.RewriteInfo;

    // mock MemArbitration value for MemTracker
    memory::SetupGlobalMemArbitratorForTest(t.TempDir());
    // Go defer 用于恢复全局配置或释放资源；Rust 接线时需要改成 guard/drop 语义。
    defer memory::CleanupGlobalMemArbitratorForTest();
    require::True(t, memory::SetGlobalMemArbitratorWorkMode(memory::ArbitratorModeStandardName));
    let mut memTracker = seVar.StmtCtx.MemTracker;
    require::True(t, memTracker.InitMemArbitratorForTest());
    memTracker.MemArbitrator.AwaitAlloc.TotalDur.Store(int64(logItems.MemArbitration * float64(time.Second.Nanoseconds())));

    // get an ExecStmt
    let mut compiler = executor.Compiler{Ctx: tk.Session()}
    let mut execStmt, err = compiler.Compile(childCtx, stmt);
    execStmt.GoCtx = childCtx;
    require::NoError(t, err);

    executor.SetSlowLogItems(execStmt, txnTS, logItems.HasMoreResults, actual);
    logItems.RUV2Metrics = seVar.RUV2Metrics.Clone();
    compareSlowLogItems(t, logItems, actual);
}

// TestSlowLogFormatIncludesTiFlashRUInRUV2Metrics 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 慢日志 RU v2 指标应包含 TiFlash RU。
#[test]
pub fn TestSlowLogFormatIncludesTiFlashRUInRUV2Metrics(t: &testing::T) {
    let mut seVar = variable::NewSessionVars(None);
    let mut logItems = variable::SlowQueryLogItems{
        SQL:         "select 1",
        Digest:      "digest",
        TimeTotal:   time.Second,
        Succ:        true,
        ExecDetail:  execdetails::ExecDetails{},
        UsedStats:   stmtctx::UsedStatsInfo{},
        RUDetails:   util::NewRUDetailsWith(0, 0, 0),
        RUV2Metrics: execdetails::NewRUV2Metrics(),
    }
    logItems.RUDetails.AddTiKVRUV2(100);
    logItems.RUDetails.UpdateTiFlash(rmpb::Consumption{RRU: 20, WRU: 30});

    let mut logString = seVar.SlowLogFormat(logItems);
    require::Contains(t, logString, "# Request_unit_v2: 150.00");
    require::Contains(t, logString, "# Request_unit_v2_detail: total_ru:150.00, tidb_ru:0.00, tikv_ru:100.00, tiflash_ru:50.00");

    t.Run("default session weights come from config defaults", |t: &testing::T| {
        let mut original = config::GetGlobalConfig();
        t.Cleanup(|| {
            if original != None {
                config::StoreGlobalConfig(original);
            }
        })

        let mut cfg = config::NewConfig();
        cfg.RUV2 = config::DefaultRUV2Config();
        config::StoreGlobalConfig(cfg);

        require::Equal(t, execdetails::RUV2Weights{
            RUScale:                 cfg.RUV2.RUScale,
            ResultChunkCells:        cfg.RUV2.ResultChunkCells,
            ExecutorL1:              cfg.RUV2.ExecutorL1,
            ExecutorL2:              cfg.RUV2.ExecutorL2,
            ExecutorL3:              cfg.RUV2.ExecutorL3,
            ExecutorL5InsertRows:    cfg.RUV2.ExecutorL5InsertRows,
            PlanCnt:                 cfg.RUV2.PlanCnt,
            PlanDeriveStatsPaths:    cfg.RUV2.PlanDeriveStatsPaths,
            ResourceManagerReadCnt:  cfg.RUV2.ResourceManagerReadCnt,
            ResourceManagerWriteCnt: cfg.RUV2.ResourceManagerWriteCnt,
            WriteKeys:               cfg.RUV2.WriteKeys,
            SessionParserTotal:      cfg.RUV2.SessionParserTotal,
            TxnCnt:                  cfg.RUV2.TxnCnt,
        }, variable::NewSessionVars(None).RUV2Weights())
    })
}

// compareSlowLogItems 对应 Go helper：参数 `t *testing.T, expected, actual *variable.SlowQueryLogItems`，返回 `无显式返回值`；保留调用形状供后续接线。
/// 比较期望与实际 SlowQueryLogItems 字段（Go 迁移桩）。
pub fn compareSlowLogItems(/* Go 参数: t *testing.T, expected, actual *variable.SlowQueryLogItems */) /* Go 返回: 无显式返回值 */ {
    require::NotNil(t, expected);
    require::NotNil(t, actual);

    let mut ev = reflect.ValueOf(expected).Elem();
    let mut av = reflect.ValueOf(actual).Elem();
    let mut et = ev.Type();

    // Some fields are hard to mock, so we skip them.
    let mut skipFields = vec![/* []string */ "KeyspaceID", "KeyspaceName", "TimeTotal", "Prepared", "ResultRows", "ResultRows", "Plan", "BinaryPlan",
        "UsedStats", "CopTasks", "RewriteInfo", "ExecRetryTime", "Warnings", "RUDetails", "RUV2Metrics", "MemMax", "DiskMax", "StorageKV"}
    let mut skipFieldsFunc = |res: &str, fields: Vec<&str>| -> bool {
        for f in fields {
            if res == f {
                return true;
            }
        }
        return false;
    }

    for i in 0..ev.NumField() {
        let mut field = et.Field(i);
        let mut expVal = ev.Field(i).Interface();
        let mut actVal = av.Field(i).Interface();

        if skipFieldsFunc(field.Name, skipFields) {
            continue;
        }
        if ev.Field(i).Kind() == reflect.Ptr {
            if ev.Field(i).IsNil() && av.Field(i).IsNil() {
                continue;
            }
            require::Equal(t, expVal, actVal, "field %s mismatch", field.Name);
        } else {
            require::Equal(t, expVal, actVal, "field %s mismatch", field.Name);
        }
    }
}

// TestIsolationRead 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 隔离读引擎集合的会话设置。
#[test]
pub fn TestIsolationRead(t: &testing::T) {
    // Go defer 用于恢复全局配置或释放资源；Rust 接线时需要改成 guard/drop 语义。
    defer config::RestoreFunc()();
    config::UpdateGlobal(|conf: &mut config::Config| {
        conf.IsolationRead.Engines = vec![/* []string */ "tiflash", "tidb"}
    })
    let mut sessVars = variable::NewSessionVars(None);
    let mut _, ok = sessVars.IsolationReadEngines[kv::TiDB];
    require::True(t, ok);
    _, ok = sessVars.IsolationReadEngines[kv::TiKV];
    require::False(t, ok);
    _, ok = sessVars.IsolationReadEngines[kv::TiFlash];
    require::True(t, ok);
}

// TestTableDeltaClone 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 表增量（table delta）克隆独立性。
#[test]
pub fn TestTableDeltaClone(t: &testing::T) {
    let mut td0 = variable::TableDelta{
        Delta:    1,
        Count:    2,
        InitTime: time::Now(),
    }
    let mut td1 = td0.Clone();
    require::Equal(t, td0, td1);

    let mut td2 = td0.Clone();
    require::Equal(t, td0, td2);
    td0.InitTime = td0.InitTime.Add(time.Second);
    require::NotEqual(t, td0, td2);
}

// TestTransactionContextSavepoint 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 事务上下文 savepoint 保存与回滚。
#[test]
pub fn TestTransactionContextSavepoint(t: &testing::T) {
    let mut tc = variable::TransactionContext{
        TxnCtxNeedToRestore: variable::TxnCtxNeedToRestore{
            TableDeltaMap: map_literal! {/* map[int64]variable::TableDelta */
                1: {
                    Delta:    1,
                    Count:    2,
                    InitTime: time::Now(),
                },
            },
        },
    }
    tc.SetPessimisticLockCache(vec![/* []byte */ 'a'}, vec![/* []byte */ 'a'});
    tc.FlushStmtPessimisticLockCache();

    tc.AddSavepoint("S1", None);
    require::Equal(t, 1, len(tc.Savepoints));
    require::Equal(t, 1, len(tc.Savepoints[0].TxnCtxSavepoint.TableDeltaMap));
    require::Equal(t, "s1", tc.Savepoints[0].Name);

    let mut succ = tc.DeleteSavepoint("s2");
    require::False(t, succ);
    require::Equal(t, 1, len(tc.Savepoints));

    tc.TableDeltaMap[2] = variable::TableDelta{
        Delta:    6,
        Count:    7,
        InitTime: time::Now(),
    }
    tc.SetPessimisticLockCache(vec![/* []byte */ 'b'}, vec![/* []byte */ 'b'});
    tc.FlushStmtPessimisticLockCache();

    tc.AddSavepoint("S2", None);
    require::Equal(t, 2, len(tc.Savepoints));
    require::Equal(t, 1, len(tc.Savepoints[0].TxnCtxSavepoint.TableDeltaMap));
    require::Equal(t, "s1", tc.Savepoints[0].Name);
    require::Equal(t, 2, len(tc.Savepoints[1].TxnCtxSavepoint.TableDeltaMap));
    require::Equal(t, "s2", tc.Savepoints[1].Name);

    tc.TableDeltaMap[3] = variable::TableDelta{
        Delta:    10,
        Count:    11,
        InitTime: time::Now(),
    }
    tc.SetPessimisticLockCache(vec![/* []byte */ 'c'}, vec![/* []byte */ 'c'});
    tc.FlushStmtPessimisticLockCache();

    tc.AddSavepoint("s2", None);
    require::Equal(t, 2, len(tc.Savepoints));
    require::Equal(t, 3, len(tc.Savepoints[1].TxnCtxSavepoint.TableDeltaMap));
    require::Equal(t, "s2", tc.Savepoints[1].Name);

    tc.RollbackToSavepoint("s1");
    require::Equal(t, 1, len(tc.Savepoints));
    require::Equal(t, 1, len(tc.Savepoints[0].TxnCtxSavepoint.TableDeltaMap));
    require::Equal(t, "s1", tc.Savepoints[0].Name);
    let mut val, ok = tc.GetKeyInPessimisticLockCache(vec![/* []byte */ 'a'});
    require::True(t, ok);
    require::Equal(t, vec![/* []byte */ 'a'}, val);
    val, ok = tc.GetKeyInPessimisticLockCache(vec![/* []byte */ 'b'});
    require::False(t, ok);
    require::Nil(t, val);

    succ = tc.DeleteSavepoint("s1");
    require::True(t, succ);
    require::Equal(t, 0, len(tc.Savepoints));
}

// TestNonPreparedPlanCacheStmt 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 非预处理计划缓存语句登记。
#[test]
pub fn TestNonPreparedPlanCacheStmt(t: &testing::T) {
    let mut sessVars = variable::NewSessionVars(None);
    sessVars.SessionPlanCacheSize = 100;
    let mut sql1 = "select * from t where a>?";
    let mut sql2 = "select * from t where a<?";
    require::Nil(t, sessVars.GetNonPreparedPlanCacheStmt(sql1));
    require::Nil(t, sessVars.GetNonPreparedPlanCacheStmt(sql2));

    sessVars.AddNonPreparedPlanCacheStmt(sql1, new(plannercore::PlanCacheStmt));
    require::NotNil(t, sessVars.GetNonPreparedPlanCacheStmt(sql1));
    require::Nil(t, sessVars.GetNonPreparedPlanCacheStmt(sql2));

    sessVars.AddNonPreparedPlanCacheStmt(sql2, new(plannercore::PlanCacheStmt));
    require::NotNil(t, sessVars.GetNonPreparedPlanCacheStmt(sql1));
    require::NotNil(t, sessVars.GetNonPreparedPlanCacheStmt(sql2));
}

// TestHookContext 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 钩子上下文绑定与取回。
#[test]
pub fn TestHookContext(t: &testing::T) {
    let mut store = testkit::CreateMockStore(t);
    let mut ctx = mock::NewContext();
    ctx.Store = store;
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "testhooksysvar", Value: vardef::On, Type: vardef::TypeBool, SetSession: |s: &mut variable::SessionVars, val: &str| -> Result<(), errors::Error> {
        require::Equal(t, s.GetStore(), store);
        return Ok(());
    }}
    variable::RegisterSysVar(&sv);

    // 系统变量设置可能访问 global accessor 并返回校验错误，保留错误分支。
    ctx.GetSessionVars().SetSystemVar("testhooksysvar", "test");
}

// TestGetReuseChunk 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 可复用 Chunk 的借还与容量。
#[test]
pub fn TestGetReuseChunk(t: &testing::T) {
    let mut fieldTypes = []*types::FieldType{
        types::NewFieldTypeBuilder().SetType(mysql::TypeVarchar).BuildP(),
        types::NewFieldTypeBuilder().SetType(mysql::TypeJSON).BuildP(),
        types::NewFieldTypeBuilder().SetType(mysql::TypeFloat).BuildP(),
        types::NewFieldTypeBuilder().SetType(mysql::TypeNewDecimal).BuildP(),
        types::NewFieldTypeBuilder().SetType(mysql::TypeDouble).BuildP(),
        types::NewFieldTypeBuilder().SetType(mysql::TypeLonglong).BuildP(),
        types::NewFieldTypeBuilder().SetType(mysql::TypeDatetime).BuildP(),
    }

    let mut sessVars = variable::NewSessionVars(None);

    // SetAlloc efficient
    sessVars.SetAlloc(None);
    require::False(t, sessVars.IsAllocValid());
    require::False(t, sessVars.GetUseChunkAlloc());
    // alloc is nil ，Allocate memory from the system
    let mut chk1 = sessVars.GetChunkAllocator().Alloc(fieldTypes, 10, 10);
    require::NotNil(t, chk1);

    let mut chunkReuseMap = make(map_literal! {/* map[*chunk::Chunk]struct */ }, 14);
    let mut columnReuseMap = make(map_literal! {/* map[*chunk::Column]struct */ }, 14);

    let mut alloc = chunk::NewAllocator();
    sessVars.EnableReuseChunk = true;
    sessVars.SetAlloc(alloc);
    require::True(t, sessVars.IsAllocValid());
    require::False(t, sessVars.GetUseChunkAlloc());

    //tries to apply from the cache
    let mut initCap = 10;
    chk1 = sessVars.GetChunkAllocator().Alloc(fieldTypes, initCap, initCap);
    require::NotNil(t, chk1);
    chunkReuseMap[chk1] = struct{}{}
    for i in 0..chk1.NumCols() {
        columnReuseMap[chk1.Column(i)] = struct{}{}
    }
    require::True(t, sessVars.GetUseChunkAlloc());

    alloc.Reset();
    let mut chkres1 = sessVars.GetChunkAllocator().Alloc(fieldTypes, 10, 10);
    require::NotNil(t, chkres1);
    let mut _, exist = chunkReuseMap[chkres1];
    require::True(t, exist);
    for i in 0..chkres1.NumCols() {
        let mut _, exist = columnReuseMap[chkres1.Column(i)];
        require::True(t, exist);
    }

    let mut allocpool: /* Go type */ chunk::Allocator = alloc;
    sessVars.ClearAlloc(&allocpool, false);
    require::Equal(t, alloc, allocpool);

    sessVars.ClearAlloc(&allocpool, true);
    require::NotEqual(t, allocpool, alloc);
    require::False(t, sessVars.IsAllocValid());
}

// TestUserVarConcurrently 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 用户变量并发读写安全性。
#[test]
pub fn TestUserVarConcurrently(t: &testing::T) {
    let mut sv = variable::NewSessionVars(None);
    let mut ctx, cancel = context.WithTimeout(context::Background(), time.Second);
    // Go WaitGroup 用于等待并发子任务；保留同步边界。
    let mut wg: /* Go type */ util2.WaitGroupWrapper;
    wg.Run(|| {
        for i in 0.. {
            select {
            case <-time.After(time.Millisecond):
                let mut name = strconv::Itoa(i);
                sv.SetUserVarVal(name, types::Datum{});
                sv.GetUserVarVal(name);
            case <-ctx.Done():
                return;
            }
        }
    })
    wg.Run(|| {
        for {
            select {
            case <-time.After(time.Millisecond):
                let mut states: /* Go type */ sessionstates::SessionStates;
                require::NoError(t, sv.EncodeSessionStates(ctx, &states));
                require::NoError(t, sv.DecodeSessionStates(ctx, &states));
            case <-ctx.Done():
                return;
            }
        }
    })
    wg.Wait();
    cancel();
}

// TestSetStatus 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 会话状态标志位设置。
#[test]
pub fn TestSetStatus(t: &testing::T) {
    let mut sv = variable::NewSessionVars(None);
    require::True(t, sv.IsAutocommit());
    sv.SetStatusFlag(mysql::ServerStatusInTrans, true);
    require::True(t, sv.InTxn());
    sv.SetStatusFlag(mysql::ServerStatusCursorExists, true);
    require::True(t, sv.InTxn());
    sv.SetStatusFlag(mysql::ServerStatusInTrans, false);
    require::True(t, sv.HasStatusFlag(mysql::ServerStatusCursorExists));
    require::False(t, sv.InTxn());
    require::Equal(t, mysql::ServerStatusAutocommit|mysql::ServerStatusCursorExists, sv.Status());
}

// TestRowIDShardGenerator 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// RowID 分片生成器取值范围。
#[test]
pub fn TestRowIDShardGenerator(t: &testing::T) {
    let mut g = variable::NewRowIDShardGenerator(rand.New(rand.NewSource(12345)), 128) // #nosec G404);
    // default settings
    require::Equal(t, 128, g.GetShardStep());
    let mut shard = g.GetCurrentShard(127);
    require::Equal(t, int64(3535546008), shard);
    require::Equal(t, shard, g.GetCurrentShard(1));
    // reset alloc step
    g.SetShardStep(5);
    require::Equal(t, 5, g.GetShardStep());
    // generate shard in step
    shard = g.GetCurrentShard(1);
    require::Equal(t, int64(1371624976), shard);
    require::Equal(t, shard, g.GetCurrentShard(1));
    require::Equal(t, shard, g.GetCurrentShard(1));
    require::Equal(t, shard, g.GetCurrentShard(2));
    // generate shard in next step
    shard = g.GetCurrentShard(1);
    require::Equal(t, int64(895725277), shard);
    // set step will reset clear remain
    g.SetShardStep(5);
    require::NotEqual(t, shard, g.GetCurrentShard(1));
}

// TestUserVars 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 用户变量 set/get/unset。
#[test]
pub fn TestUserVars(t: &testing::T) {
    let mut vars = variable::NewUserVars();
    vars.SetUserVarVal("a", types::NewIntDatum(1));
    vars.SetUserVarVal("b", types::NewStringDatum("v2"));
    let mut dt, ok = vars.GetUserVarVal("a");
    require::True(t, ok);
    require::Equal(t, types::NewIntDatum(1), dt);

    vars.SetUserVarType("a", types::NewFieldType(mysql::TypeLonglong));
    let mut tp, ok = vars.GetUserVarType("a");
    require::True(t, ok);
    require::Equal(t, types::NewFieldType(mysql::TypeLonglong), tp);

    vars.UnsetUserVar("a");
    _, ok = vars.GetUserVarVal("a");
    require::False(t, ok);
    _, ok = vars.GetUserVarType("a");
    require::False(t, ok);

    dt, ok = vars.GetUserVarVal("b");
    require::True(t, ok);
    require::Equal(t, types::NewStringDatum("v2"), dt);
}

// TestTiDBOptPartialOrderedIndexForTopNSessionAndGlobal 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 部分有序索引 TopN 优化的会话与全局变量。
#[test]
pub fn TestTiDBOptPartialOrderedIndexForTopNSessionAndGlobal(t: &testing::T) {
    let mut store = testkit::CreateMockStore(t);
    let mut tk = testkit::NewTestKit(t, store);
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("use test");

    // Test default value
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));

    // Test session scope
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = COST");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@session.tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));
    // Global should not be affected
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));

    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = DISABLE");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));

    // Test global scope
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@global.tidb_opt_partial_ordered_index_for_topn = COST");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));
    // New session should inherit global value
    let mut tk1 = testkit::NewTestKit(t, store);
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk1.MustExec("use test");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk1.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));

    // Session value should override global value
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = DISABLE");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));
    // Global should still be COST
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));

    // Test case-insensitive values (only DISABLE, COST are allowed)
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = 'cost'");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = 'disable'");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = 'Cost'");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = 'Disable'");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = 'COST'");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("COST"));
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = 'DISABLE'");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@tidb_opt_partial_ordered_index_for_topn").Check(testkit.Rows("DISABLE"));

    // Test disallowed values (old values and invalid values)
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 'ON'"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 'OFF'"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 0"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 1"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 'true'"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 'false'"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 2"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = -1"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 'yes'"));
    require::Error(t, tk.ExecToErr("set @@tidb_opt_partial_ordered_index_for_topn = 'no'"));

    // Verify the field is accessible in SessionVars
    let mut vars = tk.Session().GetSessionVars();
    require::Equal(t, "DISABLE", vars.OptPartialOrderedIndexForTopN);
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set @@tidb_opt_partial_ordered_index_for_topn = COST");
    require::Equal(t, "COST", vars.OptPartialOrderedIndexForTopN);
}

// TestTiDBOptPartialOrderedIndexForTopN 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 部分有序索引 TopN 优化开关枚举校验。
#[test]
pub fn TestTiDBOptPartialOrderedIndexForTopN(t: &testing::T) {
    // Test that the variable exists and has correct properties
    let mut sv = variable::GetSysVar(vardef::TiDBOptPartialOrderedIndexForTopN);
    require::NotNil(t, sv);
    require::True(t, sv.HasSessionScope());
    require::True(t, sv.HasGlobalScope());
    require::True(t, sv.IsHintUpdatableVerified);
    require::Equal(t, vardef::TypeEnum, sv.Type);
    require::Equal(t, "DISABLE", sv.Value) // Default is DISABLE;

    // Test validation
    let mut vars = variable::NewSessionVars(None);
    vars.GlobalVarsAccessor = variable::NewMockGlobalAccessor4Tests();

    // Test allowed values: DISABLE, COST (case-insensitive)
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "COST", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "COST", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "cost", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "COST", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "DISABLE", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "DISABLE", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "disable", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "DISABLE", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "Cost", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "COST", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "Disable", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "DISABLE", val);

    // Test disallowed values (old ON/OFF values and others)
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "ON", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "OFF", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "1", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "0", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "true", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "false", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "2", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "-1", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "yes", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "no", vardef::ScopeSession);
    require::Error(t, err);
    require::Contains(t, err.Error(), "can't be set to the value of");

    // Test SetSession function
    err = sv.SetSessionFromHook(vars, "COST");
    require::NoError(t, err);
    require::True(t, vars.IsPartialOrderedIndexForTopNEnabled());

    err = sv.SetSessionFromHook(vars, "DISABLE");
    require::NoError(t, err);
    require::False(t, vars.IsPartialOrderedIndexForTopNEnabled());
}

// TestPerformanceSchemaSessionConnectAttrsSizeGlobalSQL 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// performance_schema 连接属性大小全局 SQL。
#[test]
pub fn TestPerformanceSchemaSessionConnectAttrsSizeGlobalSQL(t: &testing::T) {
    let mut store = testkit::CreateMockStore(t);
    let mut tk = testkit::NewTestKit(t, store);

    let mut originSize = vardef::ConnectAttrsSize.Load();
    // Go defer 用于恢复全局配置或释放资源；Rust 接线时需要改成 guard/drop 语义。
    defer || {
        vardef::ConnectAttrsSize.Store(originSize);
        // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
        tk.MustExec("set global performance_schema_session_connect_attrs_size = " + strconv::FormatInt(originSize, 10));
    }()

    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set global performance_schema_session_connect_attrs_size = 0");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.performance_schema_session_connect_attrs_size").Check(testkit.Rows("0"));
    require::Equal(t, int64(0), vardef::ConnectAttrsSize.Load());

    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set global performance_schema_session_connect_attrs_size = 65536");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.performance_schema_session_connect_attrs_size").Check(testkit.Rows("65536"));
    require::Equal(t, int64(65536), vardef::ConnectAttrsSize.Load());

    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set global performance_schema_session_connect_attrs_size = -1");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.performance_schema_session_connect_attrs_size").Check(testkit.Rows("-1"));
    require::Equal(t, int64(-1), vardef::ConnectAttrsSize.Load());

    // Out-of-range values are normalized by int sysvar min/max.
    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set global performance_schema_session_connect_attrs_size = 70000");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.performance_schema_session_connect_attrs_size").Check(testkit.Rows("65536"));
    require::Equal(t, int64(65536), vardef::ConnectAttrsSize.Load());

    // Go MustExec 会立即执行 SQL 并失败即终止测试；这里保留 SQL 语句和调用位置作为迁移线索。
    tk.MustExec("set global performance_schema_session_connect_attrs_size = -2");
    // Go MustQuery 会执行 SQL 并比较结果；保留查询文本与期望结果的对应关系。
    tk.MustQuery("select @@global.performance_schema_session_connect_attrs_size").Check(testkit.Rows("-1"));
    require::Equal(t, int64(-1), vardef::ConnectAttrsSize.Load());
}

// TestSetTiDBCloudStorageURI 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
/// 云存储 URI 系统变量设置与校验。
#[test]
pub fn TestSetTiDBCloudStorageURI(t: &testing::T) {
    let mut vars = variable::NewSessionVars(None);
    let mut mock = variable::NewMockGlobalAccessor4Tests();
    mock.SessionVars = vars;
    vars.GlobalVarsAccessor = mock;
    // Prevent AWS SDK IMDS probing from creating background HTTP goroutines.
    t.Setenv("AWS_EC2_METADATA_DISABLED", "true");
    let mut cloudStorageURI = variable::GetSysVar(vardef::TiDBCloudStorageURI);
    require::Len(t, vardef::CloudStorageURI.Load(), 0);
    // Go defer 用于恢复全局配置或释放资源；Rust 接线时需要改成 guard/drop 语义。
    defer || {
        vardef::CloudStorageURI.Store("");
    }()

    // Default empty
    require::Len(t, cloudStorageURI.Value, 0);
    let mut s = httptest.NewServer(http::HandlerFunc(|w: http::ResponseWriter, r: &http::Request| {
        w.WriteHeader(200);
    }))
    t.Cleanup(s.Close);
    let mut ctx, cancel = context.WithCancel(context::Background());
    t.Cleanup(cancel);
    // Set to noop
    let mut noopURI = "noop://blackhole?access-key=hello&secret-access-key=world";
    let mut err = mock.SetGlobalSysVar(ctx, vardef::TiDBCloudStorageURI, noopURI);
    require::NoError(t, err);
    let mut val, err1 = mock.SessionVars.GetSessionOrGlobalSystemVar(ctx, vardef::TiDBCloudStorageURI);
    require::NoError(t, err1);
    require::Equal(t, noopURI, val);
    require::Equal(t, noopURI, vardef::CloudStorageURI.Load());

    // Set to s3, should fail
    err = mock.SetGlobalSysVar(ctx, vardef::TiDBCloudStorageURI, "s3://blackhole");
    require::Error(t, err, "unreachable storage URI");

    // Set to s3, should return uri without variable
    let mut s3URI = "s3://tiflow-test/?access-key=testid&secret-access-key=testkey8&session-token=testtoken&endpoint=" + s.URL;
    err = mock.SetGlobalSysVar(ctx, vardef::TiDBCloudStorageURI, s3URI);
    require::NoError(t, err);
    val, err1 = mock.SessionVars.GetSessionOrGlobalSystemVar(ctx, vardef::TiDBCloudStorageURI);
    require::NoError(t, err1);
    require::True(t, strings::HasPrefix(val, "s3://tiflow-test/"));
    require::Contains(t, val, "access-key=xxxxxx");
    require::Contains(t, val, "secret-access-key=xxxxxx");
    require::Contains(t, val, "session-token=xxxxxx");
    require::Equal(t, s3URI, vardef::CloudStorageURI.Load());

    // ks3 is like s3
    let mut ks3URI = "ks3://tiflow-test/?region=test&access-key=testid&secret-access-key=testkey8&session-token=testtoken&endpoint=" + s.URL;
    err = mock.SetGlobalSysVar(ctx, vardef::TiDBCloudStorageURI, ks3URI);
    require::NoError(t, err);
    val, err1 = mock.SessionVars.GetSessionOrGlobalSystemVar(ctx, vardef::TiDBCloudStorageURI);
    require::NoError(t, err1);
    require::True(t, strings::HasPrefix(val, "ks3://tiflow-test/"));
    require::Contains(t, val, "access-key=xxxxxx");
    require::Contains(t, val, "secret-access-key=xxxxxx");
    require::Contains(t, val, "session-token=xxxxxx");
    require::Equal(t, ks3URI, vardef::CloudStorageURI.Load());

    // Set to empty, should return no error
    err = mock.SetGlobalSysVar(ctx, vardef::TiDBCloudStorageURI, "");
    require::NoError(t, err);
    val, err1 = mock.SessionVars.GetSessionOrGlobalSystemVar(ctx, vardef::TiDBCloudStorageURI);
    require::NoError(t, err1);
    require::Len(t, val, 0);
    cancel();
    <-ctx.Done();
}
"################;

use astersql_sessionctx_vardef as vardef;
use astersql_sessionctx_variable::session::{
    PlanCacheParamList, RuntimeFilterType, TableDelta, ToRuntimeFilterType, UserVars,
};
use astersql_sessionctx_variable::{GlobalVarAccessor, VariableError};
use std::time::{Duration, Instant};

struct RegistryAccessor;

impl GlobalVarAccessor for RegistryAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        astersql_sessionctx_variable::GetSysVar(name)
            .map(|sys_var| sys_var.Value.clone())
            .ok_or_else(|| VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &astersql_sessionctx_variable::Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::new(
            astersql_sessionctx_variable::VariableErrorKind::InvalidValue,
            format!("test accessor has no mysql.tidb value for {name}"),
        ))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        Ok(())
    }
}

/// 用户变量克隆与 unset 互不影响。
#[test]
fn user_variables_clone_and_unset_are_independent() {
    let vars = UserVars::new();
    vars.SetUserVarVal("a", "1".to_owned());
    vars.SetUserVarVal("b", "v2".to_owned());
    let mut integer_type = astersql_parser::ast::ast::FieldType::default();
    integer_type.SetType(astersql_parser_mysql::r#type::TypeLonglong);
    vars.SetUserVarType("a", integer_type.clone());

    let cloned = vars.CloneVars();
    cloned.SetUserVarVal("a", "2".to_owned());
    assert_eq!(vars.GetUserVarVal("a").as_deref(), Some("1"));
    assert_eq!(cloned.GetUserVarVal("a").as_deref(), Some("2"));
    assert_eq!(vars.GetUserVarType("a"), Some(integer_type.clone()));

    vars.UnsetUserVar("a");
    assert_eq!(vars.GetUserVarVal("a"), None);
    assert_eq!(vars.GetUserVarType("a"), None);
    assert_eq!(vars.GetUserVarVal("b").as_deref(), Some("v2"));

    assert_eq!(cloned.GetUserVarType("a"), Some(integer_type));
}

/// `TableDelta::Clone` copies all Go fields and does not alias later updates.
#[test]
fn go_test_table_delta_clone_copies_value_and_timestamp() {
    let mut original = TableDelta {
        Delta: 1,
        Count: 2,
        InitTime: Some(Instant::now()),
    };
    let first_clone = original.clone();
    assert_eq!(first_clone.Delta, original.Delta);
    assert_eq!(first_clone.Count, original.Count);
    assert_eq!(first_clone.InitTime, original.InitTime);

    let second_clone = original.clone();
    original.InitTime = original.InitTime.map(|time| time + Duration::from_secs(1));
    assert_ne!(original.InitTime, second_clone.InitTime);
}

/// 运行时 filter 保序、去重并拒绝未知名。
#[test]
fn runtime_filters_preserve_order_deduplicate_and_reject_unknown_names() {
    let (filters, valid) = ToRuntimeFilterType("in,min_max,IN");
    assert!(valid);
    assert_eq!(
        filters,
        vec![RuntimeFilterType::In, RuntimeFilterType::MinMax]
    );
    assert_eq!(RuntimeFilterType::MinMax.String(), "MIN_MAX");
    assert_eq!(ToRuntimeFilterType("in,bloom"), (Vec::new(), false));
}

/// 计划缓存参数重置与 Go 会话状态一致。
#[test]
fn plan_cache_parameters_reset_like_the_go_session_state() {
    let mut params = PlanCacheParamList::new();
    params.Append(&["1".to_owned(), "abc".to_owned()]);
    params.SetForNonPrepCache(true);
    assert_eq!(params.GetParamValue(1).map(String::as_str), Some("abc"));
    params.Reset();
    assert!(params.AllParamValues().is_empty());
}

// Executable Rust coverage for the remaining small, state-only Go scenarios.
// SQL/store-heavy slow-log cases remain in GO_REFERENCE and are exercised by
// the lower-level variable and stmtctx crates until the integration testkit
// exposes the same session hook surface.
#[test]
fn go_test_set_system_variable_success_and_error_matrix() {
    astersql_sessionctx_variable::register_builtin_sysvars();
    let mut vars = astersql_sessionctx_variable::SessionVars::new(Box::new(RegistryAccessor));
    let cases = [
        (vardef::TxnIsolation, "SERIALIZABLE", true),
        (vardef::TimeZone, "xyz", true),
        (vardef::TiDBOptAggPushDown, "1", false),
        (vardef::TiDBOptDeriveTopN, "1", false),
        (vardef::TiDBOptDistinctAggPushDown, "1", false),
        (vardef::TiDBMemQuotaQuery, "1024", false),
        (vardef::TiDBMemQuotaApplyCache, "1024", false),
        (vardef::TiDBEnableStmtSummary, "1", true),
        (vardef::TiDBEnableRowLevelChecksum, "1", true),
    ];
    for (name, value, should_fail) in cases {
        let result = vars.SetSystemVar(name, value);
        assert_eq!(result.is_err(), should_fail, "SET {name}={value}");
    }
}

#[test]
fn go_test_session_statement_counters_and_retry_reset() {
    let mut ctx = astersql_util_mock::NewContext();
    let vars = ctx.GetSessionVarsMut();
    vars.Inner.SetLastInsertID(1);
    assert_eq!(vars.Inner.StmtCtx.LastInsertID, 1);
    {
        let statement = &mut vars.Inner.StmtCtx;
        statement.AddAffectedRows(1);
        assert_eq!(statement.AffectedRows(), 1);
        statement.AddAffectedRows(1);
        assert_eq!(statement.AffectedRows(), 2);

        statement.AddRecordRows(1);
        assert_eq!(statement.RecordRows(), 1);
        statement.AddRecordRows(1);
        assert_eq!(statement.RecordRows(), 2);

        statement.AddFoundRows(1);
        assert_eq!(statement.FoundRows(), 1);
        statement.AddFoundRows(1);
        assert_eq!(statement.FoundRows(), 2);

        statement.AddUpdatedRows(1);
        assert_eq!(statement.UpdatedRows(), 1);
        statement.AddUpdatedRows(1);
        assert_eq!(statement.UpdatedRows(), 2);

        statement.AddTouchedRows(1);
        assert_eq!(statement.TouchedRows(), 1);
        statement.AddTouchedRows(1);
        assert_eq!(statement.TouchedRows(), 2);

        statement.AddCopiedRows(1);
        assert_eq!(statement.CopiedRows(), 1);
        statement.AddCopiedRows(1);
        assert_eq!(statement.CopiedRows(), 2);
        statement.ResetForRetry();
        assert_eq!(statement.AffectedRows(), 0);
        assert_eq!(statement.RecordRows(), 0);
        assert_eq!(statement.FoundRows(), 0);
        assert_eq!(statement.UpdatedRows(), 0);
        assert_eq!(statement.TouchedRows(), 0);
        assert_eq!(statement.CopiedRows(), 0);
        assert_eq!(statement.WarningCount(), 0);
    }
}

#[test]
fn go_test_transaction_context_delta_savepoint_and_cleanup() {
    use astersql_sessionctx_variable::session::{SavepointRecord, TransactionContext};

    let mut txn = TransactionContext::default();
    txn.UpdateDeltaForTable(42, 10, 2);
    txn.UpdateDeltaForTable(42, -3, 1);
    assert_eq!(txn.needToRestore.TableDeltaMap[&42].Delta, 7);
    assert_eq!(txn.needToRestore.TableDeltaMap[&42].Count, 3);
    txn.SetForUpdateTS(10);
    txn.SetForUpdateTS(8);
    txn.SetStartTS(12);
    assert_eq!(txn.GetForUpdateTS(), 12);
    txn.noNeedToRestore.Savepoints.push(SavepointRecord {
        Name: "sp1".to_owned(),
        ..Default::default()
    });
    txn.Cleanup();
    assert!(txn.needToRestore.TableDeltaMap.is_empty());
    assert!(txn.noNeedToRestore.Savepoints.is_empty());
}

#[test]
fn go_test_isolation_read_and_status_state_are_session_scoped() {
    let mut vars = astersql_sessionctx_variable::session::SessionVars::new();
    assert_eq!(
        vars.GetIsolationReadEngines(),
        [
            astersql_kv::StoreType::TiKV,
            astersql_kv::StoreType::TiFlash,
            astersql_kv::StoreType::TiDB,
        ]
        .into_iter()
        .collect()
    );
    vars.SetSystemVar(vardef::TiDBIsolationReadEngines, "tikv,tidb")
        .expect("valid isolation engines");
    assert_eq!(
        vars.GetIsolationReadEngines(),
        [astersql_kv::StoreType::TiKV, astersql_kv::StoreType::TiDB]
            .into_iter()
            .collect()
    );

    assert!(vars.IsAutocommit());
    assert!(!vars.InTxn());
    vars.SetInTxn(true);
    assert!(vars.InTxn());
    vars.SetInTxn(false);
    assert!(!vars.InTxn());
}

#[test]
fn go_test_partial_ordered_index_sysvar_contract() {
    astersql_sessionctx_variable::register_builtin_sysvars();
    let sys_var =
        astersql_sessionctx_variable::GetSysVar(vardef::TiDBOptPartialOrderedIndexForTopN)
            .expect("partial ordered index sysvar must be registered");
    assert!(sys_var.HasSessionScope());
    assert!(sys_var.HasGlobalScope());
    assert!(sys_var.IsHintUpdatableVerified);
    assert_eq!(sys_var.Type, vardef::TypeEnum);
    assert_eq!(sys_var.Value, "DISABLE");

    let mut vars = astersql_sessionctx_variable::SessionVars::new(Box::new(RegistryAccessor));
    for (input, expected) in [
        ("COST", "COST"),
        ("cost", "COST"),
        ("DISABLE", "DISABLE"),
        ("disable", "DISABLE"),
        ("Cost", "COST"),
        ("Disable", "DISABLE"),
    ] {
        assert_eq!(
            sys_var
                .Validate(&mut vars, input, vardef::ScopeSession)
                .expect("allowed enum value"),
            expected
        );
    }

    for input in [
        "ON", "OFF", "1", "0", "true", "false", "2", "-1", "yes", "no",
    ] {
        let error = sys_var
            .Validate(&mut vars, input, vardef::ScopeSession)
            .expect_err("legacy and unknown values must be rejected");
        assert!(error.to_string().contains("can't be set to the value of"));
    }

    sys_var
        .SetSessionFromHook(&mut vars, "COST")
        .expect("COST session hook");
    assert_eq!(vars.OptPartialOrderedIndexForTopN, "COST");
    sys_var
        .SetSessionFromHook(&mut vars, "DISABLE")
        .expect("DISABLE session hook");
    assert_eq!(vars.OptPartialOrderedIndexForTopN, "DISABLE");
}

#[test]
fn go_test_user_vars_and_relevant_optimizer_state() {
    let vars = astersql_sessionctx_variable::session::SessionVars::new();
    vars.UserVars.SetUserVarVal("x", "1".to_owned());
    // Go session.go gates both recording methods on the initially false flag.
    vars.RecordRelevantOptVar("ignored");
    vars.RecordRelevantOptFix(99);
    assert_eq!(vars.RelevantOptVarsAndFixes(), (vec![], vec![]));
    vars.ResetRelevantOptVarsAndFixes(true);
    vars.RecordRelevantOptVar("tidb_opt_range_max_size");
    vars.RecordRelevantOptFix(7);
    vars.RecordRelevantOptVar("tidb_opt_range_max_size");
    vars.RecordRelevantOptFix(7);
    assert_eq!(vars.UserVars.GetUserVarVal("x").as_deref(), Some("1"));
    assert_eq!(
        vars.RelevantOptVarsAndFixes(),
        (vec!["tidb_opt_range_max_size".to_owned()], vec![7])
    );
    vars.ResetRelevantOptVarsAndFixes(false);
    vars.RecordRelevantOptVar("ignored_after_reset");
    vars.RecordRelevantOptFix(100);
    assert_eq!(vars.RelevantOptVarsAndFixes(), (vec![], vec![]));
    assert_eq!(vars.UserVars.GetUserVarVal("x").as_deref(), Some("1"));
}

#[test]
fn go_test_plan_cache_and_prepared_statement_state() {
    let mut vars = astersql_sessionctx_variable::session::SessionVars::new();
    assert_eq!(vars.GetNextPreparedStmtID(), 1);
    vars.SetNextPreparedStmtID(10);
    assert_eq!(vars.GetNextPreparedStmtID(), 11);
    vars.PlanCacheParams
        .Append(&["one".to_owned(), "two".to_owned()]);
    vars.PlanCacheValue = Some("cached".to_owned());
    assert_eq!(
        vars.PlanCacheParams.GetParamValue(1).map(String::as_str),
        Some("two")
    );
    assert_eq!(vars.PlanCacheValue.as_deref(), Some("cached"));
    vars.PlanCacheParams.Reset();
    assert!(vars.PlanCacheParams.AllParamValues().is_empty());
}
