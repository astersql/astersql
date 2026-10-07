# `pkg/planner/core/operator/logicalop/expression_util.rs`

## 文件定位

该文件属于 `astersql-planner-core-operator-logicalop` crate；crate 边界由同目录的 `Cargo.toml` 定义，模块由 `lib.rs` 中的 `mod expression_util` 纳入，并通过 `pub use expression_util::*` 向逻辑算子 crate 的调用方公开。它位于逻辑优化阶段的表达式判定边界：判断一组过滤条件是否已经足以证明结果集为空，但不直接创建或改写逻辑计划。

当前文件只有 3 个函数，没有类型、常量、trait、`impl` 或条件编译项：公开函数 `Conds2TableDual`、`IsConstFalse`，以及私有辅助函数 `is_mutable_constant`（`expression_util.rs:26,40,47`）。

## 核心职责

- `Conds2TableDual` 对 CNF 条件切片进行保守判定：空切片不能证明空结果；任一由 `expression::IsConstNull` 识别且不是可变常量的条件可证明过滤掉所有行；否则只在恰有一个条件时继续检查常量假（`expression_util.rs:26-38`）。
- `IsConstFalse` 识别普通 `expression::Constant` 中按 MySQL 布尔转换为 `0` 的值，也把 NULL 视作过滤语义下的假；非常量、参数常量、延迟常量和转换失败均不判为恒假（`expression_util.rs:47-64`）。
- `is_mutable_constant` 集中排除带 `DeferredExpr` 或 `ParamMarker` 的常量，防止复用缓存计划时因执行期取值变化而把非空计划提前折叠（`expression_util.rs:40-44`）。

文件名沿用 Go 侧“条件转 TableDual”的职责，但 Rust API 返回 `bool`。`LogicalTableDual` 的构造、schema/name 继承或零基数写入由各调用点负责，不在本文件内完成。

## 主要符号

### `pub fn Conds2TableDual(conds: &[Expression]) -> bool`

输入是 crate 根再导出的 `Expression`，即 `expression::ExprBox` 的别名（`logicalop/lib.rs`）。返回值表示“当前条件足以安全证明所有行都会被拒绝”，不是返回计划节点。

判定顺序是重要契约：

1. `conds.is_empty()` 立即返回 `false`。
2. 遍历条件，只要 `expression::IsConstNull` 为真且 `is_mutable_constant` 为假，立即返回 `true`。
3. 若上一步未命中，仅当 `conds.len() == 1` 时调用 `IsConstFalse`。

这里的 `expression::IsConstNull` 并不识别任意裸 NULL；当前实现只识别 `<、<=、>、>=、=、!=` 标量比较中“第二个参数是值为 NULL 且没有 `DeferredExpr` 的 `Constant`”（`pkg/expression/util.rs:2066-2079`）。因此 `[NULL, 1]` 不会在第 2 步命中，且因条件数大于 1 也不会进入第 3 步；这解释了独立测试的多条件预期。

### `fn is_mutable_constant(cond: &dyn expression::Expression) -> bool`

该私有函数通过 `as_any().downcast_ref::<expression::Constant>()` 检查具体类型；只有常量且 `DeferredExpr.is_some()` 或 `ParamMarker.is_some()` 时返回 `true`。非 `Constant` 表达式返回 `false`。它被 `Conds2TableDual` 和 `IsConstFalse` 共同调用，保证两个入口采用同一执行期可变性门槛。

### `pub fn IsConstFalse(cond: &dyn expression::Expression) -> bool`

该函数先要求表达式可下转为 `expression::Constant`，再拒绝执行期可变常量。其后 NULL 直接返回 `true`；非 NULL 值使用克隆的 `types::DefaultStmtNoWarningContext` 调用 `Datum::ToBool`，仅在转换成功且结果为 `0` 时返回 `true`。默认上下文使用 `DefaultStmtFlags`、UTC 和忽略 warning 的处理器（`pkg/types/context.rs:317-323`）。

## 执行流程

典型主链是：SQL 条件被表达式重写或谓词简化 → 调用方把条件切片传给 `Conds2TableDual` → 本文件先排除空输入和计划缓存可变值 → 检查比较结果恒 NULL或单条件恒假 → 返回布尔结果 → 调用方执行具体计划动作。

不同调用点对 `true` 的消费方式不同：

- `LogicalSelection::PredicatePushDown` / `PredicatePushDownRoot` 构造零行 `LogicalTableDual`，复制 schema（根路径还复制 output names），并清除或重新安置谓词（`logical_selection.rs:92-137,172-185,189-259`）。
- `LogicalJoin::PredicatePushDownRoot` 在 NOT 下推及谓词简化后构造 `LogicalTableDual`，继承 join 的 schema 和 output names（`logical_join.rs:648-676`）。
- `AttachSelection` 在简化条件后选择构造 `LogicalTableDual` 或普通 `LogicalSelection`（`logical_plans_misc.rs:35-75`）。
- `LogicalDataSource::PredicatePushDown` 不替换节点，而把 `TableStats.RowCount` 及各列 NDV 置零，之后仍保留原条件并继续派生访问路径（`logical_datasource.rs:1278-1302`）。
- WHERE 构建路径先移除恒真普通常量，再以判定结果把当前计划替换为 `LogicalTableDual`，同时设置谓词下推、键信息和谓词简化优化标志（`logical_plan_builder_runtime.rs:1410-1453`）。

## 数据与状态

本文件不拥有持久状态，只借用读取 `&[Expression]` 或 `&dyn expression::Expression`。它不修改表达式、不缓存结果，也不创建计划节点。

参与判定的状态来自 `expression::Constant`：

- `Value` 提供 NULL 状态和 `ToBool` 转换值。
- `DeferredExpr` 表示计划缓存中延迟到复用时求值的表达式。
- `ParamMarker` 表示执行准备语句时才提供的参数。

关键不变量是：只有对后续执行保持稳定的常量事实才能触发空结果折叠。多条件中的普通常量假不会由本文件直接折叠；源码注释明确要求后续优化阶段保留联合条件形状（`expression_util.rs:35-37`）。

## 依赖与调用关系

直接源码依赖只有 crate 根的 `Expression` 别名，以及通过依赖 crate 路径使用的 `expression` 和 `types`。`Cargo.toml` 将它们分别绑定到 `pkg/expression` 与 `pkg/types`；本文件不使用网络、存储、会话或异步运行时。

RustCodeGraph 将目标文件识别为 5 个索引符号，并报告被 7 个文件使用；精确函数级 `callers/callees` 查询未返回边。用文本引用补充后，可确认生产调用点位于：

- `pkg/planner/core/logical_plan_builder_runtime.rs:1433`；
- `logical_selection.rs:111,172,203,241`；
- `logical_join.rs:662`；
- `logical_plans_misc.rs:54`；
- `logical_datasource.rs:1292,2096`。

测试调用来自 `expression_util_test.rs` 和 `logical_d_aster_unit_test.rs`。`IsConstFalse` 在生产代码中仅由 `Conds2TableDual` 调用；其公开性同时允许独立单元测试和未来逻辑算子直接复用。

## 错误处理与边界

三个函数都不返回 `Result`，所有“不足以证明恒假”的情况都降级为 `false`：非 `Constant`、可变常量、布尔转换错误、空条件、多条件中未命中 `IsConstNull` 的常量假均属此类。这是安全侧的保守策略：漏掉一次可行优化仍保持语义正确，而误判为空会丢失结果行。

需特别区分以下边界：

- NULL 单条件：`IsConstFalse` 判真，因此可折叠。
- 多条件中的裸 NULL：`expression::IsConstNull` 不识别裸常量，且不会走单条件分支，因此当前实现不折叠。
- 比较表达式右参数为非延迟 NULL：`expression::IsConstNull` 判真，可在多条件中折叠。
- `ParamMarker` 或 `DeferredExpr`：由 `is_mutable_constant` 拒绝；不过对嵌套标量函数的计划缓存风险，本文件没有像 Go 的 `MaybeOverOptimized4PlanCache(exprCtx, conds...)` 那样递归检查整个表达式树，扩展时必须单独评估。
- 字符串等非 NULL Datum：遵循 `ToBool`；例如 `"0"` 为假、`"1"` 为真，而 `"not-a-number"` 的转换错误不会被当作已证明为假（`expression_util_test.rs`）。

## 并发与资源生命周期

本文件没有锁、原子变量、通道、任务、事务、I/O 或显式资源释放。所有访问均为不可变借用；唯一临时资源是每次 `IsConstFalse` 调用克隆出的类型转换上下文，随函数返回释放。

并发安全取决于传入表达式及 `types::Context` 的实现，但本文件既不共享也不写入它们。计划生命周期影响主要体现在判定策略：参数或延迟常量可能跨缓存计划执行改变值，因此必须在计划构造期拒绝将其当作静态空结果证据。

## 与 Go 版本的对应关系

Go 对照文件是同目录 `expression_util.go`。核心语义保持一致：空条件不处理；比较结果恒 NULL 可触发 Dual；普通恒假只检查单条件；`Datum.ToBool` 成功且为 0 才算假；计划缓存相关值不能导致过度优化。

两侧结构差异如下：

- Go `Conds2TableDual(p, conds)` 接收 `base.LogicalPlan` 并直接创建、初始化、复制 schema 后返回计划；Rust `Conds2TableDual(conds)` 只返回 `bool`，由每个调用方按自身上下文构造 Dual 或写零统计。
- Go 从 `p` 获取表达式上下文和语句 `TypeCtxOrDefault()`；Rust 没有计划上下文参数，`IsConstFalse` 固定使用 `DefaultStmtNoWarningContext`。这可能影响依赖会话类型上下文、时区、SQL mode 或 warning 行为的转换，当前文档不声称二者在这些输入上完全等价。
- Go 在命中 NULL 后、以及单条件恒假检查前，对整个条件集合调用 `expression.MaybeOverOptimized4PlanCache`；Rust 仅检查当前表达式能否直接下转为带 `DeferredExpr`/`ParamMarker` 的 `Constant`。Go 的检查依赖缓存上下文并递归标量函数参数（对应 Rust 通用实现见 `pkg/expression/core_support.rs:464-477`），因此 Rust 本文件的保护范围更窄。
- Go 和 Rust 的 `expression::IsConstNull` 都只识别指定比较运算的第二参数为非延迟 NULL 常量，而不是任意 NULL 常量（Go `pkg/expression/util.go:2352-2370`；Rust `pkg/expression/util.rs:2066-2079`）。

## 扩展指南

新增或修改折叠规则时，最可能改动 `Conds2TableDual`；改变常量布尔语义时改动 `IsConstFalse`；扩大计划缓存保护范围时应优先复用或扩展 `expression::MaybeOverOptimized4PlanCache`，并考虑让调用方传入真实 `BuildContext`，避免在本文件复制递归逻辑。

安全扩展应保持以下约束：

1. 不把运行期、会话相关或缓存复用后可变的值当作编译期事实。
2. 明确多条件折叠是否仍需保留联合谓词形状，并与 Go 增量逐项核对。
3. 若改变返回信息，不要遗漏不同消费模式：计划替换、Selection 附着、DataSource 零统计及 WHERE 构建优化标志。
4. 测试逻辑继续放在独立文件，至少同步 `expression_util_test.rs`；涉及整个逻辑算子行为时同步 `logical_d_aster_unit_test.rs` 及具体调用方测试，不把测试内嵌进生产 `.rs`。

建议覆盖：空条件；单个 0/1/NULL/非法字符串；多个普通常量；比较表达式右侧 NULL；`DeferredExpr`；`ParamMarker`；嵌套可变常量；使用非默认类型上下文时与 Go 的差异。性能上该函数当前为单次线性扫描加至多一次布尔转换，新增递归检查会增加与表达式树大小相关的成本。

## 验证依据

已读取并交叉核对：

- 目标源码：`pkg/planner/core/operator/logicalop/expression_util.rs`（完整 64 行）。
- crate 边界与模块出口：`pkg/planner/core/operator/logicalop/Cargo.toml`、`lib.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/expression_util.go`、`pkg/expression/util.go`、`pkg/expression/constant.go`。
- Rust 下游语义：`pkg/expression/util.rs`、`pkg/expression/core_support.rs`、`pkg/types/context.rs`。
- Rust 调用点：`logical_selection.rs`、`logical_join.rs`、`logical_datasource.rs`、`logical_plans_misc.rs`、`pkg/planner/core/logical_plan_builder_runtime.rs`。
- 独立测试：`pkg/planner/core/operator/logicalop/expression_util_test.rs`、`logical_d_aster_unit_test.rs`。

RustCodeGraph 证据：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter` 确认目标文件含 5 个索引符号；`node --file` 读取了目标源码及主要调用点；`query` 分别定位 Rust/Go `Conds2TableDual`、`IsConstFalse` 以及 Rust 私有 `is_mutable_constant`。精确 `callers/callees` 没有输出函数级边，因此调用清单使用 `rg` 对图缺口补证，并由上述调用文件源码复核。

本任务是纯文档分析，按计划不运行 Cargo。结构验证要求目标文档存在，且上述 11 个固定二级标题各出现一次；人工复核重点是：职责边界、单条件/多条件分支、计划缓存保护、调用方改写方式及 Go/Rust 差异均有真实符号或文件依据。
