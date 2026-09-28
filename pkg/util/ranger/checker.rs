// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 索引 access condition 可用性检查：判断谓词能否推入索引 range 构造。
//
// 对齐 Go `checker.go`。Access condition 指可转化为索引扫描区间的过滤条件；
// 无法精确覆盖时需 `shouldReserve` 保留为 filter（Selection）。覆盖比较、IS NULL、
// IN、LIKE、逻辑与/或及 collation/前缀索引边界。

// 索引 access condition 可用性检查，对齐 checker.go。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

use crate::{ast, collate, mysql, types};

// conditionChecker checks if this condition can be pushed to index planner.
// conditionChecker 对应 Go 的同名结构体，用于围绕一个待检查列判断表达式能否推入索引 range。
/// 围绕目标列检查表达式是否可推入索引 range 规划。
pub(crate) struct conditionChecker<'a> {
    // EvalContext 在 Go 中来自 expression 包；这里保留上下文对象，用于 GetType、EqualByExprAndID 等判断。
    pub(crate) ctx: &'a dyn expression::EvalContext,
    // Go 字段类型是 *expression.Column，nil 表示没有可匹配的虚拟表达式列；用 Option 显式表示。
    pub(crate) checkerCol: Option<expression::Column>,
    // length 是索引列前缀长度；types::UnspecifiedLength 或等于列完整长度时表示 full length。
    pub(crate) length: isize,
    // optPrefixIndexSingleScan 影响 IS NULL 在前缀索引上的 filter 保留策略。
    pub(crate) optPrefixIndexSingleScan: bool,
}

impl conditionChecker<'_> {
    // isFullLengthColumn 判断当前索引列是否按完整长度参与 range 构造。
    fn isFullLengthColumn(&self) -> bool {
        if self.length == types::UnspecifiedLength as isize {
            return true;
        }
        let Some(checkerCol) = self.checkerCol.as_ref() else {
            // Go 代码在 checkerCol 为 nil 时若调用到这里会触发 nil 指针问题；保守返回 false 并标明差异。
            return false;
        };
        self.length == checkerCol.GetType(self.ctx).GetFlen() as isize
    }

    // check returns two values, isAccessCond and shouldReserve.
    // isAccessCond indicates whether the condition can be used to build ranges.
    // shouldReserve indicates whether the condition should be reserved in filter conditions.
    // check 是 conditionChecker 的入口：按 Go 的 type switch 分流 ScalarFunction、Column、Constant。
    /// 返回 `(isAccessCond, shouldReserve)`：能否建 range，以及是否仍保留为 filter。
    pub(crate) fn check(&self, condition: &dyn expression::Expression) -> (bool, bool) {
        if let Some(x) = condition.as_scalar_function() {
            return self.checkScalarFunction(x);
        }
        if let Some(x) = condition.as_column() {
            // 字符串列裸条件不能直接用来构造 range，必须保留为 filter。
            if x.GetType(self.ctx).EvalType() == types::ETString {
                return (false, true);
            }
            return self.checkColumn(condition);
        }
        if condition.as_constant().is_some() {
            // 常量条件本身不依赖列，可视为 access 条件且无需额外保留。
            return (true, false);
        }
        (false, true)
    }

    // checkScalarFunction 迁移 Go 中对标量函数名的 switch。
    // 返回值第一项表示能否建 range，第二项表示是否还要保留原表达式做精确过滤。
    fn checkScalarFunction(&self, scalar: &expression::ScalarFunction) -> (bool, bool) {
        let (_, collation) = scalar.CharsetAndCollation();
        match scalar.FuncName.L.as_str() {
            ast::LogicOr | ast::LogicAnd => {
                let args = scalar.GetArgs();
                // AND/OR 两侧都必须能成为 access 条件；否则整个逻辑表达式降级为 filter。
                let (isAccessCond0, shouldReserve0) = self.check(args[0].as_ref());
                let (isAccessCond1, shouldReserve1) = self.check(args[1].as_ref());
                if isAccessCond0 && isAccessCond1 {
                    return (true, shouldReserve0 || shouldReserve1);
                }
                return (false, true);
            }
            ast::EQ | ast::NE | ast::GE | ast::GT | ast::LE | ast::LT | ast::NullEQ => {
                let args = scalar.GetArgs();
                if args[0].as_constant().is_some() {
                    if self.matchColumn(args[1].as_ref()) {
                        // Checks whether the scalar function is calculated use the collation compatible with the column.
                        // 常量在左、列在右时，字符串列必须检查函数 collation 是否与列 collation 兼容。
                        // 二进制 collation 的 EQ/NullEQ 是特殊近似范围：可建 range，但必须保留 filter 确保二进制比较语义。
                        if args[1].GetType(self.ctx).EvalType() == types::ETString
                            && !collate::CompatibleCollate(
                                args[1].GetType(self.ctx).GetCollate(),
                                &collation,
                            )
                        {
                            // When comparing a column with a binary-collation constant (e.g. col = CAST(x AS BINARY)),
                            // allow building an approximate index range for EQ/NullEQ. The condition is kept as a
                            // filter (shouldReserve=true) to ensure the binary comparison semantics are enforced.
                            if collate::IsBinCollation(&collation)
                                && (scalar.FuncName.L == ast::EQ
                                    || scalar.FuncName.L == ast::NullEQ)
                            {
                                return (true, true);
                            }
                            return (false, true);
                        }
                        let isFullLength = self.isFullLengthColumn();
                        if scalar.FuncName.L == ast::NE {
                            // 前缀索引上的 != 无法单靠 range 精确过滤；完整长度才可直接作为 access 条件。
                            return (isFullLength, !isFullLength);
                        }
                        return (true, !isFullLength);
                    }
                }
                if args[1].as_constant().is_some() {
                    if self.matchColumn(args[0].as_ref()) {
                        // Checks whether the scalar function is calculated use the collation compatible with the column.
                        // 常量在右、列在左时复用同一套 collation 和前缀索引保留规则。
                        if args[0].GetType(self.ctx).EvalType() == types::ETString
                            && !collate::CompatibleCollate(
                                args[0].GetType(self.ctx).GetCollate(),
                                &collation,
                            )
                        {
                            if collate::IsBinCollation(&collation)
                                && (scalar.FuncName.L == ast::EQ
                                    || scalar.FuncName.L == ast::NullEQ)
                            {
                                return (true, true);
                            }
                            return (false, true);
                        }
                        let isFullLength = self.isFullLengthColumn();
                        if scalar.FuncName.L == ast::NE {
                            return (isFullLength, !isFullLength);
                        }
                        return (true, !isFullLength);
                    }
                }
            }
            ast::IsNull => {
                let args = scalar.GetArgs();
                if self.matchColumn(args[0].as_ref()) {
                    let mut isNullReserve = false;
                    // We can know whether the column is null from prefix column of any length.
                    // optPrefixIndexSingleScan 打开时，Go 代码允许前缀列 IS NULL 不再额外保留 filter。
                    if !self.optPrefixIndexSingleScan {
                        isNullReserve = !self.isFullLengthColumn();
                    }
                    return (true, isNullReserve);
                }
                return (false, true);
            }
            ast::IsTruthWithoutNull | ast::IsFalsity | ast::IsTruthWithNull => {
                let args = scalar.GetArgs();
                if let Some(s) = args[0].as_column() {
                    // 字符串列的布尔真值判断不能安全转换成索引 range。
                    if s.GetType(self.ctx).EvalType() == types::ETString {
                        return (false, true);
                    }
                }
                return self.checkColumn(args[0].as_ref());
            }
            ast::UnaryNot => {
                let args = scalar.GetArgs();
                // TODO: support "not like" convert to access conditions.
                let Some(s) = args[0].as_scalar_function() else {
                    // "not column" or "not constant" can't lead to a range.
                    // Go 类型断言失败时直接返回不可建 range，避免把 NOT column/constant 误当作索引条件。
                    return (false, true);
                };
                if s.FuncName.L == ast::Like || s.FuncName.L == ast::NullEQ {
                    return (false, true);
                }
                return self.check(args[0].as_ref());
            }
            ast::In => {
                let args = scalar.GetArgs();
                if !self.matchColumn(args[0].as_ref()) {
                    return (false, true);
                }
                if args[0].GetType(self.ctx).EvalType() == types::ETString
                    && !collate::CompatibleCollate(
                        args[0].GetType(self.ctx).GetCollate(),
                        &collation,
                    )
                {
                    if !collate::IsBinCollation(&collation) {
                        return (false, true);
                    }
                    // Binary collation mismatch: verify all IN-list values are constants before
                    // allowing approximate range building with a filter for correctness.
                    // 二进制 collation 不兼容时，IN 列表必须全是常量，才允许构建近似 range 并保留 filter。
                    for v in args.iter().skip(1) {
                        if v.as_constant().is_none() {
                            return (false, true);
                        }
                    }
                    return (true, true);
                }
                for v in args.iter().skip(1) {
                    // 普通 IN 也要求每个候选值都是常量；含表达式时不能稳定建索引 range。
                    if v.as_constant().is_none() {
                        return (false, true);
                    }
                }
                return (true, !self.isFullLengthColumn());
            }
            ast::Like => {
                return self.checkLikeFunc(scalar);
            }
            ast::GetParam => {
                // TODO
                // Go 原实现暂时把参数占位视为可建 range，且不需要保留 filter；这里只保留该策略。
                return (true, false);
            }
            _ => {}
        }
        (false, true)
    }

    // checkLikeFunc 迁移 LIKE 谓词检查逻辑。
    // 它只判断是否能用 LIKE 前缀构造 range，真实模式匹配和字符串比较仍由 Go 侧表达式语义决定。
    fn checkLikeFunc(&self, scalar: &expression::ScalarFunction) -> (bool, bool) {
        let (_, collation) = scalar.CharsetAndCollation();
        let args = scalar.GetArgs();
        if !collate::CompatibleCollate(args[0].GetType(self.ctx).GetCollate(), &collation) {
            return (false, true);
        }
        if !self.matchColumn(args[0].as_ref()) {
            return (false, true);
        }
        let Some(pattern) = args[1].as_constant() else {
            // LIKE pattern 不是常量时，无法在规划阶段确定可扫描前缀。
            return (false, true);
        };
        if pattern.Value.IsNull() {
            return (false, true);
        }
        let patternStr = match pattern.Value.ToString() {
            Ok(patternStr) => patternStr,
            Err(_err) => {
                // Go 这里吞掉 ToString 错误并把条件保留为 filter；保持同样的错误处理形状。
                return (false, true);
            }
        };
        let mut likeFuncReserve = !self.isFullLengthColumn();

        // Different from `=`, trailing spaces are always significant, and can't be ignored in `like`.
        // In tidb's implementation, for PAD SPACE collations, the trailing spaces are removed in the index key. So we are
        // unable to distinguish 'xxx' from 'xxx ' by a single index range scan, and we may read more data than needed by
        // the `like` function. Therefore, a Selection is needed to filter the data.
        // PAD SPACE 排序规则会让索引 key 去掉尾随空格，因此 LIKE 必须保留 filter 才能恢复精确语义。
        if collate::IsPadSpaceCollation(&collation) {
            likeFuncReserve = true;
        }

        if patternStr.len() == 0 {
            return (true, likeFuncReserve);
        }
        let escape = args[2]
            .as_constant()
            .map(|constant| constant.Value.GetInt64() as u8)
            .unwrap_or_default();
        let patternBytes = patternStr.as_bytes();
        let mut i = 0usize;
        while i < patternBytes.len() {
            if patternBytes[i] == escape {
                // Go 的 for 循环在命中 escape 后先跳过被转义字符，再由循环自增继续扫描。
                i += 1;
                if i < patternBytes.len() - 1 {
                    i += 1;
                    continue;
                }
                break;
            }
            if i == 0 && (patternBytes[i] == b'%' || patternBytes[i] == b'_') {
                return (false, true);
            }
            if patternBytes[i] == b'%' {
                // We currently do not support using `enum like 'xxx%'` to build range
                // see https://github.com/pingcap/tidb/issues/27130 for more details
                // enum 类型 LIKE 前缀 range 仍未支持，必须退回 filter。
                if args[0].GetType(self.ctx).GetType() == mysql::TypeEnum {
                    return (false, true);
                }
                if i != patternBytes.len() - 1 {
                    likeFuncReserve = true;
                }
                break;
            }
            if patternBytes[i] == b'_' {
                // We currently do not support using `enum like 'xxx_'` to build range
                // see https://github.com/pingcap/tidb/issues/27130 for more details
                if args[0].GetType(self.ctx).GetType() == mysql::TypeEnum {
                    return (false, true);
                }
                // 单字符通配符会扩大扫描范围，所以即使能建前缀 range 也要保留 filter。
                likeFuncReserve = true;
                break;
            }
            i += 1;
        }
        (true, likeFuncReserve)
    }

    // matchColumn 匹配虚拟表达式列。
    fn matchColumn(&self, expr: &dyn expression::Expression) -> bool {
        // Check if virtual expression column matched
        if let Some(checkerCol) = self.checkerCol.as_ref() {
            return checkerCol.EqualByExprAndID(self.ctx, expr);
        }
        false
    }

    // checkColumn 迁移裸列条件判断：只有匹配当前 checkerCol 的列才可作为 access 条件。
    fn checkColumn(&self, expr: &dyn expression::Expression) -> (bool, bool) {
        if self.matchColumn(expr) {
            return (true, !self.isFullLengthColumn());
        }
        (false, true)
    }
}
