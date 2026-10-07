# `pkg/ddl/delete_range_util.rs`

## 文件定位

`delete_range_util.rs` 属于 `astersql-ddl` crate，由 [`pkg/ddl/lib.rs`](lib.rs) 以公开模块 `delete_range_util` 装配。它不是 DDL 作业调度器，也不执行 KV 删除；它是 [`pkg/ddl/delete_range.rs`](delete_range.rs) 的一个内存态辅助模块，为同一轮删除范围生成过程中的“整张物理表”和“某物理表上的某个索引”分配稳定、互不重复的 `element_id`。

生成出的编号最终进入 `DeleteRangeTask.element_id`。在持久化路径中，`delete_range.rs::persist_job_ranges` 将它写入 `mysql.gc_delete_range` 的 `element_id` 列，与 `job_id`、起止 key 和时间戳一起描述待 GC 的键范围。因此，本文件解决的是删除范围记录的对象身份编号问题，范围计算、作业参数解码、SQL 持久化和实际 GC 均在其他模块完成。

## 核心职责

- `ElementIdAllocator` 在一个分配器实例的生命周期内维护两类对象到编号的映射：物理表整体，以及 `(physical_id, index_id)` 索引对象。
- `alloc_for_physical_id` 和 `alloc_for_index_id` 都保证幂等：同一对象重复请求返回第一次分配的编号，不增加计数。
- 两类对象共享同一个从 `1` 开始的递增编号空间。新编号始终为 `physical_ids.len() + index_ids.len() + 1`，所以物理表对象和索引对象之间也不会获得相同编号。
- 索引身份必须同时包含物理表 ID 和索引 ID：分区表的同一个逻辑索引在不同物理分区上会得到不同的 `element_id`。

本文件不负责验证 ID 的业务合法性，不判断 DDL 动作是否需要 GC，也不保证跨进程、跨作业或跨分配器实例的全局唯一性。

## 主要符号

- `pub struct TableIndexId { physical_id: i64, index_id: i64 }`：索引对象的复合键。两个字段公开；派生 `Eq`/`Ord` 供 `BTreeMap` 查找和排序，派生 `Copy`/`Clone` 便于按值构造和传递。
- `pub struct ElementIdAllocator`：分配器。内部的 `index_ids: BTreeMap<TableIndexId, i64>` 与 `physical_ids: BTreeMap<i64, i64>` 均为私有，调用方只能通过分配方法改变映射。`Default` 创建两张空表；`Clone` 会复制当前分配快照；`Eq`/`PartialEq` 允许比较完整状态。
- `pub fn ElementIdAllocator::alloc_for_index_id(&mut self, physical_id: i64, index_id: i64) -> i64`：为一个物理表上的一个索引分配或复用编号。
- `pub fn ElementIdAllocator::alloc_for_physical_id(&mut self, physical_id: i64) -> i64`：为整张物理表的数据范围分配或复用编号。

本文件没有常量、trait、枚举、条件编译项、I/O API 或异步入口。

## 执行流程

索引对象的流程如下：

1. `alloc_for_index_id` 以输入的 `physical_id`、`index_id` 构造 `TableIndexId`。
2. 查询 `index_ids`；命中时立即返回已有编号。
3. 未命中时，把两张映射当前条目数相加并加一，得到下一个共享序列编号。
4. 将复合键和新编号写入 `index_ids`，随后返回编号。

物理表对象的流程相同，但只以 `physical_id` 查询和更新 `physical_ids`。例如，在空分配器上依次请求物理表 `10`、索引 `(10, 3)`、再次请求物理表 `10`，结果依次为 `1`、`2`、`1`。若索引 `(11, 3)` 随后出现，则它是不同复合键并获得 `3`。

上层有两条已接线的生成路径：`delete_range.rs::table_tasks` 调用 `alloc_for_physical_id`，`delete_range.rs::index_tasks` 调用 `alloc_for_index_id`。两者既被内存模型入口 `insert_job_into_delete_range` 使用，也被持久化参数解码入口 `finished_range_batches` 使用。`add_persistent_delete_range_job` 在处理普通作业或多 schema 子作业时共享同一个分配器，再经 `persist_job_ranges` 写入系统表，避免同一父作业内的不同批次或子作业发生编号碰撞。

## 数据与状态

分配器的全部可变状态都在两张 `BTreeMap` 中：

- `physical_ids` 的键只代表整张物理表的数据范围。
- `index_ids` 的键是 `TableIndexId`，代表特定物理表下的特定索引范围。
- 两张表的键空间语义独立，但值空间通过总条目数合并。因此，即使某个物理表 ID 与某个索引 ID 数值相同，也不会混淆两类对象。
- 删除或重编号操作不存在；在实例生命周期内，已分配值保持稳定。`Default` 会重置作用域，新实例再次从 `1` 开始。
- 使用 `BTreeMap` 使键具有确定的有序表示，但当前算法只做按键查找、插入和取长度，不依赖遍历顺序来确定编号；编号顺序由调用顺序决定。

状态不是持久化对象。真正持久化的是调用方生成的 `DeleteRangeTask`/系统表记录；本分配器用完即丢弃。

## 依赖与调用关系

本文件唯一直接依赖是标准库 `std::collections::BTreeMap`，没有使用 `pkg/ddl/Cargo.toml` 中的第三方或工作区依赖。crate 边界由 `Cargo.toml` 的 `[package] name = "astersql-ddl"` 和 `[lib] path = "lib.rs"` 确认。

已验证的调用链为：

`add_delete_range_job` / `add_persistent_delete_range_job` → `insert_job_into_delete_range` / `finished_range_batches` → `table_tasks` 或 `index_tasks` → `ElementIdAllocator::{alloc_for_physical_id, alloc_for_index_id}` → `DeleteRangeTask.element_id`。

持久化分支继续为：

`DeleteRangeTask.element_id` → `persist_job_ranges` → `DeleteRangeExecutor::execute` → `mysql.gc_delete_range.element_id`。

RustCodeGraph 将目标文件标记为仅被 `pkg/ddl/delete_range.rs` 使用；其 `table_tasks` 节点显示调用分配物理表编号并由 `insert_job_into_delete_range`、`finished_range_batches` 调用，`index_tasks` 节点给出相同的索引侧调用关系。两个分配函数的 `callees` 均为空，因为其实现只有映射操作和算术，没有调用仓库内其他业务函数。

## 错误处理与边界

- 两个公开方法直接返回 `i64`，没有 `Result`、错误分支或日志。无效、负数、零值和极值 ID 都会被当作普通键保存；业务有效性必须由上游保证。
- 重复输入不是错误，而是幂等命中。`(physical_id, index_id)` 只有两个字段都相同才算同一索引对象。
- 物理表对象与该表上的索引对象永远属于不同映射，即使输入数字相同，也会分配不同编号。
- 下一个编号先以 `usize` 对两张表的长度求和、加一，再转换为 `i64`。源码没有显式溢出或转换范围检查；达到不可实际容纳的巨大条目数时不提供安全保证，正常 DDL 作业规模下该边界不可达。
- 本模块不处理系统表写入失败、作业参数缺失或 key 编码溢出。这些错误和边界属于 `delete_range.rs`；例如持久化函数以 `Result<(), String>` 传播时间戳获取、参数解码和执行错误。

## 并发与资源生命周期

`ElementIdAllocator` 不包含锁、原子变量、通道、任务或外部资源。分配方法要求 `&mut self`，Rust 借用规则阻止同一实例在普通安全代码中被并发可变访问；类型本身未提供共享并发协议。若未来需要跨线程共享，调用方必须自行在外层同步，并保持一次 DDL 作业内的调用顺序语义。

典型生命周期是在 `add_delete_range_job` 或 `add_persistent_delete_range_job` 开始时通过 `Default` 创建，在该次作业（包括共享分配器的多 schema 子作业和批次）生成完删除范围后释放。克隆分配器会产生独立状态快照；两个克隆后续分别分配可能返回相同的新编号，因此不能把克隆后的实例并行用于同一编号域后再合并结果。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/ddl/delete_range_util.go`](delete_range_util.go)：

- Rust `TableIndexId` 对应 Go `tblIdxID`，字段分别对应 `physicalID` 与 `indexID`。
- Rust `ElementIdAllocator` 对应 Go `elementIDAlloc`，两张 `BTreeMap` 对应两张 Go `map`。
- `alloc_for_index_id` 与 `allocForIndexID`、`alloc_for_physical_id` 与 `allocForPhysicalID` 的命中复用、共享计数和从 `1` 起算的行为一致。
- Go 通过首次调用时判断 `nil` 延迟创建对应 map；Rust 的 `Default` 直接创建空 `BTreeMap`。这是初始化机制差异，不改变可观察的编号语义。
- Go 类型和方法是包内私有；Rust 类型和方法为 `pub`，但字段映射仍为私有。Rust 的公开面更宽，当前实际业务调用者仍只有 `delete_range.rs`。
- Go 的调用点位于 `delete_range.go::doBatchDeleteIndiceRange` 和 `doBatchDeleteTablesRange`；Rust 将等价编号行为接入 `delete_range.rs::index_tasks`、`table_tasks`，并在持久化批次生成中复用。

Go 测试 `pkg/ddl/delete_range_test.go::TestDoBatchDeleteTablesRangeSkipsRewrittenIDs` 通过 `elementIDAlloc` 覆盖整表删除批处理的上层接线，但没有单独断言编号序列或索引复合键。Rust 独立测试 `pkg/ddl/delete_range_test.rs` 验证范围尾键回绕、多 schema 父作业身份和列存索引过滤，也没有针对分配器的直接单元测试。

## 扩展指南

- 若新增删除对象类别，先决定它应复用“物理表”或“物理表 + 索引”身份，还是需要第三个键类型。新增键空间时必须把其条目数纳入共享 `next` 计算，否则可能与现有编号碰撞。
- 若修改身份定义，优先改 `TableIndexId` 和对应分配方法，并同步检查 `delete_range.rs::table_tasks`、`index_tasks`、`finished_range_batches` 及多 schema 子作业共享分配器的路径。
- 若改变起始值、稳定性或编号顺序，必须与 `delete_range_util.go` 保持语义一致，并评估 `mysql.gc_delete_range` 以 `(job_id, element_id)` 区分记录的兼容影响。
- 建议在同目录的独立测试文件 `pkg/ddl/delete_range_test.rs` 增加测试，而不要把测试内嵌进本生产文件。最小用例应覆盖：两类对象共享序列、重复调用幂等、相同索引 ID 在不同物理表下不复用、物理表与索引数字碰撞不混淆，以及克隆后状态独立。
- 不要让分配器承担 key 编码、系统表 I/O 或实际 GC；这些职责应继续留在 `delete_range.rs` 及存储 GC 子系统，以保持本模块为无 I/O 的确定性状态机。
- 性能方面每次分配是 `BTreeMap` 的对数复杂度；若改用其他容器，需要同时考虑 Go 对齐、确定性、哈希依赖和一次作业可能包含的大量分区/索引对象。

## 验证依据

- Rust 源：[`pkg/ddl/delete_range_util.rs`](delete_range_util.rs)；核对了全部 79 行、两个结构体、两个公开分配方法及派生 trait。
- 模块与 crate：[`pkg/ddl/lib.rs`](lib.rs) 的 `pub mod delete_range_util`；[`pkg/ddl/Cargo.toml`](Cargo.toml) 的 `astersql-ddl` 包、`lib.rs` 入口与 Go 包迁移元数据。
- Rust 调用方：[`pkg/ddl/delete_range.rs`](delete_range.rs) 中的 `add_delete_range_job`、`insert_job_into_delete_range`、`table_tasks`、`index_tasks`、`add_persistent_delete_range_job`、`persist_job_ranges`、`finished_range_batches`。
- Go 对照：[`pkg/ddl/delete_range_util.go`](delete_range_util.go) 的 `tblIdxID`、`elementIDAlloc`、`allocForIndexID`、`allocForPhysicalID`；[`pkg/ddl/delete_range.go`](delete_range.go) 的两个批量范围生成调用点。
- 测试证据：[`pkg/ddl/delete_range_test.rs`](delete_range_test.rs) 与 [`pkg/ddl/delete_range_test.go`](delete_range_test.go)。两者提供上层接线与边界背景，但当前没有直接覆盖分配器全部不变量，本文已明确该验证缺口。
- RustCodeGraph：`status` 确认索引包含 11,467 个文件且目标文件有 5 个符号；`files --filter pkg/ddl/delete_range_util.rs` 确认目标已索引；`node --file ...` 核对全文件；`query` 定位 `ElementIdAllocator`、`TableIndexId` 和两个分配函数；`node table_tasks`、`node index_tasks`、`node persist_job_ranges`、`node finished_range_batches` 核对上下游边；两个分配函数的 `callees` 查询均返回无下游调用。精确 `callers` 查询超时，调用者结论由上述调用方节点和精确文本引用交叉验证。
