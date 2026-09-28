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

// 预处理语句（Prepared Statement）执行器：PREPARE / EXECUTE / DEALLOCATE。
//
// 对应 Go prepared executor：`PrepareExec` 解析 SQL、生成可缓存计划并登记语句 ID；
// `ExecuteExec` 按已准备计划构建底层执行器；`DeallocateExec` 注销语句并可选清理计划缓存。

#![allow(non_snake_case)]

/// PREPARE 生成结果：可登记的预处理语句、执行计划与参数个数。
pub struct GeneratedPreparedStatement<S, P> {
    pub statement: S,
    pub plan: P,
    pub parameter_count: usize,
}

/// PREPARE 所需后端能力：解析 SQL、生成计划、登记语句、结果字段与 TopSQL 等。
pub trait PrepareBackend {
    type Context;
    type Statement;
    type PreparedStatement;
    type Plan;
    type ResultField;
    type Error;

    fn prepared_statement_exists(&self, id: u32) -> bool;
    fn warning_count(&self) -> usize;
    fn parse_sql<C>(
        &mut self,
        ctx: C,
        sql: &str,
        reset_protocol_context: bool,
    ) -> Result<Vec<Self::Statement>, Self::Error>;
    fn in_restricted_sql(&self) -> bool;
    fn append_statement_error(&mut self, error: &Self::Error);
    fn retain_warnings_from(&mut self, index: usize);
    fn syntax_error(&self, error: Self::Error) -> Self::Error;
    fn prepare_multiple_statements_error(&self) -> Self::Error;
    fn reset_context_of_statement(
        &mut self,
        statement: &Self::Statement,
    ) -> Result<(), Self::Error>;
    fn generate_plan_cache_statement<C>(
        &mut self,
        ctx: C,
        statement: Self::Statement,
    ) -> Result<GeneratedPreparedStatement<Self::PreparedStatement, Self::Plan>, Self::Error>;
    fn top_profiling_enabled(&self) -> bool;
    fn register_top_sql(&mut self, statement: &Self::PreparedStatement);
    fn reset_plan_identifiers(&mut self);
    fn is_no_result_plan(&self, plan: &Self::Plan) -> bool;
    fn result_fields(&self, plan: &Self::Plan) -> Vec<Self::ResultField>;
    fn next_prepared_statement_id(&mut self) -> u32;
    fn bind_prepared_statement_name(&mut self, name: String, id: u32);
    fn add_prepared_statement(
        &mut self,
        id: u32,
        statement: &Self::PreparedStatement,
    ) -> Result<(), Self::Error>;
}

/// PREPARE 执行器：持有 SQL 文本，产出语句 ID、参数个数与结果字段描述。
pub struct PrepareExec<B: PrepareBackend> {
    pub backend: B,
    pub name: String,
    pub sql_text: String,
    pub id: u32,
    pub parameter_count: usize,
    pub fields: Vec<B::ResultField>,
    pub statement: Option<B::PreparedStatement>,
    pub need_reset: bool,
}

/// 构造 PrepareExec；默认 need_reset=true，表示解析后需重置语句上下文。
pub fn NewPrepareExec<B: PrepareBackend>(backend: B, sql_text: String) -> PrepareExec<B> {
    PrepareExec {
        backend,
        name: String::new(),
        sql_text,
        id: 0,
        parameter_count: 0,
        fields: Vec::new(),
        statement: None,
        need_reset: true,
    }
}

impl<B: PrepareBackend> PrepareExec<B> {
    /// 执行一次 PREPARE：幂等（已存在同 ID 则直接返回）；否则解析、生成计划并登记。
    pub fn Next<C: Clone>(&mut self, ctx: C) -> Result<(), B::Error> {
        // 已分配且会话中仍存在该预处理语句时，视为幂等完成。
        if self.id != 0 && self.backend.prepared_statement_exists(self.id) {
            return Ok(());
        }

        // 记录解析前警告数，失败且 need_reset 时用于裁剪新增警告。
        let warning_count_before_parse = self.backend.warning_count();
        let statements = match self
            .backend
            .parse_sql(ctx.clone(), &self.sql_text, self.need_reset)
        {
            Ok(statements) => statements,
            Err(error) => {
                if !self.backend.in_restricted_sql() {
                    self.backend.append_statement_error(&error);
                }
                if self.need_reset {
                    self.backend
                        .retain_warnings_from(warning_count_before_parse);
                }
                return Err(self.backend.syntax_error(error));
            }
        };
        // PREPARE 仅允许单条语句。
        if statements.len() != 1 {
            return Err(self.backend.prepare_multiple_statements_error());
        }
        let statement = statements.into_iter().next().unwrap();
        if self.need_reset {
            self.backend.reset_context_of_statement(&statement)?;
        }

        // 生成可进入计划缓存（Plan Cache）的预处理语句与计划。
        let generated = self.backend.generate_plan_cache_statement(ctx, statement)?;
        if self.backend.top_profiling_enabled() {
            self.backend.register_top_sql(&generated.statement);
        }
        self.backend.reset_plan_identifiers();
        if !self.backend.is_no_result_plan(&generated.plan) {
            self.fields = self.backend.result_fields(&generated.plan);
        }
        // 分配新的预处理语句 ID，并按名称绑定（若 name 非空）。
        if self.id == 0 {
            self.id = self.backend.next_prepared_statement_id();
        }
        if !self.name.is_empty() {
            self.backend
                .bind_prepared_statement_name(self.name.clone(), self.id);
        }

        self.parameter_count = generated.parameter_count;
        self.statement = Some(generated.statement);
        self.backend
            .add_prepared_statement(self.id, self.statement.as_ref().unwrap())?;
        Ok(())
    }
}

/// 将已准备计划构建为可运行执行器，并决定是否降低调度优先级。
pub trait ExecuteBuilder<P> {
    type Executor;
    type Error;

    fn build(&mut self, plan: &P) -> Result<Self::Executor, Self::Error>;
    fn no_priority(&self) -> bool;
    fn need_lower_priority(&self, plan: &P) -> bool;
}

/// EXECUTE 执行器：绑定 USING 变量后，委托给由计划构建出的 statement_executor。
pub struct ExecuteExec<P, E, V, S> {
    pub name: String,
    pub using_vars: Vec<V>,
    pub statement_executor: Option<E>,
    pub statement: S,
    pub plan: P,
    pub lower_priority: bool,
}

impl<P, E, V, S> ExecuteExec<P, E, V, S> {
    /// EXECUTE delegates all work to the executor built from the prepared plan.
    /// EXECUTE 将实际执行委托给由已准备计划构建出的执行器。
    pub fn Next<C, Q>(&mut self, _ctx: C, _chunk: Q) -> Result<(), std::convert::Infallible> {
        Ok(())
    }

    /// 用 builder 从 plan 构建执行器；在 no_priority 时根据计划决定是否降优先级。
    pub fn Build<B>(&mut self, builder: &mut B) -> Result<(), B::Error>
    where
        B: ExecuteBuilder<P, Executor = E>,
    {
        self.statement_executor = Some(builder.build(&self.plan)?);
        if builder.no_priority() {
            self.lower_priority = builder.need_lower_priority(&self.plan);
        }
        Ok(())
    }
}

/// DEALLOCATE 所需后端：按名查 ID、移除登记，并按开关清理计划缓存条目。
pub trait DeallocateBackend {
    type PreparedStatement;
    type CacheKey;
    type Error;

    fn prepared_statement_id(&self, name: &str) -> Option<u32>;
    fn prepared_statement(&self, id: u32) -> Option<Self::PreparedStatement>;
    fn statement_not_found(&self) -> Self::Error;
    fn invalid_plan_cache_statement(&self) -> Self::Error;
    fn remove_prepared_statement_name(&mut self, name: &str);
    fn prepared_plan_cache_enabled(&self) -> bool;
    fn plan_cache_key(
        &self,
        statement: &Self::PreparedStatement,
    ) -> Result<Self::CacheKey, Self::Error>;
    fn keep_plan_cache_on_close(&self) -> bool;
    fn delete_plan_cache_entry(&mut self, key: Self::CacheKey);
    fn remove_prepared_statement(&mut self, id: u32);
}

/// DEALLOCATE 执行器：按语句名注销预处理语句。
pub struct DeallocateExec<B: DeallocateBackend> {
    pub backend: B,
    pub name: String,
}

impl<B: DeallocateBackend> DeallocateExec<B> {
    /// 按名查找语句 ID，可选删除计划缓存条目后移除名称与语句登记。
    pub fn Next<C, Q>(&mut self, _ctx: C, _chunk: Q) -> Result<(), B::Error> {
        let id = self
            .backend
            .prepared_statement_id(&self.name)
            .ok_or_else(|| self.backend.statement_not_found())?;

        // Go 路径始终先校验登记对象是 PlanCacheStmt，随后删除名称映射；
        // 缓存开关只决定是否生成并删除 cache key。
        let statement = self
            .backend
            .prepared_statement(id)
            .ok_or_else(|| self.backend.invalid_plan_cache_statement())?;

        self.backend.remove_prepared_statement_name(&self.name);
        let cache_key = if self.backend.prepared_plan_cache_enabled() {
            Some(self.backend.plan_cache_key(&statement)?)
        } else {
            None
        };

        if let Some(cache_key) = cache_key {
            if !self.backend.keep_plan_cache_on_close() {
                self.backend.delete_plan_cache_entry(cache_key);
            }
        }
        self.backend.remove_prepared_statement(id);
        Ok(())
    }
}
