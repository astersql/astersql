// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/rtree/logging_test.go`.
//! Pure formatting — no network / TiKV / storage.
//!
//! 对齐 Go `TestLogRanges`：验证 `ZapRanges` 在 0..3 全量、≥4 缩写、
//! 以及 1024 大列表边界下的 JSON 字段体与 Go 金标准一致。
//! 密钥用十进制 ASCII 构造，经 hex redact 后期望串含 `"30"` 等片段。

use crate::{KeyRange, ZapRanges};

/// Corresponds to Go table-driven cases in `TestLogRanges`.
///
/// `count` 为构造区间个数；`expect` 为 Go zap console 输出的 ranges 字段体。
struct LogRangesCase {
    count: usize,
    expect: &'static str,
}

// 构造半开区间，start/end 直接作为 KeyRange 字节，不做额外编码。
fn new_range(start: Vec<u8>, end: Vec<u8>) -> KeyRange {
    KeyRange {
        StartKey: start,
        EndKey: end,
    }
}

/// `TestLogRanges` → `test_log_ranges`.
///
/// Go encodes `ZapRanges` via zap console encoder; Rust encodes the returned
/// `Field` to the same JSON body (`{"ranges": [...]}`).
///
/// 表驱动覆盖空列表、短列表全展开、缩写阈值与千级规模；断言整串精确相等。
#[test]
fn test_log_ranges() {
    let cases = [
        // 空：仍输出带空数组的 ranges 字段。
        LogRangesCase {
            count: 0,
            expect: r#"{"ranges": []}"#,
        },
        // 单条：十进制 "0"/"1" hex 后为 30/31。
        LogRangesCase {
            count: 1,
            expect: r#"{"ranges": ["[30, 31)"]}"#,
        },
        LogRangesCase {
            count: 2,
            expect: r#"{"ranges": ["[30, 31)", "[31, 32)"]}"#,
        },
        // 3 条仍全展开；4 起才插入 skip。
        LogRangesCase {
            count: 3,
            expect: r#"{"ranges": ["[30, 31)", "[31, 32)", "[32, 33)"]}"#,
        },
        LogRangesCase {
            count: 4,
            expect: r#"{"ranges": ["[30, 31)", "(skip 2)", "[33, 34)"]}"#,
        },
        LogRangesCase {
            count: 5,
            expect: r#"{"ranges": ["[30, 31)", "(skip 3)", "[34, 35)"]}"#,
        },
        LogRangesCase {
            count: 6,
            expect: r#"{"ranges": ["[30, 31)", "(skip 4)", "[35, 36)"]}"#,
        },
        // 1024：尾键为 "1023"/"1024"，hex 拼接成 31303233/31303234。
        LogRangesCase {
            count: 1024,
            expect: r#"{"ranges": ["[30, 31)", "(skip 1022)", "[31303233, 31303234)"]}"#,
        },
    ];

    for cs in cases {
        let mut ranges = Vec::with_capacity(cs.count);
        for j in 0..cs.count {
            // Go: fmt.Appendf(nil, "%d", j) — decimal ASCII bytes (hex-redacted as "30"...).
            // 与 Go 一致用十进制字符串字节，而非二进制整数编码。
            ranges.push(new_range(
                format!("{j}").into_bytes(),
                format!("{}", j + 1).into_bytes(),
            ));
        }
        let out = ZapRanges(&ranges).encode_json();
        // 与 Go 金标准逐字相等，含空格与 skip 文案。
        assert_eq!(cs.expect, out, "count={}", cs.count);
    }
}
