// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/mock/mocklocal` public contracts vs Go MockGen.
//!
//! 校验 DiskUsage / TiKVModeSwitcher / StoreHelper 的录制-回放契约：
//! 正常返回、空切片边界、GetTS 错误传播、Codec 返回与 remaining 耗尽。
//! 不启动真实 TiKV；失败表示与 Go MockGen 行为漂移。
//! 各段使用独立 Controller，避免期望队列互相污染。

use astersql_br_pkg_mock::{Context, Controller, Error};

use crate::local::{NewMockDiskUsage, NewMockStoreHelper, NewMockTiKVModeSwitcher};
use crate::stubs::{Codec, EngineFileSize, Range};

/// 公开 API 对等：覆盖正路径、边界、错误与资源耗尽断言。
#[test]
fn go_rust_public_contract_matches() {
    // --- normal: DiskUsage.EngineFileSizes returns configured sizes ---
    // 正常：预置 sizes，回放相等且 Controller 无残留期望。
    let ctrl = Controller::new();
    let disk = NewMockDiskUsage(ctrl.clone());
    let sizes = vec![EngineFileSize {
        UUID: [1u8; 16],
        DiskSize: 1000,
        MemSize: 200,
        IsImporting: false,
    }];
    // Return1 预置单返回值，对齐 Go gomock Return。
    disk.EXPECT().EngineFileSizes().Return1(sizes.clone());
    let got = disk.EngineFileSizes();
    assert_eq!(got, sizes);
    // remaining==0 表示所有期望已被精确消费。
    assert_eq!(ctrl.remaining(), 0);

    // --- boundary: empty EngineFileSizes / empty ranges for mode switch ---
    // 边界：空引擎列表与空 Range 的模式切换均应合法耗尽。
    let ctrl2 = Controller::new();
    let disk2 = NewMockDiskUsage(ctrl2.clone());
    disk2
        .EXPECT()
        .EngineFileSizes()
        .Return1(Vec::<EngineFileSize>::new());
    assert!(disk2.EngineFileSizes().is_empty());

    let switcher = NewMockTiKVModeSwitcher(ctrl2.clone());
    switcher.EXPECT().ToImportMode(&(), &[]).Return(vec![]);
    switcher.EXPECT().ToNormalMode(&(), &[]).Return(vec![]);
    switcher.ToImportMode(Context::background(), &[]);
    switcher.ToNormalMode(Context::background(), &[]);
    assert_eq!(ctrl2.remaining(), 0);

    // --- error: StoreHelper.GetTS propagates error ---
    // 错误：GetTS 第三槽携带 Error，物理/逻辑时间戳仍为预置 0。
    let ctrl3 = Controller::new();
    let helper = NewMockStoreHelper(ctrl3.clone());
    helper.EXPECT().GetTS(&()).Return(vec![
        Box::new(0i64),
        Box::new(0i64),
        Box::new(Some(Error::new("tso unavailable"))),
    ]);
    let (phys, logical, err) = helper.GetTS(Context::background());
    assert_eq!(phys, 0);
    assert_eq!(logical, 0);
    assert!(err.is_some());
    assert!(err.unwrap().msg.contains("tso unavailable"));

    // --- resource cleanup: expected calls fully consumed; codec return ---
    // 资源：继续在同一 helper 上取 Codec，并确认 remaining 归零。
    helper.EXPECT().GetTiKVCodec().Return1(Codec { id: 7 });
    let codec = helper.GetTiKVCodec();
    assert_eq!(codec.id, 7);
    assert_eq!(ctrl3.remaining(), 0);

    // mode switch with ranges is recorded and drained
    // 带非空 Range 的 ToImportMode：录制仍用空占位参数，靠次序匹配。
    let ctrl4 = Controller::new();
    let switcher2 = NewMockTiKVModeSwitcher(ctrl4.clone());
    let ranges = [Range {
        start: b"a".to_vec(),
        end: b"z".to_vec(),
    }];
    switcher2.EXPECT().ToImportMode(&(), &[]).Return(vec![]);
    switcher2.ToImportMode(Context::background(), &ranges);
    assert_eq!(ctrl4.remaining(), 0);

    // Drop mocks (no hang / no panic) — resource teardown
    // 析构不应挂起或 panic（Controller 已耗尽期望）。
    drop(disk);
    drop(disk2);
    drop(switcher);
    drop(helper);
    drop(switcher2);
}

/// Go 的多返回值顺序必须保持为 physical、logical、error。
#[test]
fn get_ts_preserves_nonzero_values_and_nil_error() {
    let ctrl = Controller::new();
    let helper = NewMockStoreHelper(ctrl.clone());
    helper.EXPECT().GetTS(&()).Return(vec![
        Box::new(1_725_000_000_000i64),
        Box::new(42i64),
        Box::new(None::<Error>),
    ]);

    let (physical, logical, err) = helper.GetTS(Context::background());

    assert_eq!(physical, 1_725_000_000_000);
    assert_eq!(logical, 42);
    assert_eq!(err, None);
    assert_eq!(ctrl.remaining(), 0);
}

/// MockGen 的 error 返回槽既可承载 nil，也可承载具体 error。
#[test]
fn get_ts_accepts_a_concrete_error_return() {
    let ctrl = Controller::new();
    let helper = NewMockStoreHelper(ctrl.clone());
    helper.EXPECT().GetTS(&()).Return(vec![
        Box::new(11i64),
        Box::new(7i64),
        Box::new(Error::new("tso failed")),
    ]);

    let (physical, logical, err) = helper.GetTS(Context::background());

    assert_eq!((physical, logical), (11, 7));
    assert_eq!(
        err.expect("concrete error must be preserved").msg,
        "tso failed"
    );
    assert_eq!(ctrl.remaining(), 0);
}

/// Go 变参展开对 import/normal 两个方法及多个 Range 都应一致。
#[test]
fn mode_switcher_replays_both_variadic_methods_with_multiple_ranges() {
    let ctrl = Controller::new();
    let switcher = NewMockTiKVModeSwitcher(ctrl.clone());
    let ranges = [
        Range {
            start: b"a".to_vec(),
            end: b"m".to_vec(),
        },
        Range {
            start: b"n".to_vec(),
            end: b"z".to_vec(),
        },
    ];
    switcher.EXPECT().ToImportMode(&(), &[]).Return(vec![]);
    switcher.EXPECT().ToNormalMode(&(), &[]).Return(vec![]);

    switcher.ToImportMode(Context::background(), &ranges);
    switcher.ToNormalMode(Context::background(), &ranges);

    assert_eq!(ctrl.remaining(), 0);
}

/// 未登记的方法调用必须像 gomock 一样立即失败，不能固定成功。
#[test]
#[should_panic(expected = "Unexpected call to EngineFileSizes")]
fn unexpected_call_panics() {
    let disk = NewMockDiskUsage(Controller::new());
    let _ = disk.EngineFileSizes();
}
