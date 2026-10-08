# `pkg/planner/core/scalar_subq_expression.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate（见 `pkg/planner/core/Cargo.toml`），实现规划器对“可在优化期求值的非相关标量子查询”的两层占位对象：实现 `base::Plan` 的 `ScalarSubqueryEvalCtx` 保存物理子计划和求值环境，实现 `expression::Expression` 的 `ScalarSubQueryExpr` 表示子查询某个输出列。`pkg/planner/core/lib.rs` 公开再导出这两个类型以及执行器钩子类型/安装函数，因此 session、executor 和 planner 内部均可消费它们。

它处在 AST 子查询改写与执行器之间：`pkg/planner/core/expression_rewriter.rs` 将已优化的子查询物理计划包装为 `ScalarSubqueryEvalCtx`，为输出列创建 `ScalarSubQueryExpr`，并把上下文注册到会话；真正取首行的能力不由 planner 直接依赖 executor，而由 `InstallEvalSubqueryFirstRow` 安装的函数指针反向注入。`pkg/session/runtime/explain_query.rs` 是当前明确的 Rust 安装者和 EXPLAIN 消费者。

## 核心职责

1. `EvalSubqueryFirstRowFn`、`EVAL_SUBQUERY_FIRST_ROW`、`InstallEvalSubqueryFirstRow` 和 `eval_subquery_first_row` 定义并维护 planner 到执行器的窄接口。全进程只允许安装一次；同一函数地址重复安装幂等，不同函数会报错。
2. `ScalarSubqueryEvalCtx` 持有物理子计划、执行上下文、InfoSchema 快照、计划上下文、输出列 ID 及共享求值状态。多个列表达式克隆该上下文时共享 `Arc<Mutex<SubqueryEvaluation>>`，从而只执行一次子查询。
3. `ScalarSubQueryExpr` 把一个输出列 ID 映射到求值结果中的对应 `Datum`，并在表达式级缓存值或错误；同时实现规划阶段需要的类型、哈希、相等性、解释输出、排序规则及类型擦除接口。
4. 两个对象保留独立子查询树的计划身份和 EXPLAIN 信息。`pkg/planner/core/flat_plan.rs` 会从会话注册表向下转型 `ScalarSubqueryEvalCtx`，把 `scalar_sub_query` 附加成独立 flat-plan 树。

## 主要符号

- `EvalSubqueryFirstRowFn`：函数指针签名，输入 `context::Context`、`base::PhysicalPlan`、`infoschema::InfoSchema`、`base::PlanContext`，输出首行 `Option<Vec<Datum>>` 或表达式错误。`None` 表示没有行。
- `EVAL_SUBQUERY_FIRST_ROW: OnceLock<_>`：进程级钩子存储；没有重置接口。
- `InstallEvalSubqueryFirstRow`：公开安装入口，比较函数地址实现相同安装的幂等性，拒绝覆盖不同实现。
- `eval_subquery_first_row`：crate 内调用入口；未安装时返回 `EvalSubqueryFirstRow is not installed`。
- `SubqueryEvaluation`：上下文级状态机，只有 `Pending` 和 `Complete(Result<Vec<Datum>, Error>)`；成功和失败都被缓存。
- `ScalarSubqueryEvalCtx`：计划包装器。`New` 分配 plan ID；`getColVal` 触发求值并按 `output_col_ids` 定位列；`selfEval` 执行并缓存；`Clone` 共享执行状态和 `Arc` 资源。其固有 `Schema()` 返回 `None`，而 `base::Plan::schema()` 委托给被包装物理计划，两者用途不同。
- `ExpressionEvaluation`：单个表达式的 `evaluated/error/value` 缓存。
- `ScalarSubQueryExpr`：表达式占位符。`new` 绑定列 ID 与上下文；`selfEvaluate` 取列并缓存；`Eval` 返回完整 `Datum`；`encoded_hash` 生成标志字节加有符号列 ID 的可比较大端编码。
- `expression::Expression` 实现：声明 `IsCorrelated == false`、`ConstLevel == ConstNone`；索引解析、去相关和列重映射均克隆自身；相等性只比较列 ID。
- `expression::VecExpr` 实现：`Vectorized()` 当前返回 `true`，但所有 `VecEval*` 都返回未实现错误。所有标量类型专用 `Eval*` 同样未实现，只有通用 `Eval` 可真正求值。
- `SafeToShareAcrossSession`：明确返回 `false`，因为对象携带会话、InfoSchema 和物理计划状态。

## 执行流程

1. `pkg/planner/core/expression_rewriter.rs` 优化非相关子查询得到 `Arc<dyn PhysicalPlan>`，用 `ScalarSubqueryEvalCtx::New` 分配计划 ID 并保存上下文和 InfoSchema。
2. 改写器为子查询 schema 的每一列分配计划列 ID，写入 `output_col_ids`，再用共享上下文的克隆构造相应 `ScalarSubQueryExpr`；返回类型与 coercibility 从原输出列复制。
3. 上下文通过 session variables 的 `RegisterScalarSubQ` 注册，供 flat plan、EXPLAIN 和资源统计遍历独立子查询树。普通立即求值分支也使用同一首行桥接函数；非求值 EXPLAIN 分支保留表达式占位符。
4. 首次调用 `ScalarSubQueryExpr::Eval` 时，`selfEvaluate` 获取表达式互斥锁，调用 `eval_ctx.getColVal(column_id)`。
5. `getColVal` 调用上下文的 `selfEval`。若状态为 `Pending`，它在持有上下文互斥锁期间调用已安装的执行器函数；`Some(row)` 保存该行，`None` 转为空向量，错误原样保存。若状态已经完成，则直接复用成功或错误。
6. `getColVal` 在 `output_col_ids` 中查找列 ID，并用相同下标从结果向量取值。成功值写入表达式缓存并置 `evaluated`；失败时缓存错误并把值置为 NULL。
7. 后续同一表达式的 `Eval` 读取表达式缓存；其他共享上下文的列表达式仍会进入 `getColVal`，但上下文级缓存阻止物理子查询再次执行。

## 数据与状态

`ScalarSubqueryEvalCtx` 的不可变资源主要由 `Arc` 管理：`plan_context`、`scalar_sub_query`、执行 `ctx` 和 `is` 可以安全跨克隆存活。`output_col_ids` 在克隆时复制，要求构造者在注册/对外使用前完成列 ID 填充；求值结果则通过共享的 `evaluation` 保持一致。

上下文结果必须满足位置不变量：`output_col_ids[i]` 对应执行器首行的 `columns[i]`。代码会分别检查“找不到列 ID”和“结果列数不足”，但不会拒绝多余结果列。无返回行被规范化为空向量，因此任何列读取都会以列数不匹配报错，而不是自动产生 NULL；是否应补 NULL 由上层改写流程决定。

`ScalarSubQueryExpr` 的 `constant` 保存返回类型、排序规则及 Constant 的通用行为，但实际求值值位于 `ExpressionEvaluation.value`；只有内存估算时才临时用 `clone_with_value` 合并两者。哈希缓存只由列 ID 决定，编码为 `SCALAR_SUB_Q_FLAG` 加 `(id as u64) ^ 0x8000_0000_0000_0000` 的大端字节，与 Go 的可比较有符号整数编码一致。

## 依赖与调用关系

上游生产者是 `pkg/planner/core/expression_rewriter.rs`：其中多个标量/存在子查询分支调用 `ScalarSubqueryEvalCtx::New`、填充 `output_col_ids`、调用 `ScalarSubQueryExpr::new` 并注册上下文。`pkg/planner/core/optimizer_runtime.rs` 还会通过 `as_any_mut` 恢复具体表达式类型。

执行器边界由 `pkg/session/runtime/explain_query.rs` 的 `InstallEvalSubqueryFirstRow(eval_session_scalar_subquery_first_row)` 接线；该文件也按 `ScalarSubqueryEvalCtx` 向下转型注册项，渲染独立的 ScalarSubQuery 计划。`pkg/planner/core/flat_plan.rs` 和 `pkg/executor/statement_ru_plan_walk.rs` 把该上下文视为独立树边界，分别服务 flat plan 与 RU 计划遍历。

直接 crate 依赖来自 `pkg/planner/core/Cargo.toml`：`base-dependency` 提供计划 trait/context，`expression-dependency` 提供 `Expression`、`Constant`、`Datum`、schema、chunk 与错误类型，`infoschema-dependency` 提供元数据快照，`property-dependency` 提供计划统计信息。标准库的 `OnceLock`、`Arc` 和 `Mutex` 分别承担全局安装、共享所有权和同步缓存。

RustCodeGraph 对本文件报告 15 个使用文件，明确列出 `pkg/executor/statement_ru_plan_walk.rs`、`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/find_best_task.rs`、`pkg/planner/core/flat_plan.rs` 与 `pkg/planner/core/integration_test.rs` 等；图对函数指针和 trait 动态分派没有完整静态调用边，因此这些边又由上述源码位置核验。

## 错误处理与边界

- 钩子未安装、重复安装不同实现、上下文缺失、列 ID 不存在及结果列数不足均返回 `expression::errors::Error`，不会静默降级。
- 执行器错误会同时缓存在 `SubqueryEvaluation::Complete(Err)` 和具体表达式的 `ExpressionEvaluation.error` 中，后续求值复现同一错误而不重新执行。
- 任一互斥锁中毒都会通过 `expect` panic；这不是可恢复的表达式错误路径。
- `GetType`/`GetTypeMut` 要求 `constant.RetType` 已由改写器设置；直接使用 `Default` 后调用会 panic。`Default` 和 `new` 本身不会设置真实返回类型。
- `base::Plan::clone_for_plan_cache` 返回 `(None, false)`，`SafeToShareAcrossSession` 返回 `false`，说明该对象不能按普通计划缓存节点或跨会话复用。
- 类型专用标量求值和全部向量化求值当前都显式未实现。调用方必须走通用 `Eval`；`Vectorized() == true` 与 `VecEval*` 返回错误这一现状需要被视为兼容风险，而不能解释成向量化已可用。
- `replace_expr_columns`、`set_output_names`、不可缓存原因 setter/getter 是空实现；计划 schema、统计和输出名则委托给物理子计划。

## 并发与资源生命周期

全局执行器钩子由 `OnceLock` 同步初始化，安装后在进程生命周期内保持不变。上下文和表达式分别用不同 `Mutex` 串行化首次求值及缓存访问；上下文互斥锁覆盖实际执行器调用，因此共享同一 `evaluation` 的并发表达式只会触发一次子查询，但长时间执行会阻塞其他等待同一上下文的求值者。

克隆 `ScalarSubqueryEvalCtx` 会共享物理计划、会话上下文、InfoSchema 和求值状态；克隆 `ScalarSubQueryExpr` 则复制当时的表达式缓存与哈希缓存，同时通过所含上下文克隆继续共享子查询级结果。该差异保证不同表达式实例可以独立记录自身值/错误，又不重复执行底层子查询。

资源由 `Arc` 和值所有权自动回收，没有后台任务、通道或显式关闭流程。会话注册表可能让上下文活过表达式改写栈帧；因此不能把构造参数替换成短生命周期借用，也不能把该表达式标记为跨会话安全。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/scalar_subq_expression.go`。两版都保留 `ScalarSubqueryEvalCtx`/`ScalarSubQueryExpr` 两层结构、按输出列 ID 取首行值、只执行一次、错误缓存、仅通用 `Eval` 可用、列 ID 决定哈希与相等性、`ConstNone`、非相关以及 EXPLAIN 字符串格式。

Rust 为满足所有权和并发要求做了显式化：Go 结构体用布尔值、错误和切片记录状态，Rust 用 `Arc<Mutex<SubqueryEvaluation>>` 与 `Mutex<ExpressionEvaluation>`；Go 的全局 `EvalSubqueryFirstRow` 变量在 Rust 中变成 `OnceLock` 安装接口；Go 的嵌入 `Constant` 在 Rust 中以私有字段加 `Deref/DerefMut` 和 trait 委托表达。

需注意可观察差异：Go 的 `Traverse`、`Decorrelate` 等通常返回同一指针，Rust 返回深度克隆的表达式；Go 对空首行把 `colsData` 留为 nil，Rust 转为空向量，列读取效果仍为失败；Rust 对结果列不足增加了显式错误，避免 Go 版本潜在的越界；Rust 的克隆与互斥锁使并发语义比 Go 原结构更明确。`pkg/planner/core/scalar_subq_expression_test.rs` 验证字符串使用表达式自身列 ID，`scalar_subq_expression_aster_unit_test.rs` 验证 Go 对齐的哈希、相等性、未求值内存、向量化声明、跨会话限制和类型擦除恢复。

## 扩展指南

- 若实现某种 `EvalInt`/`VecEval*`，应以通用 `Eval` 的一次性求值和 NULL/错误语义为基准，不能绕过 `selfEvaluate` 或造成重复执行；同步扩展独立测试文件，不要把测试写入本源文件。
- 若改变输出列映射，应同时检查 `ScalarSubqueryEvalCtx::getColVal`、`expression_rewriter.rs` 中所有 `output_col_ids` 构造分支，以及多列表达式/RowFunc 路径；增加空结果、缺列、多列和未知列 ID 的测试。
- 若改变钩子生命周期或允许替换安装者，必须评估进程级并发初始化、不同 session 实现竞争及测试隔离；当前 `OnceLock` 没有卸载能力是明确不变量。
- 若改变哈希或相等性，必须保持 `Hash64`、`HashCode`、`CanonicalHashCode`、`Equal`、`Equals` 五处一致，并与 Go 的 `codec.EncodeInt` 字节语义核对。
- 若改变克隆行为，需区分“表达式缓存复制”与“上下文执行结果共享”，并覆盖并发首求值只执行一次、错误只执行一次的回归测试。
- 若改变计划接口，应同步检查 `flat_plan.rs`、`statement_ru_plan_walk.rs`、session EXPLAIN 及计划缓存行为。真实功能测试应继续放在同目录独立的 `*_test.rs` 文件中。

## 验证依据

- 源码全量：`pkg/planner/core/scalar_subq_expression.rs`（750 行）；模块边界：`pkg/planner/core/lib.rs`；crate 依赖：`pkg/planner/core/Cargo.toml`。
- 上游与消费端：`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/flat_plan.rs`、`pkg/planner/core/optimizer_runtime.rs`、`pkg/session/runtime/explain_query.rs`、`pkg/executor/statement_ru_plan_walk.rs`。
- Go 对照：`pkg/planner/core/scalar_subq_expression.go`，并参考 `pkg/planner/core/expression_rewriter.go` 的构造流程。
- 独立 Rust 测试：`pkg/planner/core/scalar_subq_expression_test.rs`、`pkg/planner/core/scalar_subq_expression_aster_unit_test.rs`；相关集成安装点：`pkg/planner/core/integration_test.rs`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`query` 找到 Rust/Go 两版 `ScalarSubqueryEvalCtx`、`ScalarSubQueryExpr`、Rust 的安装函数与内部调用函数；`node --file ... --offset ...` 覆盖目标 1–750 行，并报告 15 个使用文件；`callees eval_subquery_first_row` 确认其引用 `EVAL_SUBQUERY_FIRST_ROW`。同名类型的 trait/函数指针动态边未被完整解析，已用上述直接源码位置补证。
- 本任务是纯文档分析，按任务要求未运行 Cargo。结构验证要求为目标文件存在且恰有本文列出的 11 个固定二级标题。
