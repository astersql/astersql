# `pkg/session/ddl_tables.rs`

## 文件定位

本文件属于 `astersql-session` crate（见 `pkg/session/Cargo.toml` 的 `[package]` 与 `[lib] path = "lib.rs"`），负责描述并初始化 DDL 子系统依赖的 `mysql` 系统表。`pkg/session/lib.rs` 通过 `pub mod ddl_tables` 装配该模块，并在 crate 根重导出 `InitDDLTables`、四组表常量和 `TableBasicInfo`。

当前 Rust 生产代码中没有找到 `InitDDLTables` 的调用点；直接调用只出现在 `pkg/session/test/meta/session_test.rs`。因此，本文件目前提供了公开且经过独立测试的初始化能力，但不能据此断言它已经接入 Rust 的生产 bootstrap 主链。Go 对应入口位于 `pkg/session/session.go:InitDDLTables`，并由 Go bootstrap 流程调用。

## 核心职责

- 用 `DDLJobTables`、`MDLTables`、`BackfillTables`、`DDLNotifierTables` 将七张系统表按四个 `DDLTableVersion` 阶段分组。
- `InitDDLTables` 读取持久化的 DDL 表版本，仅创建比该版本新的组，并在所有所需组创建成功后推进版本。
- 私有函数 `create_tables` 解析每张表的建表 SQL、确认 AST 是 `CreateTableStmt`、调用 `BuildTableInfoFromAST` 验证元数据可构建并核对表名，然后通过 `Mutator` 写入表记录。

需要特别注意：当前 `create_tables` 丢弃了 `BuildTableInfoFromAST` 返回的 `built`（除名称校验外），实际传给 `create_table_or_view` 的是只设置 `id` 和 `name` 的默认 `TableInfo`。因此它尚未完整保存建表 SQL 所描述的列、索引、状态等模式信息；这是当前实现事实，不应把它描述成与 Go 完全等价。

## 主要符号

- `TableBasicInfo`：从 `crate::bootstrap` 公开重导出的轻量描述，字段为保留表 ID、静态表名和静态建表 SQL；原定义见 `pkg/session/bootstrap.rs:TableBasicInfo`。
- `DDLJobTables: [TableBasicInfo; 3]`：`tidb_ddl_job`、`tidb_ddl_reorg`、`tidb_ddl_history`，对应 `DDLTableVersion::Base`。
- `MDLTables: [TableBasicInfo; 1]`：`tidb_mdl_info`，对应 `DDLTableVersion::Mdl`。
- `BackfillTables: [TableBasicInfo; 2]`：`tidb_background_subtask` 与历史表，对应 `DDLTableVersion::Backfill`。
- `DDLNotifierTables: [TableBasicInfo; 1]`：`tidb_ddl_notifier`，对应 `DDLTableVersion::DdlNotifier`。
- `create_tables(&mut Mutator, i64, &[TableBasicInfo]) -> Result<(), String>`：单组表的顺序解析、验证与元数据写入函数，模块私有。
- `InitDDLTables(&mut Mutator) -> Result<(), String>`：公开入口；保留 Go 风格名称是因为模块启用了 `non_snake_case`/`non_upper_case_globals` 允许项。

所有表 ID 与 SQL 均来自 `astersql-meta-metadef`，本文件不自行分配 ID，也不维护 SQL 文本副本。

## 执行流程

1. `InitDDLTables` 调用 `Mutator::get_ddl_table_version` 读取当前整数版本。
2. 调用 `create_mysql_database_if_not_exists` 获取或建立 `mysql` 系统库并取得库 ID；即使当前版本已经最新，这一步仍会执行。
3. 构造按 `Base -> Mdl -> Backfill -> DdlNotifier` 排序的 `versioned_tables`。比较始终使用函数开始时读取的 `current_version`：已达到的阶段跳过，所有更高阶段依次创建。
4. `create_tables` 对每张表依次解析 `create_sql`，将语句向下转换为 `ast::CreateTableStmt`，用空选项上下文构建 `TableInfo` 并检查构建结果中的小写表名 `Name.L` 与声明名称一致。
5. 写入时使用声明的保留 ID、由表名构造的 `CiString` 和其余字段为默认值的 `TableInfo`。
6. 每完成一个阶段，入口更新内存中的 `largest_version`。只有所有待创建阶段都成功后，才将最大版本映射回 `DDLTableVersion` 并调用 `set_ddl_table_version`；最新版本场景不重复写版本。

版本数组的顺序和枚举序值共同构成不变量：新增阶段必须保持单调顺序，否则“跳过旧阶段、最终写最大版本”的逻辑会失真。

## 数据与状态

静态状态只有四个不可变数组，共七项 `TableBasicInfo`。可变状态全部经调用者传入的 `&mut Mutator` 访问，包括当前 DDL 表版本、`mysql` 库 ID 和表元数据。

`largest_version` 是一次调用内的局部提交候选值；它从 `current_version` 开始，只在成功完成一个待创建组后增长。最后的 `match` 明确恢复 `Base`、`Mdl`、`Backfill`，其余更大值统一映射为当前末级 `DdlNotifier`。在现有四阶段数组下这能得到正确结果，但若新增版本而未同步该映射，新版本会被错误降为 `DdlNotifier`。

当前 Rust 测试 `init_ddl_tables_creates_only_tables_newer_than_the_stored_version` 验证五种初始版本、创建数量、表 ID/名称和最终版本；它没有验证实际列、索引、表状态或完整 `TableInfo`。

## 依赖与调用关系

上游边界如下：

- `pkg/session/lib.rs` 声明模块并在 crate 根重导出公开符号。
- `pkg/session/test/meta/session_test.rs:init_ddl_tables_creates_only_tables_newer_than_the_stored_version` 是已找到的 Rust 直接调用者。
- RustCodeGraph 的 `callers InitDDLTables` 未给出 Rust 生产调用边；仓库级 `rg` 也只找到重导出、注释和测试，因此生产接线状态为“未发现”。

下游依赖如下：

- `astersql-meta::{DDLTableVersion, Mutator}` 提供版本枚举和元数据读写边界。
- `astersql-parser::Parser` 与 `astersql-parser-ast` 提供 SQL 解析及 `CreateTableStmt` 类型检查。
- `astersql-ddl::BuildTableInfoFromAST` 和 `astersql-meta-metabuild::NewContext` 验证 AST 能构造成表元数据。
- `astersql-meta-metadef` 提供七张系统表的稳定 ID 和建表 SQL；`astersql-meta-model`/`astersql-meta` 提供最终写入的 `TableInfo` 与 `CiString`。

`pkg/session/Cargo.toml` 明确声明了上述 `astersql-ddl`、`astersql-meta`、`astersql-meta-metabuild`、`astersql-meta-metadef`、`astersql-meta-model`、`astersql-parser` 和 `astersql-parser-ast` 路径依赖；本模块不受 crate 的 `nextgen` feature 条件编译控制。

## 错误处理与边界

所有失败统一转换为带操作上下文的 `String`：读取版本、建立系统库、解析具体表 DDL、AST 类型不符、构建元数据、表名不符、创建表和写版本均可中止流程。错误消息包含表名，便于定位具体定义。

版本只在所有待补表写入成功后更新，避免“版本已前进但后续表尚未创建”。不过函数本身不创建、提交或回滚事务：若中途出错，之前对 `Mutator` 的写入是否回滚完全取决于调用者提供的事务生命周期。重复调用的安全性也依赖 `Mutator::create_table_or_view` 及外层回滚；本文件没有显式的“已存在则跳过”检查。

解析阶段只接受 `CREATE TABLE` AST，并检查构建出的表名；它没有执行 Go 版的系统表约束检查，也没有校验建成元数据的其余字段。空表数组不是公开配置路径，当前四组均非空。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、网络连接或存储句柄。`&mut Mutator` 的独占借用保证单次调用期间不能由同一 Rust 引用并发修改该 mutator，七张表按数组顺序同步处理。

事务边界和持久化原子性不在本函数中。Go 版用 `kv.RunInNewTxn(..., true, ...)` 将建表和版本更新包在同一可重试事务中；Rust 入口只接受已构造的 `Mutator`，所以调用者必须保证等价的提交/回滚范围。Go 版还在写元数据前请求 split/scatter 表区域；Rust 文件没有存储句柄，无法执行该资源操作。

## 与 Go 版本的对应关系

直接对照为 `pkg/session/session.go:TableBasicInfo`、四组同名表定义、`ddlTableVersionTables`、`InitDDLTables` 和 `createAndSplitTables`。两版一致的核心意图是：固定 ID/名称/SQL、按四个版本阶段补建、跳过不高于当前版本的阶段，以及最后推进版本。Go 测试 `pkg/session/test/meta/session_test.go:TestInitDDLTables` 与 Rust 测试采用相同五种起始版本和相同的期望切片边界（0、3、4、6、7）。

尚未对齐之处包括：

- Go 入口接收 `kv.Storage` 并自行开启标记为 `InternalTxnDDL` 的可重试新事务；Rust 接收 `&mut Mutator`。
- Go 的 `createAndSplitTables` 保留完整的 `BuildTableInfoFromAST` 结果，设置 `StatePublic`、固定 ID 和 `UpdateTS`，再检查系统表约束；Rust 只保留 ID 和名称，其他字段使用默认值。
- Go 会收集表 ID 并调用 `splitAndScatterTable`，随后批量写入构建好的表信息；Rust 不 split/scatter，且逐表立即写入。
- Go 包含日志、failpoint 和错误类型/堆栈；Rust 返回格式化字符串，也没有对应 failpoint。
- Go 的生产 bootstrap 已调用该入口；当前 Rust 仓库未发现生产调用点。

因此当前 Rust 实现应视为一段受测试覆盖的阶段选择与基础元数据写入实现，而非 Go bootstrap 行为的完整移植。

## 扩展指南

新增 DDL 系统表时，先在 `astersql-meta-metadef` 定义唯一保留 ID 和建表 SQL，再把 `TableBasicInfo` 放入正确版本组；新增版本阶段还必须同步 `versioned_tables` 顺序、`largest_version` 到枚举的映射及 `DDLTableVersion` 定义。应在独立测试 `pkg/session/test/meta/session_test.rs` 增加每个历史起点的期望偏移，并验证 ID、名称以及完整模式字段。

若要达到 Go 等价行为，最关键的修改点是 `create_tables`：应持久化 `built` 的完整元数据、设置公开状态/时间戳、执行系统表约束检查，并明确 split/scatter 的实现归属。生产接线还需要在 Rust bootstrap 的事务拥有者处调用 `InitDDLTables`，保证表创建与版本写入同事务提交；不要在本文件内另造隐式事务而破坏调用者的原子性设计。

兼容性风险主要是保留 ID 冲突、版本顺序错误、旧集群升级时漏建或重复建表，以及默认 `TableInfo` 导致运行时读到不完整模式。性能风险集中在 bootstrap 时逐表解析与写入，以及未来引入 split/scatter 后的等待策略。Rust 单元测试和生产源码应继续分文件维护，不要把回归测试嵌入 `ddl_tables.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/session/ddl_tables.rs` 核对了 143 行完整实现；`query InitDDLTables` 定位 Rust/Go 对应符号；`callees InitDDLTables` 确认四组常量与 `create_tables` 调用；`callers InitDDLTables` 未返回 Rust 调用边。
- 模块与依赖：`pkg/session/lib.rs`、`pkg/session/Cargo.toml`、`pkg/session/bootstrap.rs:TableBasicInfo`。
- Go 对照：`pkg/session/session.go:4220-4430`，重点为 `InitDDLTables`、`createAndSplitTables` 与 split/scatter 辅助逻辑。
- 独立测试：`pkg/session/test/meta/session_test.rs:init_ddl_tables_creates_only_tables_newer_than_the_stored_version`、`ddl_system_table_reserved_ids_are_distinct_and_within_bounds`；Go 对照为 `pkg/session/test/meta/session_test.go:TestInitDDLTables`。仓库搜索还确认没有与目标源同文件内嵌的测试。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证文档恰有十一个固定二级章节，并人工复核“位置、运行方式、安全扩展”均有源码或测试依据。
