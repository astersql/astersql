// Copyright 2026 AsterSQL.

// `bootstraptest` 测试包入口。
//
// 挂接 bootstrap 系统库目录校验、升级 mock 与 harness 常量相关测试模块。

#![allow(dead_code)]

#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use astersql_testkit_testmain::{TestingM, WrapTestingM};

/// Go `TestMain` 的初始化与收尾顺序；Rust 无 TiKV Go 客户端或 goroutine 泄漏器，
/// 因此外部边界以显式契约保留，其余可观察副作用由下面的函数执行。
#[cfg(test)]
const GO_TEST_MAIN_ACTIONS: [&str; 6] = [
    "testmain.ShortCircuitForBench",
    "testsetup.SetupForCommonTest",
    "flag.Parse",
    "config.UpdateGlobal",
    "tikv.EnableFailpoints",
    "testmain.WrapTestingM",
];

/// 应用 Go `TestMain` 中可由 Rust 测试进程观察的初始化副作用。
#[cfg(test)]
fn apply_bootstraptest_harness_config() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_config::update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
    });
}

/// 表达 Go 等待 MVCCLevelDB 关闭一秒、再原样返回测试退出码的契约。
#[cfg(test)]
fn cleanup_exit_code_with<F>(status: i32, sleep: F) -> i32
where
    F: FnOnce(Duration),
{
    sleep(Duration::from_secs(1));
    status
}

/// 使用公共 `WrapTestingM` 在测试 runner 返回后执行清理回调。
#[cfg(test)]
fn wrap_bootstraptest_runner<'a, M, F>(runner: M, callback: F) -> WrapTestingM<'a, M>
where
    M: TestingM,
    F: Fn(i32) -> i32 + 'a,
{
    WrapTestingM(runner, Some(Box::new(callback)))
}

#[cfg(test)]
mod boot_test;
#[cfg(test)]
mod bootstrap_upgrade_test;
#[cfg(test)]
mod main_test;
