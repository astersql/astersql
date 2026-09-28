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

// 数值与浮点编解码单元测试：定长/变长/可比变长整数及 float 往返与序关系。
//
// 对齐 Go codec 测试向量，校验后缀保留、升序/降序字节序、错误信息，
// 以及 NaN / ±0 等 IEEE754 边界在符号变换下的行为。

use super::*;

/// 字节切片字典序比较，便于断言编码后的序关系。
fn ordering(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    left.cmp(right)
}

/// 定长有符号/无符号编解码往返、后缀保留，以及升序/降序序关系与不足字节错误。
#[test]
fn fixed_width_numbers_round_trip_preserve_suffix_and_order() {
    let signed = [
        i64::MIN,
        i32::MIN as i64,
        i16::MIN as i64,
        i8::MIN as i64,
        -(1 << 55),
        -(1 << 47),
        -(1 << 33),
        -(1 << 23),
        -1,
        0,
        1,
        (1 << 23) - 1,
        (1 << 33) - 1,
        (1 << 47) - 1,
        (1 << 55) - 1,
        i8::MAX as i64,
        i16::MAX as i64,
        i32::MAX as i64,
        i64::MAX,
    ];
    for value in signed {
        let mut encoded = EncodeInt(vec![0xaa], value);
        assert_eq!(encoded[0], 0xaa);
        encoded.extend_from_slice(&[0xbb, 0xcc]);
        let (remain, decoded) = DecodeInt(&encoded[1..]).expect("DecodeInt");
        assert_eq!(decoded, value);
        assert_eq!(remain, &[0xbb, 0xcc]);

        let mut encoded_desc = EncodeIntDesc(Vec::new(), value);
        encoded_desc.push(0xdd);
        let (remain, decoded) = DecodeIntDesc(&encoded_desc).expect("DecodeIntDesc");
        assert_eq!(decoded, value);
        assert_eq!(remain, &[0xdd]);
    }

    let unsigned = [
        0,
        1,
        u8::MAX as u64,
        u16::MAX as u64,
        (1 << 24) - 1,
        u32::MAX as u64,
        (1 << 48) - 1,
        (1 << 56) - 1,
        i64::MAX as u64,
        u64::MAX,
    ];
    for value in unsigned {
        let mut encoded = EncodeUint(Vec::new(), value);
        encoded.push(0xee);
        let (remain, decoded) = DecodeUint(&encoded).expect("DecodeUint");
        assert_eq!((remain, decoded), (&[0xee][..], value));

        let mut encoded_desc = EncodeUintDesc(Vec::new(), value);
        encoded_desc.push(0xff);
        let (remain, decoded) = DecodeUintDesc(&encoded_desc).expect("DecodeUintDesc");
        assert_eq!((remain, decoded), (&[0xff][..], value));
    }

    let mut signed_in_order = signed;
    signed_in_order.sort_unstable();
    for pair in signed_in_order.windows(2) {
        assert_eq!(
            ordering(
                &EncodeInt(Vec::new(), pair[0]),
                &EncodeInt(Vec::new(), pair[1])
            ),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            ordering(
                &EncodeIntDesc(Vec::new(), pair[0]),
                &EncodeIntDesc(Vec::new(), pair[1])
            ),
            std::cmp::Ordering::Greater
        );
    }
    for pair in unsigned.windows(2) {
        assert_eq!(
            ordering(
                &EncodeUint(Vec::new(), pair[0]),
                &EncodeUint(Vec::new(), pair[1])
            ),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            ordering(
                &EncodeUintDesc(Vec::new(), pair[0]),
                &EncodeUintDesc(Vec::new(), pair[1])
            ),
            std::cmp::Ordering::Greater
        );
    }

    assert_eq!(
        DecodeInt(&[0; 7]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeIntDesc(&[0; 7]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeUint(&[0; 7]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeUintDesc(&[0; 7]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
}

/// 变长 varint/uvarint：对照 Go 向量、往返、最大长度与溢出/不足错误。
#[test]
fn binary_varints_match_go_vectors_round_trip_and_errors() {
    let signed_vectors: &[(i64, &[u8])] = &[
        (0, &[0]),
        (-1, &[1]),
        (1, &[2]),
        (-2, &[3]),
        (63, &[126]),
        (-64, &[127]),
        (64, &[128, 1]),
    ];
    for &(value, expected) in signed_vectors {
        assert_eq!(EncodeVarint(Vec::new(), value), expected);
        let mut input = expected.to_vec();
        input.push(0xaa);
        assert_eq!(
            DecodeVarint(&input).expect("DecodeVarint"),
            (&[0xaa][..], value)
        );
    }
    for value in [i64::MIN, i32::MIN as i64, i32::MAX as i64, i64::MAX] {
        let encoded = EncodeVarint(Vec::new(), value);
        assert_eq!(DecodeVarint(&encoded).expect("signed round trip").1, value);
    }

    let unsigned_vectors: &[(u64, &[u8])] = &[
        (0, &[0]),
        (1, &[1]),
        (127, &[127]),
        (128, &[128, 1]),
        (255, &[255, 1]),
        (256, &[128, 2]),
    ];
    for &(value, expected) in unsigned_vectors {
        assert_eq!(EncodeUvarint(Vec::new(), value), expected);
        let mut input = expected.to_vec();
        input.push(0xbb);
        assert_eq!(
            DecodeUvarint(&input).expect("DecodeUvarint"),
            (&[0xbb][..], value)
        );
    }
    let max_encoded = EncodeUvarint(Vec::new(), u64::MAX);
    assert_eq!(max_encoded.len(), 10);
    assert_eq!(
        DecodeUvarint(&max_encoded).expect("maximum uvarint").1,
        u64::MAX
    );

    assert_eq!(
        DecodeVarint(&[]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeUvarint(&[0x80]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeVarint(&[0x80; 10]).unwrap_err().to_string(),
        "value larger than 64 bits"
    );
    assert_eq!(
        DecodeUvarint(&[0x80; 10]).unwrap_err().to_string(),
        "value larger than 64 bits"
    );
}

/// memcomparable 变长整数：边界向量、往返、单字节内联分支与序关系。
#[test]
fn comparable_varints_match_go_boundaries_round_trip_and_order() {
    let unsigned_vectors: &[(u64, &[u8])] = &[
        (0, &[8]),
        (239, &[247]),
        (240, &[248, 240]),
        (255, &[248, 255]),
        (256, &[249, 1, 0]),
        (u64::MAX, &[255, 255, 255, 255, 255, 255, 255, 255, 255]),
    ];
    for &(value, expected) in unsigned_vectors {
        assert_eq!(EncodeComparableUvarint(Vec::new(), value), expected);
        let mut input = expected.to_vec();
        input.push(0xcc);
        let (remain, decoded) = DecodeComparableUvarint(&input).expect("DecodeComparableUvarint");
        assert_eq!(decoded, value);
        assert_eq!(remain, &[0xcc]);
    }

    let signed = [
        i64::MIN,
        -0x1_0000_0000,
        -256,
        -255,
        -1,
        0,
        1,
        239,
        240,
        i64::MAX,
    ];
    for value in signed {
        let encoded = EncodeComparableVarint(Vec::new(), value);
        let mut input = encoded.clone();
        input.push(0xdd);
        let (remain, decoded) = DecodeComparableVarint(&input).expect("DecodeComparableVarint");
        assert_eq!(decoded, value);
        // Go returns the original slice for its single-byte inline branch.
        if (0..=239).contains(&value) {
            assert_eq!(remain, input.as_slice());
            assert_eq!(remain.as_ptr(), input.as_ptr());
        } else {
            assert_eq!(remain, &[0xdd]);
        }
    }
    for pair in signed.windows(2) {
        assert_eq!(
            ordering(
                &EncodeComparableVarint(Vec::new(), pair[0]),
                &EncodeComparableVarint(Vec::new(), pair[1])
            ),
            std::cmp::Ordering::Less
        );
    }
    for pair in unsigned_vectors.windows(2) {
        assert_eq!(
            ordering(
                &EncodeComparableUvarint(Vec::new(), pair[0].0),
                &EncodeComparableUvarint(Vec::new(), pair[1].0)
            ),
            std::cmp::Ordering::Less
        );
    }

    // 对齐 Go TestNumberCodec 的混合编码流，并验证每次解码推进到下一项。
    let mut stream = EncodeComparableVarint(Vec::new(), -1);
    stream = EncodeComparableUvarint(stream, 1);
    stream = EncodeComparableVarint(stream, 2);
    let (stream, value) = DecodeComparableVarint(&stream).expect("first signed value");
    assert_eq!(value, -1);
    let (stream, value) = DecodeComparableUvarint(stream).expect("middle unsigned value");
    assert_eq!(value, 1);
    let (_, value) = DecodeComparableVarint(stream).expect("last signed value");
    assert_eq!(value, 2);
}

/// 可比变长解码拒绝非法标签、载荷不足与符号半区越界等形式。
#[test]
fn comparable_varint_decoders_reject_go_invalid_forms() {
    assert_eq!(
        DecodeComparableUvarint(&[]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeComparableUvarint(&[7]).unwrap_err().to_string(),
        "invalid bytes to decode value"
    );
    assert_eq!(
        DecodeComparableUvarint(&[249, 1]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );

    assert_eq!(
        DecodeComparableVarint(&[]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeComparableVarint(&[0, 0, 0, 0, 0, 0, 0, 0, 0])
            .unwrap_err()
            .to_string(),
        "invalid bytes to decode value"
    );
    assert_eq!(
        DecodeComparableVarint(&[255, 128, 0, 0, 0, 0, 0, 0, 0])
            .unwrap_err()
            .to_string(),
        "invalid bytes to decode value"
    );
    assert_eq!(
        DecodeComparableVarint(&[6, 0]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
}

/// float 升序/降序往返、后缀保留与 memcomparable 序关系。
#[test]
fn floats_round_trip_and_encode_in_go_memcomparable_order() {
    let values = [
        f64::NEG_INFINITY,
        -f64::MAX,
        -1.0,
        0.0,
        f64::from_bits(1),
        f32::from_bits(1) as f64,
        1.0,
        f32::MAX as f64,
        f64::MAX,
        f64::INFINITY,
    ];
    for value in values {
        let mut encoded = EncodeFloat(Vec::new(), value);
        encoded.push(0xaa);
        let (remain, decoded) = DecodeFloat(&encoded).expect("DecodeFloat");
        assert_eq!(decoded, value);
        assert_eq!(remain, &[0xaa]);

        let mut encoded_desc = EncodeFloatDesc(Vec::new(), value);
        encoded_desc.push(0xbb);
        let (remain, decoded) = DecodeFloatDesc(&encoded_desc).expect("DecodeFloatDesc");
        assert_eq!(decoded, value);
        assert_eq!(remain, &[0xbb]);
    }
    for pair in values.windows(2) {
        assert_eq!(
            ordering(
                &EncodeFloat(Vec::new(), pair[0]),
                &EncodeFloat(Vec::new(), pair[1])
            ),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            ordering(
                &EncodeFloatDesc(Vec::new(), pair[0]),
                &EncodeFloatDesc(Vec::new(), pair[1])
            ),
            std::cmp::Ordering::Greater
        );
    }
    assert_eq!(
        DecodeFloat(&[0; 7]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
    assert_eq!(
        DecodeFloatDesc(&[0; 7]).unwrap_err().to_string(),
        "insufficient bytes to decode value"
    );
}

/// ±0 与 NaN 在符号变换下的边角行为（对齐 Go `f >= 0` 判定）。
#[test]
fn float_bit_edges_follow_go_sign_transform() {
    let negative_zero = EncodeFloat(Vec::new(), -0.0);
    let positive_zero = EncodeFloat(Vec::new(), 0.0);
    assert_eq!(negative_zero, positive_zero);
    assert_eq!(
        DecodeFloat(&negative_zero).unwrap().1.to_bits(),
        0.0f64.to_bits()
    );

    // NaN makes Go's `f >= 0` condition false. A negative NaN therefore
    // round-trips, while a positive NaN follows the negative encode branch and
    // decodes to the sign-cleared inverse payload.
    let positive_nan_bits = 0x7ff8_0000_0000_0001;
    let positive_nan = f64::from_bits(positive_nan_bits);
    let positive_nan_decoded = (!positive_nan_bits) & !signMask;
    assert_eq!(
        DecodeFloat(&EncodeFloat(Vec::new(), positive_nan))
            .unwrap()
            .1
            .to_bits(),
        positive_nan_decoded
    );
    assert_eq!(
        DecodeFloatDesc(&EncodeFloatDesc(Vec::new(), positive_nan))
            .unwrap()
            .1
            .to_bits(),
        positive_nan_decoded
    );

    let negative_nan_bits = 0xfff8_0000_0000_0001;
    let negative_nan = f64::from_bits(negative_nan_bits);
    assert_eq!(
        DecodeFloat(&EncodeFloat(Vec::new(), negative_nan))
            .unwrap()
            .1
            .to_bits(),
        negative_nan_bits
    );
    assert_eq!(
        DecodeFloatDesc(&EncodeFloatDesc(Vec::new(), negative_nan))
            .unwrap()
            .1
            .to_bits(),
        negative_nan_bits
    );
}
