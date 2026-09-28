// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// planner_bridge 公开入口的单元测试。
//
// 覆盖 GROUPING 元数据原子安装，以及 FTS（全文检索）转 LIKE/ILIKE
// 回退路径对搜索串 token 子集的严格校验。

use std::collections::HashSet;
use std::sync::Arc;

use crate::{
    BuildFTSToILikeExpression, BuiltinGroupingImplSig, GetTimeValue, NewOne,
    ValidateFTSSearchStringForLikeFallback, builtinFunc,
};
use chrono::TimeZone;
use exprstatic::{NewEvalContext, NewExprContext, WithCurrentTime, WithEvalCtx, WithLocation};

/// GROUPING 签名在 SetMetadata 前后应保持初始化状态一致且可回读。
#[test]
fn grouping_metadata_is_installed_atomically() {
    let signature = BuiltinGroupingImplSig::new(vec![Box::new(NewOne())]);
    assert_eq!(signature.groupingMetaInitialized(), Some(false));

    // ModeNumericSet：用集合包含关系判断分组标记是否命中。
    signature
        .SetMetadata(
            tipb::GroupingMode::ModeNumericSet,
            vec![HashSet::from([1, 3, 5])],
        )
        .unwrap();

    assert_eq!(signature.groupingMetaInitialized(), Some(true));
    assert_eq!(
        signature.groupingModeAndMarks(),
        Some((
            tipb::GroupingMode::ModeNumericSet as i64,
            vec![vec![1, 3, 5]],
        ))
    );
    assert!(signature.metadata().is_some());
}

/// ILIKE 回退只接受不含通配符等扩展语法的 token 子集。
#[test]
fn fts_like_fallback_accepts_only_the_supported_token_subset() {
    // 合法：普通词、布尔 +/- 修饰、中文。
    for (text, modifier) in [("alpha beta", 0), ("+alpha -beta", 1), ("中文", 0)] {
        ValidateFTSSearchStringForLikeFallback(text.to_owned(), modifier).unwrap();
    }
    // 非法：通配符、孤立修饰符、布尔模式下单独带 + 的词。
    for (text, modifier) in [("alpha*", 0), ("+", 1), ("+alpha", 0)] {
        assert!(ValidateFTSSearchStringForLikeFallback(text.to_owned(), modifier).is_err());
    }
}

/// FTS modifier bits must follow Go's mode-mask and query-expansion layout.
#[test]
fn fts_like_fallback_uses_go_modifier_bit_layout() {
    let context = NewExprContext(Vec::new());

    let query_expansion = BuildFTSToILikeExpression(
        &context,
        vec![Box::new(NewOne())],
        "alpha".to_owned(),
        1 << 4,
    )
    .err()
    .expect("query expansion must be rejected");
    assert!(query_expansion.to_string().contains("WITH QUERY EXPANSION"));

    let unsupported_mode =
        BuildFTSToILikeExpression(&context, vec![Box::new(NewOne())], "alpha".to_owned(), 2)
            .err()
            .expect("unknown mode must be rejected");
    assert!(
        unsupported_mode
            .to_string()
            .contains("modifier is not supported")
    );
}

/// An explicit parsing timezone must not move the statement timestamp.
#[test]
fn current_timestamp_uses_session_timezone_like_go() {
    let fixed = chrono_tz::UTC
        .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .unwrap();
    let eval_context = Arc::new(NewEvalContext(vec![
        WithLocation(chrono_tz::UTC),
        WithCurrentTime(Arc::new(move || Ok(fixed))),
    ]));
    let context = NewExprContext(vec![WithEvalCtx(eval_context)]);

    let value = GetTimeValue(
        &context,
        "current_timestamp",
        crate::mysql::TypeTimestamp,
        0,
        Some(chrono_tz::Asia::Tokyo),
    )
    .unwrap();
    assert_eq!(value.GetMysqlTime().String(), "2026-01-02 03:04:05");
}
