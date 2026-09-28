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

// bindinfo 模块的工具函数集合。
//
// “绑定（Binding）”是 SQL 计划管理（SQL Plan Management，SPM）的核心概念：
// 把某条原始 SQL 与一条携带优化器 Hint 的绑定 SQL 关联起来，使优化器在生成
// 执行计划（即数据库对 SQL 的具体执行步骤）时强制采用指定的 Hint，从而稳定
// 查询计划。本文件提供：
// - 绑定持久化的抽象接口（`BindingStore`）与会话池抽象（`DestroyableSessionPool`）；
// - 在会话内包裹事务执行闭包的辅助函数 `callWithSCtx`（事务：一组要么全部
//   成功、要么全部回滚的操作）；
// - 绑定 SQL 的生成、从存储读取绑定、绑定使用信息（usage info）的批量落盘、
//   加锁、校验等工具函数。

use crate::{BindError, Binding, BindingTime, Result, Statement, StatusEnabled, prepareHints};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// 批量更新绑定使用信息（usage info）时每批处理的绑定条数上限。
pub const UpdateBindingUsageInfoBatchSize: usize = 100;
/// 两次将同一条绑定的“最近使用时间”写回存储之间的最小间隔（6 小时），
/// 用于限制写入频率、避免频繁刷盘。
pub const MaxWriteInterval: Duration = Duration::from_secs(6 * 60 * 60);

/// Persistence boundary used by cache and operator.  Implementations are
/// responsible for making each bulk method atomic (normally one SQL txn).
/// 绑定的持久化边界抽象，供缓存与操作器（operator）使用。
/// 实现方需保证每个批量方法都是原子的（通常包裹在一个 SQL 事务里）。
pub trait BindingStore: Send + Sync {
    /// 读取自 `since` 时间点之后有更新的所有绑定。
    fn read_bindings_since(&self, since: BindingTime) -> Result<Vec<Arc<Binding>>>;
    /// 用给定绑定集合替换存储中的对应记录。
    fn replace_bindings(&self, bindings: &[Arc<Binding>]) -> Result<()>;
    /// 将指定 SQL 摘要（digest，SQL 规范化后的哈希指纹）对应的绑定标记为已删除，
    /// 返回受影响的行数。
    fn mark_deleted(&self, sql_digests: &[String], at: BindingTime) -> Result<u64>;
    /// 修改指定绑定的状态（如 enabled/disabled），返回是否发生了变更。
    fn set_status(&self, sql_digest: &str, status: &str, at: BindingTime) -> Result<bool>;
    /// 物理清理（GC）在 `cutoff` 之前被标记删除的绑定，返回清理条数。
    fn gc_deleted_before(&self, cutoff: BindingTime) -> Result<u64>;
    /// 记录某条绑定（由 SQL 摘要与计划摘要标识）最近一次被使用的时间。
    fn save_usage(&self, sql_digest: &str, plan_digest: &str, used_at: BindingTime) -> Result<()>;
}

/// 绑定操作所需的 SQL 执行上下文：可执行 SQL 并计算计划摘要
/// （plan digest，执行计划的哈希指纹，用于唯一标识某个执行计划）。
pub trait BindingSqlContext: crate::BindingValidator {
    /// 以参数化方式执行 SQL，返回结果行集合。
    fn execute(&self, sql: &str, args: &[SqlValue]) -> Result<Vec<BindingRow>>;
    /// 在给定 schema（数据库名）下计算绑定 SQL 的计划摘要。
    fn plan_digest(&self, schema: &str, binding_sql: &str) -> Result<String>;
}

/// 支持“销毁”语义的会话池：正常结束时归还会话复用，
/// 出错时销毁会话以避免残留脏状态（如未结束的事务）。
pub trait DestroyableSessionPool: Send + Sync {
    /// 从池中获取一个会话。
    fn acquire(&self) -> Result<Box<dyn BindingSqlContext>>;
    /// 将会话归还到池中以便复用。
    fn release(&self, session: Box<dyn BindingSqlContext>);
    /// 销毁会话（不再复用），用于出错后的清理。
    fn destroy(&self, session: Box<dyn BindingSqlContext>);
}

/// 从会话池取出一个会话执行闭包 `f`，可选地包裹在悲观事务中。
///
/// 悲观事务（PESSIMISTIC）指在执行阶段就对数据加锁的事务模式，与乐观事务
/// 在提交时才检测冲突相对。`wrapTxn` 为 true 时：闭包成功则 COMMIT 提交，
/// 失败则 ROLLBACK 回滚。整体成功时会话归还池中复用，失败时销毁会话。
pub fn callWithSCtx<T, F>(sPool: &dyn DestroyableSessionPool, wrapTxn: bool, f: F) -> Result<T>
where
    F: FnOnce(&dyn BindingSqlContext) -> Result<T>,
{
    let session = sPool.acquire()?;
    if wrapTxn {
        // 开启悲观事务，后续读写在事务内进行
        if let Err(error) = session.execute("BEGIN PESSIMISTIC", &[]) {
            sPool.destroy(session);
            return Err(error);
        }
    }
    let result = f(session.as_ref());
    // 根据闭包执行结果决定提交或回滚事务
    let final_result = if wrapTxn {
        match result {
            Ok(value) => session.execute("COMMIT", &[]).map(|_| value),
            Err(error) => {
                // 回滚失败也不覆盖原始错误，仅尽力清理
                let _ = session.execute("ROLLBACK", &[]);
                Err(error)
            }
        }
    } else {
        result
    };
    // 成功则归还会话复用；失败则销毁会话，防止脏状态泄漏到池中
    if final_result.is_ok() {
        sPool.release(session);
    } else {
        sPool.destroy(session);
    }
    final_result
}

/// SQL 参数值的枚举，用于参数化执行 SQL 时传参。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SqlValue {
    /// SQL 的 NULL 值。
    Null,
    /// 字符串值。
    String(String),
    /// 有符号 64 位整数。
    I64(i64),
    /// 无符号 64 位整数。
    U64(u64),
    /// 时间值（绑定使用的时间戳类型）。
    Time(BindingTime),
}

/// 存储中一行绑定记录，字段与系统表 `mysql.bind_info` 的列一一对应。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BindingRow {
    /// 规范化后的原始 SQL。
    pub OriginalSQL: String,
    /// 携带优化器 Hint 的绑定 SQL。
    pub BindSQL: String,
    /// 创建绑定时的默认数据库。
    pub DefaultDB: String,
    /// 绑定状态（如 enabled/disabled/deleted）。
    pub Status: String,
    /// 创建时间。
    pub CreateTime: BindingTime,
    /// 最后更新时间。
    pub UpdateTime: BindingTime,
    /// 创建绑定时使用的字符集。
    pub Charset: String,
    /// 创建绑定时使用的排序规则（collation，决定字符串比较规则）。
    pub Collation: String,
    /// 绑定来源（如手动创建、自动捕获）。
    pub Source: String,
    /// 原始 SQL 的摘要（规范化 SQL 的哈希指纹）。
    pub SQLDigest: String,
    /// 绑定对应执行计划的摘要。
    pub PlanDigest: String,
}

/// 在给定 SQL 上下文中执行一条 SQL 并返回结果行。
pub fn exec(sctx: &dyn BindingSqlContext, sql: &str, args: &[SqlValue]) -> Result<Vec<BindingRow>> {
    sctx.execute(sql, args)
}

/// 执行 SQL 并返回结果行；语义上与 `exec` 相同，保留以对应 Go 版本的同名函数。
pub fn execRows(
    sctx: &dyn BindingSqlContext,
    sql: &str,
    args: &[SqlValue],
) -> Result<Vec<BindingRow>> {
    exec(sctx, sql, args)
}

/// 返回 bindinfo 模块日志记录器的名称。
pub fn bindingLogger() -> &'static str {
    "bindinfo"
}

/// 根据语句节点与计划 Hint 生成绑定 SQL。
///
/// 先按默认数据库还原语句文本，再把 Hint 以 `/*+ ... */` 注释形式插入到
/// 对应 Go 的语句类型分支，把 Hint 注入 DELETE/UPDATE/SELECT 或 INSERT 的查询部分。
pub fn GenerateBindingSQL(stmtNode: &Statement, planHint: &str, defaultDB: &str) -> String {
    // Go restores the AST and then removes a possible EXPLAIN prefix.  The Rust
    // parser does not accept every EXPLAIN DML form, so remove that wrapper first.
    let mut statement = stmtNode.clone();
    let trimmed = statement.SQL.trim_start();
    if trimmed
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("EXPLAIN "))
    {
        statement.SQL = trimmed.get(8..).unwrap_or_default().trim_start().to_owned();
    }
    let restored = crate::RestoreDBForBinding(&statement, defaultDB);
    if restored.is_empty() {
        return String::new();
    }

    let original = stmtNode.SQL.trim_start().to_ascii_uppercase();
    let original = original
        .strip_prefix("EXPLAIN ")
        .unwrap_or(original.as_str())
        .trim_start();
    let (sql, keyword, top_level) = if original.starts_with("DELETE") {
        (
            &restored[restored.find("DELETE").unwrap_or(0)..],
            "DELETE",
            false,
        )
    } else if original.starts_with("UPDATE") {
        (
            &restored[restored.find("UPDATE").unwrap_or(0)..],
            "UPDATE",
            false,
        )
    } else if original.starts_with("SELECT") {
        (
            &restored[restored.find("SELECT").unwrap_or(0)..],
            "SELECT",
            false,
        )
    } else if original.starts_with("WITH") {
        (restored.as_str(), "SELECT", true)
    } else if original.starts_with("INSERT") || original.starts_with("REPLACE") {
        let start_keyword = if original.starts_with("REPLACE") {
            "REPLACE"
        } else {
            "INSERT"
        };
        (
            &restored[restored.find(start_keyword).unwrap_or(0)..],
            "SELECT",
            true,
        )
    } else {
        return String::new();
    };

    let Some(insertion) =
        keyword_position(sql, keyword, top_level).map(|index| index + keyword.len())
    else {
        // INSERT ... VALUES has no SELECT and Go returns the restored statement unchanged.
        return sql.to_owned();
    };
    format!(
        "{} /*+ {} */{}",
        &sql[..insertion],
        planHint.trim(),
        &sql[insertion..]
    )
}

fn keyword_position(sql: &str, keyword: &str, top_level: bool) -> Option<usize> {
    let bytes = sql.as_bytes();
    let needle = keyword.as_bytes();
    let mut depth = 0usize;
    let mut index = 0usize;
    while index + needle.len() <= bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        let boundary_before = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
        let end = index + needle.len();
        let boundary_after = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if (!top_level || depth == 0)
            && boundary_before
            && boundary_after
            && &bytes[index..end] == needle
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// 从持久化存储读取自 `since` 之后有更新的绑定，用于增量加载缓存。
pub fn readBindingsFromStorage(
    store: &dyn BindingStore,
    since: BindingTime,
) -> Result<Vec<Arc<Binding>>> {
    store.read_bindings_since(since)
}

/// 将一批绑定的使用信息按固定批次大小分批写回存储，避免单次写入过大。
pub fn updateBindingUsageInfoToStorage(
    store: &dyn BindingStore,
    bindings: &[Arc<Binding>],
) -> Result<()> {
    for batch in bindings.chunks(UpdateBindingUsageInfoBatchSize) {
        updateBindingUsageInfoToStorageInternal(store, batch)?;
    }
    Ok(())
}

/// 判断某条绑定的“最近使用时间”是否需要写回存储。
///
/// 规则：从未被使用则不写；从未保存过则必须写；否则仅当最近使用时间晚于
/// 最近保存时间且间隔达到 `MaxWriteInterval` 时才写，以此限制写入频率。
pub fn shouldUpdateBinding(lastSaved: Option<BindingTime>, lastUsed: Option<BindingTime>) -> bool {
    let Some(last_used) = lastUsed else {
        return false;
    };
    let Some(last_saved) = lastSaved else {
        return true;
    };
    // Go 以当前时间到最近保存时间的间隔节流；期间只要发生过一次更新便写回。
    let interval = i64::try_from(MaxWriteInterval.as_micros()).unwrap_or(i64::MAX);
    last_used > last_saved && BindingTime::now().0.saturating_sub(last_saved.0) >= interval
}

/// 单批次内逐条写回绑定使用信息：先用 `shouldUpdateBinding` 过滤，
/// 整批写回成功后再统一更新内存中的“最近保存时间”，避免部分失败时把
/// 尚未提交的写入误标为已经持久化。
pub fn updateBindingUsageInfoToStorageInternal(
    store: &dyn BindingStore,
    bindings: &[Arc<Binding>],
) -> Result<()> {
    let to_write = bindings
        .iter()
        .filter_map(|binding| {
            let last_used = binding.UsageInfo.last_used_at();
            shouldUpdateBinding(binding.UsageInfo.last_saved_at(), last_used)
                .then(|| (binding, last_used.expect("checked above")))
        })
        .collect::<Vec<_>>();

    for (binding, used_at) in &to_write {
        saveBindingUsage(store, &binding.SQLDigest, &binding.PlanDigest, *used_at)?;
    }

    let saved_at = BindingTime::now();
    for (binding, _) in to_write {
        binding.UpdateLastSavedAt(Some(saved_at));
    }
    Ok(())
}

/// 对一组绑定在 `mysql.bind_info` 系统表中的行执行 `SELECT ... FOR UPDATE`
/// 加悲观行锁，防止并发事务同时修改这些绑定。
pub fn addLockForBinds(sctx: &dyn BindingSqlContext, bindings: &[Arc<Binding>]) -> Result<()> {
    for binding in bindings {
        sctx.execute(
            "SELECT original_sql FROM mysql.bind_info WHERE sql_digest=? FOR UPDATE",
            &[SqlValue::String(binding.SQLDigest.clone())],
        )?;
    }
    Ok(())
}

/// 记录一条绑定的最近使用时间；SQL 摘要为空视为非法输入并报错。
pub fn saveBindingUsage(
    store: &dyn BindingStore,
    sqldigest: &str,
    planDigest: &str,
    ts: BindingTime,
) -> Result<()> {
    if sqldigest.is_empty() {
        return Err(BindError("SQL digest is empty".to_owned()));
    }
    store.save_usage(sqldigest, planDigest, ts)
}

/// 由存储中的一行记录构造内存中的 `Binding` 对象。
pub fn newBindingFromStorage(row: BindingRow) -> Arc<Binding> {
    Arc::new(Binding {
        OriginalSQL: row.OriginalSQL,
        Db: row.DefaultDB.to_lowercase(),
        BindSQL: row.BindSQL,
        // 为兼容旧数据，将历史 `using` 状态转换成 `enabled`。
        Status: if row.Status == crate::StatusUsing {
            StatusEnabled.to_owned()
        } else {
            row.Status
        },
        CreateTime: row.CreateTime,
        UpdateTime: row.UpdateTime,
        Source: row.Source,
        Charset: row.Charset,
        Collation: row.Collation,
        SQLDigest: row.SQLDigest,
        PlanDigest: row.PlanDigest,
        ..Binding::default()
    })
}

/// 计算绑定 SQL 的计划摘要；计算失败时返回空字符串而非报错。
pub fn getBindingPlanDigest(
    sctx: &dyn BindingSqlContext,
    schema: &str,
    bindingSQL: &str,
) -> String {
    sctx.plan_digest(schema, bindingSQL).unwrap_or_default()
}

/// 校验绑定合法性并解析、填充其 Hint 信息（委托给 `prepareHints`）。
pub fn validateAndPrepareBinding(
    sctx: &dyn BindingSqlContext,
    binding: &mut Binding,
) -> Result<()> {
    prepareHints(sctx, binding)
}
