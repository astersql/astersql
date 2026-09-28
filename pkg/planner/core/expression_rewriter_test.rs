// Copyright 2026 AsterSQL.

// 表达式重写器工厂安装的单元测试。
//
// 校验规划器表达式工厂（`InstallPlannerExpressionFactory`）可重复安装且幂等：
// 多次调用不会因重复注册而失败。

/// 验证表达式工厂安装是幂等的：连续两次安装均应成功。
#[test]
fn planner_factory_installation_is_idempotent() {
    crate::InstallPlannerExpressionFactory().unwrap();
    crate::InstallPlannerExpressionFactory().unwrap();
}
