# `pkg/session/upgrade_run.rs`

源文件：[`upgrade_run.rs`](./upgrade_run.rs)。本文只描述当前 Rust 代码及其直接接线，不把 Go 已有而 Rust 尚未接入的完整升级链当成 Rust 现状。

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准。

## 文件定位

本文件属于 `astersql-session` crate；`pkg/session/Cargo.toml` 以 `lib.rs` 为 crate 根，`lib.rs` 公开声明 `pub mod upgrade_run`。它位于已 bootstrap 集群再次启动时的版本迁移边界，包含三组职责：抽象完整升级循环的 `UpgradeRuntime`/`upgrade`，规划历史系统变量回填的 `BootstrapVariableUpgradeRuntime`/`upgrade_bootstrap_variables`，以及规划 v282 bind digest 刷新的数据类型与 `plan_binding_digest_refresh`。

当前生产接线并不完全等同于上述三组定义。仓库搜索只发现 `UpgradeRuntime` 在 `upgrade_run_test.rs` 中有实现，未发现生产实现或对本文件 `upgrade` 的生产调用；canonical bootstrap 路径则在 `runtime/session.rs::upgrade_canonical_domain` 中直接调用 `upgrade_bootstrap_variables` 和 `refresh_canonical_binding_digests`，后者再调用 `plan_binding_digest_refresh`。因此，`upgrade` 是已实现并可测试的通用编排边界，但目前不是 canonical Rust 启动路径的实际入口。

## 核心职责

- `upgrade` 按持久化 bootstrap 版本选择升级步骤，协调 MDL v99 前后钩子、版本写回、提交竞争后的二次确认。
- `upgrade_bootstrap_variables` 把 v54、v59、v68、v80、v81、v97、v105、v135、v215、v255、v279、v281、v283、v284、v317、v318、v319 中与持久化变量有关的迁移压缩为可注入存储接口；各分支均以 `from < versionN` 为门槛，且“缺失时插入”分支保留用户已有值。
- `plan_binding_digest_refresh` 将 v282 的 bind 归一化、非法 digest 清理、重复项淘汰和有效项更新拆成无数据库副作用的确定性规划；实际 SQL 写入由 `runtime/session.rs::refresh_canonical_binding_digests` 完成。
- `InitMDLVariableForUpgrade` 和 `printClusterState` 分别封装升级前 MDL 状态初始化、支持 HTTP 升级状态版本之后的集群状态检查。

## 主要符号

- `VersionedUpgrade { version, name }`：一个目标版本及其可诊断名称。它不保存函数指针；真正执行交给 `UpgradeRuntime::execute_upgrade`。
- `UpgradeRuntime`：完整升级循环的依赖注入接口，包括持久化版本、MDL、集群状态检查、步骤枚举与执行、v99 钩子、提交和休眠。关联类型 `Error` 贯穿所有可失败操作。
- `BootstrapVariableUpgradeRuntime`：变量迁移的最小持久化接口。`insert_global_if_missing`、条件删除/更新、`upsert_tidb_variable` 和原子迁移旧 txn-file 开关分别表达不同幂等语义。
- `upgrade_bootstrap_variables<R>(runtime, from)`：按旧版本运行变量回填；返回第一个底层错误，不自行提交或重试。
- `BindingDigestRefreshRow`：规划所需的持久化 bind 投影；调用者必须按“最新优先”提供行。`identity` 是交还给执行层定位记录的标识。
- `BindingDigestRefreshAction`：三种有序动作：清空非法行的 `plan_digest`、删除重复行、更新 `original_sql/sql_digest`。
- `plan_binding_digest_refresh(rows)`：使用 `astersql_bindinfo::NormalizeStmtForBinding` 计算新 digest，并用 `(sql_digest, plan_digest)` 去重。
- `upgrade<R>(runtime)`：通用升级主循环。
- `InitMDLVariableForUpgrade<R>(runtime)`：读取并应用 MDL 设置，返回原持久化值是否为 NULL。
- `printClusterState<R>(runtime, version)`：达到 `support_upgrade_http_version` 后调用运行时检查。

文件没有模块级常量、条件编译项或内部 `impl`；所有核心类型、trait 和函数均为 `pub`。`#![allow(dead_code, non_snake_case)]` 同时容纳当前未接线入口和 Go 风格符号名。

## 执行流程

新增分支在 v317 之后依次执行：`from < version318` 时以 `insert_global_if_missing` 写入 `TiDBOptRangeMaxCount` 的默认字符串；`from < version319` 时先 `add_analyze_options_ndv_rate()`，成功后再缺失插入 `TiDBAnalyzeSampledNDVThreshold` 默认值。冻结测试固定 range 默认 1000、sampled NDV 阈值 0，保留已有 0/2000 或 12。canonical trait 实现执行 `ALTER TABLE mysql.analyze_options ADD COLUMN IF NOT EXISTS ndv_rate DOUBLE NOT NULL DEFAULT -1`，因此 trait 已含 DDL 能力，函数不再仅处理变量。任一操作失败立即返回，不自行回滚之前已完成的版本步骤。v320 restore registry schema 不在本函数中迁移。

`upgrade` 的顺序是：先由 `InitMDLVariableForUpgrade` 读取 MDL 并设置进程内开关；再读取旧版本 `from` 和目标版本 `target`；若 `from >= target` 立即成功返回。否则按版本阈值检查集群升级状态；原 MDL 值为 NULL 时先运行 v99 before；遍历 `upgrade_functions()`，只执行 `item.version > from` 的步骤；必要时运行 v99 after；写入目标 bootstrap 版本并提交。若提交失败，休眠一秒后重读版本：其他节点已写到 `target` 或更高即视为成功，否则返回原提交错误。

`upgrade_bootstrap_variables` 按源码顺序执行所有高于 `from` 的适用迁移。v54 还要求 `from <= version38`；v54/v59 写 `mysql.tidb` 兼容值，v68 条件删除旧 clustered-index 值，v255 只把 analyze version 的 `1` 改为 `2`，v284 委托运行时原子读取旧反向开关并替换新开关，其余分支主要是缺失时插入。任一步失败立即以 `?` 停止，后续版本不再规划。

`plan_binding_digest_refresh` 逐行跳过 `source == "builtin"`；对其余 bind 调用归一化。归一化结果为空时，仅在原来存在 `plan_digest` 时生成清空动作；有效且带 `plan_digest` 的行按 `(新 sql_digest, plan_digest)` 保留第一行、把后续行标为重复；有效行生成更新动作。返回顺序固定为“清空非法项、删除重复项、更新有效项”，而不是输入顺序，这避免有效项更新先撞上旧唯一键。

## 数据与状态

完整升级流程的持久状态由 `UpgradeRuntime` 持有，本文件不直接持有 session、事务或全局变量。关键不变量是：整个步骤选择始终使用启动时读到的同一个 `from`；步骤表顺序由运行时负责；只有全部步骤与 v99 after 成功后才请求更新 bootstrap 版本。

变量迁移把 SQL 存储细节隔离在 `BootstrapVariableUpgradeRuntime`。canonical 实现 `runtime/session.rs::CanonicalBootstrapVariableRuntime` 对普通变量使用 `INSERT IGNORE`、条件 `DELETE/UPDATE`，对 v284 使用 `BEGIN PESSIMISTIC`、`SELECT ... FOR UPDATE`、`REPLACE` 和 `COMMIT/ROLLBACK`。这保证缺失插入幂等，并使旧/新 txn-file 开关替换成为事务单元。

digest 规划只在函数栈上维护 `HashSet<(String, String)>` 及三个动作向量，无全局缓存。空间复杂度随非 builtin 输入行数线性增长；`String` 被动作接管，避免额外的跨线程共享状态。

## 依赖与调用关系

crate 边界由 `pkg/session/Cargo.toml` 的 `name = "astersql-session"`、`[lib] path = "lib.rs"` 和 `lib.rs::pub mod upgrade_run` 确认。直接外部依赖是 `astersql-bindinfo`（bind 归一化）与 `astersql-sessionctx-vardef`（变量名和默认值）；版本阈值和旧兼容变量名分别来自同 crate 的 `upgrade_def`、`bootstrap`。

已核实的生产调用链为 `runtime/session.rs` 的 canonical bootstrap 流程 → `upgrade_canonical_domain` → `upgrade_bootstrap_variables`；旧版本小于 v282 时还会进入 `refresh_canonical_binding_digests` → `plan_binding_digest_refresh` → `astersql_bindinfo::NormalizeStmtForBinding`，随后由 session 执行对应 SQL。`bootstrap.rs::bootstrap` 定义的另一条抽象链是已 bootstrap → `BootstrapRuntime::upgrade`，但仓库搜索没有把该 trait 方法静态连到本文件 `upgrade` 的证据。

上述生产调用边以冻结 `runtime/session.rs` 方法体与符号搜索为依据；工作区图索引不代表该冻结提交，也不能以缺少静态边否定动态或 trait 分派。

## 错误处理与边界

`upgrade` 与 `InitMDLVariableForUpgrade` 用 `Result` 保留运行时错误；这与 Go 对照中多处 `Fatal`/`terror.MustNil` 的进程终止语义不同。MDL 读取失败时仍先调用 `set_metadata_lock_enabled(false)` 再返回错误，匹配 Go 中 `enable` 默认 false 且读取失败后仍应用的行为。`printClusterState` 的 trait 方法无返回值，因此本层无法传播检查错误。

提交失败的容错仅覆盖“另一节点已经完成升级”：二次读版本失败会覆盖不了、而是通过 `?` 返回读错误；版本仍旧时返回最初的提交错误。升级步骤本身没有回滚编排，事务界限由具体运行时负责。

变量迁移依赖运行时实现幂等和原子语义；本文件不会验证名称、值或数据库结果。digest 规划将 builtin 行完全排除；无效归一化且原来没有 `plan_digest` 的行不产生动作。canonical 执行层若查询无结果集、行列数不为四、扫描或写入失败，返回 `SessionError`。

一个重要兼容边界是 v282 身份与排序：Go `upgrade_def.go::upgradeToVer282` 使用 `_tidb_rowid` 并按 `update_time/create_time/_tidb_rowid` 降序；当前 Rust canonical 接线以 `bind_sql` 作为 `identity`，查询末级排序也是 `bind_sql DESC`。因此规划器保证“输入第一项获胜”，但执行层当前并非 Go 的逐物理行定位；修改这部分时应先补齐等价性证据和回归测试。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel 或后台资源，所有函数同步执行且要求调用者持有 `&mut runtime`，从类型层面阻止同一运行时被这些函数并发可变访问。

集群级并发主要体现在 `upgrade` 的提交竞争恢复：节点提交失败后等待一秒，再观察是否已有节点把版本推进到目标。它不提供节点间锁，步骤的重入安全由升级实现保证。canonical v284 的行锁与事务生命周期位于 `CanonicalBootstrapVariableRuntime::migrate_legacy_txn_file_variable`；digest 查询结果集由 `runtime/session.rs` 消费至结束，规划器本身只接收已物化行。

## 与 Go 版本的对应关系

`pkg/session/upgrade_run.go` 的 `upgrade`、`InitMDLVariableForUpgrade`、`printClusterState` 是通用 Rust 三个函数的直接语义来源：早退、v99 钩子条件、逐版本选择、更新后提交以及提交竞争复查顺序一致。Rust 用 trait 和 `Result` 代替具体 `sessionapi.Session`、全局 vardef 设置及 Go 的 fatal 日志，并将日志职责留给运行时。

变量分支对应 `pkg/session/upgrade_def.go` 中同版本 `upgradeToVerN` 的局部行为，不代表 Rust 已移植这些版本的其他 DDL：例如 Go v317 还修改权限列，Rust 本函数只负责 adaptive-limit 变量。v284 的 Go 路径仅对 next-gen 执行，而 Rust helper 自身没有 kernel gate；是否调用由上游路径与运行时决定。

v282 Rust 规划复现 Go 的 builtin 排除、重新归一化、非法 plan digest 清理、按 digest pair 保留最新行、先清冲突后更新的意图。差异包括上一节所述的物理行身份/末级排序，以及 Rust `BindingDigestRefreshRow` 未携带 Go 查询中的 charset/collation；`NormalizeStmtForBinding` 的 Rust 接口只接收 statement、default DB 和布尔标志。相关 Go 行为由 `upgrade_backfill_test.go::TestUpgradeToVer282RefreshesBindingDigest` 覆盖。

## 扩展指南

`BootstrapVariableUpgradeRuntime::add_analyze_options_ndv_rate` 是必需方法，扩展实现必须保留幂等增列和先列后变量的次序；独立测试 `range_count_threshold_upgrade_backfills_default_without_overwriting`、`sampled_ndv_upgrade_adds_column_and_defaults_upgraded_clusters_to_disabled` 覆盖 317/318/319 版本边界及保值行为。

新增变量型版本迁移时，应先在 `upgrade_def.rs` 定义/确认版本常量，再在 `upgrade_bootstrap_variables` 添加严格的 `from < version` 分支，并选择与 Go 相同的“缺失插入、条件改写、强制 upsert 或事务迁移”接口；同步扩展独立的 `upgrade_backfill_test.rs`，不要把测试嵌入本源文件。若迁移超出当前 trait 能力，优先增加表达原子语义的方法，而不是把 SQL 字符串泄漏进规划函数。

扩展完整升级循环时，应为 `UpgradeRuntime` 提供真实生产实现或明确接入 canonical bootstrap，并为 `upgrade` 增加早退、步骤筛选、v99 顺序、步骤错误、提交竞争成功/失败的独立测试；当前 `upgrade_run_test.rs` 只覆盖 MDL 初始化。步骤表必须保持升序，否则 `upgrade` 不会自行排序。

修改 v282 时，应同时检查 `BindingDigestRefreshRow`、动作顺序和 `runtime/session.rs::refresh_canonical_binding_digests` 的 SQL。若追求 Go 严格等价，需评估引入稳定 row id、charset/collation 和 `_tidb_rowid` 排序，并扩展 `upgrade_backfill_test.rs` 及 Go 对照测试覆盖同 digest/plan digest、非法行、builtin、并列时间与 SQL 中引号。主要风险是误删用户 binding、唯一键冲突、覆盖用户变量、启动升级不可重入，以及一次性物化大量 bind 带来的内存开销。

## 验证依据

- 冻结依据：`ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`；以 `git diff` 定位本文件增量，以 `git show 2c944fb:pkg/session/upgrade_run.rs` 和相邻接线/测试 blob 复核。未以工作区未提交 Rust 变化作为事实来源。
- 接线与对照：冻结的 `pkg/session/Cargo.toml`、`pkg/session/lib.rs`、`pkg/session/runtime/session.rs`、同名 Go 文件；`pkg/session/doc.go` 不存在。
- RustCodeGraph `status` 当前包含 Rust，但索引描述工作区而非冻结提交；本次行为与调用边以冻结 blob 为准，不继承旧文档中的历史图统计或调用覆盖结论。
- 纯文档 Ready 等价检查：目标八篇的 11 个二级章节、局部链接、尾随空白及 `git diff --check`。仓库 Ready skill 路径缺失，按计划用文档检查；不运行 Cargo、Go 测试或构建，测试证据为源码审阅。
- `upgrade_backfill_test.rs` 中 range-count 与 sampled-NDV 用例验证默认值、保值与版本边界；`runtime/session.rs::CanonicalBootstrapVariableRuntime` 实现 ndv_rate 幂等 DDL；Go `upgrade_def.go::upgradeToVer318/319` 提供对应语义。v320 与完整升级循环不由这两个变量测试证明。
