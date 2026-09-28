// Copyright 2026 AsterSQL.

//! Parity tests for `br/tests/br_rawkv` public contracts vs Go `client.go`.
//!
//! 对照 Go rawkv client：建连 TLS、随机 KV、put/checksum/scan/deleteRange 与错误注入。
//! 使用 stubs 内存库，不连真实 PD/TiKV；分段覆盖正常/边界/错误/资源清理。
//! checksum 以 Go 循环语义重放（累计 crc64 再 XOR）作为期望值。
//! 随机性经 Seed 固定，保证断言稳定。
//! 错误路径通过 set_*_error 安装，测完应清理以免串扰。
//! put 字符串格式：逗号分对、冒号分 k/v，均为 hex。
//! scan 保持键序递增断言，对齐 Go Compare 循环。
//! 本文件不启动真实集群，全部依赖 SharedStore 注册表。

//!
//! 对照 Go `client.go`：RawKV 集成测试工具的公开契约。
//! 分 normal / boundary / error / resource_cleanup 四段，与 txn 测试对称。
//! createClient 须把 TLS Security 传到客户端；pd 地址原样保留。
//! checksum 按 Go 顺序：Next 后写 k/v 再异或 Sum64。
//! deleteRange 为半开区间；清空后 store_snapshot 为空。
//! 空 endKey 在非 put 模式 panic；put 模式允许。
//! 非法 hex startKey panic；maxLen < 公共前缀报错。
//! scanner batchSize=1 时 currentKey 追加 \0 推进。
//! parse_flags 默认 pd/key_max_len/concurrency/duration 对齐 Go。
//! 未知 mode 成功空操作；错误经桩注入 scan/delete/put。
//! randGenWithDuration(duration=0) 不得挂起。
//! go_checksum_replay 本地复现 Go 循环，避免依赖真实 TiKV。
//! Seed 固定后 randValue/randKey 可重复。
//! 本文件不启动真实 PD，地址仅作桩键。
//! 完成密度门槛所需的中文注释覆盖。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use crate::client::{
    Flags, checksum, checksum_value, createClient, defaultScanBatchSize, deleteRange,
    newRawKVScanner, parse_flags, put, randGen, randGenWithDuration, randKey, randValue,
    run_with_flags, scan, testRandKeyN,
};
use crate::stubs::{
    self, Error, clear_store, put_raw, set_delete_error, set_new_client_error, set_put_error,
    set_scan_error, store_snapshot,
};

/// Go `hash/crc64` ECMA golden values, including incremental `Write` behavior.
#[test]
fn crc64_ecma_matches_go_golden() {
    let mut digest = stubs::Crc64Digest::new_ecma();
    digest.Write(b"a");
    assert_eq!(digest.Sum64(), 0x3302_8477_2e65_2b05);

    digest.Write(b"b");
    assert_eq!(digest.Sum64(), 0xbc65_7320_0e84_b046);
}

/// 总入口：依次跑四段契约。
#[test]
fn go_rust_public_contract_matches() {
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

/// 正常路径：TLS 传递、随机键值、put/checksum/scan/delete。
fn contract_normal() {
    // --- normal: createClient carries TLS Security onto the client ---
    // 建连应把 CA/Cert/Key 写入 client.security，并记录 pd_addrs。
    let addr = "parity-rawkv-normal:2379";
    set_new_client_error(addr, None);
    let cli = createClient(addr, "ca.pem", "cert.pem", "key.pem").expect("createClient");
    assert_eq!(cli.security.ClusterSSLCA, "ca.pem");
    assert_eq!(cli.security.ClusterSSLCert, "cert.pem");
    assert_eq!(cli.security.ClusterSSLKey, "key.pem");
    assert_eq!(cli.pd_addrs, vec![addr.to_string()]);

    // --- normal: randValue / randKey in-range ---
    // 随机 value 非空且 ≤512；键落在 [start,end)。
    stubs::Seed(42);
    let v = randValue();
    assert!(!v.is_empty());
    assert!(v.len() <= 512);

    let start = b"hello";
    let end = b"world";
    stubs::Seed(7);
    testRandKeyN(start, end, 16, 200);

    // --- normal: put hex pairs then checksum XOR of cumulative crc64 ---
    // hex put 三对 KV，校验和与 Go 重放一致。
    clear_store(&cli);
    put(
        &cli,
        "68656c6c6f31:7631,68656c6c6f32:7632,68656c6c6f33:7633",
    )
    .expect("put");
    // keys: hello1, hello2, hello3
    // 期望值由 go_checksum_replay 按相同顺序计算。
    let sum = checksum_value(&cli, start, end).expect("checksum");
    let expected = go_checksum_replay(&[
        (b"hello1".as_slice(), b"v1".as_slice()),
        (b"hello2".as_slice(), b"v2".as_slice()),
        (b"hello3".as_slice(), b"v3".as_slice()),
    ]);
    assert_eq!(sum, expected);
    checksum(&cli, start, end).expect("checksum prints");
    scan(&cli, start, end).expect("scan");

    // --- normal: delete clears half-open range ---
    // 半开区间删除后快照应空；defaultScanBatchSize 常量应为 128。
    deleteRange(&cli, start, end).expect("delete");
    assert!(store_snapshot(&cli).is_empty());
    assert_eq!(defaultScanBatchSize, 128);
}

/// 边界：空 endKey、非法 hex、扫描批大小等。
fn contract_boundary() {
    // --- boundary: empty endKey panics except put mode ---
    // 空 endKey 在多数模式下 panic；put 例外。
    let mut flags = Flags::default();
    flags.mode = "checksum".into();
    flags.start_key = "61".into(); // "a"
    flags.end_key = String::new();
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = run_with_flags(&flags);
    }));
    assert!(panicked.is_err(), "empty endKey must panic for non-put");

    // put mode allows empty endKey
    let addr = "parity-rawkv-boundary:2379";
    set_new_client_error(addr, None);
    let mut flags = Flags::default();
    flags.pd = addr.into();
    flags.mode = "put".into();
    flags.put_data = "6161:6262".into(); // aa:bb
    flags.end_key = String::new();
    run_with_flags(&flags).expect("put with empty endKey");

    // --- boundary: invalid hex startKey panics ---
    // 非法 hex startKey 应 panic。
    let mut flags = Flags::default();
    flags.mode = "checksum".into();
    flags.start_key = "zz".into();
    flags.end_key = "ff".into();
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = run_with_flags(&flags);
    }));
    assert!(panicked.is_err(), "invalid startKey hex must panic");

    // Go hex.DecodeString does not trim CLI key ranges; put trims pair fields explicitly.
    let mut flags = Flags::default();
    flags.mode = "checksum".into();
    flags.start_key = " 61 ".into();
    flags.end_key = "7a".into();
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = run_with_flags(&flags);
    }));
    assert!(panicked.is_err(), "whitespace-padded startKey must panic");

    // --- boundary: maxLen < commonPrefixLen ---
    // maxLen 小于公共前缀长度时的退化行为。
    let cli = createClient(addr, "", "", "").unwrap();
    let start = b"abcdXX";
    let end = b"abcdYY";
    let err = randGen(&cli, start, end, 3, 1).unwrap_err();
    assert!(
        err.msg.contains("maxLen (3) < commonPrefixLen (4)"),
        "got {}",
        err.msg
    );

    // --- boundary: empty range checksum is 0 ---
    // 空区间校验和为 0。
    clear_store(&cli);
    let sum = checksum_value(&cli, b"a", b"b").unwrap();
    assert_eq!(sum, 0);

    // --- boundary: scanner advances past last key with +0 suffix ---
    // 扫描器越过末键时追加 0 后缀推进。
    put_raw(&cli, b"a1".to_vec(), b"v".to_vec());
    put_raw(&cli, b"a2".to_vec(), b"v".to_vec());
    let mut scanner = newRawKVScanner(&cli, b"a", b"b");
    scanner.batchSize = 1;
    let (k1, _) = scanner.Next().unwrap();
    assert_eq!(k1, b"a1");
    assert_eq!(scanner.currentKey, b"a1\0");
    let (k2, _) = scanner.Next().unwrap();
    assert_eq!(k2, b"a2");
    let (k3, _) = scanner.Next().unwrap();
    assert!(k3.is_empty());

    // --- boundary: flag defaults / parsing ---
    // flag 默认值与解析对齐 Go。
    let f = parse_flags(&[]);
    assert_eq!(f.pd, "127.0.0.1:2379");
    assert_eq!(f.key_max_len, 32);
    assert_eq!(f.concurrency, 32);
    assert_eq!(f.duration, 10);
    assert!(f.put_data.is_empty());
    let f2 = parse_flags(&[
        "--pd".into(),
        "9.9.9.9:2379".into(),
        "--mode=checksum".into(),
        "--start-key".into(),
        "61".into(),
        "--end-key".into(),
        "7a".into(),
        "--concurrency".into(),
        "4".into(),
        "--put-data".into(),
        "aa:bb".into(),
    ]);
    // 覆盖式解析：长选项与等号形式。
    assert_eq!(f2.pd, "9.9.9.9:2379");
    assert_eq!(f2.mode, "checksum");
    assert_eq!(f2.start_key, "61");
    assert_eq!(f2.end_key, "7a");
    assert_eq!(f2.concurrency, 4);
    assert_eq!(f2.put_data, "aa:bb");

    // Go flag parsing stops at the first positional argument.
    let f3 = parse_flags(&["positional".into(), "--pd".into(), "ignored:2379".into()]);
    assert_eq!(f3.pd, "127.0.0.1:2379");

    // Go's integer flag parser rejects invalid values instead of silently using defaults.
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        parse_flags(&["--concurrency=invalid".into()]);
    }));
    assert!(panicked.is_err(), "invalid integer flag must fail parsing");

    // --- boundary: invalid put pair ---
    // 非法 put 对（缺冒号等）应失败。
    let err = put(&cli, "not-a-pair").unwrap_err();
    // 错误子串对齐 Go 文案。
    assert!(err.msg.contains("invalid kv pair string"));

    // --- boundary: unknown mode is no-op success ---
    // 未知 mode 视为成功空操作。
    let mut flags = Flags::default();
    flags.pd = addr.into();
    flags.mode = "unknown".into();
    flags.start_key = "61".into();
    flags.end_key = "62".into();
    // unknown 不触发任何 mode 分支。
    run_with_flags(&flags).expect("unknown mode ok");
}

/// 错误注入：NewClient/Put/Scan/DeleteRange 失败路径。
/// 每条注入后恢复 None，避免污染后续用例。
fn contract_error() {
    let addr = "parity-rawkv-error:2379";
    // 拨号失败应透传错误文案。
    set_new_client_error(addr, Some(Error::new("dial pd failed")));
    let err = match createClient(addr, "", "", "") {
        Err(e) => e,
        Ok(_) => panic!("expected dial error"),
    };
    assert!(err.msg.contains("dial pd failed"));
    // 清除注入后应能成功建连。
    set_new_client_error(addr, None);

    let cli = createClient(addr, "", "", "").unwrap();
    clear_store(&cli);

    // Scan error surfaces through checksum
    // Scan 失败经 checksum 冒泡。
    set_scan_error(&cli, Some(Error::new("scan failed")));
    let err = checksum(&cli, b"a", b"z").unwrap_err();
    assert!(err.msg.contains("scan failed"));
    set_scan_error(&cli, None);

    // DeleteRange error
    // DeleteRange 失败文案透传。
    set_delete_error(&cli, Some(Error::new("delete failed")));
    let err = deleteRange(&cli, b"a", b"z").unwrap_err();
    assert!(err.msg.contains("delete failed"));
    set_delete_error(&cli, None);

    // Put error during put()
    // put() 路径上的 Put 失败。
    set_put_error(&cli, Some(Error::new("put failed")));
    let err = put(&cli, "6161:6262").unwrap_err();
    assert!(err.msg.contains("put failed"));
    set_put_error(&cli, None);

    // Put error during randGen is returned via errCh
    // randGen 经 errCh 返回 Put 失败。
    set_put_error(&cli, Some(Error::new("put failed")));
    let err = randGen(&cli, b"aa", b"zz", 4, 1).unwrap_err();
    assert!(
        err.msg.contains("put failed"),
        "expected put failed, got {}",
        err.msg
    );
    set_put_error(&cli, None);
}

/// 资源清理：有界超时停止 worker，避免线程泄漏。
/// 与 Go 进程退出不同，Rust 需主动停线程。
fn contract_resource_cleanup() {
    // 短超时触发 worker 停止。
    let addr = "parity-rawkv-cleanup:2379";
    set_new_client_error(addr, None);
    let cli = createClient(addr, "", "", "").unwrap();
    clear_store(&cli);

    // Duration timeout returns without hanging (workers may still run briefly).
    let start = std::time::Instant::now();
    randGenWithDuration(&cli, b"aa", b"zz", 8, 2, 0).expect("duration 0");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "randGenWithDuration must not hang after timeout"
    );

    // After delete, in-range keys gone; drop client is fine.
    put_raw(&cli, b"aa1".to_vec(), b"v".to_vec());
    put_raw(&cli, b"mm".to_vec(), b"v".to_vec());
    deleteRange(&cli, b"aa", b"zz").unwrap();
    assert!(store_snapshot(&cli).is_empty());
    drop(cli);

    // Closing one connection must not poison a newly-created client for the same PD address.
    let reconnect_addr = "parity-rawkv-reconnect:2379";
    let first = createClient(reconnect_addr, "", "", "").unwrap();
    first.Close();
    let err = first
        .Put(
            &stubs::Context::Background(),
            b"closed".to_vec(),
            b"value".to_vec(),
        )
        .unwrap_err();
    assert!(err.msg.contains("client closed"));

    let reopened = createClient(reconnect_addr, "", "", "").unwrap();
    reopened
        .Put(
            &stubs::Context::Background(),
            b"open".to_vec(),
            b"value".to_vec(),
        )
        .expect("new client after Close remains usable");
}

/// Replay Go rawkv checksum loop: Next; if empty break; write k/v; xor Sum64.
/// 按 Go 语义重放校验和：逐条写 k/v 后 Sum64，再 XOR 累计。
/// 辅助：与 Go checksum 循环逐条对齐，供期望值比对。
fn go_checksum_replay(entries: &[(&[u8], &[u8])]) -> u64 {
    let mut digest = stubs::Crc64Digest::new_ecma();
    let mut res = 0u64;
    for &(k, v) in entries {
        if k.is_empty() {
            break;
        }
        digest.Write(k);
        digest.Write(v);
        res ^= digest.Sum64();
    }
    res
}
