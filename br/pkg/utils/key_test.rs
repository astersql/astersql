// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/key_test.go`.
//!
//! 覆盖 ParseKey 三种格式、CompareEndKey 空键+inf 语义、IntersectAll 区间相交，
//! 以及 FormatDate / meta 前缀判定与 Go 样例逐条对齐。
//! 断言失败信息带 case 编号或 ts，便于对照 Go 表驱动用例。
//! ParseKey 未知格式必须含 unknown format 子串，防止错误包装漂移。
//! CompareEndKey 空键用例是半开区间上界语义的回归锚点。
//! IntersectAll 正反调用后排序比较，捕获双指针推进不对称缺陷。
//! FormatDate 使用固定 +0800，避免本地时区污染断言。
//! 前缀测试覆盖 mDB/mDDLJobH/mD 边界，防止 starts_with 过短/过长。
//! r() 辅助不编码二进制键，夹具刻意使用可打印 ASCII。
//! hex 往返先 encode 再 Parse，确保字母表小写稳定。
//! escaped 用例含单位 hex（\x1），对齐 Go Sscanf 宽松行为。
//! 不在此测试 IntersectAll 的不可达分支日志。
//! 与 Go key_test.go 表驱动结构保持可对照。
//! 失败断言带 case#/ts，便于定位具体样例。
//! 本文件无异步与全局状态，可任意并行。
//! 修改实现后应优先跑本组而非扩大范围。
//! 空 end 与空 start 组合覆盖 +∞ 交叠。
//! 不验证 EncodeTxnMetaKey/IsMetaAutoIDKey（由其它用例覆盖时可扩展）。
//! 许可证与实现文件一致，保留 PingCAP 声明。

use crate::key::{
    CompareEndKey, FormatDate, IntersectAll, IsDBOrDDLJobHistoryKey, IsMetaDBKey,
    IsMetaDDLJobHistoryKey, ParseKey, hex_encode,
};
use crate::{KeyRange, KvKey};

/// 测试辅助：用 UTF-8 字面量快速构造 KeyRange。
fn r(a: &str, b: &str) -> KeyRange {
    KeyRange {
        StartKey: KvKey(a.as_bytes().to_vec()),
        EndKey: KvKey(b.as_bytes().to_vec()),
    }
}

#[test]
fn test_parse_key() {
    // raw：原样字节，不做转义。
    let test_raw_key = [
        ("1234", b"1234".as_slice()),
        ("abcd", b"abcd"),
        ("1a2b", b"1a2b"),
        ("AA", b"AA"),
        ("\u{7}", b"\x07"),
        ("\\'", b"\\'"),
    ];
    for (raw_key, ans) in test_raw_key {
        let parsed = ParseKey("raw", raw_key).expect("raw");
        assert_eq!(parsed, ans);
    }

    // escaped：对齐 Go unescapedKey 的 \a\xN\n 等序列。
    let test_escaped = [
        ("\\a\\x1", b"\x07\x01".as_slice()),
        ("\\b\\f", b"\x08\x0c"),
        ("\\n\\r", b"\n\r"),
        ("\\t\\v", b"\t\x0b"),
        ("\\'", b"'"),
    ];
    for (escaped, ans) in test_escaped {
        let parsed = ParseKey("escaped", escaped).expect("escaped");
        assert_eq!(parsed, ans);
    }

    // hex：先 encode 再 Parse，验证往返一致。
    let test_hex: &[(&[u8], &[u8])] = &[
        (b"1234", b"1234"),
        (b"abcd", b"abcd"),
        (b"1a2b", b"1a2b"),
        (b"AA", b"AA"),
        (b"\x07", b"\x07"),
        (b"\\'", b"\\'"),
        (b"\x01", b"\x01"),
        (b"\xaa", b"\xaa"),
    ];
    for (hex_key, ans) in test_hex {
        let key = hex_encode(hex_key);
        let parsed = ParseKey("hex", &key).expect("hex");
        assert_eq!(parsed, *ans);
    }

    // 未知 format 必须返回 unknown format，与 Go ErrInvalidArgument 文案一致。
    let test_not_support = [
        "1234", "abcd", "1a2b", "AA", "\u{7}", "\\'", "\u{1}", "\u{aa}",
    ];
    for any in test_not_support {
        let err = ParseKey("notSupport", any).unwrap_err();
        assert!(err.to_string().contains("unknown format"), "err={}", err);
    }
}

#[test]
fn test_parse_key_matches_go_scan_prefix_semantics() {
    // fmt.Sscanf consumes the two-character field but accepts a valid numeric prefix.
    assert_eq!(ParseKey("escaped", "\\x1z").expect("hex prefix"), b"\x01");
    assert_eq!(ParseKey("escaped", "\\1z").expect("octal prefix"), b"\x01");

    // Go ignores the hex Sscanf error, leaving c as the previously read backslash.
    assert_eq!(ParseKey("escaped", "\\x").expect("empty hex field"), b"\\");
}

#[test]
fn test_compare_end_key() {
    // 空切片视为 +∞，故 "" > "1"。
    let cases = [
        (b"1".as_slice(), b"2".as_slice(), -1),
        (b"1", b"1", 0),
        (b"2", b"1", 1),
        (b"1", b"", -1),
        (b"", b"", 0),
        (b"", b"1", 1),
    ];
    for (k1, k2, ans) in cases {
        assert_eq!(CompareEndKey(k1, k2), ans);
    }
}

#[test]
fn test_clamp_key_ranges() {
    // 表驱动：s1∩s2 与 s2∩s1 排序后应相同，覆盖空 end 与跨段裁剪。
    let cases = [
        (
            vec![r("0001", "0002"), r("0003", "0004"), r("0005", "0008")],
            vec![r("0001", "0004"), r("0006", "0008")],
            vec![r("0001", "0002"), r("0003", "0004"), r("0006", "0008")],
        ),
        (
            vec![r("0001", "0002"), r("00021", "0003"), r("0005", "0009")],
            vec![r("0001", "0004"), r("0005", "0008")],
            vec![r("0001", "0002"), r("00021", "0003"), r("0005", "0008")],
        ),
        (
            vec![r("0001", "0050"), r("0051", "0095"), r("0098", "0152")],
            vec![r("0001", "0100"), r("0150", "0200")],
            vec![
                r("0001", "0050"),
                r("0051", "0095"),
                r("0098", "0100"),
                r("0150", "0152"),
            ],
        ),
        (
            // clamp 侧 end 为空（+∞）时仍能裁出 0150..0152。
            vec![r("0001", "0050"), r("0051", "0095"), r("0098", "0152")],
            vec![r("0001", "0100"), r("0150", "")],
            vec![
                r("0001", "0050"),
                r("0051", "0095"),
                r("0098", "0100"),
                r("0150", "0152"),
            ],
        ),
        (
            // 被 clamp 侧 end 为空时，结果可延伸到 clamp 上界。
            vec![r("0001", "0050"), r("0051", "0095"), r("0098", "")],
            vec![r("0001", "0100"), r("0150", "0200")],
            vec![
                r("0001", "0050"),
                r("0051", "0095"),
                r("0098", "0100"),
                r("0150", "0200"),
            ],
        ),
        (vec![r("", "0050")], vec![r("", "")], vec![r("", "0050")]),
    ];

    for (i, (ranges, clamp_in, result)) in cases.into_iter().enumerate() {
        let mut a = IntersectAll(ranges.clone(), clamp_in.clone());
        let mut b = IntersectAll(clamp_in, ranges);
        a.sort_by(|x, y| x.StartKey.as_ref().cmp(y.StartKey.as_ref()));
        b.sort_by(|x, y| x.StartKey.as_ref().cmp(y.StartKey.as_ref()));
        let mut expected = result;
        expected.sort_by(|x, y| x.StartKey.as_ref().cmp(y.StartKey.as_ref()));
        assert_eq!(a, expected, "case #{i} forward");
        assert_eq!(b, expected, "case #{i} reverse");
    }
}

#[test]
fn test_date_format() {
    // TSO>>18 得毫秒，再转纳秒；偏移 +0800 对齐 Go 样例墙钟。
    let cases = [
        (434604259287760897u64, "2022-07-15 19:14:39.534 +0800"),
        (434605479096221697, "2022-07-15 20:32:12.734 +0800"),
        // 整秒省略小数部分，对齐 Go `.999999999` 去尾零。
        (434605478903808000, "2022-07-15 20:32:12 +0800"),
    ];
    for (ts, target) in cases {
        let unix_ms = (ts >> 18) as i128;
        let unix_nanos = unix_ms * 1_000_000;
        let date = FormatDate(unix_nanos, 8 * 3600);
        assert_eq!(date, target, "ts={ts}");
    }
}

#[test]
fn test_date_format_before_unix_epoch() {
    assert_eq!(FormatDate(-1, 0), "1969-12-31 23:59:59.999999999 +0000");
}

#[test]
fn test_prefix() {
    // 前缀判定与 TiDB meta 键约定一致：mDB / mDDLJobH / mD。
    assert!(IsMetaDBKey(b"mDBs"));
    assert!(!IsMetaDBKey(b"mDDL"));
    assert!(IsMetaDDLJobHistoryKey(b"mDDLJobHistory"));
    assert!(!IsMetaDDLJobHistoryKey(b"mDDL"));
    assert!(IsDBOrDDLJobHistoryKey(b"mDL"));
    assert!(IsDBOrDDLJobHistoryKey(b"mDB:"));
    assert!(IsDBOrDDLJobHistoryKey(b"mDDLHistory"));
    assert!(!IsDBOrDDLJobHistoryKey(b"DDL"));
}
