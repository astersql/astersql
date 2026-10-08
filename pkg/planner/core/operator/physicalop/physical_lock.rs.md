# `pkg/planner/core/operator/physicalop/physical_lock.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`（见同目录 `Cargo.toml`），实现 `SELECT ... FOR UPDATE / SHARE` 锁定读在物理规划层的表示与枚举。`lib.rs` 通过 `pub mod physical_lock` 声明模块并用 `pub use physical_lock::*` 向上层再导出公开项。

当前 Rust 文件同时保留两套表示，阅读时必须区分：`LegacyPhysicalLock` 与 `ExhaustPhysicalPlans4LogicalLock` 已接入现有 `dyn base::PhysicalPlan` 优化器主链；`PhysicalLock`、`LockInfo` 与 `exhaust_physical_lock` 是基于 `PhysicalPlanNode` 的简化表示，直接调用证据目前来自独立测试 `physical_lock_test.rs`，生产源码中没有找到对 `exhaust_physical_lock` 的调用。文件顶部的大段块注释是更贴近 Go 完整结构的迁移草稿，不是可执行代码。

## 核心职责

- `ExhaustPhysicalPlans4LogicalLock` 把 `logicalop::LogicalLock` 枚举成一个 `LegacyPhysicalLock`：拒绝 MPP 属性、保留必要的子属性、按期望行数缩放统计、继承 query block offset，并把逻辑 schema 复制到物理节点。
- `LegacyPhysicalLock` 承担已接线计划对象的初始化、Explain 文本、上下文克隆和内存估算；`lib.rs` 的 `impl_schema_leaf_operator!(LegacyPhysicalLock)` 为其接入通用物理计划 trait 与 schema producer 行为。
- `PhysicalLock` 保存锁元数据、表到 handle 列的映射、分区物理表 ID 列映射和单个子计划；其方法提供简化的内存估算、Explain、handle 列校验及整体克隆。
- `exhaust_physical_lock` 为简化表示生成 `PhysicalKind::Lock` 节点，并保留“MPP 无候选但枚举完成且发出警告”的行为。

本文件只描述和枚举锁算子，并不在规划阶段实际获取行锁。真实锁执行发生在后续执行器/存储链路；这里的输出用于表达锁类型、等待时间和定位行所需的列。

## 主要符号

- `pub struct LegacyPhysicalLock`：主链使用的兼容表示。`PhysicalSchemaProducer` 持有基类、schema、子节点、统计和上下文；`LockType: String` 是 Explain/缓存快照使用的规范化锁文本；`WaitSeconds: u64` 是锁等待秒数。
- `LegacyPhysicalLock::New(ctx, lock_type, wait_seconds)`：创建 `TypeLock` 基类，初始 offset 为 `0`。
- `LegacyPhysicalLock::Init(ctx, stats, offset, child)`：用真实 query block offset 重建基类，写入统计，并设置恰好一个子节点所需属性。
- `LegacyPhysicalLock::ExplainInfo()`：稳定输出 `"<lock type> <wait seconds>"`。
- `LegacyPhysicalLock::Clone(new_ctx)`：用新上下文克隆基类并深克隆可选 schema；锁类型字符串和等待秒数按值复制。计划缓存主路径另由 `cache_snapshot.rs` 的 `CachedSelectLock` 捕获/恢复。
- `LegacyPhysicalLock::MemoryUsage()`：统计 schema producer 与 `LockType` 已分配容量；不另计 `u64` 字段，因为结构体静态占用由 producer/外围口径处理方式决定。
- `pub fn ExhaustPhysicalPlans4LogicalLock(logical, required)`：优化器枚举入口。返回 `Vec<Box<dyn PhysicalPlan>>`，当前接口用空向量表达无候选，而不是携带完成标志或 `Result`。
- `pub struct LockInfo`：简化表示的锁类型和 `i64` 等待秒数。
- `pub struct PhysicalLock`：简化表示；`table_id_to_handles` 保存各表的 handle 列 ID，`table_id_to_physical_id_column` 保存分区场景物理表 ID 列，`child` 是被加锁的输入计划。
- `PhysicalLock::{memory_usage, explain_info, resolve_indices, clone_for_plan_cache}`：分别估算局部内存、生成 Explain、校验 handle 列是否出现在 child schema 中，以及整体深克隆。
- `pub fn exhaust_physical_lock(lock, property, stats)`：简化枚举入口，返回 `(候选节点, 枚举完成, 警告列表)`。

文件没有条件编译项、模块级常量或本地 trait 定义；测试模块的条件编译声明位于 `lib.rs`。

## 执行流程

主链流程如下：

1. `base_physical_plan.rs` 的逻辑计划枚举路由遇到 `logicalop::LogicalLock` 时调用 `ExhaustPhysicalPlans4LogicalLock`。
2. 若 `required.IsFlashProp()` 为真，函数通过 session variables 调用 `RaiseWarningWhenMPPEnforced`，使用与 Go 相同的警告文案，然后返回空候选。若逻辑计划没有 session context，也返回空候选。
3. 非 MPP 路径调用 `required.CloneEssentialFields()` 形成唯一 child property，避免把不应下传的外围属性原样带入子节点。
4. 函数把 AST 锁类型映射为 `for update`、`for update nowait`、`for update wait`、`for share`、`for share nowait`；未知类型落到 `none`。
5. `LegacyPhysicalLock::New` 创建锁节点，复制逻辑 schema；随后用 `StatsInfo::ScaleByExpectCnt(..., required.ExpectedCnt)` 缩放统计（无统计时用默认值），并由 `Init` 写入 query block offset 与 child property。
6. 通用物理计划枚举继续为候选选择/构造 child task。`base_physical_plan.rs::attach_canonical_root_lock` 将 Lock 视为严格 Root executor，要求一个子任务，并阻止 MPP 分区元数据跨越锁边界；必要时还会剥离位于特定 index join 之上的纯列投影。

简化流程 `exhaust_physical_lock` 更直接：MPP 时返回 `(空, true, 单条警告)`；其他 task type 复制 child schema，以 `child.id + 1` 生成 `PhysicalKind::Lock` 节点，把原 child 移入 `children`，原样附上调用方传入的 `stats` 和完整 `property`。它不自行缩放统计，也不消费两张表映射。

## 数据与状态

锁算子要求单子节点。主链表示把公共状态放在 `PhysicalSchemaProducer/BasePhysicalPlan` 中；锁专属状态只有规范化锁类型和等待秒数。`Init` 会替换内部基类，因此调用顺序应是构造锁专属字段后再初始化公共计划状态。

简化表示使用 `BTreeMap`，使表 ID 的迭代顺序稳定。`table_id_to_handles` 的一个表可对应多个 handle 列 ID；`resolve_indices` 只验证这些 ID 均存在于 `child.schema`。`table_id_to_physical_id_column` 当前在本文件的方法中既不参与校验，也不参与 `memory_usage` 的动态容量计算；它只是被派生的 `Clone/PartialEq` 保留。扩展者不能据此推断分区物理 ID 已被完整解析或执行消费。

`PhysicalLock::memory_usage` 包含结构体静态大小、锁类型字符串容量、每个 handle 向量的容量乘以 8 字节，以及 child 的递归用量；它没有计算 `BTreeMap` 节点开销，也没有计算 `table_id_to_physical_id_column` 的动态开销，因此是局部估算而非精确堆分析。`clone_for_plan_cache` 直接调用派生 `Clone`，会复制字符串、映射、向量与 child，不保留 Go `Lock` AST 指针的 shallow-clone 身份语义。

## 依赖与调用关系

上游与接线证据：

- `lib.rs` 声明并再导出本模块；`impl_schema_leaf_operator!(LegacyPhysicalLock)` 把兼容表示接入 `ConcretePhysicalOperator`/`PhysicalPlan` 通用实现。
- `base_physical_plan.rs` 的枚举路由将 `logicalop::LogicalLock` 指向 `ExhaustPhysicalPlans4LogicalLock`；同文件的 `attach_canonical_root_lock` 处理 Lock 的 Root task 附着。
- `cache_snapshot.rs::CachedPlan::try_capture` 将 `LegacyPhysicalLock` 转成 `CachedSelectLock`，`CachedSelectLock::restore` 用新 context 恢复 schema producer、锁类型和等待秒数。
- RustCodeGraph 对 `LegacyPhysicalLock` 给出的生产实例化边来自 `cache_snapshot.rs::restore`；对 `PhysicalLock` 和 `exhaust_physical_lock` 给出的直接调用边来自 `physical_lock_test.rs`。

主要下游依赖：

- `logicalop::LogicalLock` 提供 AST 锁信息、schema、统计、session context、query block offset，以及 Go 兼容字段 `TblID2Handle/TblID2PhysTblIDCol`；当前主链 `LegacyPhysicalLock` 没有携带后两张映射。
- `property::PhysicalProperty/StatsInfo` 决定 task 类型、child required property 和统计缩放；`PhysicalProperty/Stats`（来自 `physical_common_plans`）服务于简化表示。
- `parser_ast::SelectLockType` 提供锁模式枚举；`plancodec::TypeLock` 标识 Explain/编码中的算子类型。
- `base::{ContextRef, PhysicalPlan}`、`BasePhysicalPlan` 与 `PhysicalSchemaProducer` 提供计划上下文、trait object、schema 和公共生命周期。

同目录 `Cargo.toml` 明确依赖本地 `base`、`logicalop`、`property`、`parser_ast`、`plancodec`、`expression` 等 crate，且以 `[package.metadata.porting] go-package = "pkg/planner/core/operator/physicalop"` 标注 Go 对照包。该 manifest 没有为锁文件设置独立 feature。

## 错误处理与边界

- MPP/Flash 属性是显式不支持边界。主链在有 context 时抬升 session warning 后返回空候选；简化入口把同一文案放进返回值。两者都把枚举视为已处理，不尝试生成可下推的锁节点。
- 主链缺少 session context 时静默返回空候选；统计缺失时使用默认统计。未知 `SelectLockType` 被格式化为 `none`，不会报错。
- `LegacyPhysicalLock::Clone` 仅可能传播 `CloneWithNewCtx` 的 `expression::Error`。缓存快照路径还会传播 schema producer 捕获/恢复错误，具体定义在 `cache_snapshot.rs`。
- 简化 `resolve_indices` 找到第一个不在 child schema 的 handle 列时立即返回字符串错误；成功时不改写列 ID。它不检查物理表 ID 列、子节点数量或锁类型有效性。
- `explain_info` 不做锁类型合法性验证。简化结构要求 `lock` 总是存在；Go 版本则持有可空指针，但 `ExplainInfo` 本身也假定该指针非空。
- `exhaust_physical_lock` 使用 `child.id + 1`；没有溢出保护或全局 ID 分配器接线，因而不应被描述为与主链计划 ID 语义等价。

## 并发与资源生命周期

本文件没有锁、原子变量、异步任务、通道或后台资源。两套计划表示都由调用方按值拥有；`Box<dyn PhysicalPlan>` 或 `PhysicalPlanNode.children` 负责子计划所有权。

主链 context 通常是共享引用类型；`LegacyPhysicalLock::Clone` 与缓存快照恢复都会显式换入 `new_ctx`，避免计划缓存复用旧 session context。Rust `cache_snapshot_test.rs::cached_select_lock_round_trip_retains_lock_mode_and_child` 验证恢复后仍保留锁模式、等待秒数和 child。简化 `clone_for_plan_cache` 没有 new context 参数，只做完整值克隆，因此不能替代主链的跨 context 缓存恢复。

表映射在简化结构中由 `BTreeMap`/`Vec` 独占并在 clone 时深复制，不存在共享可变状态；这也不同于 Go 对只读 AST `Lock` 字段的 shallow-clone 标签。

## 与 Go 版本的对应关系

直接对照文件为 `physical_lock.go`。Go `PhysicalLock` 嵌入 `BasePhysicalPlan`，保存 `*ast.SelectLockInfo`、`TblID2Handle` 和 `TblID2PhysTblIDCol`；Go 枚举函数从 `LogicalLock` 原样传递三组锁元数据，缩放统计，并返回 `([]PhysicalPlan, true, nil)`。MPP 分支发出相同警告并返回“无候选、枚举完成”。

Rust 主链保留了 MPP 拒绝、child essential property、统计缩放、query block offset、schema、锁类型和等待时间，但 `LegacyPhysicalLock` 没有保存 Go 的 handle 与物理表 ID 映射；锁类型被提前降为字符串。Rust 简化 `PhysicalLock` 虽然有两张 ID 映射，却没有接入主枚举路由，且用整数列 ID 代替 Go 的 `HandleCols` trait/interface 与 `expression.Column`。

方法层面对照：Go `ResolveIndices` 先解析基类，再按 child schema 重写每个 `HandleCols`；Rust 简化方法只做存在性校验。Go `CloneForPlanCache` 深克隆基类、handle 切片和物理表 ID 列，但按标签浅共享只读 AST 锁元数据；Rust 主链依赖 `CachedSelectLock` 快照且没有两张映射，简化版本则整体深克隆。Go `MemoryUsage` 统计两张 map、handle 实例和物理 ID 列；两套 Rust 估算均未达到这一完整口径。

Go 侧 `pkg/planner/core/casetest/plancache/plan_cache_rebuild_test.go` 验证 `TblID2Handle` 的 map、slice 和 context 不得被不安全共享；Rust 侧 `physical_lock_test.rs` 验证非 MPP child property 保留、handle 校验成功与 MPP 警告，`cache_snapshot_test.rs` 验证主链锁节点缓存往返。未找到同目录直接针对 Go `ResolveIndices`/枚举函数的独立 `_test.go`。

## 扩展指南

- 若补齐主链锁元数据，首要修改点是 `LegacyPhysicalLock`、`ExhaustPhysicalPlans4LogicalLock`、`lib.rs` 中的通用 trait 接线以及 `cache_snapshot.rs::CachedSelectLock`；必须同步携带逻辑层 handle/物理表 ID 信息，不能只扩充简化 `PhysicalLock`。
- 若增强列解析，应对照 Go `PhysicalLock.ResolveIndices`：先解析公共基类，再对每个 handle 描述按唯一 child schema 做实际重写，并明确物理表 ID 列为何由逻辑裁剪保留。失败时不得留下被误认为已完整解析的部分状态。
- 若调整 task 附着或允许新执行后端，需同时检查 `ExhaustPhysicalPlans4LogicalLock` 的 MPP 分支与 `base_physical_plan.rs::attach_canonical_root_lock` 的严格 Root 约束，防止 MPP 分区属性越过锁边界。
- 若改变计划缓存语义，需同步 `LegacyPhysicalLock::Clone`、`CachedSelectLock::{capture,restore}`、Rust `cache_snapshot_test.rs`，以及 Go 的 shallow/deep clone 不变量；特别注意 context 必须替换，容器和可变列对象不得跨缓存实例共享。
- 若继续发展 `PhysicalPlanNode` 简化路径，需先建立明确的生产调用点和计划 ID 分配方式，再补 `physical_lock_test.rs` 中的缺列失败、物理表 ID 列、所有锁类型、统计与 clone 隔离测试。Rust 单元测试应继续放在独立的 `physical_lock_test.rs`，不要内嵌到生产文件。
- 兼容风险集中在 Explain 文本、警告文案、锁等待单位、未知锁类型、缓存克隆和分区锁键；性能风险集中在大映射的深克隆与内存估算偏低。任何行为变化都应逐项和 `physical_lock.go` 对照。

## 验证依据

- 目标源码：`pkg/planner/core/operator/physicalop/physical_lock.rs`，已核对全部可执行结构、块注释中的非执行迁移草稿及两套表示。
- crate/模块：`pkg/planner/core/operator/physicalop/Cargo.toml`、`lib.rs`。
- 主链接线：`pkg/planner/core/operator/physicalop/base_physical_plan.rs` 中逻辑枚举路由和 `attach_canonical_root_lock`；`cache_snapshot.rs` 中 `CachedSelectLock` 捕获/恢复。
- Rust 测试：`physical_lock_test.rs`、`cache_snapshot_test.rs`。直接测试分别覆盖简化枚举/handle 校验/MPP 警告，以及主链缓存往返。
- Go 对照：`physical_lock.go`；补充测试证据为 `pkg/planner/core/casetest/plancache/plan_cache_rebuild_test.go`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`query/node` 定位到 `LegacyPhysicalLock`、`PhysicalLock`、`ExhaustPhysicalPlans4LogicalLock` 和 `exhaust_physical_lock`。`node` 显示简化入口的调用者是 `physical_lock_test.rs` 两个测试，`LegacyPhysicalLock` 的生产实例化边来自 `cache_snapshot.rs::restore`。`callers/callees` 对这些精确函数未返回额外边，因此跨模块主链又用上述路由源码直接核验。
- 人工复核结论：该文件存在于逻辑锁到 Root 锁执行器之间；当前真实主链以 `LegacyPhysicalLock` 工作，简化表示尚未成为枚举主入口；安全扩展必须同时维护枚举、Root 附着、缓存快照和独立测试。
