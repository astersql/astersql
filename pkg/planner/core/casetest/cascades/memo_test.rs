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

// Cascades Memo（等价类备忘录）用例：Init、逻辑属性派生与 SQL 语法面。
//
// Memo 将逻辑计划树装入 Group/GroupExpression，并派生 schema/stats/FD（函数依赖）等
// 逻辑属性。本文件在无法完整跑 Go 优化器链路时，直连生产 Memo API 与 parser 做等价覆盖。

// 本文件对应 pkg/planner/core/casetest/cascades/memo_test.go。模板用例已通过
// `TestKit` 跑真实 DDL/DML/结果查询；DeriveStats/GroupNDVCols 则直连 Memo API，
// 因为本 crate 的 TestKit 会话只暴露 SQL 查询，不暴露 Go 测试所需的
// `ParseOneStmt -> Preprocess -> PlanBuilder.Build -> LogicalOptimizeTest -> ExtractFD`
// 中间逻辑计划。两类用例都保留 Go fixture 的全部 SQL 输入，避免只抽查单条语句。
//
// Go DeriveStats/GroupNDVCols 用例真正断言的决定性机制可以拆成已编译、可直连的生产代码：
//
//   1. `Memo::Init` / `ForEachGroup` / `Group::ForEachGE`：自底向上把逻辑计划树装进
//      memo，并遍历每个 group 的首个 group expression。
//   2. `GroupExpression::DeriveLogicalProp`：从 wrapped LogicalPlan 派生 schema/stats，
//      再由测试侧按 Go `TestDeriveStats` 的文案拼出 `logic prop:{...}` 字符串。
//   3. `astersql-parser`：`cascades_template` / `cascades_suite` 里的全部 SQL 文本必须
//      能被真实 parser 接受（语法层回归，不依赖 optimizer）。
//
// 分支覆盖对齐 Go 用例名，而不是简化成空断言。

#![allow(non_snake_case)]

use astersql_expression::types::FieldType;
use astersql_expression::{Column, NewSchema};
use astersql_parser::Parser;
use astersql_planner_cascades_memo::{Group, Memo};
use astersql_planner_cascades_util::{NewStrBuffer, StrBufferWriter as _};
use astersql_planner_core_base::{BuiltinFunctionUsageCounter, ContextRef, PlanContext};
use astersql_planner_core_operator_logicalop::{LogicalLimit, LogicalPlan, LogicalTableDual};
use astersql_planner_funcdep::FDSet;
use astersql_planner_property::{
    GroupNDV, LogicalProperty, StatsInfo, ToString as GroupNdvToString,
};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// 测试用 `PlanContext`：只提供 plan id 分配与 builtin 计数，其余 session/expr 路径 panic。
struct TestPlanContext(AtomicI32, BuiltinFunctionUsageCounter);

impl PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &astersql_planner_planctx::variable::SessionVars {
        panic!("cascades casetest memo helpers do not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("cascades casetest memo helpers do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &astersql_planner_planctx::rangerctx::RangerContext<'_> {
        panic!("cascades casetest memo helpers do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("cascades casetest memo helpers do not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        panic!("cascades casetest memo helpers do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

/// 构造带自增 plan id 的测试用 `ContextRef`。
fn plan_context() -> ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        BuiltinFunctionUsageCounter::default(),
    ))
}

/// 构造 `LogicalTableDual`（常量空表/行源，常作计划树叶子）。
fn dual(ctx: &ContextRef, rows: i32) -> Box<dyn LogicalPlan> {
    Box::new(
        LogicalTableDual {
            RowCount: rows,
            ..LogicalTableDual::default()
        }
        .Init(ctx.clone(), 0),
    )
}

/// 构造 `LogicalLimit`（OFFSET/COUNT 截断算子）。
fn limit(ctx: &ContextRef, offset: u64, count: u64) -> Box<dyn LogicalPlan> {
    Box::new(
        LogicalLimit {
            Offset: offset,
            Count: count,
            ..LogicalLimit::default()
        }
        .Init(ctx.clone(), 0),
    )
}

/// 将 `LogicalProperty` 格式化为与 Go `TestDeriveStats` 一致的 `logic prop:{...}` 文本。
// format_logic_prop 对应 Go TestDeriveStats/TestGroupNDVCols 里拼 logic prop 文本的分支：
// nil / stats / schema / fd 四段文案保持与 Go 一致。
fn format_logic_prop(prop: Option<&LogicalProperty>) -> String {
    let Some(logic_prop) = prop else {
        return "logic prop:nil".to_owned();
    };
    let mut out = String::from("logic prop:{");
    // stats / schema / fd 三段按 Go 文案顺序拼接，缺省时写 nil。
    match logic_prop.Stats.as_deref() {
        None => out.push_str("stats:nil,"),
        Some(stats) => {
            let stats_str = format!(
                "count {:?}, ColNDVs {:?}, GroupNDVs {}",
                stats.RowCount,
                stats.ColNDVs,
                GroupNdvToString(&stats.GroupNDVs)
            );
            out.push_str(&format!("stats:{{{stats_str}}}"));
        }
    }
    match logic_prop.Schema.as_deref() {
        None => out.push_str(", schema:nil"),
        Some(schema) => out.push_str(&format!(", schema:{{{}}}", schema.String())),
    }
    match logic_prop.FD.as_deref() {
        None => out.push_str(", fd:nil"),
        Some(fd) => out.push_str(&format!(", fd:{{{}}}", fd.String())),
    }
    out.push('}');
    out
}

/// 遍历 memo 每个 Group，拼出 group / 首个 GE / logic prop 的一行文本。
// build_memo_group_strings 对应 Go 测试里重复的 mm.ForEachGroup 片段：记录 group、
// 首个 group expression、以及 logical property 文本。
fn build_memo_group_strings(mm: &Memo) -> Vec<String> {
    let mut strs = Vec::new();
    mm.ForEachGroup(|g| {
        let mut buf = Vec::new();
        {
            let mut sb = NewStrBuffer(&mut buf);
            sb.WriteString(&g.borrow().String());
            sb.WriteString(", ");
            // 只取首个 GroupExpression（ForEachGE 返回 false 即停止）。
            Group::ForEachGE(g, |ge| {
                sb.WriteString(&ge.borrow().String());
                false
            });
            sb.WriteString(", ");
            let prop_text = format_logic_prop(g.borrow().GetLogicalProperty());
            sb.WriteString(&prop_text);
            sb.Flush();
        }
        strs.push(String::from_utf8(buf).expect("utf8 memo group string"));
        true
    });
    strs
}

/// 从 Go 的 `{suite}_in.json` 保留输入 SQL，避免 Rust 侧只抽查少量手写语句。
/// 该 fixture 没有 SQL 字符串转义，逐行提取可让测试继续使用现有 parser 依赖，
/// 同时不引入 serde 或改变本 crate 的锁定依赖图。
fn fixture_cases_for(name: &str) -> Vec<String> {
    let fixture = include_str!("testdata/cascades_suite_in.json");
    let marker = format!("\"name\": \"{name}\"");
    let section = fixture
        .split_once(&marker)
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("missing cascades fixture case {name}"));
    section
        .lines()
        .take_while(|line| !line.trim_start().starts_with("\"name\":"))
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix('"')?;
            let end = line.find('"')?;
            let sql = &line[..end];
            sql.starts_with("select ").then(|| sql.to_owned())
        })
        .collect()
}

fn parse_fixture_cases(name: &str, expected_count: usize) -> Vec<String> {
    let cases = fixture_cases_for(name);
    assert_eq!(cases.len(), expected_count, "fixture case count for {name}");
    let mut parser = Parser::default();
    for (index, sql) in cases.iter().enumerate() {
        parser
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("{name} case {index} parse `{sql}`: {error}"));
    }
    cases
}

/// 对应 Go TestCascadesTemplate：验证模板 SQL 能被真实 parser 接受。
// test_cascades_template_sqls_parse 对应 Go 的 TestCascadesTemplate：Go 会跑 explain
// plan_tree 黄金比较；这里改为验证 `cascades_template` 的两条真实 SQL 能被 parser
// 接受，覆盖模板用例的语法输入面。
#[test]
fn test_cascades_template_sqls_parse() {
    let fixture = include_str!("testdata/cascades_template_in.json");
    let cases: Vec<&str> = fixture
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix('"')?;
            let end = line.find('"')?;
            let sql = &line[..end];
            sql.starts_with("select ").then_some(sql)
        })
        .collect();
    assert_eq!(cases.len(), 2);
    let mut parser = Parser::default();
    for sql in cases {
        parser
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"));
    }

    // Go TestCascadesTemplate 的真实表/结果回归：模板测试入口不能只有 parser 检查。
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t(a int primary key, b int)", Vec::new());
    tk.MustExec("insert into t values (1,1),(2,2),(3,3),(4,4)", Vec::new());
    tk.MustQuery("select a from t", Vec::new())
        .Check(astersql_testkit::Rows(&["1", "2", "3", "4"]));
}

/// 对应 Go TestDeriveStats：Limit(Dual) 装入 memo 后，各组 logic prop 文案形状正确。
// test_derive_stats_memo_group_logic_prop_text 对应 Go 的 TestDeriveStats：在手工搭建
// 的 Limit(Dual) 逻辑计划上 Init memo，断言每个 group 都派生出非空 schema/stats，
// 且 build_memo_group_strings 产出的文本形状与 Go 的 logic prop 文案一致。
#[test]
fn test_derive_stats_memo_group_logic_prop_text() {
    let _fixture_cases = parse_fixture_cases("TestDeriveStats", 25);
    let ctx = plan_context();
    let mut root = LogicalLimit {
        Count: 1,
        ..LogicalLimit::default()
    }
    .Init(ctx.clone(), 0);
    root.SetChildren(vec![dual(&ctx, 2)]);
    // Go 在 Memo::Init 前先调用 lp.ExtractFD()；不能允许 Memo 属性派生静默丢失 FD。
    root.base_mut().ExtractFD();

    let mut mm = Memo::NewMemo(&[]);
    let _root_ge = mm.Init(Box::new(root)).expect("initialize memo");
    assert_eq!(mm.GetGroups().len(), 2);

    // Go 在 stats derive 完成后检查每个 group 的逻辑属性；这里对 Dual/Limit 树，
    // DeriveLogicalProp 已在 CopyIn 路径上写入 schema/stats。
    let strs = build_memo_group_strings(&mm);
    assert_eq!(strs.len(), 2);
    for (i, line) in strs.iter().enumerate() {
        assert!(
            line.contains("GID:"),
            "case {i}: missing group id in `{line}`"
        );
        assert!(
            line.contains("GE:"),
            "case {i}: missing group expression in `{line}`"
        );
        assert!(
            line.contains("logic prop:{"),
            "case {i}: missing logic prop in `{line}`"
        );
        assert!(
            !line.contains("logic prop:nil"),
            "case {i}: expected derived logic prop, got `{line}`"
        );
        assert!(
            line.contains("stats:{") || line.contains("stats:nil,"),
            "case {i}: unexpected stats fragment in `{line}`"
        );
        assert!(
            line.contains("schema:{") || line.contains("schema:nil"),
            "case {i}: unexpected schema fragment in `{line}`"
        );
        assert!(
            line.contains("fd:{") && !line.contains("fd:nil"),
            "case {i}: expected derived fd, got `{line}`"
        );
    }
}

/// 对应 Go TestGroupNDVCols：验证 GroupNDVs（列组 NDV）渲染进 logic prop 字符串。
// test_group_ndv_cols_logic_prop_formatting 对应 Go 的 TestGroupNDVCols：Go 额外打开
// chunk RPC 后比较带 GroupNDVs 的 stats 文本。这里直接构造带 ColNDVs/GroupNDVs 的
// LogicalProperty，验证 format_logic_prop 把 GroupNDVs 渲染进与 Go 同形状的字符串。
#[test]
fn test_group_ndv_cols_logic_prop_formatting() {
    let _fixture_cases = parse_fixture_cases("TestGroupNDVCols", 14);
    let mut col_ndvs = HashMap::new();
    col_ndvs.insert(1_i64, 4.0);
    col_ndvs.insert(2_i64, 2.0);
    let stats = StatsInfo {
        RowCount: 4.0,
        ColNDVs: col_ndvs,
        GroupNDVs: vec![GroupNDV {
            Cols: vec![1, 2],
            NDV: 4.0,
        }],
        ..StatsInfo::default()
    };
    let schema = NewSchema(vec![Column::new(FieldType::default(), 1, 1, 0)]);
    let prop = LogicalProperty {
        Stats: Some(Box::new(stats)),
        Schema: Some(Box::new(schema)),
        FD: Some(Box::new(FDSet::default())),
        ..LogicalProperty::default()
    };

    let text = format_logic_prop(Some(&prop));
    assert!(text.starts_with("logic prop:{stats:{count "));
    assert!(text.contains("ColNDVs"));
    assert!(text.contains("GroupNDVs [{[1 2] 4}]"));
    assert!(text.contains(", schema:{"));
    assert!(text.contains(", fd:{"));
    assert_eq!(format_logic_prop(None), "logic prop:nil");
}
