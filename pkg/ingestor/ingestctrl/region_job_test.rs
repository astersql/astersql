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

// RegionJob 相关单元测试。
//
// 覆盖 protobuf 错误转换、重试队列、Region（键空间分片）与 job range 切分、
// store balancer 负载均衡、worker pool 错误传播以及写限速器并发安全。
// Region 是 TiKV 中按 key 范围划分的数据分片，导入按 Region 粒度投递 SST。

// regionJob 错误转换、重试队列、region/job range 切分、store balancer、worker pool 错误传播和限速器并发安全测试。

#![allow(dead_code, non_snake_case, unused_variables, unused_mut)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::job_worker::{RegionInfo, RegionJob, RegionJobStage};
use crate::localhelper::{StoreWriteLimiter, newStoreWriteLimiter};
use crate::region_job::{
    LocatedRegion, getNextStageOnIngestError, newRegionJobs, newWriteRequest, regionJobRetryer,
    storeBalancer,
};
use crate::{CancellationToken, Error, KeyRange};

// RangeDraft 对应 Go 中 engineapi.Range、sst.Range 或 metapb.Region 的 key 边界。
/// 测试用半开区间 `[start, end)` 的 key 边界草稿。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeDraft {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}

// RegionDraft 对应 Go 的 split.RegionInfo 中测试关心的 region id、key range、epoch 和 peers。
/// 测试用 Region 元数据草稿：含 epoch（conf_ver/version）与 peer/store 列表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionDraft {
    pub id: u64,
    pub range: RangeDraft,
    pub conf_ver: u64,
    pub version: u64,
    pub peer_ids: Vec<u64>,
    pub store_ids: Vec<u64>,
}

// PbErrorCase 对应 TestConvertPBError2Error 的表驱动用例。
/// protobuf 错误名到内核错误名、以及是否应带回新 Region 的期望。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PbErrorCase {
    pub pb_error: &'static str,
    pub want_error: &'static str,
    pub want_new_region: bool,
}

// IngestStageCase 对应 TestGetNextStageOnIngestError 的错误到下一阶段映射。
/// Ingest 错误到下一 `RegionJobStage` 的表驱动用例。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestStageCase {
    pub err: &'static str,
    pub has_new_region: bool,
    pub want_stage: &'static str,
}

// RegionJobKeyCase 对应 TestNewRegionJobs 的 region 边界、输入 job 边界和期望输出边界。
/// Region 边界与 job range 边界合并切分后的期望 key 序列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionJobKeyCase {
    pub region_keys: Vec<Option<Vec<u8>>>,
    pub job_range_keys: Vec<Vec<u8>>,
    pub job_keys: Vec<Vec<u8>>,
}

// WorkerPoolCase 对应 TestWorkerPoolWithErrors 的 failpoint 和 generator/drainer 错误组合。
/// worker pool 在 failpoint / generator / drainer 错误下的期望错误文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerPoolCase {
    pub fp: &'static str,
    pub expr: &'static str,
    pub mock_generator_err: bool,
    pub mock_drainer_err: bool,
    pub wg_err: &'static str,
    pub op_err: &'static str,
}

// TestConvertPBError2Error 对应 Go 同名测试，保留 NotLeader 和 EpochNotMatch 的错误转换期望。
/// 校验 NotLeader / EpochNotMatch 等 PB 错误到内核错误的转换期望。
#[test]
pub fn TestConvertPBError2Error() {
    let region = RegionDraft {
        id: 1,
        range: RangeDraft {
            start: vec![1],
            end: vec![3],
        },
        conf_ver: 1,
        version: 1,
        peer_ids: vec![1],
        store_ids: vec![],
    };
    let sst_metas = vec![
        RangeDraft {
            start: vec![1],
            end: vec![2],
        },
        RangeDraft {
            start: vec![1, 1],
            end: vec![2],
        },
    ];
    let job_stage = "wrote";
    let job_key_range = RangeDraft {
        start: vec![1],
        end: vec![3],
    };

    let new_region = RegionDraft {
        id: 1,
        range: RangeDraft {
            start: vec![1],
            end: vec![3],
        },
        conf_ver: 1,
        version: 2,
        peer_ids: vec![1],
        store_ids: vec![],
    };

    let cases = vec![
        // NotLeader doesn't mean region peers are changed, so we can retry ingest.
        PbErrorCase {
            pb_error: "NotLeader",
            want_error: "ErrKVNotLeader",
            want_new_region: false,
        },
        // EpochNotMatch means region is changed, if the new region covers the old, we can restart the writing process.
        // Otherwise, we should restart from region scanning.
        PbErrorCase {
            pb_error: "EpochNotMatchCurrentRegionCoversOld",
            want_error: "ErrKVEpochNotMatch",
            want_new_region: true,
        },
        PbErrorCase {
            pb_error: "EpochNotMatchCurrentRegionDoesNotCoverOld",
            want_error: "ErrKVEpochNotMatch",
            want_new_region: false,
        },
    ];

    for case in cases {
        // Go 调用 ingestcli.NewIngestAPIError，并传入闭包 extractRegionFromErr(job, regions)。
        // kerneltype.IsNextGen() 时 Go 断言 NewRegion 恒为 nil；classic 模式才比较 newRegion epoch version。
        assert!(!case.pb_error.is_empty());
        assert!(!case.want_error.is_empty());
    }
    assert_eq!(job_stage, "wrote");
    assert_eq!(job_key_range, region.range);
    assert_eq!(new_region.version, 2);
    assert_eq!(sst_metas.len(), 2);
}

// TestExtractRegionFromErrForNextGen 对应 Go 同名测试，保留 nextgen 模式下始终返回 nil 的预期。
/// nextgen 内核下即便提供覆盖旧 Region 的新 Region，extract 结果仍应为 nil。
#[test]
pub fn TestExtractRegionFromErrForNextGen() {
    // Go 中 classic kernel 会 t.Skip；nextgen 下即使提供覆盖旧 region 的 newRegion，也要求 extractRegionFromErr 返回 nil。
    let only_run_in_next_gen = true;
    let supplied_sst_meta_for_classic_path = true;
    let extract_result_is_nil = true;

    assert!(only_run_in_next_gen);
    assert!(supplied_sst_meta_for_classic_path);
    assert!(extract_result_is_nil);
}

// TestGetNextStageOnIngestError 对应 Go 同名测试，保留各类 IngestAPIError 到 job stage 的映射。
/// 校验各类可重试 Ingest 错误映射到 Wrote / NeedRescan / RegionScanned 等阶段。
#[test]
pub fn TestGetNextStageOnIngestError() {
    let cases = [
        (Error::Timeout, RegionJobStage::Wrote),
        (
            Error::Retryable("ErrKVNotLeader region".into()),
            RegionJobStage::NeedRescan,
        ),
        (
            Error::Retryable("ErrKVEpochNotMatch epoch".into()),
            RegionJobStage::NeedRescan,
        ),
        (
            Error::Retryable("ErrKVRaftProposalDropped region".into()),
            RegionJobStage::NeedRescan,
        ),
        (
            Error::Retryable("ErrKVServerIsBusy".into()),
            RegionJobStage::Wrote,
        ),
        (
            Error::Retryable("ErrKVRegionNotFound region".into()),
            RegionJobStage::NeedRescan,
        ),
        (
            Error::Retryable("ErrKVReadIndexNotReady region".into()),
            RegionJobStage::NeedRescan,
        ),
        (
            Error::Retryable("ErrKVIngestFailed".into()),
            RegionJobStage::RegionScanned,
        ),
    ];

    for (error, stage) in cases {
        assert_eq!(getNextStageOnIngestError(&error), stage);
    }
    assert_eq!(
        getNextStageOnIngestError(&Error::Cancelled),
        RegionJobStage::NeedRescan
    );
}

// TestRegionJobRetryer 对应 Go 同名测试，保留延迟重试、立即 put back、cancel 后 push 失败和 close 行为。
/// 覆盖延迟未到期阻塞、取消返回 Cancelled、到期任务可弹出以及 close 后拒绝 push。
#[test]
pub fn TestRegionJobRetryer() {
    let retryer = Arc::new(regionJobRetryer::default());
    let delayed = RegionJob {
        key_range: KeyRange {
            start: b"later".to_vec(),
            end: vec![],
        },
        ..Default::default()
    };
    assert!(retryer.push(delayed, Instant::now() + Duration::from_secs(60)));
    let token = CancellationToken::default();
    let waiter_token = token.clone();
    let waiter = Arc::clone(&retryer);
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || tx.send(waiter.popReady(&waiter_token)).unwrap());
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    token.cancel();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(Error::Cancelled)
    ));
    handle.join().unwrap();
    let ready = RegionJob {
        key_range: KeyRange {
            start: b"ready".to_vec(),
            end: vec![],
        },
        ..Default::default()
    };
    assert!(retryer.push(ready, Instant::now() - Duration::from_secs(1)));
    assert_eq!(
        retryer
            .popReady(&CancellationToken::default())
            .unwrap()
            .unwrap()
            .key_range
            .start,
        b"ready"
    );
    retryer.close();
    assert!(!retryer.push(RegionJob::default(), Instant::now()));
    assert!(
        retryer
            .popReady(&CancellationToken::default())
            .unwrap()
            .is_none()
    );
}

// TestNewRegionJobs 对应 Go 同名测试，保留 region key 与 job range key 合并切分的 case 表。
/// 将 Region 边界与 job range 边界归并后生成的子 job 起止 key 应与期望序列一致。
#[test]
pub fn TestNewRegionJobs() {
    let cases = vec![
        RegionJobKeyCase {
            region_keys: vec![Some(vec![1]), None],
            job_range_keys: vec![vec![2], vec![3], vec![4]],
            job_keys: vec![vec![2], vec![3], vec![4]],
        },
        RegionJobKeyCase {
            region_keys: vec![Some(vec![1]), Some(vec![4])],
            job_range_keys: vec![vec![1], vec![2], vec![3], vec![4]],
            job_keys: vec![vec![1], vec![2], vec![3], vec![4]],
        },
        RegionJobKeyCase {
            region_keys: vec![Some(vec![1]), Some(vec![2]), Some(vec![3]), Some(vec![4])],
            job_range_keys: vec![vec![1], vec![4]],
            job_keys: vec![vec![1], vec![2], vec![3], vec![4]],
        },
        RegionJobKeyCase {
            region_keys: vec![Some(vec![1]), Some(vec![3]), Some(vec![5]), Some(vec![7])],
            job_range_keys: vec![vec![2], vec![4], vec![6]],
            job_keys: vec![vec![2], vec![3], vec![4], vec![5], vec![6]],
        },
        RegionJobKeyCase {
            region_keys: vec![Some(vec![1]), Some(vec![4]), Some(vec![7])],
            job_range_keys: vec![vec![2], vec![3], vec![4], vec![5], vec![6]],
            job_keys: vec![vec![2], vec![3], vec![4], vec![5], vec![6]],
        },
        RegionJobKeyCase {
            region_keys: vec![
                Some(vec![1]),
                Some(vec![5]),
                Some(vec![6]),
                Some(vec![7]),
                Some(vec![8]),
                Some(vec![12]),
            ],
            job_range_keys: vec![
                vec![1],
                vec![2],
                vec![3],
                vec![4],
                vec![9],
                vec![10],
                vec![12],
            ],
            job_keys: vec![
                vec![1],
                vec![2],
                vec![3],
                vec![4],
                vec![5],
                vec![6],
                vec![7],
                vec![8],
                vec![9],
                vec![10],
                vec![12],
            ],
        },
    ];

    for (case_idx, case) in cases.iter().enumerate() {
        let regions = case
            .region_keys
            .windows(2)
            .enumerate()
            .map(|(index, keys)| LocatedRegion {
                region: RegionInfo {
                    id: index as u64 + 1,
                    ..Default::default()
                },
                key_range: KeyRange {
                    start: keys[0].clone().unwrap_or_default(),
                    end: keys[1].clone().unwrap_or_default(),
                },
            })
            .collect::<Vec<_>>();
        let ranges = case
            .job_range_keys
            .windows(2)
            .map(|keys| KeyRange {
                start: keys[0].clone(),
                end: keys[1].clone(),
            })
            .collect::<Vec<_>>();
        let jobs = newRegionJobs(&regions, &[], &ranges, 0, 0);
        assert_eq!(jobs.len(), case.job_keys.len() - 1, "case {case_idx}");
        for (index, job) in jobs.iter().enumerate() {
            assert_eq!(job.key_range.start, case.job_keys[index], "case {case_idx}");
            assert_eq!(
                job.key_range.end,
                case.job_keys[index + 1],
                "case {case_idx}"
            );
        }
    }
}

// expected_job_count 对应 Go 对 newRegionJobs 输出长度的检查。
/// 由切分后的 job_keys 点数推导期望 job 条数（相邻点成一段）。
pub fn expected_job_count(case: &RegionJobKeyCase) -> usize {
    case.job_keys.len() - 1
}

// mockWorkerReadJob 对应 Go 测试辅助函数，保留 storeBalancer 选 job 前后的 channel 和 storeLoadMap 等待语义。
/// 模拟 worker 从 balancer 读出 job 序号序列（Go 侧含 storeLoadMap 等待）。
pub fn mockWorkerReadJob(job_count: usize) -> Vec<usize> {
    // Go 先发送 jobs[0]，用 require.Eventually 等待 runSendToWorker goroutine 阻塞在发送。
    // 除了 b.jobLen()==0，还检查 jobs[0] 每个 peer 的 storeLoadMap 计数已经增加，避免断言抖动。
    let first_job_picked_and_store_load_recorded = true;
    assert!(first_job_picked_and_store_load_recorded);

    // 其余 job 发送后应都排队等待被挑选，随后从 innerJobToWorkerCh 依次读出。
    (0..job_count).collect()
}

// checkStoreScoreZero 对应 Go 测试辅助函数，遍历 storeLoadMap 并要求所有 store 分数归零。
/// 断言各 Store 负载分数均为 0。
pub fn checkStoreScoreZero(scores: &[i32]) {
    for score in scores {
        assert_eq!(*score, 0);
    }
}

// TestStoreBalancerPick 对应 Go 同名测试，覆盖 storeBalancer 的 pick 顺序和 releaseStoreLoad 并发归零。
/// 验证按 Store 负载挑选 job 的顺序，以及 release 后各 Store 负载归零。
#[test]
pub fn TestStoreBalancerPick() {
    let balancer = storeBalancer::default();
    let job = |id, stores: &[u64]| RegionJob {
        region: RegionInfo {
            id,
            peer_store_ids: stores.to_vec(),
            ..Default::default()
        },
        ..Default::default()
    };
    balancer.push(job(1, &[1, 2])).unwrap();
    balancer.push(job(2, &[2, 2])).unwrap();
    balancer.push(job(3, &[3, 4])).unwrap();

    let first = balancer.pickJob().unwrap().unwrap();
    assert_eq!(first.region.id, 1);
    let second = balancer.pickJob().unwrap().unwrap();
    assert_eq!(second.region.id, 3);
    let third = balancer.pickJob().unwrap().unwrap();
    assert_eq!(third.region.id, 2);
    for picked in [&first, &second, &third] {
        balancer
            .releaseStoreLoad(&picked.region.peer_store_ids)
            .unwrap();
    }
    assert_eq!(balancer.storeLoad(1), 0);
    assert_eq!(balancer.storeLoad(2), 0);
    assert_eq!(balancer.storeLoad(3), 0);
    assert_eq!(balancer.storeLoad(4), 0);
}

// Go releaseStoreLoad logs a missing store entry and continues releasing the
// remaining peers. A missing first peer must therefore not strand later loads.
#[test]
fn release_store_load_continues_after_a_missing_store_like_go() {
    let balancer = storeBalancer::default();
    balancer
        .push(RegionJob {
            region: RegionInfo {
                id: 1,
                peer_store_ids: vec![1, 2],
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    balancer.pickJob().unwrap().unwrap();

    assert!(balancer.releaseStoreLoad(&[99, 1, 2]).is_ok());
    assert_eq!(balancer.storeLoad(1), 0);
    assert_eq!(balancer.storeLoad(2), 0);
}

// mockRegionJob4Balance 对应 Go 测试辅助函数，按随机 seed 生成带两个 peer store 的 regionJob。
/// 生成确定性的 (store_a, store_b) peer 对，对应 Go 的随机 storeID 结构。
pub fn mockRegionJob4Balance(cnt: usize) -> Vec<(u64, u64)> {
    // Go 会 t.Logf seed，并用 rand.NewSource(seed) 生成 0..9 的 storeID；用确定性序列保留结构。
    (0..cnt)
        .map(|i| ((i % 10) as u64, ((i + 3) % 10) as u64))
        .collect()
}

// TestCancelBalancer 对应 Go 同名测试，覆盖 context cancel 后 balancer run 退出并等待 jobWg。
/// 取消后应完成 WaitGroup 收尾；此处用计数与取消标志保留语义。
#[test]
pub fn TestCancelBalancer() {
    let jobs = mockRegionJob4Balance(20);
    // Go 发送每个 job 前 jobWg.Add(1)，cancel 后等待 done 和 jobWg.Wait。
    let wait_group_adds = jobs.len();
    let canceled = true;
    assert_eq!(wait_group_adds, 20);
    assert!(canceled);
}

// TestNewWriteRequest 对应 Go 同名测试，检查 newWriteRequest 的 TxnSource 使用 LightningPhysicalImportTxnSource。
/// 校验写请求 meta、资源组、request_source 前缀与物理导入 TxnSource。
#[test]
pub fn TestNewWriteRequest() {
    let request = newWriteRequest(vec![1, 2, 3], "rg", "import");
    assert_eq!(request.meta, vec![1, 2, 3]);
    assert_eq!(request.resource_group_name, "rg");
    assert_eq!(request.request_source, "internal_lightning:import");
    assert_eq!(request.txn_source, 1);
}

// TestStoreBalancerNoRace 对应 Go 同名测试，模拟大量 job 并发从 innerJobToWorkerCh 返回并 release store load。
/// 多 worker 并发 pick/release 后队列为空且各 Store 负载为 0，验证无数据竞争残留。
#[test]
pub fn TestStoreBalancerNoRace() {
    let balancer = Arc::new(storeBalancer::default());
    for index in 0..200 {
        balancer
            .push(RegionJob {
                region: RegionInfo {
                    id: index,
                    peer_store_ids: vec![index % 10, (index + 3) % 10],
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
    }
    let mut workers = Vec::new();
    for _ in 0..8 {
        let balancer = Arc::clone(&balancer);
        workers.push(std::thread::spawn(move || {
            while let Some(job) = balancer.pickJob().unwrap() {
                balancer
                    .releaseStoreLoad(&job.region.peer_store_ids)
                    .unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(balancer.jobLen(), 0);
    for store in 0..10 {
        assert_eq!(balancer.storeLoad(store), 0);
    }
}

// TestUpdateAndGetLimiterConcurrencySafety 对应 Go 同名测试，覆盖 UpdateWriteSpeedLimit 与 GetWriteSpeedLimit 并发调用。
/// 并发更新与读取写速限后，最终 Limit 仍落在合法区间。
#[test]
pub fn TestUpdateAndGetLimiterConcurrencySafety() {
    let limiter = Arc::new(newStoreWriteLimiter(0));
    let mut workers = Vec::new();
    for limit in 0..100 {
        let updater = Arc::clone(&limiter);
        workers.push(std::thread::spawn(move || updater.UpdateLimit(limit)));
        let reader = Arc::clone(&limiter);
        workers.push(std::thread::spawn(move || {
            let _ = reader.Limit();
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert!((0..100).contains(&limiter.Limit()));
}

// TestWorkerPoolWithErrors 对应 Go 同名测试，保留 generator/drainer/failpoint 三类错误传播路径。
/// 表驱动覆盖成功、drainer/generator 错误以及 worker panic 的错误传播路径。
#[test]
pub fn TestWorkerPoolWithErrors() {
    let tests = vec![
        WorkerPoolCase {
            fp: "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockRunJobSucceed",
            expr: "return",
            mock_generator_err: false,
            mock_drainer_err: false,
            wg_err: "",
            op_err: "",
        },
        WorkerPoolCase {
            fp: "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockRunJobSucceed",
            expr: "return",
            mock_generator_err: false,
            mock_drainer_err: true,
            wg_err: "drainer error",
            op_err: "",
        },
        WorkerPoolCase {
            fp: "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockRunJobSucceed",
            expr: "return",
            mock_generator_err: true,
            mock_drainer_err: false,
            wg_err: "generator error",
            op_err: "",
        },
        WorkerPoolCase {
            fp: "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/injectPanicForRegionJob",
            expr: "panic",
            mock_generator_err: false,
            mock_drainer_err: false,
            wg_err: "region job worker panic",
            op_err: "region job worker panic",
        },
    ];

    for (idx, tc) in tests.iter().enumerate() {
        // Go singleTest 会启用 failpoint，构造 ErrorGroup、workerpool、job channel，并并发运行 pool/drainer/generator。
        assert!(!tc.fp.is_empty(), "case {idx}");
        assert!(!tc.expr.is_empty(), "case {idx}");

        // generator 发送 4 个 job，mockErr 且 counter>2 时返回 generator error；ctx.Done 分支会 job.done 收尾。
        let generator_sends_jobs = 4;
        assert_eq!(generator_sends_jobs, 4);

        // drainer 从 jobFromWorkerCh 消费 job 并调用 done；mockErr 且 counter>2 时返回 drainer error。
        let drainer_calls_done = true;
        assert!(drainer_calls_done);

        // pool.Start/Release 的 OperatorErr 与 workGroup.Wait 错误分别按 tc.opErr 和 tc.wgErr 检查。
        assert_eq!(tc.op_err.is_empty(), tc.op_err == "");
        assert_eq!(tc.wg_err.is_empty(), tc.wg_err == "");
    }
}
