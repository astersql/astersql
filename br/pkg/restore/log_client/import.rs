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

//! Log file importer, matching `import.go`.
//!
//! 本文件实现日志还原的 KV 文件导入器，对齐 Go `import.go`。
//! `LogFileImporter` 持有 PD/TiKV meta 客户端、ImportSST 客户端与外部存储后端。
//! 主路径 `ImportKVFiles`：先求文件改写后的全局键范围，再经 RangeController 按 region 投递。
//! 每个 region 内过滤相交文件，组装 ApplyRequest（批量或单文件）发往 leader store。
//! `ClearFiles` 按前缀清理各 Up store 上的临时 KV 缓存；失败仅告警不中断。
//! ClearFiles 忽略单店失败是有意为之：清理是尽力而为，不应阻断主流程。
//! ImportKVFiles 把“求全局范围 + 按 region 过滤 + Apply”串成一条流水线。
//! RangeController 负责拓扑变化重试；本文件只组装请求与过滤文件。
//! supportBatch 反映 TiKV 能力：旧版本只能单文件 Apply。
//! DefaultCF 使用 shiftStartTS，是为了与日志备份的时间窗口位移对齐。
//! cacheKey 隔离不同导入会话在 TiKV 侧的临时对象命名空间。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_br_pkg_restore_utils::{
    EncodeKeyPrefix, FindMatchedRewriteRule, GetRewriteEncodedKeys, RewriteRules,
};

use crate::import_retry::{
    CreateRangeController, RPCResult, RPCResultFromError, RPCResultFromPBError, RPCResultOK,
    RangeCtlMetricListener, RegionFunc,
};
use crate::log_file_manager::LogDataFileInfo;
use crate::stubs::backuppb::{self, CipherInfo, FileType, StorageBackend};
use crate::stubs::berrors;
use crate::stubs::conn;
use crate::stubs::consts;
use crate::stubs::encryptionpb;
use crate::stubs::import_sstpb;
use crate::stubs::importclient::ImporterClient;
use crate::stubs::kv::KeyRange;
use crate::stubs::kvrpcpb;
use crate::stubs::log;
use crate::stubs::logutil;
use crate::stubs::metapb;
use crate::stubs::metrics;
use crate::stubs::pd;
use crate::stubs::split_client::{RegionInfo, SplitClient};
use crate::stubs::summary;
use crate::stubs::utils_retry;
use crate::stubs::{Context, Error, Result};

/// 日志 KV 导入器：封装跨 region 的 ApplyKVFile 调度与清理。
pub struct LogFileImporter {
    // 扫描 region 拓扑、找 leader。
    pub metaClient: Arc<dyn SplitClient>,
    // 向 TiKV importer 发 Clear/Apply RPC。
    pub importClient: Arc<dyn ImporterClient>,
    // 外部存储位置；Apply 时告诉 TiKV 从何处拉文件。
    pub backend: Option<StorageBackend>,
    // 本导入器会话的缓存键前缀，隔离并发任务的临时文件。
    pub cacheKey: String,
}

/// 构造导入器；`cacheKey` 用时间戳生成，格式与 Go `BR-<sec>-...` 一致。
pub fn NewLogFileImporter(
    metaClient: Arc<dyn SplitClient>,
    importClient: Arc<dyn ImporterClient>,
    backend: Option<StorageBackend>,
) -> LogFileImporter {
    // 用 unix 秒构造伪随机后缀，降低并发任务 cacheKey 碰撞概率。
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    LogFileImporter {
        metaClient,
        backend,
        importClient,
        cacheKey: format!("BR-{now}-{}", now.wrapping_mul(31)),
    }
}

impl LogFileImporter {
    /// 关闭底层 gRPC 客户端，释放连接。
    pub fn Close(&self) -> Result<()> {
        // 释放 importer 侧连接池；上层任务结束时调用。
        self.importClient.CloseGrpcClient()
    }

    /// 在所有 Up 状态的 TiKV 上按前缀清理 importer 临时文件；单店失败只 Warn。
    pub fn ClearFiles(&self, ctx: &Context, pdClient: &dyn pd::Client, prefix: &str) -> Result<()> {
        // SkipTiFlash：清理只针对真正持有 importer 临时文件的 TiKV。
        let allStores = conn::GetAllTiKVStoresWithRetry(ctx, pdClient, conn::util::SkipTiFlash)?;
        for s in allStores {
            // TiFlash 与非 Up store 不参与 importer 清理。
            if s.State != metapb::StoreState::Up {
                continue;
            }
            let req = import_sstpb::ClearRequest {
                Prefix: prefix.to_string(),
            };
            // 单店失败不中断循环，尽量清完其余节点。
            if let Err(err) = self.importClient.ClearFiles(ctx, s.GetId(), &req) {
                log::Warn(&format!(
                    "cleanup kv files failed store={} err={err}",
                    s.GetId()
                ));
            }
        }
        Ok(())
    }

    /// 将一批日志文件按改写规则 apply 到目标集群。
    /// 非批量模式限制 `files.len()==1`；批量模式由 RangeController 按 region 拆分投递。
    pub fn ImportKVFiles(
        &self,
        ctx: &Context,
        files: &[LogDataFileInfo],
        rule: &RewriteRules,
        shiftStartTS: u64,
        startTS: u64,
        restoreTS: u64,
        supportBatch: bool,
        cipherInfo: Option<&CipherInfo>,
        masterKeys: &[encryptionpb::MasterKey],
    ) -> Result<()> {
        // 旧 TiKV 不支持批量 Apply 时，多文件必须由上层拆批。
        // 这里提前失败，避免静默只 apply 第一个文件。
        if !supportBatch && files.len() > 1 {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument("batch not supported"),
                format!(
                    "do not support batch apply, file count: {} > 1",
                    files.len()
                ),
            ));
        }
        log::Debug(&format!("import kv files batch={}", files.len()));

        // 合并所有文件改写后的键范围，作为 RangeController 扫描区间。
        // ranges 与 files 下标对齐，供 filterFilesByRegion 做交集。
        let mut startKey = Vec::new();
        let mut endKey = Vec::new();
        let mut ranges = Vec::with_capacity(files.len());
        for f in files {
            // 编码后的起止键才是 TiKV region 空间中的真实位置。
            let (s, e) = GetRewriteEncodedKeys(f, Some(rule))?;
            let s = s.unwrap_or_default();
            let e = e.unwrap_or_default();
            // 维护全局最小 start / 最大 end。
            if startKey.is_empty() || s < startKey {
                startKey = s.clone();
            }
            if endKey.is_empty() || e > endKey {
                endKey = e.clone();
            }
            ranges.push(KeyRange {
                StartKey: s,
                EndKey: e,
            });
        }

        // 指标监听器记录 request/retry/success，便于观察 region 级重试。
        // numRegions 统计回调触发次数，用于批处理规模直方图。
        let numRegions = std::sync::Arc::new(AtomicI64::new(0));
        let listener = RangeCtlMetricListener {
            RequestRegion: metrics::KVApplyRunOverRegionsEvents::WithLabelValues("request-region"),
            RetryRegion: metrics::KVApplyRunOverRegionsEvents::WithLabelValues("retry-region"),
            RetryRange: metrics::KVApplyRunOverRegionsEvents::WithLabelValues("retry-range"),
            RegionSuccess: metrics::KVApplyRunOverRegionsEvents::WithLabelValues("region-success"),
        };
        // 与 Go 一致：最多 45 次，初始 100ms，上限 15s 指数退避。
        let rs = utils_retry::InitialRetryState(
            45,
            std::time::Duration::from_millis(100),
            std::time::Duration::from_secs(15),
        );
        let mut ctl = CreateRangeController(startKey, endKey, self.metaClient.clone(), rs);
        // 挂上指标后再 Apply，确保重试路径也被计数。
        ctl.SetEventListener(Box::new(listener));

        // 闭包需要拥有数据副本，因 RegionFunc 要求 'static+Send 语义。
        let files = files.to_vec();
        let rule = rule.Clone();
        let cipher = cipherInfo.cloned();
        let masterKeys = masterKeys.to_vec();
        let importClient = self.importClient.clone();
        let backend = self.backend.clone();
        let cacheKey = self.cacheKey.clone();
        let ranges_owned = ranges;
        let numRegions2 = numRegions.clone();
        // 闭包在每个 region 上过滤相交文件后调用 downloadAndApply。
        let mut region_fn: RegionFunc = Box::new(move |ctx, r| {
            numRegions2.fetch_add(1, Ordering::Relaxed);
            let subfiles = match filterFilesByRegion(&files, &ranges_owned, r) {
                Ok(v) => v,
                Err(err) => return RPCResultFromError(err),
            };
            // 观察每个 region 实际命中的文件数，辅助调优批大小。
            metrics::KV_APPLY_REGION_FILES.Observe(subfiles.len() as f64);
            // 无相交文件视为成功跳过，避免无意义 RPC。
            if subfiles.is_empty() {
                return RPCResultOK();
            }
            importKVFileForRegionOwned(
                ctx,
                importClient.as_ref(),
                backend.as_ref(),
                &cacheKey,
                &subfiles,
                &rule,
                shiftStartTS,
                startTS,
                restoreTS,
                r,
                supportBatch,
                cipher.as_ref(),
                &masterKeys,
            )
        });
        // Apply 返回后记录本批覆盖的 region 数。
        let err = ctl.ApplyFuncToRange(ctx, &mut region_fn);
        metrics::KV_APPLY_BATCH_REGIONS.Observe(numRegions.load(Ordering::Relaxed) as f64);
        // 直接透传 RangeController 的最终错误（含累计 multierr）。
        err
    }
}

/// Apply a region's files and preserve Go's region-level skip semantics.
/// Missing rewrite rules and empty KV ranges mean that this region has no
/// applicable data; they are not fatal to the enclosing range traversal.
fn importKVFileForRegionOwned(
    ctx: &Context,
    importClient: &dyn ImporterClient,
    backend: Option<&StorageBackend>,
    cacheKey: &str,
    files: &[LogDataFileInfo],
    rules: &RewriteRules,
    shiftStartTS: u64,
    startTS: u64,
    restoreTS: u64,
    regionInfo: &RegionInfo,
    supportBatch: bool,
    cipherInfo: Option<&CipherInfo>,
    masterKeys: &[encryptionpb::MasterKey],
) -> RPCResult {
    let result = downloadAndApplyKVFileOwned(
        ctx,
        importClient,
        backend,
        cacheKey,
        files,
        rules,
        shiftStartTS,
        startTS,
        restoreTS,
        regionInfo,
        supportBatch,
        cipherInfo,
        masterKeys,
    );
    if !result.OK() {
        let should_skip = result.Err.as_ref().is_some_and(|err| {
            matches!(
                err.code,
                Some("BR:KV:ErrKVRewriteRuleNotFound" | "BR:KV:ErrKVRangeIsEmpty")
            )
        });
        if should_skip {
            logutil::CL(ctx).Warn("download file skipped");
            return RPCResultOK();
        }
        logutil::CL(ctx).Warn("download and apply file failed");
        return result;
    }
    summary::CollectInt("RegionInvolved", 1);
    RPCResultOK()
}

/// 针对单个 region 组装 ApplyRequest 并调用 importer。
/// DefaultCF 使用 shiftStartTS（对齐日志位移），其它 CF 用 startTS。
fn downloadAndApplyKVFileOwned(
    ctx: &Context,
    importClient: &dyn ImporterClient,
    backend: Option<&StorageBackend>,
    cacheKey: &str,
    files: &[LogDataFileInfo],
    rules: &RewriteRules,
    shiftStartTS: u64,
    startTS: u64,
    restoreTS: u64,
    regionInfo: &RegionInfo,
    supportBatch: bool,
    cipherInfo: Option<&CipherInfo>,
    masterKeys: &[encryptionpb::MasterKey],
) -> RPCResult {
    // 无 leader 时返回可重试的 PD 错误，由 RangeController 再定位。
    // region 元数据缺失则直接本地错误，通常不可靠重试。
    let Some(leader) = &regionInfo.Leader else {
        let id = regionInfo.Region.as_ref().map(|r| r.Id).unwrap_or(0);
        return RPCResultFromError(Error::Annotatef(
            berrors::ErrPDLeaderNotFound("no leader"),
            format!("region id {id} has no leader"),
        ));
    };
    let Some(region) = &regionInfo.Region else {
        return RPCResultFromError(Error::new("region is nil"));
    };

    let mut metas = Vec::with_capacity(files.len());
    let mut rewriteRules = Vec::with_capacity(files.len());
    for file in files {
        // 每个文件必须能匹配到改写规则，否则无法安全 apply。
        // 规则匹配失败说明改写表未覆盖该文件，属于配置/元数据错误。
        let fileRule = FindMatchedRewriteRule(file, rules);
        let Some(fileRule) = fileRule else {
            return RPCResultFromError(Error::Annotatef(
                berrors::ErrKVRewriteRuleNotFound("not found"),
                format!("rewrite rule for file {} not find", file.Path),
            ));
        };
        // 前缀需 Encode，才能与 TiKV 内部键编码一致。
        let rule = import_sstpb::RewriteRule {
            OldKeyPrefix: EncodeKeyPrefix(&fileRule.GetOldKeyPrefix()),
            NewKeyPrefix: EncodeKeyPrefix(&fileRule.GetNewKeyPrefix()),
        };
        let meta = import_sstpb::KVMeta {
            Name: file.Path.clone(),
            Cf: file.Cf.clone(),
            RangeOffset: file.RangeOffset,
            Length: file.Length,
            RangeLength: file.RangeLength,
            IsDelete: file.Type == FileType::Delete,
            // DefaultCF 用移位后的 startTS，写 CF 仍用原始 startTS，与 Go 一致。
            StartTs: if file.Cf == consts::DefaultCF {
                shiftStartTS
            } else {
                startTS
            },
            RestoreTs: restoreTS,
            // 键范围裁到当前 region，避免跨 region 写入。
            StartKey: region.GetStartKey().to_vec(),
            EndKey: region.GetEndKey().to_vec(),
            Sha256: file.GetSha256(),
            CompressionType: file.CompressionType,
            FileEncryptionInfo: file.FileEncryptionInfo.clone(),
        };
        metas.push(meta);
        rewriteRules.push(rule);
    }

    // RPC Context 绑定 epoch 与 peer，供 TiKV 做 epoch 检查。
    let reqCtx = kvrpcpb::Context {
        RegionId: region.GetId(),
        RegionEpoch: region.RegionEpoch.clone(),
        Peer: Some(leader.clone()),
    };

    // 批量：Metas/RewriteRules 数组；非批量：单 Meta/RewriteRule 字段。
    let req = if supportBatch {
        import_sstpb::ApplyRequest {
            Metas: metas,
            StorageBackend: backend.cloned(),
            RewriteRules: rewriteRules,
            Context: Some(reqCtx),
            StorageCacheId: cacheKey.to_string(),
            CipherInfo: cipherInfo.cloned(),
            MasterKeys: masterKeys.to_vec(),
            ..Default::default()
        }
    } else {
        import_sstpb::ApplyRequest {
            Meta: metas.into_iter().next(),
            StorageBackend: backend.cloned(),
            RewriteRule: rewriteRules.into_iter().next().unwrap_or_default(),
            Context: Some(reqCtx),
            StorageCacheId: cacheKey.to_string(),
            CipherInfo: cipherInfo.cloned(),
            MasterKeys: masterKeys.to_vec(),
            ..Default::default()
        }
    };

    log::Debug("applying kv file");
    // 发往 leader 所在 store；传输错误与 PB 业务错误分流包装。
    match importClient.ApplyKVFile(ctx, leader.GetStoreId(), &req) {
        Err(err) => RPCResultFromError(Error::Trace(err)),
        Ok(resp) => {
            if let Some(err) = resp.GetError() {
                logutil::CL(ctx).Warn("import has error");
                // PB 内嵌 store/import 错误交给重试策略解析。
                RPCResultFromPBError(err)
            } else {
                RPCResultOK()
            }
        }
    }
}

/// 按 region 与文件改写后 range 的交集过滤；files/ranges 长度必须一一对应。
/// region 为空时保守地返回全部文件（与 Go 空指针回退一致）。
pub fn filterFilesByRegion(
    files: &[LogDataFileInfo],
    ranges: &[KeyRange],
    r: &RegionInfo,
) -> Result<Vec<LogDataFileInfo>> {
    // 防御性检查：调用方必须保证 files/ranges 一一对应。
    if files.len() != ranges.len() {
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument("count mismatch"),
            format!(
                "count of files no equals count of ranges, file-count:{}, ranges-count:{}",
                files.len(),
                ranges.len()
            ),
        ));
    }
    let mut output = Vec::with_capacity(files.len());
    if let Some(region) = &r.Region {
        for (i, f) in files.iter().enumerate() {
            // 区间相交判定：region.start <= file.end && (region.end 空或 region.end >= file.start)。
            // 空 EndKey 表示 region 延伸到正无穷，与 PD 约定一致。
            if region.StartKey.as_slice() <= ranges[i].EndKey.as_slice()
                && (region.EndKey.is_empty()
                    || region.EndKey.as_slice() >= ranges[i].StartKey.as_slice())
            {
                output.push(f.clone());
            }
        }
    } else {
        // region 元数据缺失时不过滤，避免误丢文件；上层应很少走到此分支。
        output.extend(files.iter().cloned());
    }
    Ok(output)
}
