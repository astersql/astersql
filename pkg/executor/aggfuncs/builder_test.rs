// Copyright 2026 AsterSQL.
// 聚合函数构建器的关键分派与参数提取回归测试。
//
// 覆盖普通聚合与窗口函数在不同执行阶段、DISTINCT、ORDER BY 以及常量参数下
// 选择的具体实现，并验证不受支持的模式会被拒绝。

use super::builder::*;

/// 构造测试常用的有符号整数类型。
fn int_type() -> FieldType {
    FieldType::new(FieldKind::LongLong, EvalType::Int)
}

/// 构造测试常用的字符串类型。
fn string_type() -> FieldType {
    FieldType::new(FieldKind::String, EvalType::String)
}

/// 创建最小聚合描述符；各用例再按需开启 DISTINCT 或追加 ORDER BY。
fn desc(
    name: FunctionName,
    mode: AggMode,
    args: Vec<ArgDesc>,
    return_type: FieldType,
) -> AggFuncDesc {
    AggFuncDesc {
        name,
        mode,
        has_distinct: false,
        args,
        return_type,
        order_by_items: Vec::new(),
    }
}

#[test]
fn build_count_matches_original_partial_and_distinct_modes() {
    let argument = ArgDesc::typed(int_type());
    // Complete 阶段直接读取原始行，Final 阶段则合并上游的部分结果。
    let mut complete = desc(
        FunctionName::Count,
        AggMode::Complete,
        vec![argument.clone()],
        int_type(),
    );
    assert!(matches!(
        build(AggFuncBuildContext::default(), &complete, 2)
            .unwrap()
            .implementation,
        AggImplementation::CountOriginal(ValueKind::Int)
    ));

    complete.mode = AggMode::Final;
    assert!(matches!(
        build(AggFuncBuildContext::default(), &complete, 2)
            .unwrap()
            .implementation,
        AggImplementation::CountPartial
    ));

    complete.mode = AggMode::Complete;
    complete.has_distinct = true;
    // DISTINCT 仍属于原始行阶段，但需要切换到带去重状态的实现。
    assert!(matches!(
        build(AggFuncBuildContext::default(), &complete, 2)
            .unwrap()
            .implementation,
        AggImplementation::CountOriginalDistinct(ValueKind::Int)
    ));
}

#[test]
fn build_group_concat_preserves_separator_arguments_and_context_limit() {
    let mut group_concat = desc(
        FunctionName::GroupConcat,
        AggMode::Complete,
        vec![
            ArgDesc::typed(string_type()),
            ArgDesc::constant(string_type(), ConstantValue::String("|".to_owned())),
        ],
        string_type(),
    );
    group_concat.order_by_items.push(OrderByItem {
        descending: true,
        collation: "utf8mb4_bin".to_owned(),
    });
    let built = build(
        AggFuncBuildContext {
            group_concat_max_len: 17,
            ..AggFuncBuildContext::default()
        },
        &group_concat,
        4,
    )
    .unwrap();
    // 最末参数是分隔符而非待聚合表达式，因此构建结果只保留一个数据参数。
    assert_eq!(built.argument_count, 1);
    assert_eq!(built.separator.as_deref(), Some("|"));
    assert_eq!(built.max_len, Some(17));
    assert!(matches!(
        built.implementation,
        AggImplementation::GroupConcatOrder
    ));
}

#[test]
fn build_window_lead_lag_converts_offset_and_default_value() {
    // 第二、三个常量参数分别固化为偏移量与缺省值，供运行时直接使用。
    let mut lead = desc(
        FunctionName::Lead,
        AggMode::Complete,
        vec![
            ArgDesc::typed(string_type()),
            ArgDesc::constant(int_type(), ConstantValue::Int(2)),
            ArgDesc::constant(string_type(), ConstantValue::String("fallback".to_owned())),
        ],
        string_type(),
    );
    let built = build_window_function(AggFuncBuildContext::default(), &lead, 1).unwrap();
    assert!(matches!(
        built.implementation,
        AggImplementation::Lead {
            kind: ValueKind::String,
            offset: 2
        }
    ));
    assert_eq!(
        built.default_value,
        Some(ConstantValue::String("fallback".to_owned()))
    );

    lead.name = FunctionName::Lag;
    // LEAD 与 LAG 共用参数解析，仅切换窗口移动方向对应的实现。
    let built = build_window_function(AggFuncBuildContext::default(), &lead, 1).unwrap();
    assert!(matches!(
        built.implementation,
        AggImplementation::Lag {
            kind: ValueKind::String,
            offset: 2
        }
    ));
}

#[test]
fn build_window_lead_lag_preserves_default_when_conversion_fails() {
    let lead = desc(
        FunctionName::Lead,
        AggMode::Complete,
        vec![
            ArgDesc::typed(int_type()),
            ArgDesc::constant(int_type(), ConstantValue::Int(1)),
            ArgDesc::constant(
                string_type(),
                ConstantValue::String("not-an-integer".to_owned()),
            ),
        ],
        int_type(),
    );

    let built = build_window_function(AggFuncBuildContext::default(), &lead, 0).unwrap();
    // Go only replaces a constant default when ConvertTo succeeds. On conversion
    // failure it keeps the original expression instead of silently using NULL.
    assert_eq!(
        built.default_value,
        Some(ConstantValue::String("not-an-integer".to_owned()))
    );
}

#[test]
fn build_rejects_dedup_json_and_invalid_percentile_modes() {
    // JSON 聚合不接受 Dedup 阶段；近似百分位也不接受 Partial2 阶段。
    let json = desc(
        FunctionName::JsonArrayAgg,
        AggMode::Dedup,
        vec![ArgDesc::typed(string_type())],
        string_type(),
    );
    assert!(build(AggFuncBuildContext::default(), &json, 0).is_none());

    let percentile = desc(
        FunctionName::ApproxPercentile,
        AggMode::Partial2,
        vec![
            ArgDesc::typed(int_type()),
            ArgDesc::constant(int_type(), ConstantValue::Int(50)),
        ],
        int_type(),
    );
    assert!(build(AggFuncBuildContext::default(), &percentile, 0).is_none());
}

#[test]
fn count_extrema_factory_preserves_argument_type_and_modes() {
    for name in [FunctionName::MaxCount, FunctionName::MinCount] {
        let mut desc = AggFuncDesc {
            name,
            mode: AggMode::Complete,
            has_distinct: false,
            args: vec![ArgDesc::typed(FieldType::new(
                FieldKind::String,
                EvalType::String,
            ))],
            return_type: FieldType::new(FieldKind::LongLong, EvalType::Int),
            order_by_items: vec![],
        };
        assert!(matches!(
            build(AggFuncBuildContext::default(), &desc, 0)
                .unwrap()
                .implementation,
            AggImplementation::MaxMinCount {
                kind: ValueKind::String,
                reject_rows: false,
                ..
            }
        ));
        assert!(matches!(
            build_window_function(AggFuncBuildContext::default(), &desc, 0)
                .unwrap()
                .implementation,
            AggImplementation::SlidingMaxMinCount {
                kind: ValueKind::String,
                ..
            }
        ));
        for mode in [AggMode::Final, AggMode::Partial2] {
            desc.mode = mode;
            assert!(matches!(
                build(AggFuncBuildContext::default(), &desc, 0)
                    .unwrap()
                    .implementation,
                AggImplementation::MaxMinCount {
                    reject_rows: false,
                    ..
                }
            ));
            desc.args.push(desc.args[0].clone());
            assert!(matches!(
                build(AggFuncBuildContext::default(), &desc, 0)
                    .unwrap()
                    .implementation,
                AggImplementation::MaxMinCount {
                    reject_rows: true,
                    ..
                }
            ));
            assert!(matches!(
                build_window_function(AggFuncBuildContext::default(), &desc, 0)
                    .unwrap()
                    .implementation,
                AggImplementation::MaxMinCount {
                    reject_rows: true,
                    ..
                }
            ));
            desc.args.pop();
        }
        desc.mode = AggMode::Dedup;
        assert!(build(AggFuncBuildContext::default(), &desc, 0).is_none());
    }
}
