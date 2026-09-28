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

// `errdef` 迁移单元测试：校验规范化错误消息/RFC code、磁盘满错误链判定与 HTTP 状态错误文本。

use super::*;
use std::error::Error;
use std::fmt;

/// 包装内层错误以构造 `source()` 链，用于测试 `IsKVDiskFullError` 的穿透行为。
#[derive(Debug)]
struct WrappedError {
    source: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for WrappedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outer: {}", self.source)
    }
}

impl Error for WrappedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// 静态错误的 Display 文本与 RFCCode 须与 Go 常量一致。
#[test]
fn normalized_errors_preserve_go_messages_and_rfc_codes() {
    let cases = [
        (
            &ErrKVEpochNotMatch,
            "epoch not match",
            "Ingest:EpochNotMatch",
        ),
        (&ErrKVNotLeader, "not leader", "Ingest:NotLeader"),
        (&ErrKVServerIsBusy, "server is busy", "Ingest:ServerIsBusy"),
        (
            &ErrKVRegionNotFound,
            "region not found",
            "Ingest:RegionNotFound",
        ),
        (
            &ErrKVReadIndexNotReady,
            "read index not ready",
            "Ingest:ReadIndexNotReady",
        ),
        (&ErrKVDiskFull, "store disk full", "Ingest:StoreDiskFull"),
        (
            &ErrKVIngestFailed,
            "ingest tikv failed",
            "Ingest:ErrKVIngestFailed",
        ),
        (
            &ErrKVRaftProposalDropped,
            "raft proposal dropped",
            "Ingest:ErrKVRaftProposalDropped",
        ),
    ];

    for (err, message, rfc_code) in cases {
        assert_eq!(err.to_string(), message);
        assert_eq!(err.RFCCode(), rfc_code);
    }
    // `%d` 模板展开与 Go GenWithStackByArgs 对齐。
    assert_eq!(
        ErrNoLeader.GenWithStackByArgs(42).to_string(),
        "region has no leader, region '42'"
    );
    assert_eq!(ErrNoLeader.RFCCode(), "KV:ErrNoLeader");
}

/// 磁盘满判定须识别同 RFC code 的实例，并沿 `source()` 链向上穿透。
#[test]
fn disk_full_detection_follows_the_error_chain_and_rfc_code() {
    assert!(IsKVDiskFullError(&ErrKVDiskFull));

    let same_class = NormalizedError::new("different detail", "Ingest:StoreDiskFull");
    assert!(IsKVDiskFullError(&same_class));

    let wrapped = WrappedError {
        source: Box::new(same_class),
    };
    assert!(IsKVDiskFullError(&wrapped));
    assert!(!IsKVDiskFullError(&ErrKVServerIsBusy));
}

/// HTTPStatusError 的 Error/Display 文本格式与 Go 一致。
#[test]
fn http_status_error_matches_go_error_text() {
    let err = HTTPStatusError {
        StatusCode: 503,
        Message: "service unavailable".to_owned(),
    };
    assert_eq!(
        err.Error(),
        "request failed with status code 503: service unavailable"
    );
    assert_eq!(err.to_string(), err.Error());
}
