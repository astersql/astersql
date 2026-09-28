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

// JobSubmitter 测试：含 Go 侧全局 ID 分配/重试/压测等注释骨架，以及可执行的 Rust
// 单测——验证 CREATE TABLE 合并提交与重复 Job ID 拒绝。

/*

/// getGlobalID 对应 Go 辅助函数：在新事务里读取 meta global ID。
pub fn getGlobalID(ctx: context::Context, t: &testing::T, store: kv::Storage) -> i64 {
    let mut res = 0i64;
    require::NoError(t, kv::RunInNewTxn(ctx, store, true, |_: context::Context, txn: kv::Transaction| {
        let m = meta::NewMutator(txn);
        let (id, err) = m.GetGlobalID();
        require::NoError(t, err);
        res = id;
        Ok(())
    }));
    res
}

/// TestGenIDAndInsertJobsWithRetry 对应 Go 的并发 ID 分配测试：多个线程反复插入 job，要求 job ID 唯一且全局 ID 前进。
#[test]
pub fn TestGenIDAndInsertJobsWithRetry() {
    let t = testing::T::current();
    let store = testkit::CreateMockStore(t, mockstore::WithStoreType(mockstore::EmbedUnistore));
    let tk = testkit::NewTestKit(t, store);
    let dom = domain::GetDomain(tk.Session());
    dom.DDL().OwnerManager().CampaignCancel();
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);

    // Go 临时降低 MaxRetryCnt，避免外层 retry 掩盖 submitter 自身重试路径。
    let bak = kv::MaxRetryCnt;
    kv::MaxRetryCnt = 1;
    t.Cleanup(|| kv::MaxRetryCnt = bak);

    let jobs = vec![ddl::JobWrapper {
        Job: model::Job {
            Version: model::GetJobVerInUse(),
            Type: model::ActionCreateTable,
            SchemaName: "test".to_string(),
            TableName: "t1".to_string(),
        },
        JobArgs: model::CreateTableArgs { TableInfo: model::TableInfo::default() },
    }];
    let initialGID = getGlobalID(ctx, t, store);
    let (threads, iterations) = (10usize, 500usize);
    let mut tks = Vec::with_capacity(threads);
    for _ in 0..threads {
        tks.push(testkit::NewTestKit(t, store));
    }

    let mut wg = util::WaitGroupWrapper::default();
    let submitter = ddl::NewJobSubmitterForTest();
    for idx in 0..threads {
        wg.Run(|| {
            let kit = &tks[idx];
            let ddlSe = sess::NewSession(kit.Session());
            for _ in 0..iterations {
                require::NoError(t, submitter.GenGIDAndInsertJobsWithRetry(ctx, ddlSe, &jobs));
            }
        });
    }
    wg.Wait();

    let jobCount = threads * iterations;
    let (gotJobs, err) = ddl::GetAllDDLJobs(ctx, tk.Session());
    require::NoError(t, err);
    require::Len(t, gotJobs, jobCount);
    let currGID = getGlobalID(ctx, t, store);
    require::Greater(t, currGID - initialGID, jobCount as i64);
    let mut uniqueJobIDs = map::HashSet::<i64>::with_capacity(jobCount);
    for j in gotJobs {
        require::Greater(t, j.ID, initialGID);
        uniqueJobIDs.insert(j.ID);
    }
    require::Len(t, uniqueJobIDs, jobCount);
}

/// idAllocationCase 对应 Go 的测试用例结构，记录 job wrapper 和预期消耗的 global ID 个数。
pub struct idAllocationCase {
    pub jobW: ddl::JobWrapper,
    pub requiredIDCount: i32,
}

/// TestCombinedIDAllocation 对应 Go 的批量 ID 分配矩阵测试。
/// 每个闭包构造一种 DDL job 参数，cases 中的 requiredIDCount 是后续断言的核心数据。
#[test]
pub fn TestCombinedIDAllocation() {
    let t = testing::T::current();
    let store = testkit::CreateMockStore(t, mockstore::WithStoreType(mockstore::EmbedUnistore));
    let tk = testkit::NewTestKit(t, store);
    let dom = domain::GetDomain(tk.Session());
    dom.DDL().OwnerManager().CampaignCancel();
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);

    let bak = kv::MaxRetryCnt;
    kv::MaxRetryCnt = 1;
    t.Cleanup(|| kv::MaxRetryCnt = bak);

    let genTblInfo = |partitionCnt: i32| -> model::TableInfo {
        let mut info = model::TableInfo { Partition: Some(model::PartitionInfo::default()), ..Default::default() };
        for _ in 0..partitionCnt {
            info.Partition.Enable = true;
            info.Partition.Definitions.push(model::PartitionDefinition::default());
        }
        info
    };

    let genCreateTblJobW = |tp: model::ActionType, partitionCnt: i32, idAllocated: bool| {
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: tp, ..Default::default() },
            model::CreateTableArgs { TableInfo: genTblInfo(partitionCnt) },
            idAllocated,
        )
    };

    let genCreateTblsJobW = |idAllocated: bool, partitionCounts: Vec<i32>| {
        let mut args = model::BatchCreateTableArgs { Tables: Vec::with_capacity(partitionCounts.len()) };
        for c in partitionCounts {
            args.Tables.push(model::CreateTableArgs { TableInfo: genTblInfo(c) });
        }
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::JobVersion1, Type: model::ActionCreateTables, ..Default::default() },
            args,
            idAllocated,
        )
    };

    let genCreateDBJob = |idAllocated: bool| {
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: model::ActionCreateSchema, ..Default::default() },
            model::CreateSchemaArgs { DBInfo: model::DBInfo::default() },
            idAllocated,
        )
    };

    let genRGroupJob = |idAllocated: bool| {
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: model::ActionCreateResourceGroup, ..Default::default() },
            model::ResourceGroupArgs { RGInfo: model::ResourceGroupInfo::default() },
            idAllocated,
        )
    };

    let genAlterTblPartitioningJob = |partCnt: i32, idAllocated: bool| {
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: model::ActionAlterTablePartitioning, ..Default::default() },
            model::TablePartitionArgs { PartInfo: model::PartitionInfo { Definitions: vec![model::PartitionDefinition::default(); partCnt as usize], ..Default::default() } },
            idAllocated,
        )
    };

    let genTruncPartitionJob = |partCnt: i32, idAllocated: bool| {
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: model::ActionTruncateTablePartition, ..Default::default() },
            model::TruncateTableArgs { OldPartitionIDs: vec![0; partCnt as usize], ..Default::default() },
            idAllocated,
        )
    };

    let genAddPartitionJob = |partCnt: i32, idAllocated: bool| {
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: model::ActionAddTablePartition, ..Default::default() },
            model::TablePartitionArgs { PartInfo: model::PartitionInfo { Definitions: vec![model::PartitionDefinition::default(); partCnt as usize], ..Default::default() } },
            idAllocated,
        )
    };

    let genReorgOrRemovePartitionJob = |remove: bool, partCnt: i32, idAllocated: bool| {
        let mut tp = model::ActionReorganizePartition;
        if remove {
            tp = model::ActionRemovePartitioning;
            require::Equal(t, 1, partCnt);
        }
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: tp, ..Default::default() },
            model::TablePartitionArgs { PartInfo: model::PartitionInfo { Definitions: vec![model::PartitionDefinition::default(); partCnt as usize], ..Default::default() } },
            idAllocated,
        )
    };

    let genTruncTblJob = |partCnt: i32, idAllocated: bool| {
        ddl::NewJobWrapperWithArgs(
            model::Job { Version: model::GetJobVerInUse(), Type: model::ActionTruncateTable, ..Default::default() },
            model::TruncateTableArgs { OldPartitionIDs: vec![0; partCnt as usize], ..Default::default() },
            idAllocated,
        )
    };

    let cases = vec![
        idAllocationCase { jobW: genCreateTblsJobW(false, vec![1, 2, 0]), requiredIDCount: 1 + 3 + 1 + 2 },
        idAllocationCase { jobW: genCreateTblsJobW(true, vec![3, 4]), requiredIDCount: 1 },
        idAllocationCase { jobW: genCreateTblJobW(model::ActionCreateTable, 3, false), requiredIDCount: 1 + 1 + 3 },
        idAllocationCase { jobW: genCreateTblJobW(model::ActionCreateTable, 0, false), requiredIDCount: 1 + 1 },
        idAllocationCase { jobW: genCreateTblJobW(model::ActionCreateTable, 8, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genCreateTblJobW(model::ActionCreateSequence, 0, false), requiredIDCount: 2 },
        idAllocationCase { jobW: genCreateTblJobW(model::ActionCreateSequence, 0, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genCreateTblJobW(model::ActionCreateView, 0, false), requiredIDCount: 2 },
        idAllocationCase { jobW: genCreateTblJobW(model::ActionCreateView, 0, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genCreateDBJob(false), requiredIDCount: 2 },
        idAllocationCase { jobW: genCreateDBJob(true), requiredIDCount: 1 },
        idAllocationCase { jobW: genRGroupJob(false), requiredIDCount: 2 },
        idAllocationCase { jobW: genRGroupJob(true), requiredIDCount: 1 },
        idAllocationCase { jobW: genAlterTblPartitioningJob(9, false), requiredIDCount: 11 },
        idAllocationCase { jobW: genAlterTblPartitioningJob(4, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genTruncPartitionJob(33, false), requiredIDCount: 34 },
        idAllocationCase { jobW: genTruncPartitionJob(2, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genAddPartitionJob(15, false), requiredIDCount: 16 },
        idAllocationCase { jobW: genAddPartitionJob(33, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genReorgOrRemovePartitionJob(false, 12, false), requiredIDCount: 13 },
        idAllocationCase { jobW: genReorgOrRemovePartitionJob(false, 12, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genReorgOrRemovePartitionJob(true, 1, false), requiredIDCount: 2 },
        idAllocationCase { jobW: genReorgOrRemovePartitionJob(true, 1, true), requiredIDCount: 1 },
        idAllocationCase { jobW: genTruncTblJob(17, false), requiredIDCount: 19 },
        idAllocationCase { jobW: genTruncTblJob(6, true), requiredIDCount: 1 },
    ];

    let submitter = ddl::NewJobSubmitterForTest();
    t.Run("process one by one", || {
        tk.MustExec("delete from mysql.tidb_ddl_job");
        for (i, c) in cases.iter().enumerate() {
            let currentGlobalID = getGlobalID(ctx, t, store);
            require::NoError(t, submitter.GenGIDAndInsertJobsWithRetry(ctx, sess::NewSession(tk.Session()), vec![&c.jobW]));
            require::Equal(t, currentGlobalID + c.requiredIDCount as i64, getGlobalID(ctx, t, store), fmt::Sprintf("case-%d", i));
        }
        let (gotJobs, err) = ddl::GetAllDDLJobs(ctx, tk.Session());
        require::NoError(t, err);
        require::Len(t, gotJobs, cases.len());
    });

    t.Run("process together", || {
        tk.MustExec("delete from mysql.tidb_ddl_job");
        let totalRequiredCnt = cases.iter().map(|c| c.requiredIDCount).sum::<i32>();
        let jobWs = cases.iter().map(|c| &c.jobW).collect::<Vec<_>>();
        let currentGlobalID = getGlobalID(ctx, t, store);
        require::NoError(t, submitter.GenGIDAndInsertJobsWithRetry(ctx, sess::NewSession(tk.Session()), jobWs));
        require::Equal(t, currentGlobalID + totalRequiredCnt as i64, getGlobalID(ctx, t, store));

        let (gotJobs, err) = ddl::GetAllDDLJobs(ctx, tk.Session());
        require::NoError(t, err);
        require::Len(t, gotJobs, cases.len());
    });

    t.Run("process IDAllocated = false", || {
        tk.MustExec("delete from mysql.tidb_ddl_job");
        let initialGlobalID = getGlobalID(ctx, t, store);
        let (mut allocIDCaseCount, mut allocatedIDCount) = (0, 0);
        for c in &cases {
            if !c.jobW.IDAllocated {
                allocIDCaseCount += 1;
                allocatedIDCount += c.requiredIDCount;
                require::NoError(t, submitter.GenGIDAndInsertJobsWithRetry(ctx, sess::NewSession(tk.Session()), vec![&c.jobW]));
            }
        }
        require::EqualValues(t, 13, allocIDCaseCount);
        let mut uniqueIDs = map::HashSet::<i64>::with_capacity(cases.len());

        // checkID/checkPartitionInfo/checkTableInfo 保留 Go 对已分配 ID 唯一性和递增性的逐层检查。
        let mut checkID = |id: i64| {
            uniqueIDs.insert(id);
            require::Greater(t, id, initialGlobalID);
        };
        let mut checkPartitionInfo = |info: model::PartitionInfo| {
            for def in info.Definitions {
                uniqueIDs.insert(def.ID);
                require::Greater(t, def.ID, initialGlobalID);
            }
        };
        let mut checkTableInfo = |info: model::TableInfo| {
            uniqueIDs.insert(info.ID);
            require::Greater(t, info.ID, initialGlobalID);
            if let Some(pInfo) = info.GetPartitionInfo() {
                checkPartitionInfo(pInfo);
            }
        };

        let (gotJobs, err) = ddl::GetAllDDLJobs(ctx, tk.Session());
        require::NoError(t, err);
        require::Len(t, gotJobs, allocIDCaseCount);
        for j in gotJobs {
            checkID(j.ID);
            match j.Type {
                model::ActionCreateTable | model::ActionCreateView | model::ActionCreateSequence => {
                    require::Greater(t, j.TableID, initialGlobalID);
                    let (args, err) = model::GetCreateTableArgs(j);
                    require::NoError(t, err);
                    require::Equal(t, j.TableID, args.TableInfo.ID);
                    checkTableInfo(args.TableInfo);
                }
                model::ActionCreateTables => {
                    let (args, err) = model::GetBatchCreateTableArgs(j);
                    require::NoError(t, err);
                    for tblArgs in args.Tables {
                        checkTableInfo(tblArgs.TableInfo);
                    }
                }
                model::ActionCreateSchema => {
                    require::Greater(t, j.SchemaID, initialGlobalID);
                    let (args, err) = model::GetCreateSchemaArgs(j);
                    require::NoError(t, err);
                    uniqueIDs.insert(args.DBInfo.ID);
                    require::Equal(t, j.SchemaID, args.DBInfo.ID);
                }
                model::ActionCreateResourceGroup => {
                    let (args, err) = model::GetResourceGroupArgs(j);
                    require::NoError(t, err);
                    checkID(args.RGInfo.ID);
                }
                model::ActionAlterTablePartitioning => {
                    let (args, err) = model::GetTablePartitionArgs(j);
                    require::NoError(t, err);
                    checkPartitionInfo(args.PartInfo);
                    checkID(args.PartInfo.NewTableID);
                }
                model::ActionAddTablePartition | model::ActionReorganizePartition => {
                    let (args, err) = model::GetTablePartitionArgs(j);
                    require::NoError(t, err);
                    checkPartitionInfo(args.PartInfo);
                }
                model::ActionRemovePartitioning => {
                    let (args, err) = model::GetTablePartitionArgs(j);
                    require::NoError(t, err);
                    checkPartitionInfo(args.PartInfo);
                    checkID(args.PartInfo.NewTableID);
                }
                model::ActionTruncateTable | model::ActionTruncateTablePartition => {
                    let (args, err) = model::GetTruncateTableArgs(j);
                    require::NoError(t, err);
                    if j.Type == model::ActionTruncateTable {
                        checkID(args.NewTableID);
                    }
                    for id in args.NewPartitionIDs {
                        checkID(id);
                    }
                }
                _ => {}
            }
        }
        require::Len(t, uniqueIDs, allocatedIDCount);
    });
}

static threadVar: flag::Int = flag::Int::new("threads", 100, "number of threads");
static iterationPerThreadVar: flag::Int = flag::Int::new("iterations", 30000, "number of iterations per thread");
static payloadSizeVar: flag::Int = flag::Int::new("payload-size", 1024, "size of payload in bytes");

/// TestGenIDAndInsertJobsWithRetryQPS 对应 Go 的离线压测；原测试在 CI 中主动 Skip。
#[test]
pub fn TestGenIDAndInsertJobsWithRetryQPS() {
    let t = testing::T::current();
    t.Skip("it's for offline test only, skip it in CI");
    let (thread, iterationPerThread, payloadSize) = (*threadVar, *iterationPerThreadVar, *payloadSizeVar);
    let store = testkit::CreateMockStore(t, mockstore::WithStoreType(mockstore::EmbedUnistore));
    let tk = testkit::NewTestKit(t, store);
    let dom = domain::GetDomain(tk.Session());
    dom.DDL().OwnerManager().CampaignCancel();
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);

    let payload = strings::Repeat("a", payloadSize);
    let jobs = vec![ddl::JobWrapper {
        Job: model::Job {
            Version: model::GetJobVerInUse(),
            Type: model::ActionCreateTable,
            SchemaName: "test".to_string(),
            TableName: "t1".to_string(),
        },
        JobArgs: model::CreateTableArgs { TableInfo: model::TableInfo { Comment: payload, ..Default::default() } },
    }];
    let counters = vec![atomic::Int64::new(0); thread + 1];
    let mut wg = util::WaitGroupWrapper::default();
    let submitter = ddl::NewJobSubmitterForTest();
    for index in 0..thread {
        wg.Run(|| {
            let kit = testkit::NewTestKit(t, store);
            let ddlSe = sess::NewSession(kit.Session());
            for _ in 0..iterationPerThread {
                require::NoError(t, submitter.GenGIDAndInsertJobsWithRetry(ctx, ddlSe, &jobs));
                counters[0].Add(1);
                counters[index + 1].Add(1);
            }
        });
    }

    // Go goroutine 每 5 秒打印一次总 QPS 和前 10 个线程 QPS；这里保留监控循环结构。
    goroutine::spawn(|| {
        let getCounts = || counters.iter().map(|c| c.Load()).collect::<Vec<i64>>();
        let mut lastCnt = getCounts();
        loop {
            time::Sleep(5 * time::Second);
            let currCnt = getCounts();
            let mut sb = strings::Builder::new();
            sb.WriteString(fmt::Sprintf("QPS - total:%.0f", (currCnt[0] - lastCnt[0]) as f64 / 5.0));
            for i in 1..std::cmp::min(counters.len(), 10) {
                sb.WriteString(fmt::Sprintf(", thread-%d: %.0f", i, (currCnt[i] - lastCnt[i]) as f64 / 5.0));
            }
            if counters.len() > 10 {
                sb.WriteString("...");
            }
            lastCnt = currCnt;
            fmt::Println(sb.String());
        }
    });
    wg.Wait();
}

/// TestGenGIDAndInsertJobsWithRetryOnErr 对应 Go 的 retry 错误路径测试：三次可重试错误后仍应清理旧 done channel。
#[test]
pub fn TestGenGIDAndInsertJobsWithRetryOnErr() {
    let t = testing::T::current();
    let store = testkit::CreateMockStore(t, mockstore::WithStoreType(mockstore::EmbedUnistore));
    let tk = testkit::NewTestKit(t, store);
    let dom = domain::GetDomain(tk.Session());
    dom.DDL().OwnerManager().CampaignCancel();
    let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnDDL);

    let ddlSe = sess::NewSession(tk.Session());
    let jobs = vec![ddl::JobWrapper {
        Job: model::Job {
            Version: model::GetJobVerInUse(),
            Type: model::ActionCreateTable,
            SchemaName: "test".to_string(),
            TableName: "t1".to_string(),
        },
        JobArgs: model::CreateTableArgs { TableInfo: model::TableInfo::default() },
    }];
    let submitter = ddl::NewJobSubmitterForTest();
    let currGID = getGlobalID(ctx, t, store);
    let mut counter = 0i64;
    testfailpoint::Enable(t, "github.com/pingcap/tidb/pkg/ddl/jobsubmit/mockGenGIDRetryableError", "3*return(true)");
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/jobsubmit/onGenGIDRetry", || {
        let m = submitter.DDLJobDoneChMap();
        // retry hook 发生在事务失败清理旧 channel 之后、下一次重试注册新 channel 之前。
        require::Empty(t, m.Keys());
        counter += 1;
        require::NoError(t, kv::RunInNewTxn(ctx, store, true, |_: context::Context, txn: kv::Transaction| {
            let m = meta::NewMutator(txn);
            let (_, err) = m.GenGlobalIDs(100);
            require::NoError(t, err);
            Ok(())
        }));
    });
    require::Zero(t, submitter.DDLJobDoneChMap().Keys().len());
    require::NoError(t, submitter.GenGIDAndInsertJobsWithRetry(ctx, ddlSe, &jobs));
    require::EqualValues(t, 3, counter);
    let newGID = getGlobalID(ctx, t, store);
    require::Equal(t, currGID + 300 + 2, newGID);
    let m = submitter.DDLJobDoneChMap();
    require::Equal(t, 1, m.Keys().len());
    let (_, ok) = m.Load(newGID);
    require::True(t, ok);
    require::Equal(t, newGID - 1, jobs[0].TableID);
}

/// TestSubmitJobAfterDDLIsClosed 对应 Go 的关闭 DDL 后提交 job 测试：afterDDLCloseCancel 钩子中提交应返回 context canceled。
#[test]
pub fn TestSubmitJobAfterDDLIsClosed() {
    let t = testing::T::current();
    let (store, dom) = testkit::CreateMockStoreAndDomain(t, mockstore::WithStoreType(mockstore::EmbedUnistore));
    let tk = testkit::NewTestKit(t, store);

    let mut ddlErr: Option<errors::Error> = None;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterDDLCloseCancel", || {
        ddlErr = Some(tk.ExecToErr("create database test2;"));
    });
    let err = dom.DDL().Stop();
    require::NoError(t, err);
    require::Error(t, ddlErr.as_ref());
    require::Equal(t, "context canceled", ddlErr.unwrap().Error());
}
*/

use crate::ddl::{Job, JobState};
use crate::job_submitter::{
    JobSpec, JobSubmitter, build_query_string_from_jobs, merge_create_table_jobs,
};

/// 同 schema 的多条 CREATE TABLE 应合并为一条 pending，且只通知一次。
#[test]
fn submitter_merges_create_table_jobs_and_notifies_once() {
    let mut submitter = JobSubmitter::default();
    let jobs = vec![
        JobSpec::new(Job::new(1, 7, 1, "create table db.t1(id int)"), false),
        JobSpec::new(Job::new(2, 7, 2, "create table db.t2(id int)"), false),
    ];
    assert_eq!(vec![Ok(1)], submitter.submit(jobs));
    let pending = submitter.take_pending();
    assert_eq!(1, pending.len());
    assert_eq!(2, pending[0].merged_jobs.len());
    assert_eq!(1, submitter.notification_count());
}

/// 已持久化的 Job ID 再次提交应返回 already exists 错误。
#[test]
fn submitter_rejects_duplicate_persisted_job_ids() {
    let mut submitter = JobSubmitter::default();
    assert_eq!(
        vec![Ok(1)],
        submitter.submit(vec![JobSpec::new(
            Job::new(1, 1, 1, "alter table t add c int"),
            false
        )])
    );
    let result = submitter.submit(vec![JobSpec::new(Job::new(1, 1, 1, "drop table t"), false)]);
    assert!(result[0].as_ref().unwrap_err().contains("already exists"));
    assert_eq!(1, submitter.notification_count());
}

/// Go 只在 query 尚无尾分号时补一个分号，已有的多个尾分号必须原样保留。
#[test]
fn query_builder_preserves_existing_trailing_semicolons() {
    let jobs = [
        JobSpec::new(Job::new(1, 1, 1, " create table t1(a int);; "), false),
        JobSpec::new(Job::new(2, 1, 2, "create table t2(a int)"), false),
    ];
    assert_eq!(
        "create table t1(a int);; create table t2(a int);",
        build_query_string_from_jobs(&jobs)
    );
}

/// Go 的合并资格只取决于动作类型、ID 是否已分配及外键，不检查 JobState。
#[test]
fn merge_eligibility_does_not_depend_on_job_state() {
    let first = JobSpec::new(Job::new(1, 1, 1, "create table t1(a int)"), false);
    let mut second = JobSpec::new(Job::new(2, 1, 2, "create table t2(a int)"), false);
    second.job.state = JobState::Running;

    let merged = merge_create_table_jobs(vec![first, second]);
    assert_eq!(1, merged.len());
    assert_eq!(2, merged[0].merged_jobs.len());
}
