# `pkg/planner/core/trace.rs`

## 文件定位

该文件属于 `astersql-planner-core` crate：`pkg/planner/core/Cargo.toml` 将 `lib.rs` 设为库入口，`pkg/planner/core/lib.rs` 以私有模块 `mod trace;` 装载它，再通过 `pub use trace::*;` 将 `Trace` 暴露到 crate 根。文件不受 feature 或条件编译控制，默认构建与 `nextgen` feature 下都会参与编译。

它是紧凑计划模型优化链的诊断数据容器，而不是优化算法本身。直接生产者是 `pkg/planner/core/optimizer.rs::DoOptimize`：该函数创建 `Trace::default()`，把同一个可变引用依次交给逻辑优化和物理优化，最后写入最终计划文本，并把轨迹与物理计划、总代价一起返回。

需要特别区分两条同名路径：`pkg/planner/core/trace.go::Trace` 是 SQL `TRACE` 语句的计划节点，持有语句、解析上下文、格式与 `OptimizerTrace` 选项；本文件的 Rust `Trace` 只保存紧凑优化器的三个字符串字段，不是该 Go 计划节点的逐字段移植。

## 核心职责

- `Trace` 汇总一次 `optimizer.rs::DoOptimize` 调用产生的可读诊断信息：实际启用的逻辑规则、选用的物理优化路线、最终物理计划摘要。
- `AppendLogical` 与 `AppendPhysical` 提供只追加的写入口，并接受任意 `Into<String>`，让调用者可传入 `&str` 或已拥有的 `String`。
- `Default` 提供空轨迹：两个列表为空且 `final_plan` 为空字符串；`Clone`、`Debug`、`Eq`、`PartialEq` 使轨迹可复制、调试打印和做精确断言。
- 本文件不决定是否开启追踪。虽然 `optimizer.rs::OptimizeOptions` 含 `trace: bool`，当前紧凑 `DoOptimize` 无条件创建和填充 `Trace`，且未读取该布尔字段；这是当前代码事实，不应把它描述为按需追踪开关。

## 主要符号

- `pub struct Trace`（`trace.rs:23`）：公开值类型。
  - `pub logical: Vec<String>`：按执行顺序保存已启用且未禁用的逻辑规则名。
  - `pub physical: Vec<String>`：按执行顺序保存物理优化步骤；当前生产调用只追加一次 `"cascades"` 或 `"volcano"`。
  - `pub final_plan: String`：后优化完成后由 `crate::ToString(&physical)` 生成的最终计划摘要。
- `Trace::AppendLogical(&mut self, rule: impl Into<String>)`（`trace.rs:33`）：将规则名转换为拥有所有权的字符串并追加到 `logical` 尾部，无去重、过滤或容量上限。
- `Trace::AppendPhysical(&mut self, step: impl Into<String>)`（`trace.rs:37`）：以相同方式追加物理步骤到 `physical` 尾部。

文件没有常量、枚举、trait、条件编译项或内部私有辅助函数。两个方法采用 Go 风格的大写名称；修改命名会影响 `optimizer.rs` 的直接调用点。

## 执行流程

1. `pkg/planner/core/optimizer.rs::DoOptimize` 在入口创建空的 `Trace`。
2. `logicalOptimize` 按固定规则数组顺序检查 `flag` 位掩码和 `disabled_rules`。仅当某一位启用且规则未被禁用时，先调用 `AppendLogical`，再调用 `normalize`。因此 `logical` 记录的是被调度的规则，而不保证规则实际改变了计划。
3. 若禁用笛卡尔积且逻辑计划中存在笛卡尔积，`DoOptimize` 在物理阶段前返回错误；此时局部轨迹随错误路径丢弃，调用者拿不到部分记录。
4. `physicalOptimize` 先依据 `OptimizeOptions.cascades` 调用 `AppendPhysical("cascades")` 或 `AppendPhysical("volcano")`，随后执行物理化。当前两个标签只标记选择，紧凑实现仍走同一个 `physicalize` 函数。
5. `postOptimize` 完成连续 Selection 合并、空 UnionScan/Lock 消除、可选并行 Apply、运行时过滤器生成和探测次数传播；这些后优化动作当前不会逐项进入 `physical`。
6. `DoOptimize` 计算总代价，再将后优化后的物理树序列化到 `final_plan`，最后返回 `(physical, cost, trace)`。

## 数据与状态

`Trace` 完全拥有三个字段的数据，不借用规划上下文或计划树。两个 `Vec<String>` 保留追加顺序并允许重复；字段公开，因此 crate 外调用者也可以直接读取、替换或清空内容，类型本身不维护“必须先逻辑、再物理、最后计划”的状态机不变量。

`Default` 派生语义是确定的空值，`Eq`/`PartialEq` 比较三个字段的全部内容和列表顺序。`AppendLogical`、`AppendPhysical` 的 `Into<String>` 在传入 `String` 时可移动已有缓冲区，在传入 `&str` 时分配拥有所有权的字符串。没有截断、脱敏或内存预算；记录量由调用次数和文本长度决定。

`final_plan` 不在本文件的方法中维护，而由 `optimizer.rs::DoOptimize` 在全部后优化之后直接赋值。这意味着直接构造或单独调用追加方法得到的 `Trace` 可以合法地保持空 `final_plan`。

## 依赖与调用关系

上游装配关系为 `pkg/planner/core/lib.rs -> mod trace -> pub use trace::*`，所以内部代码可写 `crate::Trace`，外部依赖该 crate 的代码也能访问该类型。RustCodeGraph 将目标文件识别为 4 个符号，并显示文件级使用者包括 `pkg/planner/core/optimizer.rs`；精确源码检索确认该文件定义的方法仅由 `optimizer.rs::logicalOptimize` 和 `optimizer.rs::physicalOptimize` 调用。

核心调用边如下：

- `optimizer.rs::DoOptimize -> Trace::default`；
- `optimizer.rs::DoOptimize -> logicalOptimize -> Trace::AppendLogical`；
- `optimizer.rs::DoOptimize -> physicalOptimize -> Trace::AppendPhysical`；
- `optimizer.rs::DoOptimize -> crate::ToString -> Trace.final_plan`；
- `optimizer.rs::DoOptimize -> caller`，通过返回三元组把轨迹交还调用者。

本文件只依赖 Rust 标准库的 `Vec`、`String`、`Into` 和派生 trait，不直接使用 `Cargo.toml` 中的第三方或工作区依赖。RustCodeGraph 对常见名 `Trace` 的全仓查询包含大量错误包装等同名符号，不能作为本类型调用边；上述关系以路径限定的文件源码与精确符号查询为准。

## 错误处理与边界

两个追加方法均无返回值且没有显式错误分支。潜在字符串分配失败遵循 Rust 分配器的进程级失败行为，不会转换为 `Result`。空字符串、重复名称和任意文本都会原样保存。

错误边界位于调用者：`logicalOptimize` 或 `physicalOptimize` 返回 `Err(String)`、以及笛卡尔积检查失败时，`DoOptimize` 使用 `?` 或直接返回错误，不返回已积累的 `Trace`。因此该类型目前不能用于观察失败到一半的优化过程。最终计划字符串只在所有可失败优化阶段和后优化完成后写入；失败结果中不存在“部分 final plan”。

本文件不验证规则名与真实规则集合的一致性，也不保证 `physical` 标签对应不同实现。扩展时若把轨迹用于机器解析，应先建立稳定 schema，而不能依赖当前自由文本。

## 并发与资源生命周期

`Trace` 不包含锁、原子变量、通道、任务、文件句柄、事务或引用计数。它作为 `DoOptimize` 栈上的独占可变值创建，在逻辑/物理阶段以 `&mut Trace` 串行借用，返回时整体移动给调用者；离开作用域后由 `Vec<String>`/`String` 自动释放。

字段只由普通可变引用修改，不提供内部同步。类型所含数据本身可随 Rust 自动 trait 在满足编译器规则时在线程间移动或共享，但并发写入仍必须由调用者提供互斥，不能从 `Clone` 推导出共享状态：克隆会复制轨迹值，之后两份记录相互独立。

## 与 Go 版本的对应关系

同路径 `pkg/planner/core/trace.go::Trace` 与本类型只有名称和“trace”领域概念相同，职责不同：Go 类型嵌入 `physicalop.SimpleSchemaProducer`，保存 `ast.StmtNode`、`resolve.Context`、输出格式、优化器追踪标志和目标，用于 `planbuilder.go::buildTrace` 构造 SQL `TRACE` 计划；Rust 类型不含计划节点能力，也没有这些字段。

Go 的真实优化主链位于 `pkg/planner/core/optimizer.go`：`doOptimize` 根据会话配置选择 `CascadesOptimize` 或 `VolcanoOptimize`，返回逻辑计划、物理计划、代价和错误；Go 的会话级优化追踪状态则由 `pkg/sessionctx/stmtctx/stmtctx.go` 中的 `EnableOptimizeTrace`/`OptimizeTracer` 与 `pkg/util/tracing/opt_trace.go::OptimizeTracer` 承载。Rust 本文件把规则名、路线标签和最终计划摘要集中在一个返回值中，是紧凑优化器的简化诊断模型，不能宣称已对齐 Go SQL `TRACE PLAN` 的输出、上下文或错误语义。

现有独立 Rust 测试 `pkg/planner/core/optimizer_test.rs` 以 `DoOptimizeCompact` 调用该紧凑优化链，验证并行 Apply 和探测次数传播，但将第三个返回值写成 `_`，未断言 `logical`、`physical` 或 `final_plan`。同目录未发现 `trace_test.rs`；Go 侧同路径也没有独立 `trace_test.go`。

## 扩展指南

- 若新增逻辑规则轨迹，在 `optimizer.rs::logicalOptimize` 实际调度规则的分支调用 `AppendLogical`，并明确记录的是“尝试执行”还是“确实改变计划”；两种语义不可混用。
- 若新增物理阶段或后优化明细，在对应阶段完成点调用 `AppendPhysical`。要保持时间顺序，并评估长计划或高规则数下的内存增长。
- 若让 `OptimizeOptions.trace` 真正控制采集，需要同时修改 `optimizer.rs::DoOptimize` 及其下游签名/分支，并决定关闭时是否仍生成 `final_plan`，避免只关闭列表而保留昂贵序列化。
- 若需要失败轨迹，现有 `Result<(PlanNode, f64, Trace), String>` 无法携带部分值；应先设计错误返回结构，再同步所有调用方，不能在本类型内静默吞错。
- 若目标是对齐 Go 的 SQL `TRACE` 计划节点，应修改 Rust 的计划构建/执行链，而不是把字段直接塞入这个优化轨迹容器。
- 测试应继续放在独立文件。优先扩展 `pkg/planner/core/optimizer_test.rs`，至少覆盖：规则位与禁用集合决定的顺序、Cascades/Volcano 标签、后优化后 `final_plan` 非空且与返回计划一致、错误时轨迹是否可见的约定；若测试聚焦本值类型，可新建并在 `lib.rs` 的 `#[cfg(test)]` 区域声明 `trace_test.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/planner/core/trace.rs` 确认目标文件含 4 个符号。
- RustCodeGraph 源码/符号查询：`node --file pkg/planner/core/trace.rs`、`query AppendLogical`、`query AppendPhysical`、`node AppendLogical`、`node AppendPhysical`，确认类型字段及两个追加方法；方法级 callers/callees 未返回边，因此用精确源码检索补证。
- RustCodeGraph 调用链源码：`node --file pkg/planner/core/optimizer.rs`，确认 `DoOptimize`、`logicalOptimize`、`physicalOptimize`、后优化及最终计划赋值；`node --file pkg/planner/core/optimizer_test.rs`，确认独立 Rust 测试仅间接覆盖优化入口而未校验轨迹值。
- crate/模块证据：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`，确认 crate 名、入口、feature、模块装载和公开再导出。
- Go 对照证据：`pkg/planner/core/trace.go`、`pkg/planner/core/planbuilder.go::buildTrace`、`pkg/planner/core/optimizer.go::{doOptimize,CascadesOptimize,VolcanoOptimize}`、`pkg/sessionctx/stmtctx/stmtctx.go`、`pkg/util/tracing/opt_trace.go`。
- 测试检索：对 `pkg/planner/core` 和 `pkg/planner` 中独立 Rust/Go 测试检索 `Trace::default`、`AppendLogical`、`AppendPhysical`、`final_plan` 与优化入口，未发现直接字段断言或同名独立 trace 测试。
- 本任务是纯文档分析，按计划未运行 Cargo；交付前以任务指定命令校验固定的 11 个二级章节。
