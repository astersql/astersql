# `pkg/expression/expression.rs`

## 文件定位

`expression.rs` 是 `astersql-expression` crate 的标量表达式核心协议与通用算法层。crate 根在 `pkg/expression/lib.rs` 中以 `#[path = "expression.rs"] mod expression_core` 挂载它，再用 `pub use expression_core::*` 向外暴露 API。因此，调用者通常以 `expression::Expression`、`expression::EvalExpr` 等 crate 根路径使用这些符号，而不直接引用 `expression_core`。

该文件位于 SQL 规划与执行共用的边界：上游规划器创建、改写、拆分或折叠表达式，下游执行器在 `chunk::Row` 或整个 `chunk::Chunk` 上求值。RustCodeGraph 将该文件标记为被 134 个文件使用；具体直接证据包括 `pkg/expression/chunk_executor.rs::vectorizedFilter`、`pkg/expression/aggregation/descriptor.rs::evalNullValueInOuterJoin4Count` 和 `pkg/planner/core/planbuilder_runtime.rs` 的 INSERT 目标表构建路径。

## 核心职责

- 定义表达式节点的共同协议：`VecExpr`、`Expression`、`SafeToShareAcrossSession`、`TraverseAction` 和 `ConstLevel`。具体的常量、列和标量函数实现在相邻文件中实现这些 trait。
- 在逐行与向量化路径之间分派求值，并保留 SQL NULL/三值逻辑：`EvalBool`、`VecEvalBool`、`EvalExpr`、`toBool` 和 `implicitEvalReal`。
- 组装与拆分逻辑谓词树：`ComposeCNFCondition`、`ComposeDNFCondition`、`Flatten*Conditions` 和 `Split*Items`。
- 为优化器执行空拒绝推导：`EvaluateExprWithNull` 将指定 Schema 中的列替换为 NULL，再重建并折叠标量函数。
- 把表元数据转换为规划器可用的 `Schema`/`FieldName`，同时构建唯一键信息并解析虚拟生成列：`TableInfo2SchemaAndNames` 与 `ColumnInfos2ColumnsAndNamesWithCollate`。
- 保持 expression 到 planner 的单向依赖边界：`InstallBuildSimpleExpr`/`BuildSimpleExpr` 通过 `OnceLock` 转发到规划器所有的 AST 改写实现，避免 expression crate 反向依赖 planner crate。

## 主要符号

- Hash 标记常量 `CONSTANT_FLAG`、`COLUMN_FLAG`、`SCALAR_FUNCTION_FLAG`、`PARAMETER_FLAG`、`SCALAR_SUB_Q_FLAG`、`CORRELATED_COLUMN_FLAG` 区分表达式节点编码；小写别名保留 Go 命名兼容面。
- `BuildOptions<'a>` 及 `WithTableInfo`、`WithInputSchemaAndNames`、`WithAllowCastArray`、`WithCastExprTo`、`WithUseNewCollate` 描述 AST 转表达式时的 Schema、名称、源表、数组 CAST、目标类型和新校对规则。`BuildOption<'a>` 是按顺序修改选项的闭包。
- `BuildSimpleExprFn`、`BUILD_SIMPLE_EXPR_FACTORY`、`InstallBuildSimpleExpr` 和 `BuildSimpleExpr` 定义一次安装的跨 crate 工厂。同一函数指针重复安装幂等，不同指针会返回错误。
- `VecExpr` 覆盖 Int、Real、String、Decimal、Time、Duration、JSON 和 VectorFloat32 的列式求值；`Expression` 在此基础上加入逐行求值、类型、克隆、相等性、相关性、常量等级、解相关、列索引解析、列重映射、解释文本、Hash 和内存估算。`as_any`/`as_any_mut` 是 Rust 中替代 Go 类型断言的 downcast 入口。
- `ConstLevel::{ConstNone, ConstOnlyInContext, ConstStrict}` 分别表示依赖输入行、仅在同一执行上下文中不变、以及跨上下文恒定，是常量折叠和计划缓存的契约。
- `CNFExprs` 包装 AND 谓词列表；`Assignment` 表示 UPDATE 列赋值并支持延迟错误；`VarAssignment` 保留 SET 语句的 default/global/instance/system 作用域标记。
- `TableInfo2SchemaAndNames`、`ColumnInfos2ColumnsAndNames*` 产生 `Column`、`NameSlice` 和 Schema keys；`NewValuesFunc`、`IsBinaryLiteral`、`PropagateType`、`Args2Expressions4Test` 和 `StringifyExpressionsWithCtx` 是组装、类型与测试/调试辅助入口。

## 执行流程

1. **表达式构建**：调用方组合 `BuildOption` 后调用 `BuildSimpleExpr`。该函数从 `BUILD_SIMPLE_EXPR_FACTORY` 读取规划器工厂；未安装时明确报错。`pkg/planner/core/lib.rs::InstallPlannerExpressionFactory` 安装 `PlannerBuildSimpleExpr`，再由 `pkg/planner/core/expression_rewriter.rs::buildSimpleExpr` 应用选项、验证 Schema/Name 长度、改写 AST，并按需追加 CAST。
2. **单行布尔过滤**：`EvalBool` 顺序求值 `CNFExprs`。普通 NULL 和 false 立即以 false 结束；来自 IN 子查询展开的 EQ 条件若产生 NULL，则先记录 `has_null`、继续检查后续合取项，最终返回 `(false, true)` 表示 unknown。
3. **批量布尔过滤**：`VecEvalBool` 先保存并清除 `Chunk` 原 selection，建立全行 `sel`。每个 CNF 项通过 `EvalExpr` 或 `implicitEvalReal` 写入临时列，`toBool` 把物理类型归一为 `-1/0/1`，然后逐项缩小 selection。成功或失败退出时都恢复原 selection，并处理临时缓冲。`pkg/expression/chunk_executor.rs::vectorizedFilter` 是直接消费者。
4. **按类型求值**：`EvalExpr` 仅在表达式声明 `Vectorized()` 且调用方开启向量化时分派到 `VecEval*`；否则循环 `Chunk` 中每行，调用对应 `Eval*` 并把值或 NULL 追加到结果列。
5. **谓词树处理**：`composeConditionWithBinaryOp` 以中点二分递归构建平衡 AND/OR 树，避免线性深度；`extractBinaryOpItems` 和 `splitNormalFormItems` 反向展开同名标量函数子树。
6. **空拒绝推导**：`EvaluateExprWithNull` 先根据参数表达式决定是否禁用计划缓存，再递归将 Schema 内列换成 NULL、折叠 deferred constant 并重建函数。空拒绝上下文会额外跟踪 NULL 是否来自被替换列，避免在 AND/OR 含未知非常量项时过早折叠。
7. **表元数据转换**：先为所有 `ColumnInfo` 创建列与名称，再用 mock Schema 解析虚拟生成列。最后 `TableInfo2SchemaAndNames` 仅把处于 public 状态、且所有索引列均 NOT NULL 的 unique index 记为 key；`PKIsHandle` 时还加入主键列。

## 数据与状态

- `ExprBox = Box<dyn Expression>` 是表达式树的所有权单元。对它的 `Clone` 调用虚函数 `CloneExpr`，因此是语义克隆而非仅复制 trait-object 指针。
- `Expression` 的 NULL 结果以 `(value, bool)` 中的布尔值表示，不依赖占位值的内容；`Eval` 则直接返回可表达 NULL 的 `Datum`。`VecEvalBool` 将中间布尔状态编码为 `i8` 的 `-1` (NULL)、`0` (false)、`1` (true)。
- `BUILD_SIMPLE_EXPR_FACTORY: OnceLock<BuildSimpleExprFn>` 是进程级、一次性可写的全局状态。`EvalSimpleAst` 仍是 `static mut Option<fn>`，访问时需由外部保证安全安装与读取时序；本文件未提供同等的同步包装。
- `BuildOptions` 借用 Schema、TableInfo 和目标 FieldType，其生命周期由 `BuildOption<'a>` 限定；`InputNames.Shallow()` 复制名称容器，FieldName 元素由相邻类型管理共享。
- `CNFExprs::Clone` 和当前 `Shallow` 都对每个表达式调用 `CloneExpr`。源码注释明确说明 Rust 版因 `Box` 所有权而暂以克隆模拟 Go 的浅拷贝，未使用 `Arc` 共享节点。
- `ColumnInfos2ColumnsAndNamesWithCollate` 为每列调用 `AllocPlanColumnID`，所以返回 Schema 的 `UniqueID` 依赖构建上下文的计数状态；虚拟列的 `VirtualExpr` 在存入前会针对 mock Schema 解析列索引。

## 依赖与调用关系

- **crate 边界**：`pkg/expression/Cargo.toml` 声明 crate 名为 `astersql-expression`、`autotests = false`，且通过 `lib.rs` 显式挂载测试。本文件借由 `use crate::*` 消费 chunk、types、parser AST/mysql/opcode、model、generatedexpr、exprctx 和 planner-base 再导出的类型。
- **上游构建**：`pkg/planner/core/lib.rs::InstallPlannerExpressionFactory` 将 `PlannerBuildSimpleExpr` 注册到本文件；`pkg/planner/core/expression_rewriter.rs::buildSimpleExpr` 是真正的 AST 改写实现。生成列路径从 `ColumnInfos2ColumnsAndNamesWithCollate` 再次回调 `BuildSimpleExpr`。
- **上游规划**：`pkg/expression/constant_propagation.rs`、`pkg/expression/util.rs`、`pkg/planner/core/operator/logicalop/*` 和 `pkg/util/ranger/detacher.rs` 使用 CNF/DNF 拆分与组装函数；`pkg/expression/aggregation/descriptor.rs` 用 `EvaluateExprWithNull` 推导外连接中聚合函数的 NULL 输入结果。
- **下游执行**：`pkg/expression/chunk_executor.rs::vectorizedFilter` 包装过滤器为 `CNFExprs` 并调用 `VecEvalBool`；`pkg/executor/internal/vecgroupchecker/vec_group_checker.rs` 直接调用 `EvalExpr` 生成分组检查列。
- **元数据消费**：`pkg/planner/core/planbuilder_runtime.rs` 在构建 INSERT 目标表时调用 `TableInfo2SchemaAndNames`；`pkg/planner/core/expression_rewriter.rs` 在仅提供源表而未提供 Schema 时调用 `ColumnInfos2ColumnsAndNames`。
- **内部依赖**：`NewFunction`/`NewFunctionInternal`、`FoldConstant`、`CanImplicitEvalReal`、`typeCtx`、`Schema`、`Column`、`Constant`、`ScalarFunction` 由 crate 内相邻模块提供；临时池和列分配器由 `pkg/expression/core_support.rs` 提供。

## 错误处理与边界

- 表达式求值、字符串到数值转换、二进制字面量转换、函数重建、生成列解析与索引解析均通过 `Result<_, errors::Error>` 立即向上传播。`EvalExpr`/`toBool` 对不支持的 `EvalType` 返回包含类型的明确错误。
- `BuildSimpleExpr` 在工厂尚未安装时返回 `BuildSimpleExpr factory is not installed`；已有不同工厂时返回 `a different BuildSimpleExpr factory is already installed`，不会覆盖全局函数指针。
- `EvalBool` 的 `(bool, bool)` 第二项是 NULL/unknown 标记，不是错误；IN 子查询等值条件的 NULL 延迟裁决是特殊分支，不能简化成“任一 NULL 即 false”。
- `toBool` 的 String 分支对 ENUM/SET/BIT 有特判：空字符串若是 ENUM/SET 的合法元素则按非零处理，BIT 按二进制字面量转整数，其他字符串走 `StrToFloat`。
- `TableInfo2SchemaAndNames` 不会把 nullable unique index 或非 public index 标记为 Schema key。虚拟列构建仅忽略重复的 truncate 告警，解析、名称解析、表达式构建与索引解析错误仍会返回。
- `NewValuesFunc` 认为 VALUES builtin 工厂必须已注册：工厂失败时记录错误并 panic，这是启动/注册不变量，而非用户输入错误路径。
- `Args2Expressions4Test` 是明确的测试辅助，只支持列出的 Datum kind；其他 kind 返回 `None`，不应用作生产类型推断入口。

## 并发与资源生命周期

- `installBuildSimpleExprWith` 先检查 `OnceLock::get`，再尝试 `set`。两个线程同时看到空锁时，落败者重读已安装指针：指针相同则两者都成功，不同则恰有一个返回冲突错误。`pkg/expression/extension_runtime_aster_unit_test.rs` 用 `Barrier` 验证了这两种竞态。
- `VecEvalBool` 在进入时复制 `Chunk` 原 selection，内部闭包不论返回成功或错误，闭包后的清理都会恢复 selection、处理 zero/selection 切片。这使调用方的 `Chunk` 视图不因求值失败而泄漏。
- 当前 Rust 资源池尚未与 Go 性能语义对齐：`pkg/expression/core_support.rs::DropPool::Put` 直接丢弃对象，`ColumnAllocator::get` 每次构造默认列，`put` 也丢弃。因而 `expressionSlices`/`selPool`/`zeroPool`/`globalColumnAllocator` 仅保留借还调用面，不应宣称已具备 Go `zeropool`/列分配器的内存复用效果。
- `deallocateSelSlice` 和 `deallocateZeroSlice` 仅在 capacity 不大于 `DEFAULT_CHUNK_SIZE` (1024) 时调用池归还面，避免未来真实池化时长期持有过大缓冲。
- `EvalSimpleAst` 的 `static mut` 未带锁；本文件不展示安装或并发读取约束，因此其线程安全性只能标记为由外部初始化时序保证，本件未独立验证。

## 与 Go 版本的对应关系

- 主要对照文件是 `pkg/expression/expression.go`。Hash 标记、`BuildOptions`、`VecExpr`、`Expression`、`ConstLevel`、CNF/DNF 辅助、标量/向量求值、空拒绝折叠、表元数据转换、VALUES 函数、类型传播和测试辅助的控制流均与 Go 同名符号对应。
- Go `Expression` 是 interface，Rust 使用组合多个 trait 的 `dyn Expression`；Go 类型断言换成 `Any` downcast，nil interface/指针换成 `Option` 或明确 NULL 布尔值，`Clone()` 换成 `CloneExpr()` 以避免与 Rust `Clone` 混淆。
- Go `BuildSimpleExpr` 是由 planner init 直接赋值的全局函数；Rust 因无 Go 式包 init，使用 `OnceLock` 和规划器公开入口的幂等安装。Rust `BuildOptions` 另有 `UseNewCollate`，并在生成列路径显式传给 `WithUseNewCollate`；当前 Go 结构中没有该字段，而是由 context 获取开关。
- Go `CNFExprs.Shallow` 复制 interface 切片但共享节点；Rust 当前与 `Clone` 一样逐节点 `CloneExpr`。逻辑结果一致，但所有权与分配成本不同。
- Go 用 `zeropool.New`、selection/zero pool 和全局 ColumnAllocator 复用内存；Rust 的相同名调用面在 `core_support.rs` 仍是丢弃式占位。这是已验证的性能/资源管理差异，不是本文档任务的修复范围。
- Go `ColumnInfos2ColumnsAndNames` 直接查询 context 校对开关；Rust 外层同名函数使用全局 `collate::NewCollationEnabled()`，内层 `ColumnInfos2ColumnsAndNamesWithCollate` 接收显式布尔值。扩展这条路径时应同时核对规划器传入的 `UseNewCollate`。
- Rust 独立测试 `pkg/expression/expression_test.rs` 目前直接覆盖 BuildOption、表达式切片容量、`Args2Expressions4Test` 和生成列 truncate context；Go `pkg/expression/expression_test.go` 还覆盖 `EvaluateExprWithNull`、`PropagateType`、`IsBinaryLiteral`、`NewValuesFunc` 及 `EvalExpr` 的标量/向量一致性。这些 Go 用例是 Rust 后续补齐回归面的直接语义依据。

## 扩展指南

- 新增表达式节点时，应在节点自己的生产文件完整实现 `Expression` 的标量求值、向量求值、克隆、Hash/等价、列解析、内存估算与 `as_any` 入口；不要把节点实现或测试内嵌进本文件。
- 新增物理求值类型时，必须同步更新 `VecExpr`、`Expression`、`EvalExpr`、`toBool` 及 chunk Column API，并验证 NULL、标量/向量一致性、不支持类型报错和 selection 恢复。测试应放在独立 `*_test.rs` 中，优先扩展 `pkg/expression/expression_test.rs` 或 `pkg/expression/chunk_executor_test.rs`。
- 修改 NULL/布尔语义时，必须同时处理 `EvalBool` 和 `VecEvalBool`，特别保留 IN 子查询 EQ 的 NULL 延迟裁决以及 ENUM/SET/BIT 的真值规则；建议在独立 Rust 测试中加入 Go `expression_test.go`/vectorized 测试的对齐用例。
- 扩展 `BuildOptions` 时，同步修改 `pkg/planner/core/expression_rewriter.rs::buildSimpleExpr` 的应用逻辑和规划器独立测试 `pkg/planner/core/expression_test.rs`。若改变工厂安装策略，必须保留同工厂幂等、异工厂拒绝的并发契约，并扩展 `extension_runtime_aster_unit_test.rs`。
- 修改 Schema 转换时，同时核对 unique/public/NOT NULL 键筛选、`PKIsHandle`、隐藏列、UniqueID 分配和虚拟生成列的 parse/resolve/index 三阶段。直接相关测试位于 `pkg/expression/expression_test.rs`、`pkg/planner/core/expression_test.rs` 和 Go `pkg/expression/expression_test.go`。
- 将占位池替换为真实复用实现时，修改点在 `pkg/expression/core_support.rs`，但必须回归本文件的所有早返回/错误路径，确保临时列、selection 和 zero slice 均被归还，且不保留过大分配。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/expression/expression.rs` 完整核对了 1–1197 行，并报告该文件被 134 个文件使用。
- 已读生产源：`pkg/expression/expression.rs`、`pkg/expression/lib.rs`、`pkg/expression/core_support.rs`、`pkg/expression/chunk_executor.rs`、`pkg/expression/aggregation/descriptor.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/expression_rewriter.rs`、`pkg/planner/core/planbuilder_runtime.rs`。
- 已读 crate/Go 对照：`pkg/expression/Cargo.toml` 和 `pkg/expression/expression.go`。Cargo 元数据明确 `go-package = "pkg/expression"`，且 `lib.rs` 将本文件挂载并公开再导出。
- 已读独立测试：`pkg/expression/expression_test.rs`、`pkg/expression/extension_runtime_aster_unit_test.rs` 和 Go `pkg/expression/expression_test.go`。Rust 测试证实选项应用、切片容量、测试 Datum 转换、truncate context 隔离及工厂并发安装；Go 测试提供空拒绝、类型传播和标量/向量结果一致性的对照语义。
- 已核对关键边：`PlannerBuildSimpleExpr -> InstallPlannerExpressionFactory -> InstallBuildSimpleExpr`；`ColumnInfos2ColumnsAndNamesWithCollate -> BuildSimpleExpr -> planner expression_rewriter::buildSimpleExpr`；`chunk_executor::vectorizedFilter -> VecEvalBool -> EvalExpr/implicitEvalReal -> Expression::{VecEval*, Eval*}`；`aggregation::descriptor -> EvaluateExprWithNull`；`planbuilder_runtime -> TableInfo2SchemaAndNames -> ColumnInfos2ColumnsAndNames`。
- 人工事实复核：本文档仅声明源码可见的当前行为；对 `EvalSimpleAst` 的并发安装以及占位资源池的性能效果未作未经验证的支持性声明。本任务是纯文档分析，按计划不运行 Cargo。
