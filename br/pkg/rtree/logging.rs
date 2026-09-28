// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! Logging helpers matching `br/pkg/rtree/logging.go`.
//!
//! 为 `KeyRange` 提供脱敏 `Display`，并实现 `ZapRanges`：
//! 返回与 Go `logutil.AbbreviatedStringers("ranges", ranges)` 等价的日志 `Field`。
//! 约束：密钥一律 hex 展示，避免日志泄漏明文；长度 ≥4 时只保留首尾并插入 skip。

use astersql_br_pkg_logutil::{AbbreviatedStringers, Field};

use crate::rtree::KeyRange;

// 本地 redact：等价 tidb/pkg/util/redact.Key，用稳定 hex 便于断言。
fn redact_key(key: &[u8]) -> String {
    // Stand-in for tidb/pkg/util/redact.Key: hex-encode for stable Display.
    key.iter().map(|b| format!("{b:02x}")).collect()
}

// 半开区间 `[start, end)`，两端均经 redact，与 Go Stringer 形态一致。
impl std::fmt::Display for KeyRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}, {})",
            redact_key(&self.StartKey),
            redact_key(&self.EndKey)
        )
    }
}

/// ZapRanges matches Go `logutil.AbbreviatedStringers("ranges", ranges)`.
/// 少于 4 条时全量编码；否则只保留首尾，中间以 `(skip N)` 占位。
pub fn ZapRanges(ranges: &[KeyRange]) -> Field {
    AbbreviatedStringers("ranges", ranges.to_vec())
}
