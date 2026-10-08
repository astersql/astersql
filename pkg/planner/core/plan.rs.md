# `pkg/planner/core/plan.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate。crate 入口 `pkg/planner/core/lib.rs` 以 `mod plan;` 装入该模块，并用 `pub use plan::*;` 将其中的公开项重新导出。它不是完整的计划接口定义：`PlanKind`、`PlanNode`、`PlannerContext` 和 `StoreType` 位于同 crate 的 `common_plans.rs`；本文件在这些轻量计划树结构之上补充三类辅助能力：会话规划上下文检查、物理任务包装与细粒度 Shuffle 改写、probe 父节点计数。

`pkg/planner/core/Cargo.toml` 将该 crate 定义为 `astersql-planner-core`、库入口为 `lib.rs`、关闭自动测试发现（`autotests = false`），并声明 `nextgen` feature。本文件没有条件编译项，也没有直接引用外部 crate；其输入类型都经 crate 根导入。仓库搜索只发现 Rust 测试 `pkg/planner/core/plan_test.rs` 直接调用本文件的公开 API，未发现 Rust 生产文件的调用点，因此当前应把它视为“已实现、已独立测试且由 crate 公开，但 Rust 生产主链尚未检出接线”的移植模块，而不是宣称已经驱动完整 Rust 优化器。

## 核心职责

1. `AsSctx` 把可选的 `PlannerContext` 引用变成必有引用，并在缺失时保留 Go 版本的错误文本契约。
2. `PlanTask` 表示一个可用或无效的物理计划任务；`ShuffleConfig` 表示是否开启 Shuffle 以及请求的并行流数。
3. `optimizeByShuffle` 只对 `Window`、`StreamAgg`、`MergeJoin` 三种计划节点做分派；其他节点和不满足前置条件的任务原样返回。
4. `wrap_shuffle` 在每个符合条件的 `Sort` 子节点外插入 `Shuffle` sender 和 `ShuffleReceiver`，并把两层新节点的执行存储类型标为 `TiFlash`。
5. `getEstimatedProbeCntFromProbeParents` 与 `getActualProbeCntFromProbeParents` 沿 probe 父节点序列选择 outer child，并分别连乘估算行数和实际行数。

这些职责均围绕计划树的局部改写或只读聚合，不负责生成逻辑计划、枚举物理候选、比较成本，也不负责运行 Shuffle。

## 主要符号

- `pub fn AsSctx(ctx: Option<&PlannerContext>) -> Result<&PlannerContext, String>`：`Some` 时借用并返回原上下文，`None` 时返回固定字符串错误。函数不克隆或修改上下文。
- `pub struct PlanTask { pub plan: Option<PlanNode>, pub invalid: bool }`：拥有一棵可选计划树和独立无效标记。`PlanTask::New` 建立 `Some(plan), invalid = false`；`PlanTask::Invalid` 建立 `None, invalid = true`。字段公开，调用者也能构造 `plan = None, invalid = false` 这类状态。
- `pub struct ShuffleConfig { pub enabled: bool, pub stream_count: usize }`：派生 `Default`，因此默认值为关闭且流数为零。
- `pub fn optimizeByShuffle(PlanTask, &ShuffleConfig) -> PlanTask`：按值接收任务并可能消费、替换其中的计划；无效、无计划、关闭或流数不大于 1 时直接返回。
- `fn wrap_shuffle(PlanNode, Vec<Vec<String>>, usize) -> PlanNode`：模块私有的结构校验与统一包装器。要求子节点非空、子节点数等于 key 数组数，并且每个直接子节点都是至少含一个孩子的 `Sort`。
- `fn ndv_limited_streams(&PlanNode, usize) -> Option<usize>`：从第一个 `Sort` 子节点的第一个孩子读取 `estimated_rows`，把它当作本移植模型中的 NDV 近似；值不大于 1 时拒绝改写，否则返回 `min(configured, ndv as usize)`。
- `fn probe_outer_child(&PlanNode) -> Option<&PlanNode>`：仅接受 `Apply`、`IndexJoin`、`IndexHashJoin`、`IndexMergeJoin`；由 `build_side` 推导 outer child（build 0 则 outer 1，build 1 则 outer 0），缺失、越界或其他值均返回 `None`。
- `optimizeByShuffle4Window`：用 `Window.functions` 的字符串列表作为唯一分区键数组，并受 `ndv_limited_streams` 限制。
- `optimizeByShuffle4StreamAgg`：使用固定字符串 `group-by` 作为唯一分区键，并受 `ndv_limited_streams` 限制。
- `optimizeByShuffle4MergeJoin`：把 `MergeJoin.keys` 拆成左右两组字符串键；不做 NDV 限流，直接尝试包装两侧。
- `getEstimatedProbeCntFromProbeParents`：初值 `1.0`，对可识别父节点的 outer child 连乘 `estimated_rows`。
- `getActualProbeCntFromProbeParents`：初值 `1_i64`，对可识别父节点的 outer child 连乘 `actual_rows.unwrap_or(1) as i64`，使用 `wrapping_mul`。

文件没有模块级常量、trait、宏、异步函数或条件编译项。

## 执行流程

Shuffle 主流程如下：

1. 调用者把 `PlanNode` 放入 `PlanTask`，并传入 `ShuffleConfig`。
2. `optimizeByShuffle` 先执行不改变输入的快速退出检查：任务无效、计划为空、配置关闭、或 `stream_count <= 1`。
3. 对通过检查的任务取出计划并按根节点 `PlanKind` 分派：`Window`、`StreamAgg`、`MergeJoin` 调用各自辅助函数，其他根节点保持原样。
4. Window/StreamAgg 先通过 `ndv_limited_streams` 检查第一个孩子必须是 `Sort`，再读取该 `Sort` 的第一个孩子的 `estimated_rows`；估值大于 1 时将流数限制到该整数估值以内。
5. 各辅助函数准备与根节点子节点一一对应的分区键数组，交给 `wrap_shuffle`。包装器先整体校验树形；任一孩子不是非空 `Sort`，或键数组数量不匹配，整棵计划原样返回，不做部分改写。
6. 对每个直接孩子，创建 `Shuffle(info = "streams:N, keys:...")`，其孩子仍是原来的 `Sort` 子树；再创建 `ShuffleReceiver(info = "streams:N")` 包住 sender。两层节点 id 均为 `-1`，`store_type` 均设为 `TiFlash`，最后替换根节点的 `children`。

probe 计数流程独立于 Shuffle：两个函数都从乘法单位元 1 开始，依次调用 `probe_outer_child`；可识别的 index-join/apply 父节点贡献 outer child 的估算或实际行数，不可识别或结构信息不足的父节点不改变结果。因此空切片和全是无关节点的切片均返回 1。

## 数据与状态

本文件修改的是按值拥有的 `PlanNode` 树，没有全局变量或共享注册表。`optimizeByShuffle` 通过 `Option::take` 暂时移出任务中的计划，随后总会放回一个计划；前置检查已保证该路径不会在 `expect("plan presence checked above")` 处因正常输入触发 panic。

Shuffle 新节点由 `PlanNode::New` 建立，继承其默认字段语义：默认 `StoreType::Root`、估算值为零、`actual_rows = None`、`probe_count = 1.0`、`build_side = None`；本文件随后只覆盖 `store_type = TiFlash`。根计划本身的 id、统计、算子信息和其他字段不变，原子树被移动到 sender 下面，而不是复制。

`ndv_limited_streams` 实际读取的是数据源节点的 `estimated_rows`，并以 `as usize` 截断小数。本文件没有专门的 NDV 字段；因此这是轻量 Rust 计划模型对 Go NDV 估算的近似，不应解读为完整统计估算。

实际 probe 计数来自 `PlanNode.actual_rows`。缺失时按 1 处理；转换为 `i64` 后用回绕乘法累计，所以极大值不会报溢出错误，而会按二进制补码回绕。

## 依赖与调用关系

直接下游依赖均来自 `common_plans.rs`：

- `PlanKind` 决定可改写算子、Sort 前置条件、probe 父节点类型以及 Shuffle/Receiver 新节点类型。
- `PlanNode::New` 创建包装节点；`children`、`estimated_rows`、`actual_rows`、`build_side` 提供树形和统计数据。
- `StoreType::TiFlash` 标记新插入的发送/接收边界。
- `PlannerContext` 仅由 `AsSctx` 借用检查，Shuffle 配置不从上下文读取。

内部调用边为：`optimizeByShuffle -> optimizeByShuffle4{Window,StreamAgg,MergeJoin}`；Window/StreamAgg 各调用 `ndv_limited_streams`，三者都调用 `wrap_shuffle`；两个 probe 聚合函数都调用 `probe_outer_child`。

上游方面，`lib.rs` 将公开项从 crate 根导出，`plan_test.rs` 是当前检出的 Rust 直接调用者。RustCodeGraph 对文件给出 9 个使用文件，但精确 callers/callees 查询受到同名符号消歧限制；随后按符号名做仓库级 Rust 搜索，只在 `plan_test.rs` 找到这些 API 的直接调用。因此不能据此声称 Rust 生产优化器已调用 `optimizeByShuffle`。Go 主链则明确存在对应接线：`find_best_task.go` 在非 MPP 且属性无排序项时调用 `optimizeByShuffle`；`core_init.go` 把两个 probe 计数函数赋给物理算子工具函数；`planbuilder.go` 和 `common_plans.go` 调用 `AsSctx`。

## 错误处理与边界

- `AsSctx(None)` 是本文件唯一显式返回错误的路径；错误文本与 Go 版本一致。`Some` 不验证更深层的会话能力，因为 Rust 参数已经是具体 `PlannerContext`，并非 Go 的接口类型断言。
- Shuffle 的不适用情况全部采用“原样返回”而非错误：无效/空任务、配置关闭、流数不足、不支持的根算子、孩子形状不符、估值不大于 1。
- `wrap_shuffle` 先验证所有孩子再改写，避免 MergeJoin 一侧改写成功而另一侧失败。它只检查 `Sort` 至少有一个孩子，不限制 `Sort` 的孩子数，也不验证键字符串语义。
- Window 在根类型不匹配时会生成空键列表后交给包装器；公开函数虽然命名为 Window 专用，但并未直接拒绝非 Window 根。StreamAgg 专用函数同样不校验根类型。顶层分派可保证正常调用类型正确，直接外部调用这些公开辅助函数时则需调用者承担类型契约。
- MergeJoin 专用函数明确拒绝非 MergeJoin 根；若子节点数与左右两组键不相等，包装器原样返回。正常二元 MergeJoin 满足该数量契约。
- `ndv_limited_streams` 通过安全索引返回 `None`，probe outer child 也通过 `get` 防止越界；但 `actual_rows as i64` 可能改变大于 `i64::MAX` 的数值含义，累计又明确允许回绕。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或事务，也不执行网络/磁盘 I/O。`stream_count` 只是写入计划节点 `info` 的并行度元数据，并控制树形改写；实际并行执行和资源调度不在本文件中。

所有计划和配置都遵循普通 Rust 所有权：配置只读借用，计划按值转移，返回值拥有改写后的完整树。`AsSctx` 返回的引用生命周期与输入引用一致。函数没有缓存或静态可变状态，因而本文件自身没有跨调用同步问题；是否可跨线程使用最终取决于 `PlanNode`/`PlannerContext` 及上层执行器，而不是这里建立的并发协议。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/plan.go`，函数名称与职责一一对应，但 Rust 当前是轻量移植，存在重要差异：

- Go `AsSctx` 从 `base.PlanContext` 做 `sessionctx.Context` 类型断言；Rust 接收 `Option<&PlannerContext>`，只表达上下文是否存在。两者失败错误文本相同，但 Rust 没有接口能力转换。
- Go `optimizeByShuffle` 从会话变量分别读取 Window、StreamAgg、MergeJoin 并发度，并把 `PhysicalShuffle` 通过 `Attach2Task` 接到具体物理任务；Rust 统一接收显式 `ShuffleConfig`，操作通用 `PlanNode`。
- Go Window/StreamAgg 用 `EstimateColsNDVWithMatchedLen` 基于分区列、schema 和统计估算 NDV；Rust 用首个 `Sort` 的首个孩子的 `estimated_rows` 近似。Rust StreamAgg 的键是固定 `"group-by"` 字符串，而 Go 克隆真实 `GroupByItems`。
- Go MergeJoin 克隆左右连接列，且并发度来自会话变量；Rust 从 `PlanKind::MergeJoin.keys` 拆出左右字符串，使用传入流数，并且与 Go 一样要求两侧为 `Sort`。
- Go 构造一个 `PhysicalShuffle`，内部保存 tails、data sources、splitter 和 by-items；Rust 在每个分支直接插入一对 `Shuffle`/`ShuffleReceiver` 节点，并标记 TiFlash。这是表示模型差异，不能假定二者执行对象完全等价。
- Go 的实际 probe 行数从 `RuntimeStatsColl` 按 outer child id 查询 root/cop stats，cop stats 可覆盖 root stats；Rust直接读取节点内嵌 `actual_rows`。两者缺失统计时都使用 1，但 Rust 使用 `wrapping_mul`，Go 普通 `int64` 乘法也具有固定宽度运行时结果，文档不据此宣称跨语言极端溢出行为已专门验证。
- Go 生产接线已在 `find_best_task.go`、`core_init.go`、`planbuilder.go`、`common_plans.go` 检出；Rust 当前只检出 crate 再导出和 `plan_test.rs` 测试调用。

## 扩展指南

- 新增可 Shuffle 的根算子时，先在 `optimizeByShuffle` 增加明确分派，再建立专用键提取函数；不要把算子特例塞进 `wrap_shuffle`，后者应继续只承担形状校验与统一包装。同步扩展 `pkg/planner/core/plan_test.rs`，覆盖成功改写、非 Sort 子树、关闭/低并发以及键数组与孩子数量不匹配。
- 若要提高 Window/StreamAgg 的统计准确度，应先在 `PlanNode` 或统计子系统建立真实 NDV/表达式模型，再替换 `ndv_limited_streams` 的行数近似；需要对照 Go 的 `EstimateColsNDVWithMatchedLen`，并验证空分区键、复合键、小数估值和估值小于等于 1 的行为。
- 若要把该实现接入 Rust 生产优化主链，优先寻找“物理候选确定、enforcer 完成、非 MPP 且无需排序属性”的等价位置，复刻 `find_best_task.go` 的接线条件；在接线前不要仅因 crate 公开导出就认为优化已生效。
- 新增 probe 父节点类型时，应同时更新 `probe_outer_child` 的允许集合和 build/inner-side 到 outer child 的映射，并在独立测试中覆盖 `build_side` 为 0、1、缺失、非法值以及孩子越界。不要把测试嵌入生产源文件。
- 若实际统计改为独立集合，应让 `getActualProbeCntFromProbeParents` 接收该集合或稳定查询接口，并复刻 Go 中 root/cop 统计优先级与 PhysicalApply cache-hit TODO 的语义。
- 修改错误契约时同步核对 Go `AsSctx` 和 `test_as_sctx_preserves_go_error_contract`。改变新节点 id、存储类型或 `info` 格式可能影响 EXPLAIN、编码或执行接线，应评估兼容性；增加深树克隆或表达式复制会带来规划期内存与性能风险。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/core/plan.rs` 确认目标文件已索引且有 18 个符号。
- RustCodeGraph `node --file pkg/planner/core/plan.rs --offset 1 --limit 260`：读取并核对目标文件完整 196 行，包括全部类型、函数、分支和字段访问。
- RustCodeGraph 对 `AsSctx`、`optimizeByShuffle`、三个专用 Shuffle 函数及两个 probe 计数函数执行 `query`，确认 Rust/Go 同名定义及各自路径。精确 callers/callees 命令发生符号 ID 消歧异常，未把其噪声结果作为调用关系证据；改以文件索引和限定路径的 `rg` 补核直接调用点。
- 已读 Rust 路径：`pkg/planner/core/common_plans.rs`（`StoreType`、`PlanKind`、`PlanNode`、`PlannerContext`）、`pkg/planner/core/lib.rs`（模块装配、公开再导出、独立测试挂载）、`pkg/planner/core/plan_test.rs`（Shuffle、probe、错误契约边界）。
- 已读配置与 Go 对照：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/plan.go`；并通过限定搜索核对 `pkg/planner/core/find_best_task.go`、`pkg/planner/core/core_init.go`、`pkg/planner/core/planbuilder.go`、`pkg/planner/core/common_plans.go` 的 Go 接线。
- `plan_test.rs` 证明：Window 成功插入 receiver/sender、非 Sort 或关闭配置保持不变、空计划保持不变、低估值不改写、StreamAgg/MergeJoin 可包装、非 probe 父节点被忽略、缺失上下文保留固定错误文本。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构检查和人工事实复核为验收依据。
