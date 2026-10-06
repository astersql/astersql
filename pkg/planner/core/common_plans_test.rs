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
