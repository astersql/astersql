# `pkg/table/table.rs`

## 文件定位

[`pkg/table/table.rs`](table.rs) 是 `astersql-table` crate 的表层契约文件，对应 Go 的 [`pkg/table/table.go`](table.go)。crate 入口 [`pkg/table/lib.rs`](lib.rs) 以 `#[path = "table.rs"] mod table_impl` 挂载它，并将其公共项重新导出，因此外部 crate 通常通过 `astersql_table::Table`、`astersql_table::BuildTableFromMeta` 等名称使用这里的 API。

它位于 SQL 元数据与实际行存取实现之间：向规划器、导入器和表包装器提供对象安全的表/分区/缓存接口、DML 选项和通用帮助函数，但自身不保存普通表的行数据，也不实现记录键或索引键的具体编码。`pkg/table/Cargo.toml` 表明它直接依赖 `astersql-meta-autoid`、`astersql-kv`、`astersql-expression`、`astersql-meta-model`、`astersql-table-tblctx`、`astersql-types` 等边界 crate。

RustCodeGraph 将该文件识别为 896 行、126 个符号，并显示它被 53 个文件使用。当前树中可确认的生产实现之一是 `pkg/table/mview_log.rs` 的 `MLogTable`：它实现本文件的 `Table`，在委托基础表 DML 的同时写物化视图日志。普通表的完整 KV 行为不在本文件内。

## 核心职责

1. 定义表种类 `Type` 和一组与 MySQL/TiDB errno 对齐的惰性错误原型，统一返回类型为 `TableResult<T> = Result<T, SharedError>`。
2. 定义 `AddRecordOpt`、`UpdateRecordOpt`、`RemoveRecordOpt` 及其 option trait，把 KV 上下文、重复键检查、记录 ID 生成、自增 ID 预留和删除索引布局传入实际表实现。
3. 用 `columnAPI`、`Table`、`PhysicalTable`、`PartitionedTable`、`CachedTable` 描述列视图、行变更、物理分区路由和缓存租约能力。
4. 将带关联类型的 `tblctx_dependency::MutateContext` 适配成可作为 `dyn MutateContext` 传递的对象安全门面；适配层只委托，不拥有会话状态。
5. 提供单值/批量自增 ID 分配、可安装表工厂，以及 CHECK 约束逐条解析求值的公共流程。

## 主要符号

- `Type::{NormalTable, VirtualTable, ClusterTable}`：`#[repr(i16)]` 的表类别，数值分别为 0、1、2；`Is*Table` 方法只做精确枚举比较。
- `table_error!` 与 `ErrColumnCantNull` 等静态量：通过 `LazyLock<Box<terror::Error>>` 按错误类和 errno 延迟构造错误原型。`ErrCheckConstraintViolated` 被本文件的 CHECK 求值路径直接使用。
- `RecordIterFunc`：低层记录迭代回调，接收拥有所有权的 handle、Datum 行和列集合；`Ok(false)` 表示调用者应停止迭代。
- `AddRecordOpt` / `UpdateRecordOpt` / `RemoveRecordOpt`：三类 DML 的累积选项。相应的 `New*Opt` 按切片顺序调用 option 的 `apply*`，后出现的同类设置可覆盖先前值。
- `CommonMutateOptFunc`、`WithCtx`、`WithReserveAutoIDHint`、`IsUpdate`、`SkipWriteUntouchedIndices`：具体 option 适配器/标记。`DupKeyCheckMode` 与 `PessimisticLazyDupKeyCheckMode` 也直接实现 Add/Update option trait。
- `IndexesLayout(HashMap<i64, Vec<i32>>)`：索引 ID 到该索引所需行列偏移顺序的映射，同时实现 `RemoveRecordOption`。
- `MutateContext`：继承 `IndexMutateContext` 和 `AllocatorContext`，暴露受限 SQL、事务断言、行编码、行 ID 分片、统计/缓存/临时表/交换分区支持。泛型 blanket impl 把正式 `tblctx` trait 的关联类型擦除为 trait object。
- `Table`：核心契约，组合列视图、索引/约束、键前缀、`AddRecord`/`UpdateRecord`/`RemoveRecord`、分配器、元信息、校对模式、表类别和可选分区表视图。
- `AutoIncrementContext`、`AllocAutoIncrementValue`、`AllocBatchAutoIncrementValue`：从会话读取步长/偏移并调用表的 `AutoIncrement` 分配器。
- `PhysicalTable` / `PartitionedTable`：前者补充物理 ID；后者按物理 ID 或行值返回分区、列举分区键，并提供交换分区约束检查。
- `TableFromMeta` / `MockTableFromMeta`、`BuildTableFromMeta`：两个受 `RwLock` 保护的可选工厂槽位以及统一读取入口。
- `CachedTable`：缓存初始化、租约窗口读取、远程读锁更新和写锁保活契约。
- `CheckRowConstraint` / `CheckRowConstraintWithDatum`：分别对 `chunk::Row` 和 `Vec<Datum>` 校验可写 CHECK 约束。

## 执行流程

### DML 选项与表调用

调用者先把若干 option trait object 交给 `NewAddRecordOpt`、`NewUpdateRecordOpt` 或 `NewRemoveRecordOpt`；构造器从默认值开始顺序应用选项。实际 `Table` 实现接收原始 option 切片，自行决定何时构造视图。直接实例是 `MLogTable::AddRecord`：先用 `NewAddRecordOpt` 读取更新标志、惰性重复键模式与上下文，再调用基础表 `AddRecord`，最后写日志；`MLogTable::UpdateRecord` 同理在基础更新成功后读取更新选项并写旧/新两条日志。

`UpdateRecordOpt::GetAddRecordOpt` 用于把更新转换为“更新派生的插入”，固定 `isUpdate=true`、`genRecordID=true`；`GetAddRecordOptKeepRecordID` 保持旧 handle，故 `genRecordID=false`。两者复制公共变更选项。删除路径若携带 `IndexesLayout`，实际实现可按索引 ID 取得列偏移顺序，避免重新推导索引行布局。

### 自增分配

`getIncrementAndOffset` 从 `AutoIncrementContext` 读取两个会话参数；当 offset 大于 increment 时按 MySQL/Go 规则把 offset 降为 1。`auto_increment_allocator` 经 `Table::Allocators` 取得 `AllocatorType::AutoIncrement`，缺失时返回显式错误。单值函数以 count=1 调 `alloc` 并返回区间上界；批量函数以请求数量分配区间，再用 `seek_to_first_auto_id_unsigned` 对齐第一个可用 ID，返回 `(first, increment)`。

### 表工厂

`BuildTableFromMeta` 先读 `TableFromMeta`；若已安装则以空的 `Allocators` 和 `TableInfo` 调用生产工厂。否则尝试 `MockTableFromMeta`，再否则返回 `Ok(None)`。锁中毒会转成 `SharedError`。RustCodeGraph/源码搜索在当前树中只找到测试对槽位的写入，没有确认到非测试安装点：`pkg/dxf/importinto/task_executor.rs` 因此把 `None` 变成“table metadata factory is not installed”，而 `pkg/planner/core/operator/physicalop/physical_insert.rs::NewInsertTargetTable` 会回退到 `MetadataTableAdapter`。这说明工厂接线在当前 Rust 迁移状态下并非无条件可用。

### CHECK 约束

`CheckRowConstraint` 对空约束立即成功。否则取得当前数据库，逐个将 `ConstraintInfo.ExprString` 交给 `ParseSimpleExpr`，同时以 `WithTableInfo(current_database, table_info)` 提供列解析上下文；随后调用 `EvalInt`。结果为 0 且非 NULL 时，用约束原始名称生成 `ErrCheckConstraintViolated`；NULL 或非零继续下一条。`CheckRowConstraintWithDatum` 先把 Datum 向量包装成临时 `MutRow`，再委托同一流程。

## 数据与状态

- DML option 是每次操作构造的值对象；`commonMutateOpt` 被克隆到派生 Add/CreateIdx 选项中，`WithCtx` 也克隆 KV `Context`，不借用调用者栈帧。
- `IndexesLayout` 拥有 `HashMap<i64, Vec<i32>>`；应用为删除选项时会克隆整张映射。大映射或频繁删除路径需要注意复制成本。
- `Table` 的行、索引、约束和元信息由具体实现持有；本文件只规定借用或返回 `Arc`/`Box<dyn ...>` 的边界。`AddRecord` 返回拥有所有权的动态 handle。
- `MutateContext` 的统计、缓存、临时表和交换分区支持均以 `(Option<&mut dyn Trait>, bool)` 返回，保留 Go 的“值加 ok”形状；调用者不能只凭 `Option` 猜测 ok 语义。
- 两个工厂槽位是进程级全局状态，初始为 `None`。`BuildTableFromMeta` 每次只在读锁下复制函数指针，不把锁守卫带入工厂执行。
- CHECK 求值不缓存已解析表达式；每次调用、每条约束都会重新解析，并为 Datum 入口构造一个临时可变行。

## 依赖与调用关系

上游直接证据包括：

- `pkg/table/lib.rs` 挂载并再导出本文件。
- `pkg/table/mview_log.rs::MLogTable` 实现 `Table`，调用 `NewAddRecordOpt`、`NewUpdateRecordOpt` 并委托基础表 DML。
- `pkg/planner/core/operator/physicalop/physical_insert.rs::NewInsertTargetTable` 调用 `BuildTableFromMeta`，并使用 `Table::GetPartitionedTable`/`PartitionedTable::CheckForExchangePartition`。
- `pkg/dxf/importinto/{task_executor,encode_and_sort_operator,scheduler,planner,conflict_resolution}.rs` 调用 `BuildTableFromMeta` 获取可执行导入目标。
- `pkg/table/raw_row.rs`、`pkg/util/admin/admin.rs` 等以 `&dyn Table` 消费核心契约；其他规划器/执行器模块通过 `Cargo.toml` 依赖 `astersql-table`。

主要下游为：`kv_dependency` 的 `Transaction`、`Handle`、`Key`、`MemBuffer` 与 `Storage`；`autoid_dependency` 的 allocator；`model_dependency::TableInfo/PartitionInfo`；`expression_dependency` 的构建/求值上下文；`tblctx_dependency` 的正式变更上下文；以及 `chunk_dependency`/`types_dependency` 的行与 Datum 表示。

本文件没有调用具体普通表 KV 编码逻辑。实际实现必须在 `Table` 方法内维护记录键、索引键和事务一致性；这里的 trait 是调用边界，不是这些行为的替代品。

## 错误处理与边界

- 所有本地公共流程用 `TableResult` 传播 `SharedError`；allocator 和 expression 错误分别通过 `SharedError::new` 包装，不吞掉原错误。
- 自增 allocator 不存在时返回 `auto_increment allocator is missing`，而不是 panic；批量 count 的合法性由 allocator 决定，测试证明 count=0 的 `AutoIdError::Canceled` 会保留到调用者。
- 工厂锁中毒分别生成带槽位名的错误；无任何工厂不是错误，而是 `Ok(None)`，因此每个调用者必须明确选择报错或回退。
- CHECK 表达式解析失败或求值失败立即停止并返回；只有“值为 0 且非 NULL”才算违反约束，SQL NULL 不构成失败。
- `PartitionedTable::GetPartition*` 用 `Option`/`Result` 表达不存在和路由失败；`Table::GetPartitionedTable` 对非分区表返回 `None`。
- `SkipWriteUntouchedIndices` 是性能选项，不改变 trait 本身的安全条件。Go 注释明确指出显式事务或存在外键时，后续同事务读取可能看不到正确数据；Rust 调用者应沿用该限制。
- `CachedTable::WriteLockAndKeepAlive` 的退出通道、结果通道和租约指针由实现协调；trait 没有超时、自动取消或 panic 恢复保证。

## 并发与资源生命周期

`TableFromMeta` 和 `MockTableFromMeta` 使用标准库 `RwLock`，允许并发读取和串行替换；测试安装工厂时会保存旧值并恢复。生产代码若新增安装点，应在其他线程开始构表前完成注册，避免不同请求观察到不同提供者。

`CommonMutateOptFunc` 内部是 `Arc<dyn Fn + Send + Sync>`，可跨线程共享其闭包；它应用到 option 时仍只修改调用栈中的独立 option。`Column`、`Index`、`Constraint` 以 `Arc` 返回，具体对象的线程安全要求由其 trait/实现与调用环境共同约束。

`CachedTable` 明确把缓存资源生命周期交给实现：`TryReadFromCache` 返回一个拥有所有权的 `MemBuffer` trait object；读锁由 `UpdateLockForRead` 结合 timestamp/lease 刷新；写锁保活持续到 `Receiver<()>` 收到退出信号，并通过 `Sender<TableResult<()>>` 报告结果。本文件不启动线程，也不负责关闭通道。

自增函数只短暂借用会话和表上下文，allocator 以 `Arc` 返回，可在函数返回后由其他持有者继续使用。CHECK Datum 包装行仅在函数调用期间存在，`row_to_check.clone()` 让每条约束获得可独立传入求值器的行视图。

## 与 Go 版本的对应关系

总体结构与 `pkg/table/table.go` 一一对应：表类型值、错误 errno、option 累积、列/Table/分区/缓存接口、自增步长与 offset 回退、工厂槽位、CHECK 的“0 且非 NULL 失败”规则均保留。`pkg/table/table_test.go` 的错误码与 option 场景在 Rust 的 `pkg/table/table_test.rs` 中有直接对照；更广的迁移契约由 `pkg/table/table_migration_aster_unit_test.rs` 覆盖。

Rust 为对象安全和错误所有权做了必要表达差异：Go interface 变为 `dyn Trait` 与 `Arc`/`Box`；Go 的类型别名 `MutateContext = tblctx.MutateContext` 在 Rust 中是适配 trait 加 blanket impl；Go 可空 map/interface 变为 `Option`；错误统一为 `SharedError`；全局可变函数变量用 `RwLock<Option<fn>>` 包装。

存在三项可见差异需要扩展时保留意识：

1. Go `AllocAutoIncrementValue` 启动 tracing region，Rust helper 当前没有对应 tracing。
2. Go `TableFromMeta` 的注释声明由 TiDB 初始化函数安装；当前 Rust 搜索未发现非测试槽位写入，且 `BuildTableFromMeta` 增加了生产→mock→`None` 的回退协议。
3. Go `CheckRowConstraint` 调 `BuildConstraintExprWithCtx`；Rust 当前直接 `ParseSimpleExpr` 加 `WithTableInfo`，然后执行真实 `EvalInt`。若表达式构建语义继续迁移，应以 Go 构建函数的行为和独立回归测试为准，不能仅凭函数名认为完全等价。

## 扩展指南

- 新增普通表能力时，先判断它是否属于所有表共同契约。若是，修改 `Table` 并同步所有生产实现/包装器（至少检查 `MLogTable`）及独立测试；若只属于分区或缓存，优先放入对应扩展 trait，避免扩大所有实现负担。
- 新增 DML option 时，应把状态放入正确的 `*RecordOpt`，实现相应 option trait，确认 Add/Update/CreateIdx 之间是否需要复制，并在 `pkg/table/table_test.rs` 或 `pkg/table/table_migration_aster_unit_test.rs` 覆盖默认值、组合顺序和互转语义。测试仍须保持独立文件，不应内嵌到 `table.rs`。
- 修改自增逻辑时同时覆盖 offset 大于/小于等于 increment、单值/批量、零数量、allocator 缺失与下游错误保真；还要核对 `pkg/table/table.go` 的返回值协议。
- 安装生产工厂时，应提供显式的一次性初始化位置，并验证规划器回退与导入器强制要求两种调用路径；不要把测试工厂当生产接线。若允许运行期替换，还需定义并发可见性和恢复策略。
- 扩展 CHECK 求值时需同步 `ConstraintInfo` 表达式构建、当前数据库/表信息解析上下文、NULL 三值逻辑及错误码，并在独立 Rust 测试中加入真、假、NULL、解析失败和求值失败用例。
- 修改 `CachedTable` 时应把退出信号、结果发送、租约更新、后台任务终止和 buffer 所有权写成可测试的不变量，防止锁泄漏或保活任务悬挂。

兼容性风险主要是 trait 变更导致所有实现失配、errno/Go 行为漂移和未安装工厂导致运行期失败；性能风险主要是 option/布局克隆、CHECK 每行每约束重复解析，以及不安全使用 `SkipWriteUntouchedIndices` 后的同事务读一致性。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/table/table.rs` 确认目标被索引；`node --file pkg/table/table.rs --offset 1/521` 完整读取 896 行与符号；`query` 核对 `NewAddRecordOpt`、`NewUpdateRecordOpt`、`NewRemoveRecordOpt`、`AllocAutoIncrementValue`、`AllocBatchAutoIncrementValue`、`BuildTableFromMeta`、`CheckRowConstraint*`；`explore` 用于检查目标符号的调用范围。宽泛名称导致部分图结果混入同名 Go/其他模块符号，相关精确调用边又用限定路径搜索和对应源码节点复核。
- Rust 源与 crate 边界：`pkg/table/table.rs`、`pkg/table/lib.rs`、`pkg/table/Cargo.toml`。
- 直接调用/实现证据：`pkg/table/mview_log.rs`、`pkg/planner/core/operator/physicalop/physical_insert.rs`、`pkg/dxf/importinto/task_executor.rs`，以及限定 `*.rs` 搜索得到的其他 `BuildTableFromMeta` 调用点。
- Go 对照：`pkg/table/table.go`、`pkg/table/table_test.go`。
- Rust 独立测试：`pkg/table/table_test.rs` 验证错误码与 option 默认/累积；`pkg/table/table_migration_aster_unit_test.rs` 验证枚举值、对象安全形状、删除布局、工厂安装恢复、自增规则、缺失 allocator 和错误码。
- 人工复核结论：本文件存在的目的，是把表元信息、会话变更上下文和 KV/表达式能力收敛为稳定的动态契约与共用流程；真正的行/索引持久化由实现者完成。安全扩展必须同步 Go 语义、所有相关 trait 实现和上述独立测试，并对当前未完成的生产工厂接线保持显式失败/回退。
