# `pkg/table/tblsession/table.rs`

源文件：[`table.rs`](./table.rs)

## 文件定位

本文件属于 `astersql-table-tblsession` crate，是会话状态与通用表变更接口之间的适配层。crate 入口 `pkg/table/tblsession/lib.rs` 将本文件作为私有 `table` 模块加载，再用 `pub use table::*` 导出其 API；仓库总门面 `pkg/lib.rs` 又通过 `table::tblsession` 暴露该 crate。`pkg/table/tblsession/Cargo.toml` 表明它的生产依赖只有同级 `astersql-table-tblctx`，表达式上下文、元数据、行编码、语句上下文和会话变量类型均由该依赖经 `lib.rs` 再导出。

该文件不执行 SQL，也不拥有表存储实现。它把调用方提供的 `SessionContext` 包装成 `tblctx::MutateContext`、`tblctx::AllocatorContext` 及四个可选支持接口，使表的增删改路径能通过统一 trait 读取会话配置、复用写语句缓冲，并更新事务内统计、缓存表和临时表状态。对应的 Go 实现是 `pkg/table/tblsession/table.go`。

## 核心职责

1. `NewMutateContext` 从会话中移出 `variable::WriteStmtBufs`，用 `tblctx::NewMutateBuffers` 建立一次表变更所需的复用缓冲，并保留会话所有权。
2. `impl tblctx::MutateContext for MutateContext<C>` 透传表达式上下文、连接 ID、restricted SQL 标志、事务断言级别、mutation checker、行 ID 分片器和 reserved row ID allocator，并计算行编码配置。
3. `AlternativeAllocators` 仅为带独立 allocator 的全局临时表返回替代 `autoid::Allocators`；普通表、缺失临时表或缺失 allocator 均返回“不替代”。
4. `TransactionContext` 保存本文件实际会修改的事务状态子集：物理表统计增量与缓存表 handle。三个支持探测方法都以 `HasTxnContext` 为门槛，避免在无事务上下文时声称能力可用。
5. `AddTemporaryTableToTxn` 标记临时表已修改，并把临时表与可选的已提交大小数据源组装为 `tblctx::TemporaryTableHandler`；交换分区约束检查始终通过最新 infoschema 开放。

## 主要符号

- `TransactionContext`：事务状态子集。`TableDeltaMap: HashMap<i64, TableDelta>` 按物理表 ID 累积统计，`CachedTables: HashMap<i64, Box<dyn Any + Send + Sync>>` 保存类型擦除且可跨线程传递/共享的缓存表 handle。
- `TableDelta`：包含 `Delta`（净行数变化）与 `Count`（影响行计数）。`TransactionContext::UpdateDeltaForTable` 使用 `entry(...).or_default()` 初始化后分别累加，两字段都不会被后一次调用覆盖。
- `SessionTemporaryTable`：在 `tblctx::TemporaryTable + Clone + 'static` 上增加 allocator 与 modified 状态访问。`Clone` 的约定是克隆 handle 后仍观察同一份 modified/大小状态；迁移测试用 `Arc<SharedTempState>` 验证这一实现方式。
- `SessionContext`：本 crate 定义的窄生产边界。四个关联类型分别抽象表达式上下文、infoschema、行 ID 分片器和临时表；其方法只暴露本适配器实际需要的会话能力。当前文件没有为具体 session 类型提供实现，实际接线必须由集成层实现该 trait。
- `MutateContext<C>`：持有公开的 `Context: C` 和私有 `mutateBuffers`。它同时实现 `AllocatorContext`、`MutateContext`、`StatisticsSupport`、`CachedTableSupport`、`TemporaryTableSupport`、`ExchangePartitionDMLSupport`。
- `NewMutateContext<C>`：唯一构造入口；要求 `TakeWriteStmtBufs` 能被调用一次并返回有效缓冲。
- 条件编译项：仅 `GetReservedRowIDAlloc` 的缺失分支在 `intest` feature 下调用 `intest::Assert(false, &[])`；默认 feature 为空，生产构建安全返回 `(None, false)`。

## 执行流程

典型流程从 `NewMutateContext(session)` 开始：构造函数先调用 `SessionContext::TakeWriteStmtBufs`，将结果交给 `tblctx::NewMutateBuffers`，然后返回拥有该 session 的 `MutateContext`。之后表层代码通过 `tblctx::MutateContext` 的统一方法按需访问以下分支：

1. 基础字段读取直接转发到底层 `SessionContext`，包括 `GetExprCtx`、`ConnectionID`、`InRestrictedSQL`、`TxnAssertionLevel` 和 `EnableMutationChecker`。
2. `GetRowEncodingConfig` 只有在 session 同时开启行级 checksum、启用 row encoder 且不是 restricted SQL 时才置 `IsRowLevelChecksumEnabled=true`；无论结果如何都会新建一个反映当前 `RowEncoderEnabled` 值的 `rowcodec::Encoder`。
3. 自增分配需要 `AlternativeAllocators` 时，依次检查表为 `TempTableGlobal`、session 能按元数据找到临时表、该临时表存在 allocator；三项都满足才返回单元素替代 allocator 集合和 `true`。
4. 统计、缓存表或临时表路径先调用对应 `Get*Support`。只有 `HasTxnContext()` 为真才返回 `Some(self), true`；调用方取得支持对象后，再由 `GetTxnContextMut()` 完成实际写入。
5. `UpdatePhysicalTableDelta` 将同一物理表的 `(delta, count)` 累加；`AddCachedTableHandleToTxn` 以 `or_insert` 保留首个 handle；`AddTemporaryTableToTxn` 找不到表时返回 `(None, false)`，找到后先 `SetModified(true)`，再建立 handler。
6. 交换分区 DML 支持不依赖事务上下文：`GetExchangePartitionDMLSupport` 固定返回 `Some(self), true`，实际检查时 `GetInfoSchemaToCheckExchangeConstraint` 读取 session 的最新 infoschema。

## 数据与状态

`MutateContext` 拥有传入的 session，因此它观察并修改的是该 session 自身的状态，而不是全局副本。`mutateBuffers` 在构造时从 session 取出，之后只通过 `GetMutateBuffers(&mut self)` 暴露可变借用；这使同一时刻不能经安全 Rust 获得两个可变缓冲引用。

事务状态由 `Option<TransactionContext>` 的等价抽象控制。支持探测与实际写入被分成 `HasTxnContext` 和 `GetTxnContextMut` 两步，因此 trait 实现者应保证两者一致；若前者返回真而后者返回 `None`，操作会静默跳过。`CachedTables` 中的值被类型擦除，读取方必须知道原类型才能安全 `downcast`；本文件仅负责首次插入，不负责消费或回收时机。

临时表的 clone 必须共享状态，而不能深拷贝 modified 标志或大小。`TemporaryTableDataForHandler` 是可选的：`None` 仍可构造 handler，但已提交大小能力取决于 `tblctx::TemporaryTableHandler` 的实现。`GetTemporaryTableSizeLimit` 只透传限制，本文件不执行限额检查。

## 依赖与调用关系

向下依赖均经 `astersql-table-tblctx` 提供：`tblctx::NewMutateBuffers` 消费写语句缓冲；`tblctx::NewTemporaryTableHandler` 组合临时表和大小数据源；`autoid::Allocators` 承载全局临时表的独立 allocator；`rowcodec::Encoder` 构成编码配置；`infoschema::MetaOnlyInfoSchema` 是交换分区检查所需的最小 schema 接口。

RustCodeGraph 对 `pkg/table/tblsession/table.rs` 建立了 46 个符号，并确认文件内直接边包括：`AlternativeAllocators -> SessionContext::GetTemporaryTable -> SessionTemporaryTable::GetAutoIDAllocator`、`UpdatePhysicalTableDelta -> GetTxnContextMut -> TransactionContext::UpdateDeltaForTable`、`AddTemporaryTableToTxn -> GetTemporaryTable/SetModified/TemporaryTableDataForHandler`，以及三个可选支持探测到 `HasTxnContext`。图的文件级结果还列出迁移测试引用；精确 `callers/callees` 命令没有返回额外可用调用方，因此不能据此声称已经接入完整 SQL/DML 主链。

可确认的 crate 出口是 `pkg/table/tblsession/lib.rs` 的 `pub use table::*` 和 `pkg/lib.rs` 的 `table::tblsession` 门面。源代码搜索未发现生产 Rust 中具体 `SessionContext for ...` 的本 crate 实现；当前最直接的可执行使用证据来自 `pkg/table/tblsession/migration_aster_unit_test.rs`。这意味着该适配器的内部语义已实现并测试，但具体 session 的生产接线状态在本任务证据范围内未验证。

## 错误处理与边界

本文件没有 `Result` 返回值，也不创建业务错误；不可用能力使用 `(Option<_>, bool)` 或 `(default, false)` 表示。关键边界如下：

- 全局临时表替代 allocator 的任一前置条件不满足时，返回空 allocator 集合与 `false`，由调用方继续使用原 allocator。
- reserved row ID allocator 缺失时，默认构建返回 `(None, false)`；启用 `intest` 时先触发断言，用于暴露“不应缺失”的测试环境不变量。
- 没有事务上下文时，统计、缓存表、临时表支持均报告不可用；即使绕过探测直接调用统计或缓存写方法，`GetTxnContextMut()==None` 也会使写入成为空操作。
- 缓存表同 ID 重复写入不会覆盖第一次的 handle，这是 `or_insert` 明确保证的不变量。
- 找不到临时表时不会修改任何状态，返回 `(None, false)`；找到时 `SetModified(true)` 发生在 handler 构造之前。
- `NewMutateContext` 本身不处理 `TakeWriteStmtBufs` 的失败或重复调用；该方法不返回 `Result`，具体实现必须保证可取得缓冲。迁移测试的测试 session 使用 `Option::take().unwrap()`，也表明构造应只执行一次。

## 并发与资源生命周期

文件本身不创建线程、异步任务、锁或通道。所有可变操作都要求 `&mut self` 或 `&mut TransactionContext`，依赖 Rust 独占借用串行化同一适配器内的修改。缓存 handle 被约束为 `Any + Send + Sync`，但这只说明 handle 可安全跨线程边界携带，不代表 `MutateContext<C>` 自身自动实现并发共享；是否 `Send`/`Sync` 取决于泛型 `C` 及其关联类型。

`Arc<dyn autoid::Allocator>` 明确共享 allocator 生命周期。临时表 trait 要求 `Clone + 'static`，并约定 clone 共享可观察状态；生产实现需要用 `Arc`、内部同步或等价机制满足这一语义。`mutateBuffers` 与 session 一同由 `MutateContext` 拥有，在 wrapper 被丢弃时释放；临时表 handler 接管临时表 clone 和可选 boxed 数据源，其脏大小生命周期由 `tblctx::TemporaryTableHandler` 管理。

## 与 Go 版本的对应关系

`pkg/table/tblsession/table.go` 的 `MutateContext` 嵌入 `sessionctx.Context` 并直接通过 `vars()` 访问 `SessionVars`；Rust 为避免依赖完整 session 子系统，改为泛型 `MutateContext<C>` 与窄 `SessionContext` trait。Go 的 `TxnCtx` 两个相关 map 在 Rust 中被收束为本地 `TransactionContext`，缓存 handle 从 Go `any` 对应到 `Box<dyn Any + Send + Sync>`。

主要行为保持一致：构造时取得写语句缓冲；全局临时表可替换 allocator；缺失事务上下文时三个可选支持关闭；统计增量累加；缓存 handle 不覆盖；临时表置 modified 并创建 handler；交换分区检查读取最新 infoschema；reserved allocator 缺失时仅测试构建断言而生产安全返回。

需要注意两处表达差异。第一，Go `GetRowEncodingConfig` 返回 session 持有的 `RowEncoder` 指针，而 Rust 根据布尔值新建 `rowcodec::Encoder`，因此 Rust 文档和扩展代码不应假设对象身份共享，只能依赖配置值。第二，Go 的 session/transaction map 是既有完整类型，Rust 当前只建模本 crate 需要的子集，不能把它当成完整事务上下文。`pkg/table/tblsession/table_test.go` 是 Go 行为的权威回归参考；Rust `table_test.rs` 仅保存参考伪代码字符串，而真正执行等价断言的是 `migration_aster_unit_test.rs`。

## 扩展指南

新增会话字段透传时，应先在 `SessionContext` 加入最窄方法，再在 `impl tblctx::MutateContext` 中转发；不要把完整 session 具体类型或无关依赖引入本 crate。若新增的是通用表能力，应先核对 `pkg/table/tblctx/table.rs` 的 trait 定义与 Go `pkg/table/tblctx/table.go`，再决定属于核心 `MutateContext` 还是一个可选 support trait。

扩展事务写入时，修改 `TransactionContext` 及对应 support impl，并保持“能力探测与可变获取一致”的不变量。扩展缓存 handle 时要保留首次插入语义，除非 Go 同步改变；扩展临时表时要保持 clone 共享 modified/大小状态，并明确 handler 所拥有数据源的生命周期。任何编码配置变化都应核对 restricted SQL、row encoder 与 checksum 三者组合，避免只覆盖开关为真的主路径。

测试应放在独立文件，不嵌入 `table.rs`。可执行 Rust 回归优先扩展 `pkg/table/tblsession/migration_aster_unit_test.rs`，并由 `lib.rs` 的测试模块加载；若 Go 行为改变，还应同步检查 `pkg/table/tblsession/table_test.go`。`pkg/table/tblsession/table_test.rs` 当前只做参考文本存在性检查，不能替代行为测试。兼容性风险集中在 trait 方法增加会要求所有实现者补方法；性能风险集中在每次 `GetRowEncodingConfig` 新建 encoder、缓存 handle 的堆分配和临时表 clone 是否保持轻量共享。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点，目标 `table.rs` 被索引为 46 个符号；使用 `files --filter pkg/table/tblsession`、`explore 'pkg/table/tblsession/table.rs symbols responsibilities callers callees'`、`node --file pkg/table/tblsession/table.rs --offset 1 --limit 500`，以及对 `NewMutateContext`、`MutateContext`、`TemporaryTableHandler`、`NewMutateBuffers` 的查询核对符号和直接边。精确 callers/callees 未产生额外明细，故生产主链接线明确标为未验证。
- 源码与 crate：完整阅读 `pkg/table/tblsession/table.rs`、`pkg/table/tblsession/lib.rs`、`pkg/table/tblsession/Cargo.toml`，并核对 `pkg/lib.rs` 的门面导出。
- Go 对照：完整阅读 `pkg/table/tblsession/table.go` 与 `pkg/table/tblsession/table_test.go`，逐项核对构造、字段转发、allocator、统计、缓存表、临时表、交换分区和缺失 allocator 行为。
- Rust 测试：阅读 `pkg/table/tblsession/table_test.rs` 和 `pkg/table/tblsession/migration_aster_unit_test.rs`。后者的三个可执行测试覆盖字段/allocator 路径、可选支持与累加/不覆盖语义、临时表 handler 状态，以及缺失资源的安全返回。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构以任务指定命令验证，人工复核重点是：当前接线限制未被写成“已支持”，测试参考文本未被误称为完整行为回归，所有扩展建议仍把测试放在独立文件。
