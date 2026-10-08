# `pkg/table/tables/tables.rs`

## 文件定位

本文件位于 `astersql-table-tables` crate，是 `pkg/table/tables/lib.rs` 中公开的 `tables` 模块。它把表层能力分成两类：一类是直接借用完整 catalog 模型 `model_dependency::TableInfo` 的构造前验证视图（`table_from_meta_for_validation`、`ValidatedTableMetadata`），另一类是基于本 crate 精简模型的可执行内存表、序列、扫描描述和元数据辅助函数（`TableCommon`、`SequenceCommon`、`TableScan` 等）。

Go 对照文件是 `pkg/table/tables/tables.go`，其中 `TableCommon` 实现真实事务、KV 编码、断言、统计和 DDL 过渡期写入。Rust 的 `TableCommon` 只在进程内的 `BTreeMap`/`HashMap` 中保存行与索引，不应被理解为 Go 持久表实现的透明替代。当前仓库中完整模型入口的生产调用者是 `pkg/ddl/create_table.rs:43`；RustCodeGraph 还确认本文件被 15 个文件引用，直接行为覆盖主要集中在同目录独立测试 `pkg/table/tables/tables_test.rs`。

crate 边界由 `pkg/table/tables/Cargo.toml` 定义：默认启用 `expression-runtime`；完整元数据验证依赖 `astersql-meta-model`、`astersql-util-generatedexpr` 和 `astersql-table`，表达式条件与分区表达式在该 feature 下额外依赖 `astersql-expression`、`exprstatic` 与 `stmtctx`。

## 核心职责

1. `table_from_meta_for_validation` 按 Go `TableFromMetaWithCollate` 的关键顺序验证完整表、列、索引状态，解析生成列和表达式默认值，加载 CHECK 约束，并在启用表达式运行时时构造正常分区、重组分区和部分索引条件表达式。它保留对原始 `TableInfo` 的借用，避免转换到精简模型时丢失 catalog 字段。
2. `TableCommon` 保存表/物理表标识、列、索引、约束、内存行、唯一索引键、handle 分配进度和可选序列；提供 DDL `SchemaState` 相关筛选、键前缀、行增删改查及扫描迭代。
3. `SequenceCommon` 通过 `SequenceAllocator` 管理本地序列号段，支持递增、递减、循环轮次、缓存耗尽续租以及 SETVAL/rebase。
4. 独立辅助函数处理主键/公共句柄元数据、分片位溢出、可省略列、恢复数据截断、二进制排序规则尾空格、下推扫描描述与默认值编码。
5. `TemporaryTable` 以原子变量保存“已修改”标志和估算大小，供并发读取轻量状态。

## 主要符号

- `MetadataTableKind::{Common, Cached}`：只表示完整元数据构造会选择普通表还是缓存表分支；它不创建对应运行时对象。
- `ValidatedTableMetadata<'a>`：借用完整 `model_dependency::TableInfo`，同时保存生成列表达式 AST、默认表达式 AST、修复/加载后的 CHECK 约束，以及 feature 控制的分区、重组和索引条件表达式。`public_columns` 与 `writable_columns` 分别实现 Public 过滤和排除 DeleteOnly/DeleteReorganization。
- `table_from_meta_for_validation(&mut TableInfo)`：完整 catalog 模型的验证入口。传入可变引用是因为 `LoadCheckConstraint` 可能修复 `meta.Constraints`；返回值继续借用该对象。
- `Column`、`Constraint`：精简执行模型中的列和约束描述。`Column::is_virtual_generated` 判定“生成但非 stored”的列。
- `TableError`：统一表达非法偏移、表达式索引错误、行宽错误、记录不存在/重复、唯一索引冲突、非法索引状态与序列错误。
- `SequenceInfo`、`SequenceAllocator`、`SequenceCommon`：序列配置、持久分配边界抽象和本地缓存状态。`next_value`、`set_value` 是主要入口，`seek_sequence_value` 执行带边界的等差序列查找。
- `TableCommon`：核心内存表。公开 API 包括 `new`/`copy`、元数据与列索引筛选、`alloc_handle_ids`、`add_record`、`update_record`、`remove_record`、`row_with_columns`、`iter_records` 以及序列绑定/调用。
- `find_primary_index`、`try_get_common_pk_column_ids`、`primary_prefix_column_ids`、`find_index_by_column_name`：索引元数据查询。
- `overflow_shard_bits`：检查行 ID 是否侵入预留 shard 位。
- `can_skip_with_collation`（及默认新排序规则的 `can_skip`）：判断某列写行时能否省略。
- `try_truncate_restored_data`、`convert_datum_to_tail_space_count`：公共句柄恢复数据处理。
- `PbColumnInfo`、`TableScan`、`PartitionTableScan`、`build_table_scan`、`build_partition_table_scan`、`set_pb_columns_default_value`：精简 protobuf 风格的下推描述和默认值填充。
- `TemporaryTable`：使用 `AtomicBool`/`AtomicI64` 管理临时表状态。

## 执行流程

完整元数据验证流程如下：

1. `table_from_meta_for_validation` 先拒绝 `TableInfo.State == None`。
2. 它按 catalog 中的列顺序遍历：拒绝列状态 None；偏移不等于实际位置时只输出诊断而不拒绝，这一点刻意对齐 Go；生成列表达式先解析再对完整表解析列名；表达式默认值只解析语法，不做列名解析。
3. 调用 `LoadCheckConstraint`。该步骤可能原地删除或修复无效 CHECK 元数据，且发生在分区和索引验证之前。
4. 若存在启用的分区信息，空 definitions 返回 Unknown partition；启用 `expression-runtime` 时构造常规分区表达式，并针对 REORGANIZE/REMOVE/ALTER PARTITIONING 按 DDL 状态选择 dropping 或 adding definitions 构造重组表达式。未启用 feature 时分区元数据直接报运行时缺失。
5. 最后检查索引状态；部分索引条件仅在表达式运行时解析。返回视图时根据 `TableCacheStatusType` 标记 Common/Cached。

内存行写入流程如下：

1. `TableCommon::new` 验证精简列偏移，拒绝 StateNone 索引，并通过 `Index::new` 固化物理表 ID、元数据和排序规则；行、索引映射与 handle 计数从空状态开始。
2. `add_record` 先校验行宽；无显式 handle 时调用 `alloc_handle_ids(1)`，然后拒绝重复记录、为所有可写且满足部分索引条件的索引生成键并检查唯一性，最后才提交行与索引项。显式 handle 会推进 `next_handle` 的上界。
3. `update_record` 校验旧行、新行及 `touched` 长度并确认记录存在；删除旧索引后尝试建立新索引。新索引失败时重建旧索引，因此唯一性冲突不会丢掉原索引；成功后替换行。当前实现只校验 `touched` 的长度，索引重建并未据其跳过未触碰索引。
4. `remove_record` 先确认记录存在，再删除全部可删索引键，最后删除行。`row_with_columns` 做按偏移投影，`iter_records` 继承 `BTreeMap` 的 handle 升序。

序列流程中，`next_value` 先在当前 `[base,end]` 缓存内用 `seek_sequence_value` 找严格越过 base 的值；失败时调用 `alloc_cache` 更新 base/end/round 后重试。循环序列在 round 大于零时改用 min/max 作为 offset。`set_value` 对未越过 base 的请求返回 `(0,true)`，缓存范围内直接移动 base，范围外先使缓存失效再调用 allocator `rebase`。

## 数据与状态

`TableCommon.table_id` 是逻辑表 ID，`physical_table_id` 决定记录/索引键前缀，分区实例可以因此与逻辑 ID 不同。`rows: BTreeMap<i64, Vec<Datum>>` 以整数 handle 保存完整精简行，保证迭代有序；`index_entries: HashMap<Vec<u8>, i64>` 把编码索引键映射到拥有者 handle，用于唯一性检测。该映射只保存一个 owner，非唯一索引的区分依赖 `Index::gen_index_key` 将 handle 编进键中。

列、索引和约束筛选体现在线 DDL 状态：可写列排除 DeleteOnly/DeleteReorganization；可写约束还要求 `enforced`；可写索引委托 `is_index_writable`；可删列/索引覆盖构造成功的全部对象。visible/hidden 只返回 Public 状态，并按 `hidden` 分组。

`Clone`/`copy` 深拷贝元数据、行、索引映射和 handle 计数，因此副本后续写入互不影响；`sequence` 是 `Arc<Mutex<SequenceCommon>>`，副本共享同一序列缓存与 allocator。这个差异是复制语义的重要不变量。

`ValidatedTableMetadata` 不复制完整 catalog，而是借用它；其中的表达式映射以列位置或索引 ID 为键。CHECK 加载可能改变被借用对象，调用者必须在返回视图释放前遵守 Rust 借用约束。

`TemporaryTable` 的 `modified` 与 `size` 分别用原子布尔和整数存储；元数据在构造后不变。`PbColumnInfo.default_value` 使用本文件私有 `encode_default` 的简化类型标签编码：Null=0、Int=1、Uint=2、Bytes=3，这不是 Go `tablecodec.EncodeValue` 的通用 KV 编码。

## 依赖与调用关系

上游直接证据：

- `pkg/ddl/create_table.rs:43` 在 DDL 建表路径调用 `table_from_meta_for_validation`，使完整模型的状态、表达式、约束和分区验证进入应用主链。
- `pkg/session/runtime/normal_ddl_create_table_test.rs:998,1021` 从会话/DDL 测试再次调用该验证入口，核对完整元数据生成结果。
- `pkg/table/tables/partition_expr_test.rs`、`bench_test.rs` 和 `tables_test.rs` 构造并操作 `TableCommon`；仓库搜索未发现本文件精简 `add_record`/`update_record`/`remove_record` 被 Rust 生产执行器直接调用。执行器中的同名方法属于各自 runtime trait，不能据名称视为本对象的调用边。

下游依赖：

- `crate::index::{Index, IndexInfo, TableInfo, is_index_writable}` 提供精简索引对象、键生成、部分索引条件与 DDL 可写判定。
- `crate::mutation_checker::Datum` 是精简行值类型。
- `generatedexpr::{ParseExpression, SimpleResolveName}` 解析完整元数据中的生成列/默认表达式。
- `table_dependency::constraint::LoadCheckConstraint` 加载并可能修复 CHECK 定义。
- `crate::canonical_partition_expr::build` 与 `expression::ParseSimpleExprWithTableInfo` 只在 `expression-runtime` 下参与分区/部分索引表达式构造。
- `crate::canonical_partition::CanonicalPartitionedTable` 由 `canonical_partition_router` 绑定当前表的排序规则模式。
- 标准库 `BTreeMap`、`HashMap`、`Arc<Mutex<_>>` 与原子类型分别承担有序行、索引映射、共享序列和临时表并发状态。

## 错误处理与边界

`table_from_meta_for_validation` 返回字符串错误，并保留 Go 错误码前缀：表/列/索引 StateNone 分别为 8042/8046/8044，空分区 definitions 为 1735。列 Offset 错位只是打印诊断；这是为了对齐 Go 构造器，而 `TableCommon::new` 的精简模型则将越界列偏移作为 `InvalidColumnOffset` 拒绝。两条入口的容错契约不可混用。

表达式错误按执行顺序立即传播：生成列名解析失败早于默认表达式、CHECK、分区和索引验证；CHECK 修复又早于索引状态错误。没有 `expression-runtime` 时，只要元数据确实需要分区或索引条件表达式，就返回明确的 runtime-required 错误。

内存表严格要求行宽等于列数，且 `update_record` 要求 `touched.len()` 相同。重复 handle 返回 `RecordExists`，缺失记录返回 `RecordNotFound`，投影/索引取值越界返回 `InvalidColumnOffset`。handle 连续分配和类型转换使用 checked 运算，溢出映射为 `SequenceRunOut`。

唯一索引冲突在写行前检查；更新失败会尽力恢复旧索引。恢复分支忽略重建旧索引自身的错误，因此若内部索引逻辑未来引入新的可失败条件，需要重新审视原子性保证。删除索引键成功而后续行删除不会失败于当前内存实现，但它并非通用事务回滚机制。

序列 increment 为零或候选超出 end 时 `seek_sequence_value` 返回 None；新缓存仍无可用值时返回 `SequenceRunOut`。未绑定序列返回 `SequenceMissing`。序列互斥锁中毒使用 `expect`，会 panic，而不是转换为 `TableError`。

## 并发与资源生命周期

`TableCommon` 的行、索引和 handle 修改要求 `&mut self`，本文件没有为整张内存表提供内部同步；调用者若跨线程共享，必须在外层加锁。它也没有事务、磁盘/KV 生命周期、异步任务或通道。

序列是例外：`set_sequence` 将 `SequenceCommon` 包在 `Arc<Mutex<_>>` 中，`sequence_next_value`/`set_sequence_value` 在一次查找、缓存续租或 rebase 的完整操作期间持有互斥锁。`SequenceAllocator: Send` 允许 allocator 随序列跨线程转移，但没有要求 `Sync`，访问始终串行。`TableCommon` 的 clone 共享此 Arc，保证副本不会各自重复消费同一缓存。

`TemporaryTable` 用 Release 写、Acquire 读发布 `modified` 和 `size`，两个字段各自原子，但跨字段读取不是一致快照。`ValidatedTableMetadata` 只有借用数据与拥有的解析结果，不启动后台资源；其生命周期受原始 `TableInfo` 的可变借用约束。

## 与 Go 版本的对应关系

完整元数据入口对齐 `tables.go` 的 `TableFromMetaWithCollate`：状态检查、偏移只记录日志、生成列表达式解析并解析列名、表达式默认值仅解析、CHECK 加载、分区构造和索引初始化的先后关系由 `tables_test.rs` 的 `normal_ddl_plan_*` 系列测试固定。Rust 额外返回 `ValidatedTableMetadata` 供调用方消费验证结果，不直接构造 Go `table.Table` 接口对象。

精简 `TableCommon` 与 Go 同名类型共享列/索引 DDL 状态筛选、逻辑/物理表 ID、排序规则固定、记录与索引共同维护、唯一冲突、序列缓存、扫描元数据等概念。但重要差异包括：

- Go 读写 `kv.Transaction` 的 mem-buffer staging，并维护断言、mutation checker、临时表事务大小与统计；Rust 只更新内存容器。
- Go 支持 `kv.Handle`（整数/公共句柄）、autoid allocators、列变更的依赖列、写入选项和不同重复键检查模式；Rust 只用 `i64` handle 和精简选项。
- Go `UpdateRecord` 按 `touched` 和 AffectColumn 决定索引重建；Rust 当前重建所有相关索引。
- Go 的列默认值使用表达式上下文和 `tablecodec.EncodeValue`；Rust `encode_default` 只覆盖四种 `Datum` 变体。
- Go `Copy` 是浅复制并保留指针字段；Rust clone 深拷贝行/索引状态，但共享序列 Arc。
- `try_get_common_pk_column_ids` 的 Go 入口先要求 `IsCommonHandle`；Rust 精简 `TableInfo` 没有该标志，因此只要提供 primary index 就返回列 ID。`primary_prefix_column_ids` 对前缀有效性的判断也比 Go 简化。

因此，扩展时应先确定目标是完整 catalog 验证、精简内存行为，还是 Go 持久表的真正移植；不得用精简实现的通过测试推断 Go 全部事务语义已经移植。

## 扩展指南

- 新增构造期 catalog 校验时，优先修改 `table_from_meta_for_validation`，保持与 Go `TableFromMetaWithCollate` 的错误顺序；同步扩展 `pkg/table/tables/tables_test.rs` 的 `normal_ddl_plan_*` 测试，并在真实 DDL 接线变化时检查 `pkg/ddl/create_table.rs` 与 `pkg/session/runtime/normal_ddl_create_table_test.rs`。
- 新增列/索引 SchemaState 行为时，集中修改 `ValidatedTableMetadata` 或 `TableCommon` 的筛选方法，避免调用者复制过滤规则；同步覆盖 public/visible/hidden/writable/deletable 和约束 enforcement 组合。
- 扩展 DML 时同时审视 `build_index_entries` 与 `remove_index_entries`，保证部分索引条件、唯一性和失败恢复对称。回归测试应继续放在独立 `tables_test.rs`，至少包含“修复前失败、修复后通过”的冲突与回滚场景，不把测试嵌入生产文件。
- 若要缩小与 Go 的差距，应按对应 Go commit 的增量逐项移植事务或编码语义，不应把 `BTreeMap`/`HashMap` 模型直接接到持久 SQL 执行链。引入通用 KV 编码前尤其要替换并验证私有 `encode_default`。
- 扩展序列时保持正/负 increment 的镜像分支、循环 round offset 和“allocator 成功后才更新缓存”的不变量；为缓存耗尽、递减、循环、SETVAL 范围内/范围外和 allocator 错误各加独立测试。
- 修改共享状态时说明同步粒度：行状态当前依赖外部 `&mut`/锁，序列依赖 Mutex，临时表原子字段不构成一致快照。性能上需关注每次 `writable_indices()` 分配 Vec、整行/键 clone、更新时全索引重建，以及 `copy` 对全部行的深复制。
- 若新增依赖、feature 或模块接线，同步 `pkg/table/tables/Cargo.toml`/`lib.rs`；本文件测试仍保持在 `pkg/table/tables/tables_test.rs`。Rust 源改动完成后按仓库要求运行 `cargo fmt --all`，但本次纯文档分析不修改或运行 Rust。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/table/tables` 找到目标 Rust/Go/测试文件；`node --file pkg/table/tables/tables.rs` 阅读了 1–1094 全部行并报告该文件被 15 个文件使用；`query table_from_meta_for_validation`、`query TableCommon --kind struct --json`、`query SequenceCommon --kind struct --json` 定位了 Rust/Go 对照符号。图对本文件 impl 方法的同名查询覆盖不完整，因此调用证据按技能规则由仓库搜索补齐。
- 已读生产与配置：`pkg/table/tables/tables.rs`、`pkg/table/tables/Cargo.toml`、`pkg/table/tables/lib.rs`、`pkg/table/tables/tables.go`、`pkg/ddl/create_table.rs` 的直接调用位置。
- 已读独立测试：`pkg/table/tables/tables_test.rs`，覆盖构造状态与错误顺序、部分索引、行/索引原子性、行宽与偏移、SchemaState、序列、公共句柄辅助、恢复数据、扫描/default 与临时表原子状态；另以仓库搜索核对 `pkg/table/tables/tables_test.go` 的 Go 行增删改、TableFromMeta 和排序规则测试入口。
- 关键事实由 `tables.rs` 的 `table_from_meta_for_validation`、`TableCommon::{new,add_record,update_record,remove_record}`、`SequenceCommon::{next_value,set_value}`、`set_pb_columns_default_value` 和 `TemporaryTable` 实现直接支持，并与上述测试断言交叉核验。
- 本任务是纯文档分析，未修改 Rust/Go/Cargo，也未运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级章节。
