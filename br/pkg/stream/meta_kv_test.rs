// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/stream/meta_kv_test.go`.
//!
//! 覆盖 `meta_kv` 的解析/编码往返与 write-CF 边界错误。
//! 与 Go 同名用例一一对应；断言侧重字节布局与错误文案，
//! 不改写被测实现行为。辅助常量复用实现侧 flag 字节，避免
//! 测试硬编码漂移。
//! 固定 ts/样例串便于与 Go 测试字面量对照排查差异。

use crate::meta_kv::{
    ParseTxnMetaKeyFrom, RawWriteCFValue, WriteType, WriteTypeFrom, WriteTypePut, WriteTypeRollback,
};
use crate::stubs::codec;
use crate::stubs::meta;
use crate::stubs::utils::EncodeTxnMetaKey;

// 与 meta_kv 内部 flag 字节对齐，便于手工拼装测试缓冲。
// 勿改成实现私有常量引用：测试文件需独立可读。
const FLAG_SHORT_VALUE_PREFIX: u8 = b'v';
const FLAG_OVERLAPPED_ROLLBACK: u8 = b'R';
const FLAG_GC_FENCE_PREFIX: u8 = b'F';
const FLAG_LAST_CHANGE_PREFIX: u8 = b'l';
const FLAG_TXN_SOURCE_PREFIX: u8 = b'S';

/// Same-package Go field checks → round-trip via `EncodeTo` + public getters.
/// Go 侧可直接读私有字段；此处用 EncodeTo 往返代替字段级断言。
/// 同时约束：无 GC fence 输入不得在编码侧凭空出现。
fn assert_no_gc_fence_or_last_change(v: &RawWriteCFValue, buff: &[u8]) {
    assert_eq!(v.EncodeTo(), buff);
    // 若原文无 GC fence，编码结果也不应凭空引入（或双方一致含有）。
    assert!(!buff.contains(&FLAG_GC_FENCE_PREFIX) || v.EncodeTo().contains(&FLAG_GC_FENCE_PREFIX));
}

/// `TestRawMetaKeyForDB` → `test_raw_meta_key_for_db`.
/// 场景：DBs 前缀 + DBkey(Field) 的事务 meta 键；校验 Field→db_id 与往返编码。
/// 对齐 Go：DB 列表项的 ID 编码在 Field。
#[test]
fn test_raw_meta_key_for_db() {
    let db_id: i64 = 1;
    let ts: u64 = 400036290571534337;
    let m_dbs = b"DBs";
    // 手工构造与线上一致的事务 meta 键。
    let txn_key = EncodeTxnMetaKey(m_dbs, &meta::DBkey(db_id), ts);

    let raw = ParseTxnMetaKeyFrom(&txn_key).unwrap();
    // DB 列表项的 ID 落在 Field，而非 Key。
    let parse_db_id = meta::ParseDBKey(&raw.Field).unwrap();
    assert_eq!(db_id, parse_db_id);
    // 解析再编码必须字节级还原。
    assert_eq!(txn_key, raw.EncodeMetaKey());
}

/// `TestRawMetaKeyForTable` → `test_raw_meta_key_for_table`.
/// 场景：Key=DBkey、Field=TableKey；分别校验两侧 ID 与往返。
/// 表级 meta 键是 filter/rewrite 最常见输入形态。
#[test]
fn test_raw_meta_key_for_table() {
    let db_id: i64 = 1;
    let table_id: i64 = 57;
    let ts: u64 = 400036290571534337;
    // Key/Field 分别携带 db_id 与 table_id。
    let txn_key = EncodeTxnMetaKey(&meta::DBkey(db_id), &meta::TableKey(table_id), ts);

    let raw = ParseTxnMetaKeyFrom(&txn_key).unwrap();
    assert_eq!(db_id, meta::ParseDBKey(&raw.Key).unwrap());
    assert_eq!(table_id, meta::ParseTableKey(&raw.Field).unwrap());
    assert_eq!(txn_key, raw.EncodeMetaKey());
}

/// `TestWriteType` → `test_write_type`.
/// 合法字节 'P' 应映射为 WriteTypePut。
/// 非法字节路径由实现单测/调用方覆盖，此处只验 happy path。
#[test]
fn test_write_type() {
    let wt = WriteTypeFrom(b'P').unwrap();
    assert_eq!(wt, WriteTypePut as WriteType);
}

/// `TestWriteCFValueNoShortValue` → `test_write_cf_value_no_short_value`.
/// 仅 Put+startTs+txnSource，无 shortValue/GC fence；校验类型谓词与往返。
/// txnSource 后缀须在 EncodeTo 中原样保留。
#[test]
fn test_write_cf_value_no_short_value() {
    let ts: u64 = 400036290571534337;
    let txn_source: u64 = 9527;

    // 手工拼装：类型 + uvarint(ts) + 'S' + uvarint(txnSource)。
    let mut buff = vec![WriteTypePut];
    buff = codec::EncodeUvarint(buff, ts);
    buff.push(FLAG_TXN_SOURCE_PREFIX);
    buff = codec::EncodeUvarint(buff, txn_source);

    let mut v = RawWriteCFValue::default();
    v.ParseFrom(&buff).unwrap();
    // 非 Delete/Rollback，且无内联 shortValue。
    assert!(!v.IsDelete());
    assert!(!v.IsRollback());
    assert!(!v.HasShortValue());
    // 偏移 9 起为可选后缀；此处不应以 GC fence 开头。
    assert!(!buff[9..].starts_with(&[FLAG_GC_FENCE_PREFIX]));
    // Round-trip preserves txnSource / lastChange defaults (Go private-field asserts).
    assert_eq!(buff, v.EncodeTo());
    assert_no_gc_fence_or_last_change(&v, &buff);
    // 再拼一份期望缓冲，双路径确认 EncodeTo 稳定。
    let mut expected = vec![WriteTypePut];
    expected = codec::EncodeUvarint(expected, ts);
    expected.push(FLAG_TXN_SOURCE_PREFIX);
    expected = codec::EncodeUvarint(expected, txn_source);
    assert_eq!(v.EncodeTo(), expected);
}

/// `TestWriteCFValueWithShortValue` → `test_write_cf_value_with_short_value`.
/// 含 shortValue 与 lastChange 后缀；校验取值与 EncodeTo 恒等。
/// lastChange 在恢复跳过历史版本时有意义，此处只验编解码保真。
#[test]
fn test_write_cf_value_with_short_value() {
    let ts: u64 = 400036290571534337;
    let short_value = b"pingCAP";
    let last_change_ts: u64 = 9527;
    let versions_to_last_change: u64 = 95271;

    // shortValue 后紧跟 lastChange：uint + uvarint 两段载荷。
    let mut buff = vec![WriteTypePut];
    buff = codec::EncodeUvarint(buff, ts);
    buff.push(FLAG_SHORT_VALUE_PREFIX);
    buff.push(short_value.len() as u8);
    buff.extend_from_slice(short_value);
    buff.push(FLAG_LAST_CHANGE_PREFIX);
    buff = codec::EncodeUint(buff, last_change_ts);
    buff = codec::EncodeUvarint(buff, versions_to_last_change);

    let mut v = RawWriteCFValue::default();
    v.ParseFrom(&buff).unwrap();
    assert!(v.HasShortValue());
    assert_eq!(v.GetShortValue(), short_value);
    assert_eq!(v.EncodeTo(), buff);
    // 原文无 overlapped-rollback 时，编码也不应凭空插入该 flag。
    assert!(
        !v.EncodeTo()
            .windows(1)
            .any(|w| w == [FLAG_OVERLAPPED_ROLLBACK] && !buff.contains(&FLAG_OVERLAPPED_ROLLBACK))
    );
}

/// `TestWriteCFValueShortValueOverflow` → `test_write_cf_value_short_value_overflow`.
/// 边界：长度声明大于实际载荷、仅有 flag 无长度字节、以及合法 255 字节最大值。
#[test]
fn test_write_cf_value_short_value_overflow() {
    let ts: u64 = 400036290571534337;

    // 声明长度 10，却无 value 体 → need 12。
    let mut buff = vec![WriteTypePut];
    buff = codec::EncodeUvarint(buff, ts);
    buff.push(FLAG_SHORT_VALUE_PREFIX);
    buff.push(10);
    let mut v = RawWriteCFValue::default();
    let err = v.ParseFrom(&buff).unwrap_err();
    assert!(
        err.to_string()
            .contains("insufficient data for short value")
    );
    assert!(err.to_string().contains("need 12 bytes but only have"));

    // 仅 5 字节体，仍不足声明的 10。
    let mut buff2 = vec![WriteTypePut];
    buff2 = codec::EncodeUvarint(buff2, ts);
    buff2.push(FLAG_SHORT_VALUE_PREFIX);
    buff2.push(10);
    buff2.extend_from_slice(b"short");
    let mut v2 = RawWriteCFValue::default();
    let err2 = v2.ParseFrom(&buff2).unwrap_err();
    assert!(
        err2.to_string()
            .contains("insufficient data for short value")
    );
    assert!(err2.to_string().contains("need 12 bytes but only have 7"));

    // 声明 255 但只跟 4 字节 → need 257。
    let mut buff3 = vec![WriteTypePut];
    buff3 = codec::EncodeUvarint(buff3, ts);
    buff3.push(FLAG_SHORT_VALUE_PREFIX);
    buff3.push(255);
    buff3.extend_from_slice(b"test");
    let mut v3 = RawWriteCFValue::default();
    let err3 = v3.ParseFrom(&buff3).unwrap_err();
    assert!(
        err3.to_string()
            .contains("insufficient data for short value")
    );
    assert!(err3.to_string().contains("need 257 bytes but only have 6"));

    // 合法最大 shortValue：255 字节，应成功解析。
    let mut buff4 = vec![WriteTypePut];
    buff4 = codec::EncodeUvarint(buff4, ts);
    buff4.push(FLAG_SHORT_VALUE_PREFIX);
    buff4.push(255);
    let large_value: Vec<u8> = (0..255).map(|i| (i % 256) as u8).collect();
    buff4.extend_from_slice(&large_value);
    let mut v4 = RawWriteCFValue::default();
    v4.ParseFrom(&buff4).unwrap();
    assert!(v4.HasShortValue());
    assert_eq!(v4.GetShortValue().len(), 255);
    assert_eq!(v4.GetShortValue(), large_value);

    // 仅有 'v' flag、缺少长度字节。
    let mut buff5 = vec![WriteTypePut];
    buff5 = codec::EncodeUvarint(buff5, ts);
    buff5.push(FLAG_SHORT_VALUE_PREFIX);
    let mut v5 = RawWriteCFValue::default();
    let err5 = v5.ParseFrom(&buff5).unwrap_err();
    assert!(
        err5.to_string()
            .contains("insufficient data for short value prefix")
    );
    assert!(
        err5.to_string()
            .contains("need at least 2 bytes but only have 1")
    );
}

/// `TestWriteCFValueWithRollback` → `test_write_cf_value_with_rollback`.
/// Rollback 类型可携带 protected shortValue；校验谓词、Ts 与往返。
#[test]
fn test_write_cf_value_with_rollback() {
    let ts: u64 = 400036290571534337;
    let protected = [b'P'];

    let mut buff = vec![WriteTypeRollback];
    buff = codec::EncodeUvarint(buff, ts);
    buff.push(FLAG_SHORT_VALUE_PREFIX);
    buff.push(protected.len() as u8);
    buff.extend_from_slice(&protected);

    let mut v = RawWriteCFValue::default();
    v.ParseFrom(&buff).unwrap();
    assert!(v.IsRollback());
    assert!(v.HasShortValue());
    assert_eq!(v.GetShortValue(), protected);
    assert_eq!(v.GetStartTs(), ts);
    assert_eq!(v.EncodeTo(), buff);
}

/// `TestWriteCFValueWithDelete` → `test_write_cf_value_with_delete`.
/// Delete 仅有类型+Ts，无 shortValue。
#[test]
fn test_write_cf_value_with_delete() {
    let ts: u64 = 400036290571534337;
    let mut buff = vec![b'D'];
    buff = codec::EncodeUvarint(buff, ts);

    let mut v = RawWriteCFValue::default();
    v.ParseFrom(&buff).unwrap();
    assert!(v.IsDelete());
    assert!(!v.HasShortValue());
    assert_eq!(v.EncodeTo(), buff);
}

/// `TestWriteCFValueWithGcFence` → `test_write_cf_value_with_gc_fence`.
/// overlapped-rollback + GC fence 并存时，编码须保留两 flag。
#[test]
fn test_write_cf_value_with_gc_fence() {
    let ts: u64 = 400036290571534337;
    let gc_fence: u64 = 9527;

    let mut buff = vec![WriteTypePut];
    buff = codec::EncodeUvarint(buff, ts);
    buff.push(FLAG_OVERLAPPED_ROLLBACK);
    buff.push(FLAG_GC_FENCE_PREFIX);
    buff = codec::EncodeUint(buff, gc_fence);

    let mut v = RawWriteCFValue::default();
    v.ParseFrom(&buff).unwrap();
    assert_eq!(v.GetStartTs(), ts);
    // 往返后两个 flag 均须仍在。
    assert!(v.EncodeTo().contains(&FLAG_GC_FENCE_PREFIX));
    assert!(v.EncodeTo().contains(&FLAG_OVERLAPPED_ROLLBACK));
    assert_eq!(v.EncodeTo(), buff);
}
