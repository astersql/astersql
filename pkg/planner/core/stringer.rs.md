# `pkg/planner/core/stringer.rs`

## 文件定位

`stringer.rs` 是 `astersql-planner-core` crate 内的计划树文本化模块：`pkg/planner/core/lib.rs` 以私有 `mod stringer` 纳入它，再通过 `pub use stringer::*` 对 crate 调用者公开重导出。文件处在 SQL 规划主链的“观测/调试表示”边界，不构建、优化或执行计划。目前可确认的生产接入点是 `pkg/planner/core/optimizer.rs::DoOptimize`：物理优化和后优化完成后，它调用 `crate::ToString(&physical)` 填充 `Trace.final_plan`。

该文件只依赖同 crate 的 `JoinType`、`PlanKind` 和 `PlanNode`；它不直接依赖 `pkg/planner/core/Cargo.toml` 中的外部 crate，也没有 feature 条件编译分支。`Cargo.toml` 声明本 crate 的 Go 对照包为 `pkg/planner/core`，与同路径 `stringer.go` 一致。

## 核心职责

- `ToString` 把 `PlanNode` 树转为稳定的简短算子表示；一元链按“叶子 -> 根”排列，分支算子则把子树包在当前算子的 `{...}` 或 `(...)` 内。
- `FDToString` 仅摘取会维护函数依赖（FD）的算子，输出自底向上的 FD 链。
- `describe` 统一定义 `PlanKind` 到文本协议的映射，包括连接键、扫描范围、分区 ID、TopN 参数、MPP task ID 和 DML 子计划等载荷。
- 输出主要用于优化追踪和测试断言；源码顶部注释也将 EXPLAIN/调试/单测列为目标，但在本仓库 Rust 直接调用证据中，只确认了 `DoOptimize -> Trace.final_plan` 和独立测试，不应扩大声称已接入所有 EXPLAIN 路径。

## 主要符号

- `pub fn ToString(plan: &PlanNode) -> String`：公开的计划文本化入口。创建片段向量，委托 `toString`，最后用 `->` 连接。
- `pub fn FDToString(plan: &PlanNode) -> String`：公开的 FD 摘要入口。委托 `fdToString` 收集后反转数组，用 ` >>> ` 连接。
- `pub fn needIncludeChildrenString(plan: &PlanNode) -> bool`：当算子是 `PlanKind::UnionAll` 或有多个子节点时返回 `true`，决定子树是内嵌还是在外层链中展平。UnionAll 即使子节点少于两个也保留内嵌语义，对齐 Go 的特例。
- `fn fdToString(plan, output)`：对 `Projection` 和 `Aggregation` 记录当前 FD 并递归子节点；对 `DataSource`、`Apply`、`Join` 和 `UnionAll` 只记录当前 FD 就停止；其他算子不输出。
- `fn child_string(plan, separator)`：逐个对直接子节点调用 `ToString`，再用指定分隔符合并；`Sequence` 使用逗号，其他分支主要使用 `->`。
- `fn keys_string(keys)`：把等值键对无分隔地拼成 `(left,right)` 序列。
- `fn merge_join_name(join_type)`：将七种 `JoinType` 完整映射为 MergeJoin 显示名；该 `match` 是穷尽的。
- `fn describe(plan)`：生成单个节点的短描述。对已特化的 `PlanKind` 使用固定格式，其余变体通过 `PlanKind::name()` 回退，`Generic(name)` 直接返回自定义名称。
- `fn toString(plan, output)`：负责树遍历与顺序控制。`ExchangeReceiver` 不访问子节点；分支算子直接输出包含子树的 `describe` 结果；普通节点先递归子节点、后输出自身。

## 执行流程

1. 上游将已构建的 `PlanNode` 引用传给 `ToString`；例如 `DoOptimize` 在 `postOptimize` 之后传入最终物理计划。
2. `toString` 先检查 `ExchangeReceiver`。命中时只描述 receiver 自身并返回，不读取 `children`。
3. 否则，`needIncludeChildrenString` 识别 UnionAll 或多子节点算子。这类节点交给 `describe` 通过 `child_string` 递归生成内嵌子树，外层不再重复展开。
4. 普通单子节点先递归子树，再追加当前 `describe` 结果，因此一元链自然形成 `Table(t)->Sel(...)->Projection` 的自底向上顺序。
5. `describe` 根据 `PlanKind` 载荷生成节点文本。读取器、Join、DML 和 IndexMerge 类型会递归调用 `ToString`或 `child_string`；扫描、Selection、TopN 等直接格式化载荷。
6. `ToString` 用 `->` 合并顶层片段，并把完整字符串返回上游。

FD 路径独立于上述计划描述路径：`FDToString` 从根开始调用 `fdToString`，收集顺序是先父后子；入口在收集后执行 `reverse`，才得到子节点 FD 在前的输出。`Apply`/`Join`/`UnionAll` 等边界不继续向下遍历，这是明确的 Go 语义，而不是遗漏。

## 数据与状态

`stringer.rs` 本身没有全局或持久化状态。所有输入都是不可变的 `&PlanNode`，临时状态只是调用栈、`Vec<String>` 片段和新建的 `String`。函数不修改计划节点。

`PlanNode` 定义在 `pkg/planner/core/common_plans.rs`，字符串化直接观察 `kind`、`children` 和 `fd`。`describe` 还间接消费 `PlanKind` 变体中的表/索引名、range 字符串、等值键、表达式文本、offset/count、task ID 等数据。`PlanNode` 中的估算行数、代价、内存/磁盘统计等字段不在本文件的输出协议内。

重要格式不变量包括：顶层链分隔符是 `->`；FD 分隔符是 ` >>> `；Join 键对是连续的 `(l,r)`；TopN 的 `by_items` 以空格联接且不带 Rust debug 引号；ExchangeSender/Receiver 对每个 task ID 保留 `", "` 后缀，因而关闭括号前有逗号和空格。这些字符串是测试/追踪可观测合同，不是可任意美化的日志。

## 依赖与调用关系

- 模块边界：`pkg/planner/core/lib.rs` 声明 `mod stringer` 并 `pub use stringer::*`；同一文件以 `#[path = "stringer_test.rs"] mod stringer_test` 挂载独立测试。
- 上游生产调用：`pkg/planner/core/optimizer.rs::DoOptimize -> crate::ToString -> Trace.final_plan`。全仓库 Rust 直接调用搜索未找到 `FDToString` 的生产调用点；它目前由 crate 公开并被独立单元测试使用。
- 下游数据依赖：`pkg/planner/core/common_plans.rs::{PlanNode, PlanKind, JoinType}`。`PlanKind::name` 是未专门格式化变体的回退显示名来源。
- 内部调用链：`ToString -> toString -> describe`；`toString -> needIncludeChildrenString`；`describe -> child_string -> ToString`；`describe -> keys_string/merge_join_name`；`FDToString -> fdToString`。这些边由目标源码和 RustCodeGraph 节点查询交叉核对。
- crate 边界：`pkg/planner/core/Cargo.toml` 将库根设为 `lib.rs`、关闭自动测试发现（`autotests = false`），因此 `lib.rs` 中的显式测试模块声明是 `stringer_test.rs` 进入测试构建的必要接线。`nextgen` feature 不改变本模块。

## 错误处理与边界

公开入口返回 `String` 而非 `Result`，文件内也无显式错误分支。对未专门处理的 `PlanKind`，`describe` 通过 `PlanKind::name()` 返回类型名，避免空输出，但不携带该变体的额外载荷。

输入结构不满足隐含的计划约束时，该模块不会主动报错。例如 `HashJoin.inner_child` 只区分 `0` 与非 `0`；Join 键对已在 `Vec<(String, String)>` 中成对，因此不需要在此检查左右数组长度；DML 变体把 `children.is_empty()` 视为无选择子计划。这些条件由计划构建者维护，字符串化层只观察当前状态。

递归深度与计划树深度一致，本文件没有深度上限或环检测。`PlanNode` 通过值拥有 `Vec<PlanNode>`/装箱子计划，正常安全 Rust 构造不会形成自引用环；极深树的栈消耗仍是理论边界。

## 并发与资源生命周期

模块没有锁、原子变量、通道、异步任务、线程局部状态或 I/O。每次调用只读共享的 `PlanNode` 引用，并拥有自己的临时字符串和向量；因此并发调用之间没有本模块内的可变共享状态。是否能跨线程共享输入，最终由 `PlanNode` 及其字段的 `Send`/`Sync` 自动 trait 决定，本文件未手工声明或绕过该约束。

所有临时资源在函数返回时由 Rust 所有权机制释放。计划树越大，递归的 `ToString` 和反复字符串格式化的时间/分配成本越高；当前实现没有缓存、流式 writer 或输出长度限制。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/stringer.go`。顶层 API 同名：Go `ToString(base.Plan)`/`FDToString(base.LogicalPlan)` 对应 Rust `ToString(&PlanNode)`/`FDToString(&PlanNode)`。Rust 将 Go 的接口多态和具体逻辑/物理算子类型断言收敛为一个轻量 `PlanNode + PlanKind` 枚举，并把 Go 中的 `strs/idxs` 分支片段管理简化为 `needIncludeChildrenString + describe/child_string` 的直接递归。

已有独立 Rust 测试 `pkg/planner/core/stringer_test.rs` 证明以下对齐点：

- 一元计划自底向上使用 `->`，UnionAll 内嵌子计划。
- FD 输出仅包含指定算子，并在 `Apply`/`Join`/`UnionAll` 处记录当前 FD 后停止。
- TopN 集合使用 Go `%v` 风格的空格分隔、空 alias 回退到表名、IndexScan range 不带 Rust debug 引号。
- `ExchangeReceiver` 跳过子计划遍历，并保留 Go task 列表的尾逗号格式。

两者并非完整等价实现。Go 版本从 session expression context 格式化真实表达式，并覆盖 PointGet/BatchPointGet 等更多具体计划类型；Rust 当前使用 `PlanKind` 内预先字符串化的表达式/范围，未专门处理的变体只回退到 `name()`。Go 测试 `pkg/planner/core/stringer_test.go::TestPlanStringer` 还通过 SQL 构建和逻辑优化验证 Show extractor 格式；Rust 测试是直接构造 `PlanNode` 的单元级验证，尚不能代替这类 SQL 端到端覆盖。

## 扩展指南

- 新增或修改 `PlanKind` 时，先决定它是否需要载荷感知的稳定文本。若需要，在 `describe` 增加显式分支；否则确认 `PlanKind::name()` 回退足够。同步检查 `pkg/planner/core/common_plans.rs::PlanKind::name`。
- 新算子有多子树或特殊子树所有权时，需同时审查 `needIncludeChildrenString`、`toString` 和 `child_string`，避免子树丢失或重复输出。ExchangeReceiver 式的“不遍历子节点”必须有 Go 对照或计划语义证据。
- 扩展 FD 传递显示时，在 `fdToString` 明确选择“记录并递归”、“记录后停止”或“忽略”三种行为；不要默认所有节点都继续向下遍历。
- 修改格式时把输出当作兼容合同，同步更新独立的 `pkg/planner/core/stringer_test.rs`，并对照 `pkg/planner/core/stringer.go` 与 `stringer_test.go`。不要把 Rust 测试内嵌进生产文件。
- 建议至少覆盖：单子链、UnionAll 即使少于两个子节点的内嵌行为、多子 Join 键、空/非空载荷、特殊 receiver、DML 有/无子计划以及 FD 停止边界。
- 性能敏感的扩展应避免对同一大子树重复执行 `ToString`。如果改为 writer 或显式栈，必须先保留现有排序、分隔符和特殊分支格式，再用独立测试证明兼容。

## 验证依据

- RustCodeGraph 索引状态：项目索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；使用 `node --file pkg/planner/core/stringer.rs` 读取了目标文件全部 286 行。
- RustCodeGraph 符号节点：`stringer.rs::ToString` （第 25 行）、`FDToString` （第 32 行）、`needIncludeChildrenString` （第 41 行）、`fdToString` （第 46 行）、`describe` （第 92 行）和 `toString` （第 271 行）。精确 `callers/callees` 查询未返回边，因此依技能回退规则用源码搜索核实调用点，没有把同名符号当作本模块证据。
- 数据结构证据：RustCodeGraph `node --file pkg/planner/core/common_plans.rs` 核对 `JoinType`、`PlanKind`、`PlanKind::name` 和 `PlanNode`；`PlanNode` 的直接相关字段是 `kind`、`children` 和 `fd`。
- 上游证据：RustCodeGraph 节点读取和 `rg` 都定位到 `pkg/planner/core/optimizer.rs:52`，其在 `DoOptimize` 中设置 `trace.final_plan = crate::ToString(&physical)`。
- crate/模块证据：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`；后者的 `mod stringer`、`pub use stringer::*` 和 `#[path = "stringer_test.rs"]` 分别证明编译、公开导出与独立测试接线。
- Go 对照证据：`pkg/planner/core/stringer.go`、`pkg/planner/core/stringer_test.go::TestPlanStringer`。Rust 独立测试证据：`pkg/planner/core/stringer_test.rs` 的五个 `#[test]` 用例。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时用任务指定的结构命令确认文件存在且恰有 11 个固定二级标题，并人工复核本文能回答文件为何存在、如何运行以及如何安全扩展。
