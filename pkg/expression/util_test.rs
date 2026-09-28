// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `util` 模块的基础单元测试。
//
// 验证 `Filter`/`FilterOutInPlace` 的顺序契约，以及数值前缀、二进制时间、
// 容量格式化、时区与校对定位等与 Go 对齐的边界行为。

use crate::util_kernel::*;
use crate::*;

/// 判断表达式是否为值为 NULL 的常量节点。
fn is_null_constant(expression: &dyn Expression) -> bool {
    expression
        .as_any()
        .downcast_ref::<Constant>()
        .is_some_and(|constant| constant.Value.IsNull())
}

/// 校验 Filter 保序、FilterOutInPlace 逆序移除并拆分 kept/removed。
#[test]
fn filter_and_filter_out_preserve_go_ordering_contract() {
    let input: Vec<ExprBox> = vec![
        Box::new(NewOne()),
        Box::new(NewNull()),
        Box::new(NewSignedZero()),
        Box::new(NewNull()),
    ];
    let filtered = Filter(Vec::new(), &input, is_null_constant);
    assert_eq!(filtered.len(), 2);
    assert!(
        filtered
            .iter()
            .all(|expression| is_null_constant(expression.as_ref()))
    );

    let (kept, removed) = FilterOutInPlace(input, is_null_constant);
    assert_eq!(kept.len(), 2);
    assert_eq!(removed.len(), 2);
    assert!(
        kept.iter()
            .all(|expression| !is_null_constant(expression.as_ref()))
    );
    assert!(
        removed
            .iter()
            .all(|expression| is_null_constant(expression.as_ref()))
    );
}

/// 校验进制前缀截取：符号、非法进制与仅符号输入。
#[test]
fn valid_numeric_prefix_handles_sign_radix_and_invalid_radix_boundaries() {
    assert_eq!(getValidPrefix("+1fZ", 16), "1f");
    assert_eq!(getValidPrefix("-1012", 2), "-101");
    assert_eq!(getValidPrefix("z!", 36), "z");
    assert_eq!(getValidPrefix("-", 10), "");
    assert_eq!(getValidPrefix("123", 1), "");
    assert_eq!(getValidPrefix("123", 37), "");
}

/// 校验日期/带时区时间戳/负时长的二进制解码偏移与文本。
#[test]
fn binary_temporal_helpers_preserve_offsets_fraction_and_negative_duration() {
    let date = [0xe8, 0x07, 0x0c, 0x1f];
    assert_eq!(binaryDate(0, &date), (4, "2024-12-31".to_owned()));

    let timestamp_with_tz = [
        0xe8, 0x07, 0x0c, 0x1f, 0x17, 0x3b, 0x3a, 0x40, 0xe2, 0x01, 0x00, 0x3e, 0xfe,
    ];
    assert_eq!(
        binaryTimestampWithTZ(0, &timestamp_with_tz),
        (13, "2024-12-31 23:59:58.123456-7:30".to_owned())
    );

    let duration = [1, 2, 0, 0, 0, 3, 4, 5, 0x40, 0xe2, 0x01, 0x00];
    assert_eq!(
        binaryDurationWithMS(1, &duration, duration[0]),
        (12, "-2 03:04:05.123456".to_owned())
    );
}

/// 校验格式化阈值、时区秒偏移与空 needle 定位。
#[test]
fn format_timezone_and_collation_helpers_cover_thresholds_and_empty_needles() {
    assert_eq!(GetFormatBytes(0.0), "0 bytes");
    assert_eq!(GetFormatBytes(1024.0), "1.00 KiB");
    assert_eq!(GetFormatNanoTime(1_000.0), "1.00 us");
    assert_eq!(GetFormatNanoTime(60_000_000_000.0), "1.00 min");

    assert_eq!(timeZone2int("+13:00"), 46_800);
    assert_eq!(timeZone2int("-05:30"), -19_800);
    assert_eq!(locateStringWithCollation("abc", "", "utf8mb4_bin"), 1);
}

/// Go strconv.FormatFloat preserves a signed, two-digit exponent and spells infinities as Inf.
#[test]
fn format_helpers_match_go_scientific_notation_and_special_values() {
    assert_eq!(GetFormatBytes(100_000.0 * eib), "1.00e+05 EiB");
    assert_eq!(GetFormatNanoTime(-100_000.0 * dayTime), "-1.00e+05 d");
    assert_eq!(GetFormatBytes(f64::INFINITY), "+Inf EiB");
    assert_eq!(GetFormatNanoTime(f64::NEG_INFINITY), "-Inf d");
}
