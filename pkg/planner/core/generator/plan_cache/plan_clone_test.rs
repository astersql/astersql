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

// 本文件对应 pkg/planner/core/generator/plan_cache/plan_clone_test.go。
// Go 的 TestPlanClone 用 reflect 重新生成并与 plan_clone_generated.go 逐字节对比。
// Rust 无 Go reflect；这里用真实生成器 API + 已提交 generated 文件完整性断言对齐同一意图。

// Plan Cache 物理计划克隆生成器的单元测试。
//
// 计划缓存（Plan Cache）复用参数化 SQL 的物理执行计划；`CloneForPlanCache` 在缓存命中时
// 复制算子树。本文件验证生成器输出确定性，并对照已提交的 `plan_clone_generated.go`
// 确保全部物理算子均具备克隆方法。

#![allow(non_snake_case)]

use std::io::{BufRead, Cursor};

use super::{
    CloneTag, GenPlanCloneForPlanCacheCode, PHYSICAL_STRUCTURES, StructField, Structure,
    gen_plan_clone_for_plan_cache, generate_plan_clone_for_plan_cache_code,
};

/// 按行读取，剥离 `\n`/`\r\n`，并限制单行最大长度。
fn read_line(reader: &mut Cursor<&[u8]>, max_line_size: usize) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    let count = reader
        .read_until(b'\n', &mut line)
        .map_err(|error| error.to_string())?;
    if count == 0 {
        return Err("EOF".to_owned());
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    }
    if line.len() > max_line_size {
        return Err(format!("single line length exceeds limit: {max_line_size}"));
    }
    Ok(line)
}

/// 逐行比较两段生成代码；任一行内容或 EOF 状态不一致则 panic。
fn compare_lines(left: &[u8], right: &[u8]) {
    let mut left_reader = Cursor::new(left);
    let mut right_reader = Cursor::new(right);
    loop {
        let line1 = read_line(&mut left_reader, 1024);
        let line2 = read_line(&mut right_reader, 1024);
        match (&line1, &line2) {
            (Ok(a), Ok(b)) if a == b => continue,
            (Err(a), Err(b)) if a == b => break,
            _ => {
                panic!(
                    "line unmatched, line1: {:?}, line2: {:?}",
                    line1.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                    line2.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                );
            }
        }
    }
}

/// TestPlanClone：验证生成器确定性与 generated 文件完整性。
// TestPlanClone 对应 Go 同名测试：生成器输出确定，且已提交 generated 文件覆盖全部物理算子。
#[test]
fn TestPlanClone() {
    // 二次生成并逐行比对，确认生成器无副作用且输出稳定。
    let updated_code =
        GenPlanCloneForPlanCacheCode().expect("Generate CloneForPlanCache code error");
    let again = GenPlanCloneForPlanCacheCode().expect("second generation must succeed");
    compare_lines(&updated_code, &again);
    assert_eq!(
        updated_code, again,
        "plan_clone generator must be deterministic"
    );

    let generated = String::from_utf8_lossy(&updated_code);
    for name in PHYSICAL_STRUCTURES {
        assert!(
            generated.contains(&format!(
                "func (op *{name}) CloneForPlanCache(newCtx base.PlanContext) (base.Plan, bool)"
            )),
            "generated CloneForPlanCache missing for {name}"
        );
    }

    // 对照仓库中 Go 生成文件：每个物理算子都必须仍有 CloneForPlanCache（对齐 Go 漂移检测）。
    let current_code = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../operator/physicalop/plan_clone_generated.go"
    ))
    .expect("Read current plan_clone_generated.go code error");
    let current = String::from_utf8_lossy(&current_code);
    if updated_code != current_code {
        let first_difference = updated_code
            .iter()
            .zip(&current_code)
            .position(|(generated, committed)| generated != committed)
            .unwrap_or(updated_code.len().min(current_code.len()));
        panic!(
            "plan_clone_generated.go differs at byte {first_difference}: generated={:?}, committed={:?}",
            &updated_code[first_difference..updated_code.len().min(first_difference + 80)],
            &current_code[first_difference..current_code.len().min(first_difference + 80)]
        );
    }
    for name in PHYSICAL_STRUCTURES {
        assert!(
            current.contains(&format!(
                "func (op *{name}) CloneForPlanCache(newCtx base.PlanContext) (base.Plan, bool)"
            )),
            "plan_clone_generated.go should be updated, missing CloneForPlanCache for {name}; please run 'make gogenerate'"
        );
    }

    // 字段级：must-nil / shallow / 特殊字段与表达式切片路径必须生成正确语句。
    let with_fields = generate_plan_clone_for_plan_cache_code(&[Structure::physical(
        "Update",
        vec![
            StructField::new("SimpleSchemaProducer", "physicalop.SimpleSchemaProducer"),
            StructField::new("OrderedList", "[]*expression.Assignment"),
            StructField::new("SelectPlan", "base.PhysicalPlan"),
            StructField::new("FKChecks", "[]any").with_tag(CloneTag::MustNil),
            StructField::new("FKCascades", "[]any").with_tag(CloneTag::MustNil),
            StructField::new("IgnoreErr", "bool").with_tag(CloneTag::Shallow),
        ],
    )])
    .expect("field-aware generation");
    let text = String::from_utf8_lossy(&with_fields);
    assert!(text.contains(
        "cloned.SimpleSchemaProducer = *op.SimpleSchemaProducer.CloneSelfForPlanCache(newCtx)"
    ));
    assert!(text.contains("cloned.OrderedList = util.CloneAssignments(op.OrderedList)"));
    assert!(text.contains("if op.SelectPlan != nil {"));
    assert!(text.contains("\tif op.FKChecks != nil {\n\t\treturn nil, false\n\t}"));
    assert!(text.contains("\tif op.FKCascades != nil {\n\t\treturn nil, false\n\t}"));
    assert!(!text.contains("cloned.IgnoreErr"));

    let selection = gen_plan_clone_for_plan_cache(&Structure::physical(
        "PhysicalSelection",
        vec![
            StructField::new("BasePhysicalPlan", "physicalop.BasePhysicalPlan"),
            StructField::new("Conditions", "[]expression.Expression"),
        ],
    ))
    .expect("selection field generation");
    let selection_text = String::from_utf8_lossy(&selection);
    assert!(selection_text.contains(
        "basePlan, baseOK := op.BasePhysicalPlan.CloneForPlanCacheWithSelf(newCtx, cloned)"
    ));
    assert!(selection_text.contains(
        "cloned.Conditions = utilfuncp.CloneExpressionsForPlanCache(op.Conditions, nil)"
    ));
}
