// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 执行计划编解码（plancodec）单元测试。
//
// 覆盖 `EncodeTaskType` 与归一化计划中的 task 字段往返，以及过长计划丢弃哨兵
// `PlanDiscardedEncoded` 的解码文案。执行计划（plan）是优化器产出的算子树。

use plancodec_dependency::kv::StoreType;
use plancodec_dependency::{EncodeTaskType, PlanDiscardedEncoded};

/// `EncodeTaskType` 往返用例：root/cop 与存储类型编码。
struct EncodeTaskTypeCase {
    is_root: bool,
    store_type: StoreType,
    encoded: &'static str,
    decoded: &'static str,
}

/// 把编码后的 task 字段塞进一行归一化计划，再解码取出可读 task 名。
fn decode_task_type_through_plan(encoded: &str) -> Result<String, plancodec_dependency::Error> {
    let plan = DecodeNormalizedPlan(&format!("0\t1\t{encoded}\toperator info"))?;
    Ok(plan.split('\t').nth(2).unwrap_or_default().to_owned())
}

/// root→`0`；cop 附带 store 类型 → `1_<store>`；无 store 后缀时显示 `cop`。
#[test]
fn test_encode_task_type() {
    let cases = [
        EncodeTaskTypeCase {
            is_root: true,
            store_type: StoreType::UnSpecified,
            encoded: "0",
            decoded: "root",
        },
        EncodeTaskTypeCase {
            is_root: false,
            store_type: StoreType::TiKV,
            encoded: "1_0",
            decoded: "cop[tikv]",
        },
        EncodeTaskTypeCase {
            is_root: false,
            store_type: StoreType::TiFlash,
            encoded: "1_1",
            decoded: "cop[tiflash]",
        },
        EncodeTaskTypeCase {
            is_root: false,
            store_type: StoreType::TiDB,
            encoded: "1_2",
            decoded: "cop[tidb]",
        },
    ];

    for case in cases {
        assert_eq!(EncodeTaskType(case.is_root, case.store_type), case.encoded);
        assert_eq!(
            decode_task_type_through_plan(case.encoded).unwrap(),
            case.decoded
        );
    }

    assert_eq!(decode_task_type_through_plan("1").unwrap(), "cop");
    assert!(decode_task_type_through_plan("1_x").is_err());
}

/// 文本计划丢弃哨兵解码为固定可读提示。
#[test]
fn test_decode_discard_plan() {
    assert_eq!(
        DecodePlan(PlanDiscardedEncoded).unwrap(),
        "(plan discarded because too long)"
    );
}

#[test]
fn base64_nonzero_padding_bits_match_go() {
    use plancodec_dependency::Decompress;
    // Snappy empty block is 0x00. Go StdEncoding ignores unused low bits.
    for encoded in ["AA==", "AB==", "AP==", "A\rB=\n="] {
        assert_eq!(Decompress(encoded).unwrap(), b"");
    }
    for encoded in ["AA", "AA=", "AA===", "AA== "] {
        assert!(Decompress(encoded).is_err());
    }
}

#[test]
fn signed_depth_matches_go_atoi_and_panic_boundary() {
    use plancodec_dependency::{Compress, Error};
    assert_eq!(
        DecodeNormalizedPlan("-0\t1\t0").unwrap(),
        "\tSelection\troot"
    );
    assert!(matches!(
        DecodePlan(&Compress(b"-1\t1\t0")),
        Err(Error::DecodePlanPanicked)
    ));
    assert!(std::panic::catch_unwind(|| DecodeNormalizedPlan("-1\t1\t0")).is_err());
    assert_eq!(
        DecodeNormalizedPlan("0\t1\t0").unwrap(),
        "\tSelection\troot"
    );
}

#[test]
fn tree_alignment_and_concurrent_decoder_reuse() {
    let input = "ignored line\n0\t1\t0\t根\n1\t10\t1_0\ta\n1\t13\t1_1\tb";
    let expected = "\tSelection  \troot        \t根\n\t├─TableScan\tcop[tikv]   \ta\n\t└─IndexScan\tcop[tiflash]\tb";
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..20 {
                    assert_eq!(DecodeNormalizedPlan(input).unwrap(), expected);
                    assert!(DecodeNormalizedPlan("bad\t1").is_err());
                    assert_eq!(
                        DecodePlan(PlanDiscardedEncoded).unwrap(),
                        "(plan discarded because too long)"
                    );
                    assert_eq!(DecodeNormalizedPlan("\nignored").unwrap(), "");
                }
            });
        }
    });
}

#[test]
fn decoded_plan_preserves_go_string_bytes() {
    use plancodec_dependency::{Compress, Decompress};
    // Go string retains arbitrary operator-info bytes.
    let bytes = b"0\t1\t0\t\xff";
    let encoded = Compress(bytes);
    assert_eq!(Decompress(&encoded).unwrap(), bytes);
    assert!(
        plancodec_dependency::DecodePlan(&encoded)
            .unwrap()
            .ends_with(b"\xff")
    );
}

#[test]
fn malformed_plan_errors_match_go() {
    for (input, expected) in [
        (
            "x\t1",
            "decode plan: x\t1, depth: x, error: strconv.Atoi: parsing \"x\": invalid syntax",
        ),
        (
            "0\tx",
            "decode plan: 0\tx, plan id: x, error: strconv.Atoi: parsing \"x\": invalid syntax",
        ),
        (
            "0\t1\t1_x",
            "decode plan: 0\t1\t1_x, task type: 1_x, error: strconv.Atoi: parsing \"x\": invalid syntax",
        ),
    ] {
        assert_eq!(
            DecodeNormalizedPlan(input).unwrap_err().to_string(),
            expected
        );
    }
    assert_eq!(
        plancodec_dependency::Decompress("AA=")
            .unwrap_err()
            .to_string(),
        "illegal base64 data at input byte 3"
    );
    assert_eq!(
        plancodec_dependency::Decompress("")
            .unwrap_err()
            .to_string(),
        "snappy: corrupt input"
    );
}

// These fixtures contain UTF-8; conversion is asserted, never lossy.
fn DecodePlan(input: &str) -> Result<String, plancodec_dependency::Error> {
    plancodec_dependency::DecodePlan(input).map(|bytes| String::from_utf8(bytes).unwrap())
}
fn DecodeNormalizedPlan(input: &str) -> Result<String, plancodec_dependency::Error> {
    plancodec_dependency::DecodeNormalizedPlan(input).map(|bytes| String::from_utf8(bytes).unwrap())
}

#[test]
fn encoding_and_normalization_preserve_bytes_and_platform_depth() {
    use plancodec_dependency::{EncodePlanNode, NormalizePlanNode, TypeSel};
    let mut encoded = Vec::new();
    let depth = i32::MAX as isize + 1;
    EncodePlanNode(
        depth,
        b"p\xff",
        TypeSel,
        -0.0,
        "0",
        b"x\xff\t\n",
        "",
        "",
        "",
        "",
        &mut encoded,
    );
    let mut expected = format!("{depth}\t1_p").into_bytes();
    expected.extend_from_slice(b"\xff\t0\t-0\tx\xff\\t\\n\n");
    assert_eq!(encoded, expected);
    encoded.clear();
    NormalizePlanNode(0, TypeSel, "0", b"\xff", &mut encoded);
    assert_eq!(encoded, b"0\t1\t0\t\xff\n");
    assert_eq!(
        plancodec_dependency::DecodeNormalizedPlan(&encoded).unwrap(),
        b"\tSelection\troot\t\xff"
    );
    assert_eq!(
        plancodec_dependency::DecodeNormalizedPlan(b"0\t1_\xff\t0").unwrap(),
        b"\tSelection_\xff\troot"
    );
}

#[test]
fn errors_preserve_raw_bytes_and_go_integer_ranges() {
    let error = plancodec_dependency::DecodeNormalizedPlan(b"\xff\t1").unwrap_err();
    assert_eq!(error.as_bytes().as_ref(), b"decode plan: \xff\t1, depth: \xff, error: strconv.Atoi: parsing \"\\xff\": invalid syntax");
    for (number, reason) in [
        ("9223372036854775808", "value out of range"),
        ("-9223372036854775809", "value out of range"),
        ("9223372036854775808x", "invalid syntax"),
    ] {
        let plan = format!("0\t{number}");
        assert_eq!(
            DecodeNormalizedPlan(&plan).unwrap_err().to_string(),
            format!(
                "decode plan: {plan}, plan id: {number}, error: strconv.Atoi: parsing \"{number}\": {reason}"
            )
        );
    }
    assert_eq!(
        DecodeNormalizedPlan("0\t-9223372036854775808").unwrap(),
        "\tUnknownPlanID-9223372036854775808"
    );
}

#[test]
fn snappy_accepts_go_uvarint_header() {
    use base64::Engine;
    // Go binary.Uvarint permits a non-minimal length header up to ten bytes.
    for width in 1..=10 {
        let mut header = vec![0x80; width - 1];
        header.push(0);
        let encoded = base64::engine::general_purpose::STANDARD.encode(header);
        assert_eq!(plancodec_dependency::Decompress(encoded).unwrap(), b"");
    }
}

#[test]
fn encoding_accepts_all_go_store_and_plan_type_values() {
    use plancodec_dependency::{EncodeTaskTypeForNormalize, NormalizePlanNode};
    for store in 0..=u8::MAX {
        assert_eq!(EncodeTaskType(false, store), format!("1_{store}"));
        assert_eq!(EncodeTaskType(true, store), "0");
        assert_eq!(
            EncodeTaskTypeForNormalize(false, store),
            if store == 0 {
                "1".to_owned()
            } else {
                format!("1_{store}")
            }
        );
    }
    let mut output = Vec::new();
    NormalizePlanNode(isize::MAX, b"\xff", b"0", b"\xff", &mut output);
    assert_eq!(
        output,
        [isize::MAX.to_string().as_bytes(), b"\t0\t0\t\xff\n"].concat()
    );
}

#[test]
fn error_quotes_use_go_unicode_version() {
    // U+31E4 was assigned after the Unicode version used by the Go codec.
    let input = "\u{31e4}\t1";
    assert_eq!(
        DecodeNormalizedPlan(input).unwrap_err().to_string(),
        format!(
            "decode plan: {input}, depth: \u{31e4}, error: strconv.Atoi: parsing \"\\u31e4\": invalid syntax"
        )
    );
}
