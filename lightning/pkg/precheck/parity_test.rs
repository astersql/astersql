// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/precheck` public contracts vs Go.
//! 这些测试只保护公开契约，不尝试连接真实外部依赖。
//! 如果常量、零值或跳过语义发生漂移，这里会第一时间暴露兼容性回归。
//! 测试按正常路径、边界条件、错误传播和资源收尾四组组织，
//! 对应 Go 包最容易被上层代码依赖的几类可观察行为。
//! 维护者阅读失败用例时，可以直接把断言映射回 Go 同名接口约束。
//! 因此这里宁可多解释契约原因，也不追求最短测试注释。

use crate::context;
use crate::errors;
use crate::*;

#[test]
fn go_rust_public_contract_matches() {
    // 入口函数只负责编排四类场景，
    // 让失败位置能直接映射到契约类别。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

fn contract_normal() {
    // CheckType constants match Go string values.
    // 这里直接锁定字符串字面值，
    // 避免有人把性能告警误改成别的英文词。
    assert_eq!(Critical, "critical");
    assert_eq!(Warn, "performance");

    // CheckItemID constants + DisplayName map (Go checkItemIDToDisplayName).
    // 展示名会被 CLI、日志和测试快照直接消费，
    // 所以常量和值都必须稳定。
    assert_eq!(CheckLargeDataFile, "CHECK_LARGE_DATA_FILES");
    assert_eq!(DisplayName(CheckLargeDataFile), "Large data file");
    assert_eq!(DisplayName(CheckSourcePermission), "Source permission");
    assert_eq!(DisplayName(CheckTargetTableEmpty), "Target table empty");
    assert_eq!(DisplayName(CheckSourceSchemaValid), "Source schema valid");
    assert_eq!(DisplayName(CheckCheckpoints), "Checkpoints");
    assert_eq!(DisplayName(CheckCSVHeader), "CSV header");
    assert_eq!(DisplayName(CheckTargetClusterSize), "Target cluster size");
    assert_eq!(
        DisplayName(CheckTargetClusterEmptyRegion),
        "Target cluster empty region"
    );
    assert_eq!(
        DisplayName(CheckTargetClusterRegionDist),
        "Target cluster region dist"
    );
    assert_eq!(
        DisplayName(CheckTargetClusterVersion),
        "Target cluster version"
    );
    assert_eq!(DisplayName(CheckLocalDiskPlacement), "Local disk placement");
    assert_eq!(DisplayName(CheckLocalTempKVDir), "Local temp KV dir");
    assert_eq!(
        DisplayName(CheckTargetUsingCDCPITR),
        "Target using CDC/PITR"
    );
    assert_eq!(
        DisplayName(CheckPDTiDBFromSameCluster),
        "PD and TiDB are from the same cluster"
    );

    // Passing CheckResult via Checker.
    // 成功结果必须完整携带 Item、Severity 和 Message，
    // trait 适配层不能悄悄裁剪字段。
    let mut ok = MockChecker {
        id: CheckCheckpoints,
        result: Some(CheckResult {
            Item: CheckCheckpoints,
            Severity: Warn,
            Passed: true,
            Message: "ok".into(),
        }),
        err: None,
        check_calls: 0,
        closed: false,
    };
    assert_eq!(ok.GetCheckItemID(), CheckCheckpoints);
    let res = ok.Check(context::Background()).unwrap().unwrap();
    assert!(res.Passed);
    assert_eq!(res.Severity, Warn);
    assert_eq!(res.Message, "ok");
    assert_eq!(ok.check_calls, 1);
}

fn contract_boundary() {
    // Unknown CheckItemID -> empty display name (Go map miss).
    // 未命中时返回空串而不是 `Option`，
    // 这是 Go map 零值语义的一部分。
    assert_eq!(DisplayName("CHECK_DOES_NOT_EXIST"), "");
    assert_eq!(DisplayName(""), "");

    // Zero-value CheckResult matches Go struct zero.
    // 很多调用方会依赖默认值可直接比较或填充，
    // 因此零值行为必须稳定。
    let zero = CheckResult::default();
    assert_eq!(zero.Item, "");
    assert_eq!(zero.Severity, "");
    assert!(!zero.Passed);
    assert_eq!(zero.Message, "");

    // Skipped check: Go returns nil *CheckResult with nil error.
    // 跳过检查不是失败，也不是通过，
    // 这里特意验证它被编码成 `Ok(None)`。
    let mut skipped = MockChecker {
        id: CheckCSVHeader,
        result: None,
        err: None,
        check_calls: 0,
        closed: false,
    };
    let out = skipped.Check(context::Background()).unwrap();
    assert!(out.is_none());
    assert_eq!(skipped.GetCheckItemID(), CheckCSVHeader);
}

fn contract_error() {
    // 错误路径必须把原始错误文本直接透传，
    // 否则上层就无法复用 Go 侧已有判错逻辑。
    let mut failing = MockChecker {
        id: CheckSourcePermission,
        result: None,
        err: Some(errors::New("permission denied")),
        check_calls: 0,
        closed: false,
    };
    let err = failing.Check(context::Background()).unwrap_err();
    assert_eq!(err.Error(), "permission denied");
    assert_eq!(failing.check_calls, 1);

    // Failed (not skipped) result still returns Ok(Some(...)).
    // 执行完成但检查失败，仍然属于有效结果对象，
    // 区别只在 `Passed = false`。
    let mut failed = MockChecker {
        id: CheckTargetTableEmpty,
        result: Some(CheckResult {
            Item: CheckTargetTableEmpty,
            Severity: Critical,
            Passed: false,
            Message: "table not empty".into(),
        }),
        err: None,
        check_calls: 0,
        closed: false,
    };
    let res = failed.Check(context::Background()).unwrap().unwrap();
    assert!(!res.Passed);
    assert_eq!(res.Severity, Critical);
    assert_eq!(res.Message, "table not empty");
}

fn contract_resource_cleanup() {
    // Checker "resource" is released after Check; subsequent Close marks cleanup.
    // 这里不模拟真实资源，只验证调用顺序允许先检查再清理。
    let mut checker = MockChecker {
        id: CheckLocalTempKVDir,
        result: Some(CheckResult {
            Item: CheckLocalTempKVDir,
            Severity: Critical,
            Passed: true,
            Message: String::new(),
        }),
        err: None,
        check_calls: 0,
        closed: false,
    };
    let _ = checker.Check(context::Background()).unwrap();
    checker.Close();
    assert!(checker.closed);
    assert_eq!(checker.check_calls, 1);
}

/// Test double implementing `Checker`, mirroring Go mock usage in importinto tests.
/// 这个 mock 只保留当前 trait 所需的最小状态，
/// 避免契约测试被无关依赖噪音干扰。
struct MockChecker {
    id: CheckItemID,
    result: Option<CheckResult>,
    err: Option<errors::Error>,
    check_calls: i32,
    closed: bool,
}

impl MockChecker {
    // `Close` 只记录收尾是否发生，
    // 对应 Go 侧测试里“资源已释放”的最小断言。
    fn Close(&mut self) {
        self.closed = true;
    }
}

impl Checker for MockChecker {
    fn Check(
        &mut self,
        _ctx: context::Context,
    ) -> std::result::Result<Option<CheckResult>, errors::Error> {
        // 先消费一次预设错误，
        // 模拟 Go 测试里常见的“下一次调用失败”注入方式。
        self.check_calls += 1;
        if let Some(err) = self.err.take() {
            return Err(err);
        }
        Ok(self.result.clone())
    }

    fn GetCheckItemID(&self) -> CheckItemID {
        // 即使检查被跳过或报错，调用方仍能通过 ID 识别检查项。
        self.id
    }
}
