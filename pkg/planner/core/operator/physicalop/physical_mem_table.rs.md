# `pkg/planner/core/operator/physicalop/physical_mem_table.rs`

## 文件定位

本文件属于 Cargo crate `astersql-planner-core-operator-physicalop`（见同目录 `Cargo.toml`），实现内存表扫描的物理计划节点。这里的“内存表”主要指 `INFORMATION_SCHEMA`、诊断表和性能相关虚拟表：数据不是由普通 KV 表扫描直接读取，而是在执行阶段由进程内逻辑、缓存或相应系统接口产生。

模块入口 `pkg/planner/core/operator/physicalop/lib.rs` 以 `mod physical_mem_table` 纳入实现、以 `pub use physical_mem_table::*` 导出符号，并通过 `impl_schema_leaf_operator!(PhysicalMemTable)` 把该类型接入统一的物理计划 trait。物理计划路由位于 `base_physical_plan.rs`：当动态逻辑节点是 `logicalop::LogicalMemTable` 时，调用本文件的 `ExhaustPhysicalPlans4LogicalMemTable`。

## 核心职责

- `PhysicalMemTable` 保存一次虚拟表扫描在物理规划阶段需要的元数据：数据库名、表定义、投影列、谓词提取器和可选时间范围。
- `New`、`Init` 和 `Clone` 负责建立计划基类、绑定上下文/统计信息/查询块偏移，以及为新上下文复制计划。
- `AccessObject`、`OperatorInfo` 和 `ExplainInfo` 生成当前 Rust 版本的 EXPLAIN 描述。
- `MemoryUsage` 按 Go 字段布局约定估算计划对象占用。
- `ExhaustPhysicalPlans4LogicalMemTable` 检查所需物理属性；只有内存表扫描能直接满足的属性才生成一个候选，并将逻辑节点的字段和输出 schema 搬到物理节点。

本文件只描述和枚举计划，不读取表数据；真正的内存表执行器构造与数据生产不在此文件中。

## 主要符号

### `pub struct PhysicalMemTable`

- `PhysicalSchemaProducer`：持有 `BasePhysicalPlan`、输出 schema、统计和公共物理计划状态。`lib.rs` 的叶子算子宏也通过它实现索引解析与代价委托。
- `DBName: parser_ast::CIStr`：大小写不敏感的数据库标识。
- `Table: model::TableInfo`：目标虚拟表的元信息；当前为拥有所有权的值，而 Go 版本是指针。
- `Columns: Vec<model::ColumnInfo>`：裁剪后需要输出的列定义。
- `Extractor: Option<Box<dyn logicalop::MemTablePredicateExtractor>>`：逻辑阶段已配置的谓词抽取器。trait 定义在 `logicalop/logical_mem_table.rs`，支持对象安全克隆、谓词抽取及可选行数/顺序提示。
- `QueryTimeRange: Option<(i64, i64)>`：供慢查询等诊断表使用的可选时间范围；具体单位及区间解释由消费方决定，本文件仅透传。

### 构造与复制方法

- `PhysicalMemTable::New(ctx)`：用 `plancodec::TypeMemTableScan`、查询块偏移 `0` 创建空计划；其他字段取默认值。
- `PhysicalMemTable::Init(self, ctx, stats, offset)`：重新建立带真实查询块偏移的 `BasePhysicalPlan`，再写入统计信息。它消费并返回 `self`，适合构造链末端调用。
- `PhysicalMemTable::Clone(&self, new_ctx) -> Result<Self, expression::Error>`：通过 `CloneWithNewCtx` 复制公共基类；若原计划已有 schema，则深层调用 `Schema::Clone` 后设置到新 producer；其余拥有所有权的元数据及 boxed extractor 分别克隆，时间范围按值复制。失败仅来自公共基类的换上下文克隆。

### 展示与计量方法

- `AccessObject() -> String`：返回 `table:<Table.Name.O>`。
- `OperatorInfo(_normalized) -> String`：当前固定为空串，且忽略 normalized 参数。
- `ExplainInfo() -> String`：始终先生成 access object；仅当 operator info 非空时才以 `", "` 拼接。按当前实现结果等于 `AccessObject()`。
- `MemoryUsage() -> i64`：累加 producer、`DBName`、一个表指针、一个 slice 头、`Columns.capacity()` 个列指针、一个 interface 头及 `Option<(i64, i64)>` 的大小。它是与 Go 结构布局对齐的估算，不是 Rust 分配器的精确堆追踪。

### `ExhaustPhysicalPlans4LogicalMemTable`

签名为 `(&logicalop::LogicalMemTable, &property::PhysicalProperty) -> Vec<Box<dyn PhysicalPlan>>`。返回空向量表示本算子无法直接满足属性，返回单元素向量表示生成一个内存表扫描候选。

## 执行流程

1. `base_physical_plan.rs` 的逻辑到物理路由识别 `LogicalMemTable`，调用 `ExhaustPhysicalPlans4LogicalMemTable`。
2. 枚举函数先拒绝三类要求：存在 `IndexJoinProp`、MPP 分区类型不是 `property::AnyType`、或排序项非空。内存表扫描本身不承诺这些能力。
3. 从逻辑计划的 `SCtx()` 取得并克隆计划上下文；上下文缺失时返回空候选，而不是构造不完整计划。
4. 通过 `PhysicalMemTable::New` 创建空节点，并复制 `DBName`、`TableInfo`、`Columns`、`Extractor` 和 `QueryTimeRange`。
5. 将逻辑计划的输出 schema 克隆到 `PhysicalSchemaProducer`，确保物理节点输出列身份与逻辑节点一致。
6. 调用 `Init` 写入逻辑计划的统计信息（缺失时使用默认值）和 `QueryBlockOffset`，随后装箱为 `dyn PhysicalPlan` 返回。
7. 后续统一物理计划接口由 `impl_schema_leaf_operator!(PhysicalMemTable)` 提供：EXPLAIN 转发到 `ExplainInfo`，索引解析转发到 producer，内存计量转发到 `MemoryUsage`，两版代价计算转发到 `BasePhysicalPlan`。该节点按叶子算子处理，没有本文件管理的子计划。

## 数据与状态

`PhysicalMemTable` 的状态分为两层：`PhysicalSchemaProducer` 保存通用计划身份、上下文、统计和 schema；本类型字段保存虚拟表特有的访问元数据。候选生成时，这两层均来自同一个 `LogicalMemTable`，因此 schema、列元数据和 extractor 应保持同一轮逻辑优化后的快照。

`Clone` 为新上下文重建公共计划对象，并克隆 schema 和专有字段，避免两个计划共享可变的 schema 或 extractor trait object。`TableInfo`、`ColumnInfo` 在 Rust 中按值拥有；`Columns.capacity()` 而非 `len()` 被用于内存估算，因为已分配但暂未使用的槽位仍占容量。`QueryTimeRange` 是可复制的小值，文件内既不解释也不修改它。

`New` 只产生默认空壳，尚不代表可执行的完整扫描；正常规划路径会继续复制元数据、设置 schema 并调用 `Init`。直接使用 `New` 的调用者必须自行完成这些步骤。

## 依赖与调用关系

上游直接关系：

- `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 将 `LogicalMemTable` 路由到 `ExhaustPhysicalPlans4LogicalMemTable`。
- `pkg/planner/core/operator/logicalop/logical_mem_table.rs` 定义源逻辑节点和 `MemTablePredicateExtractor`；逻辑谓词下推先通过 extractor 保存表侧过滤状态，物理化时本文件克隆并透传该对象。
- `pkg/planner/core/operator/physicalop/lib.rs` 导出本文件符号，并以 `impl_schema_leaf_operator!` 为 `PhysicalMemTable` 实现统一物理计划行为。

本文件的直接 crate 依赖由 `Cargo.toml` 声明：`base` 提供上下文和 `PhysicalPlan`，`logicalop` 提供逻辑节点与 extractor，`property` 提供统计/物理属性，`expression` 提供克隆错误类型，`model`、`parser_ast` 保存表/列/库元数据，`plancodec` 提供 `TypeMemTableScan`。`crate::{BasePhysicalPlan, PhysicalSchemaProducer}` 是同 crate 的公共计划实现。

RustCodeGraph 将该文件标记为被多个规划、执行和测试文件使用；对本任务可直接确认的主链是 `LogicalMemTable` → `ExhaustPhysicalPlans4LogicalMemTable` → `PhysicalMemTable` → 通用 `PhysicalPlan` 接口。索引对带同名方法的精确 callers/callees 查询未给出稳定结果，因此这里不将模糊的同名边当作事实。

## 错误处理与边界

- 属性不兼容和缺少逻辑计划上下文都用空候选表达，不返回错误：这表示“该实现不可选”，由上层规划器继续处理，而不是运行时故障。
- `Clone` 是本文件唯一显式返回 `Result` 的方法；`CloneWithNewCtx` 的 `expression::Error` 原样传播，schema 与字段复制本身没有额外错误分支。
- `StatsInfo()` 缺失时使用 `unwrap_or_default()`；这允许继续枚举候选，但意味着代价依据可能只是默认统计。
- `AccessObject` 只使用表原始名称 `Table.Name.O`，不包含数据库名。若 `Table` 仍是默认值，会得到 `table:`；本文件不负责验证元数据完整性。
- `OperatorInfo` 固定为空，因而 extractor 中已经抽取的过滤状态不会出现在当前 Rust EXPLAIN 文本中。这是现状限制，不能据此推断谓词未被逻辑阶段抽取。
- 本文件不校验时间范围的顺序、单位或端点语义，也不执行谓词；这些约束属于 extractor 或执行器消费方。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。计划对象在优化阶段同步构造，并以 `Box<dyn PhysicalPlan>` 转移所有权；`Clone` 创建独立的拥有型副本。

唯一间接共享的是 `ContextRef`：`New`/`Init` 接收引用计数式上下文，候选枚举从逻辑节点克隆该句柄。生命周期因此由计划对象及其他上下文持有者共同管理。本文件不对上下文内部并发性作额外保证。boxed extractor 通过 `CloneBox` 复制，而非让两个计划直接共享同一可变 trait object；新增 extractor 实现必须保证其 `CloneBox` 能完整复制已抽取的状态。

内存释放遵循 Rust 所有权：计划销毁时，字段、列向量、表元数据和 extractor 自动释放。本文件没有显式清理阶段。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/physicalop/physical_mem_table.go`。

- 字段意图一一对应：Go 的 `PhysicalSchemaProducer`、`DBName`、`Table`、`Columns`、`Extractor`、`QueryTimeRange` 在 Rust 中均有对应字段；Rust 用拥有型 `TableInfo`/`ColumnInfo` 和 `Option<Box<dyn ...>>` 表达 Go 的指针/interface 可空性。
- `Init` 均用 `TypeMemTableScan` 建立基类并设置统计；Rust 枚举函数还显式设置逻辑 schema。
- `MemoryUsage` 有意沿用 Go 的字段头部估算模型。Go 对 nil receiver 返回 0；Rust 方法需要有效引用，不存在 nil receiver 分支。Go 的 `QueryTimeRange.MemoryUsage()` 与 Rust 的 `size_of::<Option<(i64, i64)>>()` 是表示层面的近似对应。
- Go `AccessObject` 返回含 Database 和 Table 的 `ScanAccessObject`，其字符串化在测试场景中表现为表访问描述；Rust 直接返回 `table:<name>`，没有保留结构化数据库字段。
- Go `OperatorInfo` 在 extractor 存在时调用 `Extractor.ExplainInfo(p)`；当前 Rust extractor trait 没有 EXPLAIN 方法，故 Rust 固定返回空串。这是明确的移植差异。
- Go `findBestTask4LogicalMemTable` 还处理 `CanAddEnforcer`：尝试无强制属性的 best task、恢复属性、必要时加 enforcer，并最终构造 `RootTask`。Rust `ExhaustPhysicalPlans4LogicalMemTable` 是候选枚举接口，只接受自身可满足的属性并返回物理计划，不在本函数内封装 RootTask 或执行 enforcer 流程。两者所处规划框架不同，不能把 Go 函数后半段机械复制进 Rust 枚举函数。

## 扩展指南

- 新增物理扫描字段时，应同步修改 `PhysicalMemTable`、`New` 默认值、`Clone`、逻辑到物理的字段复制，并评估 `MemoryUsage`；若字段源于逻辑节点，还需同步 `logicalop/logical_mem_table.rs` 及其独立测试。
- 改变可满足属性时，入口是 `ExhaustPhysicalPlans4LogicalMemTable` 的三项拒绝条件。必须证明新能力由扫描本身或统一 enforcer 真正提供，并在独立测试中覆盖“可生成候选”和“必须拒绝”的边界，不能只让枚举返回非空。
- 补齐 extractor 的 EXPLAIN 信息需要先扩展 `MemTablePredicateExtractor` 契约，再实现 `OperatorInfo`；同时核对所有 trait 实现及 Go 的 `Extractor.ExplainInfo` 语义，避免只改展示层却丢失 normalized 行为。
- 若要提供结构化访问对象，应在统一 `DataAccesser`/EXPLAIN 接口层设计，而不是只拼接字符串；需保持现有 `table:<name>` 用户可见格式兼容。
- 修改克隆逻辑时，要保持 schema 和 extractor 状态彼此独立，并继续传播 `CloneWithNewCtx` 的错误。
- 测试逻辑必须保留在独立文件 `physical_mem_table_test.rs`。现有测试覆盖 EXPLAIN 格式和内存估算；建议新增候选枚举属性矩阵、字段/schema 复制、缺上下文、默认统计及换上下文克隆测试。不要把 `#[cfg(test)]` 测试内嵌到生产文件。

兼容风险主要是 EXPLAIN 文本变化和 Go/Rust 行为偏差；正确性风险集中在漏复制字段或错误声称支持排序/MPP/IndexJoin；性能风险主要来自不必要的元数据/extractor 深克隆，以及错误的属性声明让计划器选择实际无法执行的候选。

## 验证依据

- 目标源码：`pkg/planner/core/operator/physicalop/physical_mem_table.rs`，RustCodeGraph `node --file` 确认 150 行源码及结构体、方法和枚举入口。
- 路由与 trait 接线：`pkg/planner/core/operator/physicalop/base_physical_plan.rs` 中 `LogicalMemTable` 路由；`pkg/planner/core/operator/physicalop/lib.rs` 中模块声明、公开再导出、`impl_schema_leaf_operator!` 定义及对 `PhysicalMemTable` 的调用。
- 逻辑输入：`pkg/planner/core/operator/logicalop/logical_mem_table.rs` 中 `MemTablePredicateExtractor` 与 `LogicalMemTable` 字段和谓词下推契约。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml` 的 package、lib、依赖和 `go-package` 元数据。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_mem_table.go` 的字段、`MemoryUsage`、`Init`、EXPLAIN/访问对象和 `findBestTask4LogicalMemTable`。
- 独立 Rust 测试：`pkg/planner/core/operator/physicalop/physical_mem_table_test.rs`；`explain_info_uses_go_scan_access_object_format` 验证 `table:TABLES`，`memory_usage_counts_go_field_headers_and_column_pointers` 验证容量和字段头部计量。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter` 确认目标有 12 个索引符号；`query PhysicalMemTable` 定位 Rust/Go 类型及相关构造入口。精确 callers/callees 命令未产出可用结果，故调用关系以索引文件节点、路由源码和模块宏交叉核对。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务规定的命令验证本文恰有 11 个固定二级章节，并人工复核所有行为陈述均能回指上述源码或测试。
