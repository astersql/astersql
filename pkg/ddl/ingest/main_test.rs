// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

///
/// 防止迁移时静默漏掉 Go 测试入口允许的长期后台任务。
/// 执行测试主体，再把原始退出码交给 ingest 泄漏清理回调。
///
/// 的关键顺序和退出码契约；具体 tracker/backend/metrics 检查由 testutil 实现。
fn run_test_main(run_suite: impl FnOnce() -> i32, cleanup: impl FnOnce(i32)) -> i32 {
    let exit_code = run_suite();
    cleanup(exit_code);
    exit_code
}

#[test]
fn test_main_preserves_go_harness_contract() {
    let mut cleaned_up_with = None;
    let exit_code = run_test_main(|| 7, |code| cleaned_up_with = Some(code));
    assert_eq!(exit_code, 7);
    assert_eq!(cleaned_up_with, Some(7));
}
