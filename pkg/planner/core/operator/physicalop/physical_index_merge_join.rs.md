# `pkg/planner/core/operator/physicalop/physical_index_merge_join.rs`

## 文件定位

该文件属于 `astersql-planner-core-operator-physicalop` crate；crate 根在同目录的 `lib.rs`，并通过 `pub mod physical_index_merge_join` 暴露本模块、通过 `pub use physical_index_merge_join::PhysicalIndexMergeJoin` 再导出 Go 兼容类型。`Cargo.toml` 的 `[package.metadata.porting]` 把整个 crate 对应到 Go 包 `pkg/planner/core/operator/physicalop`。

文件描述的是“索引归并连接”物理计划，而不是执行器本身。它在索引连接共有状态之上保存连接键的索引顺序、比较器、outer 是否要排序及 inner 扫描方向。当前文件并存两套表示：`LegacyPhysicalIndexMergeJoin` 面向 `PhysicalPlanNode` 的简化/遗留模型；`PhysicalIndexMergeJoin` 包装完整的 `crate::PhysicalIndexJoin`，是由 `lib.rs` 接入 `base::PhysicalPlan` 体系并被其他 crate 使用的 Go 兼容模型。

源文件第 16—105 行还有一段被逐行注释掉的早期移植草稿，其中提到的 `Init`、`GetCost`、`GetPlanCostVer1/2` 和 `Attach2Task` 不是该段的可执行 API。判断现状应以第 108 行以后的实际定义以及 `lib.rs` 的宏接线为准。

## 核心职责

1. `LegacyPhysicalIndexMergeJoin` 保存两个简化子计划，生成可读/归一化 EXPLAIN，给出一个本地近似成本，拼装新的 `PhysicalPlanNode`，并估算自身动态内存。
2. `PhysicalIndexMergeJoin` 在完整 `PhysicalIndexJoin` 上增加 Go 同名结构的五类状态：键位映射、两组比较器、outer 排序开关和 inner 降序开关。
3. `Deref`/`DerefMut` 让完整类型复用 `PhysicalIndexJoin` 的计划上下文、schema、children、索引连接字段及方法；`lib.rs` 中的 `join_operator_core!(PhysicalIndexMergeJoin, BasePhysicalJoin)` 再把它接入统一的 `ConcretePhysicalOperator`/`PhysicalPlan` 调度。
4. `New`、`Clone`、`MemoryUsage` 维护具体变体的构造、跨上下文克隆和容量计费；缓存快照与计划改写可以据此保留 merge 专属字段。

本文件不执行行级 merge 算法，也没有生成 inner lookup range。Go 版本的任务装配和成本细节位于 planner core 的 `task.go`、`plan_cost_ver1.go`，Rust 的完整类型目前通过公共索引连接接线和外部路由使用；不能把 `LegacyPhysicalIndexMergeJoin::cost` 当作 Go 成本公式的等价移植。

## 主要符号

- `LegacyPhysicalIndexMergeJoin`：`#[derive(Clone, Debug, PartialEq)]` 的简化值类型。`outer`、`inner` 是 `PhysicalPlanNode`；`key_offset_order` 表示连接键按索引顺序的重排；两组 `Vec<String>` 仅保存比较器标识；`concurrency` 只参与简化成本分摊。
- `LegacyPhysicalIndexMergeJoin::explain_info(normalized)`：归一化输出仅保留键数量与 `need_outer_sort`，普通输出打印具体键序、排序开关和 `descending`。
- `LegacyPhysicalIndexMergeJoin::cost(...)`：返回两侧已有成本、线性扫描 CPU 成本和可选的 `outer_count * log2(outer_count)` 排序成本之和；并发除数至少为 1，避免零并发导致除零。
- `LegacyPhysicalIndexMergeJoin::attach_to_task()`：schema 依次拼接 outer、inner；新节点 ID 为两子节点最大 ID 加一，kind 为 `PhysicalKind::IndexMergeJoin`，children 顺序固定为 `[outer, inner]`，统计信息仅沿用 outer，required properties 清空。
- `LegacyPhysicalIndexMergeJoin::memory_usage()`：计入结构体本体、键位向量容量、两个 `String` 向量的槽位容量和各字符串已分配容量；不递归计算两个子计划。
- `PhysicalIndexMergeJoin`：完整计划类型。字段名和布局语义直接对齐 Go：`PhysicalIndexJoin`、`KeyOff2KeyOffOrderByIdx`、`CompareFuncs`、`OuterCompareFuncs`、`NeedOuterSort`、`Desc`。
- `crate::JoinCompareFunc`：在 `lib.rs` 定义为 `Arc<dyn Fn(Row, usize, Row, usize) -> i32 + Send + Sync>`；因此比较器可廉价克隆并可在线程间共享，而非 `Legacy` 模型中的字符串标签。
- `PhysicalIndexMergeJoin::New(join)`：把所包装基类的类型字符串改为 `IndexMergeJoin`，其余 merge 专属字段初始化为空或 `false`。它不分配新 Plan ID，也不更换上下文；这些由传入的 `PhysicalIndexJoin` 承担。
- `ExplainInfo` / `ExplainNormalizedInfo`：分别调用 `PhysicalIndexJoin::ExplainInfoInternal(false, true)` 与 `(true, true)`；第二个布尔值把解释路径标识为 merge 变体。
- `MemoryUsage`：在 `PhysicalIndexJoin::MemoryUsage()` 上增加三个 `Vec` 头、两个布尔值，以及按 capacity 计算的键位槽和 `Arc` 比较器槽。`Arc` 指向的闭包捕获内存不在这里递归计费。
- `Clone(context)`：先用新上下文克隆嵌入的 `PhysicalIndexJoin`，再克隆三个向量并复制两个布尔值；基类克隆失败时原样传播 `expression::Error`。
- `Deref` / `DerefMut`：把完整类型透明借用为 `PhysicalIndexJoin`，是复用索引连接方法和字段访问的关键接线。

## 执行流程

完整计划的典型生命周期如下：

1. 上游先构造带 `BasePhysicalJoin`、上下文、schema/children 等共有信息的 `PhysicalIndexJoin`，再调用 `PhysicalIndexMergeJoin::New` 将它包装为 merge 具体变体并设置类型标识。
2. 规划阶段填充 `KeyOff2KeyOffOrderByIdx`、`CompareFuncs`、`OuterCompareFuncs`、`NeedOuterSort` 和 `Desc`。这些字段描述执行契约，但本文件本身不消费数据行。
3. `lib.rs` 的 `join_operator_core!` 将解释、索引解析、内存和成本入口路由到具体类型或其 `Deref` 基类。`index_join_base`/`index_join_base_mut` 又让通用索引连接代码取得内嵌的 `PhysicalIndexJoin`。
4. 计划改写若重建共有基类，`preserve_index_join_variant` 会重新包装为 `PhysicalIndexMergeJoin` 并克隆五个 merge 专属字段，防止退化为普通 `PhysicalIndexJoin`。
5. 计划缓存由 `CachedPhysicalPlan::try_capture` 抽取基类和五个专属字段；`restore` 重建相同具体动态类型。`PhysicalIndexMergeJoin::Clone` 同样保留这些字段并替换基类上下文。
6. 下游遍历通过 `dyn base::PhysicalPlan` 看到该节点。例如 `pkg/executor/statement_ru_plan_walk_test.rs` 构造它并验证比较器槽会进入计划资源使用量遍历。

简化模型的流程独立：调用者直接准备 outer/inner `PhysicalPlanNode`，可调用 `cost` 比较候选、用 `attach_to_task` 得到一个新的树节点；该路径不经过完整类型的比较器闭包或缓存快照。

## 数据与状态

`key_offset_order` / `KeyOff2KeyOffOrderByIdx` 的核心不变量是：元素是“join key 原偏移到按 inner 索引顺序排列后偏移”的映射。文件不验证重复、越界或长度是否与 join keys 相等，正确性由构造者保证；改变生成逻辑时必须同步消费者对位置的解释。

`compare_functions`/`CompareFuncs` 比较 outer 与 inner 的连接键，`outer_compare_functions`/`OuterCompareFuncs` 比较两条 outer row 以支持排序。完整类型使用线程安全的 `Arc` 闭包；向量克隆只增加共享引用计数。`NeedOuterSort` 决定 outer 是否需要排序，`Desc`/`descending` 描述 inner 的有序扫描方向；两者是正交状态。

完整类型的 schema、children、统计信息、Plan ID、上下文、连接类型、inner child 下标及过滤条件全部存放在 `PhysicalIndexJoin` 及其 `BasePhysicalJoin` 链中。merge 结构没有自己的锁、缓存成本字段或运行时结果集。简化类型则直接拥有 outer/inner 节点，`attach_to_task` 新建节点时只复制 outer 统计信息，这是一项简化行为而非完整 planner 的统计合并规则。

内存估算均以容器 capacity 而不是 length 计费，因此预留但未填充的槽也会增加结果。独立 Rust 测试 `physical_index_merge_join_test.rs` 明确验证 `Legacy` 两个比较器向量 `reserve_exact` 后的容量差按 `size_of::<String>()` 计入。

## 依赖与调用关系

直接下游依赖包括：

- `physical_common_plans::{PhysicalKind, PhysicalPlanNode}`：仅供 `LegacyPhysicalIndexMergeJoin` 的树表示和装配。
- `crate::PhysicalIndexJoin`：完整类型的共有实现来源；`Deref` 以及 crate 宏使其复用 resolve、成本、schema 和 children 等能力。
- `crate::JoinCompareFunc`：完整比较器句柄，定义在 `physicalop/lib.rs`，底层 row 类型来自 `ranger::chunk::Row`。
- `base::ContextRef` 与 `expression::Error`：`Clone` 的新上下文输入与错误类型；两者分别由本 crate 的 `base`、`expression` 依赖提供。

可确认的 Rust 上游包括：

- `physicalop/lib.rs`：声明/再导出模块，以 `join_operator_core!` 实现具体物理计划协议；`index_join_base(_mut)` 识别此变体，`preserve_index_join_variant` 在共有基类改写后恢复具体变体。
- `physicalop/cache_snapshot.rs`：按动态类型捕获、恢复 `PhysicalIndexMergeJoin` 的共有与专属状态。
- `physicalop/plan_clone_generated.rs`：为 `LegacyPhysicalIndexMergeJoin` 实现 `CloneForPlanCache`，分别递归克隆 outer 和 inner。
- `physicalop/base_physical_plan.rs`：把 Legacy 与完整类型都列入 canonical index join 类型识别。
- `physicalop/single_scan_index_join.go` 的 Go 侧以及 Rust `index_join_base` 一类适配器说明：索引 merge join 作为索引连接家族成员，通用逻辑需要先取得其内嵌基类。
- `pkg/executor/statement_ru_plan_walk_test.rs`：把完整变体放入 typed physical plan 树，证明它可经统一物理计划接口参与执行器侧计划遍历。

RustCodeGraph 的项目索引状态为 11,467 个文件、307,296 个节点，但 `files --filter pkg/planner/core/operator/physicalop/physical_index_merge_join` 未命中该目标，精确 `explore` 也未给出上下文。因此本文件的调用边由上述源码引用与 `rg` 结果核对，不能宣称来自图数据库的完整 caller/callee 集合。

## 错误处理与边界

本文件唯一显式可失败的公开方法是 `PhysicalIndexMergeJoin::Clone`；错误只可能来自嵌入的 `PhysicalIndexJoin::Clone(context)`，并通过 `?` 传播为 `expression::Error`。字段向量克隆和布尔复制不产生业务错误。

`LegacyPhysicalIndexMergeJoin::cost` 使用 `outer_count.max(1.0).log2()`，避免零行时求 `log2(0)`；CPU 并发除数使用 `self.concurrency.max(1)`，避免除零。但它不拒绝负行数、负成本或负 CPU factor，调用方必须提供有效估值。该公式也没有 Go 版本中的过滤选择率、batch size、distinct factor、并行衰减和内存成本，不适合作为完整成本结果。

`attach_to_task` 不检查 children 是否已经成树、不检查 ID 溢出，也不推导 required properties；`usize` 最大 ID 加一在异常输入下可能溢出。完整类型 `New` 只修改类型标识，不替调用者验证 join key 映射、比较器数量或排序方向的一致性。

`MemoryUsage` 是估算而不是分配器精确统计：完整类型计算 `Vec` 槽位但不递归计算闭包捕获；简化类型计算字符串容量但不递归 outer/inner。Go 的 `MemoryUsage` 对 nil receiver 返回 0，Rust 方法必须有有效引用，因而不存在对应的 nil 分支。

## 并发与资源生命周期

文件本身不创建线程、任务、通道、锁或事务。`LegacyPhysicalIndexMergeJoin::concurrency` 只是成本公式的数值参数，不管理 worker 生命周期。

完整比较器使用 `Arc` 且 trait object 要求 `Send + Sync`，允许计划克隆、缓存快照和跨线程持有同一个闭包；`Clone`、缓存捕获/恢复和变体保型都是共享 `Arc`，不会复制闭包内部状态。若闭包捕获可变资源，其同步与生命周期必须由闭包自身的线程安全类型保证。

`PhysicalIndexMergeJoin` 拥有 `PhysicalIndexJoin` 和三个向量；对象释放时向量槽位及 `Arc` 引用正常释放，最后一个引用释放时闭包才销毁。`outer`/`inner` 的执行任务生命周期由通用 planner/executor 管理，本文件不启动或回收它们。Go 的 `attach2Task4PhysicalIndexMergeJoin` 会把 outer 转为 root task、按 `InnerChildIdx` 设置 children，并把预建 `InnerPlan` 放在 inner 侧；这段生命周期接线不在当前 Rust 文件内。

## 与 Go 版本的对应关系

Go 对照文件是 `physical_index_merge_join.go`。完整 Rust 结构的六个字段逐项对应 Go 结构，`ExplainInfo`、`ExplainNormalizedInfo` 和 `MemoryUsage` 的意图也一致。Rust `JoinCompareFunc` 用 `Arc<dyn Fn + Send + Sync>` 表达 Go `expression.CompareFunc` 函数槽；Rust 键位使用 `Vec<i32>` 对齐 Go `[]int` 的有符号语义，但位宽并不相同。

差异与未完全移植点如下：

- Go `Init(ctx)` 设置类型、从会话原子 PlanID 计数器取新 ID、设置上下文并更新 `Self` 回指；Rust `New(join)` 只在已有基类上设置类型并初始化 merge 字段，ID/上下文来自传入的 `PhysicalIndexJoin`。
- Go 在本文件公开 `GetCost`、`GetPlanCostVer1`、`GetPlanCostVer2`、`Attach2Task` 并通过 `utilfuncp` 函数指针下沉到 core。Rust 实际定义没有这些同名 merge 专属方法；`lib.rs` 的通用宏会调用可经 `Deref` 获得的索引连接成本入口，具体接线不能据顶部注释草稿认定为已经逐项等价。
- Go V1 merge 成本的真实实现位于 `pkg/planner/core/plan_cost_ver1.go:getCost4PhysicalIndexMergeJoin`，包含过滤、batch 排序去重、inner worker 并发、匹配对数和批结果内存；`LegacyPhysicalIndexMergeJoin::cost` 只是明显较小的近似模型。
- Go `attach2Task4PhysicalIndexMergeJoin` 位于 `pkg/planner/core/task.go`；Rust `Legacy::attach_to_task` 直接把两个节点拼成 `[outer, inner]`，不执行 root task 转换，也不遵循 `InnerChildIdx` 重排。
- Go `MemoryUsage` 含 nil receiver 保护并按 `size.SizeOfSlice`/`SizeOfFunc` 计费；完整 Rust 版本用本平台 `size_of`，还显式加入三个 `Vec` 头和两个 bool。两者目标相同，但数值只应在各自 ABI 内解释。

Go 侧直接回归证据包括 `physical_utils_test.go`：它验证嵌入 `PhysicalIndexJoin` 的 merge 变体会被 `HasSingleScanIndexJoin` 识别。Rust 对应的缓存与变体契约覆盖在 `cache_snapshot_test.rs` 和 `canonical_router_aster_unit_test.rs`，而本文件的独立测试只覆盖 Legacy 内存容量计费。

## 扩展指南

新增 merge 专属状态时，应同时检查并修改以下位置：结构体字段、`New` 默认值、`MemoryUsage`、`Clone`，以及 `lib.rs::preserve_index_join_variant`、`cache_snapshot.rs` 的枚举载荷/捕获/恢复。若 Legacy 模型仍需表达同一语义，还要同步其字段、解释、成本/装配和 `plan_clone_generated.rs` 的缓存克隆行为。

修改键序或比较器语义时，优先明确三项不变量：映射方向与 join key 长度、`CompareFuncs` 和 `OuterCompareFuncs` 的位置对应、`NeedOuterSort`/`Desc` 对排序方向的共同约束。新增校验若会返回错误，应放在真实构造或规划入口，而不是仅在 EXPLAIN 或内存方法中断言。

成本对齐应以 Go 的 `getCost4PhysicalIndexMergeJoin` 和 `getPlanCostVer14PhysicalIndexMergeJoin` 为依据，不应继续扩写 Legacy 近似后声称等价。任务装配对齐则应核对 Go `attach2Task4PhysicalIndexMergeJoin` 对 `InnerChildIdx`、`InnerPlan` 和 root task 的处理，并复用 Rust 现有的 canonical index join 路由。

测试必须保持在独立文件中。局部行为优先扩展 `physical_index_merge_join_test.rs`；完整类型的克隆/缓存字段保真扩展 `cache_snapshot_test.rs`；动态类型及基类路由扩展 `canonical_router_aster_unit_test.rs`；计划遍历资源计量扩展 `pkg/executor/statement_ru_plan_walk_test.rs`。不要把 `#[cfg(test)]` 测试内嵌回生产源文件。

兼容性风险主要是缓存恢复遗漏新字段、计划改写丢失具体变体或键位映射解释变化；性能风险主要是错误触发 outer 排序、比较器槽/闭包内存漏计以及成本模型低估并发或批排序。任何完整行为变更还应与同路径 Go 文件及其测试保持一致。

## 验证依据

- 生产源：`pkg/planner/core/operator/physicalop/physical_index_merge_join.rs`，完整读取 260 行，核对两个结构、各方法、注释草稿及无条件编译状态。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml` 与 `lib.rs`；确认 crate 名、Go 包映射、`base`/`expression`/`ranger` 依赖来源、模块公开与再导出、`join_operator_core!`、`JoinCompareFunc`、通用基类路由及变体保型。
- Rust 直接调用/状态证据：`cache_snapshot.rs`、`plan_clone_generated.rs`、`base_physical_plan.rs`、`cache_snapshot_test.rs`、`canonical_router_aster_unit_test.rs`、`pkg/executor/statement_ru_plan_walk_test.rs`。
- 独立 Rust 测试：`physical_index_merge_join_test.rs`；确认当前专属测试只断言 Legacy 比较函数字符串向量的预留容量计费。
- Go 对照：`physical_index_merge_join.go`、`base_physical_join.go`、`single_scan_index_join.go`、`physical_utils_test.go`，以及 planner core 的 `core_init.go`、`task.go`、`plan_cost_ver1.go`；确认字段、解释、内存、成本函数指针和任务装配的实际位置。
- 搜索证据：`rg` 找到 Rust 完整类型的构造点（缓存测试、canonical router 测试、executor RU 遍历测试）、缓存捕获/恢复点和 crate 宏接线；Go 搜索找到 planner 枚举、成本、task、hint/cache/stringer 等消费者。
- RustCodeGraph：`status` 成功；目标文件过滤无命中，故未将图查询当作该文件调用边的充分证据，后续结论均由直接源码与引用搜索支撑。
- 结构验收使用任务指定命令，要求目标文件存在且固定二级标题恰好为 11 个；本任务为纯文档分析，按计划不运行 Cargo。
