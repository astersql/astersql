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

// MATCH ... AGAINST 的 ILIKE 回退入口。
//
// 与 Go 一致，本模块只负责把 planner 上下文和表达式参数转交给 expression
// 包的共享构造器；token 校验、modifier 语义、NULL 处理和谓词树组合均由共享
// 实现统一维护，避免 planner 与选择率估算产生不同的回退语义。

/// 对应 Go `expressionRewriter.convertMatchAgainstToLike` 的薄封装。
pub fn convertMatchAgainstToLike(
    context: &dyn expression_dependency::BuildContext,
    columns: Vec<expression_dependency::ExprBox>,
    search_text: String,
    modifier: u8,
) -> Result<expression_dependency::ExprBox, expression_dependency::Error> {
    expression_dependency::BuildFTSToILikeExpression(context, columns, search_text, modifier)
}
