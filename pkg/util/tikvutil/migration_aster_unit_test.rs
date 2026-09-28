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

// `CommitterConcurrency` 原子变量的迁移补充单元测试。
//
// 验证顺序一致（SeqCst）的 load/store 语义与默认值 128，
// 对齐 Go `go.uber.org/atomic.Int32` 行为。

use super::tikvutil::CommitterConcurrency;

/// 断言默认值为 128，store 后可读到新值，最后恢复默认。
#[test]
fn committer_concurrency_matches_go_atomic_load_store_semantics() {
    assert_eq!(CommitterConcurrency.load(), 128);

    CommitterConcurrency.store(256);
    assert_eq!(CommitterConcurrency.load(), 256);

    CommitterConcurrency.store(128);
}
