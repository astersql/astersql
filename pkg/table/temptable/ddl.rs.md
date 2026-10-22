# `pkg/table/temptable/ddl.rs`

## 文件定位

负责在单个会话内部创建、删除和截断本地临时表，维护两类会话状态：`infoschema.rs` 中的 `SessionTables` 元数据目录，以及 `interceptor.rs` 中的 `MemBuffer` 表数据。

## 核心职责

- 通过 `TemporaryTableDdl` 统一暴露创建、删除和截断三个操作。
- 通过 `Store::generate_global_id` 为本地临时表分配真实全局表 ID，避免编码后的表键前缀与持久表冲突；随后将元数据状态设为 `SchemaState::Public`。
- 首次创建临时表时，通过 `Store::begin(0)` 获得会话 `MemBuffer`，并惰性创建 `SessionTables`。
- 删除或截断表时，先按库名和表名校验目标，再维护会话元数据索引，并清除旧 table ID 对应的 `[encode_table_prefix(id), encode_table_prefix(id + 1))` 键区间。
- 保持 Go `pkg/table/temptable/ddl.go` 的关键执行次序，包括重复创建仍消耗全局 ID、截断先删旧目录项再添加新表、数据清理失败不回滚此前元数据变化。

## 主要符号

- `Store: Send + Sync`：本文件所需的最小存储边界。`begin(start_ts)` 返回共享 `MemBuffer`；`generate_global_id()` 分配表 ID。实现者必须保证 ID 不与真实表冲突。
- `SessionContext: SessionVarsProvider`：把 `infoschema.rs` 提供的会话变量与 `Store` 组合起来；`store()` 返回 `Arc<dyn Store>`。
- `TemporaryTableDdl: Send + Sync`：公开 DDL 接口。三个方法分别接收可变 `TableInfo` 或大小写不敏感的 `CiString` 表标识，并统一返回 `Result<(), TempTableError>`。
- `SessionTemporaryTableDdl`：接口的具体实现，仅持有 `Arc<dyn SessionContext>`；`new` 构造实例，`get_temporary_table_ddl` 将其擦除为 `Arc<dyn TemporaryTableDdl>`。
- `create_local_temporary_table`：确保数据缓冲存在，写入 `db_id`，分配新 ID、置为 Public，并调用 `SessionTables::add_table` 注册表。
- `drop_local_temporary_table`：校验目标存在，移除按名和按 ID 的目录项，再调用 `clear_temporary_table_records` 写入删除标记。
- `truncate_local_temporary_table`：克隆旧元数据，分配新 ID，查回所属库，移除旧表并添加替代表，最后清理旧 ID 的数据。
- `get_session_data` / `ensure_session_data`：读取或惰性初始化 `SessionVariables::temporary_table_data`。
- `new_temporary_table_from_table_info`：修改传入 `TableInfo.id` 和 `state`，再克隆元数据构造 `Arc<Table>`。
- `check_local_temporary_exists_and_return`：按大小写不敏感的 `(schema, table)` 查表；目录或目标不存在均返回 `TempTableError::TableNotExists`。
- `clear_temporary_table_records`：按旧 table ID 扫描键，关闭迭代器后逐键调用 `MemBuffer::delete_table_key`。

## 执行流程

创建流程从 `create_local_temporary_table` 开始：先由 `ensure_session_data` 锁住会话变量中的 `temporary_table_data`；若为空，以 `start_ts = 0` 调用 `Store::begin` 并保存返回的 `Arc<MemBuffer>`。随后把数据库 ID 写入调用者传入的 `TableInfo`，由 `new_temporary_table_from_table_info` 分配全局 ID、置为 Public 并构造 `Table`，最后通过 `ensure_local_temporary_tables(...).add_table(...)` 注册。ID 分配发生在重名检查之前，所以 `add_table` 返回 `TableAlreadyExists` 时该 ID 已被消耗；Rust/Go 测试都验证下一次成功创建会跳过这个 ID。

删除流程先调用 `check_local_temporary_exists_and_return`。校验失败不会初始化目录或数据缓冲，也不会改变现有表。成功后 `SessionTables::remove_table` 同步移除名字索引、ID 索引，并在该库无其他表时移除库记录；之后按被删表的旧 ID 清理缓冲数据。若清理失败，目录删除不会自动回滚。

截断流程同样先校验存在性；然后克隆旧 `TableInfo`，给克隆体分配新 ID 并设为 Public。它通过旧元数据的 `db_id` 在 `SessionTables::schema_by_id` 找回库，找不到时返回 `SchemaNotExists` 且旧表尚未移除。找到库后先移除旧表，再添加新表，最后清除旧 ID 的键。添加替代表失败时旧目录项已经丢失；清理失败时新目录项已经生效。这一非事务性次序由源码注释明确说明与 Go 一致。

清理流程计算半开区间 `[prefix(id), prefix(id + 1))`，用 `Retriever::iter` 顺序遍历。它先把属于目标前缀的键复制到容量初值为 16 的向量，结束后显式 `close()` 迭代器，再逐键删除。两阶段处理避免在迭代借用仍活跃时改变底层缓冲。`delete_table_key` 不物理移除条目，而是写入空 `ValueEntry`，模拟 TiKV mem-buffer 的删除占位语义。

## 数据与状态

核心持久期是会话而非全局 schema。`SessionVariables` 用两个 `Mutex<Option<Arc<_>>>` 分别保存 `SessionTables` 与 `MemBuffer`；首次成功走创建初始化它们，单纯删除/截断不存在的表不会创建任何状态。

`TableInfo` 在本路径上至少使用 `id`、`db_id`、`name` 和 `state`。创建会原地修改调用者的 `TableInfo`；截断只修改其克隆体，因此外部旧元数据仍保留旧 ID。`SessionTables` 用 `RwLock<HashMap<...>>` 同时维护按规范化名称、按 ID 和按库名的索引，`CiString` 保留原文但以 Unicode 小写值比较。

每张表的数据以 `encode_table_prefix(table_id)` 隔离。真实全局 ID 是隔离不变量：如果复用持久表或其他临时表的 ID，前缀扫描和删除可能误伤其他表。`ddl_test.rs::test_truncate_local_temporary_table` 证明截断只清空旧表 ID 的键，同库另一张临时表的数据保持不变。

## 依赖与调用关系

本文件的直接下游都在同一 crate：

- `infoschema.rs` 提供 `CiString`、`DbInfo`、`TableInfo`、`Table`、`SchemaState`、`TempTableError`、`SessionVarsProvider` 以及会话目录的 get/ensure 辅助函数。
- `interceptor.rs` 提供 `MemBuffer`、`Retriever`、迭代器协议、`encode_table_prefix` 和带 table ID 校验的 `delete_table_key`。
- 标准库的 `Arc` 承担上下文、存储、表、数据库和缓冲区的共享所有权。

RustCodeGraph 的精确调用边包括：trait 的三个公开方法分别落到 `SessionTemporaryTableDdl` 的实现；创建实现调用 `ensure_session_data` 和 `new_temporary_table_from_table_info`；删除实现调用存在性检查和 `clear_temporary_table_records`；截断实现还调用新表构造；清理函数调用 `get_session_data` 与 `MemBuffer::delete_table_key`。

上游方面，`get_temporary_table_ddl` 只在独立 Rust 测试的 `create_test_suite` 中出现。`pkg/executor/ddl.rs` 的 `DDLExec` 会在 SQL 层通过其泛型 `DdlRuntime` 调用 `create_local_temporary_table`、`drop_local_temporary_table`、`truncate_local_temporary_table`，但代码搜索与调用图没有找到该 runtime 接口到本文件工厂或具体类型的适配实现；该关系目前只能视为待接线边界，不是已证实的生产调用边。

## 错误处理与边界

所有可恢复失败统一使用 `TempTableError`。存储初始化或 ID 分配错误直接向上传播；目录重复名称或重复 ID 返回 `TableAlreadyExists`；缺少目录、库名不匹配或表名不匹配返回带 `schema.table` 文本的 `TableNotExists`；截断时旧表引用的库 ID 无法解析则返回 `SchemaNotExists`；扫描、推进迭代器和删除键的错误也直接传播。

关键部分失败不是原子回滚：创建在目录注册失败前已经初始化缓冲、修改传入元数据并消耗 ID；删除在数据清理失败前已经移除目录项；截断在 `add_table` 失败前已经移除旧表，在清理失败前已经换成新 ID。扩展调用方不能假定 `Err` 意味着状态完全未变。

`get_local_temporary_tables(...).expect(...)` 依赖刚完成的存在性检查保证目录仍存在；当前会话变量不会把已创建目录重新设为 `None`，因此该断言在现有生命周期下成立。`Mutex`/`RwLock` 的 `.unwrap()` 意味着锁中毒会 panic，而非转换为 `TempTableError`。`new_temporary_table_from_table_info` 对 `table_id + 1` 的区间端点没有显式溢出保护，依赖 ID 分配器不给出 `i64::MAX`。清理扫描还以表键编码保持前缀连续且有序为前提。

## 并发与资源生命周期

公开边界要求 `Store`、`SessionContext` 和 `TemporaryTableDdl` 都是 `Send + Sync`，实例和会话资源通过 `Arc` 共享。会话变量的两个可选对象各由 `Mutex` 保护惰性初始化；`SessionTables` 内部的名称、ID、库索引和 `MemBuffer` 的有序键集合则由 `RwLock` 保护。

`ensure_session_data` 在持有 `temporary_table_data` 互斥锁时调用 `store().begin(0)`，因此同一会话的并发首次初始化只会保存一个缓冲，但慢或重入的 Store 实现会延长持锁时间。`SessionTables::add_table`、`remove_table` 通过多个独立 `RwLock` 更新复合索引，不提供覆盖整个操作的事务锁；本文件依赖会话 DDL 通常串行执行，不能把这些步骤解释为跨线程线性化事务。

清理函数显式关闭迭代器后才写删除标记，避免迭代与变更同一缓冲重叠。`Store::begin(0)` 返回的事务对象在 Rust 抽象中已缩减为 `Arc<MemBuffer>`，本文件没有 commit/rollback 或显式关闭事务；缓冲随 `SessionVariables` 及其 `Arc` 引用释放。表目录和表对象也随会话共享引用生命周期释放。

## 与 Go 版本的对应关系

实现层面存在有意抽象差异。Go 的 `ensureSessionData` 从真实事务的 MemBuffer 构造 `TemporaryTableData`，Rust 的 `Store::begin(0)` 直接返回本地 `MemBuffer`；Go 的全局 ID 通过内部事务、`meta.Mutator.GenGlobalID` 获取，Rust 将其折叠为 `Store::generate_global_id`；Go 用 `tables.TableFromMeta` 并构造临时表 AutoID allocator，Rust 的 `Table::from_metadata` 仅保留 `has_auto_id` 布尔状态。Go 的错误带 PingCAP error stack 与标准 infoschema 错误身份，Rust 使用本地枚举值。

另外，Rust 截断在 `schema_by_id` 失败时显式返回 `SchemaNotExists`，而 Go 忽略 `SchemaByID` 的布尔结果并继续把库值传给 `AddTable`；这是 Rust 的防御性边界差异。Rust 清理还显式 `iterator.close()`，Go 源码没有显式关闭该迭代器。当前 Cargo 的跨 crate 依赖被 `cfg(any())` 禁用，也说明这份 Rust 文件是可执行的局部模型，而不是对 Go 生产依赖图的一比一链接。

## 扩展指南

新增 DDL 操作时，应先在 `TemporaryTableDdl` 增加契约，再在 `SessionTemporaryTableDdl` 实现，并把测试放在独立的 `pkg/table/temptable/ddl_test.rs`，不要内嵌进生产文件。若操作影响 SQL 主链，还需补齐 `pkg/executor/ddl.rs` 的 `DdlRuntime` 适配并验证实际 runtime 接线，不能仅增加同名方法。

修改创建流程时要保留真实全局 ID、`db_id` 与 Public 状态的不变量，并明确 ID 分配失败、目录注册失败后调用者可观察的 `TableInfo` 状态。若要改变重复创建消耗 ID 的行为，必须同步评估 Go 兼容性并更新 Rust/Go 对照测试。

修改删除或截断时，应特别审查元数据变更与数据清理的先后顺序及失败语义。若引入回滚，必须同时覆盖 `add_table`、迭代推进和 `delete_table_key` 失败点；当前 API 不提供事务性目录更新，不能只调整单个调用就宣称原子化。扫描算法必须继续限制在单一 table ID 前缀，并先结束迭代器生命周期再修改缓冲。

若接入真实 AsterSQL 组件，应优先实现 `Store`/`SessionContext` 到现有事务、会话变量和全局 ID 服务的适配，而不是在本文件复制外部子系统。还应恢复为真实 `TableInfo`/`Table`/错误类型并验证 AutoID allocator 语义；同时检查 `Cargo.toml` 中 `cfg(any())` 依赖的启用策略。性能方面，大表删除当前需要收集全部键，峰值内存为目标表键总大小，若优化为批处理必须保持迭代安全和删除占位语义。
