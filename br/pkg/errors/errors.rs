// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! BR 错误码与判定辅助，移植自 Go `br/pkg/errors`。
//!
//! 通过 `br_err!` 生成带 RFC CodeText 的 `LazyLock<Error>` 静态量，
//! 分类覆盖 Common/PD/Backup/Restore/Stream/PiTR/Storage/EBS/KV。
//! `Is` 按错误 ID 在因果链中匹配；`IsContextCanceled` 对齐 Go：
//! 先 `Cause` 再比对 cancel/deadline，并遍历 `source` 链（覆盖 `%w` 包装）。
//! RFC 码字符串刻意与 Go 保持一致（含历史拼写如 `ErrPDUknownScatterResult`）。

use std::error::Error as StdError;
use std::fmt;
use std::sync::LazyLock;

use astersql_errors::{self as errors, Cause, Find, SharedError};

/// Sentinel matching Go `context.Canceled`.
/// 哨兵错误：语义等同 Go `context.Canceled`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context canceled")
    }
}

impl StdError for Canceled {}

/// Sentinel matching Go `context.DeadlineExceeded`.
/// 哨兵错误：语义等同 Go `context.DeadlineExceeded`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeadlineExceeded;

impl fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context deadline exceeded")
    }
}

impl StdError for DeadlineExceeded {}

/// Tests whether the specified error causes the error `err` (same RFC ID).
/// 判断 `err` 因果链中是否存在与 `is` 相同 RFC ID 的 `errors::Error`（对齐 Go `Is`）。
pub fn Is(err: Option<&SharedError>, is: &errors::Error) -> bool {
    Find(err, |e| {
        e.downcast_ref::<errors::Error>()
            .is_some_and(|normalized| normalized.ID() == is.ID())
    })
    .is_some()
}

// 沿 StdError::source 链查找 Canceled/DeadlineExceeded（含 SharedError 包装）。
fn chain_is_canceled(err: &(dyn StdError + 'static)) -> bool {
    let mut current: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(e) = current {
        if e.is::<Canceled>() || e.is::<DeadlineExceeded>() {
            return true;
        }
        if let Some(shared) = e.downcast_ref::<SharedError>() {
            if shared.downcast_ref::<Canceled>().is_some()
                || shared.downcast_ref::<DeadlineExceeded>().is_some()
            {
                return true;
            }
        }
        current = e.source();
    }
    false
}

// 先查 SharedError 本体，再委托 source 链；供 IsContextCanceled 复用。
fn shared_is_canceled(err: &SharedError) -> bool {
    if err.downcast_ref::<Canceled>().is_some() || err.downcast_ref::<DeadlineExceeded>().is_some()
    {
        return true;
    }
    chain_is_canceled(err as &(dyn StdError + 'static))
}

/// Checks whether the error is caused by context cancel / deadline exceeded.
/// 判断错误是否由 context 取消或超时引起。
///
/// Mirrors Go: take `errors.Cause`, compare sentinels, then `stderrors.Is`.
/// 对齐 Go：先 Cause，再比对哨兵，并沿 source 链模拟 `stderrors.Is`。
pub fn IsContextCanceled(err: Option<&SharedError>) -> bool {
    let Some(err) = err else {
        return false;
    };
    let caused = Cause(Some(err)).unwrap_or_else(|| err.clone());
    if shared_is_canceled(&caused) {
        return true;
    }
    shared_is_canceled(err)
}

/// 生成 BR 静态错误：消息 + RFCCodeText，惰性初始化以匹配 Go `errors.Normalize`。
macro_rules! br_err {
    ($name:ident, $msg:expr, $code:expr) => {
        pub static $name: LazyLock<errors::Error> =
            LazyLock::new(|| errors::Normalize($msg, &[errors::RFCCodeText($code)]));
    };
}
// —— Common：通用参数/连接/元数据类错误 ——
// 通用内部错误：未归类故障时的兜底 RFC 码。
br_err!(ErrUnknown, "internal error", "BR:Common:ErrUnknown");
// 参数非法：CLI/API 入参校验失败。
br_err!(
    ErrInvalidArgument,
    "invalid argument",
    "BR:Common:ErrInvalidArgument"
);
// 恢复目标库表未指定（RFC 名 ErrUndefinedDbOrTable）。
br_err!(
    ErrUndefinedRestoreDbOrTable,
    "undefined restore databases or tables",
    "BR:Common:ErrUndefinedDbOrTable"
);
// 组件或备份元数据版本不兼容。
br_err!(
    ErrVersionMismatch,
    "version mismatch",
    "BR:Common:ErrVersionMismatch"
);
// 建立 gRPC 通道失败（PD/TiKV 等）。
br_err!(
    ErrFailedToConnect,
    "failed to make gRPC channels",
    "BR:Common:ErrFailedToConnect"
);
// 备份元数据文件损坏或格式无效（消息可带 %s）。
br_err!(
    ErrInvalidMetaFile,
    "invalid metafile: %s",
    "BR:Common:ErrInvalidMetaFile"
);
// 必需环境变量缺失。
br_err!(
    ErrEnvNotSpecified,
    "environment variable not found",
    "BR:Common:ErrEnvNotSpecified"
);
// 当前部署/版本不支持该操作。
br_err!(
    ErrUnsupportedOperation,
    "the operation is not supported",
    "BR:Common:ErrUnsupportedOperation"
);
// 恢复键范围非法或越界。
br_err!(
    ErrInvalidRange,
    "invalid restore range",
    "BR:Common:ErrInvalidRange"
);
// 未找到对应的迁移记录。
br_err!(
    ErrMigrationNotFound,
    "no migration found",
    "BR:Common:ErrMigrationNotFound"
);
// 迁移版本号不被当前 BR 支持。
br_err!(
    ErrMigrationVersionNotSupported,
    "the migration version isn't supported",
    "BR:Common:ErrMigrationVersionNotSupported"
);
// —— PD：调度与 region 操作相关 ——
// 更新 PD 配置/状态失败。
br_err!(
    ErrPDUpdateFailed,
    "failed to update PD",
    "BR:PD:ErrPDUpdateFailed"
);
// 无法定位 PD leader。
br_err!(
    ErrPDLeaderNotFound,
    "PD leader not found",
    "BR:PD:ErrPDLeaderNotFound"
);
// PD 返回非法或非预期响应。
br_err!(
    ErrPDInvalidResponse,
    "PD invalid response",
    "BR:PD:ErrPDInvalidResponse"
);
// 批量 ScanRegion 失败。
br_err!(
    ErrPDBatchScanRegion,
    "batch scan region",
    "BR:PD:ErrPDBatchScanRegion"
);
// 等待 region scatter 结果未知/超时（Go 码拼写 Uknown）。
br_err!(
    ErrPDUnknownScatterResult,
    "failed to wait region scattered",
    "BR:PD:ErrPDUknownScatterResult"
);
// PD 侧 region 未完全打散。
br_err!(
    ErrPDNotFullyScatter,
    "pd not fully scattered",
    "BR:PD:ErrPDNotFullyScatter"
);
// 等待 region split 失败；RFC 码与 scatter 共用历史拼写。
br_err!(
    ErrPDSplitFailed,
    "failed to wait region split",
    "BR:PD:ErrPDUknownScatterResult"
);
// 目标 regions 集合未完全 scatter。
br_err!(
    ErrPDRegionsNotFullyScatter,
    "regions not fully scattered",
    "BR:PD:ErrPDRegionsNotFullyScatter"
);
// —— Backup：备份路径错误 ——
// 备份校验和与数据不一致。
br_err!(
    ErrBackupChecksumMismatch,
    "backup checksum mismatch",
    "BR:Backup:ErrBackupChecksumMismatch"
);
// 备份键范围无效。
br_err!(
    ErrBackupInvalidRange,
    "backup range invalid",
    "BR:Backup:ErrBackupInvalidRange"
);
// 备份时 region 无 leader。
br_err!(
    ErrBackupNoLeader,
    "backup no leader",
    "BR:Backup:ErrBackupNoLeader"
);
// 备份进行中 GC safepoint 越过备份进度。
br_err!(
    ErrBackupGCSafepointExceeded,
    "backup GC safepoint exceeded",
    "BR:Backup:ErrBackupGCSafepointExceeded"
);
// 备份键被锁阻塞。
br_err!(
    ErrBackupKeyIsLocked,
    "backup key is locked",
    "BR:Backup:ErrBackupKeyIsLocked"
);
// 备份过程中的 region 级错误。
br_err!(
    ErrBackupRegion,
    "backup region error",
    "BR:Backup:ErrBackupRegion"
);
// —— Restore：恢复路径错误 ——
// 恢复模式与备份/集群状态不匹配。
br_err!(
    ErrRestoreModeMismatch,
    "restore mode mismatch",
    "BR:Restore:ErrRestoreModeMismatch"
);
// 恢复范围与备份元数据不一致。
br_err!(
    ErrRestoreRangeMismatch,
    "restore range mismatch",
    "BR:Restore:ErrRestoreRangeMismatch"
);
// 恢复检查点与当前任务不匹配。
br_err!(
    ErrRestoreCheckpointMismatch,
    "restore checkpoint mismatch",
    "BR:Restore:ErrRestoreCheckpointMismatch"
);
// 恢复后校验和失败。
br_err!(
    ErrRestoreChecksumMismatch,
    "restore checksum mismatch",
    "BR:Restore:ErrRestoreChecksumMismatch"
);
// 表 ID 重写或映射不一致。
br_err!(
    ErrRestoreTableIDMismatch,
    "restore table ID mismatch",
    "BR:Restore:ErrRestoreTableIDMismatch"
);
// 移除/处理 rejected store 失败。
br_err!(
    ErrRestoreRejectStore,
    "failed to restore remove rejected store",
    "BR:Restore:ErrRestoreRejectStore"
);
// region 没有任何 peer。
br_err!(
    ErrRestoreNoPeer,
    "region does not have peer",
    "BR:Restore:ErrRestoreNoPeer"
);
// 恢复流程中 split region 失败。
br_err!(
    ErrRestoreSplitFailed,
    "fail to split region",
    "BR:Restore:ErrRestoreSplitFailed"
);
// 键重写规则非法。
br_err!(
    ErrRestoreInvalidRewrite,
    "invalid rewrite rule",
    "BR:Restore:ErrRestoreInvalidRewrite"
);
// 备份集本身无效，无法用于恢复。
br_err!(
    ErrRestoreInvalidBackup,
    "invalid backup",
    "BR:Restore:ErrRestoreInvalidBackup"
);
// SST write/ingest 阶段失败。
br_err!(
    ErrRestoreWriteAndIngest,
    "failed to write and ingest",
    "BR:Restore:ErrRestoreWriteAndIngest"
);
// 目标 schema 不存在。
br_err!(
    ErrRestoreSchemaNotExists,
    "schema not exists",
    "BR:Restore:ErrRestoreSchemaNotExists"
);
// 集群非空/非 fresh，不满足全量恢复前置。
br_err!(
    ErrRestoreNotFreshCluster,
    "cluster is not fresh",
    "BR:Restore:ErrRestoreNotFreshCluster"
);
// 系统表与目标集群不兼容。
br_err!(
    ErrRestoreIncompatibleSys,
    "incompatible system table",
    "BR:Restore:ErrRestoreIncompatibleSys"
);
// 该系统表暂不支持恢复（RFC ErrUnsupportedSysTable）。
br_err!(
    ErrUnsupportedSystemTable,
    "the system table isn't supported for restoring yet",
    "BR:Restore:ErrUnsupportedSysTable"
);
// 目标集群已存在同名库。
br_err!(
    ErrDatabasesAlreadyExisted,
    "databases already existed in restored cluster",
    "BR:Restore:ErrDatabasesAlreadyExisted"
);
// 目标集群已存在同名表。
br_err!(
    ErrTablesAlreadyExisted,
    "tables already existed in restored cluster",
    "BR:Restore:ErrTablesAlreadyExisted"
);
// —— Stream：日志备份任务错误 ——
// 日志备份任务已存在（当前仅支持单任务）。
br_err!(
    ErrStreamLogTaskExist,
    "stream task already exists",
    "BR:Stream:ErrStreamLogTaskExist"
);
// 日志备份任务未绑定外部存储。
br_err!(
    ErrStreamLogTaskHasNoStorage,
    "stream task has no storage",
    "BR:Stream:ErrStreamLogTaskHasNoStorage"
);
// —— Restore/PiTR 边界：resolved-ts 约束 ——
// resolved-ts 约束被违反（Go 注：或属 PiTR）。
br_err!(
    ErrRestoreRTsConstrain,
    "resolved ts constrain violation",
    "BR:Restore:ErrRestoreResolvedTsConstrain"
);
// —— PiTR：时间点恢复 ——
// PiTR：CDC 日志格式非法。
br_err!(
    ErrPiTRInvalidCDCLogFormat,
    "invalid cdc log format",
    "BR:PiTR:ErrPiTRInvalidCDCLogFormat"
);
// PiTR：任务不存在（RFC ErrTaskNotFound）。
br_err!(
    ErrPiTRTaskNotFound,
    "task not found",
    "BR:PiTR:ErrTaskNotFound"
);
// PiTR：任务信息无效（RFC ErrInvalidTaskInfo）。
br_err!(
    ErrPiTRInvalidTaskInfo,
    "task info is invalid",
    "BR:PiTR:ErrInvalidTaskInfo"
);
// PiTR：元数据损坏或格式错误。
br_err!(
    ErrPiTRMalformedMetadata,
    "malformed metadata",
    "BR:PiTR:ErrMalformedMetadata"
);
// PiTR：checkpoint watch 需重启。
br_err!(
    ErrPiTRCheckpointWatchRestart,
    "checkpoint watch needs restart",
    "BR:PiTR:ErrCheckpointWatchRestart"
);
// —— ExternalStorage：外部存储 ——
// 外部存储未知错误。
br_err!(
    ErrStorageUnknown,
    "unknown external storage error",
    "BR:ExternalStorage:ErrStorageUnknown"
);
// 外部存储配置非法。
br_err!(
    ErrStorageInvalidConfig,
    "invalid external storage config",
    "BR:ExternalStorage:ErrStorageInvalidConfig"
);
// 外部存储权限不足或无效。
br_err!(
    ErrStorageInvalidPermission,
    "external storage permission",
    "BR:ExternalStorage:ErrStorageInvalidPermission"
);
// —— EBS/快照恢复 ——
// EBS/快照恢复：TiKV 数量与预期不符。
br_err!(
    ErrRestoreTotalKVMismatch,
    "restore total tikvs mismatch",
    "BR:EBS:ErrRestoreTotalKVMismatch"
);
// EBS/快照恢复：遇到非法 peer。
br_err!(
    ErrRestoreInvalidPeer,
    "restore met a invalid peer",
    "BR:EBS:ErrRestoreInvalidPeer"
);
// EBS/快照恢复：region 无任何 peer。
br_err!(
    ErrRestoreRegionWithoutPeer,
    "restore met a region without any peer",
    "BR:EBS:ErrRestoreRegionWithoutPeer"
);
// —— KV：TiKV 报告及 ingest/download ——
// TiKV 存储 I/O 错误。
br_err!(
    ErrKVStorage,
    "tikv storage occur I/O error",
    "BR:KV:ErrKVStorage"
);
// TiKV 未知错误。
br_err!(
    ErrKVUnknown,
    "unknown error occur on tikv",
    "BR:KV:ErrKVUnknown"
);
// TiKV cluster ID 与预期不符。
br_err!(
    ErrKVClusterIDMismatch,
    "tikv cluster ID mismatch",
    "BR:KV:ErrKVClusterIDMismatch"
);
// 请求打到非 leader（通常可重试）。
br_err!(ErrKVNotLeader, "not leader", "BR:KV:ErrKVNotLeader");
// 存储引擎非 TiKV（RFC ErrNotTiKVStorage）。
br_err!(
    ErrKVNotTiKV,
    "storage is not tikv",
    "BR:KV:ErrNotTiKVStorage"
);
// TiKV 磁盘已满。
br_err!(ErrKVDiskFull, "disk is full", "BR:KV:ErrKVDiskFull");
// ingest 遇 epoch not match，通常可重试。
br_err!(
    ErrKVEpochNotMatch,
    "epoch not match",
    "BR:KV:ErrKVEpochNotMatch"
);
// ingest 遇 key not in region，不可重试。
br_err!(
    ErrKVKeyNotInRegion,
    "key not in region",
    "BR:KV:ErrKVKeyNotInRegion"
);
// download 缺 rewrite rule，不可重试。
br_err!(
    ErrKVRewriteRuleNotFound,
    "rewrite rule not found",
    "BR:KV:ErrKVRewriteRuleNotFound"
);
// download 遇空 range，不可重试。
br_err!(
    ErrKVRangeIsEmpty,
    "range is empty",
    "BR:KV:ErrKVRangeIsEmpty"
);
// 通用 SST download 失败，预期可重试。
br_err!(
    ErrKVDownloadFailed,
    "download sst failed",
    "BR:KV:ErrKVDownloadFailed"
);
// 通用 SST ingest 失败，预期可重试。
br_err!(
    ErrKVIngestFailed,
    "ingest sst failed",
    "BR:KV:ErrKVIngestFailed"
);
// 集群状态可能不一致，需人工排查。
br_err!(
    ErrPossibleInconsistency,
    "the cluster state might be inconsistent",
    "BR:KV:ErrPossibleInconsistency"
);
