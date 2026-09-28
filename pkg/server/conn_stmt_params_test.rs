// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// `conn_stmt_params` 二进制参数解析的单元测试。

use super::conn_stmt_params::*;
use astersql_server_internal_util::NewInputDecoder;

#[test]
/// 验证定长切片边界检查：成功切片与越界错误。
fn takes_binary_values_with_checked_bounds() {
    assert_eq!(
        takeBinaryParamValue(b"abcdef", 2, 3).unwrap(),
        (&b"cde"[..], 5)
    );
    assert_eq!(
        takeBinaryParamValue(b"abc", 2, 2),
        Err(ParamError::MalformedPacket)
    );
}

#[test]
/// 覆盖定宽数值参数、无符号标志与 null bitmap。
fn parses_fixed_width_numeric_parameters_and_nulls() {
    let mut params = vec![BinaryParam::default(); 4];
    parseBinaryParams(
        &mut params,
        &[None, None, None, None],
        &[0b0000_0100],
        &[TYPE_TINY, 0, TYPE_LONG, 0x80, TYPE_DOUBLE, 0, TYPE_NULL, 0],
        &[7, 1, 2, 3, 4],
        None,
    )
    .unwrap();
    assert_eq!(params[0].val, [7]);
    assert_eq!(params[1].val, [1, 2, 3, 4]);
    assert!(params[1].is_unsigned);
    assert!(params[2].is_null);
    assert!(params[3].is_null);
}

#[test]
/// 覆盖 length-encoded 字符串、BLOB 与 NEWDECIMAL。
fn parses_length_encoded_strings_and_blobs() {
    let mut params = vec![BinaryParam::default(); 3];
    parseBinaryParams(
        &mut params,
        &[None, None, None],
        &[0],
        &[TYPE_VAR_STRING, 0, TYPE_BLOB, 0, TYPE_NEW_DECIMAL, 0],
        &[
            3, b'f', b'o', b'o', 2, 0xff, 0x00, 4, b'1', b'2', b'.', b'5',
        ],
        None,
    )
    .unwrap();
    assert_eq!(params[0].val, b"foo");
    assert_eq!(params[1].val, [0xff, 0]);
    assert_eq!(params[2].val, b"12.5");
}

#[test]
/// 已绑定 long-data：BLOB 保二进制，STRING 走解码器。
fn bound_long_data_preserves_binary_and_decodes_text() {
    let mut params = vec![BinaryParam::default(); 2];
    parseBinaryParams(
        &mut params,
        &[Some(vec![0xff]), Some(b"text".to_vec())],
        &[0],
        &[TYPE_BLOB, 0, TYPE_STRING, 0],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(params[0].tp, TYPE_BLOB);
    assert_eq!(params[0].val, [0xff]);
    assert_eq!(params[1].tp, TYPE_STRING);
    assert_eq!(params[1].val, b"text");
}

#[test]
/// 默认 UTF-8 输入解码器直通原始字节；显式 GBK 解码器执行字符集转换。
fn input_decoder_matches_server_charset_semantics() {
    let mut params = vec![BinaryParam::default()];
    parseBinaryParams(
        &mut params,
        &[Some(vec![0xff])],
        &[0],
        &[TYPE_STRING, 0],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(params[0].val, [0xff]);

    let decoder = NewInputDecoder("gbk");
    parseBinaryParams(
        &mut params,
        &[None],
        &[0],
        &[TYPE_VARCHAR, 0],
        &[4, 178, 226, 202, 212],
        Some(&decoder),
    )
    .unwrap();
    assert_eq!(params[0].val, "测试".as_bytes());
}

#[test]
/// 截断报文与未知类型码应返回对应错误。
fn rejects_truncated_and_unknown_parameter_encodings() {
    let mut params = vec![BinaryParam::default()];
    assert_eq!(
        parseBinaryParams(&mut params, &[None], &[0], &[TYPE_LONG, 0], &[1], None),
        Err(ParamError::MalformedPacket)
    );
    assert_eq!(
        parseBinaryParams(&mut params, &[None], &[0], &[17, 0], &[], None),
        Err(ParamError::UnknownFieldType(17))
    );
}
