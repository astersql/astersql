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

// Region 导入任务（Region Job）的 Worker 实现。
//
// Region 是 TiKV 的数据分片。Worker 按阶段推进任务：扫描 Region → 写入（write）→
// 导入（ingest）；遇可重试错误或无 leader 时转为重扫（NeedRescan）并重新生成子任务。
// 另提供对象存储路径的分批写入 Worker，以及导入前检查 Store 磁盘空间的阻塞 Worker。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::region_job::getNextStageOnIngestError;
use crate::{CancellationToken, Error, KeyRange, KvPair, Result};

/// 将 i32 包装为原子变量（对应 Go atomic.Int32）。
pub fn toAtomic(value: i32) -> AtomicI32 {
    AtomicI32::new(value)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Region 任务阶段：已扫描 / 已写入 / 已导入 / 需重扫。
pub enum RegionJobStage {
    #[default]
    /// 已拿到 Region 元信息，待写入。
    RegionScanned,
    /// 数据已写入 TiKV/对象存储，待 ingest。
    Wrote,
    /// SST/对象已成功导入。
    Ingested,
    /// 需重新扫描 Region（如 NotLeader、epoch 不匹配）。
    NeedRescan,
}

#[derive(Clone, Debug, Default)]
/// Region 元信息：ID、leader Store 与全部 peer Store。
pub struct RegionInfo {
    /// Region ID。
    pub id: u64,
    /// Leader 所在 Store ID；为 0 表示无 leader。
    pub leader_store_id: u64,
    /// 全部 peer 的 Store ID 列表。
    pub peer_store_ids: Vec<u64>,
}

#[derive(Clone, Debug, Default)]
/// 向 TiKV/对象存储写入后的结果摘要。
pub struct TikvWriteResult {
    /// 写入的键值条数。
    pub count: i64,
    /// 写入的总字节数。
    pub total_bytes: i64,
    /// 若一次未写完，下一段起始键（继续扫描写入）。
    pub remaining_start_key: Option<Vec<u8>>,
    /// 范围内无数据，可直接视为已导入。
    pub empty_job: bool,
    /// 写入 RPC 响应，供后续 Ingest 使用。
    pub response: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
/// 单个 Region 的导入任务及其阶段、数据与重试状态。
pub struct RegionJob {
    /// 当前任务阶段。
    pub stage: RegionJobStage,
    /// 目标 Region 信息。
    pub region: RegionInfo,
    /// 本任务负责的键范围 [start, end)。
    pub key_range: KeyRange,
    /// 待写入的键值数据。
    pub data: Vec<KvPair>,
    /// 提交时间戳（commit TS）；对象存储路径要求非 0。
    pub timestamp: u64,
    /// 最近一次写入结果。
    pub write_result: Option<TikvWriteResult>,
    /// 最近一次可重试错误信息。
    pub last_retryable_error: Option<String>,
    /// Preserve the original error identity when the dispatcher exhausts retries.
    pub last_retryable_cause: Option<Error>,
    /// Region 按大小分裂阈值。
    pub region_split_size: i64,
    /// Region 按键数分裂阈值。
    pub region_split_keys: i64,
    /// 任务引用计数（重扫生成多子任务时递增）。
    pub(crate) references: Arc<AtomicUsize>,
    pub(crate) resources: Option<Arc<crate::import_pipeline::JobResources>>,
    pub(crate) retry_count: usize,
    pub(crate) completed: Arc<std::sync::atomic::AtomicBool>,
}

impl RegionJob {
    /// 切换任务阶段。
    pub fn convertStageTo(&mut self, stage: RegionJobStage) {
        if stage == RegionJobStage::Ingested && self.stage != stage {
            if let (Some(resources), Some(result)) = (&self.resources, &self.write_result) {
                resources.finish(result.total_bytes, result.count);
            }
        }
        self.stage = stage;
    }
    /// 增加引用计数。
    pub fn r#ref(&self) {
        self.references.fetch_add(1, Ordering::AcqRel);
        if let Some(resources) = &self.resources {
            resources.reference();
        }
    }
    /// 减少引用计数。
    pub fn done(&self) {
        if self.resources.is_some() && self.completed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.references.fetch_sub(1, Ordering::AcqRel);
        if let Some(resources) = &self.resources {
            resources.done();
        }
    }
    /// 当前引用计数。
    pub fn ref_count(&self) -> usize {
        self.references.load(Ordering::Acquire)
    }
}

/// Region 任务 Worker：处理单个任务并可能产出后续任务。
pub trait RegionJobWorker: Send {
    /// 处理任务，返回需继续调度的任务列表。
    fn HandleTask(&self, job: RegionJob) -> Result<Vec<RegionJob>>;
    /// 关闭 Worker。
    fn Close(&self) -> Result<()>;
}

/// 写入回调：将任务数据写入存储。
type WriteFn =
    Arc<dyn Fn(&CancellationToken, &mut RegionJob) -> Result<TikvWriteResult> + Send + Sync>;
/// 导入回调：将已写入数据 ingest 到 Region。
type IngestFn = Arc<dyn Fn(&CancellationToken, &mut RegionJob) -> Result<()> + Send + Sync>;
/// 任务执行前钩子（如磁盘空间检查）。
type PreRunFn = Arc<dyn Fn(&CancellationToken, &RegionJob) -> Result<()> + Send + Sync>;
/// 需重扫时重新生成子任务的回调。
type RegenerateFn =
    Arc<dyn Fn(&CancellationToken, &RegionJob) -> Result<Vec<RegionJob>> + Send + Sync>;

/// Region 任务基础 Worker：驱动阶段状态机与超时控制。
pub struct RegionJobBaseWorker {
    /// 取消令牌，用于中止长时间导入。
    token: CancellationToken,
    /// 写入实现。
    write_fn: WriteFn,
    /// 导入实现。
    ingest_fn: IngestFn,
    /// 执行前钩子。
    pre_run_job_fn: PreRunFn,
    /// 执行后回调（传入 peer store id 列表）。
    after_run_job_fn: Option<Arc<dyn Fn(&[u64]) + Send + Sync>>,
    /// 重扫时再生任务。
    regenerate_jobs_fn: RegenerateFn,
    /// 单次写入超时；超时视为可重试慢写错误。
    write_timeout: Duration,
}

/// 构造基础 Worker；默认写入超时 15 分钟。
pub fn NewRegionJobBaseWorker(
    token: CancellationToken,
    write_fn: WriteFn,
    ingest_fn: IngestFn,
    pre_run_job_fn: PreRunFn,
    regenerate_jobs_fn: RegenerateFn,
) -> RegionJobBaseWorker {
    RegionJobBaseWorker {
        token,
        write_fn,
        ingest_fn,
        pre_run_job_fn,
        after_run_job_fn: None,
        regenerate_jobs_fn,
        write_timeout: Duration::from_secs(15 * 60),
    }
}

impl RegionJobBaseWorker {
    /// 设置任务执行后的回调。
    pub fn set_after_run(&mut self, callback: Arc<dyn Fn(&[u64]) + Send + Sync>) {
        self.after_run_job_fn = Some(callback);
    }

    /// 设置写入超时。
    pub fn set_write_timeout(&mut self, timeout: Duration) {
        self.write_timeout = timeout;
    }

    /// 运行任务并按最终阶段决定返回原任务或重扫再生任务。
    fn process(&self, mut job: RegionJob) -> Result<Vec<RegionJob>> {
        let result = self.runJob(&mut job);
        if let Some(callback) = &self.after_run_job_fn {
            callback(&job.region.peer_store_ids);
        }
        match job.stage {
            RegionJobStage::RegionScanned | RegionJobStage::Wrote | RegionJobStage::Ingested => {
                self.token.check()?;
                result?;
                Ok(vec![job])
            }
            RegionJobStage::NeedRescan => {
                let mut jobs = (self.regenerate_jobs_fn)(&self.token, &job)?;
                result?;
                if jobs.len() > 1 {
                    for _ in 1..jobs.len() {
                        job.r#ref();
                    }
                }
                for (index, regenerated) in jobs.iter_mut().enumerate() {
                    if index == 0 {
                        regenerated.completed.clone_from(&job.completed);
                    }
                    regenerated.resources.clone_from(&job.resources);
                    regenerated.references.clone_from(&job.references);
                    regenerated
                        .last_retryable_cause
                        .clone_from(&job.last_retryable_cause);
                    regenerated
                        .last_retryable_error
                        .clone_from(&job.last_retryable_error);
                }
                Ok(jobs)
            }
        }
    }

    /// 阶段状态机：Scanned→Wrote→Ingested，支持分段剩余键继续写。
    pub fn runJob(&self, job: &mut RegionJob) -> Result<()> {
        (self.pre_run_job_fn)(&self.token, job)?;
        loop {
            self.token.check()?;
            if job.stage == RegionJobStage::RegionScanned {
                // 无 leader 时无法写入，标记需重扫
                if job.region.leader_store_id == 0 {
                    job.last_retryable_error =
                        Some(format!("region {} has no leader", job.region.id));
                    job.convertStageTo(RegionJobStage::NeedRescan);
                    return Ok(());
                }
                match self.writeWithTimeout(job) {
                    Ok(result) if result.empty_job => job.convertStageTo(RegionJobStage::Ingested),
                    Ok(result) => {
                        job.write_result = Some(result);
                        job.convertStageTo(RegionJobStage::Wrote);
                    }
                    // 可重试写错误：RequestTooNew 留在 Scanned，其余需重扫
                    Err(error) if isRetryableImportTiKVError(&error) => {
                        let request_too_new = error.to_string().contains("RequestTooNew");
                        job.last_retryable_error = Some(error.to_string());
                        job.last_retryable_cause = Some(error.clone());
                        job.convertStageTo(if request_too_new {
                            RegionJobStage::RegionScanned
                        } else {
                            RegionJobStage::NeedRescan
                        });
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
            }
            if job.stage == RegionJobStage::Wrote {
                match (self.ingest_fn)(&self.token, job) {
                    Ok(()) => job.convertStageTo(RegionJobStage::Ingested),
                    Err(error) if isRetryableImportTiKVError(&error) => {
                        job.last_retryable_error = Some(error.to_string());
                        job.last_retryable_cause = Some(error.clone());
                        job.convertStageTo(getNextStageOnIngestError(&error));
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
            }
            if job.stage != RegionJobStage::Ingested {
                return Ok(());
            }
            let remaining = job
                .write_result
                .as_ref()
                .and_then(|result| result.remaining_start_key.clone());
            // 无剩余键则本任务完成；否则从 remaining 继续下一轮写入
            let Some(remaining) = remaining else {
                return Ok(());
            };
            job.key_range.start = remaining;
            job.convertStageTo(RegionJobStage::RegionScanned);
        }
    }

    /// 调用写入回调并检查是否超过 write_timeout。
    fn writeWithTimeout(&self, job: &mut RegionJob) -> Result<TikvWriteResult> {
        let started = Instant::now();
        let result = (self.write_fn)(&self.token, job);
        if started.elapsed() > self.write_timeout {
            Err(Error::Retryable("write to TiKV is too slow".into()))
        } else {
            result
        }
    }
}

impl RegionJobWorker for RegionJobBaseWorker {
    fn HandleTask(&self, job: RegionJob) -> Result<Vec<RegionJob>> {
        match catch_unwind(AssertUnwindSafe(|| self.process(job.clone()))) {
            Ok(result) => result,
            Err(payload) => {
                let (label, info, _) = <RegionJob as astersql_resourcemanager_pool_workerpool::TaskMayPanic>::RecoverArgs(&job);
                log::error!(
                    "{label}: {info}: region job worker panic: {payload:?}; stack={}",
                    std::backtrace::Backtrace::force_capture()
                );
                astersql_metrics::metrics::PanicCounter
                    .with_label_values(&[&label])
                    .inc();
                job.done();
                Err(Error::InvalidData("region job worker panic".into()))
            }
        }
    }

    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

/// 查询 Store 可用空间比例的提供者。
pub trait StoreSpaceProvider: Send + Sync {
    /// 返回指定 Store 的可用容量占比（0.0–1.0）。
    fn available_ratio(&self, store_id: u64) -> Result<f64>;
}

/// 在导入前检查 TiKV Store 磁盘空间的 Worker 包装。
pub struct BlockStoreRegionJobWorker {
    /// 内嵌基础 Worker。
    pub base: RegionJobBaseWorker,
    /// 是否启用磁盘空间检查。
    pub check_tikv_space: bool,
    /// Store 空间查询实现。
    pub stores: Arc<dyn StoreSpaceProvider>,
}

impl BlockStoreRegionJobWorker {
    /// 若启用检查且任一 peer 可用空间 < 10%，返回磁盘配额错误。
    pub fn preRunJob(&self, job: &RegionJob) -> Result<()> {
        if !self.check_tikv_space {
            return Ok(());
        }
        for store_id in &job.region.peer_store_ids {
            match self.stores.available_ratio(*store_id) {
                Ok(ratio) if ratio < 0.10 => {
                    return Err(Error::DiskQuotaExceeded {
                        used: 91,
                        quota: 90,
                    });
                }
                Ok(_) | Err(_) => {}
            }
        }
        Ok(())
    }
}

/// 对象存储写入/导入客户端抽象。
pub trait ObjectWriteClient: Send + Sync {
    /// 按批写入键值，返回供 Ingest 使用的响应。
    fn Write(
        &self,
        token: &CancellationToken,
        timestamp: u64,
        batches: &[Vec<KvPair>],
    ) -> Result<Vec<u8>>;
    /// 将写入响应 ingest 到指定 Region。
    fn Ingest(&self, token: &CancellationToken, region: &RegionInfo, response: &[u8])
    -> Result<()>;
}

/// 面向对象存储的 Region 任务 Worker：按 write_batch_size 分批写入。
pub struct ObjectStoreRegionJobWorker {
    pub base: RegionJobBaseWorker,
    /// 对象存储客户端。
    pub client: Arc<dyn ObjectWriteClient>,
    /// 单批写入字节阈值。
    pub write_batch_size: usize,
}

impl ObjectStoreRegionJobWorker {
    /// 过滤键范围后分批 Write；空数据返回 empty_job。
    pub fn write(&self, token: &CancellationToken, job: &RegionJob) -> Result<TikvWriteResult> {
        if job.timestamp == 0 {
            return Err(Error::InvalidData("data commitTS is 0".into()));
        }
        let data: Vec<_> = job
            .data
            .iter()
            .filter(|pair| {
                pair.key >= job.key_range.start
                    && (job.key_range.end.is_empty() || pair.key < job.key_range.end)
            })
            .cloned()
            .collect();
        if data.is_empty() {
            return Ok(TikvWriteResult {
                empty_job: true,
                ..Default::default()
            });
        }
        let mut batches = Vec::new();
        let mut batch = Vec::new();
        let mut size = 0usize;
        for pair in data {
            size += pair.size();
            batch.push(pair);
            if size >= self.write_batch_size.max(1) {
                batches.push(std::mem::take(&mut batch));
                size = 0;
            }
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        let count = batches.iter().map(Vec::len).sum::<usize>() as i64;
        let total_bytes = batches.iter().flatten().map(KvPair::size).sum::<usize>() as i64;
        let response = self.client.Write(token, job.timestamp, &batches)?;
        Ok(TikvWriteResult {
            count,
            total_bytes,
            response,
            ..Default::default()
        })
    }

    /// 使用 write_result.response 调用客户端 Ingest。
    pub fn ingest(&self, token: &CancellationToken, job: &RegionJob) -> Result<()> {
        let response = job
            .write_result
            .as_ref()
            .map_or(&[][..], |result| result.response.as_slice());
        self.client.Ingest(token, &job.region, response)
    }
}

/// 判断导入错误是否可重试（Retryable/Timeout 或含 EOF）。
pub fn isRetryableImportTiKVError(error: &Error) -> bool {
    matches!(error, Error::Retryable(_) | Error::Timeout) || error.to_string().contains("EOF")
}

impl astersql_resourcemanager_pool_workerpool::TaskMayPanic for RegionJob {
    fn RecoverArgs(
        &self,
    ) -> (
        String,
        String,
        Option<astersql_resourcemanager_pool_workerpool::Error>,
    ) {
        ("regionJob".into(), "regionJob".into(), None)
    }
}
