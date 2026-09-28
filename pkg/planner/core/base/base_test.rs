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

// 规划器 base 抽象层契约测试。
//
// 验证内置函数用量计数、JoinType 判别值与展示字符串、以及
// PossiblePropertiesInfo 对 nil/空切片的哈希与相等语义对齐 Go。

use cascades_base::NewHashEqualer;

use crate::{
    BuildPBContext, BuiltinFunctionUsageCounter, JoinType, PlanContext, PossiblePropertiesInfo,
};

/// 仅实现 `BuiltinFunctionUsageInc` 的最小 PlanContext，其余方法刻意 panic。
#[derive(Default)]
struct UsagePlanContext {
    usage: BuiltinFunctionUsageCounter,
}

impl PlanContext for UsagePlanContext {
    fn alloc_plan_id(&self) -> i32 {
        panic!("usage contract test does not allocate plans")
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("usage contract test does not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("usage contract test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &crate::RangerContext<'_> {
        panic!("usage contract test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("usage contract test does not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &BuildPBContext {
        panic!("usage contract test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.usage.Inc(scalar_func_sig_name)
    }
}

/// 同一签名累加、不同签名独立计数，未出现的签名读回 0。
#[test]
fn builtin_function_usage_counts_each_signature_independently() {
    let context = UsagePlanContext::default();
    let erased: &dyn PlanContext = &context;
    erased.BuiltinFunctionUsageInc("CastIntAsString");
    erased.BuiltinFunctionUsageInc("CastIntAsString");
    erased.BuiltinFunctionUsageInc("PlusInt");

    assert_eq!(context.usage.Get("CastIntAsString"), 2);
    assert_eq!(context.usage.Get("PlusInt"), 1);
    assert_eq!(context.usage.Get("MinusInt"), 0);
}

/// JoinType 的 i32 判别值与 Display 英文名必须与 Go / conflict_detector 对齐。
#[test]
fn join_type_values_and_display_match_go() {
    let cases = [
        (JoinType::InnerJoin, 0, "inner join"),
        (JoinType::LeftOuterJoin, 1, "left outer join"),
        (JoinType::RightOuterJoin, 2, "right outer join"),
        (JoinType::SemiJoin, 3, "semi join"),
        (JoinType::AntiSemiJoin, 4, "anti semi join"),
        (JoinType::LeftOuterSemiJoin, 5, "left outer semi join"),
        (
            JoinType::AntiLeftOuterSemiJoin,
            6,
            "anti left outer semi join",
        ),
    ];
    for (join_type, discriminant, display) in cases {
        assert_eq!(join_type as i32, discriminant);
        assert_eq!(join_type.to_string(), display);
    }

    assert!(JoinType::InnerJoin.is_inner_join());
    assert!(!JoinType::LeftOuterJoin.is_inner_join());

    for join_type in [
        JoinType::LeftOuterJoin,
        JoinType::RightOuterJoin,
        JoinType::LeftOuterSemiJoin,
        JoinType::AntiLeftOuterSemiJoin,
    ] {
        assert!(join_type.is_outer_join());
    }
    for join_type in [
        JoinType::InnerJoin,
        JoinType::SemiJoin,
        JoinType::AntiSemiJoin,
    ] {
        assert!(!join_type.is_outer_join());
    }

    for join_type in [
        JoinType::SemiJoin,
        JoinType::AntiSemiJoin,
        JoinType::LeftOuterSemiJoin,
        JoinType::AntiLeftOuterSemiJoin,
    ] {
        assert!(join_type.is_semi_join());
    }
    for join_type in [
        JoinType::InnerJoin,
        JoinType::LeftOuterJoin,
        JoinType::RightOuterJoin,
    ] {
        assert!(!join_type.is_semi_join());
    }
}

/// None（Go nil）与 Some(空 Vec) 在 equals/hash64 上必须区分，对齐 Go slice 语义。
#[test]
fn possible_properties_preserve_nil_and_empty_distinction() {
    let nil = PossiblePropertiesInfo {
        orders: None,
        has_tiflash: false,
    };
    let empty = PossiblePropertiesInfo {
        orders: Some(Vec::new()),
        has_tiflash: true,
    };
    assert!(!nil.equals(&empty));

    let mut nil_hash = NewHashEqualer();
    nil.hash64(nil_hash.as_mut());
    let mut empty_hash = NewHashEqualer();
    empty.hash64(empty_hash.as_mut());
    assert_ne!(nil_hash.Sum64(), empty_hash.Sum64());

    let without_tiflash = PossiblePropertiesInfo {
        orders: Some(Vec::new()),
        has_tiflash: false,
    };
    assert!(empty.equals(&without_tiflash));

    let mut without_tiflash_hash = NewHashEqualer();
    without_tiflash.hash64(without_tiflash_hash.as_mut());
    assert_eq!(empty_hash.Sum64(), without_tiflash_hash.Sum64());
}
