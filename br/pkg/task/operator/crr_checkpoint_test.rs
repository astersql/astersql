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

//! Go-equivalent tests for `br/pkg/task/operator/crr_checkpoint_test.go`.
//!
//! Storage/PD/etcd boundaries use in-crate MemStorage / MemGlue stand-ins
//! (arm64: no kv/domain/kvproto/grpcio). Call order, errors, and data shapes
//! match the Go fixtures.
//! 覆盖 etcd 配置装配、CRR 服务创建拒识、lock 校验与 sync checker 选择；
//! 断言文案/路径需与 Go 夹具保持一致，禁止弱化错误关键字检查。
//! 错误消息需包含 LockFile 常量名。
//! 上游标签与下游标签分别出现在错误中。
//! downstream-check 模式下同名对象存在即同步。
//! 上游 FixedSyncChecker 可忽略下游缺失文件。
//! 纯 MemStorage 无 sync 能力必须硬失败。

use std::sync::Arc;
use std::time::Duration;

use crate::config::{
    CRRCheckpointConfig, DefineFlagsForCRRCheckpointConfig, flagDownstreamStorage, flagTaskName,
    flagUpstreamStorage,
};
use crate::crr_checkpoint::{
    NewCRRCheckpointService, buildObjectSyncChecker, buildResumeStateStore,
    checkCRRExternalStorage, etcdGRPCBackoffConfig, etcdKeepaliveParams, newEtcdClientConfig,
    storageResumeStateStore,
};
use crate::stubs::{
    Config, ExternalReader, ExternalStorage, ExternalWriter, FixedSyncChecker, FlagSet, LockFile,
    MemGlue, MemStorage, ObjectSyncChecker, ReaderOption, Result, WalkOption,
};

/// Go `syncedStorage` — embeds storage and always reports a fixed FileSynced result.
/// 包装 MemStorage，强制 `as_object_sync_checker` 返回固定结果，用于上游原生 sync 路径。
struct syncedStorage {
    inner: MemStorage,
    /// FileSynced 固定返回值；与对象是否真实存在无关。
    synced: bool,
}

// DialTimeout 断言五秒。
impl ExternalStorage for syncedStorage {
    // AutoSyncInterval 断言三十秒。
    fn URI(&self) -> String {
        self.inner.URI()
    }
    // endpoints 应等于 PD 列表。
    fn Close(&self) {
        self.inner.Close();
    }
    // keepalive 时间与超时透传自 Config。
    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()> {
        self.inner.WriteFile(name, data)
    }
    // backoff 最大值必须为三秒。
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>> {
        self.inner.ReadFile(name)
    }
    // 断言 PermitWithoutStream 恒为真。
    fn FileExists(&self, name: &str) -> Result<bool> {
        self.inner.FileExists(name)
    }
    // Go 的空下游在 Rust 用占位 MemStorage 代替。
    fn DeleteFile(&self, name: &str) -> Result<()> {
        self.inner.DeleteFile(name)
    }
    // LockFile 写入后才视为合法日志备份目录。
    fn DeleteFiles(&self, names: &[String]) -> Result<()> {
        self.inner.DeleteFiles(names)
    }
    // 空下游 URI 必须在解析阶段失败。
    fn Rename(&self, old: &str, new: &str) -> Result<()> {
        self.inner.Rename(old, new)
    }
    // etcd DialOptionsLen 在 arm64 用计数代替对象。
    fn WalkDir(&self, opt: &WalkOption, f: &mut dyn FnMut(&str, i64) -> Result<()>) -> Result<()> {
        self.inner.WalkDir(opt, f)
    }
    // 下游存在性模式与上游原生 sync 互斥覆盖。
    fn Open(&self, name: &str, opt: Option<&ReaderOption>) -> Result<Box<dyn ExternalReader>> {
        self.inner.Open(name, opt)
    }
    // resume-state 默认路径变更视为破坏性改动。
    fn Create(&self, name: &str) -> Result<Box<dyn ExternalWriter>> {
        self.inner.Create(name)
    }
    // 注入 FixedSyncChecker，使上游路径不依赖真实云厂商 sync API。
    fn as_object_sync_checker(&self) -> Option<Arc<dyn ObjectSyncChecker>> {
        Some(Arc::new(FixedSyncChecker {
            synced: self.synced,
        }))
    }
}

/// TestNewEtcdClientConfig — Go etcd gRPC backoff / keepalive / dial-option wiring.
/// 校验 backoff=3s、keepalive 透传 Config、DialOptionsLen=4、endpoints=PD。
#[test]
// ParseFromFlags 与服务创建分层断言。
fn test_new_etcd_client_config() {
    let cfg = Config {
        PD: vec!["pd-service:2379".to_string()],
        GRPCKeepaliveTime: Duration::from_secs(10),
        GRPCKeepaliveTimeout: Duration::from_secs(3),
        ..Default::default()
    };

    let backoff_cfg = etcdGRPCBackoffConfig();
    assert_eq!(backoff_cfg.MaxDelay, Duration::from_secs(3));
    let keepalive_params = etcdKeepaliveParams(&cfg);
    assert_eq!(keepalive_params.Time, Duration::from_secs(10));
    assert_eq!(keepalive_params.Timeout, Duration::from_secs(3));
    assert!(keepalive_params.PermitWithoutStream);

    let etcd_cfg = newEtcdClientConfig(&cfg).expect("new etcd config");
    assert_eq!(etcd_cfg.Endpoints, cfg.PD);
    assert_eq!(etcd_cfg.AutoSyncInterval, Duration::from_secs(30));
    assert_eq!(etcd_cfg.DialTimeout, Duration::from_secs(5));
    // Go asserts `etcdCfg.Context == ctx`; Rust config is not context-scoped.
    // Go asserts `len(DialOptions) == 4`; arm64 folds those into DialOptionsLen.
    // 上述两条注释记录与 Go 的刻意差异，勿改断言去“补齐”不存在的 Context。
    assert_eq!(etcd_cfg.DialOptionsLen, 4);
    assert_eq!(etcd_cfg.Backoff.MaxDelay, Duration::from_secs(3));
    assert_eq!(etcd_cfg.Keepalive.Time, Duration::from_secs(10));
    assert_eq!(etcd_cfg.Keepalive.Timeout, Duration::from_secs(3));
    assert!(etcd_cfg.Keepalive.PermitWithoutStream);
}

/// TestNewCRRCheckpointServiceRejectsNonLogBackupUpstream — missing flag + non-log-backup reject.
/// 先测 ParseFromFlags 缺下游；再测空存储无 LockFile 时服务创建失败。
#[test]
// syncedStorage 固定 FileSynced，不读真实对象。
fn test_new_crr_checkpoint_service_rejects_non_log_backup_upstream() {
    let mut flags = FlagSet::new();
    DefineFlagsForCRRCheckpointConfig(&mut flags);
    flags.SetString(flagTaskName, "test-task");
    flags.SetString(flagUpstreamStorage, "mem://upstream-no-downstream");
    // downstream-storage left empty — ParseFromFlags must reject.
    // 空下游必须在 Parse 阶段失败，不应进入 NewCRRCheckpointService。
    flags.SetString(flagDownstreamStorage, "");

    let mut parse_cfg = CRRCheckpointConfig::default();
    let err = parse_cfg
        .ParseFromFlags(&flags)
        .expect_err("missing downstream");
    assert!(
        err.msg
            .contains("missing required flag --downstream-storage"),
        "err={}",
        err.msg
    );

    // 上下游 URI 齐全但无 lock：服务应报 “not a log backup directory”。
    let mut cfg = CRRCheckpointConfig {
        UpstreamStorage: "mem://upstream-empty".into(),
        DownstreamStorage: "mem://downstream-empty".into(),
        ..Default::default()
    };
    cfg.CRRConfig.TaskName = "test-task".to_string();

    let g = MemGlue::default();
    let err = match NewCRRCheckpointService(&g, cfg) {
        Ok(_) => panic!("expected non-log-backup upstream error"),
        Err(e) => e,
    };
    assert!(
        err.msg.contains("is not a log backup directory"),
        "err={}",
        err.msg
    );
    assert!(
        err.msg.contains(LockFile),
        "err should mention {LockFile}: {}",
        err.msg
    );
}

/// TestCheckCRRUpstreamStorage — resume-state path + lock-file validation for up/downstream.
/// 断言默认 resume 路径，并分别验证上下游缺/有 LockFile 的行为。
#[test]
// MemStorage 与 MemGlue 替身隔离网络与 PD。
fn test_check_crr_upstream_storage() {
    let upstream = Arc::new(MemStorage::new("mem://upstream-check"));

    let state_store = buildResumeStateStore(upstream.clone());
    // Go: stateStore.(*storageResumeStateStore); path == "crr-checkpoint/resume-state.json"
    // 路径常量来自 GetStatusFileName，改名会破坏跨版本 resume。
    let storage_store: &storageResumeStateStore = state_store.as_ref();
    assert_eq!(storage_store.path, "crr-checkpoint/resume-state.json");

    let err = checkCRRExternalStorage(upstream.as_ref(), "upstream").expect_err("no lock");
    assert!(
        err.msg.contains("is not a log backup directory"),
        "err={}",
        err.msg
    );
    assert!(err.msg.contains("upstream storage"), "err={}", err.msg);
    assert!(err.msg.contains(LockFile), "err={}", err.msg);

    // Writing LockFile makes the upstream a valid log-backup directory.
    // 写入空 lock 即视为合法日志备份目录（与 Go 夹具相同）。
    upstream.WriteFile(LockFile, &[]).expect("write lock");
    checkCRRExternalStorage(upstream.as_ref(), "upstream").expect("upstream ok");

    let downstream = Arc::new(MemStorage::new("mem://downstream-check"));
    let err = checkCRRExternalStorage(downstream.as_ref(), "downstream").expect_err("no lock");
    assert!(
        err.msg.contains("is not a log backup directory"),
        "err={}",
        err.msg
    );
    // 错误消息必须带 source 标签，便于区分上下游。
    assert!(err.msg.contains("downstream storage"), "err={}", err.msg);
}

/// TestBuildObjectSyncChecker — downstream existence vs upstream native sync checker selection.
/// 覆盖：下游模式命中文件、上游 FixedSyncChecker、纯 MemStorage 硬失败两条路径。
#[test]
// 夹具错误关键字不得弱化，需与 Go 文案相交。
fn test_build_object_sync_checker() {
    let upstream = Arc::new(MemStorage::new("mem://sync-up"));
    let downstream = Arc::new(MemStorage::new("mem://sync-down"));
    downstream
        .WriteFile("synced.log", b"synced")
        .expect("seed synced.log");

    // downstream-check mode: existence of the object on downstream.
    // check=true 时忽略上游能力，只看下游是否存在同名对象。
    let checker = buildObjectSyncChecker(upstream.clone(), downstream.clone(), true)
        .expect("downstream checker");
    let synced = checker.FileSynced("synced.log").expect("file synced");
    assert!(synced);

    // Non-downstream-check mode requires upstream to implement ObjectSyncChecker.
    // synced=true 的夹具即使下游无该文件也应回报已同步。
    let synced_up = Arc::new(syncedStorage {
        inner: MemStorage::new("mem://synced-up"),
        synced: true,
    });
    let checker =
        buildObjectSyncChecker(synced_up, downstream.clone(), false).expect("upstream checker");
    let synced = checker
        .FileSynced("missing-downstream.log")
        .expect("file synced");
    assert!(synced);

    let err = buildObjectSyncChecker(upstream.clone(), downstream.clone(), false)
        .err()
        .expect("plain upstream cannot sync-check");
    assert!(
        err.msg
            .contains("upstream storage cannot check object sync"),
        "err={}",
        err.msg
    );

    // Go passes nil downstream with check=false; downstream is unused on that path.
    // Rust 用占位 MemStorage 代替 nil；断言仍要求上游无 sync 能力时报错。
    let err = buildObjectSyncChecker(
        upstream,
        Arc::new(MemStorage::new("mem://unused-down")),
        false,
    )
    .err()
    .expect("plain upstream cannot sync-check");
    assert!(
        err.msg
            .contains("upstream storage cannot check object sync"),
        "err={}",
        err.msg
    );
}
