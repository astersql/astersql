# `pkg/session/upgrade_def.rs`

## 文件定位

`upgrade_def.rs` 是 `astersql-session` crate 的 bootstrap 升级定义层，由 `pkg/session/lib.rs` 以公开模块 `upgrade_def` 装配。它集中保存历史 bootstrap 版本号、版本到升级函数的映射、少量与历史升级相关的数据结构，以及一组按函数名转发的升级桩。

这个文件不是 Rust 版本的升级 DDL/DML 实现主体。每个 `upgradeToVerN` 只把自己的名字交给 `upgrade_action`；真正动作必须由 `installUpgradeAction` 预先安装的函数指针完成。仓库搜索显示安装入口当前只在 `pkg/session/upgrade_test.rs` 中使用，生产 Rust 路径尚未安装该执行器，也没有消费 `upgradeToVerFunctions`。与此同时，`currentBootstrapVersion` 和若干 `versionN` 常量已经由 `pkg/session/runtime/session.rs`、`pkg/session/upgrade_run.rs` 等生产代码读取，因此本文件同时包含“已接线的版本契约”和“尚未接线的逐版本分发门面”。

crate 边界由 `pkg/session/Cargo.toml` 的包名 `astersql-session` 与 `[lib] path = "lib.rs"` 确认；本文件本身只直接依赖标准库的 `LazyLock`、`OnceLock`，没有条件编译项。

## 核心职责

1. 用 `version2` 至 `version317` 的离散常量表达可识别的历史 bootstrap 版本；缺号不是连续性错误，而是未设独立迁移、已由后续版本重做或为特定发行版保留的版本段。
2. 用 `currentBootstrapVersion = version317` 声明当前目标版本。它保留为可变静态量以对应 Go 测试可临时修改目标版本的设计，但 Rust 读取处必须使用 `unsafe`。
3. 以 `upgradeToVerFunctions: LazyLock<Vec<VersionedUpgradeFunction>>` 构建严格递增的迁移表。表项把目标版本与同名 `upgradeToVerN` 函数指针对齐。
4. 通过 `installUpgradeAction`、`UPGRADE_ACTION` 和 `upgrade_action` 提供一次性、按名称的执行器边界，使大量升级桩不直接依赖完整 Session/SQL 实现。
5. 保存 `bindInfo`、`bindingDigestUpdate`、`bindingDigestPair` 等 Go 迁移结构的 Rust 形状，供移植对照；当前仓库搜索未发现它们在本文件外被生产代码使用。

## 主要符号

- `version2` … `version317: i64`：历史目标版本常量。显式保留的两个版本空洞是 257–276 与 286–315；迁移表中还按 Go 规则跳过若干早期版本，例如 39、48、49、51、58、61、92、96、99、145。
- `VersionedUpgradeFunction { version, function }`：公开表项，`function` 的类型为 `fn(&sessionapi::Session, i64)`；小写别名 `versionedUpgradeFunction` 用于贴近 Go 命名。
- `bindInfo`：保存 binding SQL、状态、创建时间、字符集、排序规则和来源。Rust 将 Go 的 `types.Time` 简化为 `String`，说明它目前是兼容形状而非等价的运行时数据模型。
- `bindingDigestUpdate`：记录 v282 digest 刷新需要的行 ID、规范化 SQL、新 digest 与重复标记。
- `bindingDigestPair`：以 SQL digest 和 plan digest 组成重复检测键。
- `currentBootstrapVersion`：当前目标为 317。`pkg/session/runtime/session.rs` 用它判断、写入和完成 bootstrap；`upgrade_def_test.rs` 与 `upgrade_test.rs` 校验它等于迁移表最后一项。
- `upgradeToVerFunctions`：惰性初始化的 176 项有序向量。内部两个宏把数字同时映射到版本值和同名函数，减少手写表项的字段重复，但仍要求宏分支和最终数字清单同步维护。
- `upgradeToVer2` … `upgradeToVer317`：逐版本公开桩；参数当前均不读取，只调用 `upgrade_action("upgradeToVerN")`。此外还有 `doReentrantDDL`、`writeSystemTZ`、`writeNewCollationParameter`、`writeDefaultExprPushDownBlacklist`、`writeStmtSummaryVars`、`insertBuiltinBindInfoRow`、`updateBindInfo`、`writeMemoryQuotaQuery`、`importConfigOption`、`upgradeToVer99Before/After`、`writeDDLTableVersion`、`writeClusterID` 等同类辅助桩。
- `installUpgradeAction(handler)`：把 `fn(&str)` 写入全局 `OnceLock`；首次成功返回 `Ok(())`，重复安装返回原 handler 的 `Err`。
- `upgrade_action(name)`：读取已安装 handler 并调用；若没有安装，以包含迁移名的消息 panic，禁止静默略过迁移。
- 私有 `sessionapi::Session`：零字段占位类型，隔离定义文件与完整 session API。它不是 `astersql-session-sessionapi` 中的真实 Session trait/类型。

## 执行流程

定义层的单次分发流程如下：

1. 调用方从 `upgradeToVerFunctions` 取得按版本升序排列的 `VersionedUpgradeFunction`。
2. 上层应只选择 `version` 大于集群旧 bootstrap 版本的表项；Go 的 `upgrade_run.go` 和 Rust 的通用 `upgrade_run::upgrade` 都遵循这一筛选语义，但当前 Rust 通用运行时并未直接消费本文件的函数表。
3. 调用表项的 `function(session, old_version)`。当前桩忽略两个参数，仅把静态函数名传给 `upgrade_action`。
4. `upgrade_action` 从 `UPGRADE_ACTION` 取出一次性安装的 `fn(&str)` 并执行。未安装时立刻 panic；handler 的返回类型为 `()`，所以该边界没有可传播的业务错误。

完整 Rust bootstrap 中已经存在另一条局部接线：`pkg/session/runtime/session.rs` 调用 `upgrade_run::upgrade_bootstrap_variables`，后者读取本文件的版本常量并按 `from < versionN` 回填部分系统变量。v282 binding digest 刷新也由 `upgrade_run::plan_binding_digest_refresh` 提供独立算法。这些路径复用了本文件的版本契约，但没有经过 `upgradeToVerN` 分发桩。

## 数据与状态

- 版本常量是编译期不可变 `i64`；它们代表存储在集群 bootstrap 元数据中的步进值，而不是软件语义版本。
- `currentBootstrapVersion` 是 `static mut`，全进程共享且无同步保护。现有生产代码主要读取它，测试也注明只读；若未来写入，调用方必须自行保证无数据竞争和生命周期安全。
- `upgradeToVerFunctions` 由 `LazyLock` 在首次访问时构造一次，之后共享只读向量。关键不变量是版本严格递增、函数非空、函数名与版本一致、最后版本等于 `currentBootstrapVersion`。
- `UPGRADE_ACTION` 是 `OnceLock<fn(&str)>`：进程内最多成功安装一次，安装后不可替换或清除。这适合全局执行器，但会使测试顺序和多运行时配置受到单例约束。
- 三个 binding 结构都拥有其中的字符串，无借用、锁或外部资源；当前仅起移植形状和未来接线提示作用。

## 依赖与调用关系

- 上游模块装配：`pkg/session/lib.rs` 公开声明 `pub mod upgrade_def`，并在测试配置下独立装配 `upgrade_def_test.rs` 与 `upgrade_test.rs`。
- 已确认的生产消费者：`pkg/session/upgrade_run.rs` 导入多个 `versionN` 常量；`pkg/session/runtime/session.rs` 读取 `currentBootstrapVersion`、`version280`、`version282` 等来驱动 bootstrap 与兼容迁移；其他 session 运行时测试也读取当前版本。
- 分发门面的当前消费者：仓库 `rg` 结果中，`installUpgradeAction` 只由 `pkg/session/upgrade_test.rs` 调用，`upgradeToVerFunctions` 只由两个 Rust 测试调用；因此不能声称逐版本桩已接入生产 bootstrap。
- 下游依赖：桩函数只依赖私有占位 `sessionapi::Session` 和 `upgrade_action`；`upgrade_action` 只依赖 `UPGRADE_ACTION` 中的函数指针。
- 相邻实际逻辑：`pkg/session/upgrade_run.rs` 定义升级循环、MDL 前后处理、提交冲突恢复、系统变量迁移及 v282 digest 规划；`pkg/session/runtime/session.rs` 提供当前 Rust bootstrap 的持久化运行时实现。
- Go 对照：`pkg/session/upgrade_def.go` 直接包含版本表和各版本真实 DDL/DML；`pkg/session/upgrade_run.go` 负责根据旧版本顺序运行它们。

## 错误处理与边界

- `installUpgradeAction` 不 panic：重复安装通过 `Result<(), fn(&str)>` 暴露失败及未写入的 handler。调用方必须处理该结果。
- `upgrade_action` 对“执行器未安装”采用 fail-fast panic，错误文本包含具体迁移名。这保证调用桩时不会把未执行迁移误报为成功，但也意味着它不适合需要普通 `Result` 恢复的生产边界。
- handler 类型不返回 `Result`，定义层无法表达 SQL、事务或 DDL 错误；错误策略必须由 handler 内部完成。当前没有生产 handler，故这部分行为尚未验证。
- 升级桩忽略 `Session` 与旧版本参数，不能自行执行版本条件、幂等 DDL、事务回滚或内核类型分支。Go 中这些边界存在于具体函数体，例如 v284 的 Classic 短路与悲观事务、v285 的 `ADD COLUMN IF NOT EXISTS`、v317 的重复列容错；Rust 桩本身不具备这些语义。
- 私有占位 Session 使外部 crate 无法自然构造合法引用。测试因该类型为零大小、可构造且指针对齐，使用非空悬空指针引用调用桩；这只是测试技术，不应成为生产调用方式。

## 并发与资源生命周期

- `LazyLock` 和 `OnceLock` 的初始化由标准库同步原语保护，可安全并发读取；不会重复构造迁移表或重复成功安装执行器。
- `installUpgradeAction` 存在竞争时只有一个调用方成功，其他调用方得到 `Err(handler)`。一旦安装，handler 生命周期为整个进程，无法卸载。
- 函数指针不捕获环境，因此不会持有 Session、事务、连接或异步任务。所有迁移资源生命周期都应由未来的生产 handler 管理。
- 本文件不开启线程、任务或通道，也不持有数据库事务。Go v67/v284/v282 中的事务、结果集关闭和分批写入语义没有在这里实现；Rust 已移植的 v282 规划位于 `upgrade_run.rs`，持久化仍由调用方负责。
- `static mut currentBootstrapVersion` 不提供并发安全写入保证；若复刻 Go 测试的临时改写，必须串行化并恢复旧值，且不应与生产读取并发。

## 与 Go 版本的对应关系

Rust 的 `VersionedUpgradeFunction`、`currentBootstrapVersion` 和有序表分别对应 `pkg/session/upgrade_def.go` 的 `versionedUpgradeFunction`、同名变量和 `upgradeToVerFunctions`。两边当前目标均为 v317；Rust 测试固定表长为 176，并验证 257–276 后首项为 277、286–315 后首项为 316。

重要差异是实现深度：Go 的表项直接指向真实迁移函数，而 Rust 表项指向按名转发桩。Go v283 回填两个 analyze 默认值，v284 在 NextGen 中以悲观事务将旧反向开关迁移到新开关，v285 幂等增加 TTL `scan_index_id`，v316 创建物化视图维护表，v317 增加 `OPERATE VIEW` 权限并固化 adaptive-limit-scan 旧行为。Rust 对这些版本只有名称分发；其中 v283、v284 和 v317 的变量部分已另由 `upgrade_run::upgrade_bootstrap_variables` 实现，不能据此推断全部 Go DDL/DML 已移植。

数据结构也不是完全等价：Go `bindInfo.createTime` 是 `types.Time`，Rust 使用 `String`；Go 的 binding 结构参与 v67/v282 的 SQL 处理，Rust 本文件中的同名结构尚无调用者。Rust 的实际 v282 纯算法使用 `upgrade_run.rs` 内另外定义的 `BindingDigestRefreshRow` 与 `BindingDigestRefreshAction`。

## 扩展指南

- 新增 bootstrap 版本时，应同时新增 `versionN`、`upgrade_function!` 对应分支、`upgrades!` 末端表项和 `upgradeToVerN`，并把 `currentBootstrapVersion` 更新为最大版本。必须保留发行版预留区间和严格递增顺序。
- 若新增的是完整迁移，不能只增加名称桩并宣称完成；需要在生产 bootstrap 路径安装可靠 handler，或把真实逻辑接入现有 `UpgradeRuntime`/持久化运行时，并逐项对齐 Go 的 SQL、内核分支、幂等性、事务与错误语义。
- 若沿用全局 handler，建议优先把错误返回能力纳入边界设计；当前 `fn(&str)` 只能通过内部 panic/终止表达失败。改变签名时要同步所有 `upgradeToVerN`、安装点和独立测试。
- 修改版本表时同步 `pkg/session/upgrade_def_test.rs` 和 `pkg/session/upgrade_test.rs`：前者检查数量、升序和最终版本，后者还实际调用每个桩并验证分发名与保留区间。
- 修改版本对应行为时同步相邻的独立测试，而不要把测试嵌入源文件。变量迁移与 digest 行为应覆盖 `pkg/session/upgrade_backfill_test.rs`；端到端 bootstrap 行为应覆盖 `pkg/session/test/bootstraptest/boot_test.rs` 及对应 Go 测试。
- 兼容风险主要是遗漏旧集群升级、版本号重用/乱序、保留区间被占用和 Go/Rust 行为漂移；性能风险主要来自未来 handler 若一次性扫描或更新大型系统表，应保留 Go v282 这类顺序与分批写入约束。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件与 4,415 个 Go 文件；本次查询时索引可用。
- RustCodeGraph `node --file pkg/session/upgrade_def.rs`：完整读取 1–1633 行，确认版本常量、三类结构、176 项构造表、全部分发桩、`OnceLock` 安装边界和私有占位 Session；图同时报告该文件被 `pkg/session/upgrade_run.rs` 使用。
- RustCodeGraph `node --file pkg/session/upgrade_run.rs`：确认 `upgrade` 的版本筛选、MDL 钩子、提交恢复，以及 `upgrade_bootstrap_variables` 对本文件版本常量的生产使用。
- RustCodeGraph `query installUpgradeAction`、`query upgrade_action` 与仓库 `rg`：确认两个符号的唯一定义，并确认安装入口和函数表当前只被 Rust 独立测试消费。
- `pkg/session/Cargo.toml`、`pkg/session/lib.rs`：确认 crate 名、库入口、公开模块及独立测试模块装配；本文件没有专属外部依赖或 feature gate。
- `pkg/session/upgrade_def.go`、`pkg/session/upgrade_run.go`：核对 Go 的版本表不变量、真实迁移位置以及 v282–v317 的关键行为。
- `pkg/session/upgrade_def_test.rs`、`pkg/session/upgrade_test.rs`：核对表长 176、严格升序、最终版本 317、两个保留区间和“函数名等于版本名”的真实测试意图。
- 人工复核结论：本文明确回答了文件为何存在、当前实际如何运行、哪些能力尚未生产接线，以及新增版本时必须同步的定义与独立测试；未把 Go 的完整实现误报为 Rust 已支持。
