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

// Copr 包测试入口契约，对应 Go `TestMain`。
//
// `run_after_mvcc_cleanup` 保留 Go `(*main).Run` 的顺序：先运行测试主体，
// 再等待 MVCCLevelDB 关闭，最后返回原码。

use std::time::Duration;

const MVCC_LEVELDB_CLEANUP_WAIT: Duration = Duration::from_secs(1);

fn run_after_mvcc_cleanup(run_tests: impl FnOnce() -> i32) -> i32 {
    let code = run_tests();
    std::thread::sleep(MVCC_LEVELDB_CLEANUP_WAIT);
    code
}

#[test]
fn test_main_returns_test_code_after_mvcc_cleanup_wait() {
    let started = std::time::Instant::now();
    assert_eq!(run_after_mvcc_cleanup(|| 42), 42);
    assert!(started.elapsed() >= MVCC_LEVELDB_CLEANUP_WAIT);
}
