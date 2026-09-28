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

// 语句适配层的慢查询（slow query）规则引擎。
//
// 慢查询日志记录执行时间或代价超过阈值的 SQL。本文件负责：
// - 合并会话级与全局慢日志规则的生效字段；
// - 按规则匹配是否应写入慢日志；
// - 通过对象池复用 `SlowQueryLogItems`；
// - 支持 hint 强制写出完整慢日志条目。
//
// 规则之间为 OR，单条规则内条件为 AND（与 Go 语义一致）。

#![allow(non_snake_case)]

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use astersql_sessionctx_variable::session::SessionVars;
use astersql_sessionctx_variable::slow_log::{
    GlobalSlowLogRules, SlowLogRuleFieldAccessors, SlowLogRules, SlowQueryLogItems, Threshold,
    UnsetConnID,
};
use astersql_util_logutil::log::{self, Logger};

/// Session-local slow-log rule state. It is kept beside `SessionVars` until the
/// session package finishes exposing the corresponding Go fields.
///
/// 会话本地的慢日志规则状态（在 SessionVars 暴露对应 Go 字段前暂存于此）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSlowLogRules {
    /// 会话级慢日志规则；None 表示未配置。
    pub rules: Option<SlowLogRules>,
    /// 合并后需要填充的生效字段名集合。
    pub effective_fields: BTreeSet<String>,
    /// 上次同步时全局 raw rules 的哈希，用于检测全局规则变更。
    pub global_raw_rules_hash: u64,
    /// 是否需要重新计算 effective_fields。
    pub need_update_effective_fields: bool,
}

/// The field registry is owned by the session layer in Go. This trait keeps the
/// same ownership boundary while allowing the executor rule engine to be used
/// before all `SessionVars` fields have landed in Rust.
///
/// 慢日志规则上下文：提供连接 ID、规则状态与字段注册/匹配能力。
pub trait SlowLogRuleContext {
    /// 当前连接 ID（用于查找按连接绑定的全局规则）。
    fn connection_id(&self) -> u64;
    /// 只读访问会话慢日志规则状态。
    fn slow_log_rules(&self) -> &SessionSlowLogRules;
    /// 可变访问会话慢日志规则状态。
    fn slow_log_rules_mut(&mut self) -> &mut SessionSlowLogRules;
    /// 返回已注册的规则字段名列表。
    fn registered_rule_fields(&self) -> Vec<String>;
    /// 指定字段是否存在可用于预采集值的 setter。
    fn rule_field_has_setter(&self, field: &str) -> bool;
    /// 将指定字段的值写入 items。
    fn set_rule_field(&self, field: &str, items: &mut SlowQueryLogItems);
    /// 用阈值匹配指定字段是否满足条件。
    fn match_rule_field(
        &self,
        field: &str,
        items: &SlowQueryLogItems,
        threshold: &Threshold,
    ) -> bool;
}

/// The statement adapter supplies execution-owned fields (plan, retry timing,
/// RU/CPU details, keyspace and statement context) exactly once execution has
/// finished. Rule completion remains ordered before that copy, as in Go.
///
/// 语句适配器接口：执行结束后填充慢日志条目中由执行层拥有的字段。
pub trait SlowLogStatement {
    /// 关联的会话规则上下文类型。
    type Session: SlowLogRuleContext;

    /// 取得可变会话规则上下文。
    fn slow_log_session(&mut self) -> &mut Self::Session;
    /// 填充计划、重试耗时、RU/CPU、keyspace 等执行侧字段。
    fn fill_slow_log_items(
        &mut self,
        txn_ts: u64,
        has_more_results: bool,
        items: &mut SlowQueryLogItems,
    );
}

/// 将 src 中的条件字段名并入 dst（集合并集）。
pub fn mergeConditionFields(dst: &mut BTreeSet<String>, src: &BTreeSet<String>) {
    dst.extend(src.iter().cloned());
}

/// 在会话或全局规则哈希变化时，重新合并生效字段集合。
pub fn updateAllRuleFields(
    global_rules: &GlobalSlowLogRules,
    session: &mut impl SlowLogRuleContext,
) {
    let needs_update = {
        let state = session.slow_log_rules();
        state.need_update_effective_fields
            || state.global_raw_rules_hash != global_rules.raw_rules_hash
    };
    if !needs_update {
        return;
    }

    // 合并：会话规则 ∪ 本连接全局规则 ∪ UnsetConnID（未绑定连接的全局规则）
    let connection_id = session.connection_id() as i64;
    let mut fields = BTreeSet::new();
    if let Some(rules) = session.slow_log_rules().rules.as_ref() {
        mergeConditionFields(&mut fields, &rules.fields);
    }
    if let Some(rules) = global_rules.rules_map.get(&connection_id) {
        mergeConditionFields(&mut fields, &rules.fields);
    }
    if let Some(rules) = global_rules.rules_map.get(&UnsetConnID) {
        mergeConditionFields(&mut fields, &rules.fields);
    }

    let state = session.slow_log_rules_mut();
    state.effective_fields = fields;
    state.global_raw_rules_hash = global_rules.raw_rules_hash;
    state.need_update_effective_fields = false;
}

/// 慢查询日志条目对象池，避免频繁分配。
fn slow_query_log_items_pool() -> &'static Mutex<Vec<Box<SlowQueryLogItems>>> {
    static POOL: OnceLock<Mutex<Vec<Box<SlowQueryLogItems>>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Vec::new()))
}

/// 从池中取出或新建一个 SlowQueryLogItems。
pub fn getSlowLogItems() -> Box<SlowQueryLogItems> {
    slow_query_log_items_pool()
        .lock()
        .expect("slow-query-log item pool poisoned")
        .pop()
        .unwrap_or_else(|| Box::new(SlowQueryLogItems::default()))
}

/// 清空条目后归还对象池。
pub fn putSlowLogItems(mut items: Option<Box<SlowQueryLogItems>>) {
    let Some(mut items) = items.take() else {
        return;
    };
    *items = SlowQueryLogItems::default();
    slow_query_log_items_pool()
        .lock()
        .expect("slow-query-log item pool poisoned")
        .push(items);
}

/// Builds an item containing only fields referenced by the effective rules.
///
/// 仅填充生效规则引用到的字段，供预执行规则匹配使用。
pub fn PrepareSlowLogItemsForRules(
    global_rules: &GlobalSlowLogRules,
    session: &mut impl SlowLogRuleContext,
) -> Option<Box<SlowQueryLogItems>> {
    updateAllRuleFields(global_rules, session);
    let fields = session.slow_log_rules().effective_fields.clone();
    if fields.is_empty() {
        return None;
    }

    let mut items: Option<Box<SlowQueryLogItems>> = None;
    for field in fields {
        // Go only allocates an item after finding a registered accessor with a
        // non-nil Setter. Session-only fields such as Conn_ID therefore do not
        // manufacture an otherwise empty item.
        if !session.rule_field_has_setter(&field) {
            continue;
        }
        let item = items.get_or_insert_with(getSlowLogItems);
        session.set_rule_field(&field, item);
    }
    items
}

/// Rules are OR-ed; conditions inside one rule are AND-ed.
///
/// 规则间 OR，单规则内条件 AND；任一规则全部条件命中即返回 true。
pub fn Match(
    session: &impl SlowLogRuleContext,
    items: &SlowQueryLogItems,
    rules: Option<&SlowLogRules>,
) -> bool {
    let Some(rules) = rules else {
        return false;
    };
    rules.rules.iter().any(|rule| {
        rule.conditions.iter().all(|condition| {
            session.match_rule_field(
                &condition.field.to_ascii_lowercase(),
                items,
                &condition.threshold,
            )
        })
    })
}

/// Go-compatible Match entry that evaluates accessors against SessionVars.
///
/// 兼容 Go 的匹配入口：通过 SlowLogRuleFieldAccessors 在 SessionVars 上求值。
pub fn MatchSessionVars(
    se_vars: &SessionVars,
    items: &SlowQueryLogItems,
    rules: Option<&SlowLogRules>,
) -> bool {
    let Some(rules) = rules else {
        return false;
    };
    rules.rules.iter().any(|rule| {
        rule.conditions.iter().all(|condition| {
            let field = condition.field.to_ascii_lowercase();
            SlowLogRuleFieldAccessors
                .get(&field)
                .map(|accessor| (accessor.Match)(Some(se_vars), items, &condition.threshold))
                .unwrap_or(false)
        })
    })
}

/// 判断是否应写入慢日志：先匹配会话规则，再匹配连接级与未绑定连接的全局规则。
pub fn ShouldWriteSlowLog(
    global_rules: &GlobalSlowLogRules,
    session: &impl SlowLogRuleContext,
    items: &SlowQueryLogItems,
) -> bool {
    if Match(session, items, session.slow_log_rules().rules.as_ref()) {
        return true;
    }
    if let Some(rules) = global_rules
        .rules_map
        .get(&(session.connection_id() as i64))
    {
        if Match(session, items, Some(rules)) {
            return true;
        }
    }
    global_rules
        .rules_map
        .get(&UnsetConnID)
        .is_some_and(|rules| Match(session, items, Some(rules)))
}

/// Fills fields that were not needed during pre-execution rule matching.
///
/// 补全预匹配时未涉及的已注册字段，使最终日志条目完整。
pub fn CompleteSlowLogItemsForRules(
    session: &impl SlowLogRuleContext,
    items: &mut SlowQueryLogItems,
) {
    let effective_fields = &session.slow_log_rules().effective_fields;
    for field in session.registered_rule_fields() {
        if !effective_fields.contains(&field) {
            session.set_rule_field(&field, items);
        }
    }
}

/// 先补全规则字段，再由语句适配器填充执行侧字段（txn_ts、结果集是否还有更多行等）。
pub fn SetSlowLogItems<S: SlowLogStatement>(
    statement: &mut S,
    txn_ts: u64,
    has_more_results: bool,
    items: Option<&mut SlowQueryLogItems>,
) {
    let Some(items) = items else {
        return;
    };
    CompleteSlowLogItemsForRules(statement.slow_log_session(), items);
    statement.fill_slow_log_items(txn_ts, has_more_results, items);
}

/// Writes a fully populated slow-log item when the statement hint explicitly
/// requests it. The caller still owns item construction, matching Go's
/// `logSlowQuery` path; this function only formats and emits the real item.
///
/// 当 hint 强制要求时，格式化并写出完整慢日志条目；调用方仍负责构造 items。
pub fn WriteForcedSlowLogTo(
    logger: &Logger,
    force_by_hint: bool,
    session_vars: &SessionVars,
    items: &SlowQueryLogItems,
) -> bool {
    if !force_by_hint {
        return false;
    }
    logger.warn(session_vars.SlowLogFormat(items));
    true
}

/// 使用全局慢查询 logger 写出强制慢日志（WriteForcedSlowLogTo 的便捷封装）。
pub fn WriteForcedSlowLog(
    force_by_hint: bool,
    session_vars: &SessionVars,
    items: &SlowQueryLogItems,
) -> bool {
    WriteForcedSlowLogTo(
        &log::slow_query_logger(),
        force_by_hint,
        session_vars,
        items,
    )
}

/// Go's executor init registers the plan-owned slow-log field before rules parse.
fn plan_digest_accessor() -> astersql_sessionctx_variable::slow_log::SlowLogFieldAccessor {
    use astersql_sessionctx_variable::slow_log::{MatchEqual, ParseString, SlowLogFieldAccessor};
    SlowLogFieldAccessor {
        Parse: ParseString,
        Setter: Some(std::sync::Arc::new(|_, vars, items| {
            if let Some(vars) = vars {
                items.PlanDigest = session_plan_digest(&vars.StmtCtx).String().to_owned();
            }
        })),
        Match: std::sync::Arc::new(|_, items, threshold| {
            MatchEqual(threshold, &items.PlanDigest.to_lowercase())
        }),
    }
}

/// Resolve the session-context plan and cache its normalized digest, like GetPlanDigest.
fn session_plan_digest(
    stmt: &astersql_sessionctx_stmtctx::StatementContext,
) -> astersql_parser::digester_impl::Digest {
    use astersql_parser::digester_impl::NewDigest;
    use astersql_planner_core::{
        FlatPhysicalPlan, FlattenPhysicalPlan, NormalizeFlatPlan, PlanNode,
    };
    let (normalized, digest) = stmt.GetPlanDigest();
    if !normalized.is_empty() {
        if let Some(digest) = digest {
            return digest;
        }
    }
    let flat = stmt.GetPlan().and_then(|plan| {
        if let Some(flat) = stmt.GetFlatPlan() {
            return Some(
                flat.downcast::<FlatPhysicalPlan>()
                    .expect("flat physical plan"),
            );
        }
        let plan = plan.downcast_ref::<PlanNode>().expect("physical plan node");
        let flat = std::sync::Arc::new(FlattenPhysicalPlan(Some(plan), false)?);
        stmt.SetFlatPlan(Some(flat.clone()));
        Some(flat)
    });
    let (normalized, digest) = match flat {
        Some(flat)
            if flat
                .GetSelectPlan()
                .0
                .first()
                .is_some_and(|op| op.Origin.IsPhysical()) =>
        {
            let (normalized, digest) = NormalizeFlatPlan(&flat);
            (normalized, NewDigest(digest.Bytes().to_vec()))
        }
        _ => (String::new(), NewDigest(Vec::new())),
    };
    stmt.SetPlanDigest(normalized, Some(digest.clone()));
    digest
}

// Same package-startup convention as parser/types; no dependency cycle or mutable map.
#[cfg(any(target_family = "unix", target_os = "windows"))]
#[used]
#[cfg_attr(
    all(target_family = "unix", not(target_vendor = "apple")),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".CRT$XCU"))]
static SLOW_LOG_PACKAGE_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        astersql_sessionctx_variable::slow_log::RegisterPlanDigestAccessor(plan_digest_accessor);
    }
    initialize
};
