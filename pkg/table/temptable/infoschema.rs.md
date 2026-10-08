# `pkg/table/temptable/infoschema.rs`

## 文件定位

[`infoschema.rs`](infoschema.rs) 属于 `astersql-table-temptable` crate。crate 入口 [`lib.rs`](lib.rs) 将其作为 `infoschema` 模块公开并再导出全部公共项；同一 crate 的 [`ddl.rs`](ddl.rs) 使用这里的会话目录完成本地临时表的创建、删除和截断。它不是完整 TiDB InfoSchema 的移植，而是当前 Rust 临时表子系统自带的最小元数据模型、会话目录和挂载适配层。

[`Cargo.toml`](Cargo.toml) 的 `[lib]` 指向 `lib.rs`，`package.metadata.porting.go-package` 指向 Go 包 `pkg/table/temptable`。当前常规 `[dependencies]` 为空；列出的 AsterSQL crate 全部位于永不成立的 `target.'cfg(any())'` 下，因此本文件实际只依赖标准库以及同 crate `interceptor` 模块的 `MemBuffer`。这说明当前实现仍是一个可独立编译的局部移植边界，不能据此宣称已接入仓库的完整 Rust `pkg/infoschema` 主链。

## 核心职责

本文件承担四组职责：

1. 用 `CiString`、`DbInfo`、`TableInfo`、`Table` 和两个枚举表达临时表路径所需的最小元数据。
2. 用 `SessionTables` 同时维护“库名 + 表名”、表 ID 和库名三个内存索引，供单个会话查找本地临时表。
3. 以 `InfoSchema` trait、`MemoryInfoSchema` 和 `SessionExtendedInfoSchema` 实现“本地临时表优先、基线 InfoSchema 回退”的按 ID 查询。
4. 通过 `SessionVariables`/`SessionVarsProvider` 保存会话目录和临时表数据缓冲，并提供确保、挂载和分离目录的顶层函数。

文件不会访问持久化元数据，也不会自行创建 SQL 层表定义。具体 DDL 接线位于 [`ddl.rs`](ddl.rs)：`SessionTemporaryTableDdl::create_local_temporary_table` 先确保会话数据缓冲，再构造 `Table`，最后调用 `ensure_local_temporary_tables(...).add_table(...)`。

## 主要符号

- `CiString`：保存 `original` 和由 `String::to_lowercase` 得到的 `lower`；显示时保留原文，目录键使用小写值。它提供的是 Unicode 小写归一化，不是完整的数据库排序规则实现。
- `TempTableType::{None, Global, Local}` 与 `SchemaState::{None, Public}`：临时表类型和 DDL 可见状态的局部枚举。
- `DbInfo`、`TableInfo`：库与表的精简元数据。`TableInfo` 含表 ID、库 ID、名称、临时表类型、状态和 `has_auto_id`。
- `Table`：用 `Arc<TableInfo>` 共享不可变元数据，并在构造时把 `has_auto_id` 缓存为 `auto_id_allocator`。`metadata` 和 `has_auto_id_allocator` 是只读访问入口。
- `TempTableError`：统一描述不存在、重复、库缺失、从会话读取普通表、键不存在、存储及迭代器错误。此文件自身直接产生的目录错误主要是 `TableAlreadyExists`；其他变体供同 crate 的 DDL/拦截器复用。
- `SessionTables`：核心会话目录，包含 `tables_by_name`、`tables_by_id`、`schemas` 三个 `RwLock<HashMap<...>>`。公开方法为 `new`、`add_table`、`remove_table`、`table_by_name`、`table_exists`、`table_by_id`、`schema_by_id`、`count` 和 `is_empty`。
- `InfoSchema`：要求实现者可经 `as_any` 下转，并支持 `table_by_id` 与 `has_temporary_table`。`Any + Send + Sync` 允许跨线程共享 trait object 和运行时类型判断。
- `MemoryInfoSchema`：按 ID 保存表的轻量基线实现；`insert` 会覆盖相同 ID，`has_temporary_table` 扫描所有表的 `temp_table_type`。
- `SessionExtendedInfoSchema`：包装 `base: Arc<dyn InfoSchema>`，并以 `Mutex<Option<Arc<SessionTables>>>` 保存可选会话目录、以 `Once` 限制后续挂载。
- `SessionVariables` 与 `SessionVarsProvider`：会话侧存储边界。前者分别保存本地目录和 `MemBuffer`，后者返回共享会话变量。
- `get_local_temporary_tables`、`ensure_local_temporary_tables`：读取或惰性创建会话目录。
- `attach_local_temporary_table_info_schema`、`detach_local_temporary_table_info_schema`：给任意 `InfoSchema` 加上或去掉本地临时表查询层。

## 执行流程

创建本地临时表时，`ddl.rs::SessionTemporaryTableDdl::create_local_temporary_table` 先调用 `ensure_session_data`，再将 `TableInfo.db_id` 设为目标库 ID、构造运行时 `Table`，然后调用本文件的 `ensure_local_temporary_tables`。后者锁住 `SessionVariables.local_temporary_tables`，只在 `None` 时创建 `SessionTables`，最后由 `SessionTables::add_table` 完成登记。

`add_table` 的顺序是：校验非零 `metadata.db_id` 与 `DbInfo.id` 相等；把库名和表名的小写形式组成名称键；拒绝重复名称；拒绝已存在的表 ID；按小写库名保留 `DbInfo`；写入 ID 索引；最后写入名称索引。删除时，`remove_table` 先按名称取走表，再删除 ID 索引；若该库名下已没有任何表，同时删除库索引。

挂载时，`attach_local_temporary_table_info_schema` 首先读取当前会话目录：若尚未创建，原样返回基线对象；若输入已是 `SessionExtendedInfoSchema`，调用 `attach_once` 后仍返回原对象；否则构造新的扩展层。按 ID 查询扩展层时先查会话目录，未命中才调用 `base.table_by_id`，因此相同 ID 的本地表会遮蔽基线表。`has_temporary_table` 则在本地目录非空或基线报告含临时表时返回 `true`。

分离时，`detach_local_temporary_table_info_schema` 仅识别 `SessionExtendedInfoSchema`。命中后它不会直接返回 `base`，而是创建一个仍包装相同基线、但本地目录为 `None` 的新扩展对象；非扩展对象原样返回。

## 数据与状态

`CiString` 的原文用于显示，小写副本用于名称索引。`SessionTables` 的权威内容分散在三个映射中：`tables_by_name` 支持大小写不敏感的二元名称查询，`tables_by_id` 支持 ID 查询和计数，`schemas` 保留仍拥有至少一张会话表的库对象。`count` 与 `is_empty` 以 ID 索引为准。

`Table.metadata` 创建后不可变并通过 `Arc` 共享；`Table.auto_id_allocator` 是构造时快照，之后不会随外部状态变化。`MemoryInfoSchema.tables` 同样按 ID 保存 `Arc<Table>`，但其 `insert` 明确采用覆盖语义，与 `SessionTables::add_table` 的拒绝重复语义不同。

`SessionVariables.local_temporary_tables` 控制目录是否已创建，`temporary_table_data` 控制临时表行数据缓冲是否已创建；本文件只管理前者，并仅声明后者供 `ddl.rs`/`interceptor.rs` 使用。目录与数据缓冲的创建生命周期彼此独立，由 DDL 层按需要协调。

`SessionExtendedInfoSchema::new` 初始就保存一份本地目录，但故意没有消耗 `Once`。因此第一次对该对象再次调用 `attach_once` 会替换构造时的目录，之后的调用才会被忽略。这一看似反直觉的状态机是在源码注释中明确保留的 Go 结构体字面量语义，并由独立 Rust 测试固定。

## 依赖与调用关系

上游直接证据如下：

- `pkg/table/temptable/ddl.rs::SessionTemporaryTableDdl::create_local_temporary_table` 调用 `ensure_local_temporary_tables(...).add_table(...)`。
- 同一 DDL 实现的删除和截断路径使用会话目录的名称查询、移除与重新登记能力。
- [`infoschema_test.rs`](infoschema_test.rs) 直接调用 `attach_local_temporary_table_info_schema` 和 `detach_local_temporary_table_info_schema`，覆盖重复挂载与分离形状。
- `lib.rs` 公开 `infoschema` 模块并再导出其公共 API。

下游依赖主要是标准库的 `Arc`、`Mutex`、`RwLock`、`Once`、`HashMap` 和 `Any`；`SessionVariables.temporary_table_data` 的类型来自 `crate::interceptor::MemBuffer`。`SessionExtendedInfoSchema::table_by_id` 向下调用 `SessionTables::table_by_id` 和基线 `InfoSchema::table_by_id`，`has_temporary_table` 调用 `SessionTables::is_empty` 与基线同名方法。

RustCodeGraph 对三个顶层确保/挂载函数没有显示生产代码中的跨 crate 调用边；精确节点只确认 `attach_local_temporary_table_info_schema` 的内部调用以及两个测试调用者。结合常规 Cargo 依赖为空、真实 AsterSQL 依赖被放在 `cfg(any())` 下，可确认当前接线集中在 `astersql-table-temptable` crate 内，不能把 Go 文件的全应用调用者直接外推成 Rust 调用者。

## 错误处理与边界

`SessionTables::add_table` 对同名或同 ID 表返回 `TempTableError::TableAlreadyExists`。当表元数据的非零 `db_id` 与传入库 ID 不同，它使用 `assert_eq!` 触发 panic，而不是返回 `SchemaNotExists`；独立测试以 `#[should_panic]` 固定了这一不变量。`db_id == 0` 被视为允许登记的特殊情况。

按名称或 ID 查找均以 `Option` 表达未命中，删除不存在的表也返回 `None`。挂载在会话尚无目录时是无操作，分离非扩展对象也是无操作。`InfoSchema` 只提供按 ID 查询，不包含按名、按库或分区查询，因此它不能替代完整的生产 InfoSchema 接口。

所有锁都直接调用 `unwrap()`；持锁线程 panic 导致锁中毒时，后续访问也会 panic。`CiString` 的 `to_lowercase` 不编码 TiDB/MySQL 的具体 collation 规则；例如 Unicode 展开或语言相关大小写行为应通过新增兼容测试验证，而不能假定与 Go `model.CIStr` 的所有边界完全一致。

三个索引的更新不是一个原子事务。每个 `HashMap` 的内存访问受锁保护，但 `add_table`/`remove_table` 会分步获取不同锁；并发读可能看见中间状态，不同名称但相同 ID 的并发添加也不能仅凭一次 ID 预检视为具备全局原子唯一性。当前测试验证单线程不变量，没有证明这些复合操作在线程竞争下的线性化语义。

## 并发与资源生命周期

共享对象均通过 `Arc` 延长生命周期；目录和表元数据没有显式销毁动作，在最后一个 `Arc` 释放时由 Rust 回收。三个目录索引使用独立 `RwLock`，允许各自的并发读，但复合写入跨锁完成。`SessionVariables` 和扩展层的可选目录使用 `Mutex`，因为它们需要整体替换 `Option<Arc<_>>`。

`Once` 保证 `attach_once` 的闭包至多成功执行一次，且为并发调用提供标准库的同步保证。注意 `new` 创建的初始目录不计入这一次机会：第一次重新挂载可替换它；分离产生的新扩展对象则拥有新的 `Once` 与空目录，所以未来仍可被第一次挂载填充。

查询返回克隆后的 `Arc<Table>`/`Arc<DbInfo>`，不会把锁守卫暴露给调用者。这样锁只覆盖映射访问，不覆盖调用者使用表对象的整个时间。文件不创建线程、异步任务、通道或事务，也不负责清理 `MemBuffer` 中的表数据；数据清理由 DDL/拦截器路径承担。

## 与 Go 版本的对应关系

直接 Go 对照文件是 [`infoschema.go`](infoschema.go)。四个顶层函数一一对应：Go 的 `getLocalTemporaryTables`/`ensureLocalTemporaryTables` 对应 Rust 的同名蛇形函数，`AttachLocalTemporaryTableInfoSchema`/`DetachLocalTemporaryTableInfoSchema` 对应 Rust 的 attach/detach 函数。两边都在会话没有目录时跳过挂载，已扩展时只允许一次后续设置，分离后仍保留扩展 InfoSchema 的外形。

重要差异是 Go 文件复用 `pkg/infoschema.SessionTables`、`infoschema.SessionExtendedInfoSchema`、`model.TableInfo` 和会话变量中的 `any` 类型断言；Rust 文件为降低当前 crate 的接线依赖，在本地重新定义了精简模型和 `InfoSchema` trait，并用静态类型的 `Mutex<Option<Arc<_>>>` 代替运行时断言。Go 的完整 `SessionTables` 逻辑实际位于 `pkg/infoschema/infoschema.go`，而 Rust 当前只移植了临时表 DDL 所需子集。

[`infoschema_test.rs`](infoschema_test.rs) 记录了四项明确兼容意图：Unicode 小写键、库 ID 不一致时 panic、删除库内最后一张表时移除库索引、首次重新挂载替换初始目录且之后冻结，以及分离后保持扩展层类型但不再暴露本地表。测试没有覆盖完整 Go InfoSchema API，也没有证明 Rust 的 Unicode 小写与所有 Go `CIStr`/排序规则完全等价。

## 扩展指南

新增目录能力时，应优先扩展 `SessionTables`，同时维护名称、ID、库三个索引的一致性，并在独立的 [`infoschema_test.rs`](infoschema_test.rs) 中补充测试；不要把测试内嵌回生产文件。若新增完整 InfoSchema 查询，应先确认该职责是否应接入 `pkg/infoschema` 的真实 Rust 类型，而不是继续扩大这里的局部 trait。

改变挂载行为时，需要同时检查 `SessionExtendedInfoSchema::{new, attach_once, table_by_id, has_temporary_table, detach_temporary_table_info_schema}` 和两个顶层 attach/detach 函数。尤其不能无意中让 `new` 消耗 `Once`，否则会破坏当前与 Go 结构体字面量一致的“首次重新挂载可替换”语义。

改变大小写规则时，应修改 `CiString` 并增加非 ASCII、大小写展开和目标 collation 的对照用例。改变 ID/名称唯一性或支持并发 DDL 时，应把跨索引原子性、锁顺序、重复 ID 竞争和读者中间态纳入设计与测试，避免只增加单个 `HashMap` 操作。

若将当前 `cfg(any())` 下的真实 AsterSQL 依赖启用，应以 `Cargo.toml` 和完整 `pkg/infoschema` API 为迁移边界，逐项核对 `TableInfo`、错误类型、自动 ID 分配器和会话变量所有权；不能仅删除局部类型而不处理调用者。此类行为改动还需同步 Go 对照语义和独立 Rust 测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录内 `infoschema.rs` 被识别为 430 行、66 个符号。
- RustCodeGraph 文件节点：完整读取 `pkg/table/temptable/infoschema.rs`，确认所有类型、trait、函数、impl 与锁字段；读取 `pkg/table/temptable/lib.rs`，确认模块声明、再导出和独立测试装配。
- RustCodeGraph 精确节点：`attach_local_temporary_table_info_schema` 调用 `get_local_temporary_tables`、`as_any`、`attach_once`，并由 `first_reattach_replaces_the_initial_local_tables_then_once_freezes_it` 调用；`detach_local_temporary_table_info_schema` 调用 `as_any`，并由 `detach_preserves_the_go_extended_info_schema_shape_without_local_tables` 调用；`ensure_local_temporary_tables` 调用 `SessionVarsProvider::session_variables`。
- RustCodeGraph 精确节点：`pkg/table/temptable/ddl.rs::SessionTemporaryTableDdl::create_local_temporary_table` 的源码在第 105 行起，直接执行 `ensure_local_temporary_tables(...).add_table(...)`，构成生产侧局部入口证据。
- 对照读取：`pkg/table/temptable/Cargo.toml`、`pkg/table/temptable/infoschema.go`、`pkg/table/temptable/infoschema_test.rs`。Cargo 证明 crate 边界与当前禁用依赖；Go 文件证明四个顶层函数的来源语义；Rust 独立测试证明关键不变量和挂载/分离行为。
- 结构检查要求：本文必须恰好包含本计划规定的十一个二级标题；本任务是纯文档分析，按计划不运行 Cargo 或代码测试。
