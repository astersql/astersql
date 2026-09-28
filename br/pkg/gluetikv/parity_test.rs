// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/gluetikv` vs Go `glue.go`.
//
// 本文件对 Rust `Glue` 与 Go `br/pkg/gluetikv/glue.go` 的公开契约做对照断言。
// 覆盖版本串、存储所有权、会话桩、进度计数、Open SSL 副作用与控制台嵌入；
// 失败即表示 Go/Rust 语义漂移，而不是业务回归用例。

use std::sync::Arc;

use astersql_br_pkg_glue::{ClientCLP, Context, Glue as GlueTrait, SecurityOption, Storage};
use astersql_config::{get_global_config, store_global_config};

use crate::{Glue, set_open_hook_for_test, take_records_for_test};

#[test]
fn go_rust_public_contract_matches() {
    let g = Glue::new();

    // normal: GetVersion matches Go TestGetVersion shape
    // 与 Go 一致：版本必须以 "BR\n" 开头，并含 Release Version / Git Commit Hash。
    let ver = g.GetVersion();
    assert!(
        ver.starts_with("BR\n"),
        "version must start with BR\\n, got {ver:?}"
    );
    assert!(ver.contains("Release Version"), "{ver}");
    assert!(ver.contains("Git Commit Hash"), "{ver}");

    // boundary: OwnsStorage / GetClient constants
    // Go glue 声明拥有 storage，且客户端类型固定为 ClientCLP。
    assert!(g.OwnsStorage());
    assert_eq!(g.GetClient(), ClientCLP);

    // GetDomain / CreateSession: Go returns (nil, nil) — no error
    // TiKV glue 不真正建 Domain/Session，成功但无实质对象，对应 Go 的 (nil, nil)。
    struct Dummy;
    impl Storage for Dummy {}
    let store = Dummy;
    assert!(g.GetDomain(&store).is_ok());
    assert!(g.CreateSession(&store).is_ok());

    // UseOneShotSession: Go returns nil without calling fn
    // 一次性会话在 TiKV 路径是空操作：回调不得被调用，否则语义偏离 Go。
    let mut called = false;
    g.UseOneShotSession(&store, false, &mut |_se| {
        called = true;
        Ok(())
    })
    .unwrap();
    assert!(!called, "UseOneShotSession must not invoke fn");

    // side-effect: Record → CollectSuccessUnit(name, 1, val)
    // Record 映射到 CollectSuccessUnit，计数恒为 1，便于与 Go 侧指标对齐。
    let _ = take_records_for_test();
    g.Record("test-unit", 7);
    let recs = take_records_for_test();
    assert_eq!(recs, vec![("test-unit".to_string(), 1, 7)]);

    // progress: Inc / Close contract (utils.StartProgress stand-in)
    // 进度条是 utils.StartProgress 的替身：Inc/IncBy 累加，Close 结束生命周期。
    let p = g.StartProgress(Context::new(), "cmd", 10, true);
    p.Inc();
    p.IncBy(2);
    assert_eq!(p.GetCurrent(), 3);
    p.Close();

    // Open: CAPath writes cluster SSL into global config (Go Open)
    // Open 在挂接 hook 前会把 SecurityOption 写入全局 cluster SSL，对齐 Go Open。
    // 测试结束必须还原配置并清空 hook，避免污染其他用例。
    let prev = (*get_global_config()).clone();
    set_open_hook_for_test(Some(Arc::new(|path: &str, opt: SecurityOption| {
        assert_eq!(path, "tikv://pd:2379");
        assert_eq!(opt.CAPath, "/ca.pem");
        Ok(Box::new(Dummy) as Box<dyn Storage>)
    })));
    let opened = g
        .Open(
            "tikv://pd:2379",
            SecurityOption {
                CAPath: "/ca.pem".into(),
                CertPath: "/cert.pem".into(),
                KeyPath: "/key.pem".into(),
            },
        )
        .expect("open");
    assert_eq!(opened.name(), "storage");
    let conf = get_global_config();
    assert_eq!(conf.security.cluster_ssl_ca, "/ca.pem");
    assert_eq!(conf.security.cluster_ssl_cert, "/cert.pem");
    assert_eq!(conf.security.cluster_ssl_key, "/key.pem");
    store_global_config(prev);
    set_open_hook_for_test(None);

    // Go guards all three assignments with CAPath != "": cert/key alone do not mutate config.
    let prev = (*get_global_config()).clone();
    let mut sentinel = prev.clone();
    sentinel.security.cluster_ssl_ca = "sentinel-ca".into();
    sentinel.security.cluster_ssl_cert = "sentinel-cert".into();
    sentinel.security.cluster_ssl_key = "sentinel-key".into();
    store_global_config(sentinel.clone());
    set_open_hook_for_test(Some(Arc::new(|_path, _option| {
        Ok(Box::new(Dummy) as Box<dyn Storage>)
    })));
    g.Open(
        "tikv://pd:2379",
        SecurityOption {
            CAPath: String::new(),
            CertPath: "/ignored-cert.pem".into(),
            KeyPath: "/ignored-key.pem".into(),
        },
    )
    .expect("open without CA");
    let unchanged = get_global_config();
    assert_eq!(unchanged.security.cluster_ssl_ca, "sentinel-ca");
    assert_eq!(unchanged.security.cluster_ssl_cert, "sentinel-cert");
    assert_eq!(unchanged.security.cluster_ssl_key, "sentinel-key");
    set_open_hook_for_test(None);
    store_global_config(prev);

    // error path: open hook failure propagates
    // hook 返回错误时 Open 必须原样向上传播，不得吞掉。
    set_open_hook_for_test(Some(Arc::new(|_p, _o| Err(astersql_errors::New("boom")))));
    let err = g
        .Open("tikv://x", SecurityOption::default())
        .err()
        .expect("expected error");
    let msg = format!("{err:?}");
    assert!(msg.contains("boom"), "{msg}");
    set_open_hook_for_test(None);

    // default Open must use TiKVDriver path validation rather than fixed success.
    // 非 tikv:// 路径在连接外部集群前就应该失败，可作为真实 driver 接线证据。
    let err = g
        .Open("not-a-tikv-path", SecurityOption::default())
        .err()
        .expect("invalid TiKV path must fail");
    assert!(format!("{err:?}").contains("tikv://"), "{err:?}");
    let err = g
        .Open("tikv://", SecurityOption::default())
        .err()
        .expect("empty PD authority must fail");
    assert!(
        format!("{err:?}").contains("PD address is empty"),
        "{err:?}"
    );
    let err = g
        .Open(
            "tikv://pd:2379?disableGC=invalid",
            SecurityOption::default(),
        )
        .err()
        .expect("invalid disableGC must fail");
    assert!(format!("{err:?}").contains("disableGC"), "{err:?}");
    let opened = g
        .Open(
            "tikv://pd-1:2379,pd-2:2379?disableGC=true&keyspaceName=ks",
            SecurityOption::default(),
        )
        .expect("valid TiKV path reaches the injected external boundary");
    assert_eq!(
        opened.name(),
        "tikv://pd-1:2379,pd-2:2379?disableGC=true&keyspaceName=ks"
    );

    // console glue embedded (Go: Glue embeds StdIOGlue)
    // Go 通过嵌入 StdIOGlue 提供控制台能力；Rust 侧 AsConsoleGlue 应非空。
    // 缺控制台 glue 会导致依赖 StdIO 的 CLI 路径无法对齐 Go。
    assert!(g.AsConsoleGlue().is_some());
}
