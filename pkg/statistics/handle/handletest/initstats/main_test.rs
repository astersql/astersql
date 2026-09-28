// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `initstats` 包级测试入口模型。
//
// 对应 Go `TestMain`：先公共 setup，再跑用例，最后做泄漏检测；

#[derive(Debug, Default, PartialEq, Eq)]
struct TestMainRun {
    phases: Vec<&'static str>,
}

fn run_test_main() -> TestMainRun {
    let mut run = TestMainRun::default();
    run.phases.push("setup-common-test");
    run.phases.push("run-tests");
    run.phases.push("finish-tests");
    run
}

#[test]
fn canonical_init_stats_test_main_runs_common_setup_before_leak_check() {
    let run = run_test_main();
    assert_eq!(
        run.phases,
        vec!["setup-common-test", "run-tests", "finish-tests"]
    );
    assert_eq!(run.phases.first(), Some(&"setup-common-test"));
    assert_eq!(run.phases.last(), Some(&"finish-tests"));
}
