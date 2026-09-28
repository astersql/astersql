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

// 过期读（stale read）failpoint 断言辅助。
//
// 过期读指按历史时间戳（TSO，即 Timestamp Oracle 分配的全局时间戳）读取快照数据，
// 而非读最新提交版本。本文件仅在测试中校验语句层与 Provider 层的过期读标记是否一致。

use crate::{SessionRef, is_stmt_staleness};

/// Test-only invariant helper matching the Go failpoint assertion.
/// 测试专用：断言当前语句是否为过期读，并在期望为真时校验 Provider 类型。
pub fn assert_stmt_staleness(session: &SessionRef, expected: bool) {
    let actual = is_stmt_staleness(session);
    assert_eq!(
        actual, expected,
        "stmtctx isStaleness wrong, expected:{expected}, got:{actual}"
    );
    // 过期读必须挂载 StalenessTxnContextProvider；否则语句标记与事务上下文不一致。
    if expected {
        let provider_is_staleness = session
            .lock()
            .map(|session| session.provider_is_staleness)
            .unwrap_or(false);
        assert!(
            provider_is_staleness,
            "stale read should use StalenessTxnContextProvider"
        );
    }
}
