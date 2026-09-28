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

// `CtxMatcher` 迁移期单元测试。
//
// 校验仅接受统计前台优先内部上下文、非 Context 类型保持 Go 侧 type assert panic，
// 以及 `String()` 描述与 Go 一致。

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{Context, CtxMatcher, kv};

/// 仅 `InternalTxnStatsForegroundPriority` 内部上下文应匹配；默认、其他 stats、带显式任务类型均不匹配。
#[test]
fn ctx_matcher_accepts_only_stats_foreground_internal_context() {
    let matcher = CtxMatcher {};
    let matching =
        kv::WithInternalSourceType(Context::default(), kv::InternalTxnStatsForegroundPriority);
    let other_stats = kv::WithInternalSourceType(Context::default(), kv::InternalTxnStats);
    let with_explicit_task = kv::WithInternalSourceAndTaskType(
        Context::default(),
        kv::InternalTxnStatsForegroundPriority,
        "analyze",
    );

    assert!(matcher.Matches(&matching));
    assert!(!matcher.Matches(&Context::default()));
    assert!(!matcher.Matches(&other_stats));
    assert!(!matcher.Matches(&with_explicit_task));
}

/// 传入非 `Context` 时 downcast 失败应 panic，与 Go type assertion 行为对齐。
#[test]
fn ctx_matcher_preserves_go_type_assertion_panic() {
    let matcher = CtxMatcher {};
    let result = catch_unwind(AssertUnwindSafe(|| matcher.Matches(&"not a context")));

    assert!(result.is_err());
}

/// `String()` 描述字符串须与 Go 侧完全一致。
#[test]
fn ctx_matcher_string_matches_go_description() {
    assert_eq!(
        CtxMatcher {}.String(),
        "all txns should be internal stats foreground priority source"
    );
}
