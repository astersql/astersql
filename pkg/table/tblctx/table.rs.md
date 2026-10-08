# `pkg/table/tblctx/table.rs`

源文件：[`table.rs`](./table.rs)  
Go 对照：[`table.go`](./table.go)

## 文件定位

本文件位于 `astersql-table-tblctx` crate，是表写入路径与会话、DDL/导入等上层运行环境之间的能力契约。它不执行完整的增删改，也不持有事务；它把行编码、统计增量、缓存表、临时表、交换分区约束检查和 ID 分配所需的环境能力拆成 trait，供表实现通过统一上下文访问。crate 入口 `pkg/table/tblctx/lib.rs` 以私有 `mod table` 加公开 `pub use table::*` 导出这些 API；`pkg/table/tblctx/Cargo.toml` 声明 crate 名为 `astersql-table-tblctx`，并通过本地依赖接入 `model`、`autoid`、`stmtctx`、`variable`、`rowcodec`、`tableutil`、`exprctx` 与 `infoschema`。

在 Rust 表 API 中，`pkg/table/table.rs` 的 object-safe `MutateContext` 对本文件的正式 `MutateContext` 做 blanket adapter，使 `Table::AddRecord`、`UpdateRecord`、`RemoveRecord` 等动态分发入口能接收具体实现。当前主要会话实现是 `pkg/table/tblsession/table.rs::MutateContext<C>`；该实现从 session 状态生成编码配置、返回复用缓冲，并按事务上下文是否存在暴露可选能力。

## 核心职责

1. `RowEncodingConfig` 汇集一次行编码需要的 checksum 开关和编码器，避免表层直接依赖完整 session。
2. `MutateContext` 统一暴露表达式上下文、连接/受限 SQL 标志、事务断言、mutation checker、写缓冲、行 ID 分片器、预留 ID 分配器以及四组可选能力；`AllocatorContext` 是其父契约。
3. `StatisticsSupport`、`CachedTableSupport`、`TemporaryTableSupport`、`ExchangePartitionDMLSupport` 将只在部分运行环境存在的能力拆开，并以 `(Option<&mut T>, bool)` 表达是否可用。
4. `TemporaryTableHandler` 把事务内临时表对象与可选的会话已提交大小数据源组合起来，提供元数据、dirty size、committed size 和增量更新操作。
5. `TemporaryTable`、`TemporaryTableData` 的 blanket impl 分别把 `tableutil::TempTable` 和 `variable::TemporaryTableData` 接到本地小接口上，既保持生产依赖，又允许独立测试使用轻量 mock。

本文件没有 SQL 解析、KV 写入、大小上限报错或提交逻辑；这些属于调用方。Go 的实际消费证据包括 `pkg/table/tables/tables.go` 的行编码/统计/临时表路径、`pkg/table/tables/cache.go` 的缓存表登记，以及 `pkg/table/tables/partition.go` 的交换分区约束检查。Rust 侧已有正式上下文和 `pkg/table/table.rs` 桥接，但不能仅凭本文件推断所有 Go 消费路径均已完整移植。

## 主要符号

- `pub struct RowEncodingConfig`：包含 `IsRowLevelChecksumEnabled: bool` 和 `RowEncoder: Option<rowcodec::Encoder>`。`Option` 对应 Go 指针可能为空；具体会话实现目前构造 `Some(Encoder)`。
- `pub trait StatisticsSupport`：`UpdatePhysicalTableDelta(physicalTableID, delta, count)` 将物理表行数/修改量增量交给运行环境记录。
- `pub trait CachedTableSupport`：`AddCachedTableHandleToTxn(tableID, handle)` 接收 `Box<dyn Any + Send + Sync>`。类型擦除避免表上下文反向依赖缓存表具体类型，同时要求跨线程安全。
- `pub trait TemporaryTable`：本地最小接口 `GetMeta`、`GetSize`、`SetSize`；所有 `tableutil::TempTable` 自动实现它。
- `pub trait TemporaryTableData`：按表 ID 查询会话级已提交临时表大小；所有 `variable::TemporaryTableData` 自动实现它。
- `pub struct TemporaryTableHandler`：私有字段 `tblInTxn` 与 `data`，确保调用方只能通过受控方法读取/调整大小。
- `pub fn NewTemporaryTableHandler<T>`：取得拥有所有权且为 `'static` 的临时表，将其装箱，并保存可选 committed-size 数据源。
- `TemporaryTableHandler::{Meta, GetDirtySize, GetCommittedSize, UpdateTxnDeltaSize}`：分别代理元数据、事务内大小、会话已提交大小，并执行 `new_size = old_size + delta`。
- `pub trait TemporaryTableSupport`：提供大小限制和 `AddTemporaryTableToTxn`；后者同时返回可选 handler 与 Go 风格的成功布尔值。
- `pub trait ExchangePartitionDMLSupport`：以关联类型约束 `InfoSchema: MetaOnlyInfoSchema`，返回交换分区约束检查所需的 infoschema。
- `pub trait MutateContext: AllocatorContext`：核心聚合契约。关联类型保留各实现的具体类型，避免在正式边界全部擦除；`pkg/table/table.rs` 再在动态表 API 边界转换为 trait object。
- `pub trait AllocatorContext`：`AlternativeAllocators` 允许全局临时表改用会话内 ID allocator；布尔值指明替代集合是否有效。

文件无模块级常量、enum、条件编译项或错误类型；所有公开行为均由上述 struct、trait、构造函数和 handler 方法组成。

## 执行流程

常规会话写入的已验证装配流程如下：

1. `pkg/table/tblsession/table.rs::NewMutateContext` 从 session 取出 `WriteStmtBufs`，构造持有复用缓冲的会话 `MutateContext<C>`。
2. 表写入通过 `pkg/table/table.rs` 的 object-safe adapter 调用本文件 trait。编码路径调用 `GetRowEncodingConfig`；会话实现仅在 row checksum 开启、编码器开启且不是 restricted SQL 时启用行级 checksum。
3. 写入成功后，调用方可通过 `GetStatisticsSupport` 取得支持对象，并调用 `UpdatePhysicalTableDelta`。会话实现仅在事务上下文存在时返回支持。
4. 缓存表写入通过 `GetCachedTableSupport` 和 `AddCachedTableHandleToTxn` 将 handle 登记到事务上下文；Go 证据在 `pkg/table/tables/cache.go::txnCtxAddCachedTable`。
5. 临时表写入先通过 `GetTemporaryTableSupport` 获取能力。`pkg/table/tblsession/table.rs::AddTemporaryTableToTxn` 查找临时表、标记 `modified`，再用 `NewTemporaryTableHandler(table, data)` 组合事务内表和会话数据。
6. handler 的 `GetCommittedSize` 用 `Meta().ID` 查询已提交大小；缺少 `data` 时返回 0。每次事务内存缓冲大小变化后，调用方用 `UpdateTxnDeltaSize(delta)` 累加 dirty size。Go `checkTempTableSize` 将 committed 与 dirty 相加后和限制比较。
7. 交换分区写入通过 `GetExchangePartitionDMLSupport` 取得支持对象，再由 `GetInfoSchemaToCheckExchangeConstraint` 提供元数据视图以定位交换目标并检查约束。
8. 需要为全局临时表分配 ID 时，表层经 `AllocatorContext::AlternativeAllocators` 请求替代 allocator；没有替代项时必须遵循 `bool == false`，不能把默认空集合当成有效覆盖。

非会话环境可以只实现所支持的子集，并让各可选 getter 返回不可用。Go 的 DDL reorg 与 Lightning 实现即采用该模式；Rust 中同名模块存在部分独立实现，但是否完整实现本文件正式 trait 应以各 impl 为准，不能按同名方法推定。

## 数据与状态

`RowEncodingConfig` 是按调用返回的配置值，不在本文件内部缓存。`MutateContext` 借用实现者拥有的表达式上下文、缓冲、分片器和可选支持对象；其 `&mut self` getter 保证同一时刻对这些可变资源只有一个安全借用。

`TemporaryTableHandler` 拥有 `Box<dyn TemporaryTable>` 与可选的 `Box<dyn TemporaryTableData>`。dirty size 存在 `tblInTxn` 内，由 `GetSize`/`SetSize` 维护；committed size 不复制到 handler，而是每次以表 ID向 `data` 查询。因此底层数据源可观察更新时，后续 `GetCommittedSize` 也会返回新值，`pkg/table/tblsession/migration_aster_unit_test.rs` 对应测试验证了这一点。

`UpdateTxnDeltaSize` 直接执行有符号 `i64` 加法，允许正负增量，也未在本层做非负约束或溢出处理。独立测试 `pkg/table/tblctx/table_test.rs::update_txn_delta_size_accepts_go_int_range` 明确验证大于 `i32::MAX` 的增量不会被缩窄；`migration_aster_unit_test.rs::temporary_table_handler_matches_go_size_semantics` 验证正负累加、缺少 data 返回 0 和 data 透传。

关联类型 `ExprContext`、`Statistics`、`CachedTables`、`TemporaryTables`、`ExchangePartitions` 可为 unsized 类型；`RowIDShardGenerator` 未施加 trait 约束，由更靠近表 API 的桥接层补充实际所需行为。这种分层使本 crate 不必导入所有具体表实现。

## 依赖与调用关系

上游实现与装配：

- `pkg/table/tblsession/table.rs::MutateContext<C>` 实现 `AllocatorContext`、`MutateContext` 及四个 support trait；`AddTemporaryTableToTxn` 是 `NewTemporaryTableHandler` 的已验证 Rust 生产调用者。
- `pkg/table/table.rs::MutateContext` 通过 blanket impl 委托本文件同名正式 trait，并把关联类型引用转换成 object-safe trait object，供 `Table` 的增删改接口使用。
- `pkg/table/tblctx/lib.rs` 定义依赖再导出并公开本文件全部符号；测试通过独立的 `table_test.rs` 与 `migration_aster_unit_test.rs` 模块接入，没有把测试嵌入生产源文件。

下游能力依赖：

- `rowcodec::Encoder` 提供行编码器；`variable::AssertionLevel` 和 `stmtctx::ReservedRowIDAlloc` 提供事务断言与预留 ID 状态。
- `autoid::Allocators` 是替代分配器返回类型；`model::TableInfo` 提供表 ID、临时表类型和元数据。
- `exprctx::ExprContext`、`infoschema::MetaOnlyInfoSchema` 分别约束表达式求值和交换分区元数据视图。
- `tableutil::TempTable`、`variable::TemporaryTableData` 通过 blanket impl 适配为本地临时表边界。

RustCodeGraph 对 `table.rs` 报告 45 个符号并显示该文件被 63 个已索引文件引用；精确查询定位了 `pkg/table/tblsession/table.rs`、`pkg/table/table.rs` 和相关测试。对泛型构造函数执行 callers/callees 查询返回空边，说明索引没有解析该调用边，因此文档以已索引源码中的显式调用 `tblctx::NewTemporaryTableHandler(table, data)` 补证，不把空图解释为“没有调用者”。

## 错误处理与边界

本文件的方法签名均不返回 `Result`，也不创建领域错误；它定义的是能力和状态访问边界。实际编码错误、临时表超限、交换分区目标缺失等错误由消费层产生。例如 Go `pkg/table/tables/tables.go::checkTempTableSize` 负责生成 `ErrTempTableFull`，`pkg/table/tables/partition.go::checkConstraintForExchangePartition` 负责不支持能力、类型不匹配和查表失败等错误。

几个重要边界必须由调用方尊重：

- 可选 support 的 `Option` 与 `bool` 应保持一致；`bool == false` 表示能力不可用，不应调用缺失对象。
- `GetCommittedSize` 在 `data == None` 时明确返回 0，但这只表示当前上下文没有已提交数据源，不证明表从未有已提交数据。
- `UpdateTxnDeltaSize` 不校验结果非负，也不使用 checked arithmetic；delta 必须来自可信的事务大小差，调用方承担范围不溢出的责任。
- `CachedTableSupport` 的 handle 只有 `Any + Send + Sync` 静态约束；具体类型约定由生产者和提交阶段消费者共同维护，本文件无法做动态类型语义校验。
- `ExchangePartitionDMLSupport::InfoSchema` 必须实现 `MetaOnlyInfoSchema`；跨到 object-safe 适配层后还要求 `Any + Sized`，新增实现需同时检查两层约束。
- `RowEncoder` 是 `Option`，消费端不能无条件解包；虽然当前会话实现返回 `Some`，接口本身允许缺失。

## 并发与资源生命周期

`TemporaryTable` 要求 `Send + Sync`，`CachedTableSupport` 的擦除 handle 也要求 `Send + Sync`，因此对象可被放入具有跨线程约束的事务/session 结构中。但 `TemporaryTableHandler` 的大小变更需要 `&mut self`，本文件没有内部锁、原子变量、任务或 channel；并发串行化由其所有者负责。

handler 对临时表和 data 使用拥有所有权的 `Box`，没有借用外部栈帧；构造参数要求 `T: 'static`，从类型层面阻止保存短生命周期引用。handler 被丢弃时两个 box 随之释放。blanket impl 本身不改变底层对象的同步或释放语义。

`MutateContext` 的可变 getter 返回绑定于 `&mut self` 的借用，Rust 借用规则防止同时取得互相冲突的 buffer/support/allocator 可变引用。`GetExprCtx`、连接标志和编码配置只需共享借用。是否存在更高层 mutex、事务锁或 session 单线程约束属于具体实现；本文件不作保证。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/table/tblctx/table.go`：`RowEncodingConfig`、四个 support 接口、`TemporaryTableHandler`、`MutateContext` 与 `AllocatorContext` 均保留 Go 名称和职责。主要语言映射如下：

- Go 接口返回值在 Rust 中用关联类型表达，随后由 `pkg/table/table.rs` 在需要动态分发的位置对象化；这避免在底层接口中无差别使用 `dyn Any`。
- Go 的 `*rowcodec.Encoder` 映射为 `Option<rowcodec::Encoder>`；Go 的 nil 接口映射为 `Option` 加可用布尔值。
- Go `any` 缓存表 handle 映射为 `Box<dyn Any + Send + Sync>`，增加了 Rust 跨线程类型约束。
- Go 的 `tableutil.TempTable` 与 `variable.TemporaryTableData` 直接作为字段接口；Rust 增加两个本地 trait 和 blanket impl，以便隔离依赖并构造小型 mock。
- Go `UpdateTxnDeltaSize(delta int)` 在 Rust 使用 `i64`。这与受支持的 64 位生产目标语义一致，并由 `table_test.rs` 的大于 `i32::MAX` 回归测试锁定。
- Go handler 方法使用指针 receiver；Rust 的只读方法用 `&self`，变更 dirty size 的方法用 `&mut self`。
- Go 注释称 `AlternativeAllocators` 的“第二返回值为 nil”属于文字错误，实际签名和 Rust 都是 `bool`；语义由 `bool` 表示是否提供替代 allocator。

Go 表写入消费链比本文件更完整可见：`tables.go` 在 Add/Update 路径使用编码、统计和临时表能力，`cache.go` 使用缓存表能力，`partition.go` 使用交换分区能力。Rust 已有接口、session 实现和 object-safe 桥接；未在直接证据中发现与这些 Go 文件完全等价的所有 Rust 消费逻辑，因此这里只陈述已接线部分，不宣称全链路已经移植完成。

## 扩展指南

新增上下文能力时，应先判断它是所有表变更都需要的核心能力，还是可选的独立 support：前者增加到 `MutateContext`，后者优先新增小 trait 和 `Get...Support` getter，避免强迫 DDL/Lightning/测试上下文持有无关状态。任何新增项都要同步检查 `pkg/table/table.rs` 的 object-safe adapter、`pkg/table/tblsession/table.rs` 的会话实现，以及其他正式 trait 实现者；仅添加同名固有方法不能满足 trait 接线。

修改行编码配置时，需同步 `RowEncodingConfig`、`pkg/table/tblctx/buffers.rs` 的消费方式和 `pkg/table/tblsession/table.rs::GetRowEncodingConfig`，并覆盖 disabled/restricted SQL/编码器缺失等组合。修改临时表大小语义时，应保持 committed 与 dirty 的来源分离，明确负 delta 与溢出策略，并同步独立测试 `pkg/table/tblctx/table_test.rs`、`pkg/table/tblctx/migration_aster_unit_test.rs` 及会话级 `pkg/table/tblsession/migration_aster_unit_test.rs`。

修改 `TemporaryTable` 或 `TemporaryTableData` 时，要验证其上游依赖仍可通过 blanket impl，并避免把测试实现放进 `table.rs`。修改缓存表 handle 类型时，需要评估 `Any` downcast 契约、`Send + Sync` 兼容性和事务提交阶段的消费者。修改交换分区 infoschema 类型时，要同时满足正式 trait 的 `MetaOnlyInfoSchema` 与桥接层的 `Any + Sized`，并验证约束检查所需方法仍可访问。

兼容性风险主要来自公开 trait 的破坏性变更（所有实现者都需更新）、Go/Rust 数值宽度差异、可选能力双返回值不一致，以及关联类型到 trait object 的转换限制。性能风险主要在不必要地重复构造编码器、丢失 `MutateBuffers` 复用、对临时表 committed size 做昂贵查询，或把原本按借用返回的支持对象改为频繁分配。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `table.rs` 已索引。
- RustCodeGraph `files --filter pkg/table/tblctx`：确认 `table.rs`、`lib.rs`、Cargo 之外的 Go 对照与独立 Rust 测试布局。
- RustCodeGraph `node --file pkg/table/tblctx/table.rs --offset 1 --limit 500`：读取目标文件全部 179 行，并获得 45 个符号及 63 个文件引用概览。
- RustCodeGraph `query`：查询 `RowEncodingConfig`、`TemporaryTableHandler`、`NewTemporaryTableHandler`、`MutateContext`、`AllocatorContext`、`TemporaryTableSupport`、`ExchangePartitionDMLSupport`，定位定义、实现和桥接候选。
- RustCodeGraph `callers/callees`：对精确的 Rust `NewTemporaryTableHandler` 节点查询均返回空；随后由已索引的 `pkg/table/tblsession/table.rs:240-250` 显式调用补足调用边证据，并在本文注明图索引限制。
- 已读生产文件：`pkg/table/tblctx/lib.rs`、`pkg/table/tblctx/Cargo.toml`、`pkg/table/tblsession/table.rs`、`pkg/table/table.rs`；Go 对照/消费文件为 `pkg/table/tblctx/table.go`、`pkg/table/tables/tables.go`、`pkg/table/tables/cache.go`、`pkg/table/tables/partition.go`。
- 已读独立测试：`pkg/table/tblctx/table_test.rs`、`pkg/table/tblctx/migration_aster_unit_test.rs`、`pkg/table/tblsession/table_test.rs` 与 `pkg/table/tblsession/migration_aster_unit_test.rs` 的相关引用。它们覆盖 64 位 delta、正负 dirty 增量、无/有 committed data、事务上下文能力开关、编码配置及交换分区支持。
- `pkg/table/tblctx` 不存在 `doc.go`，因此没有额外的包级 Go 契约文件；最近的权威定义是同目录 `table.go` 和 crate 入口 `lib.rs`。

本任务为纯文档分析，未运行 Cargo 或代码测试。结构验证要求本文恰好包含计划规定的 11 个二级标题；行为结论通过上述源码、索引查询、Go 对照和独立测试进行人工交叉核验。
