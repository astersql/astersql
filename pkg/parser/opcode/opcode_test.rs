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

// 运算符 Format/String 行为的单元测试。
//
// 对应 Go 的 `opcode_test.go`：遍历全部有效 Op，并确认下标 0 哨兵为空名。

// 本文件对照 pkg/parser/opcode/opcode_test.go，保持有效 opcode 遍历和无效 opcode 的空字符串语义。

use super::{Op, ops};

/// ALL_OPS 列出全部有效运算符，长度比 `ops` 少 1（排除下标 0 空位）。
const ALL_OPS: [Op; 31] = [
    Op::LogicAnd,
    Op::LeftShift,
    Op::RightShift,
    Op::LogicOr,
    Op::GE,
    Op::LE,
    Op::EQ,
    Op::NE,
    Op::LT,
    Op::GT,
    Op::Plus,
    Op::Minus,
    Op::And,
    Op::Or,
    Op::Mod,
    Op::Xor,
    Op::Div,
    Op::Mul,
    Op::Not,
    Op::Not2,
    Op::BitNeg,
    Op::IntDiv,
    Op::LogicXor,
    Op::NullEQ,
    Op::In,
    Op::Like,
    Op::Case,
    Op::Regexp,
    Op::IsNull,
    Op::IsTruth,
    Op::IsFalsity,
];

// test_t 对应 Go 的 TestT，覆盖 Plus.String、所有 ops 的 Format 输出和无效 opcode 的容错分支。
/// test_t 对应 Go 的 TestT：抽查 Plus.String，并校验每个 Op 的 Format 与表内 literal 一致。
#[test]
fn test_t() {
    let op = Op::Plus;
    if op.String() != "plus" {
        panic!("invalid op code");
    }

    let mut buf: Vec<u8> = Vec::new();
    // ops 含下标 0 哨兵，故长度 = 有效 Op 数 + 1。
    assert_eq!(ops.len(), ALL_OPS.len() + 1);
    for op in ALL_OPS {
        op.Format(&mut buf);
        let formatted = String::from_utf8(buf.clone()).expect("opcode literal should be utf8");
        assert_eq!(
            formatted, ops[op as usize].literal,
            "format op fail {:?}",
            op
        );
        buf.clear();
    }

    // Test invalid opcode
    // Go 的 Op 是整数别名，可以表示 0；Rust 枚举不能安全构造无效判别值，因此直接核对同一张表的 0 号哨兵。
    assert!(ops[0].name.is_empty());
}
