// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 分步 TestKit 的后台查询结果保留回归测试。
//
// 验证工作线程完成并被回收后，主线程仍能取得查询结果并继续断言。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use crate::Rows;
use crate::db_driver::{DbValue, QueryRows};
use crate::mockstore::{MockStore, MockStoreConfig};
use crate::stepped::{StepController, SteppedTestKit};

#[test]
fn continue_without_a_reached_breakpoint_fails_instead_of_pre_releasing_it() {
    let controller = StepController::default();

    let result = catch_unwind(AssertUnwindSafe(|| controller.resume("before-executor")));

    assert!(
        result.is_err(),
        "Go Continue requires the command to be stopped at a breakpoint"
    );
}

#[test]
fn continue_releases_exactly_one_reached_breakpoint() {
    let controller = StepController::default();
    let worker_controller = controller.clone();
    let worker = std::thread::spawn(move || worker_controller.checkpoint("before-executor"));

    controller.wait("before-executor", 1);
    controller.resume("before-executor");

    worker
        .join()
        .expect("checkpoint worker should not panic")
        .expect("reached checkpoint should be released");
}

#[test]
fn stepped_query_preserves_the_first_result_for_later_assertion() {
    let store = Arc::new(MockStore::new(MockStoreConfig::default()));
    store.expect_query(
        "select 1",
        QueryRows {
            columns: vec!["x".into()],
            rows: vec![vec![DbValue::String("1".into())]],
        },
    );
    let mut stepped = SteppedTestKit::new(store);
    // 查询在后台线程执行；`Wait` 回收线程时不得丢失已保存的结果。
    stepped.SteppedMustQuery("select 1", Vec::new()).Start();
    stepped.Wait().expect("stepped query");
    stepped.GetQueryResult().Check(Rows(&["1"]));
}
