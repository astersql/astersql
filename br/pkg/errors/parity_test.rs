// Copyright 2026 AsterSQL.
//! errors 包公开契约对等测试：RFC ID、`Is`/`Equal`、取消判定与消息模板。
//!
//! 相对 `errors_test` 更偏契约快照（含 ErrUnknown 模板/ID），确保与 Go Normalize 一致。

use std::error::Error as StdError;
use std::fmt;

use astersql_errors::{Annotate, New, SharedError, Trace};

use super::*;

/// 轻量包装：仅实现 `source`，用于验证非 Trace 的 std 错误链取消识别。
#[derive(Debug)]
struct WrappedCancel {
    inner: SharedError,
}

impl fmt::Display for WrappedCancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wrapped: {}", self.inner)
    }
}

impl StdError for WrappedCancel {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.inner)
    }
}

/// 核对 Is/Equal、取消边界、source 包装及若干 RFC 码/消息模板与 Go 对齐。
#[test]
fn go_rust_public_contract_matches() {
    // normal: RFC ID equality via Is / Equal after annotate
    // Annotate 后 Equal 与 Is 均按 RFC ID 命中；不同码（ErrUnknown）必须为假。
    let annotated = Annotate(
        Some(SharedError::new((*ErrPDBatchScanRegion).clone())),
        "test error equla",
    )
    .expect("annotate keeps error");
    assert!(ErrPDBatchScanRegion.Equal(Some(&annotated)));
    assert!(Is(Some(&annotated), &ErrPDBatchScanRegion));
    assert!(!Is(Some(&annotated), &ErrUnknown));

    // boundary: nil / unrelated errors are not canceled
    // 边界：None 与无关错误不得判为 context canceled。
    assert!(!IsContextCanceled(None));
    assert!(!IsContextCanceled(Some(&New("connection closed"))));

    // error: cancel and deadline sentinels, including Trace wrappers
    // 哨兵及 Trace 包装均应识别（对齐 Go Cause + stderrors.Is）。
    let canceled = SharedError::new(Canceled);
    let deadline = SharedError::new(DeadlineExceeded);
    assert!(IsContextCanceled(Some(&canceled)));
    assert!(IsContextCanceled(Some(&deadline)));
    assert!(IsContextCanceled(Trace(Some(canceled.clone())).as_ref()));
    assert!(IsContextCanceled(Trace(Some(deadline.clone())).as_ref()));

    // resource / wrapping: std Error source chain (url.Error shape)
    // 自定义 source 链包装（类 url.Error）同样穿透识别。
    let wrapped_cancel = SharedError::new(WrappedCancel {
        inner: canceled.clone(),
    });
    let wrapped_deadline = SharedError::new(WrappedCancel { inner: deadline });
    assert!(IsContextCanceled(Some(&wrapped_cancel)));
    assert!(IsContextCanceled(Some(&wrapped_deadline)));

    // message templates and RFC codes stay aligned with Go
    // 消息模板与 RFC ID 字符串必须与 Go errors.Normalize 常量一致。
    // 抽样 Common 与 PD 两类码，防止迁移时误改 RFC 文本。
    assert_eq!(ErrUnknown.MessageTemplate(), "internal error");
    assert_eq!(ErrUnknown.ID(), "BR:Common:ErrUnknown");
    assert_eq!(ErrPDBatchScanRegion.ID(), "BR:PD:ErrPDBatchScanRegion");
}

/// Exhaustive snapshot of every Go `errors.Normalize` declaration in `errors.go`.
#[test]
fn all_normalized_errors_match_go() {
    macro_rules! assert_errors {
        ($($error:ident => ($message:literal, $id:literal)),+ $(,)?) => {
            $(
                assert_eq!($error.MessageTemplate(), $message, stringify!($error));
                assert_eq!($error.ID(), $id, stringify!($error));
            )+
        };
    }

    assert_errors! {
        ErrUnknown => ("internal error", "BR:Common:ErrUnknown"),
        ErrInvalidArgument => ("invalid argument", "BR:Common:ErrInvalidArgument"),
        ErrUndefinedRestoreDbOrTable => ("undefined restore databases or tables", "BR:Common:ErrUndefinedDbOrTable"),
        ErrVersionMismatch => ("version mismatch", "BR:Common:ErrVersionMismatch"),
        ErrFailedToConnect => ("failed to make gRPC channels", "BR:Common:ErrFailedToConnect"),
        ErrInvalidMetaFile => ("invalid metafile: %s", "BR:Common:ErrInvalidMetaFile"),
        ErrEnvNotSpecified => ("environment variable not found", "BR:Common:ErrEnvNotSpecified"),
        ErrUnsupportedOperation => ("the operation is not supported", "BR:Common:ErrUnsupportedOperation"),
        ErrInvalidRange => ("invalid restore range", "BR:Common:ErrInvalidRange"),
        ErrMigrationNotFound => ("no migration found", "BR:Common:ErrMigrationNotFound"),
        ErrMigrationVersionNotSupported => ("the migration version isn't supported", "BR:Common:ErrMigrationVersionNotSupported"),
        ErrPDUpdateFailed => ("failed to update PD", "BR:PD:ErrPDUpdateFailed"),
        ErrPDLeaderNotFound => ("PD leader not found", "BR:PD:ErrPDLeaderNotFound"),
        ErrPDInvalidResponse => ("PD invalid response", "BR:PD:ErrPDInvalidResponse"),
        ErrPDBatchScanRegion => ("batch scan region", "BR:PD:ErrPDBatchScanRegion"),
        ErrPDUnknownScatterResult => ("failed to wait region scattered", "BR:PD:ErrPDUknownScatterResult"),
        ErrPDNotFullyScatter => ("pd not fully scattered", "BR:PD:ErrPDNotFullyScatter"),
        ErrPDSplitFailed => ("failed to wait region split", "BR:PD:ErrPDUknownScatterResult"),
        ErrPDRegionsNotFullyScatter => ("regions not fully scattered", "BR:PD:ErrPDRegionsNotFullyScatter"),
        ErrBackupChecksumMismatch => ("backup checksum mismatch", "BR:Backup:ErrBackupChecksumMismatch"),
        ErrBackupInvalidRange => ("backup range invalid", "BR:Backup:ErrBackupInvalidRange"),
        ErrBackupNoLeader => ("backup no leader", "BR:Backup:ErrBackupNoLeader"),
        ErrBackupGCSafepointExceeded => ("backup GC safepoint exceeded", "BR:Backup:ErrBackupGCSafepointExceeded"),
        ErrBackupKeyIsLocked => ("backup key is locked", "BR:Backup:ErrBackupKeyIsLocked"),
        ErrBackupRegion => ("backup region error", "BR:Backup:ErrBackupRegion"),
        ErrRestoreModeMismatch => ("restore mode mismatch", "BR:Restore:ErrRestoreModeMismatch"),
        ErrRestoreRangeMismatch => ("restore range mismatch", "BR:Restore:ErrRestoreRangeMismatch"),
        ErrRestoreCheckpointMismatch => ("restore checkpoint mismatch", "BR:Restore:ErrRestoreCheckpointMismatch"),
        ErrRestoreChecksumMismatch => ("restore checksum mismatch", "BR:Restore:ErrRestoreChecksumMismatch"),
        ErrRestoreTableIDMismatch => ("restore table ID mismatch", "BR:Restore:ErrRestoreTableIDMismatch"),
        ErrRestoreRejectStore => ("failed to restore remove rejected store", "BR:Restore:ErrRestoreRejectStore"),
        ErrRestoreNoPeer => ("region does not have peer", "BR:Restore:ErrRestoreNoPeer"),
        ErrRestoreSplitFailed => ("fail to split region", "BR:Restore:ErrRestoreSplitFailed"),
        ErrRestoreInvalidRewrite => ("invalid rewrite rule", "BR:Restore:ErrRestoreInvalidRewrite"),
        ErrRestoreInvalidBackup => ("invalid backup", "BR:Restore:ErrRestoreInvalidBackup"),
        ErrRestoreWriteAndIngest => ("failed to write and ingest", "BR:Restore:ErrRestoreWriteAndIngest"),
        ErrRestoreSchemaNotExists => ("schema not exists", "BR:Restore:ErrRestoreSchemaNotExists"),
        ErrRestoreNotFreshCluster => ("cluster is not fresh", "BR:Restore:ErrRestoreNotFreshCluster"),
        ErrRestoreIncompatibleSys => ("incompatible system table", "BR:Restore:ErrRestoreIncompatibleSys"),
        ErrUnsupportedSystemTable => ("the system table isn't supported for restoring yet", "BR:Restore:ErrUnsupportedSysTable"),
        ErrDatabasesAlreadyExisted => ("databases already existed in restored cluster", "BR:Restore:ErrDatabasesAlreadyExisted"),
        ErrTablesAlreadyExisted => ("tables already existed in restored cluster", "BR:Restore:ErrTablesAlreadyExisted"),
        ErrStreamLogTaskExist => ("stream task already exists", "BR:Stream:ErrStreamLogTaskExist"),
        ErrStreamLogTaskHasNoStorage => ("stream task has no storage", "BR:Stream:ErrStreamLogTaskHasNoStorage"),
        ErrRestoreRTsConstrain => ("resolved ts constrain violation", "BR:Restore:ErrRestoreResolvedTsConstrain"),
        ErrPiTRInvalidCDCLogFormat => ("invalid cdc log format", "BR:PiTR:ErrPiTRInvalidCDCLogFormat"),
        ErrPiTRTaskNotFound => ("task not found", "BR:PiTR:ErrTaskNotFound"),
        ErrPiTRInvalidTaskInfo => ("task info is invalid", "BR:PiTR:ErrInvalidTaskInfo"),
        ErrPiTRMalformedMetadata => ("malformed metadata", "BR:PiTR:ErrMalformedMetadata"),
        ErrPiTRCheckpointWatchRestart => ("checkpoint watch needs restart", "BR:PiTR:ErrCheckpointWatchRestart"),
        ErrStorageUnknown => ("unknown external storage error", "BR:ExternalStorage:ErrStorageUnknown"),
        ErrStorageInvalidConfig => ("invalid external storage config", "BR:ExternalStorage:ErrStorageInvalidConfig"),
        ErrStorageInvalidPermission => ("external storage permission", "BR:ExternalStorage:ErrStorageInvalidPermission"),
        ErrRestoreTotalKVMismatch => ("restore total tikvs mismatch", "BR:EBS:ErrRestoreTotalKVMismatch"),
        ErrRestoreInvalidPeer => ("restore met a invalid peer", "BR:EBS:ErrRestoreInvalidPeer"),
        ErrRestoreRegionWithoutPeer => ("restore met a region without any peer", "BR:EBS:ErrRestoreRegionWithoutPeer"),
        ErrKVStorage => ("tikv storage occur I/O error", "BR:KV:ErrKVStorage"),
        ErrKVUnknown => ("unknown error occur on tikv", "BR:KV:ErrKVUnknown"),
        ErrKVClusterIDMismatch => ("tikv cluster ID mismatch", "BR:KV:ErrKVClusterIDMismatch"),
        ErrKVNotLeader => ("not leader", "BR:KV:ErrKVNotLeader"),
        ErrKVNotTiKV => ("storage is not tikv", "BR:KV:ErrNotTiKVStorage"),
        ErrKVDiskFull => ("disk is full", "BR:KV:ErrKVDiskFull"),
        ErrKVEpochNotMatch => ("epoch not match", "BR:KV:ErrKVEpochNotMatch"),
        ErrKVKeyNotInRegion => ("key not in region", "BR:KV:ErrKVKeyNotInRegion"),
        ErrKVRewriteRuleNotFound => ("rewrite rule not found", "BR:KV:ErrKVRewriteRuleNotFound"),
        ErrKVRangeIsEmpty => ("range is empty", "BR:KV:ErrKVRangeIsEmpty"),
        ErrKVDownloadFailed => ("download sst failed", "BR:KV:ErrKVDownloadFailed"),
        ErrKVIngestFailed => ("ingest sst failed", "BR:KV:ErrKVIngestFailed"),
        ErrPossibleInconsistency => ("the cluster state might be inconsistent", "BR:KV:ErrPossibleInconsistency"),
    }
}
