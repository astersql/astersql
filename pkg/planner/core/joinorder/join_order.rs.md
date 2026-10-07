# `pkg/planner/core/joinorder/join_order.rs` 逻辑说明

## 文件定位

本文件属于独立 crate `astersql-planner-core-joinorder`，由同目录 `Cargo.toml` 的 `[lib] path = "lib.rs"` 装配，并由 `lib.rs` 公开为 `join_order` 模块。它在该 crate 内位于 `conflict_detector.rs`（把逻辑连接树拆成顶点和约束边）与 `util.rs`（自包含的 `PlanNode`、连接类型及 hint 数据）之上，负责从合法候选中选择连接树。

当前 Rust 接线范围需要特别区分：根 `Cargo.toml` 将该 crate 注册为 `facade_planner_core_joinorder`，`pkg/lib.rs` 再导出它，`pkg/planner/core/rule/Cargo.toml` 也声明依赖；但生产 Rust 源码中没有 `JoinOrder::optimize` 的调用点。现有直接使用者是 `join_order_test.rs` 和 `bitset_bench_test.rs`。因此它是可执行、受测试的局部移植实现，但尚不能表述为已经接入完整 SQL 优化主链。

文件第 16—395 行是整个块注释中的 Go 风格迁移草案，不参与 Rust 编译。真正的 Rust 实现从第 396 行开始；分析和扩展必须以这一部分为准。

## 核心职责

- `JoinOrder::optimize` 调用 `ConflictDetector::build` 将输入 `PlanNode` 构造成叶节点集合与冲突边集合，按 `dp_threshold` 在精确子集 DP 和贪心近似之间切换。
- `optimize_dp` 为每个可达顶点子集保留累计代价最低的合法 `Node`；全覆盖方案不可得时，调用 `make_bushy_cartesian` 将剩余森林按固定顺序拼接。
- `optimize_greedy` 最多比较两个起点，通过 `greedy_connect` 反复选择当前节点的最低成本邻居，然后把残余森林拼成 bushy tree。
- `apply_cartesian_factor` 给无等值边/笛卡尔连接施加代价惩罚，并拒绝非法代价；`significantly_less` 用相对与绝对容差稳定候选选择。
- 无论走哪条路径，公开入口在返回前都以 `ConflictDetector::has_remaining_edges` 验证所有有语义的冲突边已经消费。

它不负责 Go 版本中的连接组抽取、递归遍历完整逻辑计划、LEADING hint 树构造、会话警告和输出 schema 恢复；这些能力只出现在文件顶部的注释草案或相邻模块中，不是当前可执行 `JoinOrder` API 的行为。

## 主要符号

- `pub struct JoinOrder`：无内部可变状态的配置对象。`dp_threshold` 决定算法分支，`cartesian_factor` 控制无等值连接的代价，`vertex_hints: BTreeMap<usize, JoinMethodHint>` 按顶点 ID 传递连接算法偏好。
- `impl Default for JoinOrder`：默认 DP 阈值为 10，笛卡尔因子为 10,000，hint 表为空。
- `pub fn JoinOrder::optimize(&self, root: PlanNode) -> Result<PlanNode, String>`：唯一公开优化入口；零或一个叶节点时原样返回，多叶节点时运行 DP/贪心并执行剩余边检查。
- `fn optimize_dp`：以 `usize` bit mask 表示子集，`BTreeMap<usize, Node>` 保存每个子集的最优结果；要求叶数小于 `usize::BITS`。
- `fn optimize_greedy`：比较前两个叶节点作为起点的结果；先按真实/允许的非等值边收敛，必要时进行第二轮，再拼接森林。
- `pub(crate) fn make_bushy_cartesian`：crate 内可见的确定性两两归并器；优先使用 `check_connection` 找到的真实边，否则调用 `make_cartesian_candidate`。
- `fn greedy_connect`：逐轮推进的局部选择循环。对每个当前节点扫描其后节点，取累计成本显著更低的候选并从工作向量移除被合并项。
- `fn significantly_less`、`fn apply_cartesian_factor`、`fn collect_used_edges`、`fn make_cartesian_candidate`：分别负责稳定成本比较、代价校验/放大、边集合汇总及显式笛卡尔候选构造。

文件没有 trait、枚举、模块级常量或条件编译项；主要容器采用 `BTreeMap`/`BTreeSet`，使候选和边集合遍历具有确定顺序。

## 执行流程

1. 调用者把一棵 `PlanNode` 交给 `JoinOrder::optimize`。`ConflictDetector::build` 返回检测器和叶 `Node` 列表；若叶数不超过 1，直接返回原根。
2. 叶数 `<= dp_threshold` 时进入 `optimize_dp`。它用单叶 mask 初始化 `best`，随后按子集大小递增枚举；每个子集只考察包含最低有效位的左半部分，从而避免左右对称重复。
3. DP 对已存在的左右子计划调用 `check_connection`。只有 `connected()` 的组合才由 `make_join` 构造；若所用边没有等值条件，则通过 `apply_cartesian_factor` 调整 `Node` 与内部 `PlanNode` 的累计代价。每个 mask 只保留按 `significantly_less` 判定更优的候选。
4. DP 找到 full mask 时直接返回；否则由 `make_bushy_cartesian` 按输入森林顺序逐层两两合并。这是当前 Rust 实现相对 Go `buildBushyTreeFromDP` 的简化点：Rust 没有从 DP 表筛选“最大、有限且已完整消费子集边”的森林。
5. 叶数超过阈值时进入 `optimize_greedy`。它最多尝试起始索引 0、1，每次复制叶节点，移动起点到首位，并调用 `greedy_connect`。
6. `greedy_connect` 对每个当前节点扫描其余节点：真实边合法时调用 `make_join`；允许笛卡尔且没有连接边时调用 `make_cartesian_candidate`；无等值候选按因子放大。一次外层循环没有任何合并就停止。
7. 若第一轮禁止无等值边且仍有未消费边，贪心分支以至少为 1 的因子再运行一轮。仍有剩余边的起点候选被丢弃；合法森林由 `make_bushy_cartesian` 拼接。两个起点间按浮点容差保留更低成本者。
8. 公开入口再次检查最终 `used_edges`；未完全消费时返回错误，否则返回 `Node.plan`。

## 数据与状态

`JoinOrder` 只保存配置，不在优化期间修改自身。一次运行的主要状态都由值拥有：叶节点/中间节点向量、DP 的 `best` 映射、贪心候选以及 `Node.used_edges` 集合。`Node` 来自 `conflict_detector.rs`，携带计划、覆盖顶点、累计代价和已用边；实际连接合法性及边消费由 `ConflictDetector` 决定。

DP 的时间/空间随叶数指数增长：mask 空间为 `2^n`，每个子集还枚举二分；`dp_threshold` 是防止该路径在大连接组失控的主要资源阀门。贪心路径会克隆叶集合，最多运行两个起点，并在各轮扫描剩余节点，避免 Go 版本注释所说的多起点计划克隆放大。`BTreeMap` 与 `BTreeSet` 牺牲部分常数性能来换取确定性顺序。

`cumulative_cost` 有两份需要保持同步的表示：`Node.cumulative_cost` 与 `Node.plan.cumulative_cost`。本文件在施加笛卡尔因子后显式同时更新二者；新增成本变换时也必须保持这一不变量。

## 依赖与调用关系

上游事实如下：

- `lib.rs` 公开 `join_order`；根 workspace/facade 暴露整个 crate。
- `join_order_test.rs` 直接构造 `JoinOrder`，覆盖默认 DP、强制贪心、笛卡尔惩罚和 bushy 顺序。
- `bitset_bench_test.rs` 在 16/32/64/128 叶规模运行贪心路径，并在 8 叶规模比较 DP 与贪心的顶点覆盖。
- RustCodeGraph 将目标文件列为 15 个符号，并显示它被本 crate 的测试及相邻模块引用；针对 `JoinOrder::optimize_dp`、`optimize_greedy`、`greedy_connect` 的图边查询没有返回可用 callers/callees，故调用边以已索引源码和 `rg` 交叉确认。生产 Rust 范围内未发现 `JoinOrder::optimize` 调用者。

下游直接依赖是 `ConflictDetector::{build, check_connection, make_join, cartesian_join, has_remaining_edges}`、`Node`、`PlanNode`、`JoinMethodHint`，以及标准库 `BTreeMap`/`BTreeSet`。`Cargo.toml` 中外部 planner/expression 等路径依赖全部位于 `target.'cfg(windows)'.dependencies`；本文件的可执行部分本身只通过 crate 内模块和标准库工作。

完整应用的 Go 调用链不同：`pkg/planner/core/rule_join_reorder.go` 导入同路径 Go 包并调用其 `Optimize`，Go `Optimize` 再递归抽取连接组、选择 DP/贪心并恢复 schema。当前 Rust `rule` crate虽声明 joinorder 依赖，但其 Rust 源码没有对应调用，因此不能从 facade 注册推断运行时接线。

## 错误处理与边界

- 所有失败用 `Result<_, String>` 传播，没有结构化错误类型。`ConflictDetector` 的构图、连接检查和构造错误均通过 `?` 原样向上传递。
- DP 在叶数达到 `usize::BITS` 时返回 `too many join vertices for DP`，避免移位溢出；正常默认阈值 10 会更早切到贪心，但调用者可配置更大阈值。
- `apply_cartesian_factor` 拒绝 NaN、负数和负无穷累计代价；拒绝 NaN/任意无穷因子；因子 `<= 0` 返回正无穷。乘法溢出到正无穷目前被接受，因为后置校验只拒绝 NaN、负无穷和负数。
- 空森林传给 `make_bushy_cartesian` 会返回 `cannot build bushy tree from empty node list`；贪心没有可用起点结果时返回 `cannot optimize empty join group`。
- 最终仍有冲突边时，公开入口返回 `optimized join tree left conflict edges unused`，不会静默返回部分合法计划。
- 成本差不超过 `max(|cost|, |best|, 1) * 1e-12` 时保留先出现的候选，保证浮点微小噪声不改变顺序。
- 当前实现没有 Go 版本的“无合法顺序则警告并返回原计划”策略，也没有测试模式下的专用诊断、缺失边摘要、schema 恢复或 LEADING hint 警告。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。`optimize(&self, root)` 仅读取配置，工作状态全部为函数局部拥有值；`Node` 和 `PlanNode` 的克隆隔离每个贪心起点，`join_order_test.rs::cloned_node_used_edges_are_independent_of_original` 验证修改克隆的 `used_edges` 不影响原值。

资源风险主要是内存和 CPU，而不是同步：DP 的指数级子集枚举由阈值控制；贪心最多两个起点，每个起点克隆叶集合并反复移动/删除 `Vec` 元素。若未来将一个 `JoinOrder` 在多线程共享，当前字段类型可只读使用，但是否可跨线程仍取决于 `PlanNode`、hint 类型和调用者边界，本文没有发现并发调用证据，不能据此声称线程安全保证。

## 与 Go 版本的对应关系

对应文件是 `pkg/planner/core/joinorder/join_order.go`，Rust 可执行代码复用了以下核心语义：按阈值选 DP/贪心；DP 子集枚举；贪心最多比较两个起点；无等值连接代价放大；浮点容差比较；按森林顺序两两生成 bushy tree；以及最终边消费约束。`join_order_test.rs` 还以字符串保存了 Go `TestChooseBestGreedyStart` 和 `TestCloneNodesForGreedyStartIsolation`，随后用真实 Rust API 验证相应行为。

两者并非完整一一移植：

- Go `JoinOrder` 持有 `PlanContext` 和 `joinGroup`；Rust 只持有阈值、因子与 hint 映射，输入是自包含 `PlanNode`。
- Go 的 `extractJoinGroup`/`optimizeRecursive` 处理 Selection、outer join、安全边界、会话变量、hint、递归子树与 schema 顺序；Rust 可执行部分没有这些入口，顶部同名逻辑仅是块注释。
- Go DP 会对正无穷 full plan 先调用 `buildBushyTreeFromDP`，从 DP 表选择完整合法子集构造森林；Rust 只在 full mask 完全不存在时直接拼原始叶子，行为和计划质量可能不同。
- Go 贪心先按累计成本稳定排序节点、支持 LEADING hint、在失败时回退原计划并输出诊断；Rust 保留构图返回顺序，没有 leading-tree 接入，失败返回字符串错误。
- Go 的连接方法 hint 以逻辑计划 ID 建模且由连接组抽取；Rust 由调用者直接填写 `vertex_hints`，当前没有生产接线证明其来源。

因此新增行为时应先确定目标是保持当前轻量 Rust 模型，还是继续对齐完整 Go planner；不可仅根据顶部注释草案宣布功能已经存在。

## 扩展指南

- 修改算法分支或阈值语义：从 `JoinOrder::optimize` 接入，并同步覆盖 `join_order_test.rs` 的 DP/贪心测试及 `bitset_bench_test.rs` 的规模矩阵。必须评估 DP 指数复杂度和 `usize` 位宽边界。
- 修改候选成本：集中调整 `apply_cartesian_factor`、`significantly_less` 或 `greedy_connect`，同时维护 `Node.cumulative_cost == Node.plan.cumulative_cost`；补测 NaN、正/负无穷、零/负因子和接近容差的成本。
- 修改边合法性或连接构造：优先在 `conflict_detector.rs` 的对应符号实现，本文件只编排 `check_connection`/`make_join`；同步相邻 `conflict_detector_test.rs`，并保留公开入口的剩余边后置条件。
- 对齐 Go 的 DP bushy 恢复：最可能新增 Go `buildBushyTreeFromDP` 对应 helper，而不是直接把所有叶子交给 `make_bushy_cartesian`；需要验证有限子集优先、完整消费子集内边、稳定排序和正无穷 full plan。
- 接入完整 planner：需要在本文件之外实现/复用连接组抽取、会话配置、hint 警告、schema 恢复和 rule 调用。此任务不能通过仅在 `rule/Cargo.toml` 声明依赖完成，必须增加生产调用与跨 crate 测试。
- 保持测试与源码分文件：现有独立测试入口由 `lib.rs` 的 `#[cfg(test)] mod join_order_test` 和 `mod bitset_bench_test` 装配，新增回归应继续放在这些测试文件或新的独立 `*_test.rs` 中。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/planner/core/joinorder` 确认目标 crate 的 Rust/Go 源与测试集合；`node --file ...join_order.rs --offset 1/499` 读取全部 691 行；`query` 精确定位 `JoinOrder`、`optimize_dp`、`optimize_greedy`、`greedy_connect`、`make_bushy_cartesian`。限定方法的 callers/callees 查询未返回边，因此没有把图缺失当作“无调用”的唯一证据。
- 已读生产/装配文件：`pkg/planner/core/joinorder/join_order.rs`、`conflict_detector.rs` 与 `util.rs` 的直接 API 证据、同目录 `Cargo.toml`、`lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/planner/core/rule/Cargo.toml`，以及生产 Rust 全局引用搜索。
- 已读 Go 对照：`pkg/planner/core/joinorder/join_order.go` 的入口、DP、贪心、成本与 bushy-tree 实现；`pkg/planner/core/rule_join_reorder.go` 的包接入引用。
- 已读测试：`pkg/planner/core/joinorder/join_order_test.rs`、`join_order_test.go`，以及 `bitset_bench_test.rs` 中实际运行 `ConflictDetector::build + JoinOrder::optimize` 的规模测试。
- 本任务是纯文档分析，按任务约束未运行 Cargo。最终只执行固定 11 章节的结构验证，并人工复核本文明确回答文件为何存在、实际如何运行、当前接线边界和安全扩展位置。
