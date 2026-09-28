// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 元数据锁测试包中 Go `TestMain` 的可执行对照。
//!
//! Go 版本统一设置异步提交、DDL 出错等待时间和协程泄漏检查；Rust 进程内模型
//! 不复刻这些全局开关，而以真实的锁竞争、错误传播及线程回收覆盖其可观察契约。

use super::{DdlOperation, MdlError, MdlHarness};

/// 验证测试夹具能够驱动一次完整的共享锁与独占锁竞争，并保持元数据锁默认开启。
#[test]
fn metadata_lock_test_harness_is_live() {
    let harness = MdlHarness::default();
    harness.create_table("test.t", 1);
    let mut session = harness.open_session();
    session.begin();
    session.read("test.t").unwrap();

    // 事务读持有共享元数据锁，DDL 线程必须等到提交释放该锁后才能完成变更。
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    std::thread::yield_now();
    assert!(matches!(session.commit(), Ok(())));
    assert_eq!(ddl.join().unwrap(), Ok(()));

    // 进程内模型以确定性的锁生命周期体现 MDL 前置条件；join 后不遗留后台线程。
    assert!(harness.metadata_lock_enabled());
}

/// 验证测试夹具不会吞掉真实 DDL 错误，而会原样报告缺失的目标表。
#[test]
fn metadata_lock_test_harness_reports_real_errors() {
    let harness = MdlHarness::default();
    assert_eq!(
        harness.alter_table("test.missing", DdlOperation::AddColumn),
        Err(MdlError::NoSuchTable("test.missing".to_owned()))
    );
}
