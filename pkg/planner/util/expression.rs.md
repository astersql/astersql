# `pkg/planner/util/expression.rs`

## 文件定位

[对应 Rust 源文件](./expression.rs)属于 `astersql-planner-util` crate，是规划器通用工具层的表达式回调边界。`pkg/planner/util/lib.rs` 将私有模块 `expression` 的公开项整体再导出，因此其他 crate 应通过 `astersql_planner_util` 的 crate 根访问这些 API，而不是依赖模块内部路径。

该边界的设计意图是让较低层的 planner util 只依赖抽象的 `plan_base::PlanContext`、AST、表达式和值类型，而由较高层规划器提供实际求值/改写实现，从而避免 util 反向依赖 planner core。当前 Rust 仓库只完成了回调类型、全局槽位和注册函数：代码搜索没有找到槽位读取或 setter 调用；`pkg/planner/core/core_init.rs` 中的 `CALLBACKS` 只是名称注册表，不会安装函数实现。因此这两个回调目前是“可注册但尚未接线”的基础设施，不是已运行的 SQL 主链。

## 核心职责

- `EvalAstExprWithPlanCtxFn` 描述“在规划上下文中求值一个 AST 表达式并返回 `Datum`”的动态回调契约。
- `RewriteAstExprWithPlanCtxFn` 描述“结合规划上下文、输入 schema、字段名和数组 cast 开关，将 AST 改写成可执行表达式”的动态回调契约。
- `EvalAstExprWithPlanCtx` 与 `RewriteAstExprWithPlanCtx` 保存可被全局共享、运行期替换的回调。
- `SetEvalAstExprWithPlanCtx` 与 `SetRewriteAstExprWithPlanCtx` 负责以写锁原子地安装或覆盖回调。

本文件不解析 AST、不执行表达式，也不实现 planner rewrite；它只定义注入接口与存储。实际 Rust 规划逻辑可见 `pkg/planner/core/expression_rewriter.rs` 的 `evalAstExprWithPlanCtx` 和 `rewriteAstExprWithPlanCtx`，但当前签名与这里的槽位契约并不直接一致，且未发现适配注册代码。

## 主要符号

- `pub type EvalAstExprWithPlanCtxFn = dyn Fn(&dyn plan_base::PlanContext, &parser_ast::ast::ExprNode) -> Result<expression::types::Datum, expression::Error> + Send + Sync`：类型擦除的求值回调。借用上下文和 AST，不取得其所有权；成功返回运行期标量值，失败返回 expression crate 的错误。
- `pub type RewriteAstExprWithPlanCtxFn = dyn Fn(&dyn plan_base::PlanContext, &parser_ast::ast::ExprNode, &expression::Schema, types::metadata::NameSlice, bool) -> Result<expression::ExprBox, expression::Error> + Send + Sync`：类型擦除的改写回调。schema 必须存在，字段名切片按值传入，最后的布尔值对应 `allowCastArray` 语义。
- `pub static EvalAstExprWithPlanCtx: RwLock<Option<Arc<EvalAstExprWithPlanCtxFn>>>`：求值槽位，进程启动时为 `None`。
- `pub static RewriteAstExprWithPlanCtx: RwLock<Option<Arc<RewriteAstExprWithPlanCtxFn>>>`：改写槽位，进程启动时为 `None`。
- `pub fn SetEvalAstExprWithPlanCtx(...)`：取得求值槽位的写锁，将值替换为 `Some(callback)`。
- `pub fn SetRewriteAstExprWithPlanCtx(...)`：以相同方式安装改写回调。

这些符号沿用 Go 风格命名；`pkg/planner/util/lib.rs` 通过 crate 级 `#![allow(non_snake_case, non_upper_case_globals)]` 接受这种命名。

## 执行流程

当前文件实际可执行的流程只有注册：

1. 上层构造一个满足对应 `Fn + Send + Sync + 'static` 要求的闭包或函数，并放入 `Arc`。
2. 调用对应 setter。
3. setter 获取静态 `RwLock` 的独占写锁；若锁曾因其他线程 panic 而 poisoned，则用 `PoisonError::into_inner` 继续取得内部值。
4. setter 用 `Some(callback)` 覆盖原值，随后释放写锁。

本文件没有提供调用包装器。未来消费者必须自行取得读锁，处理初始 `None`，并在适当时机克隆 `Arc` 后调用。仓库当前没有这样的 Rust 消费流程，不能据此声称 AST 已通过这些槽位完成求值或改写。

Go 对照中的预期业务流程位于 `pkg/planner/core/expression_rewriter.go`：字面值可直接返回；其他表达式先经 `rewriteAstExprWithPlanCtx` 构造表达式，再以空行求值。Rust 的同路径移植实现也保留了这一算法，但它额外显式接收 `InfoSchema`，尚未适配成本文件的回调签名。

## 数据与状态

两个静态变量是本文件唯一的持久状态，彼此独立，初值均为 `None`。`Option` 区分“尚未安装”和“已有实现”；`Arc` 支持读者跨线程共享回调；`RwLock` 使多读者与少量注册写入可协调。

setter 是覆盖而非“一次初始化”：重复注册时，新 `Arc` 替换槽位中的旧 `Arc`。旧回调只有在槽位及所有外部克隆都释放后才销毁。文件没有版本号、注册来源、重复注册保护、注销 API 或初始化完成标志，也没有保证两个槽位作为一个事务成对更新。

回调参数均为借用，回调不能仅凭签名长期保存 `PlanContext` 或 AST 引用。改写回调的 `NameSlice` 按值移动；schema 为非可选引用，这与 Go 版允许 `nil` 的契约存在差异。

## 依赖与调用关系

crate 边界由 `pkg/planner/util/Cargo.toml` 给出：本文件直接使用其中的 `plan-base`、`parser-ast`、`expression` 和 `types` 路径依赖，标准库提供 `Arc`/`RwLock`。Cargo 元数据将该 crate 对应到 Go 包 `pkg/planner/util`。

静态关系如下：

- 上游导出：`pkg/planner/util/lib.rs` 的 `pub use expression::*` 暴露全部符号。
- 预期实现位置：`pkg/planner/core/expression_rewriter.rs` 定义同目的的 `evalAstExprWithPlanCtx`、`rewriteAstExprWithPlanCtx`。
- 当前注册关系：RustCodeGraph 对两个 setter 均未找到 caller，`rg` 也未找到任何 Rust 调用；它们的 callees 仅是语言/同步原语，图中未形成业务调用边。
- 当前消费关系：`rg` 未找到 Rust 代码读取两个静态槽位。`pkg/planner/core/core_init.rs` 只把两个 Go 风格名称放入 `CALLBACKS` 字符串集合，不能视为函数注册。
- Go 业务调用者包括 `pkg/sessiontxn/staleread/util.go`、`pkg/executor/importer/import.go` 和 `pkg/executor/internal/querywatch/query_watch.go`；这些是对照证据，不是 Rust 运行时调用边。

## 错误处理与边界

setter 本身没有业务错误返回。写锁 poisoning 不会令注册 panic：`unwrap_or_else(PoisonError::into_inner)` 明确选择恢复内部数据并继续覆盖。该策略保证可以重装回调，但不会证明被 poison 前的其他共享状态仍一致；调用方不能把成功返回理解为整个规划器初始化成功。

业务错误由回调契约以 `Result<_, expression::Error>` 向上传播，本文件不转换、包装或记录错误。初始 `None` 也没有统一错误类型，因为这里没有调用帮助函数；未来消费者必须显式决定未注册时返回错误、延迟初始化还是拒绝启动，不能直接 `unwrap` 并假设已注册。

现有 Rust core 实现返回的是其导入的 `errors::Error`，并且需要 `ContextRef` 与 `Arc<dyn InfoSchema>`；接线前必须确认它能否无损适配成本文件要求的 `expression::Error`、`&dyn PlanContext` 参数。改写签名还需解决本文件强制 `&Schema`、Go/Rust core 接受可选 schema，以及 `NameSlice` 所属路径/所有权的差异。

## 并发与资源生命周期

`Send + Sync` 限定保证回调对象可在线程间转移和共享；`Arc` 管理回调生命周期；`RwLock` 保护槽位替换。setter 仅在赋值期间持有写锁，临界区很短，没有 I/O、await、递归回调或其他锁获取。

本文件没有异步任务、通道、事务或外部资源。由于没有消费辅助函数，安全的未来调用模式应在读锁内克隆 `Arc`，释放读锁后再执行回调，避免长时间求值阻塞重新注册，也避免回调重入 setter 时产生锁问题。两个槽位分别加锁，读者可能在并发注册期间观察到一新一旧或一有一无的组合；若系统要求成对一致初始化，需要在本文件之外提供更高层同步，或重构成单一注册对象。

## 与 Go 版本的对应关系

`pkg/planner/util/expression.go` 同样只声明两个包级函数变量，意图是绕开包依赖环，并区分完整 planner 能力与 `expression.EvalSimpleAst` / `expression.BuildSimpleExpr` 的简单表达式能力。Go 的 `pkg/planner/core/core_init.go` 会把它们分别赋为 `evalAstExprWithPlanCtx` 和 `rewriteAstExprWithPlanCtx`，所以 Go 运行链已接通。

Rust 保留了相同的两个概念，但用 `RwLock<Option<Arc<dyn Fn...>>>` 显式表达并发共享、未初始化状态与动态替换。当前迁移并非语义等价完成：

- Rust 仓库没有 setter 调用，也没有槽位消费者；Go 有初始化赋值和实际调用者。
- Go 改写参数的 schema 与 names 可为 `nil`；Rust 回调契约要求 `&Schema` 且 `NameSlice` 按值传入。
- Go 实现从 `PlanContext.GetInfoSchema()` 获取 info schema；当前 Rust core 实现要求调用方额外传入 `Arc<dyn InfoSchema>`，所以不能直接注册到两参数求值回调或五参数改写回调。
- Go 函数变量可直接调用；Rust 槽位必须先加读锁、处理 `None` 并取得 `Arc`。

Go 测试 `pkg/executor/importer/import_test.go` 多处通过 `RewriteAstExprWithPlanCtx` 覆盖导入选项表达式的真实使用场景。未找到对应 Rust 独立测试；`pkg/planner/util/lib.rs` 的测试模块列表也没有 `expression_test.rs`。

## 扩展指南

若要真正接通该边界，优先从以下位置工作：

1. 先统一 `pkg/planner/util/expression.rs` 与 `pkg/planner/core/expression_rewriter.rs` 的上下文、info schema、可选 schema/names 和错误类型契约；不要用丢失 planner 能力的简化桩满足类型检查。
2. 在明确的 planner 初始化位置调用两个 setter，并决定是否必须原子地成对注册。
3. 为消费者增加集中式调用函数，统一处理未注册错误、锁生命周期和 `Arc` 克隆，避免各调用点自行 `unwrap`。
4. 在独立测试文件（建议同目录 `expression_test.rs` 并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入）验证初始未注册、注册后调用、覆盖注册、并发读取和 poison 恢复策略；Rust 测试逻辑应尽量对齐 Go 的实际表达式场景。
5. 同步补齐 importer、stale read、query watch 等对应 Rust 调用链时，参考各 Go 文件及 `pkg/executor/importer/import_test.go`，但将不属于本边界的子系统迁移作为独立任务。

兼容风险主要是改变公开回调签名或未注册行为；正确性风险是 schema/names 不匹配、丢失 info schema 或错误类型转换；性能风险主要来自在读锁持有期间执行复杂 rewrite。扩展时应保持锁外执行实际回调，并为完整 planner 能力保留必要上下文。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被索引为 62 行、3 个顶层图符号。
- RustCodeGraph `node --file pkg/planner/util/expression.rs`：核对完整源码、两个类型别名、两个静态槽位和两个 setter。
- RustCodeGraph `query`：定位 `SetEvalAstExprWithPlanCtx`、`SetRewriteAstExprWithPlanCtx` 及 core 中两个同目的实现；`callers` 对两个 setter 均无结果，`callees` 均无业务调用边。
- `rg` 全仓精确搜索四个符号：确认 Rust 中只有本文件定义和 `pkg/planner/core/core_init.rs` 的字符串名称，未发现注册或读取；同时定位 Go 初始化、业务调用者和测试引用。
- 已读 Rust/Cargo 路径：`pkg/planner/util/expression.rs`、`pkg/planner/util/lib.rs`、`pkg/planner/util/Cargo.toml`、`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/core_init.rs`、`pkg/planner/util/main_test.rs`。
- 已读 Go/测试证据：`pkg/planner/util/expression.go`、`pkg/planner/core/expression_rewriter.go`；精确搜索确认 `pkg/executor/importer/import_test.go` 是当前直接引用该 Go 回调的测试，未发现同名 Rust 独立测试。
- 人工复核结论：本文件存在的原因是提供跨 crate 的 planner 表达式回调注入边界；当前实际运行仅覆盖线程安全注册，业务求值/改写尚未通过该边界接线；安全扩展必须先解决签名适配、未注册行为、锁外调用和独立回归测试。
