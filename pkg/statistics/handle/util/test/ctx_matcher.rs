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

// 统计相关请求源（request source）的上下文匹配器。
//
// 用于测试断言：事务上下文是否标记为内部统计前台优先（stats foreground priority）。

use std::any::Any;

use crate::{context, kv};

/// 内部请求源前缀。
const INTERNAL_REQUEST: &str = "internal";
/// 外部请求源前缀。
const EXTERNAL_REQUEST: &str = "external";
/// 无法判定请求源时的占位串。
const SOURCE_UNKNOWN: &str = "unknown";

// CtxMatcher is a matcher for context.Context
/// 断言 `context.Context` 是否为内部统计前台优先请求源的匹配器。
pub struct CtxMatcher {}

impl CtxMatcher {
    // Matches returns true if the context is internal stats foreground priority source.
    /// 当上下文请求源等于 `internal_<InternalTxnStatsForegroundPriority>` 时返回 true。
    pub fn Matches(&self, x: &dyn Any) -> bool {
        let ctx = x
            .downcast_ref::<context::Context>()
            .expect("CtxMatcher expects context.Context");
        request_source_from_ctx(ctx)
            == format!(
                "{INTERNAL_REQUEST}_{}",
                kv::InternalTxnStatsForegroundPriority
            )
    }

    // String returns the description of CtxMatcher.
    /// 返回匹配器的英文描述，与 Go 侧 gomock 描述字符串一致。
    pub fn String(&self) -> &'static str {
        "all txns should be internal stats foreground priority source"
    }
}

/// 从上下文拼出请求源字符串：`{internal|external}_{type}[_explicit]`。
fn request_source_from_ctx(ctx: &context::Context) -> String {
    let Some(source) = ctx.RequestSource() else {
        return SOURCE_UNKNOWN.to_owned();
    };
    if source.RequestSourceType.is_empty() && source.ExplicitRequestSourceType.is_empty() {
        return SOURCE_UNKNOWN.to_owned();
    }

    let origin = if source.RequestSourceInternal {
        INTERNAL_REQUEST
    } else {
        EXTERNAL_REQUEST
    };
    let source_type = if source.RequestSourceType.is_empty() {
        SOURCE_UNKNOWN
    } else {
        &source.RequestSourceType
    };
    let mut request_source = format!("{origin}_{source_type}");
    // 显式任务类型与主类型不同时追加后缀，便于区分 analyze 等子路径
    if !source.ExplicitRequestSourceType.is_empty()
        && source.ExplicitRequestSourceType != source.RequestSourceType
    {
        request_source.push('_');
        request_source.push_str(&source.ExplicitRequestSourceType);
    }
    request_source
}
