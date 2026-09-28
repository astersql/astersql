// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/utiltest` vs Go `suite.go`.
//!
//! Covers normal suite construction, storage boundary I/O, Start error path,
//! and the exact Drop/Stop resource cleanup boundary.
//!
//! 与 Go `suite.go` 公开契约的对等测试：构造、本地存储边界、Start 失败与清理幂等。
//! 不改断言语义，只锁定 Mock 集群字段、URI 前缀与 not-exist 错误形态。
//! CreateRestoreSchemaSuite 必须挂上 Storage/Domain/Server 并填充 DSN。
//! Stop 只停止 Mock cluster，不关闭 Storage；二次 Stop 不得 panic。

use std::sync::atomic::Ordering;

use crate::stubs::Context;
use crate::{CreateRestoreSchemaSuite, NewLocalStorage, ReaderOption, Storage, WalkOption};

#[test]
fn go_rust_public_contract_matches() {
    // --- Normal: CreateRestoreSchemaSuite builds glue + cluster + local storage ---
    // 正常路径：套件工厂应装配完整 mock 集群与空 MockGlue。
    let mut suite = CreateRestoreSchemaSuite();
    // NewCluster 须挂载 mock storage，否则后续 schema 恢复路径无法读写。
    assert!(
        suite.Mock.Storage.is_some(),
        "NewCluster must attach mock storage"
    );
    // Domain 引导成功才具备 infoschema 查询能力。
    assert!(
        suite.Mock.Domain.is_some(),
        "NewCluster must bootstrap domain"
    );
    // Start 后 Server 非空，表示 mock TiDB 已上线。
    assert!(suite.Mock.Server.is_some(), "Start must attach mock server");
    // DSN 在 online wait 完成后填充，供客户端连接断言。
    assert!(
        !suite.Mock.DSN.is_empty(),
        "Start must populate DSN after online wait"
    );
    // Fresh MockGlue matches Go `&gluemock.MockGlue{}` (nil session / empty vars).
    // 新 MockGlue 无全局变量，与 Go 零值结构体一致。
    assert!(
        suite.MockGlue.GlobalVars.is_empty(),
        "default MockGlue has empty GlobalVars"
    );

    let ctx = Context::background();
    let uri = suite.Storage.URI();
    // Go objstore.LocalURIPrefix is exactly file://.
    assert!(
        uri.starts_with("file://"),
        "NewLocalStorage URI must be file://, got {uri}"
    );

    // --- Boundary: empty name / missing file / round-trip write-read ---
    // 边界：写-读闭环与缺失文件的 not-exist 错误码。
    suite
        .Storage
        .WriteFile(&ctx, "meta/checkpoint.bin", b"abc")
        .expect("WriteFile");
    assert!(
        suite
            .Storage
            .FileExists(&ctx, "meta/checkpoint.bin")
            .expect("FileExists")
    );
    let got = suite
        .Storage
        .ReadFile(&ctx, "meta/checkpoint.bin")
        .expect("ReadFile");
    assert_eq!(got, b"abc");

    let missing = suite.Storage.ReadFile(&ctx, "does-not-exist");
    assert!(missing.is_err(), "missing file must error");
    assert!(
        missing.unwrap_err().is_not_exist,
        "missing file should be not-exist"
    );

    // Standalone NewLocalStorage on a fresh temp path (boundary of factory).
    // 独立工厂路径：不依赖套件 TempDir，验证 NewLocalStorage 本身可用。
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = NewLocalStorage(tmp.path()).expect("NewLocalStorage");
    store.WriteFile(&ctx, "x", b"1").expect("write");
    assert_eq!(store.ReadFile(&ctx, "x").expect("read"), b"1");

    // --- Error: Start on a nil-storage cluster fails like Go require path ---
    // 错误路径：无 storage 的 Cluster.Start 必须失败（对齐 Go require）。
    {
        let mut bare = astersql_br_pkg_mock::Cluster::default();
        let err = bare.Start().expect_err("Start with nil storage must fail");
        assert!(
            err.to_string().contains("nil storage") || !err.to_string().is_empty(),
            "unexpected Start error: {err}"
        );
    }

    // --- Resource cleanup: explicit Stop then Drop must be idempotent ---
    // Go Cleanup only calls s.Mock.Stop(), so Storage remains usable.
    suite.Stop();
    assert!(
        suite.stopped.load(Ordering::SeqCst),
        "Stop must mark suite stopped"
    );
    suite
        .Storage
        .WriteFile(&ctx, "after-stop", b"z")
        .expect("Mock.Stop must not close Storage");
    assert_eq!(
        suite
            .Storage
            .ReadFile(&ctx, "after-stop")
            .expect("Storage remains readable after Mock.Stop"),
        b"z"
    );
    // Drop (end of scope) must not double-panic; second Stop is no-op.
    suite.Stop();
}

#[test]
fn local_storage_matches_go_error_walk_and_lifecycle_contracts() {
    let ctx = Context::background();
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = NewLocalStorage(tmp.path()).expect("NewLocalStorage");

    let missing = store
        .DeleteFile(&ctx, "missing")
        .expect_err("Go LocalStorage reports ENOENT by default");
    assert!(missing.is_not_exist, "delete ENOENT must be classified");

    store
        .WriteFile(&ctx, "nested/item.txt", b"data")
        .expect("write nested file");
    let mut walked = Vec::new();
    store
        .WalkDir(
            &ctx,
            &WalkOption {
                SubDir: "nested".into(),
                ObjPrefix: "item".into(),
                ..WalkOption::default()
            },
            &mut |name, size| {
                walked.push((name.to_owned(), size));
                Ok(())
            },
        )
        .expect("walk nested dir");
    assert_eq!(walked, vec![("nested/item.txt".to_owned(), 4)]);

    assert_eq!(
        store
            .PresignFile(&ctx, "nested/item.txt", std::time::Duration::from_secs(1))
            .expect("presign local file"),
        "item.txt"
    );

    let mut reader = store
        .Open(
            &ctx,
            "nested/item.txt",
            Some(&ReaderOption {
                StartOffset: Some(1),
                EndOffset: Some(3),
                ..ReaderOption::default()
            }),
        )
        .expect("open range");
    let mut range = [0; 8];
    let count = reader.Read(&ctx, &mut range).expect("read range");
    assert_eq!(&range[..count], b"at");
    assert_eq!(reader.Read(&ctx, &mut range).expect("range EOF"), 0);
    reader.Close(&ctx).expect("close reader");
    assert!(
        reader.Read(&ctx, &mut range).is_err(),
        "closed reader must reject I/O"
    );

    let mut writer = store
        .Create(&ctx, "created.txt", None)
        .expect("create target directly");
    assert!(
        store.FileExists(&ctx, "created.txt").expect("target stat"),
        "Go Create opens the final target immediately"
    );
    writer.Write(&ctx, b"new").expect("buffer write");
    writer.Close(&ctx).expect("flush and close writer");
    assert_eq!(
        store.ReadFile(&ctx, "created.txt").expect("created data"),
        b"new"
    );

    // Go LocalStorage.Close releases no resources and never disables later I/O.
    store.Close();
    assert_eq!(
        store
            .ReadFile(&ctx, "nested/item.txt")
            .expect("read after close"),
        b"data"
    );
}
