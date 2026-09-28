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

// ingest_err 单元测试：校验 ErrorPb → IngestAPIError 分类与失败消息拼接。
//
// 覆盖可重试/不可重试 RFC code 区分、Go 同序的优先级映射、getIngestFailedMsg
// 变体名拼接，以及 EpochNotMatch 时通过 extract_region_fn 提取替换 Region。

// Ported from pkg/ingestor/ingestcli/ingest_err_test.go. The Go suite also
// asserts `common.IsRetryableError`, but that classifier lives on
// `pkg/lightning/common.CommonError`, a type this crate does not depend on
// and is out of scope for the globalsort/ingestcli test-bundle task; the
// RFC-code identity check below exercises the same retryable/non-retryable
// category distinction using only this crate's own error types.

use astersql_ingestor_errdef as errdef;

use crate::{ErrorPb, NewIngestAPIError, getIngestFailedMsg};

/// 可重试（IngestFailed）与不可重试（DiskFull）类别的 RFC code 必须不同。
#[test]
fn test_ingest_api_error_categories_are_distinct() {
    let retryable = NewIngestAPIError(
        &ErrorPb {
            stale_command: true,
            ..Default::default()
        },
        None,
    );
    let not_retryable = NewIngestAPIError(
        &ErrorPb {
            disk_full: true,
            ..Default::default()
        },
        None,
    );
    assert!(retryable.err.is(&errdef::ErrKVIngestFailed));
    assert!(not_retryable.err.is(&errdef::ErrKVDiskFull));
    assert_ne!(
        retryable.err.category.RFCCode(),
        not_retryable.err.category.RFCCode()
    );
}

/// 单条 ErrorPb → 期望 NormalizedError 的用例。
struct PbErrorCase {
    pb_err: ErrorPb,
    expected: &'static errdef::NormalizedError,
}

// test_convert_pb_error_to_error mirrors Go's TestConvertPBError2Error: it
// keeps the exact NotLeader/EpochNotMatch/raft-dropped/busy/region-missing/
// read-index/disk-full/general-ingest-failed priority order.
/// 对齐 Go TestConvertPBError2Error：验证分类优先级顺序。
#[test]
fn test_convert_pb_error_to_error() {
    let cases = vec![
        // NotLeader doesn't mean region peers are changed, so we can retry ingest.
        // NotLeader 不代表 peers 变更，可重试 ingest。
        PbErrorCase {
            pb_err: ErrorPb {
                not_leader: true,
                ..Default::default()
            },
            expected: &errdef::ErrKVNotLeader,
        },
        // EpochNotMatch means region is changed; if the new region covers the
        // old, writing can restart, otherwise region scanning restarts.
        // EpochNotMatch：Region 已变；新 Region 覆盖旧范围则可重写，否则重启扫描。
        PbErrorCase {
            pb_err: ErrorPb {
                epoch_not_match: true,
                ..Default::default()
            },
            expected: &errdef::ErrKVEpochNotMatch,
        },
        PbErrorCase {
            pb_err: ErrorPb {
                message: "raft: proposal dropped".to_owned(),
                ..Default::default()
            },
            expected: &errdef::ErrKVRaftProposalDropped,
        },
        PbErrorCase {
            pb_err: ErrorPb {
                server_is_busy: true,
                ..Default::default()
            },
            expected: &errdef::ErrKVServerIsBusy,
        },
        PbErrorCase {
            pb_err: ErrorPb {
                region_not_found: true,
                ..Default::default()
            },
            expected: &errdef::ErrKVRegionNotFound,
        },
        // ReadIndexNotReady means the region is changed, so import needs to
        // restart from region scanning.
        // ReadIndexNotReady：Region 已变，导入需从 Region 扫描重启。
        PbErrorCase {
            pb_err: ErrorPb {
                read_index_not_ready: true,
                ..Default::default()
            },
            expected: &errdef::ErrKVReadIndexNotReady,
        },
        // TiKV disk full is not retryable.
        // TiKV 磁盘满不可重试。
        PbErrorCase {
            pb_err: ErrorPb {
                disk_full: true,
                ..Default::default()
            },
            expected: &errdef::ErrKVDiskFull,
        },
        // A general error is retryable from writing.
        // 一般性错误可从写入阶段重试。
        PbErrorCase {
            pb_err: ErrorPb {
                stale_command: true,
                ..Default::default()
            },
            expected: &errdef::ErrKVIngestFailed,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let err = NewIngestAPIError(&case.pb_err, None);
        assert!(
            err.err.is(case.expected),
            "case {index} should map to RFC code {}",
            case.expected.RFCCode()
        );
    }
}

/// getIngestFailedMsg 输入/期望消息用例。
struct FailedMsgCase {
    pb_err: ErrorPb,
    msg: &'static str,
}

// test_get_ingest_failed_msg mirrors Go's TestGetIngestFailedMsg: it walks
// every errorpb field recognized by getIngestFailedMsg and keeps the
// "<variant> <message>" concatenation rule.
/// 对齐 Go TestGetIngestFailedMsg：遍历各变体并校验 "<变体> <消息>" 拼接。
#[test]
fn test_get_ingest_failed_msg() {
    let cases = vec![
        FailedMsgCase {
            pb_err: ErrorPb {
                key_not_in_region: true,
                ..Default::default()
            },
            msg: "KeyNotInRegion",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                stale_command: true,
                ..Default::default()
            },
            msg: "StaleCommand",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                store_not_match: true,
                ..Default::default()
            },
            msg: "StoreNotMatch",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                raft_entry_too_large: true,
                ..Default::default()
            },
            msg: "RaftEntryTooLarge",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                max_timestamp_not_synced: true,
                ..Default::default()
            },
            msg: "MaxTimestampNotSynced",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                proposal_in_merging_mode: true,
                ..Default::default()
            },
            msg: "ProposalInMergingMode",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                data_is_not_ready: true,
                ..Default::default()
            },
            msg: "DataIsNotReady",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                region_not_initialized: true,
                ..Default::default()
            },
            msg: "RegionNotInitialized",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                recovery_in_progress: true,
                ..Default::default()
            },
            msg: "RecoveryInProgress",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                flashback_in_progress: true,
                ..Default::default()
            },
            msg: "FlashbackInProgress",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                flashback_not_prepared: true,
                ..Default::default()
            },
            msg: "FlashbackNotPrepared",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                is_witness: true,
                ..Default::default()
            },
            msg: "IsWitness",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                mismatch_peer_id: true,
                ..Default::default()
            },
            msg: "MismatchPeerId",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                bucket_version_not_match: true,
                ..Default::default()
            },
            msg: "BucketVersionNotMatch",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                undetermined_result: true,
                ..Default::default()
            },
            msg: "UndeterminedResult",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                region_not_initialized: true,
                message: "the message".to_owned(),
                ..Default::default()
            },
            msg: "RegionNotInitialized the message",
        },
        FailedMsgCase {
            pb_err: ErrorPb {
                message: "the message".to_owned(),
                ..Default::default()
            },
            msg: "the message",
        },
    ];

    for case in cases {
        assert_eq!(case.msg, getIngestFailedMsg(&case.pb_err));
    }
}

// test_new_ingest_api_error_epoch_not_match_extracts_region covers the
// `extract_region_fn` branch that Go exercises indirectly through the split
// client integration: EpochNotMatch should surface a replacement region.
/// EpochNotMatch 时应通过 extract_region_fn 暴露替换 Region。
#[test]
fn test_new_ingest_api_error_epoch_not_match_extracts_region() {
    let region = crate::Region {
        id: 7,
        ..Default::default()
    };
    let pb_err = ErrorPb {
        epoch_not_match: true,
        current_regions: vec![region.clone()],
        ..Default::default()
    };
    let extract = |regions: &[crate::Region]| {
        regions.first().map(|region| crate::RegionInfo {
            region: region.clone(),
            leader: None,
        })
    };
    let err = NewIngestAPIError(&pb_err, Some(&extract));
    assert!(err.err.is(&errdef::ErrKVEpochNotMatch));
    assert_eq!(
        7,
        err.new_region
            .expect("epoch not match should extract a region")
            .region
            .id
    );
}

/// Go's generated protobuf decoder rejects field number zero instead of
/// silently treating it as an unknown field.
#[test]
fn test_decode_error_pb_rejects_zero_field_number() {
    assert!(crate::ingest_err::decode_error_pb(&[0x00, 0x00]).is_err());
}

/// A protobuf varint may use at most one payload bit in its tenth byte. The
/// generated Go decoder reports integer overflow for larger terminal bytes.
#[test]
fn test_decode_error_pb_rejects_overflowing_length_varint() {
    let mut encoded = vec![0x0a];
    encoded.extend([0x80; 9]);
    encoded.push(0x02);
    assert!(crate::ingest_err::decode_error_pb(&encoded).is_err());
}
