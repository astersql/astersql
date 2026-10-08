# `pkg/table/tables/index.rs`

## 文件定位

该文件属于 `astersql-table-tables` crate；模块入口 `pkg/table/tables/lib.rs` 以 `pub mod index` 对外公开它。它为 Rust 表实现提供一组自包含的二级索引元数据、索引键值编码、部分索引谓词和 DDL 临时索引状态辅助。当前最直接的运行时使用者是同 crate 的 `TableCommon`：`pkg/table/tables/tables.rs::TableCommon::new` 构造 `Index`，`writable_indices` 过滤可写索引，`build_index_entries` 与 `remove_index_entries` 判断部分索引条件并生成键。

这不是 Go `pkg/table/tables/index.go` 的完整等价移植。Go 文件实现 `table.Index` 的事务写入、删除、唯一键冲突检查、多值索引、全局索引和 tablecodec 编码等完整 KV 路径；本 Rust 文件只提供当前内存表路径所需的较小模型和辅助函数。文档下文将“Rust 当前事实”与“Go 对照目标”分开描述。

## 核心职责

- 定义索引生命周期元数据：`SchemaState`、`BackfillState`、`ColumnInfo`、`IndexColumn`、`IndexInfo` 和 `TableInfo`。
- 通过 `Index::new` 绑定物理表 ID、表元数据与索引元数据，校验列偏移，并在启用 `expression-runtime` 时编译部分索引条件。
- 通过 `Index::gen_index_key` 和 `Index::gen_index_value` 生成当前 Rust 模型的字节表示；唯一且所有索引值非 `NULL` 时，handle 放入 value，否则放入 key。
- 通过 `meet_partial_condition*`/`matches_partial_condition` 实现无条件索引恒匹配、SQL `NULL` 不匹配以及错误传播。
- 通过 `gen_temp_index_key_by_state`、`is_index_writable` 和 `dedup_index_columns` 提供 DDL 回填双写、写入资格与索引列去重规则。

## 主要符号

- 常量 `INDEX_ID_MASK` 与 `TEMP_INDEX_PREFIX`：前者保留低 48 位索引 ID，后者是当前 Rust 临时键前置的字节 `b't'`。`TEMP_INDEX_KEY_TYPE_{NONE,DELETE,BACKFILL,MERGE}` 分别为版本 `0..=3`。
- `SchemaState`：包含 `None`、`DeleteOnly`、`WriteOnly`、`WriteReorganization`、`DeleteReorganization`、`Public`。`is_index_writable` 只拒绝两个删除阶段。
- `BackfillState`：`Inapplicable`（默认）、`Running`、`ReadyToMerge`、`Merging`，驱动正式键、临时键以及版本号的组合。
- `ColumnInfo`：保存列 ID、名称、是否需要 restored data、MySQL 类型码和排序规则；类型码与排序规则还用于部分索引表达式编译。
- `IndexColumn`：以 `offset` 关联表列，以 `length: Option<usize>` 表示前缀索引；`dedup_index_columns` 也以 `offset` 为唯一身份。
- `IndexInfo`：保存索引 ID、名称、列、`unique`/`primary`、schema/backfill 状态及可选条件 SQL。
- `Index`：保存 `physical_id`、表/索引元数据、新排序规则开关和预计算的 `restored_data`；feature 开启时还保存编译表达式、表达式上下文以及共享行缓冲池。
- `IndexError`：目前只有非法列偏移 `ColumnOffset`、索引值数量错误 `ValueCount`、表达式编译/求值失败 `Evaluation`。
- 私有函数 `encode_datum`：按 `Null=0`、`Int=1 + i64 BE`、`Uint=2 + u64 BE`、`Bytes=3 + u32 BE 长度 + 内容` 写入缓冲区。

## 执行流程

1. `TableCommon::new` 为每个 `IndexInfo` 调用 `Index::new`。构造函数先确认所有 `IndexColumn::offset` 均落在 `TableInfo::columns` 内，再调用 `need_restored_data`。未启用 `expression-runtime` 时，非空条件立即返回 `IndexError::Evaluation`；启用时则从列类型/排序规则建立 expression 表模型，并用 `expression::ParseSimpleExpr` 编译条件。
2. 写入路径 `TableCommon::build_index_entries` 先从 `writable_indices` 取得非删除阶段索引，再以 `matches_partial_condition` 排除不满足条件的行，按索引列 offset 收集 `Datum`，最后调用 `gen_index_key`。若 `distinct` 键已有不同 handle，则上层返回 `TableError::DuplicateIndex`。
3. `gen_index_key` 先严格校验值数量。`unique && 无 NULL` 得到 `distinct=true`；键先写文本前缀 `t{physical_id}_i{index_id}`，再逐项调用 `encode_datum`。非 distinct 情况在键尾追加大端 i64 handle。
4. `gen_index_value` 依次写入 untouched 字节、仅 distinct 时写入 handle，并在 `restored_data=true` 时编码调用方传入的 restored values。当前 `TableCommon` 的内存索引映射只消费 key 和 handle，未调用此 value 生成器。
5. 删除路径 `TableCommon::remove_index_entries` 对所有非 `SchemaState::None` 的索引重复条件判断与键生成，然后从 `index_entries` 删除键。
6. DDL 辅助 `gen_temp_index_key_by_state` 对 `Public` 或 `Inapplicable` 只返回正式键；`Running` 只返回临时键（`DeleteOnly` 用 DELETE 版本，否则 BACKFILL）；`ReadyToMerge`/`Merging` 同时返回正式键和临时键并使用 MERGE 版本。

## 数据与状态

`Index` 在构造后拥有自己的 `TableInfo` 与 `IndexInfo`，而 `Clone` 会复制这些值。`restored_data` 在构造时一次计算，不像 Go `index.initNeedRestoreData sync.Once` 那样延迟初始化。计算不变量是：必须启用新排序规则，且至少一个索引列是前缀列或其表列标记了 `needs_restored_data`；越界 offset 已由 `Index::new` 先拒绝。

键的 distinct 不变量是“唯一索引且所有索引值非 NULL”。因此唯一索引中的 NULL 仍把 handle 编入键，允许多行 NULL；非唯一索引始终把 handle 编入键。distinct 键将 handle 放入 value，以便从唯一键恢复行身份。

部分索引条件中，`None` 与空字符串都表示无条件。回调式 `meet_partial_condition_with_collation` 把构造时的 `use_new_collation` 传给求值器；返回 `None`（SQL NULL）转换为 `false`。feature 路径把本地 `Datum` 转为 expression datum，并把列排序规则附到对应值上。

## 依赖与调用关系

- crate 边界由 `pkg/table/tables/Cargo.toml` 定义：默认 feature 是 `expression-runtime`，它启用可选的 `astersql-expression`、`astersql-expression-exprstatic` 和 `astersql-sessionctx-stmtctx`。文件本身还直接使用同 crate 的 `mutation_checker::Datum` 和标准库 `HashSet`。
- RustCodeGraph 的精确边确认 `Index::new → need_restored_data`、`gen_index_key → encode_datum`、`gen_index_value → encode_datum`，以及 `tables.rs::writable_indices → is_index_writable`。
- 精确源码检索补足方法分派边：`TableCommon::new → Index::new`；`build_index_entries → matches_partial_condition/gen_index_key`；`remove_index_entries → matches_partial_condition/gen_index_key`。
- `gen_index_value`、`meet_partial_condition`（回调版本）、`gen_temp_index_key_by_state` 和 `dedup_index_columns` 的直接证据主要来自独立测试；在非测试 Rust 代码中未找到直接调用。它们是公开 API，但不能据此推断已接入完整事务/KV 主链。
- RustCodeGraph 曾把 `pkg/server/handler/tikv_handler.rs` 中另一个不同签名的 `Index::gen_index_key` 解析为同名调用；源码签名核对后不将其列为本文件的调用者。

## 错误处理与边界

- `Index::new` 对任何列 offset 越界返回 `IndexError::ColumnOffset`；部分索引条件无法编译或 feature 不可用时返回 `Evaluation`。`TableCommon::new` 将前者映射为 `InvalidColumnOffset`，其他构造错误映射为 `IndexCondition`。
- `gen_index_key` 要求值数量与索引列数量完全相等，否则返回带 expected/actual 的 `ValueCount`；它不会自行截断前缀索引值，也不会执行 Go 的 changing type 转换。
- 回调求值函数的错误原样传播；SQL NULL 被视为不匹配。真实 expression 求值错误和 panic 都被转换为 `IndexError::Evaluation`，panic 文本会带 `panic in partial-index condition` 前缀。
- `matches_partial_condition` 假定“已编译表达式一定同时拥有上下文”，以 `expect` 维护该内部不变量；行池 mutex 中毒也会 panic，而不是返回业务错误。
- `gen_temp_index_key_by_state` 只基于本文件的简化状态模型处理键，不负责任务/事务原子性、临时 value 合并或冲突锁定。`INDEX_ID_MASK` 在本文件中定义但当前没有使用点。

## 并发与资源生命周期

除 feature 下的部分索引行池外，该文件不启动线程、异步任务、事务或通道。编译后的表达式与 `ExprContext` 随 `Index` 生命周期持有；`Arc` 让 clone 后的索引共享上下文和行池。

`matches_partial_condition` 从 `Arc<Mutex<Vec<MutRow>>>` 中优先复用列数相同的行缓冲；求值后归还。池最多保留 64 个缓冲，限制并发高峰后的常驻内存。锁只覆盖取出或归还，不覆盖表达式求值。求值包在 `catch_unwind` 中，即使 panic 也会先进入归还阶段，然后转成 `Evaluation`。Go 对照使用 `sync.Pool`，Rust 的固定上限与 poison 行为是本地实现差异。

## 与 Go 版本的对应关系

- `need_restored_data` 对应 Go `NeedRestoredData` 的目的，但 Go 通过 `types.NeedRestoredDataWithCollate(model.GetIdxChangingFieldType(...))` 判断真实字段类型；Rust 使用 `length.is_some()` 或显式布尔标记，属于较小元数据模型。
- `Index::new` 对应 `NewIndex/newIndex/initPartialCondition`；两者都会按索引自身的 collation 模式编译条件。Rust 构建 expression 所需的精简表模型，Go 直接使用完整 `model.TableInfo`，并为非聚簇表补额外 handle 缓冲列。
- `gen_index_key/gen_index_value` 对应 Go 同名方法的职责，但 Go 委托 `tablecodec`，覆盖真实 TiDB 编码、global/partition handle、changing field type、错误上下文和复用缓冲；Rust 的文本前缀与私有 tag 编码不能宣称与 TiDB 持久化格式兼容。
- `matches_partial_condition` 对应 Go `MeetPartialCondition/MeetPartialConditionWithChunk`：二者都让 NULL 为 false、捕获 panic并传播求值错误。Go 测试还覆盖 clustered index、PK handle、隐式 rowid 及真实 SQL DML；Rust 的 `index_test.rs` 覆盖回调语义，`pkg/testkit/go_merge_49_partial_index_test.rs` 额外验证真实 planner 编译、NULL 和大小写不敏感排序规则。
- `gen_temp_index_key_by_state` 对应 Go `GenTempIdxKeyByState` 的状态矩阵。Go 用 `tablecodec.IndexKey2TempIndexKey` 就地转换真实索引键；Rust 简单在原键前加 `TEMP_INDEX_PREFIX`。
- `is_index_writable` 与 Go `IsIndexWritable` 状态判断一致；`dedup_index_columns` 与 Go `DedupIndexColumns` 一样按 offset 去重并保留首次出现顺序。
- Go 文件中事务 `Create/Delete/Exist`、多值索引展开、临时索引 value 合并和重复 handle 查询等大段逻辑，在本 Rust 文件中不存在；扩展时应明确逐项移植，而不能把公开辅助函数视为这些能力已完成。

## 扩展指南

- 增加新 schema/backfill 状态时，应同步审查 `gen_temp_index_key_by_state`、`is_index_writable`、`TableCommon::{writable_indices,remove_index_entries}` 以及 `index_test.rs::temporary_index_key_tracks_backfill_state`，避免写入/删除阶段不对称。
- 扩展 `Datum` 或持久化编码时，修改点至少包括 `encode_datum`、`gen_index_key`、`gen_index_value` 与独立测试。若目标是 TiDB KV 兼容，应接入正式 tablecodec 语义，而不是继续扩展当前私有格式，并对照 Go 的 global index、common/partition handle、prefix truncation 与 changing type 路径。
- 扩展部分索引表达式时，应保持构造期编译、索引自有 collation、NULL=false、panic 转错误和缓冲归还这些不变量；同步 `pkg/table/tables/index_test.rs` 与跨 crate 的 `pkg/testkit/go_merge_49_partial_index_test.rs`。
- 新增 `IndexError` 变体时，应更新 `TableCommon::new/build_index_entries/remove_index_entries` 的映射，避免把真实求值错误误报成列偏移错误。
- 测试逻辑应继续放在独立的 `pkg/table/tables/index_test.rs`，不要内嵌回生产文件；需要验证完整 SQL/表达式编译时使用现有 `pkg/testkit/go_merge_49_partial_index_test.rs` 表面。
- 兼容性风险集中在键字节格式、handle 放置、collation 与临时索引状态；性能风险集中在每次键/value 分配、Datum 转换和 mutex 行池竞争。任何这些修改都应与 Go `pkg/table/tables/index.go` 的对应路径逐项核对。

## 验证依据

- 源码：`pkg/table/tables/index.rs`（全部 481 行），以及直接入口 `pkg/table/tables/lib.rs`、`pkg/table/tables/tables.rs`。
- crate：`pkg/table/tables/Cargo.toml`，确认 crate 名、默认 `expression-runtime` feature、可选 expression 依赖和 Go package 映射。
- RustCodeGraph：索引状态显示 11,467 个文件、307,296 个节点；查询/节点检查覆盖 `need_restored_data`、`Index::new`、`gen_index_key`、`gen_index_value`、`meet_partial_condition*`、`matches_partial_condition`、`gen_temp_index_key_by_state`、`is_index_writable`、`dedup_index_columns` 与 `encode_datum`。对图中同名误配使用文件和签名复核。
- Rust 测试：`pkg/table/tables/index_test.rs` 覆盖 offset 去重、可写状态、restored data、构造越界、空/缺 feature 条件、distinct/NULL/handle、值数量、value 布局、NULL/错误求值、collation 与临时键状态；`pkg/testkit/go_merge_49_partial_index_test.rs` 覆盖真实表达式编译与 collation。
- Go 对照：`pkg/table/tables/index.go`；Go 测试 `pkg/table/tables/index_test.go` 的 `TestMeetPartialCondition`、`TestPartialIndexDML*`、`TestDedupIndexColumns4Test` 及临时索引相关用例用于界定完整语义与 Rust 当前缺口。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前以任务给定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核所有“当前已接线”陈述均能追溯到上述源码或调用边。
