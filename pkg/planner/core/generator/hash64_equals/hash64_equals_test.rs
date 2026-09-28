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

// Hash64/Equals 生成器测试。
//
// 对齐 Go `hash64_equals_test.go`：校验生成确定性、算子覆盖，以及与已提交
// `hash64_equals_generated.go` 的完整性对照；Rust 侧用显式 API 替代 reflect。

// 本文件对应 pkg/planner/core/generator/hash64_equals/hash64_equals_test.go。
// Go 的 TestHash64Equals 用 reflect 重新生成并与 hash64_equals_generated.go 逐字节对比。
// Rust 无 Go reflect；这里用真实生成器 API + 已提交 generated 文件完整性断言对齐同一意图。

#![allow(non_snake_case)]

use std::io::{BufRead, Cursor};

use super::{
    GenHash64Equals4LogicalOps, HashField, HashFieldKind, LOGICAL_STRUCTURES, LogicalOperator,
    gen_hash64_equals_for_logical_ops, logical_op_name_to_plan_codec,
};

// Go 测试中的接口形状：指针接收者与值类型均可实现。
trait Test {
    fn Hello(&self) -> String;
}

// 嵌套结构：B1 持有可选指针，B2 持有值，用于 IsNil/接口断言对照。
struct A {}

struct B1 {
    b: Option<Box<A>>,
}

impl Test for B1 {
    fn Hello(&self) -> String {
        "B1".to_string()
    }
}

struct B2 {
    b: A,
}

impl Test for B2 {
    fn Hello(&self) -> String {
        "B2".to_string()
    }
}

// TestGenHash64EqualsField 对应 Go 同名测试：指针字段可判空，值字段不可 IsNil；
// *B1/*B2 均实现 Test。
#[test]
fn TestGenHash64EqualsField() {
    let v_value_1 = B1 { b: None };
    assert!(v_value_1.b.is_none(), "B1.b is pointer-like and may be nil");

    let v_value_1 = &B1 { b: None };
    let v_value_2 = &B2 { b: A {} };
    let _test_1: &dyn Test = v_value_1;
    let _test_2: &dyn Test = v_value_2;
    assert_eq!(_test_1.Hello(), "B1");
    assert_eq!(_test_2.Hello(), "B2");
}

/// 按行读取并限制单行长度，模拟 Go bufio 行比较辅助。
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

/// 逐行比对两段字节；任一侧提前结束或内容不同则 panic。
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

// TestHash64Equals 对应 Go 同名测试：生成器输出确定，且已提交 generated 文件覆盖全部算子。
#[test]
fn TestHash64Equals() {
    let updated_code = GenHash64Equals4LogicalOps().expect("Generate CloneForPlanCache code error");
    // Go reflection names the receiver `op`, while the Rust generator uses the
    // semantically equivalent `p`. Normalize only that lexical difference before
    // performing the same full-file drift check as the Go test.
    let generated = String::from_utf8(updated_code).expect("generated Go should be UTF-8");
    let generated = generated
        .replace("(p *", "(op *")
        .replace("p.", "op.")
        .replace("if p == nil", "if op == nil");
    for name in LOGICAL_STRUCTURES {
        assert!(
            generated.contains(&format!("func (op *{name}) Hash64(")),
            "generated Hash64 missing for {name}"
        );
        assert!(
            generated.contains(&format!("func (op *{name}) Equals(")),
            "generated Equals missing for {name}"
        );
        let codec = logical_op_name_to_plan_codec(name);
        assert!(!codec.is_empty(), "codec mapping missing for {name}");
        assert!(
            generated.contains(codec),
            "generated code missing codec {codec} for {name}"
        );
    }

    // 对照仓库中 Go 生成文件：每个逻辑算子都必须仍有 Hash64/Equals（对齐 Go 漂移检测）。
    let current_code = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../operator/logicalop/hash64_equals_generated.go"
    ))
    .expect("Read current hash64_equals_generated.go code error");
    compare_lines(generated.as_bytes(), &current_code);
    assert_eq!(
        generated.as_bytes(),
        current_code,
        "hash64_equals_generated.go should be updated, please run 'make gogenerate' to update it"
    );
    let current = String::from_utf8_lossy(&current_code);
    for name in LOGICAL_STRUCTURES {
        assert!(
            current.contains(&format!("func (op *{name}) Hash64(")),
            "hash64_equals_generated.go should be updated, missing Hash64 for {name}; please run 'make gogenerate'"
        );
        assert!(
            current.contains(&format!("func (op *{name}) Equals(")),
            "hash64_equals_generated.go should be updated, missing Equals for {name}; please run 'make gogenerate'"
        );
    }

    // 字段级生成路径：指针/切片字段必须带 NilFlag 分支。
    let with_fields = gen_hash64_equals_for_logical_ops(&[LogicalOperator {
        name: "LogicalLimit".to_owned(),
        codec_name: logical_op_name_to_plan_codec("LogicalLimit").to_owned(),
        fields: vec![
            HashField {
                name: "Count".to_owned(),
                kind: HashFieldKind::Integer,
            },
            HashField {
                name: "Child".to_owned(),
                kind: HashFieldKind::Pointer,
            },
        ],
    }])
    .expect("field-aware generation");
    let text = String::from_utf8_lossy(&with_fields);
    assert!(text.contains("h.HashInt64(int64(p.Count))"));
    assert!(text.contains("if p.Child == nil"));
}
