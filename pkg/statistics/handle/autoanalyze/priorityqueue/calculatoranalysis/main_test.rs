// Copyright 2026 AsterSQL.

// `calculatoranalysis` 包级测试入口（对应 Go `TestMain`）。
//
// Rust 无包级 `TestMain`；此处保留公共 harness 初始化副作用与完整的
/// 调用公共测试初始化，确认不 panic（对应 Go `SetupForCommonTest`）。
#[test]
fn common_test_harness_setup_does_not_panic() {
    astersql_testkit_testsetup::SetupForCommonTest();
}
