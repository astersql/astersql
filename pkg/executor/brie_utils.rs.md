# `pkg/executor/brie_utils.rs`

## 文件定位

[目标源文件](brie_utils.rs)位于 `astersql-executor` crate。`pkg/executor/Cargo.toml` 将 crate 根指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/executor"` 声明其 Go 对照包；`pkg/executor/lib.rs` 通过 `pub mod brie_utils` 公开本模块，并仅在 `cfg(test)` 下装配独立测试 `pkg/executor/brie_utils_test.rs`。

它是 BRIE（Backup/Restore/Import/Export）恢复阶段的 DDL 辅助层：把“渲染恢复用 CREATE SQL、暂存会话可观察状态、调用建库/建表、批量过大时拆分重试”抽象到泛型 trait `BrieDDLContext` 后面。当前仓库的 Rust 生产代码没有 `BrieDDLContext` 的实现，也没有从 `pkg/executor/brie.rs` 调用这些函数；RustCodeGraph 与仓库搜索只确认了测试中的实现和调用。因此本文件目前是已移植并有单元测试的公共能力，尚不能据此认定 Rust BRIE 主执行链已经接入。

对应的 Go 线上实现位于 `pkg/executor/brie_utils.go`，由 `pkg/executor/brie.go` 的 glue session 建库、建表入口调用。

## 核心职责

1. `showRestoredCreateDatabase` 与 `showRestoredCreateTable` 在上下文渲染的 DDL 前原样附加 BR 注释，形成会话审计/展示使用的 query string。
2. `BRIECreateDatabase` 克隆数据库元数据、补默认字符集，以“已存在即报错”语义建库，并在调用结束后恢复原 query string。
3. `BRIECreateTable` 以“已存在即忽略”语义建单表；DDL 调用期间临时关闭外键检查并设置 query string，成功或返回错误后均恢复原会话状态。
4. `BRIECreateTables` 按数据库分组渲染所有表的 SQL，关闭外键检查，并把批量创建委托给 `splitBatchCreateTable`；外层结束时恢复 query string 与外键检查状态。
5. `splitBatchCreateTable` 先尝试完整批次，只对上下文判定为 TiKV entry/transaction-too-large 的错误递归二分，并严格先完成左半批再处理右半批。

本文件不解析 SQL、不持有真实 session/domain/DDL executor，也不识别具体 TiKV 错误类型；这些职责都由 `BrieDDLContext` 的实现者提供。

## 主要符号

- `defaultCapOfCreateTable: usize = 512`、`defaultCapOfCreateDatabase: usize = 64`：仅是渲染结果字符串的预分配容量提示，不限制最终 SQL 长度。
- `BrieDDLContext`：生产边界 trait。关联类型 `Database`、`Table`、`CreateTableOption` 必须可克隆；`Error` 保持调用方的原始错误类型。其方法分为四组：DDL 渲染，query string/外键检查状态访问，单个及批量 DDL 执行，以及“是否为可拆分的大 entry/事务错误”分类。
- `showRestoredCreateDatabase(context, database, br_comment)`：渲染数据库 DDL并在前面拼接注释；渲染错误直接返回。
- `BRIECreateDatabase(context, database, br_comment)`：设置临时 query string，克隆数据库后补字符集，调用 `create_database_on_exist_error`，随后恢复 query string。
- `showRestoredCreateTable(context, table, br_comment)`：渲染表 DDL 并拼接注释。
- `BRIECreateTable(context, schema, table, br_comment, options)`：设置临时 query string、关闭外键检查，克隆表并调用 `create_table_on_exist_ignore`，再恢复两项状态。
- `BRIECreateTables(context, cloned_tables, br_comment, options)`：输入使用 `BTreeMap<String, Vec<Table>>`，所以 Rust 版本按数据库名有序遍历；每个库内保持 `Vec` 的表顺序。
- `mergeQuerys(queries)`：顺序拼接每条字符串，并在每条后追加分号；空输入返回空字符串，不检查输入是否已经含分号。
- `splitBatchCreateTable(context, schema, tables, queries, options)`：要求表与 SQL 数量相同，设置本批 query string并尝试批量 DDL；可拆分错误且批次大于一项时递归处理两半。

全部函数和常量都是公开符号，并通过 crate 的公开模块可访问；命名保留 Go 风格，文件级 `#![allow(non_snake_case)]` 明确允许这些名称。

## 执行流程

建库流程从 `BRIECreateDatabase` 开始：先调用 `showRestoredCreateDatabase`。渲染成功后保存原 query string，写入“BR 注释 + CREATE DATABASE”，克隆输入数据库，调用 `ensure_default_charset`，再执行 `create_database_on_exist_error`。无论 DDL 返回成功还是普通 `Err`，函数都会在返回该结果前恢复原 query string。渲染阶段若失败，会在任何状态修改前返回。

单表流程与建库相似，但 `BRIECreateTable` 还保存外键检查开关，并在 DDL 前将其置为 `false`。`create_table_on_exist_ignore` 收到克隆后的表和调用方 options；调用结束后先恢复 query string，再恢复外键检查，最后原样返回 DDL 结果。

批量流程由 `BRIECreateTables` 保存原 query string/外键状态并关闭外键检查。它按 `BTreeMap` 次序遍历数据库，为该库的每张表调用 `showRestoredCreateTable`，保持表和 query 两个切片位置一一对应，然后调用 `splitBatchCreateTable`。任一渲染或 DDL 错误通过闭包的 `Result` 短路，闭包外仍恢复两项状态。

拆批流程如下：

1. `assert_eq!` 检查 `tables.len() == queries.len()`。
2. `mergeQuerys` 生成当前批次 query string，并写入上下文。
3. 调用 `batch_create_tables_on_exist_ignore` 尝试整批。
4. 成功则结束；普通错误、或只有一张表时仍然过大，直接返回原错误。
5. 只有“错误属于 entry/transaction too large 且表数大于 1”才按 `len / 2` 切分；递归完成左半批后才开始右半批。左半批失败会阻止右半批执行。

因此一次拆分成功后，context 中最后留下的是最后一个成功叶子批次的 query string；外层 `BRIECreateTables` 再把它恢复为调用前的值。直接调用 `splitBatchCreateTable` 则不会自动恢复。

## 数据与状态

本模块自身没有全局可变状态。主要数据是借用的数据库/表描述、创建选项以及由上下文承载的会话状态。

- 数据库和表在执行单项 DDL 前克隆，调用方传入对象不会被本模块直接修改；数据库默认字符集只写入克隆值。
- 批量表切片直接借给上下文，不在本模块内复制；query 列表由 `BRIECreateTables` 为每个数据库重新创建。
- `query_string` 用于让恢复 DDL 在审计、日志或展示链路中可观察。拆批时每次递归都把它更新为与当前表子切片完全对应的 SQL 子串。
- 外键检查只在单表/批量建表期间临时关闭，建库不改变它。
- `BTreeMap` 使跨数据库执行顺序确定；库内和递归叶子批次顺序与输入 `Vec` 一致。

状态恢复依赖 trait 方法正常返回。Rust 代码没有 RAII guard，也没有捕获 panic：若上下文方法 panic，query string 或外键检查可能来不及恢复。正常的 `Result::Err` 路径则由 `pkg/executor/brie_utils_test.rs` 覆盖并确认恢复。

## 依赖与调用关系

直接语言依赖只有标准库 `std::collections::BTreeMap`；具体 DDL、session、元数据和 TiKV 错误依赖全部反转到 `BrieDDLContext`，因此文件本身不直接引用 `astersql-ddl`、`astersql-domain` 或 `astersql-kv`。

RustCodeGraph 的下游边显示：

- `showRestoredCreateDatabase -> BrieDDLContext::render_create_database`。
- `BRIECreateDatabase -> showRestoredCreateDatabase`，并调用 query 状态、默认字符集和建库方法。
- `showRestoredCreateTable -> BrieDDLContext::render_create_table`。
- `BRIECreateTable -> showRestoredCreateTable`，并调用 query/外键状态及单表 DDL 方法。
- `BRIECreateTables -> showRestoredCreateTable -> splitBatchCreateTable`。
- `splitBatchCreateTable -> mergeQuerys`，并调用 query 设置、批量 DDL 和错误分类方法；它还递归调用自身。

上游方面，`pkg/executor/lib.rs` 公开模块，`pkg/executor/brie_utils_test.rs` 为当前仓库中唯一明确实现 `BrieDDLContext` 并调用 Rust API 的文件。仓库搜索未找到生产实现或 `pkg/executor/brie.rs` 到本模块的调用。Go 主链则由 `pkg/executor/brie.go` 的 `CreateDatabaseOnExistError`、`CreateTable`、`CreateTables` 分别调用 Go 版 `BRIECreateDatabase`、`BRIECreateTable`、`BRIECreateTables`。

## 错误处理与边界

渲染和 DDL 错误使用 `C::Error` 原样传播，本模块不包装、不记录也不改变错误身份。只有 `splitBatchCreateTable` 会询问 `is_entry_or_transaction_too_large` 来决定是否重试；其他错误绝不拆分。即使属于过大错误，单表批次也直接返回，避免零长度切分或无限递归。

重要边界包括：

- `tables` 与 `queries` 数量不相等会触发断言 panic，而不是返回 `C::Error`。调用方必须维持位置对应不变量。
- 空批次会设置空 query string并调用一次批量 DDL；其结果完全由上下文决定。
- `mergeQuerys` 总是追加分号，调用方应传入不带终止分号的单条 query，避免产生重复分号。
- 左半批已经成功、右半批随后失败时不会回滚左半批；本算法保证顺序，不提供跨批原子性。
- `BRIECreateDatabase` 的渲染失败发生在状态修改前；建库失败后 query string仍恢复。单表和批量路径对普通错误也恢复 query string/外键状态。
- `BRIECreateTables` 在不同数据库之间遇错即停止，后续数据库不会执行。

测试 `split_propagates_singleton_size_and_non_size_errors_without_retry` 验证单项过大错误与普通错误都只尝试一次；`single_table_creation_restores_session_state_on_ddl_error` 和 `table_render_error_does_not_mutate_session_state` 验证两类失败边界。

## 并发与资源生命周期

所有入口都接受独占的 `&mut C`，文件内没有线程、异步任务、锁、通道或共享所有权；同一 context 上的调用在 Rust 类型层面是串行的。是否可跨线程共享完全取决于具体 `BrieDDLContext`，本 trait 没有 `Send`/`Sync` 约束。

临时字符串和每库 query `Vec` 在函数调用内创建并自动释放。递归拆批借用原 `tables`/`queries` 的子切片，不复制表数据；递归深度按二分约为 `O(log n)`，但每个失败的内部节点都会重新合并其 query 子切片，最坏情况下会产生多轮字符串分配和总计约 `O(n log n)` 的 SQL 字符复制。DDL 尝试次数在全部拆到单项时最多形成一棵完整二叉树，即约 `2n - 1` 次批量调用。

会话状态的生命周期是显式“保存—覆盖—调用—恢复”。它覆盖普通返回和 `Result::Err`，但不具备 panic 安全；将来若上下文方法可能 panic，应优先引入作用域 guard，而不是继续增加手写恢复分支。

## 与 Go 版本的对应关系

Rust 直接对照 `pkg/executor/brie_utils.go`：容量常量值相同；渲染函数同样先写 BR 注释；建库使用 OnExistError；建表/批量建表使用 OnExistIgnore；建表期间关闭外键检查；批量过大时只识别 entry-too-large/transaction-too-large，按中点先左后右递归；每批 query 都是原顺序加分号拼接。

Rust 用 `BrieDDLContext` 抽象了 Go 中对 `sessionctx.Context`、`domain.GetDomain(...).DDLExecutor()`、`model.DBInfo`/`TableInfo`、DDL option 和 `kv.ErrEntryTooLarge`/`ErrTxnTooLarge` 的直接依赖。错误分类是否与 Go 精确一致，最终取决于生产 context 的实现；当前仓库尚无该实现。

可观察差异与迁移状态：

- Go 的跨库输入是普通 `map`，遍历次序未规定；Rust 使用 `BTreeMap`，按键排序，行为更确定。
- Go 使用 `defer` 恢复 session 状态；Rust 在普通返回路径显式恢复。两者都不把拆分后的已成功 DDL回滚，但 Rust 显式恢复不具备 panic 时的 `defer` 等价保证。
- Go 会对部分错误使用 `errors.Trace` 并记录批量失败/拆批日志；Rust 保留原 `C::Error`，本文件没有日志。
- Go 暴露 `SplitBatchCreateTableForTest` 变量；Rust 直接公开 `splitBatchCreateTable` 并从独立测试模块调用。
- Go 测试 `TestBRIECreateDatabase`、`TestBRIECreateTable`、`TestBRIECreateTables` 使用真实 mock store/domain 验证 DDL 结果；`TestSplitTablesQueryMatch` 验证拆分 query 与成功表一一对应。Rust 测试使用 `SplitContext` 验证抽象算法和状态契约，但不是实际 domain/DDL 集成测试。

## 扩展指南

若要把本模块接入 Rust BRIE 主链，首先应在合适的生产适配层实现 `BrieDDLContext`，把渲染、session query string、外键开关、OnExist 语义、DDL executor 和两类 TiKV 大小错误精确映射到现有 Rust 类型；随后从 `pkg/executor/brie.rs` 的 glue session 建库/建表入口调用本模块。不能仅提供 mock 实现后宣称接线完成。

修改渲染规则时应同时检查 `showRestoredCreateDatabase`、`showRestoredCreateTable` 以及 `pkg/executor/brie_utils_test.rs` 的注释前缀/SQL 断言，并与 Go 的 `ConstructResultOfShowCreateDatabase`、`ConstructResultOfShowCreateTable` 行为对齐。修改拆批策略时必须保持 `tables[i]` 与 `queries[i]` 对应、只对两类大小错误重试、左批先完成、单项错误终止等不变量，并扩充独立 Rust 测试，不能把测试嵌入生产文件。

若新增可变 session 状态，应与 query string/外键开关一样在全部普通成功和错误路径恢复；更稳妥的方向是建立统一 guard，以补足 panic 安全。若改变跨库容器或并行化批次，应先明确是否允许改变数据库执行顺序、部分成功语义和 session query 的可观察时序。

性能扩展应关注失败节点重复构造 SQL 的成本。可以在不改变每批 query 内容及顺序的前提下评估复用容量或按范围构造字符串，但不能让 query string 与实际提交表集合失配。

## 验证依据

- 源文件：`pkg/executor/brie_utils.rs`，RustCodeGraph `node --file` 覆盖 1–211 行，确认 2 个常量、`BrieDDLContext`、6 个 DDL/渲染/拆批入口和 `mergeQuerys` 的完整实现。
- 符号与调用图：对 `BrieDDLContext`、`showRestoredCreateDatabase`、`BRIECreateDatabase`、`showRestoredCreateTable`、`BRIECreateTable`、`BRIECreateTables`、`mergeQuerys`、`splitBatchCreateTable` 执行 `query`/`callees`；图结果确认本文“依赖与调用关系”所列下游边。`callers` 子命令没有产出有效结果，因此上游接线结论另由 `explore` 和精确仓库搜索交叉验证，不把空图结果当作不存在调用的唯一依据。
- crate 边界：`pkg/executor/Cargo.toml` 确认包名 `astersql-executor`、crate 根 `lib.rs`、Go 包元数据；`pkg/executor/lib.rs` 确认公开 `brie_utils` 以及独立 `cfg(test)` 测试装配。`pkg/executor` 下没有 `doc.go`。
- Go 对照：`pkg/executor/brie_utils.go` 1–186 行；上游入口来自 `pkg/executor/brie.go` 中 glue session 的建库、建表和批量建表方法。
- Rust 测试：`pkg/executor/brie_utils_test.rs` 1–269 行，覆盖递归顺序、query 拼接、默认字符集、状态恢复、渲染错误、单项过大错误及非大小错误。
- Go 测试：`pkg/executor/brie_utils_test.go` 的 `TestBRIECreateDatabase`、`TestBRIECreateTable`、`TestBRIECreateTables`、`TestSplitTablesQueryMatch`，以及其单项过大错误检查，提供真实 Go DDL 行为和 query 对齐证据。
- 接线限制：`rg` 对 `BrieDDLContext`、`BRIECreateDatabase`、`BRIECreateTable`、`BRIECreateTables` 和 `brie_utils::` 的 Rust 搜索仅命中本文件与 `pkg/executor/brie_utils_test.rs`，未发现生产 trait 实现或 `pkg/executor/brie.rs` 调用。因此“尚未接入 Rust 生产主链”是当前仓库事实，而非设计推测。
