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

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/snap_client/import.rs`对应的Snap SST 下载/ingest 与令牌背压，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/snap_client/import.go` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 实现文件注释强调 Restorer/Importer 的投递、背压、checkpoint 与错误收束顺序。
//! 本任务要求至少86行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `KvMode`：TiDBFull/Raw/Txn/TiDBCompacted：决定 key 编码与 SST meta 构造。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `RewriteMode`：Legacy vs Keyspace：改写规则是否含 keyspace 前缀。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `gRPCTimeOut`：导入 RPC 超时（200 分钟），对齐长时间 download/ingest。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `DownloadRateLimitTTLSeconds`：限速配置 TTL，过期后需重新下发。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `storeTokenChannelMap`：每 store 令牌通道；耗尽时 ShouldBlock 为真触发背压。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `acquireTokenCh`：惰性创建 store 令牌池，buffer_size=0 表示不限流。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `newStoreTokenChannelMap`：按 store 列表预建令牌通道。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SnapFileImporterOptions`：构造 importer 所需 PD/split/import client 与回调。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `NewSnapFileImporterOptions`：校验并发等参数；非法配置应在 New 时失败。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SnapFileImporter`：实现 FileImporter/Balanced：download→ingest，带 PD scan 令牌。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `NewSnapFileImporter`：按 KvMode 初始化；Raw 需后续 SetRawRange。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `PauseForBackpressure/ShouldBlock`：多表 Restorer 投递前调用，令牌耗尽则阻塞。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetDownloadSpeedLimit`：向各 store 下发下载限速。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `CheckBatchDownloadSupport/CheckBatchDownloadLatestMVCCSupport/CheckMultiIngestSupport`：能力探测，决定走批量或回退路径。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetRawRange`：Raw 模式键范围；未设置时 Raw Import 应失败。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `getKeyRangeForFiles/getKeyRangeByMode`：按模式计算文件覆盖的起止键，供 split/scan。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `Import`：核心：分页扫 region、申请令牌、下载并 ingest；失败释放令牌。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `getSSTMetaFromFile`：从 backup File 构造 import_sstpb::SSTMeta（UUID、CF、range）。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `paginateScanRegion`：按 scanConcurrency 分页扫 region，受 PD 令牌限制。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `acquirePDReqToken/releasePDReqToken`：限制并发 PD ScanRegions，防止打爆 PD。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `AddBeforeIngestCallback`：ingest 前钩子（如校验/统计），可返回 defer 清理。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 中文注释索引结束

//! SST download/ingest importer matching `import.go`.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::stubs::{
    BackupFileSet, BuildWorkerTokenChannel, Context, DefaultCFName, Error, GetRewriteRawKeys,
    ImporterClient, RegionInfo, Result, RewriteRules, SplitClient, TokenCh, WriteCFName,
    acquire_token, backuppb, berrors, codec, import_sstpb, log, metapb, new_uuid_bytes,
    release_token, summary, token_len, try_acquire_token,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
// 与 Go 常量数值对齐，序列化/日志依赖判别值稳定。
pub enum KvMode {
    TiDBFull = 0,
    Raw = 1,
    Txn = 2,
    TiDBCompacted = 3,
}

pub const gRPCTimeOut: Duration = Duration::from_secs(200 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum RewriteMode {
    RewriteModeLegacy = 0,
    RewriteModeKeyspace = 1,
}

// 与 Go 一致 3600s；超时后限速可能被 TiKV 丢弃。
pub const DownloadRateLimitTTLSeconds: u64 = 3600;

// 双检锁创建通道；ShouldBlock 要求所有已建池令牌皆空才阻塞。
pub struct storeTokenChannelMap {
    tokens: Mutex<HashMap<u64, TokenCh>>,
}

impl storeTokenChannelMap {
    pub fn acquireTokenCh(&self, store_id: u64, buffer_size: u32) -> TokenCh {
        {
            let guard = self.tokens.lock().unwrap();
            if let Some(ch) = guard.get(&store_id) {
                return ch.clone();
            }
        }
        let mut guard = self.tokens.lock().unwrap();
        guard
            .entry(store_id)
            .or_insert_with(|| BuildWorkerTokenChannel(buffer_size as usize))
            .clone()
    }

    pub fn ShouldBlock(&self) -> bool {
        let guard = self.tokens.lock().unwrap();
        if guard.is_empty() {
            return false;
        }
        for pool in guard.values() {
            if token_len(pool) > 0 {
                return false;
            }
        }
        true
    }
}

pub fn newStoreTokenChannelMap(stores: &[metapb::Store], buffer_size: u32) -> storeTokenChannelMap {
    let map = storeTokenChannelMap {
        tokens: Mutex::new(HashMap::new()),
    };
    if buffer_size == 0 {
        return map;
    }
    {
        let mut guard = map.tokens.lock().unwrap();
        for store in stores {
            guard.insert(store.Id, BuildWorkerTokenChannel(buffer_size as usize));
        }
    }
    map
}

pub type BeforeIngestCallback = Box<
    dyn Fn(&Context, &[BackupFileSet]) -> Result<Option<Box<dyn FnOnce() -> Result<()>>>>
        + Send
        + Sync,
>;

// BalancedFileImporter：Import 持令牌，PauseForBackpressure 观察令牌水位。
pub struct SnapFileImporter {
    pub taskId: String,
    pub cipher: Option<backuppb::CipherInfo>,
    pub apiVersion: i32,
    pub metaClient: Arc<dyn SplitClient>,
    pub importClient: Arc<dyn ImporterClient>,
    pub backend: Option<backuppb::StorageBackend>,
    downloadTokensMap: storeTokenChannelMap,
    ingestTokensMap: storeTokenChannelMap,
    closeCallbacks: Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>>,
    beforeIngestCallbacks: Vec<BeforeIngestCallback>,
    concurrencyPerStore: u32,
    pdReqTokens: Option<TokenCh>,
    pub kvMode: KvMode,
    pub rawStartKey: Vec<u8>,
    pub rawEndKey: Vec<u8>,
    pub rewriteMode: RewriteMode,
    pub cacheKey: String,
    cond: Arc<(Mutex<()>, Condvar)>,
    pub mergeSst: bool,
    pub retainLatestMVCCVersion: bool,
    peerDownloadRetry: bool,
}

pub struct SnapFileImporterOptions {
    pub cipher: Option<backuppb::CipherInfo>,
    pub metaClient: Arc<dyn SplitClient>,
    pub importClient: Arc<dyn ImporterClient>,
    pub backend: Option<backuppb::StorageBackend>,
    pub rewriteMode: RewriteMode,
    pub tikvStores: Vec<metapb::Store>,
    pub scanConcurrency: u32,
    pub concurrencyPerStore: u32,
    pub retainLatestMVCCVersion: bool,
    pub createCallbacks: Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>>,
    pub closeCallbacks: Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>>,
}

pub fn NewSnapFileImporterOptions(
    cipher: Option<backuppb::CipherInfo>,
    meta_client: Arc<dyn SplitClient>,
    import_client: Arc<dyn ImporterClient>,
    backend: Option<backuppb::StorageBackend>,
    rewrite_mode: RewriteMode,
    tikv_stores: Vec<metapb::Store>,
    concurrency_per_store: u32,
    scan_concurrency: u32,
    retain_latest_mvcc_version: bool,
    create_callbacks: Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>>,
    close_callbacks: Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>>,
) -> SnapFileImporterOptions {
    SnapFileImporterOptions {
        cipher,
        metaClient: meta_client,
        importClient: import_client,
        backend,
        rewriteMode: rewrite_mode,
        tikvStores: tikv_stores,
        scanConcurrency: scan_concurrency,
        concurrencyPerStore: concurrency_per_store,
        retainLatestMVCCVersion: retain_latest_mvcc_version,
        createCallbacks: create_callbacks,
        closeCallbacks: close_callbacks,
    }
}

pub fn NewSnapFileImporter(
    _ctx: &Context,
    api_version: i32,
    kv_mode: KvMode,
    options: SnapFileImporterOptions,
) -> Result<SnapFileImporter> {
    if options.concurrencyPerStore == 0 {
        return Err(Error::new("concurrencyPerStore must be greater than 0"));
    }
    let pd_req_tokens = if options.scanConcurrency > 0 {
        Some(BuildWorkerTokenChannel(options.scanConcurrency as usize))
    } else {
        None
    };
    let mut file_importer = SnapFileImporter {
        taskId: format!(
            "task-{}",
            new_uuid_bytes()[8..]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
        apiVersion: api_version,
        kvMode: kv_mode,
        cipher: options.cipher,
        metaClient: options.metaClient,
        backend: options.backend,
        importClient: options.importClient,
        downloadTokensMap: newStoreTokenChannelMap(
            &options.tikvStores,
            options.concurrencyPerStore,
        ),
        ingestTokensMap: newStoreTokenChannelMap(&options.tikvStores, options.concurrencyPerStore),
        rewriteMode: options.rewriteMode,
        cacheKey: format!(
            "BR-{}",
            new_uuid_bytes()[12..]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
        concurrencyPerStore: options.concurrencyPerStore,
        pdReqTokens: pd_req_tokens,
        cond: Arc::new((Mutex::new(()), Condvar::new())),
        closeCallbacks: options.closeCallbacks,
        beforeIngestCallbacks: Vec::new(),
        rawStartKey: Vec::new(),
        rawEndKey: Vec::new(),
        mergeSst: false,
        retainLatestMVCCVersion: options.retainLatestMVCCVersion,
        peerDownloadRetry: false,
    };
    for f in options.createCallbacks {
        f(&mut file_importer)?;
    }
    Ok(file_importer)
}

impl SnapFileImporter {
    pub fn GetMergeSst(&self) -> bool {
        self.mergeSst
    }

    pub fn SetMergeSst(&mut self, v: bool) {
        self.mergeSst = v;
    }

    pub fn AddBeforeIngestCallback(&mut self, cb: BeforeIngestCallback) {
        self.beforeIngestCallbacks.push(cb);
    }

    pub fn AddCloseCallback(
        &mut self,
        cb: Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>,
    ) {
        self.closeCallbacks.push(cb);
    }

    // 忙等/条件变量等待，直到任一 store 有空闲令牌。
    pub fn PauseForBackpressure(&self) {
        let (lock, cvar) = &*self.cond;
        let mut guard = lock.lock().unwrap();
        while self.ShouldBlock() {
            guard = cvar.wait(guard).unwrap();
        }
    }

    pub fn ShouldBlock(&self) -> bool {
        self.downloadTokensMap.ShouldBlock() || self.ingestTokensMap.ShouldBlock()
    }

    pub fn releaseToken(&self, token_ch: &TokenCh) {
        release_token(token_ch);
        self.cond.1.notify_all();
    }

    pub fn Close(&mut self) -> Result<()> {
        // Go keeps the callbacks registered and treats their failures as best-effort cleanup:
        // every callback still runs and the gRPC close result remains the returned error.
        let callbacks = std::mem::take(&mut self.closeCallbacks);
        for cb in &callbacks {
            if cb(self).is_err() {
                log::Warn("failed on close snap importer");
            }
        }
        self.closeCallbacks = callbacks;
        self.importClient.CloseGrpcClient()
    }

    // PD 扫描令牌与 store 下载令牌分离，避免互相饿死。
    pub fn acquirePDReqToken(&self, ctx: &Context) -> Result<()> {
        if let Some(ch) = &self.pdReqTokens {
            if ctx.Done() {
                return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
            }
            // Non-blocking prefer; fall back to blocking acquire.
            if !try_acquire_token(ch) {
                acquire_token(ch);
            }
        }
        Ok(())
    }

    pub fn releasePDReqToken(&self) {
        if let Some(ch) = &self.pdReqTokens {
            release_token(ch);
        }
    }

    pub fn paginateScanRegion(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
    ) -> Result<Vec<RegionInfo>> {
        self.acquirePDReqToken(ctx)?;
        let result = self.metaClient.PaginateScanRegion(ctx, start_key, end_key);
        self.releasePDReqToken();
        result
    }

    pub fn SetDownloadSpeedLimit(
        &self,
        ctx: &Context,
        store_id: u64,
        rate_limit: u64,
    ) -> Result<()> {
        let req = import_sstpb::SetDownloadSpeedLimitRequest {
            TaskId: self.taskId.clone(),
            SpeedLimit: rate_limit,
            TtlSeconds: DownloadRateLimitTTLSeconds,
        };
        self.importClient.SetDownloadSpeedLimit(ctx, store_id, &req)
    }

    pub fn CheckBatchDownloadSupport(
        &mut self,
        ctx: &Context,
        tikv_stores: &[metapb::Store],
    ) -> Result<()> {
        let ids: Vec<u64> = tikv_stores
            .iter()
            .filter(|store| store.State == metapb::StoreState::Up)
            .map(|store| store.Id)
            .collect();
        self.mergeSst = self.importClient.CheckBatchDownloadSupport(ctx, &ids)?;
        Ok(())
    }

    pub fn CheckBatchDownloadLatestMVCCSupport(
        &mut self,
        ctx: &Context,
        tikv_stores: &[metapb::Store],
    ) -> Result<()> {
        let ids: Vec<u64> = tikv_stores
            .iter()
            .filter(|store| store.State == metapb::StoreState::Up)
            .map(|store| store.Id)
            .collect();
        self.importClient
            .CheckBatchDownloadLatestMVCCSupport(ctx, &ids)?;
        self.peerDownloadRetry = true;
        Ok(())
    }

    pub fn CheckPeerDownloadRetrySupport(
        &mut self,
        ctx: &Context,
        stores: &[metapb::Store],
    ) -> Result<()> {
        let ids: Vec<_> = stores
            .iter()
            .filter(|s| s.State == metapb::StoreState::Up)
            .map(|s| s.Id)
            .collect();
        self.peerDownloadRetry = match self
            .importClient
            .IsBatchDownloadLatestMVCCSupported(ctx, &ids)
        {
            Ok(supported) => supported,
            Err(_) => {
                log::Warn(
                    "failed to check peer download retry support, fallback to legacy download retry",
                );
                false
            }
        };
        Ok(())
    }

    fn downloadWithOptionalPeerRetry(
        &self,
        ctx: &Context,
        mut rpc: impl FnMut(&Context) -> Result<import_sstpb::DownloadResponse>,
    ) -> Result<import_sstpb::DownloadResponse> {
        use astersql_br_pkg_utils::backoff::{
            NewDownloadSSTBackoffStrategy, NewPeerDownloadSSTBackoffStrategy,
        };
        let mut backoff = if self.peerDownloadRetry {
            NewPeerDownloadSSTBackoffStrategy()
        } else {
            NewDownloadSSTBackoffStrategy()
        };
        let mut errors = Vec::new();
        loop {
            if let Some(error) = ctx.Err() {
                return Err(error);
            }
            let request_ctx = ctx.WithTimeout(std::time::Duration::from_secs(200 * 60));
            let result = rpc(&request_ctx);
            request_ctx.cancel(Error::with_code("context.Canceled", "context canceled"));
            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let normalized = match error.code {
                        Some("BR:KV:ErrKVEpochNotMatch") => {
                            Some(&*astersql_br_pkg_errors::ErrKVEpochNotMatch)
                        }
                        Some("BR:KV:ErrKVDownloadFailed") => {
                            Some(&*astersql_br_pkg_errors::ErrKVDownloadFailed)
                        }
                        Some("BR:KV:ErrKVIngestFailed") => {
                            Some(&*astersql_br_pkg_errors::ErrKVIngestFailed)
                        }
                        Some("BR:PD:ErrPDLeaderNotFound") => {
                            Some(&*astersql_br_pkg_errors::ErrPDLeaderNotFound)
                        }
                        Some("BR:KV:ErrKVRangeIsEmpty") => {
                            Some(&*astersql_br_pkg_errors::ErrKVRangeIsEmpty)
                        }
                        Some("BR:KV:ErrKVRewriteRuleNotFound") => {
                            Some(&*astersql_br_pkg_errors::ErrKVRewriteRuleNotFound)
                        }
                        _ => None,
                    };
                    let classified = if error.code == Some("context.Canceled")
                        || (error.code.is_none() && error.msg == "context canceled")
                    {
                        astersql_errors::SharedError::new(astersql_br_pkg_errors::Canceled)
                    } else if let Some(normalized) = normalized {
                        astersql_errors::Annotate(
                            Some(astersql_errors::SharedError::new(normalized.clone())),
                            &error.msg,
                        )
                        .unwrap()
                    } else if let Some(code) = error.code {
                        astersql_errors::SharedError::new(DownloadRPCStatus {
                            code,
                            message: error.msg.clone(),
                        })
                    } else {
                        astersql_errors::SharedError::new(error.clone())
                    };
                    errors.push(error.clone());
                    let delay = backoff.NextBackoff(&classified);
                    if backoff.RemainingAttempts() == 0 {
                        return Err(Error {
                            code: error.code,
                            msg: errors
                                .iter()
                                .map(|e| e.msg.as_str())
                                .collect::<Vec<_>>()
                                .join("; "),
                        });
                    }
                    let until = std::time::Instant::now() + delay;
                    while std::time::Instant::now() < until {
                        if let Some(error) = ctx.Err() {
                            return Err(error);
                        }
                        std::thread::sleep(
                            until
                                .saturating_duration_since(std::time::Instant::now())
                                .min(std::time::Duration::from_millis(10)),
                        );
                    }
                }
            }
        }
    }

    // 任一 store 不支持则整集群回退单文件 ingest。
    pub fn CheckMultiIngestSupport(
        &self,
        ctx: &Context,
        tikv_stores: &[metapb::Store],
    ) -> Result<()> {
        let ids: Vec<u64> = tikv_stores
            .iter()
            .filter(|store| store.State == metapb::StoreState::Up)
            .map(|store| store.Id)
            .collect();
        self.importClient.CheckMultiIngestSupport(ctx, &ids)
    }

    // Raw/Txn 模式必须设置；TiDBFull 忽略该字段。
    pub fn SetRawRange(&mut self, start_key: Vec<u8>, end_key: Vec<u8>) -> Result<()> {
        if self.kvMode != KvMode::Raw {
            return Err(Error::Annotate(
                berrors::ErrRestoreModeMismatch("mode mismatch"),
                "file importer is not in raw kv mode",
            ));
        }
        self.rawStartKey = start_key;
        self.rawEndKey = end_key;
        Ok(())
    }

    pub fn getKeyRangeForFiles(&self, files_group: &[BackupFileSet]) -> Result<(Vec<u8>, Vec<u8>)> {
        let get_range = getKeyRangeByMode(self.kvMode);
        let mut start_key = Vec::new();
        let mut end_key = Vec::new();
        for files in files_group {
            for f in &files.SSTFiles {
                let (start, end) = get_range(f, files.RewriteRules.as_ref())?;
                if start_key.is_empty() || start.as_slice() < start_key.as_slice() {
                    start_key = start;
                }
                if end_key.is_empty() || end_key.as_slice() < end.as_slice() {
                    end_key = end;
                }
            }
        }
        Ok((start_key, end_key))
    }

    // 按文件组计算 key range → 扫 region → 每 peer 下载/ingest。
    pub fn Import(&mut self, ctx: &Context, backup_file_sets: &[BackupFileSet]) -> Result<()> {
        let mut delay_cbs = Vec::new();
        for (i, cb) in self.beforeIngestCallbacks.iter().enumerate() {
            let d = cb(ctx, backup_file_sets).map_err(|e| {
                Error::Annotatef(e, format!("failed to executing the callback #{i}"))
            })?;
            if let Some(d) = d {
                delay_cbs.push(d);
            }
        }

        let (start_key, end_key) = self.getKeyRangeForFiles(backup_file_sets)?;
        let region_infos = self.paginateScanRegion(ctx, &start_key, &end_key)?;
        for region_info in &region_infos {
            let metas = self.download(ctx, region_info, backup_file_sets)?;
            self.ingest(ctx, region_info, &metas)?;
        }
        for (i, cb) in delay_cbs.into_iter().enumerate() {
            cb().map_err(|e| {
                Error::Annotatef(e, format!("failed to execute the delaied callback #{i}"))
            })?;
        }
        for files in backup_file_sets {
            for f in &files.SSTFiles {
                summary::CollectSuccessUnit(summary::TotalKV, 1, f.TotalKvs);
                summary::CollectSuccessUnit(summary::TotalBytes, 1, f.TotalBytes);
            }
        }
        Ok(())
    }

    fn buildDownloadRequest(
        &self,
        file: &backuppb::File,
        rules: Option<&RewriteRules>,
        region_info: &RegionInfo,
    ) -> Result<Option<(import_sstpb::DownloadRequest, import_sstpb::SSTMeta)>> {
        let Some(rule) = rules.and_then(|rules| {
            rules
                .Data
                .iter()
                .filter(|rule| file.StartKey.starts_with(&rule.OldKeyPrefix))
                .max_by_key(|rule| rule.OldKeyPrefix.len())
        }) else {
            return Ok(None);
        };
        let rewrite = |key: &[u8]| {
            let raw = [
                rule.NewKeyPrefix.as_slice(),
                &key[rule.OldKeyPrefix.len()..],
            ]
            .concat();
            codec::EncodeBytes(Vec::new(), &raw)
        };
        let encoded_start = rewrite(&file.StartKey);
        let encoded_end = rewrite(&file.EndKey);
        if (!region_info.Region.EndKey.is_empty()
            && encoded_start.as_slice() >= region_info.Region.EndKey.as_slice())
            || encoded_end.as_slice() <= region_info.Region.StartKey.as_slice()
        {
            return Ok(None);
        }
        let mut region_rule = rule.clone();
        if self.rewriteMode == RewriteMode::RewriteModeLegacy {
            region_rule.OldKeyPrefix = codec::EncodeBytes(Vec::new(), &region_rule.OldKeyPrefix);
            region_rule.NewKeyPrefix = codec::EncodeBytes(Vec::new(), &region_rule.NewKeyPrefix);
        }
        let meta = getSSTMetaFromFile(file, &region_info.Region, &region_rule, self.rewriteMode)?;
        Ok(Some((
            import_sstpb::DownloadRequest {
                Sst: meta.clone(),
                StorageBackend: self.backend.clone(),
                Name: file.Name.clone(),
                RewriteRule: region_rule,
                CipherInfo: self.cipher.clone(),
                StorageCacheId: self.cacheKey.clone(),
                ..Default::default()
            },
            meta,
        )))
    }

    fn download(
        &self,
        ctx: &Context,
        region_info: &RegionInfo,
        file_sets: &[BackupFileSet],
    ) -> Result<Vec<import_sstpb::SSTMeta>> {
        let mut metas = Vec::new();
        for files in file_sets {
            let mut requests = Vec::new();
            let mut has_write_cf = false;
            for file in &files.SSTFiles {
                if let Some(request) =
                    self.buildDownloadRequest(file, files.RewriteRules.as_ref(), region_info)?
                {
                    has_write_cf |= file.Cf.contains("write");
                    requests.push(request);
                }
            }
            // Write CF determines MVCC visibility; keep default CF only in groups
            // that also contain a write SST overlapping this region.
            if self.retainLatestMVCCVersion && !has_write_cf {
                continue;
            }
            for (req, meta) in requests {
                for peer in &region_info.Region.Peers {
                    if ctx.Done() {
                        return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
                    }
                    let token = self
                        .downloadTokensMap
                        .acquireTokenCh(peer.StoreId, self.concurrencyPerStore);
                    acquire_token(&token);
                    let result = self.downloadWithOptionalPeerRetry(ctx, |ctx| {
                        if self.retainLatestMVCCVersion {
                            self.importClient
                                .BatchDownloadLatestMVCC(ctx, peer.StoreId, &req)
                        } else if self.mergeSst {
                            self.importClient.BatchDownloadSST(ctx, peer.StoreId, &req)
                        } else {
                            self.importClient.DownloadSST(ctx, peer.StoreId, &req)
                        }
                    });
                    self.releaseToken(&token);
                    let response = result?;
                    if let Some(err) = response.Error {
                        return Err(Error::new(err.Message));
                    }
                }
                metas.push(meta);
            }
        }
        Ok(metas)
    }

    fn ingest(
        &self,
        ctx: &Context,
        region_info: &RegionInfo,
        metas: &[import_sstpb::SSTMeta],
    ) -> Result<()> {
        if metas.is_empty() {
            return Ok(());
        }
        let leader = region_info
            .Leader
            .as_ref()
            .ok_or_else(|| Error::new("region has no leader"))?;
        let token = self
            .ingestTokensMap
            .acquireTokenCh(leader.StoreId, self.concurrencyPerStore);
        acquire_token(&token);
        let response = self.importClient.MultiIngest(
            ctx,
            leader.StoreId,
            &import_sstpb::MultiIngestRequest {
                Context: Some(import_sstpb::KvContext {
                    RegionId: region_info.Region.Id,
                    RegionEpoch: region_info.Region.RegionEpoch.clone(),
                    Peer: Some(leader.clone()),
                }),
                Ssts: metas.to_vec(),
            },
        );
        self.releaseToken(&token);
        if let Some(err) = response?.Error {
            return Err(Error::new(err.Message));
        }
        Ok(())
    }
}

pub type KeyRangeFn = fn(&backuppb::File, Option<&RewriteRules>) -> Result<(Vec<u8>, Vec<u8>)>;

pub fn getKeyRangeByMode(mode: KvMode) -> KeyRangeFn {
    match mode {
        KvMode::Raw => |f: &backuppb::File, _rules: Option<&RewriteRules>| {
            Ok((f.StartKey.clone(), f.EndKey.clone()))
        },
        KvMode::Txn => |f: &backuppb::File, _rules: Option<&RewriteRules>| {
            let start = if f.StartKey.is_empty() {
                Vec::new()
            } else {
                codec::EncodeBytes(Vec::new(), &f.StartKey)
            };
            let end = if f.EndKey.is_empty() {
                Vec::new()
            } else {
                codec::EncodeBytes(Vec::new(), &f.EndKey)
            };
            Ok((start, end))
        },
        _ => |f: &backuppb::File, rules: Option<&RewriteRules>| GetRewriteRawKeys(f, rules),
    }
}

pub fn GetKeyRangeByMode(mode: KvMode) -> KeyRangeFn {
    getKeyRangeByMode(mode)
}

pub fn getSSTMetaFromFile(
    file: &backuppb::File,
    region: &metapb::Region,
    region_rule: &import_sstpb::RewriteRule,
    rewrite_mode: RewriteMode,
) -> Result<import_sstpb::SSTMeta> {
    let mut r = region.clone();
    if rewrite_mode == RewriteMode::RewriteModeKeyspace {
        if !region.StartKey.is_empty() {
            let (_rest, decoded) =
                codec::DecodeBytes(&region.StartKey, None).map_err(Error::new)?;
            r.StartKey = decoded;
        }
        if !region.EndKey.is_empty() {
            let (_rest, decoded) = codec::DecodeBytes(&region.EndKey, None).map_err(Error::new)?;
            r.EndKey = decoded;
        }
    }

    let mut cf_name = file.Cf.clone();
    if file.Name.contains(DefaultCFName) {
        cf_name = DefaultCFName.to_string();
    } else if file.Name.contains(WriteCFName) {
        cf_name = WriteCFName.to_string();
    }

    let mut range_start = region_rule.NewKeyPrefix.clone();
    if range_start.as_slice() < r.StartKey.as_slice() {
        range_start = r.StartKey.clone();
    }

    let suffix = vec![0xffu8; 10];
    let mut range_end = [region_rule.NewKeyPrefix.clone(), suffix].concat();
    if !r.EndKey.is_empty() && range_end.as_slice() > r.EndKey.as_slice() {
        range_end = r.EndKey.clone();
    }

    if range_start.as_slice() > range_end.as_slice() {
        log::Panic("range start exceed range end");
    }

    Ok(import_sstpb::SSTMeta {
        Uuid: new_uuid_bytes(),
        CfName: cf_name,
        Range: Some(import_sstpb::Range {
            Start: range_start,
            End: range_end,
        }),
        Length: file.GetSize_(),
        RegionId: region.GetId(),
        RegionEpoch: region.GetRegionEpoch(),
        CipherIv: file.CipherIv.clone(),
    })
}

pub fn GetSSTMetaFromFile(
    file: &backuppb::File,
    region: &metapb::Region,
    region_rule: &import_sstpb::RewriteRule,
    rewrite_mode: RewriteMode,
) -> Result<import_sstpb::SSTMeta> {
    getSSTMetaFromFile(file, region, region_rule, rewrite_mode)
}

#[derive(Debug)]
struct DownloadRPCStatus {
    code: &'static str,
    message: String,
}
impl std::fmt::Display for DownloadRPCStatus {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            fmt,
            "rpc error: code = {} desc = {}",
            self.code, self.message
        )
    }
}
impl std::error::Error for DownloadRPCStatus {}

/// Adapts the existing snapshot importer to the shared SST restorer boundary.
/// The mutex follows the importer's existing mutable Import/Close contract.
pub struct SnapshotFileImporter(pub std::sync::Mutex<SnapFileImporter>);

fn snapshot_context(ctx: &astersql_br_pkg_restore::stubs::Context) -> Context {
    let parent = ctx.clone();
    Context::WithCancellationSource(move || {
        parent.Err().map(|e| Error {
            msg: e.msg,
            code: e.code,
        })
    })
}

impl astersql_br_pkg_restore::FileImporter for SnapshotFileImporter {
    fn ConfigureDownloadRetry(
        &self,
        ctx: &astersql_br_pkg_restore::stubs::Context,
        stores: &[u64],
    ) -> astersql_br_pkg_restore::stubs::Result<()> {
        let ctx = snapshot_context(ctx);
        let stores: Vec<_> = stores
            .iter()
            .map(|&Id| metapb::Store {
                Id,
                State: metapb::StoreState::Up,
                ..Default::default()
            })
            .collect();
        let mut importer = self.0.lock().unwrap();
        let result = if importer.retainLatestMVCCVersion {
            importer.CheckBatchDownloadLatestMVCCSupport(&ctx, &stores)
        } else {
            importer.CheckPeerDownloadRetrySupport(&ctx, &stores)
        };
        result.map_err(|e| astersql_br_pkg_restore::stubs::Error {
            msg: e.msg,
            code: e.code,
        })
    }

    fn Import(
        &self,
        ctx: &astersql_br_pkg_restore::stubs::Context,
        sets: &[astersql_br_pkg_restore::BackupFileSet],
    ) -> astersql_br_pkg_restore::stubs::Result<()> {
        let sets: Vec<_> = sets
            .iter()
            .map(|set| BackupFileSet {
                TableID: set.TableID,
                SSTFiles: set
                    .SSTFiles
                    .iter()
                    .map(|file| backuppb::File {
                        Name: file.Name.clone(),
                        StartKey: file.StartKey.clone(),
                        EndKey: file.EndKey.clone(),
                        TotalBytes: file.TotalBytes,
                        Size_: file.Size_,
                        TotalKvs: file.TotalKvs,
                        Cf: file.Cf.clone(),
                        Crc64Xor: file.Crc64Xor,
                        ..Default::default()
                    })
                    .collect(),
                RewriteRules: set.RewriteRules.as_ref().map(|rules| RewriteRules {
                    Data: rules
                        .Data
                        .iter()
                        .map(|rule| import_sstpb::RewriteRule {
                            OldKeyPrefix: rule.OldKeyPrefix.clone(),
                            NewKeyPrefix: rule.NewKeyPrefix.clone(),
                            NewTimestamp: rule.NewTimestamp,
                            IgnoreAfterTimestamp: rule.IgnoreAfterTimestamp,
                            IgnoreBeforeTimestamp: rule.IgnoreBeforeTimestamp,
                        })
                        .collect(),
                    NewTableID: rules.NewTableID,
                    NewKeyspace: rules.NewKeyspace.clone(),
                    TableIDRemapHint: rules
                        .TableIDRemapHint
                        .iter()
                        .map(|hint| crate::stubs::TableIDRemap {
                            Origin: hint.Origin,
                            Rewritten: hint.Rewritten,
                        })
                        .collect(),
                }),
            })
            .collect();
        self.0
            .lock()
            .unwrap()
            .Import(&snapshot_context(ctx), &sets)
            .map_err(|e| astersql_br_pkg_restore::stubs::Error {
                msg: e.msg,
                code: e.code,
            })
    }

    fn Close(&self) -> astersql_br_pkg_restore::stubs::Result<()> {
        self.0
            .lock()
            .unwrap()
            .Close()
            .map_err(|e| astersql_br_pkg_restore::stubs::Error {
                msg: e.msg,
                code: e.code,
            })
    }
}

impl astersql_br_pkg_restore::BalancedFileImporter for SnapshotFileImporter {
    fn PauseForBackpressure(&self) {
        self.0.lock().unwrap().PauseForBackpressure();
    }
}
