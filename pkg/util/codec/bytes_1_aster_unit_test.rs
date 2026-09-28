// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Bytes 编解码与 key/value 往返的 Aster 补充单元测试。
//
// 覆盖 memcomparable（可字典序比较）正序/倒序向量、compact bytes 前缀保留，
// 以及 Datum key 有序性与 Decimal 编解码精度。

use super::*;

/// 对照 Go 向量验证 Encode/Decode 往返，并拒绝畸形 group。
#[test]
fn bytes_codec_matches_go_vectors_and_rejects_malformed_groups() {
    let cases = [
        (vec![], vec![0, 0, 0, 0, 0, 0, 0, 0, 247]),
        (vec![0], vec![0, 0, 0, 0, 0, 0, 0, 0, 248]),
        (vec![1, 2, 3], vec![1, 2, 3, 0, 0, 0, 0, 0, 250]),
        (
            vec![1, 2, 3, 4, 5, 6, 7, 8],
            vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 0, 0, 0, 0, 0, 0, 0, 0, 247],
        ),
        (
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            vec![1, 2, 3, 4, 5, 6, 7, 8, 255, 9, 0, 0, 0, 0, 0, 0, 0, 248],
        ),
    ];

    for (plain, ascending) in cases {
        assert_eq!(EncodedBytesLength(plain.len()), ascending.len());
        assert_eq!(EncodeBytes(Vec::new(), &plain), ascending);
        let (remain, decoded) = DecodeBytes(&ascending, None).unwrap();
        assert!(remain.is_empty());
        assert_eq!(decoded, plain);

        let descending = EncodeBytesDesc(Vec::new(), &plain);
        assert_eq!(
            descending,
            ascending.iter().map(|byte| !byte).collect::<Vec<_>>()
        );
        let (remain, decoded) = DecodeBytesDesc(&descending, None).unwrap();
        assert!(remain.is_empty());
        assert_eq!(decoded, plain);
    }

    // 长度不足、错误 marker、错误 padding、非终止组 marker=0 均应失败。
    for malformed in [
        vec![1, 2, 3, 4],
        vec![0, 0, 0, 0, 0, 0, 0, 0, 246],
        vec![0, 0, 0, 0, 0, 0, 0, 1, 247],
        vec![1, 2, 3, 4, 5, 6, 7, 8, 0],
    ] {
        assert!(DecodeBytes(&malformed, None).is_err());
    }
}

/// Compact 编码保留前缀字节，并正确切分 payload 与 leftover。
#[test]
fn compact_bytes_preserve_prefix_payload_and_leftover() {
    let encoded = EncodeCompactBytes(vec![42], b"codec");
    assert_eq!(encoded[0], 42);
    let (leftover, decoded) = DecodeCompactBytes(&encoded[1..]).unwrap();
    assert!(leftover.is_empty());
    assert_eq!(decoded, b"codec");
    assert!(DecodeCompactBytes(&[20, 1, 2]).is_err());
}

/// Datum key 往返与 memcomparable 有序性应对齐 Go（负数 key < 正数 key）。
#[test]
fn datum_key_round_trip_and_memcomparable_order_match_go() {
    let mut negative = types::Datum::default();
    negative.SetInt64(-1);
    let mut positive = types::Datum::default();
    positive.SetInt64(1);

    let negative_key = EncodeKey(time::UTC, Vec::new(), vec![negative.clone()]).unwrap();
    let positive_key = EncodeKey(time::UTC, Vec::new(), vec![positive.clone()]).unwrap();
    assert!(negative_key < positive_key);
    assert_eq!(Decode(negative_key, 1).unwrap()[0].GetInt64(), -1);
    assert_eq!(Decode(positive_key, 1).unwrap()[0].GetInt64(), 1);

    let value = EncodeValue(time::UTC, Vec::new(), vec![negative.clone()]).unwrap();
    assert_eq!(
        value.len(),
        EstimateValueSize(types::StrictContext.clone(), negative).unwrap()
    );
    assert_eq!(Decode(value, 1).unwrap()[0].GetInt64(), -1);
}

/// Decimal 编解码保留数值、精度、小数位，并正确报告 leftover。
#[test]
fn decimal_codec_preserves_value_precision_fraction_and_leftover() {
    for text in ["123400", "12.34", "0.01234", "-0.1234", "-12.3400", "0"] {
        let decimal = types::NewDecFromStringForTest(text);
        let (precision, frac) = decimal.PrecisionAndFrac();
        let encoded = EncodeDecimal(vec![7, 8], &decimal, 0, 0).unwrap();
        let (leftover, decoded, decoded_precision, decoded_frac) =
            DecodeDecimal(&encoded[2..]).unwrap();
        assert!(leftover.is_empty());
        assert_eq!(
            (decoded_precision, decoded_frac),
            (precision as i32, frac as i32)
        );
        assert_eq!(decimal.Compare(&decoded), 0, "{text}");
    }

    assert!(DecodeDecimal(&[1, 2]).is_err());
}
