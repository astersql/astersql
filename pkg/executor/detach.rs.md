# `pkg/executor/detach.rs`

## 文件定位

[`pkg/executor/detach.rs`](./detach.rs) 定义执行器树与执行上下文的“拆离”抽象：从仍绑定 session 可变状态的对象构造可独立使用的副本。其典型目标是允许游标继续拉取旧语句结果时，原 session 去执行新语句（见 `detach.rs:16-19` 与 Go `detach.go:23-31`）。

该文件属于 `astersql-executor` crate；`pkg/executor/Cargo.toml` 声明 crate 名和 `lib.rs` 入口，`lib.rs:95-96` 将它作为公开 `detach` 模块导出。当前 Rust 生产执行主链中，`adapter.rs:327` 的 `ExecExecutor::Detach` 及 typed executor 各自的实现是已接线路径；仓库搜索未找到生产代码实现本文件的 `DetachableExecutor` 或调用其顶层 `Detach`，直接使用证据集中在 `detach_test.rs` 和 `detach_integration_test.rs`。因此，这里是 Go 语义的逐项泛型移植边界，不应误述为已统一替代 Rust 运行时的拆离路径。

## 核心职责

- `DetachableExecutor` 和顶层 `Detach` 提供执行器树的递归、全或无拆离协议：节点先制作不含子树的副本，再拆离所有子树，任一节点拒绝就丢弃临时副本并返回失败（`detach.rs:27-53`）。
- 四个 context trait 把表读取器的表达式、DistSQL、range 和 PB 构建状态的拆离动作交给具体类型（`DetachableExprContext`、`DetachableDistSqlContext`、`DetachableRangeContext`、`DetachableBuildPbContext`）。
- `TableReaderExecutorContext::Detach` 统筹上述四种状态，确保 range/PB 拆离使用同一个新的静态表达式上下文（`detach.rs:86-104`）。
- `ProjectionExecutorContext` 与别名 `SelectionExecutorContext` 负责静态化求值上下文；`ProjectionExec::DetachWith` 与 `SelectionExec::DetachWith` 在复制 shell 前拒绝仍需 session 可选求值属性的算子。
- `TableReaderExecutor` 及 `IndexReaderExecutor`/`IndexLookUpExecutor` 别名提供轻量 shell，通过调用者传入的转换函数拆离 context（`detach.rs:152-172`）。

## 主要符号

- `pub trait DetachableExecutor: Sized`：三步协议。`detach_shallow(&self) -> Option<Self>` 必须不破坏原节点；`all_children(&self) -> Vec<Self>` 返回要递归处理的子节点；`set_all_children` 仅在子树全部成功后重建副本树。
- `pub fn Detach<E: DetachableExecutor>(&E) -> (Option<E>, bool)`：树级入口。`bool == true` 时实现隐含不变式是 `Option` 必为 `Some`，源码用 `expect("successful detach must return an executor")` 显式固化该不变式（`detach.rs:45-50`）。
- `DetachableExprContext::into_static` / `DetachableEvalContext::into_static`：返回 `Some` 表示已从 session context 转为静态副本；本文件的约定中 `None` 表示本来就不是 session 上下文，应保留/clone，而不是错误。
- `TableReaderExecutorContext<E,D,R,P>` 和 `IndexReaderExecutorContext` 别名：同时携带 `expression_context`、`distsql_context`、`range_context`、`build_pb_context` 的通用聚合体。
- `IndexLookUpExecutorContext<C>::Detach`：目前只 `clone` 原 context，特意保留 Go `detach.go:83-88` “计算了 `newCtx` 但返回 `iluCtx`”的可观测行为。
- `ProjectionExecutorContext<E>` / `SelectionExecutorContext<E>`：前者持有 `evaluation_context`，后者是完全相同结构的类型别名。
- `TableReaderExecutor<C>::DetachWith`：总是返回 `(Some(new_shell), true)`；`IndexReaderExecutor` 与 `IndexLookUpExecutor` 只是该 shell 的别名。
- `OptionalEvalProperties::is_empty` 和 `DetachableFilter::required_optional_properties_are_empty`：将“是否仍依赖未被复制的可选 session 属性”抽象为门禁。
- `ProjectionExec<C,P>::DetachWith` 与 `SelectionExec<C,F>::DetachWith`：分别检查聚合属性集与每个 filter，通过后 clone 配置/过滤器并转换 context。

## 执行流程

1. 树级 `Detach` 先调用当前节点的 `detach_shallow`。若返回 `None`，立即返回 `(None, false)`，不读取或修改子节点。
2. 通过 `all_children` 获得原节点子列表，按原顺序深度优先递归。每个成功的子副本先放入局部 `detached_children`。
3. 任一子树返回 `ok == false` 时，当前函数直接失败；因为之前得到的都是独立局部副本，原树仍然可用。
4. 只在所有子树成功后调用 `detached.set_all_children(detached_children)`，然后返回 `(Some(detached), true)`。
5. 表读取 context 先调用 `expression_context.into_static()`。成功时，先拆离 DistSQL context，并将同一个 `static_expression_context` 传给 range/PB context；返回 `None` 则整体 `clone` 原 context，不调用其他三个 detach hook。
6. Projection/Selection context 优先静态化 eval context，`None` 时 clone 原值。算子 shell 还要先检查可选属性：Projection 集合非空或 Selection 任一 filter 不可拆离时返回 `(None, false)`；只有通过门禁才调用 `detach_context`。

## 数据与状态

本文件不定义全局可变状态、缓存或单例。所有生产类型都由泛型字段组成，状态转换依赖 trait 实现和 `Clone`：

- 树拆离的临时状态只有浅副本 `detached`、原子列表 `children` 与预分配容量的 `detached_children`。子顺序与 `all_children` 返回顺序一致。
- `TableReaderExecutorContext` 不允许 range/PB 各自重新创建不同的静态表达式上下文；它们均借用先生成的同一局部值。
- shell 拆离是浅层 clone/字段复制；具体 context 是否真正切断 session 指针，由 `detach_context` 和四个 context trait 的实现保证。
- `IndexLookUpExecutorContext` 目前不替换其 `table_reader_context`，这是明确的兼容行为，不是此文件已实现的完全静态化。

## 依赖与调用关系

代码本身只使用 Rust 标准库的 `Option`、`Vec`、`Clone` 和泛型/trait 机制，没有直接 `use` 任何外部 crate。`Cargo.toml` 证明它编译在 `astersql-executor` 中，但本文件不直接消费该 manifest 中的 DistSQL/expression 等具体 crate；这些边界被 trait 参数化。

RustCodeGraph 对 `pkg/executor/detach.rs` 的文件节点报告 8 个使用文件，展示的使用者为 `detach_test.rs`、`detach_integration_test.rs` 及若干 typed executor 测试；对精确名称的调用图显示 `detach_shallow`、`all_children`、`set_all_children` 均由本文件的递归 `Detach` 调用，各 context hook 由相应 `Detach`/`DetachWith` 调用。由于 `Detach` 名在仓库中高度重载，自然语言 `explore` 结果会同时混入 Go、memory tracker 和 typed executor 同名边；本文档仅采用文件限定后可复核的边。

相邻的已接线运行时路径是 `adapter.rs` 的 `ExecExecutor::Detach`/`RecordSet::TryDetach` 及 `typed_*.rs` 实现，与本文件的 `DetachableExecutor` 是两套不同 trait；不能仅因同名就认定存在调用边。

## 错误处理与边界

- 本 API 不使用 `Result`：“不支持拆离”是预期能力分支，用 `(None, false)` 表示；不会传递具体错误原因。
- 子节点失败时不返回部分树。这保证调用者可继续使用原树，但也意味着 trait 实现不得在 `detach_shallow(&self)` 中通过内部可变性破坏原对象。
- `expect` 仅检查内部协议：递归 `Detach` 返回 `true` 却给出 `None` 时才会 panic；按当前函数的所有返回路径，这个组合无法由正常调用产生。
- `into_static() == None` 不是拆离失败。对 reader/projection/selection context，它表示保留非 session 上下文；添加实现时不得把其解释为应返回 `(None, false)`。
- Projection 的任一非空 optional property、Selection 的任一不可拆离 filter 都是硬性边界，而不是忽略该属性后继续 clone。Go `detach.go:142-174` 的 TODO 也说明，未来只有在对应可选属性可被安全拆离时才能放宽。
- 递归实现没有深度限制；异常深的人工执行树可带来调用栈风险。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁或事务，也没有显式 `Drop`/`Close` 行为。它的并发保证是结构性的：对 `&self` 生成新值，让新旧执行可以分开继续。这不自动意味着深层字段不共享；trait 实现必须为 session 可变资源创建静态/独立副本，只能共享本来就支持该生命周期的资源。

`detach_integration_test.rs:85-123` 给出具体生命周期证据：在真实 TestKit session 上构造 reader context，拆离后表达式变为静态、DistSQL 清除 session ID、range/PB 记录使用静态表达式；关闭底层 store 后，副本的静态字段仍保持可检查。这是测试 fixture 层面的生命周期证明，不是本泛型类型自动具备 `Send`/`Sync` 约束；源码没有声明这两个 bound。

## 与 Go 版本的对应关系

Rust 文件直接对照 `pkg/executor/detach.go`，`Cargo.toml` 的 `[package.metadata.porting] go-package = "pkg/executor"` 也确认了包级移植归属。

- Go `Detach(exec.Executor)` 对应 Rust `Detach<E: DetachableExecutor>`：都先调用单节点 detach，再按子节点递归，任一失败则整体失败，最后才 `SetAllChildren`/`set_all_children`。Rust 用 `Option + bool` 表达 Go 的 `nil + bool`。
- Go 对 table reader 和 index reader context 各写一份同构逻辑（`detach.go:51-80`）；Rust 用 `IndexReaderExecutorContext` 类型别名复用同一实现。Go 以类型断言识别 `*sessionexpr.ExprContext`，Rust 用 `into_static() -> Option<Self>` 抽象该分支。
- Go `indexLookUpExecutorContext.Detach` 构造了 `newCtx` 并拆离其 table-reader context，但最后返回原 `iluCtx`（`detach.go:83-88`）。Rust `IndexLookUpExecutorContext::Detach` 刻意 `self.clone()`，`detach_test.rs::index_lookup_context_preserves_go_return_value` 固定了这一现状。如要更正为返回拆离的内层 context，必须先明确 Go 端兼容决策，不能单边“修复” Rust。
- Go projection/selection context 仅对 session eval context 调用 `IntoStatic`；Rust 的 `DetachableEvalContext::into_static` 与 `None` 回退 clone 保留相同分支语义。
- Go 的 Table/Index/IndexLookUp reader 通过浅复制执行器后替换 context 且总是成功（`detach.go:108-139`）；Rust 用一个 `TableReaderExecutor` 和两个别名、再由 `DetachWith` 注入具体 context 转换。
- Go projection/selection 分别拒绝非空 `RequiredOptionalEvalProps` 和任一 filter 的非空 optional props（`detach.go:142-174`）；Rust 用 `OptionalEvalProperties` 和 `DetachableFilter` 保留同样的全或无门禁。

Go `detach_test.go::TestDetachExecutor` 验证不可拆离节点、单 reader、失败子节点与成功嵌套 reader。Rust `detach_test.rs` 扩展了相同树级意图，并增加 context 四元组、Go index-lookup 返回值、optional-property 门禁的独立验证。

## 扩展指南

- 为新执行器接入本泛型框架时，实现 `DetachableExecutor`，并确保 `detach_shallow` 既不修改原对象，也不把旧子树留在副本中；否则子树失败时的原子性保证会被破坏。同步在独立 `pkg/executor/detach_test.rs` 增加成功、中途失败和原树仍可用的回归，不把测试写入生产源文件。
- 扩展 reader context 时，将任何依赖表达式的子 context 显式接收同一静态表达式上下文；必须同时更新 `TableReaderExecutorContext::Detach`、构造点、`detach_test.rs` 以及真实 session 生命周期用例 `detach_integration_test.rs`。
- 支持某个新 optional property 的拆离不应简单删除门禁：先定义该属性的独立副本/生命周期语义，再收窄 `OptionalEvalProperties` 或 `DetachableFilter` 的判定，并在 Go `detach.go` 的对应 TODO 与 Go/Rust 测试中保持语义一致。
- 若要让本文件真正接入 Rust 结果集主链，需要明确调和 `DetachableExecutor` 与 `adapter.rs::ExecExecutor::Detach` 的所有权签名（前者是 `&self -> Self`，后者是 `&mut self -> Box<dyn ExecExecutor>`），并按 typed executor 的状态/清理规则逐类迁移。这是架构接线工作，不是在本文件增加一个调用就能安全完成。
- 修改 `IndexLookUpExecutorContext::Detach` 前必须先核实并同步 Go 的实际返回值，再更新 `index_lookup_context_preserves_go_return_value`，避免 Rust 与 Go 行为分叉。
- 性能上，`all_children() -> Vec<Self>` 会先 clone/移出整个子列表，随后再为拆离子列表分配同等容量；若要优化大扇出树，需在不破坏原树可用性和子顺序的前提下设计新 trait 访问方式，并用大树测试/基准证明收益。

## 验证依据

- 源码与模块边界：`pkg/executor/detach.rs` 全文；`pkg/executor/lib.rs:95-96,402-408`；`pkg/executor/Cargo.toml` 的 `[package]`、`[lib]`、`[features]` 与 `[package.metadata.porting]`。该文件无条件编译项、无模块常量和外部 `use`。
- RustCodeGraph：`status` 显示本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/executor/detach.rs --offset 1 --limit 260` 返回全部 232 行及 8 个使用文件；`explore "pkg/executor/detach.rs Detach DetachableExecutor TableReaderExecutorContext ProjectionExec SelectionExec"` 验证了 trait hook 到 `Detach`/`DetachWith` 的调用边和测试调用者。单独 `callers detach_shallow` 在 40 秒内未返回并已中止，因此不将该次命令作为额外证据。
- Rust 测试：`pkg/executor/detach_test.rs` 的 `detach_recursively_resets_executor_state_without_mutating_original`、`detach_is_atomic_when_a_child_cannot_detach`、`reader_context_detaches_every_session_bound_component`、`reader_context_preserves_non_session_context_without_detaching_dependencies`、`index_lookup_context_preserves_go_return_value`、`projection_and_selection_contexts_staticize_only_session_evaluation_contexts`、`executor_shells_detach_context_and_reject_required_optional_properties`；`pkg/executor/detach_integration_test.rs::detached_reader_context_survives_its_real_sql_session`。
- Go 对照：`pkg/executor/detach.go` 全文与 `pkg/executor/detach_test.go::TestDetachExecutor`。Rust 运行时边界还核对了 `pkg/executor/adapter.rs:327-348,768-835`、`pkg/executor/internal/exec/executor.rs:143` 以及 `rg` 定位的 typed/session runtime `Detach` 实现。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文件存在且上述 11 个固定二级标题各出现一次；内容人工复核点是能区分拆离树、context 静态化、optional-property 门禁，以及本泛型框架尚未接入 Rust 生产主链的事实。
