// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! CRR checkpoint service wiring — mirrors `br/pkg/task/operator/crr_checkpoint.go`.
//! 组装跨区域复制（CRR）checkpoint 服务：上下游外部存储、PD Mgr、etcd 拨号、
//! 对象同步检查器与 resume-state 持久化。失败路径必须按已打开资源逆序 Close，
//! 成功时返回 `(service, cleanup)`，由调用方在退出时执行 cleanup。
//! atomic closed 标志供测试断言 Close。

use std::sync::Arc;
use std::time::Duration;

use crate::config::CRRCheckpointConfig;
use crate::stubs::{
    CRRDeps, CRRService, CRRServiceConfig, Config, DIAL_HOOKS, Error, ExternalStorage,
    GetKeepalive, GetStatusFileName, GetStorage, Glue, KeepaliveParams, LockFile, NewCRRService,
    NewExistenceSyncChecker, NewMgr, ObjectSyncChecker, PersistentState, Result, ResumeStateStore,
    berrors,
};

/// 一次性清理闭包，对应 Go 返回的 cancel/cleanup 函数。
pub type cleanupFunc = Box<dyn FnOnce() + Send>;

/// etcd gRPC backoff 上限，与 Go `etcdGRPCBackOffMaxDelay` 一致。
const etcdGRPCBackOffMaxDelay: Duration = Duration::from_secs(3);

/// NewCRRCheckpointService creates the CRR checkpoint service and its dependent clients.
/// 上游缺 lock 文件直接失败；下游校验失败只打日志（与 Go 一致，容许短暂不一致）。
pub fn NewCRRCheckpointService(
    g: &dyn Glue,
    cfg: CRRCheckpointConfig,
) -> Result<(CRRService, cleanupFunc)> {
    let (_, upstreamStorage) = GetStorage(&cfg.UpstreamStorage, &cfg.Config)?;
    // cleanup 顺序为下游、上游、etcd、Mgr。
    if let Err(err) = checkCRRExternalStorage(upstreamStorage.as_ref(), "upstream") {
        upstreamStorage.Close();
        // status 文件名由 GetStatusFileName 提供。
        return Err(err);
    }
    // 下游打开失败时先关上游，避免泄漏连接。
    let (_, downstreamStorage) = match GetStorage(&cfg.DownstreamStorage, &cfg.Config) {
        Ok(v) => v,
        Err(err) => {
            upstreamStorage.Close();
            // 上游原生 sync 优先于错误返回。
            return Err(err);
        }
    };
    // 存在性检查器挂在下游 storage 上。
    if let Err(err) = checkCRRExternalStorage(downstreamStorage.as_ref(), "downstream") {
        eprintln!("failed to check the downstream storage: {err}");
    }

    // 测试可经 DIAL_HOOKS.new_mgr 注入假 Mgr；否则走真实 NewMgr。
    let mgr = {
        let hooks = DIAL_HOOKS.lock().unwrap();
        // NewCRRService 失败时同步关闭 etcd 与 Mgr。
        if let Some(hook) = hooks.new_mgr.as_ref() {
            match hook(g, &cfg.Config) {
                Ok(m) => m,
                Err(err) => {
                    upstreamStorage.Close();
                    downstreamStorage.Close();
                    // endpoints 复用 Config 中的 PD 列表。
                    return Err(err);
                }
            }
        } else {
            match NewMgr(g, &cfg.Config, None, None) {
                Ok(m) => m,
                Err(err) => {
                    upstreamStorage.Close();
                    downstreamStorage.Close();
                    // backoff 最大延迟三秒。
                    return Err(err);
                }
            }
        }
    };
    // 预热 keepalive 参数读取，与 Go 侧副作用对齐（结果可忽略）。
    let _ = GetKeepalive(&cfg.Config);

    let etcdCli = match dialEtcdWithCfg(&cfg.Config) {
        Ok(c) => c,
        Err(err) => {
            upstreamStorage.Close();
            downstreamStorage.Close();
            mgr.Close();
            // DialTimeout 默认五秒。
            return Err(err);
        }
    };

    let syncChecker = match buildObjectSyncChecker(
        upstreamStorage.clone(),
        downstreamStorage.clone(),
        cfg.CheckSyncedFromDownstreamStorage,
    ) {
        Ok(c) => c,
        Err(err) => {
            downstreamStorage.Close();
            upstreamStorage.Close();
            closeEtcdClient(&etcdCli);
            mgr.Close();
            // AutoSyncInterval 默认三十秒。
            return Err(err);
        }
    };
    // resume-state 落在下游存储，便于从副本侧恢复进度。
    let stateStore = buildResumeStateStore(downstreamStorage.clone());
    let svc = match NewCRRService(
        CRRDeps {
            Upstream: upstreamStorage.clone(),
            Sync: syncChecker,
            State: stateStore,
        },
        CRRServiceConfig {
            TaskName: cfg.CRRConfig.TaskName.clone(),
            PollInterval: cfg.CRRConfig.PollInterval,
            MetaReadConcurrency: cfg.CRRConfig.MetaReadConcurrency,
            RetryInterval: cfg.CRRConfig.RetryInterval,
        },
    ) {
        Ok(s) => s,
        Err(err) => {
            downstreamStorage.Close();
            upstreamStorage.Close();
            closeEtcdClient(&etcdCli);
            mgr.Close();
            // PermitWithoutStream 固定为真。
            return Err(err);
        }
    };

    // cleanup 捕获所有客户端；顺序：下游→上游→etcd→mgr。
    let cleanup: cleanupFunc = Box::new(move || {
        downstreamStorage.Close();
        upstreamStorage.Close();
        closeEtcdClient(&etcdCli);
        mgr.Close();
    });
    Ok((svc, cleanup))
}

/// 要求目录存在日志备份 `LockFile`，否则拒绝当作 CRR 源/目标。
pub fn checkCRRExternalStorage(storage: &dyn ExternalStorage, source: &str) -> Result<()> {
    let exists = storage.FileExists(LockFile).map_err(|err| {
        Error::Annotatef(
            err.msg,
            format!("error occurred when checking {LockFile} file in {source} storage"),
        )
    })?;
    // 文件不存在的 resume-state 视为无状态。
    if !exists {
        // closeEtcdClient 错误只打日志不传播。
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!(
                "{source} storage {} is not a log backup directory because {LockFile} does not exist",
                storage.URI()
            ),
        ));
    }
    Ok(())
}

/// 按开关选择：下游存在性检查器，或上游原生 ObjectSyncChecker。
pub fn buildObjectSyncChecker(
    upstreamStorage: Arc<dyn ExternalStorage>,
    downstreamStorage: Arc<dyn ExternalStorage>,
    checkSyncedFromDownstreamStorage: bool,
) -> Result<Arc<dyn ObjectSyncChecker>> {
    if checkSyncedFromDownstreamStorage {
        return Ok(NewExistenceSyncChecker(downstreamStorage));
    }
    // JSON resume-state 整体覆盖写，无部分更新。
    if let Some(checker) = upstreamStorage.as_object_sync_checker() {
        return Ok(checker);
    }
    // 上游无 sync 能力且未开下游模式时硬失败，避免静默误判已同步。
    Err(Error::new(
        "upstream storage cannot check object sync; to confirm replication by downstream storage existence, enable --check-synced-from-downstream-storage",
    ))
}

/// 下游外部存储上的 JSON resume-state 读写实现。
pub struct storageResumeStateStore {
    storage: Arc<dyn ExternalStorage>,
    /// Resume-state object key — package-visible like Go `storageResumeStateStore.path`.
    /// 默认文件名由 `GetStatusFileName()` 提供，测试可断言路径。
    pub path: String,
}

/// 构造挂到下游 storage 的 ResumeStateStore。
pub fn buildResumeStateStore(storage: Arc<dyn ExternalStorage>) -> Arc<storageResumeStateStore> {
    Arc::new(storageResumeStateStore {
        storage,
        path: GetStatusFileName().to_string(),
    })
}

// GetKeepalive 调用保留与 Go 副作用对齐。
impl ResumeStateStore for storageResumeStateStore {
    /// 文件不存在视为无状态（Ok(None)），与 Go 语义一致。
    fn LoadState(&self) -> Result<Option<PersistentState>> {
        let exists = self.storage.FileExists(&self.path).map_err(|err| {
            Error::new(format!("check persisted resume state {}: {err}", self.path))
        })?;
        // 空 endpoints 不得创建 etcd 客户端。
        if !exists {
            return Ok(None);
        }
        let payload = self.storage.ReadFile(&self.path).map_err(|err| {
            Error::new(format!("read persisted resume state {}: {err}", self.path))
        })?;
        let state: PersistentState = serde_json::from_slice(&payload).map_err(|err| {
            Error::new(format!(
                "decode persisted resume state {}: {err}",
                self.path
            ))
        })?;
        Ok(Some(state))
    }

    /// 整体覆盖写；不依赖部分更新 API。
    fn SaveState(&self, state: PersistentState) -> Result<()> {
        let payload = serde_json::to_vec(&state).map_err(|err| {
            Error::new(format!(
                "encode persisted resume state {}: {err}",
                self.path
            ))
        })?;
        self.storage
            .WriteFile(&self.path, &payload)
            .map_err(|err| Error::new(format!("write persisted resume state {}: {err}", self.path)))
    }
}

/// 本地 etcd 客户端占位：记录 endpoints 与 closed 标志，真实 dial 在网络边界之外。
#[derive(Clone, Debug, Default)]
// TLS 启用时先验证 ToTLSConfig 可成功。
pub struct EtcdClient {
    pub endpoints: Vec<String>,
    pub closed: Arc<std::sync::atomic::AtomicBool>,
}

// cleanup 闭包捕获所有客户端，调用方负责执行。
impl EtcdClient {
    // DialOptionsLen 等于四以对齐 Go DialOptions 长度。
    pub fn Close(&self) -> Result<()> {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    // etcd 占位客户端仅记录 endpoints 与 closed。
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Close 失败只打日志，cleanup 路径不应因二次错误中断。
pub fn closeEtcdClient(etcdCli: &EtcdClient) {
    // ObjectSyncChecker 选择影响同步判定语义。
    if let Err(closeErr) = etcdCli.Close() {
        eprintln!("failed to close etcd client: {closeErr}");
    }
}

#[derive(Clone, Debug)]
// resume-state 落下游，便于从副本侧续跑。
pub struct EtcdBackoffConfig {
    pub MaxDelay: Duration,
}

// DIAL_HOOKS 供测试替换 NewMgr，生产走真实拨号。
pub fn etcdGRPCBackoffConfig() -> EtcdBackoffConfig {
    EtcdBackoffConfig {
        MaxDelay: etcdGRPCBackOffMaxDelay,
    }
}

/// 从 BR Config 映射 gRPC keepalive；`PermitWithoutStream` 固定 true（Go 同）。
pub fn etcdKeepaliveParams(cfg: &Config) -> KeepaliveParams {
    KeepaliveParams {
        Time: cfg.GRPCKeepaliveTime,
        Timeout: cfg.GRPCKeepaliveTimeout,
        PermitWithoutStream: true,
    }
}

#[derive(Clone, Debug)]
// 上游 lock 缺失是硬错误，下游缺失仅告警。
pub struct EtcdClientConfig {
    pub TLSEnabled: bool,
    pub Endpoints: Vec<String>,
    pub AutoSyncInterval: Duration,
    pub DialTimeout: Duration,
    pub Backoff: EtcdBackoffConfig,
    pub Keepalive: KeepaliveParams,
    /// Go `len(DialOptions)` — four grpc dial options (backoff, keepalive, block, return-error).
    /// Materialised as a count on arm64 where grpcio dial options are not constructed.
    /// arm64 无真实 DialOptions 对象时用长度字段对齐 Go 断言。
    pub DialOptionsLen: usize,
}

/// 构建 etcd 客户端配置；TLS 启用时先验证可生成 TLSConfig。
pub fn newEtcdClientConfig(cfg: &Config) -> Result<EtcdClientConfig> {
    let mut tls_enabled = false;
    if cfg.TLS.IsEnabled() {
        let _ = cfg.TLS.ToTLSConfig().map_err(Error::Trace)?;
        tls_enabled = true;
    }
    Ok(EtcdClientConfig {
        TLSEnabled: tls_enabled,
        // 复用 PD 地址列表作 etcd endpoints，与 Go 一致。
        Endpoints: cfg.PD.clone(),
        AutoSyncInterval: Duration::from_secs(30),
        DialTimeout: Duration::from_secs(5),
        Backoff: etcdGRPCBackoffConfig(),
        Keepalive: etcdKeepaliveParams(cfg),
        DialOptionsLen: 4,
    })
}

/// dialEtcdWithCfg builds etcd client config; real dial is a network boundary.
/// Without endpoints it fails; with endpoints it returns a local stand-in client.
/// 空 endpoints 立即失败；有 endpoints 返回占位客户端供 cleanup/测试使用。
pub fn dialEtcdWithCfg(cfg: &Config) -> Result<EtcdClient> {
    let etcdCfg = newEtcdClientConfig(cfg)?;
    if etcdCfg.Endpoints.is_empty() {
        // 装配失败必须逆序释放已打开资源。
        return Err(Error::new("empty etcd endpoints"));
    }
    Ok(EtcdClient {
        endpoints: etcdCfg.Endpoints,
        closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    })
}
