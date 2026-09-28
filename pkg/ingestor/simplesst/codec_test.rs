// Copyright 2026 AsterSQL.

// RangeProperty（范围属性）编解码的单元测试。
//
// 验证单条与多条属性往返编解码，以及截断输入被拒绝。
// RangeProperty：SST 中一段连续键范围的统计元数据（首末键、偏移、大小、键数）。

/// 编码后解码应还原属性；截断多属性缓冲须返回错误。
#[test]
fn canonical_range_property_codec_round_trips_and_rejects_truncation() {
    use crate::codec::{
        PROPERTY_LENGTH_EXCEPT_KEYS, RangeProperty, decode_multi_props, decode_prop,
        encode_multi_props, encode_prop,
    };

    // 构造两条范围属性，覆盖不同键区间与文件偏移。
    let props = vec![
        RangeProperty {
            FirstKey: b"a".to_vec(),
            LastKey: b"b".to_vec(),
            Offset: 11,
            Size: 7,
            Keys: 2,
        },
        RangeProperty {
            FirstKey: b"c".to_vec(),
            LastKey: b"z".to_vec(),
            Offset: 29,
            Size: 18,
            Keys: 5,
        },
    ];
    let mut one = Vec::new();
    encode_prop(&mut one, &props[0]).unwrap();
    // 单条长度 = 固定字段 + FirstKey/LastKey 正文（此处各 1 字节）。
    assert_eq!(one.len(), PROPERTY_LENGTH_EXCEPT_KEYS + 2);
    assert_eq!(decode_prop(&one).unwrap(), props[0]);
    let encoded = encode_multi_props(&props).unwrap();
    assert_eq!(decode_multi_props(&encoded).unwrap(), props);
    // 去掉末尾一字节后解码应失败，防止静默吞掉截断数据。
    assert!(decode_multi_props(&encoded[..encoded.len() - 1]).is_err());
}

/// 对照 Go `TestPropertyLengthExceptKeys`：零值属性正文只有固定字段。
#[test]
fn test_property_length_except_keys() {
    use crate::codec::{PROPERTY_LENGTH_EXCEPT_KEYS, RangeProperty, encode_prop};

    let mut data = Vec::new();
    encode_prop(&mut data, &RangeProperty::default()).unwrap();
    assert_eq!(data.len(), PROPERTY_LENGTH_EXCEPT_KEYS);
}

/// Go `decodeProp` 只消费定义的字段，不会因后续字节拒绝已解码的属性。
#[test]
fn decode_prop_ignores_trailing_bytes_like_go() {
    use crate::codec::{RangeProperty, decode_prop, encode_prop};

    let expected = RangeProperty {
        FirstKey: b"key".to_vec(),
        LastKey: b"key2".to_vec(),
        Offset: 1,
        Size: 2,
        Keys: 3,
    };
    let mut encoded = Vec::new();
    encode_prop(&mut encoded, &expected).unwrap();
    encoded.extend_from_slice(b"trailing");

    assert_eq!(decode_prop(&encoded).unwrap(), expected);
}
