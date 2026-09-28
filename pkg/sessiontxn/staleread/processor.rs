// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 过期读（Stale Read）处理器：判定语句是否按历史快照读。
//
// 协调事务内复用、`AS OF TIMESTAMP`、`tx_read_ts`、`tidb_read_staleness`
// 与外部时间戳（external ts）等多条路径，求值一次后锁定读 ts 与
// InfoSchema（信息模式，指定快照下的元数据视图）。PREPARE 时可缓存
// ts 求值器供 EXECUTE 复用。

use std::sync::Arc;

use crate::{
    Context, Error, ErrorKind, Expression, InfoSchema, SessionRef, calculate_as_of_ts_expr,
    calculate_ts_with_read_staleness, get_external_timestamp, get_session_snapshot_info_schema,
};

/// 可缓存的过期读 ts 求值闭包（供 PREPARE 后每次 EXECUTE 复用）。
pub type StalenessTsEvaluator =
    Arc<dyn Fn(&Context, &SessionRef) -> Result<u64, Error> + Send + Sync>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 表引用上的可选 `AS OF TIMESTAMP` 表达式。
pub struct TableName {
    pub as_of: Option<Expression>,
}

/// 过期读处理器接口：查询结果、处理 SELECT 与预编译执行。
pub trait Processor {
    /// 当前是否已判定为过期读（ts != 0）。
    fn is_staleness(&self) -> bool;
    /// 过期读对应的快照 InfoSchema。
    fn staleness_info_schema(&self) -> Option<&InfoSchema>;
    /// 过期读读时间戳；非过期读为 0。
    fn staleness_read_ts(&self) -> u64;
    /// 供 PREPARE 缓存的 ts 求值器。
    fn staleness_ts_evaluator_for_prepare(&self) -> Option<StalenessTsEvaluator>;
    /// 处理 SELECT 表引用上的过期读判定。
    fn on_select_table(&mut self, table: &TableName) -> Result<(), Error>;
    /// 执行预编译语句时的过期读判定。
    fn on_execute_prepared_stmt(
        &mut self,
        evaluator: Option<StalenessTsEvaluator>,
    ) -> Result<(), Error>;
}

/// Processor 公共状态：求值一次后锁定 ts / InfoSchema / evaluator。
struct BaseProcessor {
    context: Context,
    session: SessionRef,
    evaluated: bool,
    ts: u64,
    evaluator: Option<StalenessTsEvaluator>,
    info_schema: Option<InfoSchema>,
}

impl BaseProcessor {
    /// 构造未求值的基类状态。
    fn new(context: Context, session: SessionRef) -> Self {
        Self {
            context,
            session,
            evaluated: false,
            ts: 0,
            evaluator: None,
            info_schema: None,
        }
    }

    /// 标记为普通读（非过期读）。
    fn set_non_stale_read(&mut self) -> Result<(), Error> {
        self.set_evaluated_values(0, None, None)
    }

    /// 以固定 ts 完成求值，并生成常量 evaluator。
    fn set_evaluated_ts(&mut self, ts: u64) -> Result<(), Error> {
        let info = get_session_snapshot_info_schema(&self.session, ts)?;
        let evaluator: StalenessTsEvaluator = Arc::new(move |_, _| Ok(ts));
        self.set_evaluated_values(ts, Some(info), Some(evaluator))
    }

    /// 以固定 ts 完成求值但不缓存 evaluator（外部 ts 路径）。
    fn set_evaluated_ts_without_evaluator(&mut self, ts: u64) -> Result<(), Error> {
        let info = get_session_snapshot_info_schema(&self.session, ts)?;
        self.set_evaluated_values(ts, Some(info), None)
    }

    /// 先运行 evaluator 得到 ts，再固化结果。
    fn set_evaluated_evaluator(&mut self, evaluator: StalenessTsEvaluator) -> Result<(), Error> {
        let ts = evaluator(&self.context, &self.session)?;
        let info = get_session_snapshot_info_schema(&self.session, ts)?;
        self.set_evaluated_values(ts, Some(info), Some(evaluator))
    }

    /// 写入求值结果；同一 Processor 只允许求值一次。
    fn set_evaluated_values(
        &mut self,
        ts: u64,
        info_schema: Option<InfoSchema>,
        evaluator: Option<StalenessTsEvaluator>,
    ) -> Result<(), Error> {
        // 重复求值视为逻辑错误（对应 Go AlreadyEvaluated）。
        if self.evaluated {
            return Err(Error::new(ErrorKind::AlreadyEvaluated, "already evaluated"));
        }
        self.ts = ts;
        self.info_schema = info_schema;
        self.evaluated = true;
        self.evaluator = evaluator;
        self.session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .statement_is_staleness = ts != 0;
        Ok(())
    }
}

/// 默认过期读 Processor：协调事务内/语句级/会话变量多条路径。
pub struct StaleReadProcessor {
    base: BaseProcessor,
    statement_ts: u64,
}

impl StaleReadProcessor {
    /// 创建绑定到会话的过期读 Processor。
    pub fn new(context: Context, session: SessionRef) -> Self {
        if let Ok(mut state) = session.lock() {
            state.begin_statement();
        }
        Self {
            base: BaseProcessor::new(context, session),
            statement_ts: 0,
        }
    }

    /// 事务内：复用过期读事务上下文，忽略外部 ts 与会话变量。
    fn evaluate_from_transaction(&mut self) -> Result<(), Error> {
        let transaction = self
            .base
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .txn_context
            .clone();
        if let Some(transaction) = transaction.filter(|transaction| transaction.is_staleness) {
            let mut info = transaction.info_schema;
            info.local_temporary_tables_attached = true;
            return self
                .base
                .set_evaluated_values(transaction.start_ts, Some(info), None);
        }
        // External TS and session variables are deliberately ignored inside a transaction.
        self.base.set_non_stale_read()
    }

    /// 非事务路径：按语句 ts / tx_read_ts / read_staleness / external_ts 优先级求值。
    fn evaluate_from_statement_or_variables(
        &mut self,
        statement_ts: u64,
        evaluator: Option<StalenessTsEvaluator>,
    ) -> Result<(), Error> {
        let transaction_read_ts = self
            .base
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .use_txn_read_ts();
        // AS OF 与 SET TRANSACTION AS OF 不能同时生效。
        if transaction_read_ts > 0 && statement_ts > 0 {
            return Err(Error::as_of(
                "can't use select as of while already set transaction as of",
            ));
        }
        // 语句级 AS OF 优先。
        if statement_ts > 0 {
            self.statement_ts = statement_ts;
            if let Some(evaluator) = evaluator {
                let info = get_session_snapshot_info_schema(&self.base.session, statement_ts)?;
                return self
                    .base
                    .set_evaluated_values(statement_ts, Some(info), Some(evaluator));
            }
            return self.base.set_evaluated_ts(statement_ts);
        }
        // 其次使用事务级读 ts。
        if transaction_read_ts > 0 {
            return self.base.set_evaluated_ts(transaction_read_ts);
        }
        let read_staleness = self
            .base
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .read_staleness_millis;
        // 再次使用 tidb_read_staleness（相对偏移，需可复用 evaluator）。
        if read_staleness != 0 {
            let evaluator: StalenessTsEvaluator = Arc::new(move |_, session| {
                calculate_ts_with_read_staleness(session, read_staleness)
            });
            return self.base.set_evaluated_evaluator(evaluator);
        }
        let use_external = {
            let session = self
                .base
                .session
                .lock()
                .map_err(|_| Error::backend("session lock poisoned"))?;
            session.enable_external_ts_read && !session.restricted_sql
        };
        // 最后尝试外部时间戳读；内部 SQL 已在上面排除。
        if use_external {
            let ts = get_external_timestamp(&self.base.session)
                .map_err(|error| Error::as_of(error.message))?;
            if ts > 0 {
                return self.base.set_evaluated_ts_without_evaluator(ts);
            }
        }
        self.base.set_non_stale_read()
    }
}

impl Processor for StaleReadProcessor {
    fn is_staleness(&self) -> bool {
        self.base.ts != 0
    }

    /// 过期读对应的快照 InfoSchema。
    fn staleness_info_schema(&self) -> Option<&InfoSchema> {
        self.base.info_schema.as_ref()
    }

    fn staleness_read_ts(&self) -> u64 {
        self.base.ts
    }

    /// 供 PREPARE 缓存的 ts 求值器。
    fn staleness_ts_evaluator_for_prepare(&self) -> Option<StalenessTsEvaluator> {
        self.base.evaluator.clone()
    }

    /// 处理 SELECT 表引用上的过期读判定。
    fn on_select_table(&mut self, table: &TableName) -> Result<(), Error> {
        let in_transaction = self
            .base
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .in_txn;
        // 活跃事务内禁止新的 AS OF；已求值则直接返回。
        if in_transaction {
            if table.as_of.is_some() {
                return Err(Error::as_of("as of timestamp can't be set in transaction."));
            }
            return if self.base.evaluated {
                Ok(())
            } else {
                self.evaluate_from_transaction()
            };
        }

        // 无 AS OF 时 evaluator 返回 0，交由会话变量路径继续判定。
        let evaluator: StalenessTsEvaluator = if let Some(expression) = table.as_of.clone() {
            Arc::new(move |_, session| parse_and_validate_as_of(session, Some(&expression)))
        } else {
            Arc::new(|_, _| Ok(0))
        };
        let statement_ts = evaluator(&self.base.context, &self.base.session)?;
        // UNION 等多表场景：允许重复相同 AS OF，拒绝冲突时间。
        if self.base.evaluated {
            if self.statement_ts != statement_ts {
                return Err(Error::as_of("can not set different time in the as of"));
            }
            return Ok(());
        }
        self.evaluate_from_statement_or_variables(statement_ts, Some(evaluator))
    }

    /// 执行预编译语句时的过期读判定。
    fn on_execute_prepared_stmt(
        &mut self,
        evaluator: Option<StalenessTsEvaluator>,
    ) -> Result<(), Error> {
        if self.base.evaluated {
            return Err(Error::new(ErrorKind::AlreadyEvaluated, "already evaluated"));
        }
        let in_transaction = self
            .base
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .in_txn;
        if in_transaction {
            if evaluator.is_some() {
                return Err(Error::as_of("as of timestamp can't be set in transaction."));
            }
            return self.evaluate_from_transaction();
        }
        let statement_ts = match &evaluator {
            Some(evaluator) => evaluator(&self.base.context, &self.base.session)?,
            None => 0,
        };
        // Prepared evaluators are evaluated once per execution and never overwrite the cached evaluator.
        self.evaluate_from_statement_or_variables(statement_ts, None)
    }
}

/// 解析可选 AS OF 表达式并校验快照读 ts；无表达式返回 0。
pub fn parse_and_validate_as_of(
    session: &SessionRef,
    expression: Option<&Expression>,
) -> Result<u64, Error> {
    let Some(expression) = expression else {
        return Ok(0);
    };
    let ts = calculate_as_of_ts_expr(session, expression)?;
    let backend = Arc::clone(
        &session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .backend,
    );
    backend.validate_snapshot_read_ts(ts)?;
    Ok(ts)
}
