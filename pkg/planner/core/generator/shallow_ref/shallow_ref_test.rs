// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 本文件对应 pkg/planner/core/generator/shallow_ref/shallow_ref_test.go。
// Go TestHash64Equals（名虽含 hash64）重新生成 shallow_ref 并与 generated 文件对比。
// Rust 无 Go reflect；这里用真实生成器 API + 已提交 generated 文件完整性断言对齐同一意图。

// 逻辑算子浅引用（ShallowRef）生成器的单元测试。
//
// 对应 Go `TestHash64Equals`（历史命名）：验证生成器确定性，并对照已提交的
// `shallow_ref_generated.go` 确保目标逻辑算子均具备 `XxxShallowRef` 方法。

#![allow(non_snake_case)]

use std::io::{BufRead, Cursor};

use super::{
    Field, FieldKind, GenShallowRef4LogicalOps, LogicalOperator, SHALLOW_REF_STRUCTURES,
    gen_shallow_ref_for_logical_ops, refine_field_type_name,
};

/// 按行读取，剥离换行符并限制单行最大长度。
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

/// 逐行比较两段生成代码；内容或 EOF 不一致则 panic。
fn compare_lines(left: &[u8], right: &[u8]) {
    let mut left_reader = Cursor::new(left);
    let mut right_reader = Cursor::new(right);
    loop {
        let line1 = read_line(&mut left_reader, 1024);
        let line2 = read_line(&mut right_reader, 1024);
        match (&line1, &line2) {
            (Ok(a), Ok(b)) if a == b => continue,
            (Err(a), Err(b)) if a == "EOF" && b == "EOF" => break,
            (Err(a), Err(b)) if a == b => continue,
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

/// TestHash64Equals：验证浅引用生成器确定性与 generated 文件完整性。
// TestHash64Equals 对应 Go 文件中的同名测试（实际验证 shallow_ref 生成器）。
#[test]
fn TestHash64Equals() {
    // 二次生成并比对，确认确定性。
    let updated_code = GenShallowRef4LogicalOps().expect("Generate XXXShallowRef code error");
    let again = GenShallowRef4LogicalOps().expect("second generation must succeed");
    compare_lines(&updated_code, &again);
    assert_eq!(
        updated_code, again,
        "shallow_ref generator must be deterministic"
    );

    let current_code = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../operator/logicalop/shallow_ref_generated.go"
    ))
    .expect("Read current shallow_ref_generated.go code error");
    compare_lines(&updated_code, &current_code);
    assert_eq!(
        updated_code, current_code,
        "shallow_ref_generated.go should be updated, please run 'make gogenerate' to update it."
    );

    let current = String::from_utf8_lossy(&current_code);
    for name in SHALLOW_REF_STRUCTURES {
        assert!(
            current.contains(&format!("func (op *{name}) {name}ShallowRef()")),
            "shallow_ref_generated.go should be updated, missing {name}ShallowRef; please run 'make gogenerate'"
        );
    }

    // 字段级：Slice 字段应生成 append 拷贝；refine_field_type_name 对齐 Go。
    assert_eq!(
        refine_field_type_name("logicalop.LogicalJoin"),
        "LogicalJoin"
    );
    let with_fields = gen_shallow_ref_for_logical_ops(&[LogicalOperator {
        name: "LogicalJoin".to_owned(),
        fields: vec![Field {
            name: "EqualConditions".to_owned(),
            type_name: "[]*expression.ScalarFunction".to_owned(),
            kind: FieldKind::Slice,
            exported: true,
        }],
    }])
    .expect("field-aware generation");
    let text = String::from_utf8_lossy(&with_fields);
    assert!(text.contains("EqualConditions = append("));
}
