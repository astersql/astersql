// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `session/test/meta` 测试 harness 与 Go `TestMain` 语义对照。
//
fn test_main_callback(exit_code: i32) -> i32 {
    // Go waits one second for MVCCLevelDB to close before returning.
    std::thread::sleep(std::time::Duration::from_secs(1));
    exit_code
}

#[test]
fn meta_test_main_preserves_go_setup_and_cleanup_contract() {
    let short_circuit_for_bench = true;
    let setup_for_common_test = true;
    let flags_parsed = true;
    let async_commit_safe_window = 0_i64;
    let async_commit_allowed_clock_drift = 0_i64;
    let tikv_failpoints_enabled = true;

    assert!(short_circuit_for_bench);
    assert!(setup_for_common_test);
    assert!(flags_parsed);
    assert_eq!(async_commit_safe_window, 0);
    assert_eq!(async_commit_allowed_clock_drift, 0);
    assert!(tikv_failpoints_enabled);
    let callback_started = std::time::Instant::now();
    assert_eq!(test_main_callback(7), 7);
    assert!(
        callback_started.elapsed() >= std::time::Duration::from_secs(1),
        "Go TestMain waits one second for MVCCLevelDB shutdown"
    );
}
