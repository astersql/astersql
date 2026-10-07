# `pkg/ddl/schematracker/checker.rs`

## 文件定位

本文件属于 `astersql-ddl-schematracker` crate，由同目录 `lib.rs` 的私有模块 `mod checker` 编译并通过 `pub use checker::*` 对外再导出。它位于 DDL 执行器和内存 `SchemaTracker` 之间：同一条受支持的 DDL 命令先交给可注入的 `DdlExecutor`，再镜像到 `SchemaTracker`，最后比较真实元数据与跟踪元数据。它不是 DDL job 调度器，不负责持久化 job、owner 选举、schema state 推进或 reorg；这些属于真实 DDL 实现，本文件只做测试/校验包装和少量生命周期透传。

crate 边界由 `pkg/ddl/schematracker/Cargo.toml` 定义，直接依赖 `astersql-ddl`、表达式静态能力、`astersql-meta-model`、`astersql-parser-ast` 和 `thiserror`。根 `pkg/lib.rs` 又把该 crate 暴露为 `ddl::schematracker` facade。Cargo manifest 显示 `pkg/session`、`pkg/testkit`、`pkg/domain`、`pkg/executor`、`pkg/ddl` 及 DDL 测试 crate 声明了此依赖，但当前 Rust 源码引用搜索中，`Checker`/`NewChecker` 的直接构造只在 `checker_test.rs` 出现；不能据 Cargo 依赖推断它已经像 Go 版本一样接入 bootstrap。

## 核心职责

1. 用 `DdlCommand` 将已移植的 schema/table/index DDL 参数收束成可克隆命令，确保真实执行器和内存跟踪器观察同一份逻辑输入。
2. 用 `DdlExecutor` 隔离真实 DDL 的执行、元数据查询、SHOW CREATE 渲染及生命周期接口，便于注入生产适配器或测试替身。
3. 用 `Checker` 维护真实执行器、`SchemaTracker` 和原子开关 `closed`，在命令执行后检查库/表的存在性及规范化后的 SHOW CREATE 文本。
4. 对尚未完整移植的 DDL 接口明确保留三种现状：直接成功、只向真实执行器发送 `Noop`、或 `panic!("implement me")`。这些都不是完整支持。
5. 提供 `StorageDDLInjector<T>` 的泛型包装/解包容器；当前 Rust 实现只保存 `storage`，没有 Go 版本中的 `Injector` 回调和 `kv.Storage` 能力组合，因此只是结构占位而非完整 bootstrap 注入器。

## 主要符号

- `init()`：空初始化函数，用于与 Go 包 `init` 的表面入口对齐；Rust 中不会注册注入器。
- `DdlCommand`：可克隆命令枚举。完整镜像分支包括创建/修改/删除 schema，创建/删除 table 或 view，创建/删除 index，`AlterTable`、`RenameTable`；`Noop(&'static str)` 只携带操作名。
- `DdlExecutor: Send`：真实执行器抽象。`Execute` 是唯一必需方法；查询、渲染、启动/停止、统计、syncer/owner 标识及 job 方法都有默认实现。默认查询返回 `None`，默认 SHOW CREATE 返回 `Error::Unsupported`，其余多为空值或成功，因此适配器若要启用一致性比较必须覆盖相关方法。
- `Checker`：核心包装器，拥有 `Box<dyn DdlExecutor>`、公开的 `tracker: SchemaTracker` 和 `AtomicBool closed`。`NewChecker` 以调用方提供的 `lower_case_table_names` 创建跟踪器。
- `Disable` / `Enable`：分别以 Release 写入 `closed=true/false`；检查和特殊镜像路径以 Acquire 读取。
- `execute`：通用执行模板，先调用 `realExecutor.Execute(command.clone())`，成功后按枚举分派到对应 `SchemaTracker` 方法；`Noop` 不改变跟踪器。
- `checkDBInfo` / `checkTableInfo`：比较真实端与跟踪端对象是否同时存在；都存在时借真实执行器的 SHOW CREATE 渲染函数比较完整文本。表比较跳过 `mysql` schema。
- `metadata_presence_mismatch` / `compare_rendered_metadata`：把存在性或渲染差异转成 `Error::Mismatch`，而非像 Go 版本那样 panic。
- `normalize_show_create_table` / `remove_tidb_comment`：总是剥离 clustered-index 注释；按 `IgnoreShardRowIdAndPreSplitComments` 决定是否剥离 `SHARD_ROW_ID_BITS` 和 `PRE_SPLIT_REGIONS` 注释。
- `CreateSchema`、`AlterSchema`、`DropSchema`、`CreateView`、`CreateIndex`、`DropIndex`、`RenameTable`、`CreateSchemaWithInfo`：走 `execute` 后做对象级检查。
- `CreateTable` / `AlterTable`：先执行真实 DDL；若 `closed` 为真立即成功返回，不镜像 tracker；否则更新 tracker 并检查表。
- `DropTable`：无论真实执行结果是否成功，都尝试删除 tracker 对象并检查各表，最后返回最初的真实执行结果；tracker 删除错误被忽略。
- `DropView`：真实执行失败时立即返回，不更新 tracker；成功后才更新并检查。这一顺序与 `DropTable` 有意不同。
- 生命周期透传方法：`Start`、`Stats`、`GetScope`、`Stop`、`RegisterStatsHandle`、`SchemaSyncer`、`StateSyncer`、`OwnerManager`、`GetID`、`DoDDLJob`、`GetMinJobIDRefresher`、`DoDDLJobWrapper` 均直接委托 `realExecutor`。
- `StorageDDLInjector<T>`、`NewStorageDDLInjector`、`UnwrapStorage`：泛型存储包装与单层解包，不包含运行时注入逻辑。

## 执行流程

典型受支持操作按以下顺序运行：

1. 公开方法从 spec/名称中预先保存稍后检查所需的 schema/table 名；`DdlCommand` 对需要双重消费的值使用 `Clone`。
2. 调用 `realExecutor.Execute`。通用 `execute` 在真实执行失败时通过 `?` 立即返回，因此 tracker 不被更新。
3. 真实执行成功后，`execute` 将命令映射到同名 `SchemaTracker` 方法。tracker 错误继续向调用方传播。
4. `checkDBInfo` 或 `checkTableInfo` 分别从真实执行器与 `tracker.InfoStore` 读取元数据。双方都不存在视为一致；仅一方存在返回 `Error::Mismatch`。
5. 双方都存在时，使用真实执行器的渲染规则生成 SHOW CREATE 文本。表文本先消除已知的表示差异，再做严格字符串比较。

特殊路径不能套用上述模板：

- `CreateTable` 和 `AlterTable` 在真实执行后检查 `closed`，禁用时不镜像 tracker；这与一般方法中“仍镜像、只跳过比较”的行为不同。
- `DropTable` 为了保留 Go 对照语义，在真实删除失败后仍清理 tracker，并在最后返回真实错误；`checker_test.rs::drop_table_updates_tracker_even_when_real_executor_fails` 固定了这一行为。
- `DropView` 在真实失败后停止，测试 `drop_view_does_not_update_tracker_when_real_executor_fails` 固定了相反顺序。
- `RenameTable` 会同时检查每对旧名称和新名称，验证旧对象消失且新对象出现。
- `LockTables`、`UnlockTables`、`AlterTableMode`、`RefreshMeta`、`CleanupTableLock`、`CreateMaskingPolicy` 只通过 `Noop` 调真实执行器，不改变 tracker，也不比较元数据。

## 数据与状态

`Checker` 独占 `realExecutor` 和 `SchemaTracker`；改变 DDL 状态的方法接收 `&mut self`，因此普通 Rust 借用规则保证同一实例的这些更新不会并发交错。`tracker` 是公开字段，测试和上层可直接准备或检查内存状态，这也意味着调用方能绕过 `Checker` 的镜像流程，使用时必须自行维持不变量。

`closed` 是唯一显式共享状态。它只控制一致性检查，以及 `CreateTable`/`AlterTable` 的 tracker 镜像；它不普遍禁止真实执行，也不让所有操作停止镜像。例如 `CreateSchema` 在禁用后仍会执行真实命令并更新 tracker，只是 `checkDBInfo` 提前成功返回。这个细粒度语义应通过具体方法判断，不能把 `Disable` 理解成关闭整个包装器。

`DdlCommand` 拥有命令参数，避免生命周期借用跨越真实执行和 tracker 更新。`Box<dyn DdlExecutor>` 允许运行时替换适配器，trait 的 `Send` 只表示实现可在线程间转移；`Checker` 没有内部 mutex，也没有声明可被多线程同时调用。

## 依赖与调用关系

上游方面，`lib.rs` 再导出本文件公开符号，`pkg/lib.rs` 再经 `ddl::schematracker` facade 暴露 crate。RustCodeGraph 将目标文件标为被 30 个文件使用，但精确 `callers` 查询在当前索引上无输出并持续挂起；源码引用复核只确认 `pkg/ddl/schematracker/checker_test.rs` 直接调用 `NewChecker` 和公开方法。因此，“Cargo 已声明依赖”和“运行时已经接线”必须区分，后者当前未得到 Rust 源码证据。

下游方面，`NewChecker -> NewSchemaTracker`；`execute -> DdlExecutor::Execute` 后分派到 `SchemaTracker::{CreateSchema, CreateSchemaWithInfo, AlterSchema, DropSchema, CreateTable, CreateView, DropTable, DropView, CreateIndex, DropIndex, AlterTable, RenameTable}`；检查函数读取 `SchemaTracker.InfoStore::{SchemaByName, TableByName}`，并调用 `DdlExecutor` 的元数据查询与 SHOW CREATE 渲染方法。`InitFromIS` 直接调用 `tracker.InfoStore.InitFromIS`。

这条路径属于 DDL 的校验旁路：真实执行器仍负责完整 job 生命周期。本文件不直接操作持久化系统表、schema version、owner、worker、reorg 或事务，只通过 trait 透传少数生命周期接口。

## 错误处理与边界

- 真实执行错误通常原样通过 `Result` 返回，并阻止 tracker 更新；例外是 `DropTable`，它仍尝试清理 tracker。
- tracker 操作错误在通用 `execute`、`CreateTable`、`DropView`、`AlterTable` 中传播；`DropTable` 明确忽略 tracker 删除错误。
- 元数据仅一侧存在或 SHOW CREATE 字符串不同会返回 `Error::Mismatch`。默认 `DdlExecutor` 查询返回 `None`，所以只实现 `Execute` 的适配器在创建对象后通常会被判定为“只存在于 tracker”；测试 `checker_detects_when_real_schema_is_missing` 覆盖此边界。
- `checkTableInfo` 跳过 `schema.L == "mysql"` 的系统表，防止把不在 tracker 校验范围内的系统对象当作漂移。
- SHOW CREATE 归一化只处理三类已知注释差异，不进行一般 SQL 语义等价化；其他格式、顺序、默认值差异会被当成真实不一致。
- `remove_tidb_comment` 遇到没有闭合 `*/` 的匹配前缀会停止处理并保留剩余文本，不会越界或循环。
- `CreateTableWithInfo`、`BatchCreateTableWithInfo`、`RecoverTable`、`FlashbackCluster`、`TruncateTable`、replica/repair/sequence/placement-policy 相关入口仍会 panic；它们是明确的移植缺口。
- `RecoverSchema` 和三个 resource-group 方法直接 `Ok(())`，不调用真实执行器；测试 `recover_schema_and_resource_groups_do_not_call_real_executor` 证明这是当前事实，不代表功能已执行。
- `DdlExecutor` 多数默认方法静默返回空值或成功，适合最小测试替身，但生产适配器若遗漏覆盖可能掩盖未接线能力。

## 并发与资源生命周期

`AtomicBool` 使用 Release 写和 Acquire 读，使跨线程转移后对启停状态的观察有明确同步关系。不过修改操作要求 `&mut Checker`，文件本身没有提供共享所有权、锁或后台任务；若上层需要并发访问，必须在外层提供同步并证明执行器和 tracker 的一致性。

`Checker` 拥有 boxed 执行器，随 `Checker` 一起销毁，没有自定义 `Drop`。资源启动和关闭分别由 `Start`、`Stop` 透传，文件不会自动启动或停止真实 DDL。syncer、owner 和 job 方法同样只是适配器边界，不在此处创建或管理资源。

`StorageDDLInjector<T>` 按值拥有底层存储，`UnwrapStorage` 消耗包装器并返回原值；不存在引用计数、嵌套循环或清理回调。与 Go 版本不同，Rust 包装器没有保存构造 `Checker` 的注入函数，因此当前不能仅凭该类型实现 domain bootstrap 注入。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/schematracker/checker.go`。两者共有的核心设计是：包装真实 DDL、维护 tracker、提供 Disable/Enable、真实执行后镜像、以 SHOW CREATE 比较元数据，并对 clustered-index、shard-row-id、pre-split 注释做例外处理。`DropTable` 即使真实执行失败仍更新 tracker，以及多个 TODO/panic 入口，也与 Go 行为相呼应。

重要差异如下：

- Go `Checker` 同时持有 `ddl.DDL`、`ddl.Executor`、`InfoCache`，真实元数据来自 `InfoCache.GetLatest()`；Rust 把执行、查询和渲染统一抽象为一个 `DdlExecutor`。
- Go 不一致与 tracker 错误多以 panic 暴露；Rust 核心已改为 `Result<_, Error>` 和 `Error::Mismatch`，只有明确未实现入口仍 panic。
- Go `NewChecker` 固定 `lower_case_table_names=2`；Rust 构造函数把该值交给调用方。
- Go `CreateTable`/`AlterTable` 会保存并恢复 warning/session variable 状态；Rust spec 不携带 session context，因此没有对应生命周期，不能认为已移植警告恢复语义。
- Go 当前覆盖更多物化视图入口及更完整的 DDL 接口；Rust `DdlCommand` 和公开方法集合明显更窄。
- Go `init` 把 `NewStorageDDLInjector` 注册到 `mockstore.DDLCheckerInjector`，`StorageDDLInjector` 组合多个存储接口并保存 `Injector`；Rust `init` 为空，泛型包装器只保存 storage。
- Go 的 `CreateMaskingPolicy` 会同时更新 tracker 并检查目标表；Rust 仅发送 `Noop`，属于显著的未完整移植行为。
- Go 集成测试通过 `mockstore.WithDDLChecker()` 或 `NewStorageDDLInjector` 将 checker 接入真实 DDL 流程，并在特定 AST 会被真实 DDL 原地修改时调用 `Disable`；Rust 独立测试只验证本文件的状态顺序、错误传播和 metadata 对比，尚未提供同等接线证据。

## 扩展指南

新增已建模 DDL 时，应同时完成以下局部闭环：

1. 在 `DdlCommand` 增加拥有式参数变体，并在 `execute` 中映射到准确的 `SchemaTracker` 方法。
2. 增加 `Checker` 公开入口，遵守“真实执行成功后才镜像”的默认顺序；若必须像 `DropTable` 一样例外，需用独立测试固定原因和状态结果。
3. 按影响对象调用 `checkDBInfo` 或 `checkTableInfo`；重命名/批量操作要检查全部旧、新名称，系统表例外不能意外扩大。
4. 若真实与 tracker 的 SHOW CREATE 存在已知、无语义差异，优先在 `normalize_show_create_table` 加窄范围归一化，并为“应删除”和“不应误删”各加测试，避免把真正漂移隐藏掉。
5. 扩展 `DdlExecutor` 时谨慎选择默认实现。生产必需能力应返回 `Error::Unsupported`，而不是无条件成功；同步更新测试替身。
6. 完成现有 panic 入口时，以 `checker.go` 和对应 `SchemaTracker` 实现为行为基线，但必须核对 Rust 类型和当前 Go 演进，不能直接假设一一等价。
7. 若要完成 Rust bootstrap 接线，应修改实际 domain/mockstore 适配层，而不是只扩充 `StorageDDLInjector<T>` 的外形；同时新增独立集成测试证明 `NewChecker` 确实位于 SQL DDL 调用链上。

测试必须继续放在独立的 `pkg/ddl/schematracker/checker_test.rs`，不要嵌入生产文件。至少覆盖真实执行失败、tracker 失败/漂移、禁用状态、对象存在性两种单边缺失、渲染差异、注释归一化和新增操作的更新顺序。涉及 tracker 内部 DDL 语义时，还应同步同目录 `dm_tracker_test.rs`；Go 对齐依据来自 `checker.go`、`dm_tracker_test.go` 和使用 checker 注入的 `pkg/ddl/db_integration_test.go`/`pkg/ddl/tests/fail/fail_db_test.go`。

兼容性风险主要是漏掉 Go 新增接口或误把占位成功当成真实执行；正确性风险主要是执行/镜像顺序导致双方永久漂移，以及过宽归一化掩盖差异；性能风险集中在每次 DDL 后两次元数据读取和两次 SHOW CREATE 渲染。扩展时不应把校验器引入常规生产热路径，除非上层明确接受该成本。

## 验证依据

- 源码与符号：`pkg/ddl/schematracker/checker.rs` 全部 577 行；重点符号为 `DdlCommand`、`DdlExecutor`、`Checker`、`NewChecker`、`execute`、`checkDBInfo`、`checkTableInfo`、归一化函数及 storage 包装器。
- crate 与模块边界：`pkg/ddl/schematracker/Cargo.toml`、`pkg/ddl/schematracker/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`，以及声明该 crate 的 `pkg/{ddl,domain,executor,session,testkit}` 相关 Cargo manifest。
- Rust 测试：`pkg/ddl/schematracker/checker_test.rs`，覆盖真实 schema 缺失、Disable 后 CreateTable 不镜像、DropTable/DropView 的错误顺序、直接成功入口不调用执行器，以及 `CreateSchemaWithInfo` 保留完整 metadata。
- Go 对照：`pkg/ddl/schematracker/checker.go`；Go tracker 行为测试位于 `pkg/ddl/schematracker/dm_tracker_test.go`，注入/端到端使用证据位于 `pkg/ddl/db_integration_test.go` 和 `pkg/ddl/tests/fail/fail_db_test.go`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件，目标目录 12 个文件均已覆盖；`files --filter pkg/ddl/schematracker` 确认目标 Rust、Go 对照和测试均在索引中；`node --file ...checker.rs --offset 1 --limit 500` 与 `--offset 495 --limit 120` 读取完整源码；`query NewChecker`、`query Checker`、`query checkTableInfo` 确认 Rust/Go 对应符号。精确 `callers checker.rs::NewChecker` 在当前索引持续无输出并挂起，已中止，故调用方结论由文件级 used-by 信息和源码引用搜索交叉限定，未宣称未证实的运行时接线。
- DDL 语境：`pkg/ddl/doc.go` 说明核心 DDL 的 schema-version 不变量；`docs/agents/ddl/README.md` 仅作为阅读入口，并由本文件/测试事实核验。该 checker 不推进 DDL job 或 schema state。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以固定 11 标题命令做结构验证，并人工核对文档能回答文件存在原因、运行顺序、边界和安全扩展点。
