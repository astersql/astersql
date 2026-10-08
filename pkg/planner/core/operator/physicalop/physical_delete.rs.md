# `pkg/planner/core/operator/physicalop/physical_delete.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate；其 crate 根是同目录的 `lib.rs`，后者以 `mod physical_delete` 装载模块并通过 `pub use physical_delete::*` 导出符号。`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/planner/core/operator/physicalop`，对应的 Go 定义位于 `physical_common_plans.go`，计划缓存克隆逻辑位于 `plan_clone_generated.go`。

这里的 `Delete` 和 `Update` 是优化后 SELECT 子树外层的“非逻辑 DML 计划”。二者实现 `base::Plan`，但没有实现 `base::PhysicalPlan`：真正产出旧行或待删行的是字段 `SelectPlan: Box<dyn base::PhysicalPlan>`，外层 DML 计划保存执行器解释这些行所需的元数据，并声明自身结果 Schema 为空。Rust 构建入口分别在 `pkg/planner/core/planbuilder_runtime.rs::buildDelete` 和 `buildUpdate`。

## 核心职责

- `Delete` 保存单表/多表标志、待删行的 `SelectPlan`、每张表在混合行中的列区间 `TblColPosInfos`、`IGNORE` 标志及外键检查/级联计划。
- `Update` 保存有序赋值 `OrderedList`、常量赋值快速路径标志、虚拟列赋值分界、旧行 `SelectPlan`、`IGNORE` 标志及外键计划；`ResolveIndices` 把赋值两侧的列重新绑定到优化后子计划 Schema。
- `TblColPosInfo` 和 `TblColPosInfoSliceExt` 描述多表 DML 行布局，并按有序的 `Start` 找到一个列序号所属的候选表区间。
- `DeleteIndexLayout`/`DeleteIndexRowLayout` 保存列裁剪后的索引列布局：既支持按索引 ID 查询，也保留稳定遍历顺序。
- 两个静态值 `EMPTY_DELETE_STATS`、`EMPTY_UPDATE_STATS` 为无结果行的 DML 外层提供共享空统计信息。

## 主要符号

- `DeleteIndexLayout`：公开的数据结构，字段 `ID`、`Name`、`Columns`、`Offsets` 分别记录索引标识、名称、索引列名及这些列在裁剪后混合行中的偏移。
- `DeleteIndexRowLayout::{New, Get, Iter}`：`New` 从稳定有序列表建立 `index ID -> ordered 下标` 的 `HashMap`；`Get` 安全地执行两段查找；`Iter` 与 `IntoIterator for &DeleteIndexRowLayout` 都按原始顺序遍历。重复 ID 会让 `by_id` 指向最后一个重复项，但 `ordered` 仍保留全部项；构建方应保证索引 ID 唯一。
- `TblColPosInfo`：公开字段 `TblID`、半开区间 `[Start, End)`、`HandleCols` 和可选 `IndexesRowLayout`。`None` 表示有意未生成索引布局，而不是空索引集合。
- `TblColPosInfo::{MemoryUsage, Cmp}`：前者累计结构本体和 handle 列内存，按 Go 行为不重复计算索引布局；后者只比较 `Start`。
- `TblColPosInfoSliceExt::FindTblIdx`：对按 `Start` 升序排列的切片使用 `partition_point(Start <= col_ordinal)`，返回最后一个起点不大于列序号的候选项；不会检查 `col_ordinal < End`。
- `Delete::{New, HasForeignKeyPlans, MemoryUsage}`：构造空 Schema 的删除计划，检测外键计划并汇总主要持有对象的内存。
- `Update::{New, HasForeignKeyPlans, ResolveIndices, MemoryUsage}`：构造空 Schema 的更新计划、检测外键计划、重解赋值索引并统计内存。
- `impl base::Plan for Delete/Update`：将计划标识、解释信息、上下文、输出名和不可缓存原因委托给 `SimpleSchemaProducer`；返回固定空统计；实现计划缓存克隆。

## 执行流程

DELETE 主链由现有代码证明如下：

1. `planbuilder_runtime.rs::buildDelete` 从语句和逻辑计划收集目标表、公共列、handle 与索引元数据，构造每表的 `TblColPosInfo`。非分区表会建立 `DeleteIndexRowLayout`；分区场景保留 `None`。
2. 构建器插入投影，仅保留删除所需列和 handle，然后优化为 `Box<dyn PhysicalPlan>`。简单主键等值或 `IN` 条件还可把子树替换成 `PointGetPlan`/`BatchPointGetPlan`。
3. `Delete::New` 建立 `TypeDelete` 基座和空 Schema；构建器再写入 `IsMultiTable`、`IgnoreErr`、`TblColPosInfos` 与输出名。
4. 计划遍历时，`pkg/planner/core/flat_plan.rs::FlattenTypedPhysicalPlan` 把 `SelectPlan` 作为 `Delete` 的首个特殊子节点，并在其后追加外键检查与级联节点。`pkg/executor/statement_ru_result.rs` 也识别该具体类型，用于资源消耗计划分类。
5. 通用执行器构建框架在 `pkg/executor/builder.rs` 定义 `DeletePlanData`、`Plan::Delete` 和 `buildDelete`：它先更新时间戳、构建 SELECT 子执行器、构建外键检查/级联，再调用依赖注入的删除执行器工厂。不过仓库内未检索到 `Delete` 到 `DeletePlanData` 的具体适配实现，因此不能据此声称这条 Rust 执行链已经端到端接通。

UPDATE 主链相似：`buildUpdate` 重写赋值并优化保留投影的子计划，填充 `OrderedList` 等字段，随后立即调用 `Update::ResolveIndices`。该方法按列表顺序先解析赋值目标列，再解析右侧表达式；任一步失败就停止并返回错误。通用 `builder.rs::buildUpdate` 还计算赋值标志并构建外键节点，但同样未找到具体 `UpdatePlanData` 适配实现。

## 数据与状态

`Delete`/`Update` 拥有 `SelectPlan`，不通过 `PhysicalPlan::children()` 保存它；需要遍历完整计划的代码必须像 `flat_plan.rs` 一样显式处理这种特殊子节点。两种计划的 Schema 在构造时设置为 `expression::NewSchema(Vec::new())`，`schema()` 通过 `expect` 依赖该构造不变量。

`TblColPosInfos` 的正确性依赖构建方维持两项不变量：各项按 `Start` 升序排列，且 `[Start, End)` 与 `SelectPlan` 的输出布局一致。`FindTblIdx` 只根据 `Start` 找候选，调用者若需要确认真实归属，还必须检查 `End`。`HandleCols` 中列索引也必须指向裁剪后的输出；`buildDelete` 会在建立投影前重算这些索引。

计划缓存有两套现存路径。`base::Plan::clone_for_plan_cache` 深克隆 `SelectPlan` 和 Update 赋值，克隆 Delete 布局；`cache_snapshot.rs` 则把表达式、列和子计划转成上下文无关快照后恢复。两条路径都要求 `FKChecks` 与 `FKCascades` 为空，恢复出的外键向量也固定为空。

内存统计是与 Go 对齐的选择性口径，不等于 Rust 容器的完整堆占用：`Delete::MemoryUsage` 累加 producer、子计划和 handle 列；`Update::MemoryUsage` 再累加赋值；索引布局、外键向量容量以及若干 `String`/`Vec` 容量没有被完整纳入。

## 依赖与调用关系

直接依赖均由同目录 `Cargo.toml` 声明：`base` 提供 `Plan`/`PhysicalPlan` 和上下文，`expression` 提供 Schema、Column、Assignment 与索引解析错误，`property` 提供统计信息，`plancodec` 提供 `TypeDelete`/`TypeUpdate`；本 crate 内的 `physical_schema_producer::SimpleSchemaProducer` 承担通用计划元数据。标准库依赖为 `HashMap` 和线程安全惰性静态值 `LazyLock`。

已验证的上游包括：

- `pkg/planner/core/planbuilder_runtime.rs` 构造并填充 `Delete`/`Update`；这是 SQL 规划主链中的直接生产者。
- `pkg/planner/core/operator/physicalop/cache_snapshot.rs` 捕获和恢复两种计划，并调用 `HasForeignKeyPlans`、`DeleteIndexRowLayout::{New, Iter}`。
- `pkg/planner/core/flat_plan.rs` 显式遍历 `SelectPlan`、`FKChecks`、`FKCascades`。
- `pkg/executor/statement_ru_result.rs` 和 `statement_ru_plan_walk_test.rs` 识别或构造这些类型，用于资源消耗计划遍历。

已验证的下游包括 `SimpleSchemaProducer` 的元数据方法、`PhysicalPlan::{schema, clone_physical, memory_usage}`、`expression::{Column, Assignment}::ResolveIndices/MemoryUsage`。RustCodeGraph 对目标文件建立了 65 个符号节点；精确查询确认 `ResolveIndices`、`FindTblIdx`、`MemoryUsage`、`HasForeignKeyPlans` 等节点。图的通用名称查询歧义较大，且精确 callers/callees 命令未返回可用调用边，所以上述跨文件边由路径限定源码搜索与直接读取核实。

## 错误处理与边界

- `Update::ResolveIndices` 返回 `expression::Error`，目标列或表达式任一解析失败都会原样传播；该方法没有部分回滚，因此失败前的较早赋值可能已被替换。当前直接构建入口在计划交付前使用 `?` 传播错误。
- `DeleteIndexRowLayout::Get` 对未知 ID、失效下标都返回 `None`，不 panic；但是 `New` 不拒绝重复 ID。
- `FindTblIdx` 对空切片或序号位于首个 `Start` 之前返回 `None`；等于边界时选择该边界起始项；超过所有区间时仍返回最后一项。独立测试明确覆盖这些情况。
- `schema()` 中的 `expect` 仅在对象绕过 `New` 且 producer 没有 Schema 时触发；正常构造路径维持空 Schema 不变量。
- `clone_for_plan_cache` 遇到任何外键计划或不可克隆的 `SelectPlan` 时返回 `(None, false)`，而不是生成语义不完整缓存项。
- `SelectPlan` 是必填 `Box`，不同于 Go 的可空接口；Rust 的 `New` 因而要求调用方先拥有有效物理子计划。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。`EMPTY_DELETE_STATS` 和 `EMPTY_UPDATE_STATS` 使用 `std::sync::LazyLock`，初始化线程安全，初始化后只以共享不可变引用返回。

`Delete`/`Update` 通过 `Box` 独占子计划和外键计划；对象释放时这些所有权树随之释放。计划缓存克隆生成独立的子计划和赋值表达式，避免复用可变执行上下文；快照恢复也要求显式的新 `ContextRef`。`DeleteIndexRowLayout` 内部的 `Vec` 与 `HashMap` 由值拥有，没有共享可变状态。类型是否可跨线程最终取决于 `ContextRef`、表达式和 trait object 的约束，本文件没有额外声明 `Send`/`Sync`，因此不应仅凭字段形态推断可并发执行。

## 与 Go 版本的对应关系

主要对应文件是 `pkg/planner/core/operator/physicalop/physical_common_plans.go`：Rust 的 `Update`、`Delete`、`TblColPosInfo`、`TblColPosInfoSliceExt::FindTblIdx`、`Update::ResolveIndices` 分别复刻同名 Go 语义；缓存克隆对应 `plan_clone_generated.go`。

保持一致的关键点包括：DML 外层无结果 Schema；`FindTblIdx` 搜索第一个 `Start > colOrdinal` 再退一位；`Cmp` 只看 `Start`；索引布局不在 `TblColPosInfo::MemoryUsage` 重复计入；外键检查或级联使计划不可缓存；Update 赋值相对 `SelectPlan.Schema()` 重解索引。

当前 Rust 数据模型不是 Go 结构的逐字段全集：Go `Update` 还有 `TblColPosInfos`、`PartitionedTable`、`TblID2Table`，Rust 本文件的 `Update` 没有这些字段；Go 的 `HandleCols` 是 `planner/util.HandleCols` 接口，Rust 使用 `Vec<expression::Column>`；Go 外键字段按表 ID 分组为 map，Rust 是扁平 `Vec<Box<_>>`；Go `Delete` 还有 `CleanTblID2HandleMap`/`matchingDeletingTable`，Rust 本文件没有对应方法。Rust 额外定义本地 `DeleteIndexLayout`/`DeleteIndexRowLayout` 以表达索引 ID 查询和稳定顺序。扩展时应按真实缺口评估，不能假设这些 Go 能力已经由本文件提供。

## 扩展指南

- 新增 DELETE 行布局字段时，至少同步 `TblColPosInfo`、`planbuilder_runtime.rs::buildDelete`、`cache_snapshot.rs` 的捕获/恢复、`Delete::clone_for_plan_cache`（若不能由 `Clone` 自动满足）、内存统计和 `physical_delete_test.rs`/`cache_snapshot_test.rs`。
- 改动 `FindTblIdx` 前必须保留“按 `Start` 排序、返回候选而非验证 `End`”的契约；若要验证完整区间，宜新增单独 API，避免改变 Go 对齐行为。应覆盖空列表、首项前、恰好等于起点、区间间隙和末项后的用例。
- 新增 UPDATE 赋值语义时，应在 `Update::ResolveIndices` 保持左到右顺序，并同步 `planbuilder_runtime.rs::buildUpdate` 与独立测试；涉及虚拟列时必须维持 `VirtualAssignmentsOffset` 的分界含义。
- 修改缓存能力时必须同时审查 `clone_for_plan_cache` 和 `cache_snapshot.rs` 两条路径；外键计划若要可缓存，需要证明其上下文、表元数据及级联子计划能安全克隆，不能只删除拒绝分支。
- 接通执行器时，最可能的桥接点是 `pkg/executor/builder.rs::{UpdatePlanData, DeletePlanData, Plan}` 与具体 `physicalop::{Update, Delete}` 之间的适配层。当前仓库搜索未发现该实现，新增时应提供真实端到端构建测试，而不是只验证 trait 桩。
- 兼容风险集中在 Go 未移植字段、混合行偏移和外键缓存规则；性能风险集中在重复克隆大子计划/赋值树及索引布局的额外复制。任何变更都应避免把 Rust 测试放回生产源文件。

## 验证依据

已读取并交叉核对以下路径：目标源码 `pkg/planner/core/operator/physicalop/physical_delete.rs`；crate 声明与模块入口 `Cargo.toml`、`lib.rs`；直接 Rust 测试 `physical_delete_test.rs`；缓存实现与测试 `cache_snapshot.rs`、`cache_snapshot_test.rs`；构建入口 `pkg/planner/core/planbuilder_runtime.rs`；计划遍历 `pkg/planner/core/flat_plan.rs`；执行器抽象 `pkg/executor/builder.rs`；Go 对照 `physical_common_plans.go`、`plan_clone_generated.go`。

RustCodeGraph 状态显示索引包含目标文件（目标文件 65 个符号）；执行过文件查询、文件节点读取、`ResolveIndices`/`FindTblIdx`/`MemoryUsage`/`HasForeignKeyPlans` 精确查询，以及 callers/callees 尝试。调用图未给出足以消歧的跨文件边，因此用 `rg` 路径限定搜索和相邻源码读取补齐，并明确保留执行器具体适配未验证这一限制。

直接测试 `physical_delete_test.rs` 证明 `FindTblIdx` 的严格起点边界、空切片、首起点前序号和 `Cmp` 只比较 `Start`；`cache_snapshot_test.rs` 证明 Update 赋值与 Delete 布局可往返、索引 ID 可查询，并证明带外键检查/级联的 DML 计划被缓存快照拒绝。本任务是纯文档分析，按计划不运行 Cargo；交付结构校验要求本文恰有上述 11 个固定二级章节。
