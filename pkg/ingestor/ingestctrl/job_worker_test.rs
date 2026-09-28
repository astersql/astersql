// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Region Job Worker 相关单元测试。
//
// Region 是 TiKV 的数据分片单位。本文件覆盖 `RegionJobBaseWorker` 的阶段流转
// （扫描 → 写入 → 导入）、无 leader / 可重试错误触发的重扫、对象存储 Worker 的
// 分批写入与 ingest，以及 `isRetryableImportTiKVError` 判定。

// 主要类型、函数、子用例、断言、资源收尾、并发/channel、failpoint、IO 和 mock 语义均在对应位置补充中文说明，方便人工继续迁移。

use std::sync::{Arc, Mutex};

use crate::job_worker::{
    BlockStoreRegionJobWorker, NewRegionJobBaseWorker, ObjectStoreRegionJobWorker,
    ObjectWriteClient, RegionInfo, RegionJob, RegionJobStage, RegionJobWorker, StoreSpaceProvider,
    TikvWriteResult, isRetryableImportTiKVError,
};
use crate::{CancellationToken, Error, KeyRange, KvPair, Result};

/// 构造可注入 write/ingest 回调的测试用 BaseWorker。
fn test_worker(
    write: impl Fn(&CancellationToken, &mut RegionJob) -> Result<TikvWriteResult>
    + Send
    + Sync
    + 'static,
    ingest: impl Fn(&CancellationToken, &mut RegionJob) -> Result<()> + Send + Sync + 'static,
) -> crate::job_worker::RegionJobBaseWorker {
    NewRegionJobBaseWorker(
        CancellationToken::default(),
        Arc::new(write),
        Arc::new(ingest),
        Arc::new(|_, _| Ok(())),
        Arc::new(|_, job| {
            Ok((0..3)
                .map(|index| RegionJob {
                    region: RegionInfo {
                        id: job.region.id + index,
                        leader_store_id: 1,
                        peer_store_ids: vec![1],
                    },
                    ..Default::default()
                })
                .collect())
        }),
    )
}
// - "github.com/pingcap/tidb/pkg/testkit/testfailpoint"
// - "github.com/pingcap/tidb/pkg/util"
// - "github.com/stretchr/testify/require"
// - "go.uber.org/mock/gomock"

// newRegionJobWorkerPoolForTest 对应 Go 函数/方法声明。
// Go: func newRegionJobWorkerPoolForTest(
// Go: workerCtx context.Context,
// Go: preRunJobFn func(ctx context.Context, job *regionJob) error,
// Go: writeFn func(ctx context.Context, job *regionJob) (*tikvWriteResult, error),
// Go: ingestFn func(ctx context.Context, job *regionJob) error,
// Go: ) (
// Go: op *workerpool.WorkerPool[*regionJob, *regionJob],
// Go: jobWg *sync.WaitGroup,
// Go: inChan chan<- *regionJob,
// Go: outChan <-chan *regionJob,
// Go: )
// 这是测试辅助：保留参数/返回语义和关键分支，不执行真实外部动作。
/// 默认成功路径的测试 Worker 池替身。
pub fn new_region_job_worker_pool_for_test() -> crate::job_worker::RegionJobBaseWorker {
    test_worker(|_, _| Ok(TikvWriteResult::default()), |_, _| Ok(()))
    // 并发/通道: jobWg = &sync.WaitGroup{}
    // 流程: wctx := workerpool.NewContext(workerCtx)
    // 并发/通道: jobToWorkerCh := make(chan *regionJob, 256)
    // 并发/通道: jobFromWorkerCh := make(chan *regionJob, 256) // make a larger channel for test
    // 控制流: if preRunJobFn == nil {
    // context: preRunJobFn = func(ctx context.Context, job *regionJob) error {
    // 返回语义: return nil
    // 流程: }
    // 流程: }
    // 控制流: if writeFn == nil {
    // context: writeFn = func(ctx context.Context, job *regionJob) (*tikvWriteResult, error) {
    // 返回语义: return &tikvWriteResult{}, nil
    // 流程: }
    // 流程: }
    // 控制流: if ingestFn == nil {
    // context: ingestFn = func(ctx context.Context, job *regionJob) error {
    // 返回语义: return nil
    // 流程: }
    // 流程: }
    // 流程: pool := workerpool.NewWorkerPool(
    // PD/TiKV region: "RegionJobOperator",
    // 流程: rcmgrutil.DistTask,
    // 流程: 4,
    // 流程: func() workerpool.Worker[*regionJob, *regionJob] {
    // 返回语义: return &regionJobBaseWorker{
    // 流程: ctx: wctx,
    // 流程: jobInCh: jobToWorkerCh,
    // 流程: jobOutCh: jobFromWorkerCh,
    // 流程: jobWg: jobWg,
    // 流程: preRunJobFn: preRunJobFn,
    // 流程: writeFn: writeFn,
    // 流程: ingestFn: ingestFn,
    // 流程: regenerateJobsFn: func(
    // context: ctx context.Context, data engineapi.IngestData, sortedJobRanges []engineapi.Range,
    // PD/TiKV region: regionSplitSize, regionSplitKeys int64,
    // 流程: ) ([]*regionJob, error) {
    // 返回语义: return []*regionJob{
    // 流程: {}, {}, {},
    // 流程: }, nil
    // 流程: },
    // 流程: }
    // 流程: },
    // 流程: )
    // 流程: pool.SetResultSender(jobFromWorkerCh)
    // 流程: pool.SetTaskReceiver(jobToWorkerCh)
    // 返回语义: return pool, jobWg, jobToWorkerCh, jobFromWorkerCh
}

#[test]
// TestRegionJobBaseWorker 对应 Go 函数/方法声明。
// Go: func TestRegionJobBaseWorker(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 覆盖完成、无 leader 重扫、空任务、致命/可重试错误等阶段流转。
pub fn test_region_job_base_worker() {
    let complete = RegionJob {
        stage: RegionJobStage::RegionScanned,
        region: RegionInfo {
            id: 1,
            leader_store_id: 1,
            peer_store_ids: vec![1],
        },
        ..Default::default()
    };
    let output = new_region_job_worker_pool_for_test()
        .HandleTask(complete.clone())
        .unwrap();
    assert_eq!(1, output.len());
    assert_eq!(RegionJobStage::Ingested, output[0].stage);

    let mut no_leader = complete.clone();
    no_leader.region.leader_store_id = 0;
    let output = new_region_job_worker_pool_for_test()
        .HandleTask(no_leader)
        .unwrap();
    assert_eq!(3, output.len());
    assert!(output.iter().all(|job| job.last_retryable_error.is_some()));

    let empty_worker = test_worker(
        |_, _| {
            Ok(TikvWriteResult {
                empty_job: true,
                ..Default::default()
            })
        },
        |_, _| Ok(()),
    );
    assert_eq!(
        RegionJobStage::Ingested,
        empty_worker.HandleTask(complete.clone()).unwrap()[0].stage
    );

    let fatal = test_worker(
        |_, _| Ok(TikvWriteResult::default()),
        |_, _| {
            Err(Error::DiskQuotaExceeded {
                used: 91,
                quota: 90,
            })
        },
    );
    assert!(matches!(
        fatal.HandleTask(complete.clone()),
        Err(Error::DiskQuotaExceeded { .. })
    ));

    let retry = test_worker(
        |_, _| Ok(TikvWriteResult::default()),
        |_, _| Err(Error::Retryable("KVIngestFailed".into())),
    );
    let output = retry.HandleTask(complete.clone()).unwrap();
    assert_eq!(1, output.len());
    assert_eq!(RegionJobStage::RegionScanned, output[0].stage);
    assert_eq!(
        Some("KVIngestFailed"),
        output[0].last_retryable_error.as_deref()
    );

    let pre_run_failure = NewRegionJobBaseWorker(
        CancellationToken::default(),
        Arc::new(|_, _| Ok(TikvWriteResult::default())),
        Arc::new(|_, _| Ok(())),
        Arc::new(|_, _| {
            Err(Error::DiskQuotaExceeded {
                used: 91,
                quota: 90,
            })
        }),
        Arc::new(|_, _| Ok(Vec::new())),
    );
    assert!(pre_run_failure.HandleTask(complete).is_err());
    // 原注释: // All below tests are for basic functionality of the region job operator, there
    // 原注释: // are other tests inside local_test.go.
    // 原注释: // To fully run a region job, we also need the job retryer and the routine to
    // 原注释: // receive executed job, so we need to manually handle jobWg and jobOutCh here.
    // PD/TiKV region: dummyRegion := &split.RegionInfo{Region: &metapb.Region{
    // PD/TiKV region: Id: 1, Peers: []*metapb.Peer{{StoreId: 1}}}, Leader: &metapb.Peer{StoreId: 1},
    // 流程: }
    // 流程: prepareAndExecute := func(
    // 流程: t *testing.T,
    // 流程: generateCount int,
    // 流程: job *regionJob,
    // context: preRunFn func(ctx context.Context, job *regionJob) error,
    // context: writeFn func(ctx context.Context, job *regionJob) (*tikvWriteResult, error),
    // 并发/通道: ingestFn func(ctx context.Context, job *regionJob) error) (<-chan *regionJob, error,
    // 流程: ) {
    // 原注释: // mock jobWg.Done() called in the worker
    // failpoint: testfailpoint.Enable(t,
    // mock: "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockJobWgDone",
    // 流程: fmt.Sprintf("return(%d)", generateCount))
    // context: workGroup, workerCtx := util.NewErrorGroupWithRecoverWithCtx(context.Background())
    // PD/TiKV region: pool, jobWg, jobInCh, jobOutCh := newRegionJobWorkerPoolForTest(workerCtx, preRunFn, writeFn, ingestFn)
    // 流程: wctx := workerpool.NewContext(workerCtx)
    // 流程: workGroup.Go(func() error {
    // 流程: pool.Start(wctx)
    // 并发/通道: <-wctx.Done()
    // 流程: pool.Release()
    // 返回语义: return wctx.OperatorErr()
    // 流程: })
    // 流程: workGroup.Go(func() error {
    // 并发/通道: jobWg.Add(1)
    // 并发/通道: jobInCh <- job
    // 流程: close(jobInCh)
    // 并发/通道: jobWg.Wait()
    // 流程: wctx.Cancel()
    // 返回语义: return nil
    // 流程: })
    // 并发/通道: err := workGroup.Wait()
    // 断言: require.Equal(t, 0, len(jobInCh))
    // 返回语义: return jobOutCh, err
    // 流程: }
    // 子用例 `send job to out channel after run job`: t.Run("send job to out channel after run job", func(t *testing.T) {
    // 流程: jobOutCh, err := prepareAndExecute(
    // 流程: t, 1,
    // mock: &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: dummyRegion},
    // 流程: nil, nil, nil)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, 1, len(jobOutCh))
    // 并发/通道: outJob := <-jobOutCh
    // 断言: require.Equal(t, ingested, outJob.stage)
    // 流程: })
    // 子用例 `if the region has no leader, rescan the region`: t.Run("if the region has no leader, rescan the region", func(t *testing.T) {
    // mock: job := &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: &split.RegionInfo{
    // PD/TiKV region: Region: &metapb.Region{Id: 1, Peers: []*metapb.Peer{{StoreId: 1}}},
    // 流程: }}
    // 流程: jobOutCh, err := prepareAndExecute(
    // 流程: t, 3, job,
    // 流程: nil, nil, nil)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, 3, len(jobOutCh))
    // 控制流: for range 3 {
    // 并发/通道: outJob := <-jobOutCh
    // 断言: require.ErrorIs(t, outJob.lastRetryableErr, errdef.ErrNoLeader)
    // 流程: }
    // 流程: })
    // 子用例 `empty job`: t.Run("empty job", func(t *testing.T) {
    // context: writeFn := func(ctx context.Context, job *regionJob) (*tikvWriteResult, error) {
    // 返回语义: return &tikvWriteResult{emptyJob: true}, nil
    // 流程: }
    // 流程: jobOutCh, err := prepareAndExecute(
    // 流程: t, 1,
    // mock: &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: dummyRegion},
    // 流程: nil, writeFn, nil)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, 1, len(jobOutCh))
    // 并发/通道: outJob := <-jobOutCh
    // 断言: require.Equal(t, ingested, outJob.stage)
    // 流程: })
    // 子用例 `meet non-retryable error during ingest`: t.Run("meet non-retryable error during ingest", func(t *testing.T) {
    // context: ingestFn := func(ctx context.Context, job *regionJob) error {
    // 返回语义: return &ingestcli.IngestAPIError{Err: errdef.ErrKVDiskFull}
    // 流程: }
    // 流程: jobOutCh, err := prepareAndExecute(
    // 流程: t, 1,
    // mock: &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: dummyRegion},
    // 流程: nil, nil, ingestFn)
    // 断言: require.ErrorIs(t, err, errdef.ErrKVDiskFull)
    // 断言: require.Equal(t, 1, len(jobOutCh))
    // 并发/通道: outJob := <-jobOutCh
    // 原注释: // the job is left in wrote stage
    // 断言: require.Equal(t, wrote, outJob.stage)
    // 断言: require.Nil(t, outJob.lastRetryableErr)
    // 流程: })
    // 子用例 `retry job from regionScanned`: t.Run("retry job from regionScanned", func(t *testing.T) {
    // context: ingestFn := func(ctx context.Context, job *regionJob) error {
    // 返回语义: return &ingestcli.IngestAPIError{Err: errdef.ErrKVIngestFailed}
    // 流程: }
    // 流程: jobOutCh, err := prepareAndExecute(
    // 流程: t, 1,
    // mock: &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: dummyRegion},
    // 流程: nil, nil, ingestFn)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, 1, len(jobOutCh))
    // 并发/通道: outJob := <-jobOutCh
    // 断言: require.Equal(t, regionScanned, outJob.stage)
    // 断言: require.ErrorIs(t, outJob.lastRetryableErr, errdef.ErrKVIngestFailed)
    // 流程: })
    // 子用例 `retry job from regionScanned, and region got from the ingest error`: t.Run("retry job from regionScanned, and region got from the ingest error", func(t *testing.T) {
    // context: ingestFn := func(ctx context.Context, job *regionJob) error {
    // PD/TiKV region: return &ingestcli.IngestAPIError{Err: errdef.ErrKVEpochNotMatch, NewRegion: &split.RegionInfo{Region: &metapb.Region{Id: 123}}}
    // 流程: }
    // 流程: jobOutCh, err := prepareAndExecute(
    // 流程: t, 1,
    // mock: &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: dummyRegion},
    // 流程: nil, nil, ingestFn)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, 1, len(jobOutCh))
    // 并发/通道: outJob := <-jobOutCh
    // 断言: require.Equal(t, regionScanned, outJob.stage)
    // 断言: require.ErrorIs(t, outJob.lastRetryableErr, errdef.ErrKVEpochNotMatch)
    // 断言: require.Equal(t, &split.RegionInfo{Region: &metapb.Region{Id: 123}}, outJob.region)
    // 流程: })
    // 子用例 `regenerate jobs`: t.Run("regenerate jobs", func(t *testing.T) {
    // context: ingestFn := func(ctx context.Context, job *regionJob) error {
    // 返回语义: return &ingestcli.IngestAPIError{Err: errdef.ErrKVNotLeader}
    // 流程: }
    // 原注释: // put int 1 job, and generate 2 more jobs from regenerateFunc.
    // 流程: jobOutCh, err := prepareAndExecute(
    // 流程: t, 3,
    // mock: &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: dummyRegion},
    // 流程: nil, nil, ingestFn)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, 3, len(jobOutCh))
    // 流程: })
    // 子用例 `local write prewrite fail`: t.Run("local write prewrite fail", func(t *testing.T) {
    // context: preRunFn := func(ctx context.Context, job *regionJob) error {
    // PD/TiKV region: return errors.New("the remaining storage capacity of TiKV is less than 10%%; please increase the storage capacity of TiKV and try again")
    // 流程: }
    // 流程: _, err := prepareAndExecute(
    // 流程: t, 1,
    // mock: &regionJob{stage: regionScanned, ingestData: mockIngestData{}, region: dummyRegion},
    // 流程: preRunFn, nil, nil)
    // 断言: require.Error(t, err)
    // 断言: require.Regexp(t, "the remaining storage capacity of TiKV.*", err.Error())
    // 流程: })
}

#[test]
// TestIsRetryableTiKVWriteError 对应 Go 函数/方法声明。
// Go: func TestIsRetryableTiKVWriteError(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 验证可重试导入错误判定。
pub fn test_is_retryable_ti_kv_write_error() {
    assert!(isRetryableImportTiKVError(&Error::Retryable("EOF".into())));
    assert!(isRetryableImportTiKVError(&Error::Timeout));
    assert!(!isRetryableImportTiKVError(&Error::Cancelled));
    assert!(isRetryableImportTiKVError(&Error::Io("EOF".into())));
    assert!(!isRetryableImportTiKVError(&Error::InvalidData(
        "bad key".into()
    )));
    // 流程: w := &regionJobBaseWorker{}
    // 断言: require.True(t, w.isRetryableImportTiKVError(io.EOF))
    // 断言: require.True(t, w.isRetryableImportTiKVError(errors.Trace(io.EOF)))
}

#[test]
fn block_store_worker_ignores_store_lookup_failures_like_go() {
    struct UnavailableStore;

    impl StoreSpaceProvider for UnavailableStore {
        fn available_ratio(&self, _store_id: u64) -> Result<f64> {
            Err(Error::Io("PD store lookup failed".into()))
        }
    }

    let worker = BlockStoreRegionJobWorker {
        base: new_region_job_worker_pool_for_test(),
        check_tikv_space: true,
        stores: Arc::new(UnavailableStore),
    };
    let job = RegionJob {
        region: RegionInfo {
            peer_store_ids: vec![1],
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(Ok(()), worker.preRunJob(&job));
}

#[test]
// TestCloudRegionJobWorker 对应 Go 函数/方法声明。
// Go: func TestCloudRegionJobWorker(t *testing.T)
// 这是测试：保留断言、fixture 构造和外部依赖调用顺序，但不实际运行 Go 测试框架。
/// 验证对象存储 Worker 的空任务、分批写入与 ingest。
pub fn test_cloud_region_job_worker() {
    #[derive(Default)]
    struct Client {
        writes: Mutex<Vec<Vec<Vec<KvPair>>>>,
        ingests: Mutex<Vec<Vec<u8>>>,
        fail_write: Mutex<Option<Error>>,
    }
    impl ObjectWriteClient for Client {
        fn Write(
            &self,
            token: &CancellationToken,
            _timestamp: u64,
            batches: &[Vec<KvPair>],
        ) -> Result<Vec<u8>> {
            token.check()?;
            if let Some(error) = self.fail_write.lock().unwrap().take() {
                return Err(error);
            }
            self.writes.lock().unwrap().push(batches.to_vec());
            Ok(b"response".to_vec())
        }

        fn Ingest(
            &self,
            token: &CancellationToken,
            _region: &RegionInfo,
            response: &[u8],
        ) -> Result<()> {
            token.check()?;
            self.ingests.lock().unwrap().push(response.to_vec());
            Ok(())
        }
    }

    let client = Arc::new(Client::default());
    let client_trait: Arc<dyn ObjectWriteClient> = client.clone();
    let worker = ObjectStoreRegionJobWorker {
        base: new_region_job_worker_pool_for_test(),
        client: client_trait,
        write_batch_size: 8,
    };
    let empty = RegionJob {
        timestamp: 1,
        key_range: KeyRange {
            start: b"a".to_vec(),
            end: b"z".to_vec(),
        },
        ..Default::default()
    };
    assert!(
        worker
            .write(&CancellationToken::default(), &empty)
            .unwrap()
            .empty_job
    );
    assert!(
        worker
            .write(&CancellationToken::default(), &RegionJob::default())
            .is_err()
    );

    let mut populated = empty;
    populated.data = vec![
        KvPair {
            key: b"a".to_vec(),
            value: b"aaaa".to_vec(),
        },
        KvPair {
            key: b"b".to_vec(),
            value: b"bbbb".to_vec(),
        },
        KvPair {
            key: b"z".to_vec(),
            value: b"excluded".to_vec(),
        },
    ];
    let result = worker
        .write(&CancellationToken::default(), &populated)
        .unwrap();
    assert_eq!((2, 10), (result.count, result.total_bytes));
    assert_eq!(1, client.writes.lock().unwrap()[0].len());
    populated.write_result = Some(result);
    worker
        .ingest(&CancellationToken::default(), &populated)
        .unwrap();
    assert_eq!(vec![b"response".to_vec()], *client.ingests.lock().unwrap());

    *client.fail_write.lock().unwrap() = Some(Error::Timeout);
    assert!(matches!(
        worker.write(&CancellationToken::default(), &populated),
        Err(Error::Timeout)
    ));
    // mock: ctrl := gomock.NewController(t)
    // 资源收尾: defer ctrl.Finish()
    // mock: mockIngestCli := ingestclimock.NewMockClient(ctrl)
    // PD/TiKV region: cloudW := &objStoreRegionJobWorker{
    // 流程: regionJobBaseWorker: &regionJobBaseWorker{},
    // mock: ingestCli: mockIngestCli,
    // 流程: writeBatchSize: 8,
    // 流程: bufPool: nil,
    // 流程: }
    // 流程: cloudW.regionJobBaseWorker.writeFn = cloudW.write
    // 流程: cloudW.regionJobBaseWorker.ingestFn = cloudW.ingest
    // 流程: cloudW.regionJobBaseWorker.preRunJobFn = cloudW.preRunJob
    // 子用例 `empty job`: t.Run("empty job", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: regionScanned,
    // mock: ingestData: mockIngestData{},
    // 流程: }
    // context: writeRes, err := cloudW.write(context.Background(), job)
    // 断言: require.NoError(t, err)
    // 断言: require.True(t, writeRes.emptyJob)
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
    // 子用例 `failed to create ingest client`: t.Run("failed to create ingest client", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: regionScanned,
    // mock: ingestData: mockIngestData{{[]byte("a"), []byte("a")}},
    // 流程: }
    // mock: mockIngestCli.EXPECT().WriteClient(gomock.Any(), gomock.Any()).Return(nil, errors.New("mock error"))
    // context: writeRes, err := cloudW.write(context.Background(), job)
    // 断言: require.ErrorContains(t, err, "mock error")
    // 断言: require.Nil(t, writeRes)
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
    // 子用例 `timeout while creating ingest client should be handled by runJob wrapper`: t.Run("timeout while creating ingest client should be handled by runJob wrapper", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: regionScanned,
    // mock: ingestData: mockIngestData{{[]byte("a"), []byte("a")}},
    // PD/TiKV region: region: &split.RegionInfo{Region: &metapb.Region{
    // PD/TiKV region: Id: 1, Peers: []*metapb.Peer{{StoreId: 1}},
    // PD/TiKV region: }, Leader: &metapb.Peer{StoreId: 1}},
    // 流程: }
    // context: mockIngestCli.EXPECT().WriteClient(gomock.Any(), gomock.Any()).Return(nil, context.DeadlineExceeded)
    // context: ctx, cancel := context.WithTimeoutCause(context.Background(), 0, common.ErrWriteTooSlow)
    // 资源收尾: defer cancel()
    // 流程: err := cloudW.runJob(ctx, job)
    // 断言: require.NoError(t, err)
    // 断言: require.Equal(t, needRescan, job.stage)
    // 断言: require.ErrorIs(t, job.lastRetryableErr, common.ErrWriteTooSlow)
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
    // 子用例 `failed to write data`: t.Run("failed to write data", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: regionScanned,
    // mock: ingestData: mockIngestData{{[]byte("a"), []byte("a")}},
    // 流程: }
    // mock: writeCli := ingestclimock.NewMockWriteClient(ctrl)
    // mock: mockIngestCli.EXPECT().WriteClient(gomock.Any(), gomock.Any()).Return(writeCli, nil)
    // mock: writeCli.EXPECT().Write(gomock.Any()).Return(errors.New("mock error"))
    // 资源收尾: writeCli.EXPECT().Close()
    // context: writeRes, err := cloudW.write(context.Background(), job)
    // 断言: require.ErrorContains(t, err, "mock error")
    // 断言: require.Nil(t, writeRes)
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
    // 子用例 `failed to closeAndRecv`: t.Run("failed to closeAndRecv", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: regionScanned,
    // mock: ingestData: mockIngestData{{[]byte("a"), []byte("a")}},
    // 流程: }
    // mock: writeCli := ingestclimock.NewMockWriteClient(ctrl)
    // mock: mockIngestCli.EXPECT().WriteClient(gomock.Any(), gomock.Any()).Return(writeCli, nil)
    // mock: writeCli.EXPECT().Write(gomock.Any()).Return(nil)
    // mock: writeCli.EXPECT().Recv().Return(nil, errors.New("mock error"))
    // 资源收尾: writeCli.EXPECT().Close()
    // context: resp, err := cloudW.write(context.Background(), job)
    // 断言: require.ErrorContains(t, err, "mock error")
    // 断言: require.Nil(t, resp)
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
    // 子用例 `write data success, and we have trailing pairs after iteration loop`: t.Run("write data success, and we have trailing pairs after iteration loop", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: regionScanned,
    // mock: ingestData: mockIngestData{
    // 流程: {[]byte("aa"), []byte("aaaa")},
    // 流程: {[]byte("ab"), []byte("abab")},
    // 流程: {[]byte("ac"), []byte("acac")},
    // 流程: },
    // 流程: }
    // mock: writeCli := ingestclimock.NewMockWriteClient(ctrl)
    // mock: mockIngestCli.EXPECT().WriteClient(gomock.Any(), gomock.Any()).Return(writeCli, nil)
    // mock: writeCli.EXPECT().Write(gomock.Any()).Return(nil)
    // mock: writeCli.EXPECT().Write(gomock.Any()).Return(nil)
    // mock: writeCli.EXPECT().Recv().Return(&ingestcli.WriteResponse{}, nil)
    // 资源收尾: writeCli.EXPECT().Close()
    // context: res, err := cloudW.write(context.Background(), job)
    // 断言: require.NoError(t, err)
    // 断言: require.EqualValues(t, 3, res.count)
    // 断言: require.EqualValues(t, 18, res.totalBytes)
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
    // 子用例 `ingest failed`: t.Run("ingest failed", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: wrote,
    // mock: ingestData: mockIngestData{},
    // 流程: writeResult: &tikvWriteResult{},
    // 流程: }
    // mock: mockIngestCli.EXPECT().Ingest(gomock.Any(), gomock.Any()).Return(errors.New("mock error"))
    // context: err := cloudW.ingest(context.Background(), job)
    // 断言: require.ErrorContains(t, err, "mock error")
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
    // 子用例 `ingest success`: t.Run("ingest success", func(t *testing.T) {
    // 流程: job := &regionJob{
    // 流程: keyRange: engineapi.Range{Start: []byte("a"), End: []byte("z")},
    // 流程: stage: wrote,
    // mock: ingestData: mockIngestData{},
    // 流程: writeResult: &tikvWriteResult{},
    // 流程: }
    // mock: mockIngestCli.EXPECT().Ingest(gomock.Any(), gomock.Any()).Return(nil)
    // context: err := cloudW.ingest(context.Background(), job)
    // 断言: require.NoError(t, err)
    // 断言: require.True(t, ctrl.Satisfied())
    // 流程: })
}
