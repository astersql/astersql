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

//! Go-equivalent coverage for `FuzzParseBackupMetaFileNameRoundTrip`.
//!
//! 备份元数据文件名解析往返测试：对齐 Go `FuzzParseBackupMetaFileNameRoundTrip`。
//! 覆盖旧四段十六进制格式与带 tag 的新格式（含 store id、flags、DDL 标记）。
//! 断言依据：`ParseName` 对字段、HasDDLFiles/HasFlags 及缺 tag 错误文案与 Go 一致。
//! 不修改解析实现，仅验证 `astersql_br_pkg_stream_backupmetas` 行为。

// legacy 格式小写十六进制；tagged 格式大写并含 store id。
// HasDDLFiles：flags 缺省或 0 为真，flags=1 为假。
// 缺 tag 错误文案使用 Debug 字符形式（如 'd'）。
// extra_tag 撞保留字时回落 x，保证 tagged 用例可解析。
// 固定元组集覆盖零值、MAX、数字 tag 与保留 tag。
// 不启用真实 libFuzzer，仅用确定性用例逼近 Go fuzz 覆盖。
// ParseName 失败路径用 unwrap_err 断言，不用 panic 字符串匹配以外条件。
// tagged 串中零值额外 tag 段验证解析器忽略未知 tag。
use astersql_br_pkg_stream_backupmetas::{
    NAME_FLAGS_TAG, NAME_MAX_TS_TAG, NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG, NAME_MIN_TS_TAG,
    ParseName, ParsedName,
};

// 额外 tag 须为 ASCII 字母数字，且不能与保留 tag 冲突。
// 与 Go fuzz 生成器过滤规则一致，保证构造出的文件名可被 ParseName 接受。
fn is_ascii_alphanumeric(ch: u8) -> bool {
    ch.is_ascii_alphanumeric()
}

// 单组模糊用例：legacy / tagged / flags / 缺 tag 四类路径。
// 参数覆盖时间戳字段与 store id；extra_tag 模拟非保留扩展段。
fn fuzz_case(
    flush_ts: u64,
    store_id: u64,
    min_begin: u64,
    min_ts: u64,
    max_ts: u64,
    mut extra_tag: u8,
) {
    // 与 Go fuzz 相同：非法或保留 tag 回落到 'x'，避免污染合法字段解析。
    if !is_ascii_alphanumeric(extra_tag)
        || extra_tag == NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG
        || extra_tag == NAME_MIN_TS_TAG
        || extra_tag == NAME_MAX_TS_TAG
        || extra_tag == NAME_FLAGS_TAG
    {
        extra_tag = b'x';
    }

    // 旧格式：flush-minBegin-min-max，无 store id；其余字段保持默认。
    let legacy = format!("{flush_ts:016x}-{min_begin:016x}-{min_ts:016x}-{max_ts:016x}");
    let legacy_parsed = ParseName(&legacy).unwrap();
    assert_eq!(
        legacy_parsed,
        ParsedName {
            FlushTS: flush_ts,
            MinBeginTsInDefaultCf: min_begin,
            MinTS: min_ts,
            MaxTS: max_ts,
            ..Default::default()
        }
    );

    // 新格式：flush+store + 任意额外 tag + d/u/l 必选 tag；无 flags 时默认含 DDL。
    // 十六进制用大写，与 Go `fmt.Sprintf("%016X", …)` 输出一致。
    let tagged = format!(
        "{flush_ts:016X}{store_id:016X}-{extra}{zero:016X}d{min_begin:016X}u{max_ts:016X}l{min_ts:016X}",
        extra = extra_tag as char,
        zero = 0u64
    );
    let tagged_parsed = ParseName(&tagged).unwrap();
    assert_eq!(
        tagged_parsed,
        ParsedName {
            FlushTS: flush_ts,
            StoreID: store_id,
            MinBeginTsInDefaultCf: min_begin,
            MinTS: min_ts,
            MaxTS: max_ts,
            ..Default::default()
        }
    );
    // flags 缺省为 0 → HasDDLFiles 为真。
    assert!(tagged_parsed.HasDDLFiles());

    // flags=1 显式关闭 DDL 文件标记。
    // `p` 为 NAME_FLAGS_TAG；HasFlags 须为真且 HasDDLFiles 为假。
    let tagged_no_ddl = format!(
        "{flush_ts:016X}{store_id:016X}-d{min_begin:016X}u{max_ts:016X}l{min_ts:016X}p{flags:016X}",
        flags = 1u64
    );
    let no_ddl = ParseName(&tagged_no_ddl).unwrap();
    assert_eq!(
        no_ddl,
        ParsedName {
            FlushTS: flush_ts,
            StoreID: store_id,
            MinBeginTsInDefaultCf: min_begin,
            MinTS: min_ts,
            MaxTS: max_ts,
            Flags: 1,
            HasFlags: true,
        }
    );
    assert!(!no_ddl.HasDDLFiles());

    // flags=0 与缺省等价，仍应报告含 DDL。
    let tagged_default_ddl = format!(
        "{flush_ts:016X}{store_id:016X}-d{min_begin:016X}u{max_ts:016X}l{min_ts:016X}p{flags:016X}",
        flags = 0u64
    );
    assert!(ParseName(&tagged_default_ddl).unwrap().HasDDLFiles());

    // 依次省略 d/u/l 中一个必选 tag，错误信息需点名缺失 tag。
    // 与 Go 错误文案 `missing 'x' tag` 对齐，防止静默吞掉缺字段。
    let tag_order = [
        NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG,
        NAME_MAX_TS_TAG,
        NAME_MIN_TS_TAG,
    ];
    let tag_values = [
        (NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG, min_begin),
        (NAME_MIN_TS_TAG, min_ts),
        (NAME_MAX_TS_TAG, max_ts),
    ];
    for missing in tag_order {
        let mut segments = Vec::new();
        for tag in tag_order {
            if tag == missing {
                continue;
            }
            let val = tag_values.iter().find(|(t, _)| *t == tag).unwrap().1;
            segments.push(format!("{}{val:016X}", tag as char));
        }
        let name = format!("{flush_ts:016X}{store_id:016X}-{}", segments.join(""));
        let err = ParseName(&name).unwrap_err();
        assert!(
            err.contains(&format!("missing {:?} tag", missing as char)),
            "err={err}"
        );
    }
}

/// 用固定种子驱动 `fuzz_case`，对齐 Go 模糊测试的确定性子集。
#[test]
fn fuzz_parse_backup_meta_file_name_round_trip() {
    // 固定种子集：常规值、全零、边界 MAX、数字 extra_tag、以及撞到保留 tag 的回落路径。
    for (a, b, c, d, e, f) in [
        (10u64, 11u64, 5u64, 10u64, 30u64, b'x'),
        (0, 0, 0, 0, 0, b'a'),
        (u64::MAX, 1, 2, 3, 4, b'Z'),
        (1, 2, 3, 4, 5, b'0'),
        (100, 200, 50, 60, 70, NAME_MIN_TS_TAG),
    ] {
        fuzz_case(a, b, c, d, e, f);
    }
}
