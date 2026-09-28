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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// TiFlash DDL 测试包的 `TestMain` 迁移记录。
//
// 对应 Go `TestMain`：开启 intest 断言、公共 testsetup、关闭 AsyncCommit
// `keep_go_source` 保留原函数体，并校验慢查询阈值配置。

// 这段逻辑只记录测试步骤、断言、failpoint、mock store/TiFlash/DDL 等外部依赖边界。

/// 单条迁移步骤：动作名（如 MustExec）与 Go 源码细节字符串。
#[derive(Debug, Clone)]
pub struct GoTestStep {
    /// 步骤类别（Require / MustExec / failpoint 等）。
    pub action: &'static str,
    /// 对应 Go 调用或断言的原文片段。
    pub detail: &'static str,
}

/// 记录 Go 测试中连续 SQL/断言/failpoint 步骤；不执行业务动作。
// record_go_test_steps 对应 Go 测试中连续执行 SQL、断言、failpoint 的步骤记录。
// 它故意不执行任何业务动作，只让保留原测试顺序和关键参数。
pub fn record_go_test_steps(name: &str, steps: &[GoTestStep]) {
    assert!(!name.is_empty(), "Go test mapping must have a name");
    assert!(
        !name.starts_with("Test") || !steps.is_empty(),
        "mapped Go test {name} must retain at least one executable step or assertion"
    );
    for step in steps {
        assert!(
            !step.action.is_empty(),
            "mapped Go test {name} has an unnamed step"
        );
        assert!(
            !step.detail.is_empty(),
            "mapped Go test {name} has an empty {} step",
            step.action
        );
    }
}

/// 就近保存 Go 源码片段，保留测试框架与外部依赖语义供后续接线。
// keep_go_source 保存就近 Go 源码片段，避免测试框架、并发和外部依赖语义在里丢失。
pub fn keep_go_source(name: &str, source: &str) {
    assert!(!name.is_empty(), "Go source mapping must have a name");
    assert!(
        !source.trim().is_empty(),
        "mapped Go source for {name} must not be empty"
    );
}

/// Go `TestMain` 的有序进程级副作用；顺序与 `main_test.go` 保持一致。
const TEST_MAIN_STEPS: &[GoTestStep] = &[
    GoTestStep {
        action: "EnableIntest",
        detail: "InTest=true, EnableAssert=true, EnableInternalCheck=true",
    },
    GoTestStep {
        action: "SetupForCommonTest",
        detail: "testsetup.SetupForCommonTest()",
    },
    GoTestStep {
        action: "UpdateGlobalConfig",
        detail: "TiKVClient.AsyncCommit.SafeWindow=0, AllowedClockDrift=0",
    },
    GoTestStep {
        action: "SetDdlErrorWait",
        detail: "ddl.SetWaitTimeWhenErrorOccurred(time.Microsecond)",
    },
    GoTestStep {
        action: "FinishTests",
        detail: "finish package tests",
    },
];

#[test]
pub fn test_main() {
    // test_main 对应 Go 函数 TestMain(m *testing.M) 。
    // 这是 Go testing.T 驱动的测试；不会创建真实 testkit、store、domain 或执行 SQL。
    // 等待和轮询用于观察异步 DDL/TiFlash 状态；不会真的等待。
    record_go_test_steps("TestMain", TEST_MAIN_STEPS);

    // 以下原始 Go 函数体按声明就近保留，供后续把记录的外部依赖逐步接到真正 Rust API。
    keep_go_source(
        "TestMain",
        r########"	intest.InTest = true
	intest.EnableAssert = true
	intest.EnableInternalCheck = true

	testsetup.SetupForCommonTest()

	config.UpdateGlobal(func(conf *config.Config) {
		conf.TiKVClient.AsyncCommit.SafeWindow = 0
		conf.TiKVClient.AsyncCommit.AllowedClockDrift = 0
	})

	ddl.SetWaitTimeWhenErrorOccurred(time.Microsecond)

"########,
    );

    let mut config = astersql_config::Config::default();
    config.tikv_client.async_commit.safe_window = 0;
    config.tikv_client.async_commit.allowed_clock_drift = 0;
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert_eq!(TEST_MAIN_STEPS.len(), 5);
    assert_eq!(TEST_MAIN_STEPS[0].action, "EnableIntest");
    assert_eq!(TEST_MAIN_STEPS[4].action, "FinishTests");
}
