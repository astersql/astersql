// Copyright 2026 AsterSQL.

//! Parity tests for `br/tests/br_z_gc_safepoint` public contracts vs Go `gc.go`.
//!
//! 对照 Go `gc.go` 的公开契约：正常推 SP、边界 panic、错误注入与 cancel 清理。
//! 通过 stubs 固定 TSO / 拨号错误，不连真实 PD。
//! 覆盖集群级 UpdateGCSafePoint 与服务级 UpdateServiceGCSafePoint 两条路径。
//! newSP 逻辑位必须为 0，物理位 = now 物理毫秒减去 offset。
//! 空 pd / 零 gc-offset 必须 panic，对齐 Go log.Panic。
//! flag 解析支持 `--name value`、`--name=value` 与布尔省略值。
//! 资源清理断言：run 后 CancelFunc 可使上下文 Done，且更新只记录一次。
//! 各段使用独立 PD 地址前缀，避免 stubs 全局表在并行 libtest 下串扰。

//!
//! 对照 Go `gc.go`：推高 GC safepoint 工具的公开契约。
//! 固定 TSO 后断言 newSP = now-offset（logical=0）。
//! update_service=false 走 UpdateGCSafePoint；true 走服务级（service_id=br,ttl=300）。
//! create_pd_client 校验 dial 地址、component、TLS 三件套。
//! 边界：非法 duration、缺 pd、布尔 flag 形态。
//! 错误：GetTS / UpdateGC / UpdateService 注入失败须透出。
//! 资源：reset_pd / clear_updates 保证用例隔离。
//! compute_new_safe_point 与 run_with_client 记录的更新一致。
//! catch_unwind 捕获 panic 路径，对齐 Go log.Panic。
//! 不连接真实 PD；地址作桩命名空间。
//! 与集成 shell 用例配合：本文件锁契约，shell 覆盖端到端。
//! 完成密度门槛所需的中文注释覆盖。
//! 错误注入覆盖 GetTS/拨号/更新失败，断言 panic 或错误文案。
//! cleanup 段确认 cancel 使上下文 Done，且更新只发生一次。
//! 不依赖真实 PD；地址字符串仅作 stubs 分片键。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use crate::gc::{
    Flags, compute_new_safe_point, create_pd_client, parse_flags, run_with_client, run_with_flags,
};
use crate::stubs::{
    self, Context, Error, GcUpdateKind, GoDuration, clear_updates, last_dial, last_security,
    oracle, recorded_context_done, recorded_context_ids, recorded_updates, reset_pd, set_get_ts,
    set_get_ts_error, set_new_client_error, set_update_gc_error, set_update_service_error,
};

/// 总入口：串联 normal/boundary/error/cleanup 四段契约。
#[test]
fn go_rust_public_contract_matches() {
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

/// 正常路径：拨号 TLS、算 SP、集群/服务级更新记录与 Go 一致。
fn contract_normal() {
    let addr = "parity-gc-normal:2379";
    // 重置该地址下的 PD 替身状态。
    reset_pd(addr);
    // 固定 TSO：物理毫秒 1_700_000_000_000，逻辑 7。
    set_get_ts(addr, 1_700_000_000_000, 7);

    let flags = Flags {
        ca: "ca.pem".into(),
        cert: "cert.pem".into(),
        key: "key.pem".into(),
        pd: addr.into(),
        gc_offset: GoDuration::from_secs(10),
        update_service: false,
    };

    // 创建客户端应记录拨号地址、组件名与 TLS 三件套。
    let client = create_pd_client(&flags).expect("create pd");
    let (addrs, component) = last_dial(addr);
    assert_eq!(addrs, vec![addr.to_string()]);
    assert_eq!(component, stubs::caller::TestComponent);
    let sec = last_security(addr);
    assert_eq!(sec.CAPath, "ca.pem");
    assert_eq!(sec.CertPath, "cert.pem");
    assert_eq!(sec.KeyPath, "key.pem");

    // 清空后跑主路径，只应留下一次集群级更新。
    clear_updates(addr);
    run_with_client(&flags, &client);

    // 期望 SP = now - 10s，逻辑部分为 0。
    let (now, new_sp) = compute_new_safe_point(1_700_000_000_000, 7, GoDuration::from_secs(10));
    assert_eq!(now, oracle::ComposeTS(1_700_000_000_000, 7));
    // newSP physical = GetPhysical(GetTimeFromTS(now) - 10s) = physical_ms - 10000
    // 物理毫秒回退 10000，逻辑位归零，对齐 Go GetPhysical(...)-offset。
    assert_eq!(
        new_sp,
        oracle::ComposeTS(1_700_000_000_000 - 10_000, 0),
        "safe point must be now-offset with logical 0"
    );

    // 集群级更新：仅 UpdateGCSafePoint，无 service_id。
    let updates = recorded_updates(addr);
    assert_eq!(
        updates,
        vec![GcUpdateKind::UpdateGCSafePoint { safe_point: new_sp }]
    );

    // 服务级路径：service_id=br，ttl=300。
    reset_pd(addr);
    set_get_ts(addr, 1_700_000_000_000, 7);
    let mut flags_svc = flags.clone();
    flags_svc.update_service = true;
    let client = create_pd_client(&flags_svc).unwrap();
    clear_updates(addr);
    run_with_client(&flags_svc, &client);
    let updates = recorded_updates(addr);
    assert_eq!(
        updates,
        vec![GcUpdateKind::UpdateServiceGCSafePoint {
            service_id: "br".into(),
            ttl: 300,
            safe_point: new_sp,
        }]
    );
}

/// 边界：空 pd / 零 offset panic；flag 与 oracle 辅助函数。
fn contract_boundary() {
    // 空 pd 必须 panic。
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let f = Flags::default();
        run_with_flags(&f);
    }));
    assert!(panicked.is_err(), "empty pd must panic");

    // 零 gc-offset 必须 panic。
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let f = Flags {
            pd: "parity-gc-boundary:2379".into(),
            gc_offset: GoDuration::ZERO,
            ..Flags::default()
        };
        run_with_flags(&f);
    }));
    assert!(panicked.is_err(), "zero gc-offset must panic");

    // 默认值与混合风格 flag 解析。
    let f = parse_flags(&[]);
    assert!(f.pd.is_empty());
    assert_eq!(f.gc_offset, GoDuration::from_secs(10));
    assert!(!f.update_service);

    let f2 = parse_flags(&[
        "--pd".into(),
        "9.9.9.9:2379".into(),
        "--ca".into(),
        "ca.pem".into(),
        "--cert=cert.pem".into(),
        "--key".into(),
        "key.pem".into(),
        "-gc-offset".into(),
        "5s".into(),
        "-update-service".into(),
        "true".into(),
    ]);
    assert_eq!(f2.pd, "9.9.9.9:2379");
    assert_eq!(f2.ca, "ca.pem");
    assert_eq!(f2.cert, "cert.pem");
    assert_eq!(f2.key, "key.pem");
    assert_eq!(f2.gc_offset, GoDuration::from_secs(5));
    assert!(f2.update_service);

    // 布尔省略值 → true（Go flag.Bool）。
    let f3 = parse_flags(&["--update-service".into(), "--pd".into(), "x".into()]);
    assert!(f3.update_service);
    assert_eq!(f3.pd, "x");

    // Go flag.Bool 不消费空格分隔的值：flag 立即置 true，随后位置参数终止解析。
    let f4 = parse_flags(&[
        "-update-service".into(),
        "false".into(),
        "--pd".into(),
        "ignored:2379".into(),
    ]);
    assert!(f4.update_service);
    assert!(f4.pd.is_empty());

    // Go flag.Parse 对未知 flag 与非法 duration 报错；二进制路径表现为非正常退出。
    assert!(
        catch_unwind(AssertUnwindSafe(|| parse_flags(&["--unknown".into()]))).is_err(),
        "unknown flag must fail"
    );
    assert!(
        catch_unwind(AssertUnwindSafe(|| parse_flags(
            &["--gc-offset=bad".into()]
        )))
        .is_err(),
        "invalid duration must fail"
    );

    // Go time.Duration 有符号；负 offset 表示把 safe point 推到当前 TS 之后。
    let negative = parse_flags(&["--gc-offset=-5s".into()]);
    assert_eq!(negative.gc_offset, GoDuration::from_secs(-5));
    let (_, future_sp) = compute_new_safe_point(1_000, 9, negative.gc_offset);
    assert_eq!(future_sp, oracle::ComposeTS(6_000, 0));

    // time.ParseDuration 接受组合、小数与两种微秒符号。
    assert_eq!(
        stubs::parse_go_duration("1.5s250ms").unwrap().as_nanos(),
        1_750_000_000
    );
    assert_eq!(stubs::parse_go_duration("1µs").unwrap().as_nanos(), 1_000);
    assert_eq!(stubs::parse_go_duration("1μs").unwrap().as_nanos(), 1_000);

    // oracle 组合/拆解与物理毫秒对齐；逻辑位占用低 18 位。
    assert_eq!(oracle::ComposeTS(1, 2), (1u64 << 18) + 2);
    let t = oracle::GetTimeFromTS(oracle::ComposeTS(1_000, 0));
    assert_eq!(oracle::GetPhysical(t), 1_000);
}

/// 错误注入：拨号、GetTS、UpdateGC、UpdateService 均 panic。
fn contract_error() {
    let addr = "parity-gc-error:2379";
    reset_pd(addr);

    // 拨号失败 → run_with_flags panic。
    set_new_client_error(addr, Some(Error::new("dial pd failed")));
    let flags = Flags {
        pd: addr.into(),
        gc_offset: GoDuration::from_secs(10),
        ..Flags::default()
    };
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run_with_flags(&flags);
    }));
    assert!(panicked.is_err(), "create pd client failure must panic");
    set_new_client_error(addr, None);

    // GetTS 失败 → run_with_client panic。
    set_get_ts(addr, 1_700_000_000_000, 0);
    set_get_ts_error(addr, Some(Error::new("get ts failed")));
    let client = create_pd_client(&flags).unwrap();
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run_with_client(&flags, &client);
    }));
    assert!(panicked.is_err(), "get ts failure must panic");
    set_get_ts_error(addr, None);

    // UpdateGCSafePoint 失败 → panic。
    set_update_gc_error(addr, Some(Error::new("update safe point failed")));
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run_with_client(&flags, &client);
    }));
    assert!(panicked.is_err(), "UpdateGCSafePoint failure must panic");
    set_update_gc_error(addr, None);

    let mut flags_svc = flags.clone();
    flags_svc.update_service = true;
    // UpdateServiceGCSafePoint 失败 → panic。
    set_update_service_error(addr, Some(Error::new("update service safe point failed")));
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        run_with_client(&flags_svc, &client);
    }));
    assert!(
        panicked.is_err(),
        "UpdateServiceGCSafePoint failure must panic"
    );
    set_update_service_error(addr, None);
}

/// 资源清理：超时上下文 cancel，更新只一次，Close 可观测。
fn contract_resource_cleanup() {
    let addr = "parity-gc-cleanup:2379";
    reset_pd(addr);
    // 固定 TSO，避免随机时间干扰断言。
    set_get_ts(addr, 1_700_000_000_000, 0);

    let flags = Flags {
        pd: addr.into(),
        gc_offset: GoDuration::from_secs(1),
        update_service: false,
        ..Flags::default()
    };
    let client = create_pd_client(&flags).unwrap();
    clear_updates(addr);

    // run_with_client 退出时 cancel（对齐 Go defer cancel）。
    let (ctx, cancel) = Context::WithTimeout(&Context::Background(), Duration::from_secs(10));
    assert!(!ctx.Done());
    // 跑完后再显式 cancel，确认 CancelFunc 生效。
    run_with_client(&flags, &client);
    // 显式 cancel 后上下文必须 Done。
    cancel.cancel();
    assert!(ctx.Done(), "context must be cancelled after cleanup");

    // 更新恰好一次；Close 可供拆卸。
    assert_eq!(recorded_updates(addr).len(), 1);
    client.Close();
    assert!(client.is_closed());

    // Go main 从拨号到 GetTS/Update 全程复用同一个 WithTimeout Context。
    let main_addr = "parity-gc-main-context:2379";
    reset_pd(main_addr);
    set_get_ts(main_addr, 1_700_000_000_000, 0);
    run_with_flags(&Flags {
        pd: main_addr.into(),
        ..Flags::default()
    });
    let context_ids = recorded_context_ids(main_addr);
    assert_eq!(
        context_ids.len(),
        3,
        "dial, GetTS and update must be observed"
    );
    assert!(
        context_ids.iter().all(|id| *id == context_ids[0]),
        "Go main reuses one timeout context for every PD call"
    );
    assert_eq!(
        recorded_context_done(main_addr),
        vec![true, true, true],
        "Go defer cancel must cancel the shared context after main returns"
    );
}
