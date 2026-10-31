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

// `common_plans` 中 `NewLineFieldsInfo` 的单元测试。
//
// 覆盖 LOAD DATA 语句的 FIELDS / LINES 子句解析结果，校验字段分隔符、
// 包围符、转义符与行起止符等默认值及显式覆盖行为是否与期望一致。

use super::{Explain, ExplainInfoForEncode, JSONToString, LineFieldsInfo, NewLineFieldsInfo};
use crate::{FlattenPhysicalPlan, NewExplainRUResult, PlanKind, PlanNode, StoreType};
use parser_ast_dependency::LoadDataStmt;

/// 单条用例：SQL 文本与期望的 `LineFieldsInfo`。
struct LineFieldsCase {
    name: &'static str,
    sql: &'static str,
    expected: LineFieldsInfo,
}

/// 构造期望的行/字段分隔信息，减少用例表中的样板代码。
fn expected(
    fields_terminated_by: &str,
    fields_enclosed_by: &str,
    fields_escaped_by: &str,
    fields_opt_enclosed: bool,
    lines_starting_by: &str,
    lines_terminated_by: &str,
) -> LineFieldsInfo {
    LineFieldsInfo {
        FieldsTerminatedBy: fields_terminated_by.to_owned(),
        FieldsEnclosedBy: fields_enclosed_by.to_owned(),
        FieldsEscapedBy: fields_escaped_by.to_owned(),
        FieldsOptEnclosed: fields_opt_enclosed,
        LinesStartingBy: lines_starting_by.to_owned(),
        LinesTerminatedBy: lines_terminated_by.to_owned(),
    }
}

/// 解析若干 LOAD DATA SQL，确认 `NewLineFieldsInfo` 正确吸收 FIELDS/LINES 子句。
#[test]
fn test_new_line_fields_info() {
    // 覆盖默认值及各子句单独覆盖的场景。
    let cases = vec![
        LineFieldsCase {
            name: "defaults",
            sql: "load data infile 'a' into table t",
            expected: expected("\t", "", "\\", false, "", "\n"),
        },
        LineFieldsCase {
            name: "fields terminated by",
            sql: "load data infile 'a' into table t fields terminated by 'a'",
            expected: expected("a", "", "\\", false, "", "\n"),
        },
        LineFieldsCase {
            name: "fields optionally enclosed by",
            sql: "load data infile 'a' into table t fields optionally enclosed by 'a'",
            expected: expected("\t", "a", "\\", true, "", "\n"),
        },
        LineFieldsCase {
            name: "fields enclosed by",
            sql: "load data infile 'a' into table t fields enclosed by 'a'",
            expected: expected("\t", "a", "\\", false, "", "\n"),
        },
        LineFieldsCase {
            name: "fields escaped by",
            sql: "load data infile 'a' into table t fields escaped by 'a'",
            expected: expected("\t", "", "a", false, "", "\n"),
        },
        LineFieldsCase {
            name: "lines starting by",
            sql: "load data infile 'a' into table t lines starting by 'a'",
            expected: expected("\t", "", "\\", false, "a", "\n"),
        },
        LineFieldsCase {
            name: "lines terminated by",
            sql: "load data infile 'a' into table t lines terminated by 'aa'",
            expected: expected("\t", "", "\\", false, "", "aa"),
        },
    ];

    let mut parser = parser_dependency::New();
    for case in cases {
        // 先解析为 AST，再下转型为 LoadDataStmt 以取出 Fields/Lines 子句。
        let statement = parser
            .ParseOneStmt(case.sql, "", "")
            .unwrap_or_else(|error| panic!("parse {}: {error}", case.sql));
        let load_data = statement
            .into_any()
            .downcast::<LoadDataStmt>()
            .expect("LOAD DATA must produce LoadDataStmt");
        let actual = NewLineFieldsInfo(load_data.FieldsInfo.as_ref(), load_data.LinesInfo.as_ref());
        assert_eq!(case.expected, actual, "{}", case.name);
    }
}

/// Go 使用 `json.Encoder` 的缩进、omitempty 与字段名契约必须原样保留。
#[test]
fn test_json_to_string_matches_go_encoder_contract() {
    let rows = vec![ExplainInfoForEncode {
        ID: "Projection_1".to_owned(),
        EstRows: "1.00".to_owned(),
        TaskType: "root".to_owned(),
        OperatorInfo: "select \"a\\b\"\nnext".to_owned(),
        EstCost: "2.50".to_owned(),
        CostFormula: "cpu(1)".to_owned(),
        TotalMemoryConsumed: "1 KB".to_owned(),
        SubOperators: vec![ExplainInfoForEncode {
            ID: "TableFullScan_2".to_owned(),
            TaskType: "cop[tikv]".to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    }];

    assert_eq!(
        JSONToString(&rows),
        concat!(
            "[\n",
            "    {\n",
            "        \"id\": \"Projection_1\",\n",
            "        \"estRows\": \"1.00\",\n",
            "        \"taskType\": \"root\",\n",
            "        \"operatorInfo\": \"select \\\"a\\\\b\\\"\\nnext\",\n",
            "        \"estCost\": \"2.50\",\n",
            "        \"costFormula\": \"cpu(1)\",\n",
            "        \"totalMemoryConsumed\": \"1 KB\",\n",
            "        \"subOperators\": [\n",
            "            {\n",
            "                \"id\": \"TableFullScan_2\",\n",
            "                \"estRows\": \"\",\n",
            "                \"taskType\": \"cop[tikv]\"\n",
            "            }\n",
            "        ]\n",
            "    }\n",
            "]\n",
        )
    );
}

#[test]
fn explain_render_result_routes_ru_format_to_ru_columns() {
    let mut scan = PlanNode::New(2, PlanKind::TableScan { table: "t".into() }, Vec::new());
    scan.store_type = StoreType::TiKV;
    scan.actual_rows = Some(0);
    let mut root = PlanNode::New(1, PlanKind::TableReader, vec![scan]);
    root.actual_rows = Some(0);
    let flat = FlattenPhysicalPlan(Some(&root), false).unwrap();
    let mut result = NewExplainRUResult(Some(&flat));
    result.Main[0].self_ru = 7.0;
    result.Main[0].cum_ru = 7.0;
    result.TotalRU = 7.0;
    let mut explain = Explain {
        TargetPlan: Some(root),
        Format: "ru".into(),
        Analyze: true,
        ..Default::default()
    };
    explain.SetRUResult(Some(result));

    explain.RenderResult().unwrap();
    assert_eq!(explain.Rows[0][3..6], ["7.00", "7.00", "100.00%"]);
    assert_eq!(explain.Rows[1][3..6], ["0.00", "0.00", "0.00%"]);
}

#[test]
fn explain_ru_result_owns_occurrences_and_clears_stale_values() {
    let original = PlanNode::New(
        1,
        PlanKind::Projection,
        vec![PlanNode::New(2, PlanKind::Dual, vec![])],
    );
    let mut flat = FlattenPhysicalPlan(Some(&original), false).unwrap();
    flat.CTE = FlattenPhysicalPlan(Some(&PlanNode::New(7, PlanKind::Dual, vec![])), false)
        .unwrap()
        .Main;
    flat.ScalarSubQ = FlattenPhysicalPlan(Some(&PlanNode::New(7, PlanKind::Dual, vec![])), false)
        .unwrap()
        .Main;
    let mut result = NewExplainRUResult(Some(&flat));
    result.Main[0].self_ru = 3.0;
    result.Main[0].cum_ru = 3.0;
    result.CTE[0].self_ru = 2.0;
    result.CTE[0].cum_ru = 2.0;
    result.ScalarSubQ[0].self_ru = 1.0;
    result.ScalarSubQ[0].cum_ru = 1.0;
    result.TotalRU = 6.0;

    let mut explain = Explain {
        TargetPlan: Some(PlanNode::New(9, PlanKind::Dual, vec![])),
        Format: "ru".into(),
        Analyze: true,
        ..Default::default()
    };
    explain.SetRUResult(Some(result));
    explain.RenderResult().unwrap();
    assert_eq!(explain.Rows.len(), 4);
    assert!(explain.Rows[0][0].contains("Projection_1"));
    assert_eq!(explain.Rows[0][3..6], ["3.00", "3.00", "50.00%"]);
    assert_eq!(explain.Rows[2][0], "Dual_7");
    assert_eq!(explain.Rows[2][3..6], ["2.00", "2.00", "33.33%"]);
    assert_eq!(explain.Rows[3][0], "Dual_7");
    assert_eq!(explain.Rows[3][3..6], ["1.00", "1.00", "16.67%"]);

    let mut invalid = NewExplainRUResult(Some(&flat));
    invalid.Main[0].operator = None;
    explain.SetRUResult(Some(invalid));
    explain.RenderResult().unwrap();
    assert_eq!(explain.Rows.len(), 1);
    assert!(explain.Rows[0][0].contains("Dual_9"));
    assert_eq!(explain.Rows[0][3..6], ["", "", ""]);

    explain.SetRUResult(None);
    explain.RenderResult().unwrap();
    assert_eq!(explain.Rows.len(), 1);
    assert!(explain.Rows[0][0].contains("Dual_9"));
    assert_eq!(explain.Rows[0][3..6], ["", "", ""]);
}

#[test]
fn port_63ec01b244_mlog_proof_follows_only_grouped_left_input() {
    use base_dependency::{JoinType, PhysicalPlan};
    use physicalop_dependency as op;
    let ctx = crate::cbo_test::cbo_plan_context();
    let dual =
        |rows| -> Box<dyn PhysicalPlan> { Box::new(op::PhysicalTableDual::New(ctx.clone(), rows)) };
    let projection = |child| -> Box<dyn PhysicalPlan> {
        let mut p = op::PhysicalProjection::New(ctx.clone());
        p.set_children(vec![child]);
        Box::new(p)
    };
    let agg = |child, grouped, stream| -> Box<dyn PhysicalPlan> {
        let mut base = op::BasePhysicalAgg::New(op::PhysicalSchemaProducer::New(
            op::BasePhysicalPlan::New(ctx.clone(), "Aggregation", 0),
        ));
        if grouped {
            base.GroupByItems
                .push(Box::new(expression_dependency::Column::default()));
        }
        let mut p: Box<dyn PhysicalPlan> = if stream {
            Box::new(op::PhysicalStreamAgg {
                BasePhysicalAgg: base,
            })
        } else {
            Box::new(op::PhysicalHashAgg {
                BasePhysicalAgg: base,
                TiflashPreAggMode: String::new(),
            })
        };
        p.set_children(vec![child]);
        p
    };
    let join = |left, right, kind| -> Box<dyn PhysicalPlan> {
        let base = op::BasePhysicalJoin::New(
            op::PhysicalSchemaProducer::New(op::BasePhysicalPlan::New(ctx.clone(), "HashJoin", 0)),
            kind,
        );
        let mut p = op::NewPhysicalHashJoin(base, 1, false);
        p.set_children(vec![left, right]);
        projection(Box::new(p))
    };
    let cases = vec![
        ("zero-row Dual", dual(0), true),
        ("one-row Dual", dual(1), false),
        (
            "empty grouped mlog",
            join(
                agg(projection(dual(0)), true, false),
                dual(1),
                JoinType::LeftOuterJoin,
            ),
            true,
        ),
        (
            "empty stream aggregation",
            join(agg(dual(0), true, true), dual(1), JoinType::LeftOuterJoin),
            true,
        ),
        (
            "empty MV side",
            join(agg(dual(1), true, false), dual(0), JoinType::LeftOuterJoin),
            false,
        ),
        (
            "scalar hash aggregation",
            join(agg(dual(0), false, false), dual(1), JoinType::LeftOuterJoin),
            false,
        ),
        (
            "scalar stream aggregation",
            join(agg(dual(0), false, true), dual(1), JoinType::LeftOuterJoin),
            false,
        ),
        (
            "inner join",
            join(dual(0), dual(1), JoinType::InnerJoin),
            false,
        ),
        (
            "right join",
            join(dual(0), dual(1), JoinType::RightOuterJoin),
            false,
        ),
        (
            "unknown shape zero estimate",
            Box::new(op::PhysicalSelection::New(ctx.clone())) as Box<dyn PhysicalPlan>,
            false,
        ),
    ];
    for (name, plan, empty) in cases {
        assert_eq!(
            crate::mview_refresh_lookup::MViewRefreshMLogIsEmpty(Some(plan.as_ref())),
            empty,
            "{name}"
        );
    }
    assert!(!crate::mview_refresh_lookup::MViewRefreshMLogIsEmpty(None));
}

#[test]
fn port_63ec01b244_zero_estimate_scan_is_not_empty_proof() {
    use base_dependency::PhysicalPlan;
    let ctx = crate::cbo_test::cbo_plan_context();
    let mut scan = physicalop_dependency::PhysicalTableScan::New(ctx);
    scan.set_stats(property_dependency::StatsInfo {
        RowCount: 0.0,
        ..Default::default()
    });
    assert!(!crate::mview_refresh_lookup::MViewRefreshMLogIsEmpty(Some(
        &scan
    )));
}
