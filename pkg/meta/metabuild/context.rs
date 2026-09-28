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

// 元数据构建上下文（metabuild::Context）：构造 TableInfo / IndexInfo 时携带的表达式与会话选项。
//
// 对应 Go `pkg/meta/metabuild`：通过 Option 函数链式覆盖默认值（如 SQL mode、聚簇索引、
// 预分裂 Region 数等）。InfoSchema 表示库表元数据目录的只读视图。

use std::convert::Infallible;
use std::sync::Arc;

use crate::{contextutil, exprctx, exprstatic, infoschemactx, mysql, vardef};

/// Shared expression-context interface value, corresponding to Go's interface copy semantics.
/// 共享表达式上下文（ExprContext）引用，对应 Go 接口值的拷贝语义。
pub type ExprContextRef = Arc<dyn exprctx::ExprContext>;

/// Shared metadata-only InfoSchema interface value.
///
/// The migrated InfoSchema trait exposes Go's context and error types as associated types, so
/// metabuild carries those types without fixing a concrete InfoSchema implementation.
/// 仅元数据 InfoSchema 的共享引用；关联类型保留 Go 的 Context/Error，避免绑死具体实现。
pub type InfoSchemaRef<C, E> = Arc<dyn infoschemactx::MetaOnlyInfoSchema<Context = C, Error = E>>;

/// Option mutates a metadata-building context.
/// 元数据构建上下文的可选项：应用到 Context 上覆盖默认字段。
pub trait Option<C: ?Sized + 'static, E: 'static> {
    fn apply_ctx(&self, ctx: &mut Context<C, E>);
}

/// 将闭包包装为 Option 的内部类型。
struct FuncCtxOption<C: ?Sized + 'static, E: 'static> {
    f: Box<dyn Fn(&mut Context<C, E>)>,
}

impl<C: ?Sized + 'static, E: 'static> Option<C, E> for FuncCtxOption<C, E> {
    fn apply_ctx(&self, ctx: &mut Context<C, E>) {
        (self.f)(ctx);
    }
}

/// 由闭包构造 `Box<dyn Option>`。
fn func_opt<C, E, F>(f: F) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
    F: Fn(&mut Context<C, E>) + 'static,
{
    Box::new(FuncCtxOption { f: Box::new(f) })
}

/// Sets the expression context. Arc makes nil unrepresentable while retaining shared ownership.
/// 设置表达式上下文；Arc 保证非空同时保留共享所有权。
pub fn WithExprCtx<C, E>(expr_ctx: ExprContextRef) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.expr_ctx = Some(Arc::clone(&expr_ctx)))
}

/// 是否允许在生成列上使用 AUTO_INCREMENT。
pub fn WithEnableAutoIncrementInGenerated<C, E>(enable: bool) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.enable_auto_increment_in_generated = enable)
}

/// 是否强制表必须有主键。
pub fn WithPrimaryKeyRequired<C, E>(required: bool) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.primary_key_required = required)
}

/// 聚簇索引（Clustered Index）定义模式：主键是否作为聚簇键组织行数据。
pub fn WithClusteredIndexDefMode<C, E>(mode: vardef::ClusteredIndexDefMode) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.clustered_index_def_mode = mode)
}

/// 行 ID 分片位数（shard_row_id_bits），用于打散热点写入。
pub fn WithShardRowIDBits<C, E>(bits: u64) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.shard_row_id_bits = bits)
}

/// 建表时预分裂的 Region（数据分片）数量。
pub fn WithPreSplitRegions<C, E>(regions: u64) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.pre_split_regions = regions)
}

/// 是否抑制索引过长错误（转为 warning）。
pub fn WithSuppressTooLongIndexErr<C, E>(suppress: bool) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.suppress_too_long_index_err = suppress)
}

/// Sets or clears the InfoSchema, preserving Go's ability to pass a nil interface.
/// 设置或清空 InfoSchema，保留 Go 传入 nil 接口的能力。
pub fn WithInfoSchema<C, E>(
    schema: std::option::Option<InfoSchemaRef<C, E>>,
) -> Box<dyn Option<C, E>>
where
    C: ?Sized + 'static,
    E: 'static,
{
    func_opt(move |ctx| ctx.info_schema = schema.clone())
}

/// Context carries the inputs used to build TableInfo, IndexInfo, and related metadata.
/// 构建表/索引元数据时使用的上下文：表达式环境与各类会话默认值。
pub struct Context<C: ?Sized + 'static = (), E: 'static = Infallible> {
    /// 表达式求值上下文；NewContext 保证最终非空。
    expr_ctx: std::option::Option<ExprContextRef>,
    /// 生成列是否允许 AUTO_INCREMENT。
    enable_auto_increment_in_generated: bool,
    /// 是否要求主键。
    primary_key_required: bool,
    /// 聚簇索引定义模式。
    clustered_index_def_mode: vardef::ClusteredIndexDefMode,
    /// 行 ID 分片位数。
    shard_row_id_bits: u64,
    /// 预分裂 Region 数。
    pre_split_regions: u64,
    /// 是否抑制过长索引错误。
    suppress_too_long_index_err: bool,
    /// 可选 InfoSchema（元数据目录）。
    info_schema: std::option::Option<InfoSchemaRef<C, E>>,
}

/// Creates a context with vardef defaults, applying options from left to right.
/// 用 vardef 默认值创建上下文，再按从左到右顺序应用 Option。
pub fn NewContext<C, E>(opts: Vec<Box<dyn Option<C, E>>>) -> Context<C, E>
where
    C: ?Sized + 'static,
    E: 'static,
{
    let mut ctx = Context {
        expr_ctx: None,
        enable_auto_increment_in_generated: vardef::DefTiDBEnableAutoIncrementInGenerated,
        primary_key_required: false,
        clustered_index_def_mode: vardef::DefTiDBEnableClusteredIndex,
        shard_row_id_bits: u64::try_from(vardef::DefShardRowIDBits)
            .expect("DefShardRowIDBits must be non-negative"),
        pre_split_regions: u64::try_from(vardef::DefPreSplitRegions)
            .expect("DefPreSplitRegions must be non-negative"),
        suppress_too_long_index_err: false,
        info_schema: None,
    };

    // 按声明顺序覆盖；后出现的 Option 覆盖先前同字段设置。
    for option in opts {
        option.apply_ctx(&mut ctx);
    }

    // 未显式传入表达式上下文时，使用空选项的静态默认 ExprContext。
    if ctx.expr_ctx.is_none() {
        ctx.expr_ctx = Some(Arc::new(exprstatic::NewExprContext(Vec::new())));
    }
    ctx
}

/// Creates a metadata-building context with SQL mode disabled.
/// 创建 SQL mode 为 ModeNone 的非严格元数据构建上下文。
pub fn NewNonStrictContext() -> Context<(), Infallible> {
    let eval_ctx = Arc::new(exprstatic::NewEvalContext(vec![exprstatic::WithSQLMode(
        mysql::ModeNone,
    )]));
    let expr_ctx: ExprContextRef =
        Arc::new(exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(
            eval_ctx,
        )]));
    NewContext::<(), Infallible>(vec![WithExprCtx(expr_ctx)])
}

impl<C, E> Context<C, E>
where
    C: ?Sized + 'static,
    E: 'static,
{
    /// 返回表达式上下文；NewContext 保证已初始化。
    pub fn GetExprCtx(&self) -> &dyn exprctx::ExprContext {
        self.expr_ctx
            .as_deref()
            .expect("NewContext must initialize expr_ctx")
    }

    /// 取 UTF8MB4 默认排序规则（collation）。
    pub fn GetDefaultCollationForUTF8MB4(&self) -> String {
        self.GetExprCtx().GetDefaultCollationForUTF8MB4()
    }

    /// 取当前 SQL mode（决定严格/宽松等行为）。
    pub fn GetSQLMode(&self) -> mysql::SQLMode {
        self.GetExprCtx().GetEvalCtx().SQLMode()
    }

    /// 向求值上下文追加 warning。
    pub fn AppendWarning(&self, err: contextutil::errors::SharedError) {
        self.GetExprCtx().GetEvalCtx().AppendWarning(err);
    }

    /// 向求值上下文追加 note。
    pub fn AppendNote(&self, note: contextutil::errors::SharedError) {
        self.GetExprCtx().GetEvalCtx().AppendNote(note);
    }

    /// 生成列是否允许 AUTO_INCREMENT。
    pub fn EnableAutoIncrementInGenerated(&self) -> bool {
        self.enable_auto_increment_in_generated
    }

    /// 是否强制要求主键。
    pub fn PrimaryKeyRequired(&self) -> bool {
        self.primary_key_required
    }

    /// 聚簇索引定义模式。
    pub fn GetClusteredIndexDefMode(&self) -> vardef::ClusteredIndexDefMode {
        self.clustered_index_def_mode
    }

    /// 行 ID 分片位数。
    pub fn GetShardRowIDBits(&self) -> u64 {
        self.shard_row_id_bits
    }

    /// 预分裂 Region 数。
    pub fn GetPreSplitRegions(&self) -> u64 {
        self.pre_split_regions
    }

    /// 是否抑制过长索引错误。
    pub fn SuppressTooLongIndexErr(&self) -> bool {
        self.suppress_too_long_index_err
    }

    /// Returns a cloned interface value and whether it is present, matching Go's `(is, ok)` pair.
    /// 返回 InfoSchema 克隆与是否存在，对应 Go 的 `(is, ok)`。
    pub fn GetInfoSchema(&self) -> (std::option::Option<InfoSchemaRef<C, E>>, bool) {
        (self.info_schema.clone(), self.info_schema.is_some())
    }
}
