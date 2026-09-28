// Copyright 2026 AsterSQL.

//! Parity tests for `br/tests/br_txn` public contracts vs Go `client.go`.
//!
//! 对照 Go `client.go` 公开契约：TLS 建连、checksum、randGen、deleteRange、
//! flag 解析与错误注入。通过 stubs 注入 Begin/Commit/Delete/拨号失败，不连真实集群。
//! checksum 必须复刻 Go「先 Next 再读」顺序，否则 XOR 结果会与 Go 不一致。
//! deleteRange 为半开区间 [start,end)；appendIndex 可能把键推过 end，Go 亦接受。
//! 未知 mode 视为成功 no-op；空 endKey 必须 panic（对齐 Go log.Panic）。
//! randGenWithDuration(duration=0) 应立即取消，不得挂起测试线程。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::client::{
    Flags, appendIndex, checksum, checksum_value, createClient, deleteRange, parse_flags, randGen,
    randGenWithDuration, randKey, randValue, run_with_flags, testRandKeyN,
};
use crate::stubs::{
    self, Context, Error, GetGlobalConfig, StoreGlobalConfig, clear_store, put_raw,
    set_begin_error, set_commit_error, set_delete_error, set_iter_error, set_new_client_error,
    set_next_error, set_set_error, store_snapshot,
};

/// 总入口：串联 normal/boundary/error/cleanup 四段契约。
#[test]
fn go_rust_public_contract_matches() {
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

/// 正常路径：TLS 写入全局配置、辅助函数、checksum、randGen+delete。
fn contract_normal() {
    // Go hash/crc64 with the ECMA table uses complemented input/output.
    // This published check value prevents the Rust stand-in from validating itself.
    let mut digest = stubs::Crc64Digest::new_ecma();
    digest.Write(b"123456789");
    assert_eq!(digest.Sum64(), 0x995d_c9bb_df19_39fa);
    let mut chunked_digest = stubs::Crc64Digest::new_ecma();
    chunked_digest.Write(b"1234");
    chunked_digest.Write(b"56789");
    assert_eq!(chunked_digest.Sum64(), 0x995d_c9bb_df19_39fa);

    // createClient 成功后应把 TLS 三件套写入全局 Security。
    StoreGlobalConfig(stubs::Config::default());
    let addr = "parity-txn-normal:2379";
    set_new_client_error(addr, None);
    let cli = createClient(addr, "ca.pem", "cert.pem", "key.pem").expect("createClient");
    let conf = GetGlobalConfig();
    assert_eq!(conf.Security.ClusterSSLCA, "ca.pem");
    assert_eq!(conf.Security.ClusterSSLCert, "cert.pem");
    assert_eq!(conf.Security.ClusterSSLKey, "key.pem");
    assert_eq!(cli.pd_addrs, vec![addr.to_string()]);

    // appendIndex / randValue / randKey：固定 Seed 保证可复现。
    assert_eq!(appendIndex(b"ab".to_vec(), 0x41), b"abA");
    stubs::Seed(42);
    let v = randValue();
    assert!(!v.is_empty());
    // Go 侧 value 上限 512 字节。
    assert!(v.len() <= 512);

    let start = b"xhello".as_slice();
    let end = b"xworld".as_slice();
    stubs::Seed(7);
    // 抽样 200 次，键必须落在 [start,end) 语义内。
    testRandKeyN(start, end, 16, 200);

    // checksum：XOR 累积 crc64；Go 循环先 Next 再读，首条被跳过语义要复刻。
    clear_store(&cli);
    put_raw(&cli, b"xhello1".to_vec(), b"v1".to_vec());
    put_raw(&cli, b"xhello2".to_vec(), b"v2".to_vec());
    put_raw(&cli, b"xhello3".to_vec(), b"v3".to_vec());
    // 区间 [xhello, xworld) 内三条；用 go_checksum_replay 手工对照。
    let sum = checksum_value(&cli, start, end).expect("checksum");
    // 手工重放 Go 循环：Next-first，耗尽时空键 break。
    let expected = go_checksum_replay(&[
        (b"xhello1".as_slice(), b"v1".as_slice()),
        (b"xhello2".as_slice(), b"v2".as_slice()),
        (b"xhello3".as_slice(), b"v3".as_slice()),
    ]);
    assert_eq!(sum, expected);
    // checksum 包装函数仅打印，结果应同样成功。
    checksum(&cli, start, end).expect("checksum prints");

    // randGen 写入后 deleteRange 清空半开区间内键。
    clear_store(&cli);
    let ctx = Context::Background();
    randGen(&ctx, &cli, start, end, 8, 2).expect("randGen");
    let after_gen = store_snapshot(&cli);
    assert!(
        !after_gen.is_empty(),
        "randGen must commit at least one batch"
    );
    for (k, _) in &after_gen {
        assert!(k.as_slice() >= start);
        // appendIndex 可能把键推过 endKey；Go 为避冲突接受此行为。
        assert!(!k.is_empty());
    }
    deleteRange(&cli, start, end).expect("delete");
    // 半开 [start,end)；带 appendIndex 后缀且 >=end 的键可能残留，这里只断言区间内为空。
    let left_in_range: Vec<_> = store_snapshot(&cli)
        .into_iter()
        .filter(|(k, _)| k.as_slice() >= start && k.as_slice() < end)
        .collect();
    assert!(left_in_range.is_empty());
}

/// 边界：空 endKey panic、maxLen 过短、空区间 checksum、flag、未知 mode。
fn contract_boundary() {
    // 空 endKey 必须 panic（对齐 Go log.Panic）。
    let mut flags = Flags::default();
    flags.mode = "checksum".into();
    flags.start_key = "a".into();
    flags.end_key = String::new();
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = run_with_flags(&flags);
    }));
    assert!(panicked.is_err(), "empty endKey must panic");

    // maxLen 小于公共前缀长度时 randGen 返回明确错误。
    let addr = "parity-txn-boundary:2379";
    set_new_client_error(addr, None);
    let cli = createClient(addr, "", "", "").unwrap();
    let start = b"abcdXX";
    let end = b"abcdYY";
    // commonPrefixLen = 4 ("abcd")，maxLen=3 应失败。
    let err = randGen(&Context::Background(), &cli, start, end, 3, 1).unwrap_err();
    assert!(
        err.msg.contains("maxLen (3) < commonPrefixLen (4)"),
        "got {}",
        err.msg
    );

    // 空区间 checksum 结果为 0。
    clear_store(&cli);
    let sum = checksum_value(&cli, b"a", b"b").unwrap();
    assert_eq!(sum, 0);

    // 默认 flag 与混合风格解析。
    let f = parse_flags(&[]);
    assert_eq!(f.pd, "127.0.0.1:2379");
    assert_eq!(f.key_max_len, 32);
    assert_eq!(f.concurrency, 32);
    assert_eq!(f.duration, 10);
    let f2 = parse_flags(&[
        "--pd".into(),
        "9.9.9.9:2379".into(),
        "--mode=checksum".into(),
        "--start-key".into(),
        "xhello".into(),
        "--end-key".into(),
        "xworld".into(),
        "--concurrency".into(),
        "4".into(),
    ]);
    assert_eq!(f2.pd, "9.9.9.9:2379");
    assert_eq!(f2.mode, "checksum");
    assert_eq!(f2.start_key, "xhello");
    assert_eq!(f2.end_key, "xworld");
    assert_eq!(f2.concurrency, 4);

    // Go flag.Parse stops at the first positional argument.
    let positional = parse_flags(&["payload".into(), "--concurrency".into(), "4".into()]);
    assert_eq!(positional.concurrency, 32);
    assert!(catch_unwind(|| parse_flags(&["--unknown=x".into()])).is_err());
    assert!(catch_unwind(|| parse_flags(&["--duration=not-an-int".into()])).is_err());

    // make(chan error, concurrency) panics for a negative capacity in Go.
    let negative_concurrency = catch_unwind(AssertUnwindSafe(|| {
        let _ = randGen(&Context::Background(), &cli, b"aa", b"zz", 4, -1);
    }));
    assert!(negative_concurrency.is_err());

    // 未知 mode 视为成功 no-op，不 panic。
    let mut flags = Flags::default();
    flags.pd = addr.into();
    flags.mode = "scan".into();
    flags.start_key = "a".into();
    flags.end_key = "b".into();
    run_with_flags(&flags).expect("unknown mode ok");
}

/// 错误注入：拨号、Begin、Delete、Commit 失败均按 Go 语义上浮。
fn contract_error() {
    let addr = "parity-txn-error:2379";
    // 拨号失败应返回错误，不 panic。
    set_new_client_error(addr, Some(Error::new("dial pd failed")));
    let err = match createClient(addr, "", "", "") {
        Err(e) => e,
        Ok(_) => panic!("expected dial error"),
    };
    assert!(err.msg.contains("dial pd failed"));
    set_new_client_error(addr, None);

    let cli = createClient(addr, "", "", "").unwrap();
    clear_store(&cli);

    // Begin 失败经 checksum 路径上浮。
    set_begin_error(&cli, Some(Error::new("begin failed")));
    let err = checksum(&cli, b"a", b"z").unwrap_err();
    assert!(err.msg.contains("begin failed"));
    set_begin_error(&cli, None);

    // Iter / Next failures are distinct Go checksum error branches.
    set_iter_error(&cli, Some(Error::new("iter failed")));
    let err = checksum(&cli, b"a", b"z").unwrap_err();
    assert!(err.msg.contains("iter failed"));
    set_iter_error(&cli, None);

    put_raw(&cli, b"a1".to_vec(), b"v1".to_vec());
    put_raw(&cli, b"a2".to_vec(), b"v2".to_vec());
    set_next_error(&cli, Some(Error::new("next failed")));
    let err = checksum(&cli, b"a", b"z").unwrap_err();
    assert!(err.msg.contains("next failed"));
    set_next_error(&cli, None);

    // DeleteRange 失败直接返回。
    set_delete_error(&cli, Some(Error::new("delete failed")));
    let err = deleteRange(&cli, b"a", b"z").unwrap_err();
    assert!(err.msg.contains("delete failed"));
    set_delete_error(&cli, None);

    // Commit 失败经 randGen 的 errCh 返回。
    set_commit_error(&cli, Some(Error::new("commit failed")));
    let err = randGen(&Context::Background(), &cli, b"aa", b"zz", 4, 0).unwrap_err();
    assert!(
        err.msg.contains("commit failed"),
        "expected commit failed, got {}",
        err.msg
    );
    set_commit_error(&cli, None);

    // Begin and Set failures from workers must reach randGen's error channel.
    set_begin_error(&cli, Some(Error::new("worker begin failed")));
    let err = randGen(&Context::Background(), &cli, b"aa", b"zz", 4, 0).unwrap_err();
    assert!(err.msg.contains("worker begin failed"));
    set_begin_error(&cli, None);

    set_set_error(&cli, Some(Error::new("set failed")));
    let err = randGen(&Context::Background(), &cli, b"aa", b"zz", 4, 0).unwrap_err();
    assert!(err.msg.contains("set failed"));
    set_set_error(&cli, None);
}

/// 资源清理：超时取消不挂起；删除后区间空；concurrency 原子可更新。
fn contract_resource_cleanup() {
    let addr = "parity-txn-cleanup:2379";
    set_new_client_error(addr, None);
    let cli = createClient(addr, "", "", "").unwrap();
    clear_store(&cli);

    // duration=0 立即取消工人；必须在数秒内返回。
    let start = std::time::Instant::now();
    randGenWithDuration(&cli, b"aa", b"zz", 8, 4, 0).expect("duration 0");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "randGenWithDuration must not hang after cancel"
    );

    // Go context.WithTimeout treats a negative duration as already expired.
    clear_store(&cli);
    randGenWithDuration(&cli, b"aa", b"zz", 8, 0, -1).expect("negative duration");
    assert!(
        store_snapshot(&cli).is_empty(),
        "negative duration must cancel before the first transaction"
    );

    // 删除后区间键清空；drop 客户端无泄漏断言（桩侧）。
    put_raw(&cli, b"aa1".to_vec(), b"v".to_vec());
    put_raw(&cli, b"mm".to_vec(), b"v".to_vec());
    deleteRange(&cli, b"aa", b"zz").unwrap();
    assert!(store_snapshot(&cli).is_empty());
    drop(cli);

    // parse_flags 会更新 CONCURRENCY 原子，供后续工人读取。
    let _ = parse_flags(&["--concurrency".into(), "7".into()]);
    assert_eq!(crate::client::CONCURRENCY.load(Ordering::SeqCst), 7);
}

/// 重放 Go checksum 循环：while Valid { Next; if Key empty break; write; xor }。
/// 首条经 Next 跳过，故 entries[0] 不参与 XOR。
fn go_checksum_replay(entries: &[(&[u8], &[u8])]) -> u64 {
    let mut digest = stubs::Crc64Digest::new_ecma();
    let mut res = 0u64;
    // 空表时 idx=-1，循环不进入。
    let mut idx: isize = if entries.is_empty() { -1 } else { 0 };
    while idx >= 0 && (idx as usize) < entries.len() {
        idx += 1;
        if idx as usize >= entries.len() {
            // 耗尽 → 空键 → break（不写 digest）。
            break;
        }
        let (k, v) = entries[idx as usize];
        if k.is_empty() {
            break;
        }
        digest.Write(k);
        digest.Write(v);
        res ^= digest.Sum64();
    }
    res
}
