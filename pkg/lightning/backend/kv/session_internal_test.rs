// Copyright 2022 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// Session / MemBuf 内部行为单元测试。
//
// 验证字节缓冲分配与回收策略（容量取 2 的幂对齐），以及 Session 系统变量
// 注入、TakeKvPairs 与用户变量生命周期。

use std::collections::HashMap;

use encode::{Datum, SessionOptions};
use verification::KvPair;

use crate::*;

/// 1 MiB，与 session 模块分配下限一致。
const MIB: usize = 1024 * 1024;

/// 交错 AllocateBuf / Recycle：验证池中最终保留的容量序列。
#[test]
fn TestKVMemBufInterweaveAllocAndRecycle() {
    let cases = [
        (
            vec![MIB, 2 * MIB, 3 * MIB, 4 * MIB, 5 * MIB],
            vec![4 * MIB, 2 * MIB, 8 * MIB, 16 * MIB],
        ),
        (
            vec![5 * MIB, 4 * MIB, 3 * MIB, 2 * MIB, MIB],
            vec![16 * MIB],
        ),
        (vec![5, 4, 3, 2, 1], vec![MIB]),
        (
            vec![MIB, 2 * MIB, 3 * MIB, 2 * MIB, MIB, 5 * MIB],
            vec![8 * MIB, 4 * MIB, 2 * MIB, 16 * MIB],
        ),
    ];
    for (requests, expected) in cases {
        let mut buffer = MemBuf::default();
        for request in requests {
            buffer.AllocateBuf(request);
            let allocated = buffer.TakeBuf().unwrap();
            buffer.Recycle(allocated);
        }
        let actual = buffer
            .availableBufs
            .lock()
            .unwrap()
            .iter()
            .map(|buf| buf.cap)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

/// 批量分配两档容量后统一回收：池大小上限为 maxAvailableBufSize，且只保留较大档。
#[test]
fn TestKVMemBufBatchAllocAndRecycle() {
    let mut buffer = MemBuf::default();
    let mut allocated = Vec::new();
    for _ in 0..maxAvailableBufSize {
        buffer.AllocateBuf(MIB);
        allocated.push(buffer.TakeBuf().unwrap());
    }
    for _ in 0..maxAvailableBufSize {
        buffer.AllocateBuf(2 * MIB);
        allocated.push(buffer.TakeBuf().unwrap());
    }
    for item in allocated {
        buffer.Recycle(item);
    }
    let available = buffer.availableBufs.lock().unwrap();
    assert_eq!(available.len(), maxAvailableBufSize);
    // 2*MIB 请求对齐后 cap 为 4*MIB；池满时小缓冲被淘汰。
    assert!(available.iter().all(|buf| buf.cap == 4 * MIB));
    drop(available);

    let mut reused = Vec::new();
    for index in 0..maxAvailableBufSize {
        buffer.AllocateBuf(MIB);
        let allocated = buffer.TakeBuf().unwrap();
        assert_eq!(allocated.cap, 4 * MIB);
        reused.push(allocated);
        assert_eq!(
            buffer.availableBufs.lock().unwrap().len(),
            maxAvailableBufSize - index - 1
        );
    }
    for item in reused {
        buffer.Recycle(item);
    }
    assert_eq!(
        buffer.availableBufs.lock().unwrap().len(),
        maxAvailableBufSize
    );
}

/// Session 构造：过滤已知 sysvar、写入/取出 KV、用户变量与 Close。
#[test]
fn TestSessionInternalState() {
    let mut session = NewSession(&SessionOptions {
        SQLMode: 0x0040_0000,
        Timestamp: 123456,
        SysVars: HashMap::from([
            ("max_allowed_packet".into(), "40960".into()),
            ("div_precision_increment".into(), "9".into()),
            ("time_zone".into(), "SYSTEM".into()),
            ("lc_time_names".into(), "en_US".into()),
            ("default_week_format".into(), "1".into()),
            ("block_encryption_mode".into(), "aes-256-ecb".into()),
            ("group_concat_max_len".into(), "2048".into()),
            ("tidb_backoff_weight".into(), "6".into()),
            ("tidb_row_format_version".into(), "2".into()),
        ]),
        ..Default::default()
    })
    .unwrap();
    // lc_time_names 不在 KNOWN 列表，不应影响上下文。
    assert_eq!(session.GetExprCtx().MaxAllowedPacket, 40960);
    assert_eq!(session.GetExprCtx().DivPrecisionIncrement, 9);
    assert_eq!(session.GetExprCtx().TimeZone, "SYSTEM");
    assert_eq!(session.GetExprCtx().DefaultWeekFormat, "1");
    assert_eq!(session.GetExprCtx().BlockEncryptionMode, "aes-256-ecb");
    assert_eq!(session.GetExprCtx().GroupConcatMaxLen, 2048);
    assert_eq!(session.GetExprCtx().CurrentTimestamp, 123456);
    assert!(session.GetTableCtx().RowEncodingEnabled);
    session.Txn().Set(b"k1", b"v1").unwrap();
    session.Txn().Set(b"k2", b"v2").unwrap();
    assert_eq!(
        session.TakeKvPairs().Pairs,
        vec![
            KvPair {
                key: b"k1".to_vec(),
                val: b"v1".to_vec()
            },
            KvPair {
                key: b"k2".to_vec(),
                val: b"v2".to_vec()
            },
        ]
    );
    session.SetUserVarVal("x", Datum::Int(1));
    assert!(session.GetExprCtx().UserVars.contains_key("x"));
    session.UnsetUserVar("x");
    session.Close();
}
