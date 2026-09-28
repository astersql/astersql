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

// 表达式运算符（opcode）枚举及其显示/恢复元数据。
//
// 每个 `Op` 对应 AST 中一类运算；`ops` 表给出内部名、SQL 字面量与是否关键字。
// 本文件不执行运算，只服务于 Format 与 Restore。

// 本文件对照 pkg/parser/opcode/opcode.go，保留运算符编号、显示文本及恢复 SQL 的分支。

use std::io::{self, Write};

use crate::format::RestoreCtx;

// Op 对应 Go 的 int 别名；repr(usize) 让枚举值可直接作为 ops 表下标。
/// Op 为表达式运算符枚举；判别值从 1 起，与 Go iota 对齐。
#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Op {
    /// 逻辑与 AND。
    LogicAnd = 1,
    /// 左移 <<。
    LeftShift,
    /// 右移 >>。
    RightShift,
    /// 逻辑或 OR。
    LogicOr,
    /// 大于等于 >=。
    GE,
    /// 小于等于 <=。
    LE,
    /// 等于 =。
    EQ,
    /// 不等于 !=。
    NE,
    /// 小于 <。
    LT,
    /// 大于 >。
    GT,
    /// 加法 +。
    Plus,
    /// 减法 -。
    Minus,
    /// 按位与 &。
    And,
    /// 按位或 |。
    Or,
    /// 取模 %。
    Mod,
    /// 按位异或 ^。
    Xor,
    /// 除法 /。
    Div,
    /// 乘法 *。
    Mul,
    /// 逻辑非 not（带空格字面量）。
    Not,
    /// 逻辑非 !。
    Not2,
    /// 按位取反 ~。
    BitNeg,
    /// 整数除 DIV。
    IntDiv,
    /// 逻辑异或 XOR。
    LogicXor,
    /// NULL-safe 等于 <=>。
    NullEQ,
    /// IN 谓词。
    In,
    /// LIKE 谓词。
    Like,
    /// CASE 表达式。
    Case,
    /// REGEXP 谓词。
    Regexp,
    /// IS NULL。
    IsNull,
    /// IS TRUE。
    IsTruth,
    /// IS FALSE。
    IsFalsity,
}

// OpInfo 对应 Go 匿名结构体；name 用于 String，literal 用于输出 SQL，isKeyword 决定恢复时的大小写策略。
/// OpInfo 描述单个运算符的内部名、SQL 字面量与关键字标记。
#[derive(Clone, Copy)]
struct OpInfo {
    name: &'static str,
    literal: &'static str,
    isKeyword: bool,
}

/// 下标 0 的空占位，对应 Go iota 从 1 开始留下的空洞。
const EMPTY_OP: OpInfo = OpInfo {
    name: "",
    literal: "",
    isKeyword: false,
};

// ops 的第 0 项保持为空，因为 Go 的 iota 从 1 开始；其余下标与 Op 判别值一一对应。
/// ops 按下标存放全部运算符元数据；`ops[op as usize]` 即对应该 Op。
static ops: [OpInfo; 32] = [
    EMPTY_OP,
    OpInfo {
        name: "and",
        literal: "AND",
        isKeyword: true,
    },
    OpInfo {
        name: "leftshift",
        literal: "<<",
        isKeyword: false,
    },
    OpInfo {
        name: "rightshift",
        literal: ">>",
        isKeyword: false,
    },
    OpInfo {
        name: "or",
        literal: "OR",
        isKeyword: true,
    },
    OpInfo {
        name: "ge",
        literal: ">=",
        isKeyword: false,
    },
    OpInfo {
        name: "le",
        literal: "<=",
        isKeyword: false,
    },
    OpInfo {
        name: "eq",
        literal: "=",
        isKeyword: false,
    },
    // 与 Go 保持一致使用 !=；源码注释指出另一种可能形式是 <>。
    OpInfo {
        name: "ne",
        literal: "!=",
        isKeyword: false,
    },
    OpInfo {
        name: "lt",
        literal: "<",
        isKeyword: false,
    },
    OpInfo {
        name: "gt",
        literal: ">",
        isKeyword: false,
    },
    OpInfo {
        name: "plus",
        literal: "+",
        isKeyword: false,
    },
    OpInfo {
        name: "minus",
        literal: "-",
        isKeyword: false,
    },
    OpInfo {
        name: "bitand",
        literal: "&",
        isKeyword: false,
    },
    OpInfo {
        name: "bitor",
        literal: "|",
        isKeyword: false,
    },
    OpInfo {
        name: "mod",
        literal: "%",
        isKeyword: false,
    },
    OpInfo {
        name: "bitxor",
        literal: "^",
        isKeyword: false,
    },
    OpInfo {
        name: "div",
        literal: "/",
        isKeyword: false,
    },
    OpInfo {
        name: "mul",
        literal: "*",
        isKeyword: false,
    },
    OpInfo {
        name: "not",
        literal: "not ",
        isKeyword: true,
    },
    OpInfo {
        name: "!",
        literal: "!",
        isKeyword: false,
    },
    OpInfo {
        name: "bitneg",
        literal: "~",
        isKeyword: false,
    },
    OpInfo {
        name: "intdiv",
        literal: "DIV",
        isKeyword: true,
    },
    OpInfo {
        name: "xor",
        literal: "XOR",
        isKeyword: true,
    },
    OpInfo {
        name: "nulleq",
        literal: "<=>",
        isKeyword: false,
    },
    OpInfo {
        name: "in",
        literal: "IN",
        isKeyword: true,
    },
    OpInfo {
        name: "like",
        literal: "LIKE",
        isKeyword: true,
    },
    OpInfo {
        name: "case",
        literal: "CASE",
        isKeyword: true,
    },
    OpInfo {
        name: "regexp",
        literal: "REGEXP",
        isKeyword: true,
    },
    OpInfo {
        name: "isnull",
        literal: "IS NULL",
        isKeyword: true,
    },
    OpInfo {
        name: "istrue",
        literal: "IS TRUE",
        isKeyword: true,
    },
    OpInfo {
        name: "isfalse",
        literal: "IS FALSE",
        isKeyword: true,
    },
];

impl Op {
    // String 对应 Go Stringer 实现，返回稳定的内部名称而不是 SQL 字面量。
    /// String 返回稳定的内部名称（如 `"plus"`），不是 SQL 字面量。
    pub fn String(self) -> &'static str {
        ops[self as usize].name
    }

    // Format 对应 Go io.WriteString；Go 源码有意忽略写入错误，这里也不把错误向上传播。
    /// Format 将 SQL 字面量写入 writer；与 Go 一样忽略写入错误。
    pub fn Format<W: Write>(self, writer: &mut W) {
        let _ = writer.write_all(ops[self as usize].literal.as_bytes());
    }

    // IsKeyword 报告 Restore 应调用 WriteKeyWord 还是 WritePlain。
    /// IsKeyword 为 true 时 Restore 走关键字路径（可变换大小写）。
    pub fn IsKeyword(self) -> bool {
        ops[self as usize].isKeyword
    }

    // Restore 对应 Go 的 SQL 恢复逻辑；上下文本身属于外部 format 模块，本文件不执行 IO。
    /// Restore 按 isKeyword 选择 WriteKeyWord 或 WritePlain 写回 SQL 片段。
    pub fn Restore(self, ctx: &mut RestoreCtx) -> io::Result<()> {
        let info = &ops[self as usize];
        // 关键字需经 RestoreCtx 处理大小写标志；普通符号原样写出。
        if info.isKeyword {
            ctx.WriteKeyWord(info.literal)?;
        } else {
            ctx.WritePlain(info.literal)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "opcode_test.rs"]
mod opcode_test;
