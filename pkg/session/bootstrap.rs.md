# `pkg/session/bootstrap.rs`

## 文件定位

[`bootstrap.rs`](bootstrap.rs) 是 `astersql-session` crate 中对 TiDB 集群首次初始化与版本升级公共语义的 Rust 建模文件，由 [`lib.rs`](lib.rs) 通过 `pub mod bootstrap` 对外公开。它集中保存系统库/系统表清单、bootstrap 元数据变量名、运行时能力 trait，以及首次初始化、系统表增量创建、初始化 SQL 和若干升级辅助函数。

这个文件不是当前 Rust 服务端完整启动流程的唯一实现。生产态的具体 Domain/事务接线位于 [`runtime/session.rs`](runtime/session.rs) 的 `BootstrapCanonicalDomain` 及其辅助函数；其中 `bootstrap_canonical_nextgen_schemas` 已通过 `BootstrapSchemaRuntime` 调用本文件的 `bootstrapSchemas`，而本文件的通用 `bootstrap<R: BootstrapRuntime>` 尚未被生产 Rust 入口直接调用。理解或扩展本文件时必须区分“公共语义/可测试编排”与 `runtime/session.rs` 中“实际存储、Domain 和 SQL 执行接线”。

crate 边界由 [`Cargo.toml`](Cargo.toml) 声明：包名为 `astersql-session`，库入口为 `lib.rs`，`nextgen` feature 向 `astersql-config-deploymode` 和 `astersql-config-kerneltype` 透传；本文件直接使用 `astersql-meta-metadef` 提供的保留 ID 与建表 SQL。

## 核心职责

1. **定义持久化 bootstrap 协议。** `bootstrappedVar`、`tidbServerVersionVar`、`tidbSystemTZ`、`TidbNewCollationEnabled`、`tidbDDLTableVersion` 和 `tidbClusterID` 对应 `mysql.tidb` 中跨版本保存的键；`varTrue`/`varFalse` 固定其布尔文本编码。
2. **维护系统 schema 清单。** `BASE_TABLES` 等表数组引用 `astersql_meta_metadef` 的保留表 ID 和权威建表 SQL；`versionedBootstrapSchemas` 将这些定义按版本 1 至 4 分批，并用 `nextgen_only` 区分仅 next-gen 创建的存储级系统表。
3. **抽象 bootstrap 外部能力。** `BootstrapRuntime` 把 DDL Owner 判定、内部 SQL、事务提交、系统变量、锁、SQL 文件、密码哈希和分区映射等副作用交给调用方；`BootstrapSchemaRuntime` 是只含 next-gen schema 事务操作的窄接口。
4. **编排首次初始化与已初始化升级。** `bootstrap` 先初始化 MDL，循环检查标志；已初始化时调用 `upgrade`，未初始化且当前节点为 DDL Owner 时依次执行 `doDDLWorks`、`doDMLWorks`，其他节点每 200 ms 重试。
5. **提供可复用辅助逻辑。** 包括读取/写入 bootstrap 版本、补全全局变量、系统表约束检查、初始化 SQL 文件执行、旧密码转换和升级后的分区映射重建。

## 主要符号

- `BootstrapError<E>`：统一错误边界。`External(E)` 包装运行时错误；其余变体表达缺失结果集/变量、非法版本、非法十六进制密码和非法系统表。当前文件实际直接构造 `InvalidBootstrapVersion`、`InvalidPasswordHex`、`InvalidSystemTable`；`MissingRecordSet`、`MissingVariable` 作为接口预留，尚未由本文件流程返回。
- `SqlValue`：内部 SQL 参数的最小值集合，支持字符串、`i64` 和布尔值，避免编排层依赖具体 SQL executor 参数类型。
- `TableBasicInfo` / `DatabaseBasicInfo`：系统表和系统库的静态定义，分别携带保留 ID、名称、建表 SQL及所属表清单。
- `versionedBootstrapSchema`：按版本组织增量 schema；类型本身私有，但 `versionedBootstrapSchemas` 为公开常量切片。`systemDatabases` 指向基础 `mysql`/`sys` 定义。
- `SystemTableInfo`：约束检查所需的最小表属性投影，仅记录是否分区及是否使用分离自增。
- `BootstrapSchemaRuntime`：`bootstrapSchemas` 的事务级依赖；对任意 `T: BootstrapRuntime` 有 blanket implementation，便于完整运行时复用窄流程。
- `BootstrapRuntime`：完整 bootstrap 端口，共 28 个方法；关联类型 `Error` 负责下层错误，`LockGuard` 负责分布式锁生命周期。
- `bootstrap`：通用顶层编排函数。当前没有生产 Rust 调用者，不能将其描述为 `BootstrapCanonicalDomain` 的现行入口。
- `bootstrapSchemas`：版本化系统表创建器；当前由 `runtime/session.rs::bootstrap_canonical_nextgen_schemas` 在真实事务中调用。
- `doDDLWorks` / `doDMLWorks`：分别处理首次初始化的 schema/视图/资源组与 root 用户/全局变量/bootstrap 标志等持久化数据。
- `doBootstrapSQLFile`：读取、解析并顺序执行可选初始化 SQL；单条执行失败被忽略，读取或解析错误向上传播。
- `mustExecute`：统一以 `internalSQLTimeout`（75 秒）调用 `execute_internal`。
- `oldPasswordUpgrade`：把十六进制旧密码解码后交给运行时做 SHA1，再输出大写的 `*<40 hex>` 格式。
- `runBootstrapSQLFile`：全局 `AtomicBool`；`bootstrap` 成功做完 DDL/DML 后以 `SeqCst` 设为 `true`，测试可用 `DisableRunBootstrapSQLFileInTest` 清零。它不负责实际调用 `doBootstrapSQLFile`。

## 执行流程

通用 `bootstrap(runtime)` 的控制流如下：

1. 调用 `init_mdl_for_bootstrap` 建立 bootstrap 所需 MDL 状态。
2. 进入循环并调用 `checkBootstrapped`。后者先尝试切换到系统库；库不存在时返回 `false`，存在时通过 `getTiDBVar` 读取 `bootstrapped`。只有值严格等于 `"True"` 才视为已完成，并先提交检查所处事务。
3. 若已初始化，立即调用运行时 `upgrade` 并返回；升级的版本阶梯不在本文件，而在 `upgrade_def.rs`/`upgrade_run.rs` 与具体运行时中。
4. 若未初始化且 `is_ddl_owner` 为真，执行 `doDDLWorks`、`doDMLWorks`，随后发布 `runBootstrapSQLFile=true` 并返回。
5. 非 Owner 不执行写操作，只通过 `sleep(200ms)` 退避，然后重新检查持久化标志。

`doDDLWorks` 在 classic kernel 下创建 `mysql`、`sys`，遍历 `versionedBootstrapSchemas` 并跳过 `nextgen_only` 批次，再执行各表的 `create_sql`。两种 kernel 都继续插入内置 bind、创建 `mysql.tidb_mdl_view` 与 `sys.schema_unused_indexes`、创建 `test` 库，并把默认资源组的统计任务标记为后台任务。

`doDMLWorks` 先执行 `BEGIN`。安全 bootstrap 有用户名时创建 `localhost` 的 `auth_socket` root；否则创建 `%` 主机、空密码、`mysql_native_password` root。之后写入所有全局作用域系统变量，写 bootstrap 标志和当前版本，再依次委托运行时保存系统时区、新排序规则开关、语句摘要参数、DDL 表版本和集群 ID。提交失败时等待一秒并重新检查标志：若别的节点已完成则成功返回，否则返回原提交错误。

next-gen 的 `bootstrapSchemas` 独立工作：读取 `nextgen_schema_version`，跳过已应用版本；对每个新版本先确保系统库存在，再按保留 ID 建表并预分裂；全部完成后只在版本确有推进时写入最大版本。真实接线 `CanonicalBootstrapSchemaRuntime` 在事务元数据键 `BootTableVersion` 上读写版本，解析建表 AST、调用 `checkSystemTableConstraint`，并以保留 ID 直接创建 metadata；提交后 `bootstrap_canonical_nextgen_schemas` 触发 schema version diff、Domain reload 和统计 catalog 发布。

## 数据与状态

- **静态定义：** `BASE_TABLES` 包含基础 `mysql` 系统表；`MASKING_TABLES`、`STORAGE_CLASS_TABLES`、`MVIEW_TABLES` 分别对应版本 2、3、4 的增量。表 ID 与 SQL 都来自 `astersql-meta-metadef`，不可在此另造编号或复制一份漂移的 DDL。
- **版本状态：** `versionedBootstrapSchemas` 必须按版本严格递增；`bootstrapSchemas` 用当前版本作为下界并最终写最大已应用版本，保证重复执行时跳过已完成批次。
- **集群状态：** `mysql.tidb` 中 `bootstrapped=True` 是通用流程的完成判据，`tidb_server_version` 为升级版本来源。缺失版本按 0 处理，非十进制值返回 `InvalidBootstrapVersion`。
- **事务状态：** `checkBootstrapped` 只在确认已初始化后提交读事务；`doDMLWorks` 显式开始事务并把标志、版本和各参数作为一个初始化阶段写入。提交竞争通过“一秒等待后复查标志”化解。
- **进程状态：** `runBootstrapSQLFile` 是进程内原子开关，不是持久化完成标志；它采用 `SeqCst`，但不会替代 `mysql.tidb` 的集群一致性状态。
- **运行时记录：** `BootstrapRuntime` 的具体实现拥有连接、事务、锁 guard、SQL parser 或哈希实现；本文件不持有这些资源，也不缓存 Domain。

## 依赖与调用关系

上游与接线关系：

- `lib.rs` 公开 `bootstrap` 模块，并仅在 `cfg(test)` 下编译独立的 `bootstrap_test.rs`。
- `runtime/session.rs::bootstrap_canonical_nextgen_schemas` 构造 `CanonicalBootstrapSchemaRuntime`，调用 `bootstrapSchemas`，随后发布 schema diff、reload Domain 并同步统计 catalog；这是本文件已确认的生产调用边。
- `runtime/session.rs::CanonicalBootstrapSchemaRuntime::create_and_split_system_table` 调用 `checkSystemTableConstraint`，把 `TableBasicInfo.create_sql` 解析成 `CreateTableStmt`，构造 `TableInfo` 并按保留 ID 写事务元数据。
- `runtime/session.rs::BootstrapCanonicalDomain` 是现行 Rust Domain bootstrap 主链。它自行完成数据库/系统表创建、升级、root/全局变量与 `mysql.tidb` 写入，并没有调用通用 `bootstrap`、`doDDLWorks` 或 `doDMLWorks`；两条实现需保持 Go 语义一致，但不能视为同一调用栈。
- `upgrade_run.rs` 使用 `tidbDefMemoryQuotaQuery` 与 `tidbDefOOMAction` 等常量；多个 session 集成测试引用 `tidbClusterID`、`tidbDDLTableVersion` 和 `internalSQLTimeout`。

下游依赖：

- `astersql_meta_metadef` 提供 `SystemDatabaseID`、各系统表保留 ID 与 `Create*Table` SQL，是系统表清单的权威来源。
- `BootstrapRuntime` 的方法边界对应 Owner/锁、session 内部 SQL、系统变量注册、配置文件、密码摘要及 metadata 修复等外部子系统；本文件通过 trait 调用，不依赖这些实现类型。
- `mustExecute` 把所有内部 SQL 交给 `execute_internal(sql, args, internalSQLTimeout)`，因此 SQL 格式化、参数绑定、日志和取消行为属于运行时实现。

RustCodeGraph 的文件节点报告该文件被 16 个文件使用，并准确索引了 `bootstrap`、`checkBootstrapped`、`doDMLWorks`、`bootstrapSchemas` 等符号；但本次 `callers`/`callees` 子命令在这些节点上超时无输出，因此具体边由上述源码引用与模块接线交叉核验。

## 错误处理与边界

- 所有 `BootstrapRuntime`/`BootstrapSchemaRuntime` 错误通过 `From<E>` 转成 `BootstrapError::External`；编排层使用 `?` 保留首个失败点。
- `getTiDBVar` 将“变量不存在”规范化为 `(String::new(), true)`，所以调用者必须同时检查 `is_null`，不能仅以空字符串判断。与 Go 版不同，结果集的打开、读取和关闭已下沉到 `read_tidb_variable`；因此 `MissingRecordSet` 在当前实现中没有触发路径。
- `getBootstrapVersion` 对缺失值返回 0，对非法数字返回包含原文本的 `InvalidBootstrapVersion`。
- `checkSystemTableConstraint` 拒绝任何分区系统表，并拒绝 `AUTO_ID_CACHE=1` 所代表的分离自增；两个条件按该顺序检查。
- `doBootstrapSQLFile` 没有文件时成功返回；读文件或解析失败终止流程；已成功解析后，单条语句失败被有意忽略并继续执行后续语句。trait 边界没有 logger，因此诊断必须由 `execute_statement` 实现承担。
- `oldPasswordUpgrade` 要求偶数长度且每对字符均为合法十六进制；否则返回 `InvalidPasswordHex`。它只做一次由运行时提供的 SHA1，并把结果格式化为大写字符串。
- `doDMLWorks` 对 commit 失败有竞争恢复分支，但在 commit 失败后的 `checkBootstrapped` 自身报错时，该复查错误会通过 `?` 返回，可能遮蔽原 commit 错误；扩展错误类型时应保持或明确改变这一优先级。
- `doDDLWorks`/`doDMLWorks` 使用的部分 SQL 是语义占位字符串（例如 `"UPSERT mysql.tidb bootstrap flag"`），它们依赖运行时解释或测试记录；现行生产 `BootstrapCanonicalDomain` 使用具体 SQL。新增生产调用前必须确认这些字符串可由真实 executor 执行。

## 并发与资源生命周期

`bootstrap` 采用“DDL Owner 单写、其他节点轮询”的集群并发模型。Owner 身份由 `is_ddl_owner` 提供；非 Owner 每 200 ms 重查持久化标志。通用函数 `acquireLock` 可用固定键 `/tidb/distributeDDLOwnerLock/` 获取 `LockGuard`，但 `bootstrap` 本身没有调用它，guard 的释放依赖具体类型的生命周期。现行生产锁接线可见 `runtime/session.rs::BootstrapOwnerLock`，其 `Drop` 中异步释放分布式锁。

`bootstrapSchemas` 假定调用者提供事务作用域。`CanonicalBootstrapSchemaRuntime` 借用 `&mut dyn kv::Transaction`，所有版本读取、建库建表和版本推进都在该借用期内完成；外层 `kv::RunInNewTxn` 负责提交或回滚。`changed` 只用于决定是否生成 schema version diff，不是并发锁。

`doDMLWorks` 的事务从 `BEGIN` 到 `commit_transaction`；提交失败后主动等待，为另一 Owner 的成功提交留出可见窗口。`doBootstrapSQLFile` 顺序处理语句，不创建任务或并行执行。文件内没有线程、异步任务或 channel。

唯一共享内存状态 `runBootstrapSQLFile` 是 `AtomicBool`，读写采用 `Ordering::SeqCst`。`DisableRunBootstrapSQLFileInTest` 无条件清零，与 Go 版仅在 `intest.InTest` 时清零有差异；调用者应把它限制在测试控制流中。

## 与 Go 版本的对应关系

直接对照文件是 [`bootstrap.go`](bootstrap.go)，独立测试是 [`bootstrap_test.rs`](bootstrap_test.rs) 与 Go 的 [`bootstrap_test.go`](bootstrap_test.go)。Rust 保留了 Go 中同名的 `bootstrap`、`checkBootstrapped`、`getTiDBVar`、`acquireLock`、`bootstrapSchemas`、`doDDLWorks`、`doDMLWorks`、`doBootstrapSQLFile`、`oldPasswordUpgrade` 等概念，并复用相同的系统表保留 ID、schema 版本批次和关键错误文本。

主要对应与差异如下：

- Go `bootstrap(s sessionapi.Session)` 直接访问 DDL Owner、session 和升级函数；Rust 将这些行为注入 `BootstrapRuntime`，使编排可被 recorder 测试，但完整 trait 目前没有生产实现。
- Go `bootstrapSchemas(store)` 自行打开新事务并操作 meta mutator；Rust `bootstrapSchemas` 只实现版本算法，事务创建与具体 meta 操作由 `CanonicalBootstrapSchemaRuntime` 负责。
- Go `getTiDBVar` 直接管理 `RecordSet` 和 `Close`；Rust 把查询生命周期收敛到 `read_tidb_variable`，所以 `BootstrapError::MissingRecordSet` 尚未实际使用。
- Go `acquireLock` 创建/关闭 etcd client 并返回释放闭包；Rust 只向运行时索取 `LockGuard`，资源释放契约由 guard 实现决定。
- Go `internalSQLTimeout` 等于 `owner.ManagerSessionTTL + 15`；Rust 当前固定为 75 秒。若 Owner TTL 配置或常量变化，二者可能漂移。
- Go 的 `DisableRunBootstrapSQLFileInTest` 检查 `intest.InTest`；Rust 版本直接执行原子写。
- Go `doBootstrapSQLFile` 在非测试环境对读/解析错误会 fatal，在测试中返回错误，并记录单条执行错误；Rust 统一返回读/解析错误，忽略单条执行错误且要求运行时记录诊断。
- Go `oldPasswordUpgrade` 通过 `hex.DecodeString` 后做 SHA1；Rust 的 `decode_hex` 与运行时 `sha1` 保留相同输入输出形状。
- 现行 Rust `BootstrapCanonicalDomain` 复刻了大量 Go `doDDLWorks`/`doDMLWorks` 行为，但它是相邻文件中的独立生产实现。新增逻辑时应同步审查两处，直到通用 trait 编排真正成为生产入口。

测试证据也必须分层看待：`bootstrap_test.rs` 前半保留许多来自 Go 测试的草稿 helper，其中部分是空实现，不能作为生产行为证明；文件后部的 `BootstrapRuntimeRecorder` 测试真实调用了 `oldPasswordUpgrade`、`doDMLWorks` 和约束函数，`nextgen_production_bootstrap_preserves_reserved_schema_ids_and_version` 则通过真实 mock KV/Domain 验证 next-gen 保留 ID、版本 4 与重复执行幂等性。`pkg/session/test/session_test.rs::bootstrap_sql_file_empty_parse_and_statement_errors_match_go` 真实覆盖初始化 SQL 的空输入、解析失败与单条执行失败继续行为。

## 扩展指南

- **新增系统表：** 先在 `astersql-meta-metadef` 增加唯一保留 ID 与权威建表 SQL，再把 `TableBasicInfo` 放入正确的新版本批次；保持 `versionedBootstrapSchemas` 版本递增，并确认 classic 是否应跳过 `nextgen_only`。同时更新 `bootstrap_test.rs::TestMySQLDBTables`、`TestVersionedBootstrapSchemas` 和 next-gen 真实 mock KV 测试。不要只改 `BootstrapCanonicalDomain` 的平铺清单。
- **新增持久化 bootstrap 参数：** 在相应常量、`BootstrapRuntime` 能力和 `doDMLWorks` 写入顺序中接线，同时同步 `runtime/session.rs::BootstrapCanonicalDomain` 的具体 SQL与 Go `bootstrap.go`。考虑旧集群的 upgrade 路径，而不只是首次初始化。
- **扩展初始化流程：** 若动作属于 schema，优先放入 `doDDLWorks` 或版本化 `bootstrapSchemas`；若属于数据/变量，放入 `doDMLWorks` 并保持 bootstrap 标志最后可见的事务语义。涉及外部资源时通过 trait 方法注入，并明确 guard/事务/文件句柄的释放责任。
- **调整系统表约束：** 同时修改 `checkSystemTableConstraint`、`CanonicalBootstrapSchemaRuntime::create_and_split_system_table` 的属性投影及 `TestCheckSystemTableConstraint`/`canonical_system_table_constraints_cover_all_rejection_branches`。测试必须继续放在独立 `bootstrap_test.rs`，不得嵌入生产文件。
- **修改初始化 SQL：** 保持“读/解析失败终止、单条执行失败继续”的 Go 控制流，或明确记录兼容性变化；同步 `pkg/session/test/session_test.rs` 的真实 recorder 测试。
- **把通用 `bootstrap` 接入生产：** 接线前必须实现完整 `BootstrapRuntime`，替换占位 SQL，证明 Owner/锁、事务、升级、Domain reload 与错误日志语义等价，并避免与 `BootstrapCanonicalDomain` 双重执行。当前不能假设该函数已经生产可用。
- **性能风险：** 系统表数量增加会放大首次 bootstrap 的串行 SQL/metadata 工作；轮询间隔、内部 SQL 75 秒超时和每表预分裂都可能影响启动时间。应使用聚焦的 bootstrap/next-gen 测试观察，而不是删减 Go 流程。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/session/bootstrap.rs --offset 1 --limit 1000` 读取了完整 897 行并报告 16 个使用文件；`query` 确认 `bootstrap`、`checkBootstrapped`、`doDMLWorks`、`bootstrapSchemas`、`oldPasswordUpgrade` 等符号及行号。`callers`/`callees` 对精确节点在本次运行中超时且无输出，调用边因此用精确源码引用补证。
- 生产源码：`pkg/session/bootstrap.rs`（完整文件）；`pkg/session/lib.rs`（模块公开与独立测试模块）；`pkg/session/runtime/session.rs`（`CanonicalBootstrapSchemaRuntime`、`bootstrap_canonical_nextgen_schemas`、`BootstrapCanonicalDomain`、`BootstrapOwnerLock`）；`pkg/session/upgrade_run.rs`（常量消费者）。
- crate 配置：`pkg/session/Cargo.toml`（`astersql-session`、`lib.rs`、`nextgen` feature、`astersql-meta-metadef` 依赖与 `go-package = "pkg/session"` 移植元数据）。
- Go 对照：`pkg/session/bootstrap.go`（同名常量、schema 版本清单、主流程、锁、DDL/DML、SQL 文件和旧密码逻辑）；`pkg/session/bootstrap_test.go`（原始回归意图）。
- Rust 测试：`pkg/session/bootstrap_test.rs`（系统表清单/版本、旧密码、约束、root SQL 形状、next-gen 真实存储幂等性）；`pkg/session/test/session_test.rs`（`doBootstrapSQLFile` 错误与继续执行语义）。这些文件只作为证据读取，本任务未运行 Cargo。
- 人工核对结论：该文件存在于 session bootstrap 的共享协议层；已生产接线的是版本化 next-gen schema 与约束检查，完整通用 `bootstrap` 仍是未接线抽象；安全扩展必须同步权威 metadef、Go 对照、现行 `BootstrapCanonicalDomain` 和独立 Rust 测试。
