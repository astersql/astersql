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

// 全文索引解析与下推（push-down）规则。
//
// 将逻辑计划中的 `FTS_MATCH_WORD()` 表达式解析为可下推到 TiFlash 的
// `FTSPushDown` 元数据：WHERE 绑定单列全文索引、TopN/Projection 对齐评分列
// `_FTS_SCORE`，最后拒绝残留的不支持用法。脏事务（有未提交写）禁止 FTS。

use crate::{PlanKind, PlanNode};

/// Top-K 上界：未指定 LIMIT 时取满量（u32::MAX）。
pub const maxFTSTopK: u32 = u32::MAX;
/// 脏事务（存在未提交变更）下使用 FTS_MATCH_WORD 的错误文案。
pub const ftsMatchWordDirtyTxnErrMsg: &str =
    "FTS_MATCH_WORD() cannot be used in a transaction with uncommitted changes";

// 编码进 DataSource.operator_info 的下推标记与字段分隔符。
const FTS_MARKER: &str = "fts_push_down";
const FTS_SEPARATOR: char = '\u{1f}';

#[derive(Clone, Debug, Eq, PartialEq)]
/// 全文索引描述：索引名与覆盖列。
pub struct FullTextIndex {
    /// 索引名。
    pub name: String,
    /// 索引覆盖的列名（当前下推仅支持单列）。
    pub columns: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// FTS 查询类型：是否需要返回相关性评分。
pub enum FTSQueryType {
    /// 仅匹配，不需要评分。
    NoScore,
    /// 需要 `_FTS_SCORE` 评分（ORDER BY / SELECT 使用）。
    WithScore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 编码到扫描算子上的 FTS 下推载荷。
pub struct FTSPushDown {
    /// 匹配到的全文索引名。
    pub index_name: String,
    /// 检索列名。
    pub column_name: String,
    /// 常量查询文本。
    pub query_text: String,
    /// 是否带评分。
    pub query_type: FTSQueryType,
    /// 下推的 Top-K 上限。
    pub top_k: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 从 `FTS_MATCH_WORD(query, column)` 解析出的查询与列。
struct FTSInfo {
    query: String,
    column: String,
}

impl FTSPushDown {
    /// 将下推载荷序列化为带标记的分隔字符串。
    fn encode(&self) -> String {
        let query_type = match self.query_type {
            FTSQueryType::NoScore => "no_score",
            FTSQueryType::WithScore => "with_score",
        };
        [
            FTS_MARKER.to_owned(),
            self.index_name.clone(),
            self.column_name.clone(),
            self.query_text.clone(),
            query_type.to_owned(),
            self.top_k.to_string(),
        ]
        .join(&FTS_SEPARATOR.to_string())
    }

    /// 从算子信息反序列化下推载荷；格式不符返回 None。
    fn decode(value: &str) -> Option<Self> {
        let mut parts = value.split(FTS_SEPARATOR);
        if parts.next()? != FTS_MARKER {
            return None;
        }
        let index_name = parts.next()?.to_owned();
        let column_name = parts.next()?.to_owned();
        let query_text = parts.next()?.to_owned();
        let query_type = match parts.next()? {
            "no_score" => FTSQueryType::NoScore,
            "with_score" => FTSQueryType::WithScore,
            _ => return None,
        };
        let top_k = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            index_name,
            column_name,
            query_text,
            query_type,
            top_k,
        })
    }
}

/// 尝试从计划节点的 operator_info 解码 FTS 下推信息。
pub fn FullTextPushDown(plan: &PlanNode) -> Option<FTSPushDown> {
    FTSPushDown::decode(&plan.operator_info)
}

/// 字符串中是否包含 FTS_MATCH_WORD( 调用（大小写不敏感）。
fn contains_fts_text(value: &str) -> bool {
    value.to_ascii_uppercase().contains("FTS_MATCH_WORD(")
}

/// FTS 查询参数是否为非常量（如 `?` 参数标记）。
fn contains_non_constant_fts_query(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    let Some(function_start) = upper.find("FTS_MATCH_WORD(") else {
        return false;
    };
    let arguments = &value[function_start + "FTS_MATCH_WORD(".len()..];
    let Some((query, _column)) = split_function_arguments(arguments) else {
        return false;
    };
    parse_query_literal(query).is_none()
}

/// 计划树中是否仍残留 FTS_MATCH_WORD 文本。
fn contains_fts(plan: &PlanNode) -> bool {
    (match &plan.kind {
        PlanKind::Selection { conditions } | PlanKind::UnionScan { conditions } => conditions
            .iter()
            .any(|condition| contains_fts_text(condition)),
        PlanKind::TopN { by_items, .. } => by_items.iter().any(|item| contains_fts_text(item)),
        _ => contains_fts_text(&plan.operator_info),
    }) || plan.children.iter().any(contains_fts)
}

/// 在引号感知下按首个逗号拆分函数实参为 (query, column)。
fn split_function_arguments(value: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\'' if quoted && chars.peek().is_some_and(|(_, next)| *next == '\'') => {
                chars.next();
            }
            '\'' => quoted = !quoted,
            ',' if !quoted => return Some((&value[..index], &value[index + 1..])),
            _ => {}
        }
    }
    None
}

/// 解析单引号字符串字面量，还原转义的 `''`。
fn parse_query_literal(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 2 || !value.starts_with('\'') || !value.ends_with('\'') {
        return None;
    }
    Some(value[1..value.len() - 1].replace("''", "'"))
}

/// 解析形如 `FTS_MATCH_WORD('q', col)` 的表达式文本。
fn interpret_fts_expression(value: &str) -> Option<FTSInfo> {
    let value = value.trim();
    let open = value.find('(')?;
    if !value[..open].trim().eq_ignore_ascii_case("FTS_MATCH_WORD") || !value.ends_with(')') {
        return None;
    }
    let (query, column) = split_function_arguments(&value[open + 1..value.len() - 1])?;
    let query = parse_query_literal(query)?;
    let column = column.trim().trim_matches('`');
    if column.is_empty() || column.contains(',') {
        return None;
    }
    Some(FTSInfo {
        query,
        column: column.to_owned(),
    })
}

/// 查找恰好覆盖该单列的全文索引。
fn find_matching_full_text_index(
    indexes: &[FullTextIndex],
    info: &FTSInfo,
) -> Option<FullTextIndex> {
    indexes
        .iter()
        .find(|index| index.columns.len() == 1 && index.columns[0] == info.column)
        .cloned()
}

/// WHERE 阶段解析器：将 FTS 谓词下推到 DataSource 并移除该谓词。
pub struct FullTextIndexResolverWhere;

impl FullTextIndexResolverWhere {
    /// 优化规则名（与 Go 优化器规则名对齐）。
    pub fn Name(&self) -> &'static str {
        "fts_resolve_index_where"
    }

    /// 在脏事务检查后递归解析 WHERE 中的 FTS 谓词。
    pub fn Optimize(
        &self,
        mut plan: PlanNode,
        indexes: &[FullTextIndex],
        dirty_txn: bool,
    ) -> Result<(PlanNode, bool), String> {
        if dirty_txn && contains_fts(&plan) {
            return Err(ftsMatchWordDirtyTxnErrMsg.to_owned());
        }
        let changed = resolve_where(&mut plan, indexes, true)?;
        Ok((plan, changed))
    }
}

/// 递归处理：在 Selection→DataSource 形态下绑定索引并编码下推。
fn resolve_where(
    plan: &mut PlanNode,
    indexes: &[FullTextIndex],
    is_root: bool,
) -> Result<bool, String> {
    let mut changed = false;
    for child in &mut plan.children {
        changed |= resolve_where(child, indexes, false)?;
    }

    let PlanKind::Selection { conditions } = &plan.kind else {
        return Ok(changed);
    };
    let Some((condition_index, info)) =
        conditions
            .iter()
            .enumerate()
            .find_map(|(index, condition)| {
                interpret_fts_expression(condition).map(|info| (index, info))
            })
    else {
        return Ok(changed);
    };
    if plan.children.len() != 1 || !matches!(plan.children[0].kind, PlanKind::DataSource { .. }) {
        return Ok(changed);
    }
    let index = find_matching_full_text_index(indexes, &info).ok_or_else(|| {
        "Full text search can only be used with a matching fulltext index".to_owned()
    })?;

    let data_source = &mut plan.children[0];
    data_source.access_object = index.name.clone();
    data_source.operator_info = FTSPushDown {
        index_name: index.name,
        column_name: info.column,
        query_text: info.query,
        query_type: FTSQueryType::NoScore,
        top_k: maxFTSTopK,
    }
    .encode();

    let PlanKind::Selection { conditions } = &mut plan.kind else {
        unreachable!();
    };
    conditions.remove(condition_index);
    if conditions.is_empty() && !is_root {
        *plan = plan.children.remove(0);
    }
    Ok(true)
}

/// TopN 阶段：ORDER BY FTS_MATCH_WORD 改为 `_FTS_SCORE`，并可能收紧 top_k。
pub struct FullTextIndexResolverTopN;

impl FullTextIndexResolverTopN {
    /// 优化规则名。
    pub fn Name(&self) -> &'static str {
        "fts_resolve_index_topn"
    }

    /// 递归解析 TopN 上的 FTS 排序项。
    pub fn Optimize(
        &self,
        mut plan: PlanNode,
        _indexes: &[FullTextIndex],
    ) -> Result<(PlanNode, bool), String> {
        let changed = resolve_top_n(&mut plan)?;
        Ok((plan, changed))
    }
}

/// 拆分 ORDER BY 项的表达式与 DESC/ASC 方向；返回 (expr, is_desc)。
fn split_order_direction(value: &str) -> (&str, bool) {
    let value = value.trim();
    if value.len() >= 5 && value[value.len() - 5..].eq_ignore_ascii_case(" DESC") {
        (&value[..value.len() - 5], true)
    } else if value.len() >= 4 && value[value.len() - 4..].eq_ignore_ascii_case(" ASC") {
        (&value[..value.len() - 4], false)
    } else {
        (value, false)
    }
}

/// 取紧邻的 DataSource，以及中间是否隔着 Selection。
fn adjacent_data_source_mut(plan: &mut PlanNode) -> Option<(&mut PlanNode, bool)> {
    if matches!(plan.kind, PlanKind::DataSource { .. }) {
        return Some((plan, false));
    }
    if matches!(plan.kind, PlanKind::Selection { .. }) && plan.children.len() == 1 {
        let child = &mut plan.children[0];
        if matches!(child.kind, PlanKind::DataSource { .. }) {
            return Some((child, true));
        }
    }
    None
}

/// 将 TopN 首项 FTS 排序对齐到已下推的扫描，并改写为 `_FTS_SCORE`。
fn resolve_top_n(plan: &mut PlanNode) -> Result<bool, String> {
    let mut changed = false;
    for child in &mut plan.children {
        changed |= resolve_top_n(child)?;
    }

    let PlanKind::TopN {
        by_items,
        offset,
        count,
    } = &plan.kind
    else {
        return Ok(changed);
    };
    let Some(first_item) = by_items.first() else {
        return Ok(changed);
    };
    let (expression, descending) = split_order_direction(first_item);
    let Some(order_by_info) = interpret_fts_expression(expression) else {
        return Ok(changed);
    };
    let offset = *offset;
    let count = *count;
    let single_item = by_items.len() == 1;
    if plan.children.len() != 1 {
        return Ok(changed);
    }
    let Some((data_source, has_selection)) = adjacent_data_source_mut(&mut plan.children[0]) else {
        return Ok(changed);
    };
    let Some(mut push_down) = FTSPushDown::decode(&data_source.operator_info) else {
        return Ok(changed);
    };
    if order_by_info.column != push_down.column_name || order_by_info.query != push_down.query_text
    {
        return Err("'FTS_MATCH_WORD()' in ORDER BY must match the one in WHERE".to_owned());
    }
    push_down.query_type = FTSQueryType::WithScore;
    if !has_selection && descending && single_item {
        push_down.top_k = offset.saturating_add(count).min(u64::from(maxFTSTopK)) as u32;
    }
    data_source.operator_info = push_down.encode();
    let PlanKind::TopN { by_items, .. } = &mut plan.kind else {
        unreachable!();
    };
    by_items[0] = if descending {
        "_FTS_SCORE DESC".to_owned()
    } else {
        "_FTS_SCORE".to_owned()
    };
    Ok(true)
}

/// Projection 阶段：SELECT 中的 FTS_MATCH_WORD 改为 `_FTS_SCORE`。
pub struct FullTextIndexResolverProjection;

impl FullTextIndexResolverProjection {
    /// 优化规则名。
    pub fn Name(&self) -> &'static str {
        "fts_resolve_index_projection"
    }

    /// 递归解析投影中的 FTS 表达式。
    pub fn Optimize(
        &self,
        mut plan: PlanNode,
        _indexes: &[FullTextIndex],
    ) -> Result<(PlanNode, bool), String> {
        let changed = resolve_projection(&mut plan)?;
        Ok((plan, changed))
    }
}

/// 沿 Selection/TopN 单链向下查找已编码 FTS 下推的 DataSource。
fn find_push_down_mut(plan: &mut PlanNode) -> Option<&mut PlanNode> {
    if matches!(plan.kind, PlanKind::DataSource { .. })
        && FTSPushDown::decode(&plan.operator_info).is_some()
    {
        return Some(plan);
    }
    if plan.children.len() == 1
        && matches!(
            plan.kind,
            PlanKind::Selection { .. } | PlanKind::TopN { .. }
        )
    {
        return find_push_down_mut(&mut plan.children[0]);
    }
    None
}

/// 校验投影 FTS 与 WHERE 下推一致，并切换为 WithScore。
fn resolve_projection(plan: &mut PlanNode) -> Result<bool, String> {
    let mut changed = false;
    for child in &mut plan.children {
        changed |= resolve_projection(child)?;
    }
    if !matches!(plan.kind, PlanKind::Projection) {
        return Ok(changed);
    }
    let Some(projection_info) = interpret_fts_expression(&plan.operator_info) else {
        return Ok(changed);
    };
    if plan.children.len() != 1 {
        return Ok(changed);
    }
    let Some(data_source) = find_push_down_mut(&mut plan.children[0]) else {
        return Ok(changed);
    };
    let Some(mut push_down) = FTSPushDown::decode(&data_source.operator_info) else {
        unreachable!();
    };
    if projection_info.column != push_down.column_name
        || projection_info.query != push_down.query_text
    {
        return Err("'FTS_MATCH_WORD()' in SELECT must match the one in WHERE".to_owned());
    }
    push_down.query_type = FTSQueryType::WithScore;
    data_source.operator_info = push_down.encode();
    plan.operator_info = "_FTS_SCORE".to_owned();
    Ok(true)
}

/// 收尾规则：拒绝仍未解析的 FTS 用法，保证不支持路径明确报错。
pub struct FullTextIndexResolverRejectRemaining;

impl FullTextIndexResolverRejectRemaining {
    /// 优化规则名。
    pub fn Name(&self) -> &'static str {
        "fts_resolve_reject_remaining"
    }

    /// 扫描计划树，对残留 FTS 表达式给出与 Go 一致的错误。
    pub fn Optimize(&self, plan: PlanNode) -> Result<(PlanNode, bool), String> {
        reject_remaining(&plan)?;
        Ok((plan, false))
    }
}

/// 按 Go 解析器顺序串联各阶段：WHERE → TopN → Projection → Reject。
/// 每阶段消费上一阶段产物；最终拒绝规则证明无不支持 FTS 表达式漏网。
/// Runs the Go resolver order as one production entry point. Each phase sees
/// the plan produced by the previous phase, and the final reject rule proves
/// that no unsupported FTS expression escaped resolution.
pub fn ResolveFullTextPlan(
    plan: PlanNode,
    indexes: &[FullTextIndex],
    dirty_txn: bool,
) -> Result<PlanNode, String> {
    let (plan, _) = FullTextIndexResolverWhere.Optimize(plan, indexes, dirty_txn)?;
    let (plan, _) = FullTextIndexResolverTopN.Optimize(plan, indexes)?;
    let (plan, _) = FullTextIndexResolverProjection.Optimize(plan, indexes)?;
    FullTextIndexResolverRejectRemaining
        .Optimize(plan)
        .map(|(plan, _)| plan)
}

/// 按节点种类检查残留 FTS，并递归子节点。
fn reject_remaining(plan: &PlanNode) -> Result<(), String> {
    match &plan.kind {
        PlanKind::Projection => {
            if interpret_fts_expression(&plan.operator_info).is_some() {
                return Err("'FTS_MATCH_WORD()' in SELECT requires a matching 'FTS_MATCH_WORD()' in WHERE. A valid example: SELECT FTS_MATCH_WORD(...) FROM <TABLE> WHERE FTS_MATCH_WORD(...)".to_owned());
            }
            if contains_fts_text(&plan.operator_info) {
                return Err("'FTS_MATCH_WORD()' in SELECT must not be wrapped in expressions. A valid example: SELECT FTS_MATCH_WORD(...) FROM <TABLE> WHERE FTS_MATCH_WORD(...)".to_owned());
            }
        }
        PlanKind::UnionScan { conditions } => {
            if conditions
                .iter()
                .any(|condition| contains_fts_text(condition))
            {
                return Err(ftsMatchWordDirtyTxnErrMsg.to_owned());
            }
        }
        PlanKind::DataSource { .. } => {
            if FTSPushDown::decode(&plan.operator_info).is_none()
                && contains_fts_text(&plan.operator_info)
            {
                return Err("Currently 'FTS_MATCH_WORD()' must be used alone. It cannot be placed inside any other function or expression as a parameter, or used multiple times. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...)".to_owned());
            }
        }
        PlanKind::Selection { conditions } => {
            let contains_fts = conditions
                .iter()
                .any(|condition| contains_fts_text(condition));
            if conditions
                .iter()
                .any(|condition| contains_non_constant_fts_query(condition))
            {
                return Err("match against a non-constant string".to_owned());
            }
            if contains_fts
                && plan.children.len() == 1
                && matches!(plan.children[0].kind, PlanKind::UnionScan { .. })
            {
                return Err(ftsMatchWordDirtyTxnErrMsg.to_owned());
            }
            if contains_fts {
                return Err("Currently 'FTS_MATCH_WORD()' must be used alone. It cannot be placed inside any other function or expression as a parameter, or used multiple times. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...)".to_owned());
            }
        }
        PlanKind::TopN { by_items, .. } => {
            for (index, item) in by_items.iter().enumerate() {
                if contains_fts_text(item) {
                    if index > 0 {
                        return Err("FTS_MATCH_WORD() must be used as the first item in ORDER BY. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...) ORDER BY FTS_MATCH_WORD(...) LIMIT ..".to_owned());
                    }
                    return Err("Unsupported 'FTS_MATCH_WORD()' usage. It must be used with a WHERE clause and must be used alone. A valid example: SELECT * FROM <TABLE> WHERE FTS_MATCH_WORD(...) ORDER BY FTS_MATCH_WORD(...) LIMIT ..".to_owned());
                }
            }
        }
        PlanKind::Sort => {
            if contains_fts_text(&plan.operator_info) {
                return Err("Currently 'FTS_MATCH_WORD()' in ORDER BY without a LIMIT clause is not supported, try specify a very large LIMIT as a workaround".to_owned());
            }
        }
        _ => {}
    }
    for child in &plan.children {
        reject_remaining(child)?;
    }
    Ok(())
}
