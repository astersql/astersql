# `pkg/planner/util/funcdep_misc.rs`

## 文件定位

本文件属于 `astersql-planner-util` crate（见 `pkg/planner/util/Cargo.toml`），位于逻辑计划与表达式/函数依赖实现之间：它把过滤条件中的语义事实转换成 `funcdep::FDSet` 能消费的列 ID 集合。`pkg/planner/util/lib.rs` 将 `funcdep_misc` 声明为私有模块，再通过 `pub use funcdep_misc::*` 导出三个公开提取函数；内部的 `hashCodeKey` 仅在 crate 内可见。

当前 Rust 主链的直接使用点是 `pkg/planner/core/operator/logicalop/logical_selection.rs` 的 `LogicalSelection::ExtractFD`。该调用者先继承子计划的 FD，调用本文件提取非空列、常量列和等价对，再依次执行 `MakeNotNull`、`AddConstants`、`AddEquivalence`，最后把 FD 投影到 Selection 的输出列。Go 版本还由 DataSource、Selection 和 Join 的 `ExtractFD` 路径调用；不能据此推断这些 Rust 算子均已完成同样接线。

## 核心职责

- `ExtractNotNullFromConds`：逐条件、逐列验证 null-reject 性质，把“该条件成立时必定非 NULL”的列 `UniqueID` 汇总为集合。
- `ExtractConstantCols`：识别与常量或关联列相等的列/标量表达式；列直接使用现有 ID，标量表达式则在 `FDSet` 中取得稳定的虚拟列 ID。
- `ExtractEquivalenceCols`：识别 `=`、`<=>` 以及受限的单值 `IN` 两端，把每端映射为单元素 ID 集合，供调用者建立等价关系。
- `uniqueIDForExpression` 与 `hashCodeKey`：共同维持“同一表达式哈希复用同一计划列 ID”的不变量，并把任意哈希字节无损编码为 Rust 可用的 UTF-8 `String` 键。

本文件只抽取事实，不直接把事实写成 FD 边，也不负责闭包、投影或算子间传播；这些操作由 `funcdep::FDSet` 及上层 `ExtractFD` 完成。

## 主要符号

- `pub fn ExtractNotNullFromConds(conditions: &[expression::ExprBox], context: &dyn plan_base::PlanContext) -> intset::FastIntSet`：读取条件中出现的普通列，为每一列构造单列 `Schema`，并通过 `IsNullRejected` 判断该列是否应加入结果。
- `pub fn ExtractConstantCols(conditions, context, dependencies) -> FastIntSet`：调用 `expression::ExtractConstantEqColumnsOrScalar`；`Column` 使用 `UniqueID`，`ScalarFunction` 交给 `uniqueIDForExpression`。传入可变 `FDSet` 是因为首次遇到标量表达式时要登记哈希到 ID 的映射。
- `pub fn ExtractEquivalenceCols(conditions, context, dependencies) -> Vec<[FastIntSet; 2]>`：调用 `expression::ExtractEquivalenceColumns`，只接受长度恰为 2 的分组，并把左右两端分别封装为单元素集合。固定长度数组明确表达了调用者所需的二元关系。
- `fn uniqueIDForExpression(context, dependencies, object) -> i32`：列直接返回 `UniqueID`；其它表达式以 `HashCode` 查 `FDSet::IsHashCodeRegistered`，命中则复用，未命中则用 `AllocPlanColumnID` 分配并经 `RegisterUniqueID` 登记。
- `pub(crate) fn hashCodeKey(hash_code: &[u8]) -> String`：把每个字节编码成两个小写十六进制字符。输出长度恒为输入的两倍，不会因非法 UTF-8 替换而合并不同哈希。

文件中没有自定义类型、trait、模块级可变状态或条件编译项。公开 API 是前三个提取函数；后两个函数是实现细节，其中 `hashCodeKey` 的 `pub(crate)` 仅用于同 crate 独立测试。

## 执行流程

1. `LogicalSelection::ExtractFD` 克隆条件及当前 FD 集合，并取得 `PlanContext`（`pkg/planner/core/operator/logicalop/logical_selection.rs`）。
2. 非空提取遍历每个条件。`ExtractColumnsMapFromExpressions` 去重取得条件内的普通列；每列单独构造 `Schema`，再让 `IsNullRejected` 对完整条件做否定下推、NULL 替换与证明。证明成立才插入该列 ID。
3. 常量提取由 `expression::ExtractConstantEqColumnsOrScalar` 识别 `column/scalar = constant/correlated-column`、`<=>`，以及右侧常量全部相同的 `IN`。普通列直接入集；标量函数经哈希注册后入集。
4. 等价提取由 `expression::ExtractEquivalenceColumns` 识别 `=`、`<=>` 或恰有两个参数的 `IN`。两端必须是列或标量函数，且至少一端是列；本文件将每端转成 ID，再生成 `[left_set, right_set]`。
5. 上层把三个结果分别传给 `FDSet::MakeNotNull`、`AddConstants` 和 `AddEquivalence`，然后 `ProjectCols`；因此本文件输出的是中间事实，不是最终 FD 集合。

同一非列表达式在常量提取与等价提取中出现时，`uniqueIDForExpression` 先查询 `HashCodeToUniqueID`，从而复用同一 ID。首次注册才消耗新的计划列 ID。

## 数据与状态

输入条件使用共享拥有的 `ExprBox`；本文件只克隆表达式句柄或列值，不改写原条件。三个公开结果都按列的 `i32` ID 表示：非空与常量是 `FastIntSet`，等价关系是二元单元素集合列表，因此天然去除单个集合内的重复 ID。

唯一持久化副作用是 `uniqueIDForExpression` 修改传入 `FDSet::HashCodeToUniqueID`。`FDSet::RegisterUniqueID` 对空键不登记，并通过 `entry(...).or_insert(...)` 保留首次绑定；本文件生成的十六进制键对非空哈希一定非空。列本身不进入该映射，因为已有稳定的 `Column::UniqueID`。

虚拟 ID 来自 `context.GetExprCtx().AllocPlanColumnID()`，随后以 `as i32` 转换。底层标准分配器是原子、单调递增的 `i64` 计数器（`pkg/expression/exprctx/context.rs`），但本文件与 `FDSet` 的 ID 类型均为 `i32`；极端越界时 Rust 转换会截断，这是当前接口边界而非本文件额外检查的约束。

## 依赖与调用关系

crate 边界由 `pkg/planner/util/Cargo.toml` 明示：本文件直接依赖 `expression`、`funcdep`、`intset` 和 `plan-base`；`lib.rs` 还通过同 crate 的 `IsNullRejected` 再导出连接到 `null_misc.rs`。

已核实的 Rust 上游边为：

- `LogicalSelection::ExtractFD` → `ExtractNotNullFromConds` / `ExtractConstantCols` / `ExtractEquivalenceCols`（`pkg/planner/core/operator/logicalop/logical_selection.rs`）。

主要下游边为：

- `ExtractNotNullFromConds` → `expression::ExtractColumnsMapFromExpressions`、`expression::NewSchema`、`IsNullRejected`、`FastIntSet::Insert`。
- `ExtractConstantCols` → `expression::ExtractConstantEqColumnsOrScalar` → `uniqueIDForExpression`（仅标量函数）。
- `ExtractEquivalenceCols` → `expression::ExtractEquivalenceColumns` → `uniqueIDForExpression`（左右两端）。
- `uniqueIDForExpression` → `Expression::HashCode`、`hashCodeKey`、`FDSet::IsHashCodeRegistered`、`BuildContext::AllocPlanColumnID`、`FDSet::RegisterUniqueID`。

RustCodeGraph 的 `query` 确认了三个公开函数在 `funcdep_misc.rs` 和 `funcdep_misc.go` 中的双版本定义，也确认了两个表达式提取函数的 Rust/Go 定义。当前索引的 `callers`/`callees` 对这些 Rust 符号没有给出可用边，因此上述调用边由实际 Rust 调用点和被调用实现补充核验，而非声称来自完整图遍历。

## 错误处理与边界

本文件不返回 `Result`，无法识别的表达式通常被下游提取器忽略：非标量顶层条件不会形成常量或等价项；常量提取只输出列/标量函数；等价提取要求合法运算符、参数形态以及至少一个列端。`ExtractEquivalenceCols` 还防御性跳过长度不为 2 的分组，避免索引越界。

null-reject 是按“条件中的每一列 + 完整条件”验证，而不是仅凭语法出现就判非空。其精度与安全性由 `null_misc.rs::IsNullRejected` 决定；例如不能证明时返回 false，只会少补充优化事实，不应制造错误的非空事实。

表达式身份依赖 `Expression::HashCode`。`hashCodeKey` 用十六进制保留所有字节，包括非法 UTF-8；相关测试专门确认 `[0x80]` 与 `[0x81]`、`[0xff, 0x00]` 与 `[0xfe, 0x00]` 不会碰撞。真正的哈希算法碰撞仍受各表达式 `HashCode` 合约约束，本文件不做结构相等二次验证。

## 并发与资源生命周期

函数本身不启动线程、任务、通道或事务，也不持有锁和外部资源。临时 `Schema`、表达式列表、哈希字符串和结果集合均由单次调用拥有，返回或离开作用域后按 Rust 所有权规则释放。

计划列分配器的标准实现使用 `AtomicI64`，可并发分配 ID；但 `ExtractConstantCols` 和 `ExtractEquivalenceCols` 要求对 `FDSet` 的独占 `&mut` 借用，哈希注册与集合构造在该调用内串行完成。因此不能把同一个 `FDSet` 同时传给多个线程调用这些函数，除非上层另行同步并满足 Rust 的借用/线程安全约束。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/util/funcdep_misc.go`。三个公开函数的目的与处理顺序保持一致：逐列做 null-reject；抽取常量列/标量；抽取等价两端；非列表达式通过哈希在 FDSet 中复用或分配 ID。Rust 的 `PlanContext` 参数替代 Go 中 `LogicalPlan`/`PlanContext` 的接口传递，计划列分配则从 `GetExprCtx()` 取得，而 Go 从 SessionVars 分配。

Rust 针对类型和内存模型做了三处显式化：等价结果用 `[FastIntSet; 2]` 表示固定二元关系；`pair.len() != 2` 时安全跳过；任意 Go `string(x.HashCode())` 字节键改为十六进制 UTF-8 字符串。十六进制会改变键的文本表示，但保持字节序列的一一对应关系，符合 FDSet 只需要稳定映射键的用途。

Go 注释明确指出当前常量抽取尚不能从组合约束（如 `x <= 1 AND x >= 1`）推导常量；Rust 复用相同形态的表达式提取器，也没有在本文件补充这种区间推理。Go 的 DataSource/Join/Selection 均有调用，Rust 搜索到的直接调用目前只有 Selection；这是接线覆盖差异，不应在本文档任务中扩张实现范围。

## 扩展指南

- 若增加可形成常量或等价关系的谓词形态，优先修改 `pkg/expression/util.rs` 的 `ExtractConstantEqColumnsOrScalar` / `ExtractEquivalenceColumns`，再确认本文件的类型分派仍能为所有返回类型生成稳定 ID；同步独立测试，而不要把测试嵌入生产文件。
- 若修改表达式虚拟 ID 策略，必须保持同一 `HashCode` 在同一 `FDSet` 内复用、不同原始哈希字节不因字符串转换合并，并同步 `pkg/planner/util/funcdep_misc_test.rs`。还应检查 `FDSet::RegisterUniqueID`/`IsHashCodeRegistered` 的键合约。
- 若扩展 null-reject 规则，应在 `pkg/planner/util/null_misc.rs` 及其独立测试中完成证明逻辑；本文件只负责逐列调用并汇总 ID。
- 若把这组事实接入新的逻辑算子，调用顺序应参考 `LogicalSelection::ExtractFD`：基于子计划 FD 提取并写入事实，最后按输出 schema 投影。必须同时核对对应 Go 算子的增量，避免把尚未移植的完整子系统纳入单次改动。
- 性能上，非空提取会对每个条件中的每个去重列运行一次 null-reject 证明；新增更复杂规则时应避免重复遍历或不必要克隆。兼容性上尤其要防止错误扩大常量、等价或非空集合，因为错误 FD 会允许上层执行不安全的优化。

## 验证依据

- 源文件：`pkg/planner/util/funcdep_misc.rs`；确认三个公开函数、两个内部辅助函数、可见性、数据流与无条件编译项。
- crate/模块：`pkg/planner/util/Cargo.toml`、`pkg/planner/util/lib.rs`；确认 crate 名、直接依赖、模块私有声明、公开再导出及独立测试挂载。
- Rust 上下游：`pkg/planner/core/operator/logicalop/logical_selection.rs`、`pkg/expression/util.rs`、`pkg/planner/util/null_misc.rs`、`pkg/planner/funcdep/fd_graph.rs`、`pkg/expression/exprctx/context.rs`；确认调用位置、谓词形态、FD 哈希注册和 ID 分配语义。
- Go 对照：`pkg/planner/util/funcdep_misc.go`，以及其中函数在 `logical_datasource.go`、`logical_selection.go`、`logical_join.go` 的调用搜索结果；确认原始算法与当前 Rust 接线差异。
- 测试：`pkg/planner/util/funcdep_misc_test.rs`；当前直接覆盖 `hashCodeKey` 对任意字节的区分能力。`pkg/planner/funcdep/extract_fd_test.rs` 说明了完整 LogicalPlan FD 链路测试受 crate 依赖环限制，并以 FDSet API 序列验证其它算子传播；它不是本文件三个提取函数的直接单元覆盖。
- RustCodeGraph：`status` 显示索引含 Rust/Go 文件；`query` 找到三个公开函数及两个 expression 提取函数的 Rust/Go 符号。文件过滤和调用边查询没有返回可靠边，故调用关系又以源码搜索和局部实现读取验证。
- 按任务约束未运行 Cargo；最终仅执行文档固定十一章节的结构验证，并人工复核没有把 Go 调用点误写成已存在的 Rust 接线。
