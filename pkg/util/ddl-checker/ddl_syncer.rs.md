# `pkg/util/ddl-checker/ddl_syncer.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-util-ddl-checker`，包入口是同目录的 [`lib.rs`](./lib.rs)，后者公开导出本文件的 `DBConfig`、`UpstreamDatabase`、`UpstreamDatabaseFactory`、`DDLSyncer` 和 `NewDDLSyncer`。它位于“从上游读取表定义”和“在隔离的 TiDB 检查会话中执行 SQL”之间：上游访问由两个 trait 隔离，实际落地委托给 [`ExecutableChecker`](./executable_checker.rs)。

当前仓库事实是：这些符号虽然已从 crate 根公开，但对 Rust 源码做精确引用搜索只找到本文件和 `lib.rs`，没有生产调用者，也没有 `ddl_syncer_test.rs` 或覆盖这些 API 的测试。因此它是已实现、可注入的迁移边界，但尚不能据此宣称已接入应用主链。

## 核心职责

- 用 `DBConfig` 描述打开上游数据库所需的连接与快照参数；本文件只传递配置，不解释 `snapshot` 的格式，也不自行建立网络连接。
- 用 `UpstreamDatabaseFactory::open_database` 把具体驱动的打开、认证和 ping 行为留给实现方，并由 `NewDDLSyncer` 统一传播构造错误。
- 用 `DDLSyncer::SyncTable` 读取指定 `schemaName.tableName` 的 `CREATE TABLE` 文本，删除检查会话里的同名表，再执行上游建表语句，以更新本地可执行性检查环境。
- 用 `DDLSyncer::Close` 同时尝试关闭本地检查器和上游数据库，并保持 Go 版本“检查器错误优先”的返回规则。

本文件不解析 DDL、不检查表依赖，也不持久化 schema；这些能力分别属于上游实现和 `ExecutableChecker`。

## 主要符号

- `DBConfig { host, user, password, schema, snapshot, port }`：拥有连接参数的公开值类型，派生 `Clone`、`Debug`、`Eq`、`PartialEq`。注意派生的 `Debug` 会输出 `password`；调用方不应直接把完整配置写入日志。
- `UpstreamDatabase: Send`：已打开上游连接的最小接口。`get_create_table_sql(&mut self, context, schema_name, table_name)` 返回建表 SQL；`close(&mut self)` 释放连接。`Send` 只规定句柄可在线程间转移，不表示方法可并发调用。
- `UpstreamDatabaseFactory`：以 `DBConfig` 创建 `Box<dyn UpstreamDatabase>`。trait 本身没有默认实现；打开、ping、DSN 组装和快照应用都必须由外部实现提供。
- `DDLSyncer<'checker>`：拥有上游句柄 `db`，同时独占可变借用 `&'checker mut ExecutableChecker`。生命周期参数保证同步器存活期间，调用方不能另行可变使用或销毁该检查器。
- `NewDDLSyncer(cfg, executableChecker, factory)`：公开构造函数。仅调用 `factory.open_database(cfg)`；成功后保存句柄和检查器借用，失败时不构造部分对象。
- `DDLSyncer::from_parts(db, executable_checker)`：绕过 factory 的依赖注入入口，适合独立测试或已持有连接的集成方。
- `DDLSyncer::SyncTable(tidbContext, schemaName, tableName)`：同步单表的主入口。
- `DDLSyncer::Close()`：显式关闭两个资源；不实现 `Drop`，离开作用域不会自动调用该方法。
- `checker()` / `checker_mut()`：分别暴露内嵌检查器的共享/可变访问；后者允许调用方在同步器生命周期内执行额外检查器操作。
- `impl From<&str> for CheckerError`：把字符串转成无底层 cause 的 `CheckerError::new`。它不是同步流程必需步骤，但扩大了本 crate 的错误构造便利性。

文件内没有模块级常量、枚举、条件编译项或异步函数。

## 执行流程

构造流程如下：

1. 调用方先拥有一个可变的 `ExecutableChecker`，并准备 `DBConfig` 与 `UpstreamDatabaseFactory` 实现。
2. `NewDDLSyncer` 调用 `factory.open_database(cfg)`。
3. 若打开或 ping 失败，`?` 原样返回 `CheckerError`；若成功，连接所有权移入 `DDLSyncer`，检查器则以独占可变借用保存。

`SyncTable` 的顺序是严格且可观察的：

1. 用新建的 `ExecutionContext::background()` 调用 `db.get_create_table_sql`。因此获取上游 DDL 不继承调用方 `tidbContext` 的 `request_id` 或 `cancelled` 状态。
2. 获取成功后，以调用方的 `tidbContext` 执行 `ExecutableChecker::DropTable(tableName)`。相邻实现会生成带反引号的 `drop table if exists`。
3. 删除成功后，以同一 `tidbContext` 调用 `ExecutableChecker::Execute(create_table_sql)` 重建表。
4. 任一步失败立即返回，后续步骤不执行；全部成功才返回 `Ok(())`。

`Close` 不采用遇错即停：先调用 `ec.Close()` 并保存错误，再调用 `db.close()` 并保存错误，最后按“检查器错误、数据库错误、成功”的优先级返回。这样即使检查器关闭失败，数据库关闭仍会被尝试。

## 数据与状态

`DBConfig` 是纯拥有型配置对象；六个字段均公开。`schema` 是默认目标库信息，但 `SyncTable` 仍显式接收 `schemaName`，本文件没有校验两者一致。`snapshot` 也只交给 factory，是否应用一致性读取由具体 factory/数据库实现保证。

`DDLSyncer` 的长期状态只有两个字段：拥有的 `Box<dyn UpstreamDatabase>` 和借用的 `ExecutableChecker`。它不缓存建表 SQL、不记录已同步表、不维护重试计数。一次 `SyncTable` 只处理一个表，而且删除与重建之间没有事务或补偿状态。

`ExecutionContext` 来自 `executable_checker.rs`，当前只含 `request_id` 和 `cancelled`。上游读取刻意使用默认后台上下文，本地 Drop/Execute 使用调用方上下文，这一上下文分离与 Go 版本一致。

## 依赖与调用关系

直接源码依赖只有 `crate::executable_checker::{CheckerError, CheckerResult, ExecutableChecker, ExecutionContext}`。关键被调边经 RustCodeGraph 核对为：

- `NewDDLSyncer` → `UpstreamDatabaseFactory::open_database`；
- `SyncTable` → `UpstreamDatabase::get_create_table_sql`；源码随后直接调用 `ExecutableChecker::DropTable` 和 `ExecutableChecker::Execute`；
- `Close` → `ExecutableChecker::Close` 与 `UpstreamDatabase::close`。

`Cargo.toml` 声明该 crate 的 Go 对照包为 `pkg/util/ddl-checker`，并依赖 parser、session、mockstore、dbutil、logutil 等本地 crate。对本文件而言，这些具体设施由 `ExecutableChecker` 或将来的 factory 实现间接承接；本文件本身没有直接使用 `astersql-util-dbutil`，而是定义了独立的 `DBConfig` 和数据库 trait 边界。

上游调用关系方面，`lib.rs` 是当前唯一 Rust 引用方，负责再导出公共 API。RustCodeGraph 的 callers 查询和仓库级 `rg` 均未找到 `NewDDLSyncer`、`SyncTable`、`from_parts` 的其他 Rust 调用点，因此真实生产接线和具体上游数据库实现仍未验证/未发现。

## 错误处理与边界

所有可失败入口统一返回 `CheckerResult<T>`。构造、上游取 DDL、删除本地表和执行建表 SQL均使用 `?` 保留首个错误；本文件不重写消息、不重试，也不附加 schema/table 上下文。`From<&str>` 构造的错误没有 source 链，而 `CheckerError::trace` 才会保留底层错误。

重要边界包括：

- 上游取 DDL 失败时，本地状态不变。
- 本地 Drop 失败时，不执行 Create。
- Drop 成功而 Create 失败时，本地表保持已删除状态；没有回滚、事务包裹或恢复旧 DDL。这是扩展重试/原子性时必须保留或明确改变的行为契约。
- `tableName` 在 Drop 路径由 `ExecutableChecker` 用反引号包围，但上游返回的 `create_table_sql` 被直接执行；其正确性和可信边界由 `UpstreamDatabase` 实现负责。
- `Close` 总会尝试两个关闭动作；两者都失败时只返回检查器错误，数据库错误会被遮蔽且本文件不记录日志。
- `ExecutableChecker::Close` 只允许成功一次；因此重复调用 `DDLSyncer::Close` 会得到检查器的 “already closed” 错误，但仍再次尝试 `db.close()`。

## 并发与资源生命周期

本文件没有线程、任务、锁、通道或异步运行时。所有会改变资源的操作都要求 `&mut self`，同一个 `DDLSyncer` 不能在安全 Rust 中被两个调用者同时执行 `SyncTable` 或 `Close`。`UpstreamDatabase: Send` 允许其 trait object 被转移到另一线程，但不提供 `Sync` 或内部并发保证。

连接由 `DDLSyncer` 独占拥有，检查器由它独占借用；`checker_mut` 返回的借用受普通 Rust 借用规则限制。资源释放依赖显式 `Close`：结构体没有 `Drop` 实现，所以直接丢弃同步器时，数据库 trait object 自身会被 drop，但协议级 `close()` 与 `ExecutableChecker::Close()` 不会由本文件保证执行。调用方应在不再同步时显式关闭，并处理返回错误。

## 与 Go 版本的对应关系

同路径 `ddl_syncer.go` 是直接语义依据。两版都持有上游数据库与 `ExecutableChecker`，都按“后台 context 拉取 DDL → 调用方 context 删除表 → 调用方 context 执行 CREATE”的顺序同步，也都在关闭时尝试两个资源并优先返回检查器错误。

Rust 为可测试性和类型隔离增加了几处显式抽象：Go 直接使用 `*sql.DB`、`dbutil.DBConfig`、`dbutil.OpenDB/GetCreateTableSQL/CloseDB`，Rust 则使用本地 `DBConfig`、`UpstreamDatabaseFactory` 和 `UpstreamDatabase` trait；`from_parts`、`checker`、`checker_mut` 也没有 Go 同名入口。Rust 构造函数因此要求调用方额外传 factory，当前仓库未发现具体实现。

错误语义基本对齐但表达不同：Go 用 `errors.Trace` 包装每一步，Rust 用 `CheckerResult` 和 `?` 传播 factory/trait/检查器返回的错误，不在 `SyncTable` 中新增 trace 层。Go 的 `context.Background()` 对应 Rust 的 `ExecutionContext::background()`，后者只是最小化数据结构，并不等同于完整 Go context 的 deadline/value/cancellation 传播能力。

测试现状并不对称：同目录 Go 测试 `executable_checker_test.go` 只覆盖解析与执行检查器，没有直接覆盖 `DDLSyncer`；Rust 的 `executable_checker_test.rs` 同样只覆盖检查器。因而同步顺序、错误短路、双关闭和错误优先级目前都缺少独立回归测试。

## 扩展指南

- 接入真实上游时，应在独立生产模块实现 `UpstreamDatabaseFactory` 和 `UpstreamDatabase`，明确负责 DSN、认证、ping、snapshot 和关闭语义；不要把驱动细节塞回 `SyncTable`。接线后应更新 crate 依赖与调用入口，并验证敏感 `DBConfig` 不被 Debug 日志泄露。
- 改变同步流程时，优先修改 `SyncTable`，并保留或有意识地调整三个阶段的次序、两种 context 的使用以及失败短路。若要保证 Drop/Create 原子性，需要设计事务或旧结构补偿，而不能只增加重试。
- 改变资源清理时，修改 `Close`，并明确双失败时的返回/聚合策略。若增加 `Drop`，不能在析构中假定可可靠报告协议关闭错误。
- 扩展上游能力时，在 `UpstreamDatabase` 增加最小必要方法；若仅影响创建策略，则优先扩展 factory，避免让 `DDLSyncer` 依赖具体数据库类型。
- 应新增独立测试文件（建议 `pkg/util/ddl-checker/ddl_syncer_test.rs`），并从 `lib.rs` 的 `#[cfg(test)]` 区域挂载，避免把测试内嵌进生产 `.rs`。至少覆盖：factory 成败、后台/调用方 context 分离、get/Drop/Execute 调用顺序、每阶段错误短路、Create 失败后的非回滚状态、两个 close 都被调用、双错误时检查器错误优先、重复 Close，以及访问器借用行为。同步修改 Go 行为时还应补同目录 Go 独立测试并保持断言一致。
- 兼容性风险集中在公开 trait 和 `DBConfig` 字段变更（会破坏实现方/结构体字面量）；正确性风险集中在非原子的 Drop/Create 与错误遮蔽；性能风险主要来自每张表一次上游查询加两次本地 SQL 执行，本文件当前没有批量、缓存或并行机制。

## 验证依据

- 源文件：[`ddl_syncer.rs`](./ddl_syncer.rs)，核对全部 137 行以及 `DBConfig`、两个 trait、`DDLSyncer`、构造/同步/关闭/访问器和错误转换实现。
- 相邻实现：[`executable_checker.rs`](./executable_checker.rs)，核对 `ExecutionContext::background`、`DropTable`、`Execute`、一次性 `Close` 和 `CheckerError` 的真实语义。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)，核对包名、Go 包元数据、依赖、模块声明、公开再导出及当前测试挂载。
- Go 对照：[`ddl_syncer.go`](./ddl_syncer.go) 与 [`executable_checker_test.go`](./executable_checker_test.go)，核对 OpenDB/GetCreateTableSQL/Drop/Execute/Close 顺序及 Go 测试覆盖范围。
- Rust 测试：[`executable_checker_test.rs`](./executable_checker_test.rs)，确认现有独立测试只覆盖 `ExecutableChecker` 的解析/执行行为，未直接覆盖同步器。
- RustCodeGraph：`status` 显示索引含目标文件；`node --file pkg/util/ddl-checker/ddl_syncer.rs` 读取完整源码；精确 query 确认 15 个目标符号；callees 确认构造、取 DDL、关闭的 trait 调用边。callers 查询无结果，随后以 `rg` 精确引用搜索复核，除 `lib.rs` 再导出外未发现 Rust 调用者。
- 未运行 Cargo：本任务只新增说明文档，计划明确排除 Cargo 验证。最终结构验证以任务文件指定命令为准。
