// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 非相关标量子查询（Scalar Subquery）表达式与求值上下文。
//
// 优化阶段把不引用外层列的标量子查询物化为常量：通过执行器钩子
// `EvalSubqueryFirstRow` 取首行，结果缓存在会话级上下文中。
// `ScalarSubQueryExpr` 作为表达式占位符，首次 Eval 时触发求值并缓存 Datum。
//

use std::any::Any;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, OnceLock};

use base_dependency as base;
use expression_dependency as expression;
use infoschema_dependency as infoschema;

use crate::context;

type Error = expression::errors::Error;

/// 执行器初始化后安装的钩子：对物理计划求值并返回首行各列 Datum。
/// Datum 是表达式层的通用值容器。
/// Executor hook installed after the executor package has initialized.
pub type EvalSubqueryFirstRowFn = fn(
    &dyn context::Context,
    &dyn base::PhysicalPlan,
    &dyn infoschema::InfoSchema,
    &dyn base::PlanContext,
) -> Result<Option<Vec<expression::types::Datum>>, Error>;

/// 已安装的求值钩子；进程内至多安装一次。
static EVAL_SUBQUERY_FIRST_ROW: OnceLock<EvalSubqueryFirstRowFn> = OnceLock::new();

/// 安装标量子查询首行求值钩子；重复安装不同函数则报错。
pub fn InstallEvalSubqueryFirstRow(evaluator: EvalSubqueryFirstRowFn) -> Result<(), Error> {
    // 已安装且地址相同则幂等成功；地址不同则拒绝。
    if let Some(installed) = EVAL_SUBQUERY_FIRST_ROW.get() {
        if std::ptr::fn_addr_eq(*installed, evaluator) {
            return Ok(());
        }
        return Err(expression::errors::New(
            "a different EvalSubqueryFirstRow evaluator is already installed",
        ));
    }
    EVAL_SUBQUERY_FIRST_ROW
        .set(evaluator)
        .map_err(|_| expression::errors::New("EvalSubqueryFirstRow evaluator is already installed"))
}

/// 调用已安装钩子求值；未安装时返回错误。
pub(crate) fn eval_subquery_first_row(
    ctx: &dyn context::Context,
    plan: &dyn base::PhysicalPlan,
    info_schema: &dyn infoschema::InfoSchema,
    plan_context: &dyn base::PlanContext,
) -> Result<Option<Vec<expression::types::Datum>>, Error> {
    EVAL_SUBQUERY_FIRST_ROW
        .get()
        .ok_or_else(|| expression::errors::New("EvalSubqueryFirstRow is not installed"))?(
        ctx,
        plan,
        info_schema,
        plan_context,
    )
}

/// 子查询求值状态：尚未执行，或已完成（含成功/失败结果）。
#[derive(Clone)]
enum SubqueryEvaluation {
    Pending,
    Complete(Result<Vec<expression::types::Datum>, Error>),
}

/// 持有求值一个非相关标量子查询所需的全部上下文。
///
/// Go 的 interface 值是可克隆句柄；Rust 用 `Arc` 表达相同生命周期：
/// 该上下文注册在会话上，可长于创建它的表达式改写栈帧。
/// Owns everything required to evaluate one non-correlated scalar subquery.
///
/// Go interface values are owned, cloneable handles. Rust uses `Arc` for the
/// same lifetime: this context is registered on the session and can outlive the
/// expression-rewrite stack frame that created it.
pub struct ScalarSubqueryEvalCtx {
    /// 本上下文在计划中的分配 ID。
    plan_id: i32,
    /// 查询块偏移，用于 EXPLAIN 定位。
    query_block_offset: i32,
    /// 计划上下文（分配 plan id、会话变量等）。
    plan_context: base::ContextRef,
    /// 待执行的标量子查询物理计划。
    pub scalar_sub_query: Arc<dyn base::PhysicalPlan>,
    /// 执行/优化共用的上下文。
    pub ctx: Arc<dyn context::Context>,
    /// 信息模式（InfoSchema）：当前可见的库表元数据快照。
    pub is: Arc<dyn infoschema::InfoSchema>,
    /// 子查询输出列 ID 列表，与求值结果列一一对应。
    pub output_col_ids: Vec<i64>,
    /// 惰性求值缓存；多表达式可共享同一 Arc。
    evaluation: Arc<Mutex<SubqueryEvaluation>>,
}

impl Clone for ScalarSubqueryEvalCtx {
    fn clone(&self) -> Self {
        Self {
            plan_id: self.plan_id,
            query_block_offset: self.query_block_offset,
            plan_context: Arc::clone(&self.plan_context),
            scalar_sub_query: Arc::clone(&self.scalar_sub_query),
            ctx: Arc::clone(&self.ctx),
            is: Arc::clone(&self.is),
            output_col_ids: self.output_col_ids.clone(),
            evaluation: Arc::clone(&self.evaluation),
        }
    }
}

impl ScalarSubqueryEvalCtx {
    /// 分配 plan id 并构造待求值上下文。
    pub fn New(
        plan_context: base::ContextRef,
        query_block_offset: i32,
        scalar_sub_query: Arc<dyn base::PhysicalPlan>,
        ctx: Arc<dyn context::Context>,
        info_schema: Arc<dyn infoschema::InfoSchema>,
    ) -> Self {
        let plan_id = plan_context.alloc_plan_id();
        Self {
            plan_id,
            query_block_offset,
            plan_context,
            scalar_sub_query,
            ctx,
            is: info_schema,
            output_col_ids: Vec::new(),
            evaluation: Arc::new(Mutex::new(SubqueryEvaluation::Pending)),
        }
    }

    /// 返回计划节点 ID。
    pub fn ID(&self) -> i32 {
        self.plan_id
    }

    /// 返回查询块偏移。
    pub fn QueryBlockOffset(&self) -> i32 {
        self.query_block_offset
    }

    /// 返回计划上下文引用。
    pub fn SCtx(&self) -> &base::ContextRef {
        &self.plan_context
    }

    /// 标量子查询本身无独立 Schema，恒返回 None。
    pub fn Schema(&self) -> Option<&expression::Schema> {
        None
    }

    /// EXPLAIN 输出列列表摘要。
    pub fn ExplainInfo(&self) -> String {
        let output = self
            .output_col_ids
            .iter()
            .map(|id| format!("ScalarQueryCol#{id}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("Output: {output}")
    }

    /// 按输出列 ID 取已求值 Datum；必要时先触发 selfEval。
    pub fn getColVal(&self, col_id: i64) -> Result<expression::types::Datum, Error> {
        // 确保子查询已求值成功，再按列 ID 定位结果。
        self.selfEval()?;
        let evaluation = self
            .evaluation
            .lock()
            .expect("scalar subquery lock poisoned");
        let SubqueryEvaluation::Complete(Ok(columns)) = &*evaluation else {
            unreachable!("successful selfEval must cache a successful result")
        };
        let index = self
            .output_col_ids
            .iter()
            .position(|id| *id == col_id)
            .ok_or_else(|| {
                expression::errors::New(format!(
                    "Could not found the ScalarSubQueryExpr#{col_id} in the ScalarSubquery_{}",
                    self.ID()
                ))
            })?;
        columns.get(index).cloned().ok_or_else(|| {
            expression::errors::New(format!(
                "ScalarSubquery_{} returned {} columns for {} output IDs",
                self.ID(),
                columns.len(),
                self.output_col_ids.len()
            ))
        })
    }

    /// 惰性求值：已完成则复用缓存，否则调用执行器钩子并写入状态。
    fn selfEval(&self) -> Result<(), Error> {
        let mut evaluation = self
            .evaluation
            .lock()
            .expect("scalar subquery lock poisoned");
        // 已有完整结果则直接返回成功或克隆错误。
        if let SubqueryEvaluation::Complete(result) = &*evaluation {
            return result.as_ref().map(|_| ()).map_err(Clone::clone);
        }

        // 调用钩子取首行；空结果视为空 Vec。
        let result = eval_subquery_first_row(
            self.ctx.as_ref(),
            self.scalar_sub_query.as_ref(),
            self.is.as_ref(),
            self.plan_context.as_ref(),
        )
        .map(|row| row.unwrap_or_default());
        *evaluation = SubqueryEvaluation::Complete(result.clone());
        result.map(|_| ())
    }
}

/// 单个 ScalarSubQueryExpr 的求值缓存：是否已求值、错误与值。
#[derive(Clone)]
struct ExpressionEvaluation {
    evaluated: bool,
    error: Option<Error>,
    value: expression::types::Datum,
}

/// 非相关标量子查询在优化期的表达式占位符。
/// 首次 Eval 时经 eval_ctx 取列值并缓存。
/// Expression placeholder for a non-correlated scalar subquery evaluated
/// during optimization.
pub struct ScalarSubQueryExpr {
    /// 对应 ScalarSubqueryEvalCtx.output_col_ids 中的列 ID。
    pub scalar_subquery_col_id: i64,
    /// 求值上下文；缺省构造时可为 None。
    pub eval_ctx: Option<Box<ScalarSubqueryEvalCtx>>,
    /// 本表达式级求值缓存。
    evaluation: Mutex<ExpressionEvaluation>,
    /// HashCode 字节缓存。
    hashcode: Mutex<Vec<u8>>,
    /// 内嵌 Constant，承载返回类型与部分 Expression 默认行为。
    constant: expression::Constant,
}

impl Default for ScalarSubQueryExpr {
    fn default() -> Self {
        let constant = expression::NewNull();
        Self {
            scalar_subquery_col_id: 0,
            eval_ctx: None,
            evaluation: Mutex::new(ExpressionEvaluation {
                evaluated: false,
                error: None,
                value: constant.Value.clone(),
            }),
            hashcode: Mutex::new(Vec::new()),
            constant,
        }
    }
}

impl Clone for ScalarSubQueryExpr {
    fn clone(&self) -> Self {
        Self {
            scalar_subquery_col_id: self.scalar_subquery_col_id,
            eval_ctx: self.eval_ctx.clone(),
            evaluation: Mutex::new(
                self.evaluation
                    .lock()
                    .expect("scalar expression lock poisoned")
                    .clone(),
            ),
            hashcode: Mutex::new(
                self.hashcode
                    .lock()
                    .expect("scalar expression hash lock poisoned")
                    .clone(),
            ),
            constant: self.constant.Clone(),
        }
    }
}

impl Deref for ScalarSubQueryExpr {
    type Target = expression::Constant;

    fn deref(&self) -> &Self::Target {
        &self.constant
    }
}

impl DerefMut for ScalarSubQueryExpr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.constant
    }
}

impl ScalarSubQueryExpr {
    /// 绑定列 ID 与求值上下文。
    pub fn new(scalar_subquery_col_id: i64, eval_ctx: ScalarSubqueryEvalCtx) -> Self {
        let mut expression = Self::default();
        expression.scalar_subquery_col_id = scalar_subquery_col_id;
        expression.eval_ctx = Some(Box::new(eval_ctx));
        expression
    }

    /// 惰性求值本表达式：成功则缓存值，失败则缓存错误。
    fn selfEvaluate(&self) -> Result<(), Error> {
        let mut evaluation = self
            .evaluation
            .lock()
            .expect("scalar expression lock poisoned");
        // 已成功求值则直接返回。
        if evaluation.evaluated {
            return Ok(());
        }
        // 曾失败则重复返回同一错误。
        if let Some(error) = &evaluation.error {
            return Err(error.clone());
        }

        let result = self
            .eval_ctx
            .as_ref()
            .ok_or_else(|| expression::errors::New("scalar subquery eval context is required"))?
            .getColVal(self.scalar_subquery_col_id);
        match result {
            Ok(value) => {
                evaluation.value = value;
                evaluation.evaluated = true;
                Ok(())
            }
            Err(error) => {
                evaluation.value = expression::NewNull().Value;
                evaluation.error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// 调试/EXPLAIN 用列名字符串。
    pub fn String(&self) -> String {
        format!("ScalarQueryCol#{}", self.scalar_subquery_col_id)
    }

    /// 标量按类型 Eval* 尚未实现时的统一错误。
    fn unsupported_evaluation() -> Error {
        expression::errors::New("Evaluation methods is not implemented for ScalarSubQueryExpr")
    }

    /// 向量化求值尚未实现时的统一错误。
    fn unsupported_vectorized_evaluation() -> Error {
        expression::errors::New("ScalarSubQueryExpr doesn't implement the vec eval yet")
    }

    /// 编码 HashCode：标志字节 + 列 ID 的可比较大端表示。
    fn encoded_hash(&self) -> Vec<u8> {
        let mut hashcode = self
            .hashcode
            .lock()
            .expect("scalar expression hash lock poisoned");
        // 首次计算后缓存，避免重复编码。
        if hashcode.is_empty() {
            hashcode.reserve(9);
            hashcode.push(expression::SCALAR_SUB_Q_FLAG);
            let comparable = (self.scalar_subquery_col_id as u64) ^ 0x8000_0000_0000_0000;
            hashcode.extend_from_slice(&comparable.to_be_bytes());
        }
        hashcode.clone()
    }
}

/// 声明可向量化，但各 VecEval* 暂返回未实现错误。
impl expression::VecExpr for ScalarSubQueryExpr {
    fn Vectorized(&self) -> bool {
        true
    }

    fn VecEvalInt(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
    fn VecEvalReal(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
    fn VecEvalString(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
    fn VecEvalDecimal(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
    fn VecEvalTime(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
    fn VecEvalDuration(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
    fn VecEvalJSON(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
    fn VecEvalVectorFloat32(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::chunk::Chunk,
        _: &mut expression::chunk::Column,
    ) -> Result<(), Error> {
        Err(Self::unsupported_vectorized_evaluation())
    }
}

/// 排序规则信息委托给内嵌 Constant。
impl expression::CollationInfo for ScalarSubQueryExpr {
    fn HasCoercibility(&self) -> bool {
        self.constant.HasCoercibility()
    }
    fn Coercibility(&self) -> expression::Coercibility {
        self.constant.Coercibility()
    }
    fn SetCoercibility(&self, value: expression::Coercibility) {
        self.constant.SetCoercibility(value)
    }
    fn Repertoire(&self) -> expression::Repertoire {
        self.constant.Repertoire()
    }
    fn SetRepertoire(&mut self, value: expression::Repertoire) {
        self.constant.SetRepertoire(value)
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.constant.CharsetAndCollation()
    }
    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.constant.SetCharsetAndCollation(charset, collation)
    }
    fn IsExplicitCharset(&self) -> bool {
        self.constant.IsExplicitCharset()
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.constant.SetExplicitCharset(explicit)
    }
}

/// 标量子查询依赖会话态，不可跨会话共享。
impl expression::SafeToShareAcrossSession for ScalarSubQueryExpr {
    fn SafeToShareAcrossSession(&self) -> bool {
        false
    }
}

/// 带参数上下文的字符串化，忽略参数直接返回列名。
impl expression::StringerWithCtx for ScalarSubQueryExpr {
    fn StringWithCtx(&self, _: Option<&dyn expression::ParamValues>, _: &str) -> String {
        self.String()
    }
}

/// Hash64：标志字节 + 列 ID。
impl expression::base::Hash64 for ScalarSubQueryExpr {
    fn Hash64(&self, hasher: &mut dyn expression::base::Hasher) {
        hasher.HashByte(expression::SCALAR_SUB_Q_FLAG);
        hasher.HashInt64(self.scalar_subquery_col_id);
    }
}

/// 相等性仅比较 scalar_subquery_col_id。
impl expression::base::Equals for ScalarSubQueryExpr {
    fn Equals(&self, other: &dyn Any) -> bool {
        other
            .downcast_ref::<Self>()
            .is_some_and(|other| self.scalar_subquery_col_id == other.scalar_subquery_col_id)
    }
}

/// Expression 实现：Eval 触发惰性求值；多数按类型 Eval* 未实现。
impl expression::Expression for ScalarSubQueryExpr {
    fn Traverse(&self, _: &dyn expression::TraverseAction) -> expression::ExprBox {
        Box::new(self.clone())
    }

    /// 求值并返回缓存的 Datum。
    fn Eval(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<expression::types::Datum, Error> {
        // 先完成惰性求值，再读出缓存值或错误。
        self.selfEvaluate()?;
        let evaluation = self
            .evaluation
            .lock()
            .expect("scalar expression lock poisoned");
        if let Some(error) = &evaluation.error {
            return Err(error.clone());
        }
        Ok(evaluation.value.clone())
    }

    fn EvalInt(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(i64, bool), Error> {
        Err(Self::unsupported_evaluation())
    }
    fn EvalReal(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(f64, bool), Error> {
        Err(Self::unsupported_evaluation())
    }
    fn EvalString(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(String, bool), Error> {
        Err(Self::unsupported_evaluation())
    }
    fn EvalDecimal(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(expression::types::MyDecimal, bool), Error> {
        Err(Self::unsupported_evaluation())
    }
    fn EvalTime(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(expression::types::Time, bool), Error> {
        Err(Self::unsupported_evaluation())
    }
    fn EvalDuration(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(expression::types::Duration, bool), Error> {
        Err(Self::unsupported_evaluation())
    }
    fn EvalJSON(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(expression::types::BinaryJSON, bool), Error> {
        Err(Self::unsupported_evaluation())
    }
    fn EvalVectorFloat32(
        &self,
        _: &dyn expression::EvalContext,
        _: expression::chunk::Row,
    ) -> Result<(expression::types::VectorFloat32, bool), Error> {
        Err(Self::unsupported_evaluation())
    }

    /// 返回类型取自内嵌 Constant.RetType。
    fn GetType(&self, _: &dyn expression::EvalContext) -> &expression::types::FieldType {
        self.constant
            .RetType
            .as_ref()
            .expect("scalar subquery return type is required")
    }
    fn GetTypeMut(&mut self) -> &mut expression::types::FieldType {
        self.constant
            .RetType
            .as_mut()
            .expect("scalar subquery return type is required")
    }
    fn CloneExpr(&self) -> expression::ExprBox {
        Box::new(self.clone())
    }
    fn Equal(&self, _: &dyn expression::EvalContext, other: &dyn expression::Expression) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self.scalar_subquery_col_id == other.scalar_subquery_col_id)
    }
    /// 非相关，恒为 false。
    fn IsCorrelated(&self) -> bool {
        false
    }
    /// 非常量级别（求值依赖执行），返回 ConstNone。
    fn ConstLevel(&self) -> expression::ConstLevel {
        expression::ConstNone
    }
    fn Decorrelate(&self, _: &expression::Schema) -> expression::ExprBox {
        Box::new(self.clone())
    }
    fn ResolveIndices(&self, _: &expression::Schema) -> Result<expression::ExprBox, Error> {
        Ok(Box::new(self.clone()))
    }
    fn resolveIndices(&mut self, _: &expression::Schema) -> Result<(), Error> {
        Ok(())
    }
    fn ResolveIndicesByVirtualExpr(
        &self,
        _: &dyn expression::EvalContext,
        _: &expression::Schema,
    ) -> (expression::ExprBox, bool) {
        (Box::new(self.clone()), false)
    }
    fn resolveIndicesByVirtualExpr(
        &mut self,
        _: &dyn expression::EvalContext,
        _: &expression::Schema,
    ) -> bool {
        false
    }
    fn RemapColumn(
        &self,
        _: &HashMap<i64, expression::Column>,
    ) -> Result<expression::ExprBox, Error> {
        Ok(Box::new(self.clone()))
    }
    fn ExplainInfo(&self, _: &dyn expression::EvalContext) -> String {
        self.String()
    }
    fn ExplainNormalizedInfo(&self) -> String {
        self.String()
    }
    fn ExplainNormalizedInfo4InList(&self) -> String {
        self.String()
    }
    fn HashCode(&self) -> Vec<u8> {
        self.encoded_hash()
    }
    fn CanonicalHashCode(&self) -> Vec<u8> {
        self.encoded_hash()
    }
    /// 已求值时按 Constant 估算内存，否则为 0。
    fn MemoryUsage(&self) -> i64 {
        let evaluation = self
            .evaluation
            .lock()
            .expect("scalar expression lock poisoned");
        if evaluation.evaluated {
            self.constant
                .clone_with_value(evaluation.value.clone())
                .MemoryUsage()
        } else {
            0
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
