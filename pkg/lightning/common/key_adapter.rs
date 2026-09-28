// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 键编码适配器：在业务键上附加 RowID，供重复检测排序与解码。
//
// `NoopKeyAdapter` 原样透传；`DupDetectKeyAdapter` 使用 TiDB 风格的
// memcomparable bytes 编码业务键，再追加 row_id 与其长度，使相同业务键
// 因不同 RowID 可区分，且编码后仍保持字典序。

use crate::CommonError;
/// 全零最短 RowID 占位，编码后应不大于其它常见 handle 形式。
pub const MinRowID: [u8; 9] = [0; 9];
/// 键适配器：编码/解码/预估长度，写入可复用 `dst` 缓冲区。
pub trait KeyAdapter: Send + Sync {
    fn Encode(&self, dst: Vec<u8>, key: &[u8], row_id: &[u8]) -> Vec<u8>;
    fn Decode(&self, dst: Vec<u8>, data: &[u8]) -> Result<Vec<u8>, CommonError>;
    fn EncodedLen(&self, key: &[u8], row_id: &[u8]) -> usize;
}
/// 为追加 `additional` 字节预留容量（对应 Go `reallocBytes`）。
pub fn reallocBytes(mut bytes: Vec<u8>, additional: usize) -> Vec<u8> {
    bytes.reserve(additional);
    bytes
}
/// 不做变换的键适配器：Encode/Decode 仅复制原始键。
#[derive(Clone, Copy, Default)]
pub struct NoopKeyAdapter;
impl KeyAdapter for NoopKeyAdapter {
    fn Encode(&self, mut dst: Vec<u8>, key: &[u8], _: &[u8]) -> Vec<u8> {
        dst.extend_from_slice(key);
        dst
    }
    fn Decode(&self, mut dst: Vec<u8>, data: &[u8]) -> Result<Vec<u8>, CommonError> {
        dst.extend_from_slice(data);
        Ok(dst)
    }
    fn EncodedLen(&self, key: &[u8], _: &[u8]) -> usize {
        key.len()
    }
}
/// TiDB memcomparable 字节编码：每 8 字节一组，末尾跟 pad 标记；整组对齐时再写终止组。
fn encode_bytes(mut dst: Vec<u8>, key: &[u8]) -> Vec<u8> {
    for chunk in key.chunks(8) {
        dst.extend_from_slice(chunk);
        let pad = 8 - chunk.len();
        dst.extend(std::iter::repeat_n(0, pad));
        dst.push(0xff - pad as u8);
    }
    if key.len() % 8 == 0 {
        dst.extend_from_slice(&[0; 8]);
        dst.push(0xf7)
    }
    dst
}
/// 解码 memcomparable 字节串，遇非零 pad 终止；允许尾部仍有未读字节（RowID 等）。
fn decode_bytes(data: &[u8]) -> Result<Vec<u8>, CommonError> {
    // Match TiDB codec.DecodeBytes: consume groups until a non-zero pad terminator,
    // allowing trailing bytes to remain unread.
    // 与 TiDB codec.DecodeBytes 一致：读到带 pad 的终止组即返回，尾部留给 RowID。
    let mut out = vec![];
    let mut offset = 0;
    while offset + 9 <= data.len() {
        let group = &data[offset..offset + 9];
        let pad = (0xff - group[8]) as usize;
        if pad > 8 {
            return Err(CommonError::new(
                "decode",
                format!("invalid marker byte, group bytes {group:?}"),
            ));
        }
        out.extend_from_slice(&group[..8 - pad]);
        offset += 9;
        if pad > 0 {
            if group[8 - pad..8].iter().any(|byte| *byte != 0) {
                return Err(CommonError::new(
                    "decode",
                    format!("invalid padding byte, group bytes {group:?}"),
                ));
            }
            return Ok(out);
        }
    }
    Err(CommonError::new(
        "decode",
        "insufficient bytes to decode value",
    ))
}
/// 重复检测用适配器：encoded = encode_bytes(key) || row_id || row_id_len(u16 BE)。
#[derive(Clone, Copy, Default)]
pub struct DupDetectKeyAdapter;
impl KeyAdapter for DupDetectKeyAdapter {
    fn Encode(&self, dst: Vec<u8>, key: &[u8], row_id: &[u8]) -> Vec<u8> {
        let mut dst = encode_bytes(dst, key);
        dst.extend_from_slice(row_id);
        dst.extend_from_slice(&(row_id.len() as u16).to_be_bytes());
        dst
    }
    fn Decode(&self, mut dst: Vec<u8>, data: &[u8]) -> Result<Vec<u8>, CommonError> {
        if data.len() < 2 {
            return Err(CommonError::new(
                "decode",
                "insufficient bytes to decode value",
            ));
        }
        let row_len = u16::from_be_bytes([data[data.len() - 2], data[data.len() - 1]]) as usize;
        if data.len() < row_len + 2 {
            return Err(CommonError::new(
                "decode",
                "insufficient bytes to decode value",
            ));
        }
        // 去掉尾部 row_id 与长度后，对前缀做 memcomparable 解码。
        dst.extend(decode_bytes(&data[..data.len() - row_len - 2])?);
        Ok(dst)
    }
    fn EncodedLen(&self, key: &[u8], row_id: &[u8]) -> usize {
        (key.len() / 8 + 1) * 9 + row_id.len() + 2
    }
}
