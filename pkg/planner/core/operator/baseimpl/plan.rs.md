# `pkg/planner/core/operator/baseimpl/plan.rs`

## 文件定位

本文件属于 crate `astersql-planner-core-operator-baseimpl`，由同目录 `lib.rs` 的 `mod plan; pub use plan::*;` 对外导出。它定义物理算子和简单非物理计划可复用的最小公共元数据 `Plan`，位于规划上下文与具体算子之间：上游由 `physicalop::BasePhysicalPlan::New`、`physicalop::SimpleSchemaProducer::New` 等构造入口创建，下游依赖 `base::ContextRef` 分配节点 ID、读取 EXPLAIN 配置，并持有 `property::StatsInfo`。

它不是完整的计划树节点实现：文件本身不保存 schema、孩子、代价或表达式，也没有直接为 `Plan` 实现 `base::Plan` trait。完整物理节点由 `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 等上层基座组合并转发这些公共能力。当前 Rust 的 `BaseLogicalPlan` 在 `pkg/planner/core/operator/logicalop/base_logical_plan.rs` 中独立保存上下文、类型、ID、查询块和统计字段，没有嵌入本文件的 `Plan`；因此“逻辑/物理共用”是 Go 设计来源和接口语义目标，不应理解为当前 Rust 两条路径已经共用同一个结构体。

## 核心职责

- `NewBasePlan` 从共享规划上下文分配唯一计划 ID，并记录算子类型 `tp` 与查询块偏移 `qb_block`。
- `Plan` 集中提供上下文、身份、统计信息、EXPLAIN 标识、内存估算和不可缓存原因的访问与更新入口。
- `ReAlloc4Cascades` 为 Cascades 重用节点身份：更换类型、重新分配 ID、清空旧统计，同时保留上下文与查询块归属。
- `CloneWithNewCtx` 为普通克隆路径复制元数据、替换上下文，并通过 `Arc` 浅共享统计信息。
- `CloneForPlanCache` 明确采用“默认不支持”策略，要求具体算子或更高层基座显式实现安全的缓存克隆。
- `ExplainId` 延迟读取上下文中的 `ignore_explain_id_suffix`，使已取得的格式化对象仍能反映随后发生的会话选项变化。

## 主要符号

- `pub type PlanContextRef = base::ContextRef`：`Arc<dyn base::PlanContext>` 的本地别名，模拟 Go 接口值的共享传递。
- `pub struct Plan`：保存 `ctx`、`stats`、`tp`、`id`、`qb_block` 和公开字段 `NoncacheableReason`。除不可缓存原因外，字段私有，只能通过本文件方法维护。
- `struct ExplainId<'a>(&'a Plan)`：私有格式化适配器；其 `Display::fmt` 在格式化时决定输出 `tp` 还是 `tp_id`。
- `pub fn NewBasePlan(ctx, tp, qb_block) -> Plan`：唯一的基础构造器；调用 `ctx.alloc_plan_id()`，统计初始为 `None`，不可缓存原因为空串。
- `ReAlloc4Cascades`：重新分配 ID、替换类型并清空统计；不改 `ctx`、`qb_block` 和不可缓存原因。
- `SCtx` / `SetSCtx`、`ID` / `SetID`、`TP` / `SetTP`、`QueryBlockOffset` / `SetQueryBlockOffset`：公共元数据访问器。`TP` 和 `ExplainID` 的布尔切片参数用于保持接口形状，当前基础实现只在 `ExplainID` 中读取上下文配置，不读取切片内容。
- `StatsInfo` / `SetStats`：以 `Option<Arc<StatsInfo>>` 表示统计缺失或共享统计对象；getter 返回借用而不增加引用计数。
- `OutputNames`、`SetOutputNames`、`ReplaceExprColumns`：基础层的空实现。列名和表达式替换由真正拥有 schema/表达式的上层算子负责。
- `MemoryUsage` 与 `PlanSize`：静态结构大小加 `tp` 的 UTF-8 字节长度；不递归统计 `ctx`、`stats` 指向的共享对象，也没有额外重复计算 `NoncacheableReason` 的堆容量。
- `CloneWithNewCtx`：由派生的 `Clone` 先复制全部字段，再只替换 `ctx`；`stats` 的 `Arc`、ID、类型、查询块和原因均保留。
- `CloneForPlanCache`：返回 `(None, false)`；签名保留 `base::Plan` trait object 边界，但基础实现不尝试构造节点。
- `SetNoncacheableReason` / `GetNoncacheableReason`：只接受第一次非空状态下的写入，保留最早发现的违规原因；getter 返回字符串副本。

## 执行流程

1. 具体计划基座调用 `NewBasePlan`，上下文通过 `alloc_plan_id` 产生新 ID；构造结果带空统计与空不可缓存原因。
2. 规划和优化阶段通过访问器读取类型、ID、查询块及上下文。成本或基数推导完成后，上层调用 `SetStats` 绑定共享统计；例如 `BasePhysicalPlan::restore_cached_fields` 同步自身统计后再写入这里。
3. EXPLAIN 需要节点标签时调用 `ExplainID` 得到借用 `Plan` 的 `Box<dyn Display>`。直到调用 `fmt`/`to_string` 时，`ExplainId` 才读取 `ctx.ignore_explain_id_suffix()`：关闭时生成 `类型_ID`，开启时仅生成类型。
4. Cascades 若复用基础计划，调用 `ReAlloc4Cascades` 获取新身份并丢弃与旧身份关联的统计；查询块仍保持原值。
5. 普通跨上下文复制调用 `CloneWithNewCtx`。物理基座随后会深拷贝孩子、schema 等自身字段，并可用自身统计重新设置基础 `Plan` 的统计指针；本文件只负责公共元数据。
6. 计划缓存路径若直接落到 `Plan::CloneForPlanCache` 会得到失败结果；支持缓存的具体物理算子通过上层 `CloneForPlanCacheWithSelf` 或生成的克隆实现选择并复制可共享字段。
7. 当任一阶段发现缓存限制时调用 `SetNoncacheableReason`；后续原因不会覆盖第一个原因，缓存检查或快照逻辑再通过 getter 读取它。

## 数据与状态

`ctx` 是共享的 trait object，决定 ID 分配与 EXPLAIN 后缀策略。`id` 是节点身份，通常在构造或 Cascades 重分配时产生，也允许快照恢复等受控路径用 `SetID` 覆盖。`tp` 是展示和分派所用的算子类型名。`qb_block` 标记节点所属查询块，供子查询、hint 和重写保持作用域。

`stats` 使用 `Option<Arc<_>>`：`None` 表示尚未推导或已因重分配失效；克隆时共享同一对象，不提供本文件内的可变入口。`NoncacheableReason` 以空串表示尚无原因，并用“首次写入获胜”保存最早的诊断。`PlanSize` 只表示 Rust 值本体的编译期大小；`MemoryUsage` 再加类型字符串当前长度，属于与 Go 口径对齐的近似值，而不是完整堆占用统计。

关键不变量包括：`NewBasePlan` 与 `ReAlloc4Cascades` 都通过当前上下文分配 ID；重分配保留查询块但清空统计；普通换上下文克隆保留身份与查询块且浅共享统计；不可缓存原因一旦写入不再被覆盖。

## 依赖与调用关系

同目录 `Cargo.toml` 将本文件装配为独立 crate，并直接声明以下路径依赖：`base` 提供 `ContextRef`、`PlanContext` 和 `Plan` trait；`property` 提供 `StatsInfo`；`types` 提供 `NameSlice`；`expression` 提供列替换参数类型。manifest 还声明 `planctx`，本文件没有直接引用它，但同 crate 的测试实现 `PlanContext` 时使用其会话与表达式上下文类型。

RustCodeGraph 将本文件标记为被 42 个文件使用。可确认的直接生产入口包括：

- `physicalop::BasePhysicalPlan::New -> NewBasePlan`：构建物理计划公共状态；其 `CloneWithNewCtx` 再调用本文件同名方法，并为基础计划重新绑定自身统计副本。
- `physicalop::SimpleSchemaProducer::New -> NewBasePlan`：构建 Insert/Update/Delete 等简单 schema 生产者；`CloneSelfForPlanCache` 使用 `Plan::CloneWithNewCtx`。
- `physicalop::CachedContext::restore -> NewBasePlan -> SetID/SetStats/SetNoncacheableReason`：从无上下文快照恢复基础元数据；`capture` 则读取类型、ID、查询块、统计和原因。
- 多个具体物理算子的 `CloneWithNewCtx` 经各自基座间接进入本文件；文本检索可见 `physical_index_reader.rs`、`physical_sort.rs`、`physical_window.rs`、`physical_union_scan.rs` 等调用点。
- `planbuilder_runtime.rs`、`physical_insert.rs`、`physical_delete.rs` 和 `base_physical_plan.rs` 转发不可缓存原因访问；缓存快照也保存并恢复该原因。

RustCodeGraph 的精确 `query` 能区分 Rust/Go 的 `NewBasePlan`，但当前索引未为本文件多数 inherent method 生成可供 `query/callers/callees` 解析的独立方法节点；调用边因此以文件级“used by”关系、已索引源码节点和限定范围的 `rg` 结果交叉验证，不把空的图查询输出当作“没有调用者”。

## 错误处理与边界

本文件没有业务 `Result` 返回。构造依赖非空的 `Arc<dyn PlanContext>`，因此不像 Go 的 `ExplainID` 那样需要先判断 `ctx != nil`；传入上下文的实现若在 `alloc_plan_id` 或 `ignore_explain_id_suffix` 内 panic，本层不会捕获。

`Display::fmt` 原样传播格式化器错误。`CloneForPlanCache` 用 `(None, false)` 表达可预期的不支持，而非错误或 panic。`OutputNames` 返回空 `NameSlice`，两个 setter/替换方法为 no-op；调用者不能据此推断具体算子没有输出列，只能说明能力不属于该基础层。

`MemoryUsage` 不接受空引用（Rust 借用保证 `self` 存在），与 Go 对 nil receiver 返回 0 的防御分支不同。它也是有意的近似统计：共享上下文、统计对象和不可缓存原因的堆分配不在当前公式中。`SetNoncacheableReason` 只检查现有状态是否为空；若首次传入空串，之后仍可写入非空原因。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或 I/O 资源。共享生命周期由 `Arc` 管理：`ctx` 可在多个计划和克隆之间共享，`stats` 在 `CloneWithNewCtx` 中增加引用计数而不复制统计对象。`Plan` 的变更方法都要求 `&mut self`，因此本层不会并发修改普通字段；是否能跨线程共享还取决于 `base::PlanContext` trait object 及组合它的上层类型约束，不能仅凭这里使用 `Arc` 推断整个计划树可并发访问。

`ExplainId<'a>` 借用原 `Plan`，不能活得比计划更久。它故意不缓存后缀选择，格式化时才读取上下文；独立测试使用原子开关证明在创建 `ExplainId` 后改变上下文状态，最终字符串仍使用新状态。统计对象在重分配时通过将 `Option` 设为 `None` 释放本节点的引用；若其他克隆仍持有同一 `Arc`，对象会继续存活。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/baseimpl/plan.go`。字段与大部分方法一一对应：Go 的 `planctx.PlanContext` 映射为 `base::ContextRef`，`*property.StatsInfo` 映射为 `Option<Arc<StatsInfo>>`，`int` 映射为 `i32`；`NewBasePlan`、Cascades 重分配、空输出名/列替换、Explain 信息、普通克隆、默认缓存克隆失败和首次原因保留均维持 Go 语义。

需要特别注意的差异：

- Go 的 `Plan` 同时嵌入逻辑与物理实现；当前 Rust `BaseLogicalPlan` 是独立移植，只有物理/简单计划路径明确组合本结构。
- Go `ExplainID` 返回闭包式 `fmt.Stringer` 并允许 nil context；Rust 用借用结构 `ExplainId` 实现相同的延迟读取效果，但上下文类型本身不可为空。
- Go `MemoryUsage` 对 nil receiver 返回 0；Rust 不存在安全的 nil `&self` 调用。两边字符串长度都按 UTF-8 字节计数。
- Rust 额外提供 `SetQueryBlockOffset`，为物理算子初始化和移植接线更新查询块；所对照的 Go `plan.go` 只有 getter。
- Rust `CloneWithNewCtx` 返回值而非指针；统计的 `Arc` 浅共享对应 Go 字段上的 `plan-cache-clone:"shallow"` 标记。

同目录没有 Go `*_test.go`。Go 行为依据来自 `plan.go` 本身及其真实调用点；Rust 回归由独立的 `plan_test.rs` 与 `plan_aster_unit_test.rs` 覆盖，不应把测试代码移入生产源文件。

## 扩展指南

新增公共元数据时，应先判断它属于所有计划的最小身份状态，还是只属于逻辑/物理/schema 生产者；后者应放在对应上层基座，避免把树结构或代价状态塞入 `Plan`。若确需新增字段，必须同步审查 `NewBasePlan`、`ReAlloc4Cascades`、派生 `Clone`/`CloneWithNewCtx`、`PlanSize`/`MemoryUsage`、`physicalop::CachedContext::{capture,restore}` 以及计划缓存克隆生成/快照路径，明确该字段应重置、深拷贝还是共享。

改变身份或 EXPLAIN 行为时，要保持 `base::Plan` trait 的 `id/tp/explain_id/query_block_offset` 语义以及 Go `plan.go` 一致；尤其不要把 `ExplainID` 改为创建时就固化字符串，否则会破坏会话开关的延迟读取。改变统计生命周期时，应同时检查 `BasePhysicalPlan::restore_cached_fields` 和 `CloneWithNewCtx` 的重新绑定逻辑，避免基础统计与物理基座统计分叉。

若让基础 `CloneForPlanCache` 成功，必须先证明所有字段的缓存安全语义，并检查具体算子的显式克隆和生成代码是否会重复或绕过深拷贝；默认拒绝是安全边界，不能仅为方便改成普通 `clone()`。不可缓存原因若改变覆盖规则，也会影响缓存资格诊断与快照恢复。

测试应继续放在同目录独立文件：通用行为扩展 `plan_aster_unit_test.rs`，延迟 EXPLAIN 格式化场景扩展 `plan_test.rs`。至少覆盖初始值、重分配保留/清空集合、跨上下文克隆的共享关系、缓存克隆返回契约、Unicode 字节计数以及首次原因策略；涉及上层组合时，在对应 `physicalop` 独立测试中补充集成覆盖。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中收录 `plan.rs`、`plan.go`、`plan_test.rs`、`plan_aster_unit_test.rs` 与 `lib.rs`。
- RustCodeGraph `node --file pkg/planner/core/operator/baseimpl/plan.rs`：读取完整 188 行源码并确认 31 个符号、文件级 42 个使用方；`query NewBasePlan` 同时定位 Rust 第 57 行与 Go 第 41 行。
- RustCodeGraph 精确查询：执行了 `query`、`callers`、`callees`；索引能定位函数和文件级使用关系，但未解析本文件多数 inherent method 的独立调用节点，随后用已索引调用方源码和限定 `rg` 补证。
- crate 与模块边界：读取 `pkg/planner/core/operator/baseimpl/Cargo.toml` 和 `pkg/planner/core/operator/baseimpl/lib.rs`，确认 crate 名、路径依赖、模块导出及两个独立测试模块。
- Go 对照：读取 `pkg/planner/core/operator/baseimpl/plan.go` 全文，并检索 `base_logical_plan.go`、`base_physical_plan.go`、`plan_clone_generated.go` 等调用点。
- Rust 上下游：读取 `pkg/planner/core/base/plan_base.rs` 的 `PlanContext`/`Plan` trait，`pkg/planner/core/operator/physicalop/base_physical_plan.rs` 的构造、克隆、统计与内存路径，`physical_schema_producer.rs` 的简单计划路径，`cache_snapshot.rs` 的捕获/恢复路径，以及 `logicalop/base_logical_plan.rs` 的独立逻辑基座。
- 测试证据：`pkg/planner/core/operator/baseimpl/plan_test.rs` 验证 EXPLAIN 延迟读取；`plan_aster_unit_test.rs` 验证 ID、重分配、查询块、统计浅共享、setter/no-op、缓存克隆失败、首次原因和 UTF-8 字节计数。同目录不存在 Go 测试文件。
- 按任务约束，本次是纯文档分析，未运行 Cargo；交付前另运行固定 11 章节结构验证，并人工复核文档仅陈述上述源码、调用和测试能够支持的事实。
