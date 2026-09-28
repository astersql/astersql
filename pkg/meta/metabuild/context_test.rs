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

// metabuild::Context 单元测试：默认值与 Option 覆盖顺序。
//
// `GO_REFERENCE` 保留 Go 表驱动测试的参考实现文本；下方 Rust 用例验证 NewContext
// 与 Session 默认常量对齐，以及多个 Option 按声明顺序覆盖。

const GO_REFERENCE: &str = r################"

use std::any::Any;

// ContextGetter 对应 Go 表格中的 getter 字段：从 Context 取出待验证值。
type ContextGetter = fn(&metabuild::Context) -> Box<dyn Any>;

// ContextOptionBuilder 对应 Go 表格中的 option 字段：把测试值包装成 metabuild.Option。
type ContextOptionBuilder = fn(Box<dyn Any>) -> Box<dyn metabuild::Option>;

// DefaultChecker 保留 Go 中 checkDefault 既可以是普通值，也可以是函数断言的形状。
enum DefaultChecker {
    Value(Box<dyn Any>),
    Function(fn(&metabuild::Context)),
}

// ContextField 是 TestMetaBuildContext 的表驱动项，字段顺序与 Go 匿名结构体一致。
struct ContextField {
    name: &'static str,
    getter: ContextGetter,
    check_default: DefaultChecker,
    option: ContextOptionBuilder,
    test_vals: Vec<Box<dyn Any>>,
}

#[test]
fn test_meta_build_context() {
    // Go 测试先创建一份默认 SessionVars，后续每个默认值都与这份基准比较。
    let def_vars = variable::NewSessionVars(None);

    // fields 逐项覆盖 Context 的所有公开配置访问器，保持 Go 中声明顺序，方便和 Context 结构字段对照。
    let fields = vec![
        ContextField {
            name: "exprCtx",
            getter: |ctx| Box::new(ctx.GetExprCtx()),
            check_default: DefaultChecker::Function(|ctx| {
                // 默认表达式上下文必须非空，并继承 TiDB 默认字符集、排序规则与 SQL mode。
                require::NotNil(ctx.GetExprCtx());
                let (cs, col) = ctx.GetExprCtx().GetCharsetInfo();
                let (def_cs, def_col) = charset::GetDefaultCharsetAndCollate();
                require::Equal(def_cs, cs);
                require::Equal(def_col, col);
                let (def_sql_mode, err) = mysql::GetSQLMode(mysql::DefaultSQLMode);
                require::NoError(err);
                require::Equal(def_sql_mode, ctx.GetSQLMode());
                require::Equal(ctx.GetExprCtx().GetEvalCtx().SQLMode(), ctx.GetSQLMode());
                require::Equal(def_vars.DefaultCollationForUTF8MB4, ctx.GetDefaultCollationForUTF8MB4());
                require::Equal(
                    ctx.GetExprCtx().GetDefaultCollationForUTF8MB4(),
                    ctx.GetDefaultCollationForUTF8MB4(),
                );
            }),
            option: |val| {
                // Go 通过 val.(exprctx.ExprContext) 做接口断言；这里保留同等动态类型要求。
                let expr_ctx = val.downcast::<exprctx::ExprContext>().unwrap();
                metabuild::WithExprCtx(*expr_ctx)
            },
            test_vals: vec![Box::new(exprstatic::NewExprContext())],
        },
        ContextField {
            name: "enableAutoIncrementInGenerated",
            getter: |ctx| Box::new(ctx.EnableAutoIncrementInGenerated()),
            check_default: DefaultChecker::Value(Box::new(def_vars.EnableAutoIncrementInGenerated)),
            option: |val| metabuild::WithEnableAutoIncrementInGenerated(*val.downcast::<bool>().unwrap()),
            test_vals: vec![Box::new(true), Box::new(false)],
        },
        ContextField {
            name: "primaryKeyRequired",
            getter: |ctx| Box::new(ctx.PrimaryKeyRequired()),
            check_default: DefaultChecker::Value(Box::new(def_vars.PrimaryKeyRequired)),
            option: |val| metabuild::WithPrimaryKeyRequired(*val.downcast::<bool>().unwrap()),
            test_vals: vec![Box::new(true), Box::new(false)],
        },
        ContextField {
            name: "clusteredIndexDefMode",
            getter: |ctx| Box::new(ctx.GetClusteredIndexDefMode()),
            check_default: DefaultChecker::Value(Box::new(def_vars.EnableClusteredIndex)),
            option: |val| {
                metabuild::WithClusteredIndexDefMode(*val.downcast::<vardef::ClusteredIndexDefMode>().unwrap())
            },
            test_vals: vec![
                Box::new(vardef::ClusteredIndexDefModeOn),
                Box::new(vardef::ClusteredIndexDefModeOff),
            ],
        },
        ContextField {
            name: "shardRowIDBits",
            getter: |ctx| Box::new(ctx.GetShardRowIDBits()),
            check_default: DefaultChecker::Value(Box::new(def_vars.ShardRowIDBits)),
            option: |val| metabuild::WithShardRowIDBits(*val.downcast::<u64>().unwrap()),
            test_vals: vec![Box::new(6_u64), Box::new(8_u64)],
        },
        ContextField {
            name: "preSplitRegions",
            getter: |ctx| Box::new(ctx.GetPreSplitRegions()),
            check_default: DefaultChecker::Value(Box::new(def_vars.PreSplitRegions)),
            option: |val| metabuild::WithPreSplitRegions(*val.downcast::<u64>().unwrap()),
            test_vals: vec![Box::new(123_u64), Box::new(456_u64)],
        },
        ContextField {
            name: "suppressTooLongIndexErr",
            getter: |ctx| Box::new(ctx.SuppressTooLongIndexErr()),
            check_default: DefaultChecker::Value(Box::new(false)),
            option: |val| metabuild::WithSuppressTooLongIndexErr(*val.downcast::<bool>().unwrap()),
            test_vals: vec![Box::new(true), Box::new(false)],
        },
        ContextField {
            name: "is",
            getter: |ctx| {
                let (schema, ok) = ctx.GetInfoSchema();
                // Go 要求 ok 与返回值是否为 nil 完全一致，避免包装 Option 后语义漂移。
                require::Equal(ok, schema.is_some());
                Box::new(schema)
            },
            check_default: DefaultChecker::Value(Box::new(None::<infoschemactx::MetaOnlyInfoSchema>)),
            option: |val| {
                if val.is::<Option<infoschemactx::MetaOnlyInfoSchema>>() {
                    return metabuild::WithInfoSchema(None);
                }
                metabuild::WithInfoSchema(Some(*val.downcast::<infoschemactx::MetaOnlyInfoSchema>().unwrap()))
            },
            test_vals: vec![Box::new(infoschema::MockInfoSchema(None)), Box::new(None::<infoschemactx::MetaOnlyInfoSchema>)],
        },
    ];

    let def_ctx = metabuild::NewContext(vec![]);
    let mut all_fields = Vec::with_capacity(fields.len());

    for field in &fields {
        // 对应 t.Run("default_of_"+field.name)：每个字段单独验证默认值，便于定位缺失的默认接线。
        test::run(format!("default_of_{}", field.name), || match field.check_default {
            DefaultChecker::Function(check) => check(&def_ctx),
            DefaultChecker::Value(ref expected) => require::Equal(expected, &(field.getter)(&def_ctx), field.name),
        });
        all_fields.push(format!("$.{}", field.name));
    }

    for field in &fields {
        // 对应 t.Run("option_of_"+field.name)：每个候选值都新建 Context，确认 Option 覆盖当前字段。
        test::run(format!("option_of_{}", field.name), || {
            for val in &field.test_vals {
                let ctx = metabuild::NewContext(vec![(field.option)(val.clone())]);
                require::Equal(val, &(field.getter)(&ctx), "{} {:?}", field.name, val);
            }
        });
    }

    // test allFields are tested
    // Go 使用 deeptest 保证 Context 新增字段时测试会失败；这里保留忽略路径列表的递归检查语义。
    deeptest::AssertRecursivelyNotEqual(
        metabuild::Context::default(),
        metabuild::Context::default(),
        deeptest::WithIgnorePath(all_fields),
    );
}
"################;

use std::convert::Infallible;

use crate::{
    NewContext, WithEnableAutoIncrementInGenerated, WithPreSplitRegions, WithPrimaryKeyRequired,
    WithShardRowIDBits, vardef,
};

/// 空 Option 列表时，关键字段应等于 vardef 中的会话默认常量。
#[test]
fn context_defaults_match_session_defaults() {
    let context = NewContext::<(), Infallible>(Vec::new());
    assert_eq!(
        context.EnableAutoIncrementInGenerated(),
        vardef::DefTiDBEnableAutoIncrementInGenerated
    );
    assert!(!context.PrimaryKeyRequired());
    assert_eq!(
        context.GetShardRowIDBits(),
        vardef::DefShardRowIDBits as u64
    );
    assert_eq!(
        context.GetPreSplitRegions(),
        vardef::DefPreSplitRegions as u64
    );
    assert!(!context.GetInfoSchema().1);
}

/// 同一字段多次 Option 时以后者为准；不同字段各自生效。
#[test]
fn context_options_override_in_declaration_order() {
    let context = NewContext::<(), Infallible>(vec![
        WithEnableAutoIncrementInGenerated(false),
        WithEnableAutoIncrementInGenerated(true),
        WithPrimaryKeyRequired(true),
        WithShardRowIDBits(8),
        WithPreSplitRegions(456),
    ]);
    assert!(context.EnableAutoIncrementInGenerated());
    assert!(context.PrimaryKeyRequired());
    assert_eq!(context.GetShardRowIDBits(), 8);
    assert_eq!(context.GetPreSplitRegions(), 456);
}
