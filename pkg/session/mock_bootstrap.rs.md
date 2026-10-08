# `pkg/session/mock_bootstrap.rs`

## 文件定位

该文件位于 `astersql-session` crate。`pkg/session/Cargo.toml` 将 crate 根指定为 `pkg/session/lib.rs`，后者以 `pub mod mock_bootstrap;` 公开本模块，因此其他 Rust crate 可以通过 `astersql_session::mock_bootstrap` 使用这里的测试辅助 API。

它模拟“在正常 bootstrap 版本链末尾再追加一个最新版本”的测试场景：调用方可选择完整或精简 DDL 序列、观察生命周期回调，并用显式传入的版本号决定是否执行。不过，当前仓库搜索只找到 Rust 测试调用方 `pkg/session/test/bootstraptest/bootstrap_upgrade_test.rs` 和 `pkg/session/upgrade_backfill_test.rs`；Rust 的 `pkg/session/upgrade_run.rs` 没有调用本模块。与之相对，Go 主链 `pkg/session/upgrade_run.go::upgrade` 会调用同路径 Go 实现 `addMockBootstrapVersionForTest`。因此本文件目前是已导出的、可测试的移植机制，而不是已接入 Rust 生产升级主链的入口。

## 核心职责

- 用 `WithMockUpgrade` 和 `MockUpgradeToVerLatestKind` 保存进程级 mock 开关及模式选择，并由 `RegisterMockUpgradeFlag` 同步测试侧标志。
- 用 `MockBootstrapRuntime` 隔离 SQL 执行与等待，使升级逻辑不绑定具体 Session，也能在独立测试中录制副作用。
- 用 `Callback`、`BaseCallback`、`TestCallback` 暴露追加版本前、每条完整升级 DDL 前、升级完成后的观测点。
- 用 `mockUpgradeToVerLatest` 执行完整 schema 变化，用 `mockSimpleUpgradeToVerLatest` 提供更快的代表性路径。
- 用 `modifyBootstrapVersionForTest` 和 `addMockBootstrapVersionForTest` 模拟版本上限改写以及向既有升级函数列表尾部接线。

本模块不负责真正解析或调度 DDL，也不持有数据库 Session；这些能力都由 `MockBootstrapRuntime::execute_sql` 的实现者提供。它也不直接运行 `versionedUpgradeFunction`，只构造带 `version` 与 `MockUpgradeAction` 的描述值。

## 主要符号

- `WithMockUpgrade: AtomicBool`：全局启用开关，初始为 `false`；所有读写均使用 `Ordering::SeqCst`。
- `MockUpgradeToVerLatestKind: AtomicI32`：模式开关，默认值 `defaultMockUpgradeToVerLatest`（0）；等于 `MockSimpleUpgradeToVerLatest`（1）时选择精简动作，其他值都回退到完整动作。
- `allDDLs: &[&str]`：完整路径的 31 条迁移 DDL，覆盖索引、主键、列、外键、重命名、字符集和分区变化。独立 Rust 测试把它当作升级步骤数量与末条 SQL 的公开契约。
- `FULL_SETUP_SQL: &[&str]`：私有的 11 条准备语句，先切换到 `mysql` 库并建立完整 DDL 所需的模拟系统表及会话变量。
- `MockBootstrapRuntime`：关联错误类型为 `Error`；`execute_sql` 执行一条 SQL 并可失败，`sleep` 提供可控的 DDL 间隔。
- `Callback`：定义 `OnBootstrapBefore`、`OnBootstrap`、`OnBootstrapAfter` 三个无返回值钩子。`BaseCallback` 全部为空实现；`TestCallback` 持有三个可选的 `FnMut() + Send` 闭包，存在时才转发。`TestCallback::Cnt` 只是公开槽位，本文件不读取或递增它。
- `MockUpgradeAction::{Full, Simple}`：版本函数的动作标签。
- `versionedUpgradeFunction { version, action }`：升级条目描述。类型名保持 Go 风格且公开字段可供测试断言；当前没有内嵌函数指针或执行方法。
- `MockUpgradeFlags { with_mock_upgrade }` 与 `RegisterMockUpgradeFlag`：前者保存调用方可见的布尔值，后者同时更新结构字段和全局原子开关。
- `mockUpgradeToVerLatest`、`mockSimpleUpgradeToVerLatest`：分别执行完整和精简升级。
- `modifyBootstrapVersionForTest`：满足开关与 HTTP 升级版本门槛时改写当前 bootstrap 版本。
- `addMockBootstrapVersionForTest`：启用时触发 before 回调、改写当前版本，并返回追加了一个 mock 条目的新 `Vec`。

## 执行流程

1. 测试通过 `RegisterMockUpgradeFlag(flags, true)` 同时打开局部标志和 `WithMockUpgrade`；如需快速路径，再把 `MockUpgradeToVerLatestKind` 设为 1。
2. `modifyBootstrapVersionForTest(version, support_upgrade_http_version, current_bootstrap_version, mock_latest_version)` 先检查全局开关。仅当输入版本和当前版本都不低于 HTTP 升级支持门槛时，才把当前版本改成 mock 最新版本。
3. `addMockBootstrapVersionForTest` 在开关关闭时仅克隆并返回原列表，不触发回调、不改版本；开关打开时先调用 `OnBootstrapBefore`，再写入 mock 最新版本，按原子模式值选择 `Full` 或 `Simple`，最后在原顺序后追加一个条目。
4. 动作执行由调用方负责。选择 `Full` 时调用 `mockUpgradeToVerLatest`：若 `version >= mock_latest_version` 立即成功返回；否则顺序执行 11 条准备 SQL，再对 `allDDLs` 中每条语句依次执行 `OnBootstrap`、SQL 和 20 ms sleep，全部成功后执行 `OnBootstrapAfter`。
5. 选择 `Simple` 时调用 `mockSimpleUpgradeToVerLatest`：同样先做版本短路；需要升级时顺序执行 `use mysql`、建表、加列、加索引四条 SQL，随后只调用一次 `OnBootstrapAfter`，不调用逐步回调也不 sleep。
6. 测试结束应显式把 `WithMockUpgrade` 和模式恢复默认值；现有独立 Rust 测试在末尾这样做，以减少跨测试串扰。

## 数据与状态

进程共享状态只有两个原子变量。`SeqCst` 提供单一全序的可见性，但 `RegisterMockUpgradeFlag` 先写普通的 `MockUpgradeFlags` 字段、再写全局原子，这两个位置不是一个原子事务；调用方不能把结构字段与全局值的组合当成不可分割快照。模式值与启用值也是两个独立原子，切换期间可能被并发读者观察为新旧组合。

升级函数自身不保留状态，所有 SQL 和等待副作用都委托给可变借用的 runtime，回调状态则由可变借用的 callback 持有。输入升级函数切片不会被原地修改：`addMockBootstrapVersionForTest` 总是复制为新 `Vec`，启用时再追加一项。`current_bootstrap_version` 则通过可变引用原地更新。

DDL 顺序是行为的一部分。完整路径必须先完成 `FULL_SETUP_SQL`，之后才进入 `allDDLs`；每个逐步回调发生在对应 DDL 之前，每次 sleep 发生在成功执行 DDL 之后。精简路径的四条 SQL 没有等待间隔。

## 依赖与调用关系

直接标准库依赖只有 `std::sync::atomic::{AtomicBool, AtomicI32, Ordering}` 和 `std::time::Duration`；该文件不直接使用 `pkg/session/Cargo.toml` 中的外部 crate 依赖或 feature，`nextgen` feature 也不改变本文件编译内容。

RustCodeGraph 的文件符号图识别出两个 trait、三个数据类型、两个标志结构/动作类型和五个顶层函数；调用边显示 `mockUpgradeToVerLatest` 与 `mockSimpleUpgradeToVerLatest` 调用 `execute_sql`，完整路径额外调用 `sleep`，三个流程函数分别调用相应的 callback 方法。精确 callers 查询在当前索引上未正常返回，因此上游证据由仓库全局符号搜索补足。

当前 Rust 上游关系为：

- `pkg/session/lib.rs` 公开模块。
- `pkg/session/test/bootstraptest/bootstrap_upgrade_test.rs` 直接调用完整/精简执行函数、版本改写和版本条目追加函数，并实现录制 runtime 与计数 callback。
- `pkg/session/upgrade_backfill_test.rs::canonical_mock_upgrade_retargets_only_supported_versions` 直接验证 HTTP 版本门槛。
- 未发现非测试 Rust 文件调用这些函数；`pkg/session/upgrade_run.rs` 未接入它们。

Go 对照主链是 `pkg/session/upgrade_run.go::upgrade -> pkg/session/mock_bootstrap.go::addMockBootstrapVersionForTest -> versionedUpgradeFunction.fn`，最终由真实 Session 执行 DDL。该边只能证明 Go 设计来源，不能证明 Rust 已有相同生产接线。

## 错误处理与边界

两个执行函数以 `Result<(), R::Error>` 返回 runtime 自定义错误。每次 `execute_sql` 后使用 `?`：首个错误会原样向上传播，后续 SQL、对应 sleep 以及最终 `OnBootstrapAfter` 都不会执行。回调和 `sleep` 没有错误返回通道；若其实现 panic，本模块不捕获。

版本边界是 `version >= mock_latest_version`：相等或调用方版本更高时均完全短路，不执行 SQL、sleep 或 after 回调。`modifyBootstrapVersionForTest` 还要求 `version` 与 `*current_bootstrap_version` 同时达到门槛，任一不足都保持原值。`addMockBootstrapVersionForTest` 不去重，也不检查原列表是否已有相同版本；调用方必须保证只追加一次或能处理重复条目。

完整路径中的建表语句并非全部带 `if not exists`，因此重复执行可能由具体 runtime/数据库返回错误。DDL 文本还依赖 TiDB 方言和 `mysql` 系统库权限。当前 Rust 测试只使用永不失败的录制 runtime，没有直接覆盖中途错误、重复执行或非法模式值；这些边界应视为由源码控制流确认、尚无独立回归测试。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或数据库事务。全局开关使用原子类型避免数据竞争，但不会自动隔离并行测试；现有测试通过结束时恢复开关降低污染风险，仍要求并行使用者协调全局状态。

`MockBootstrapRuntime` 和 `Callback` 都通过 `&mut` 独占借用在单次调用中顺序使用，因此单次执行不会并发调用 runtime 或 callback。完整路径的 20 ms 等待是同步阻塞式 `sleep` 抽象；真实实现若直接睡眠，会把整条调用链阻塞 `allDDLs.len()` 次。资源的创建、事务提交/回滚及 Session 关闭均由 runtime 或外层升级流程负责，本模块没有清理钩子。发生 SQL 错误时也不会补偿已成功的前序 DDL。

Go 集成测试 `pkg/session/test/bootstraptest/bootstrap_upgrade_test.go::TestUpgradeWithPauseDDL` 利用 callback 并发提交用户/系统 DDL，验证系统 DDL 不被暂停且用户 DDL 后执行；这说明这些钩子的设计用途，但 Rust 当前的录制测试没有复现该真实并发调度。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/session/mock_bootstrap.go`。Rust 保留了 Go 的完整/精简 SQL 序列、版本短路、20 ms 间隔、三阶段 callback、HTTP 版本门槛和“向升级列表尾部追加 mock 最新版本”的核心语义。`pkg/session/test/bootstraptest/bootstrap_upgrade_test.rs` 明确以 Go 的升级测试契约为依据，验证完整路径为 11 条准备 SQL 加全部 `allDDLs`、每条迁移都 sleep，以及 callback 次数和简单动作选择。

主要差异如下：

- Go 的 `RegisterMockUpgradeFlag` 在 `flag.FlagSet` 注册命令行参数并令 `WithMockUpgrade` 为 `*bool`；Rust 接收自定义 `MockUpgradeFlags`，只同步内存状态，不注册 CLI。`cmd/tidb-server/main.rs` 中出现的 `session::RegisterMockUpgradeFlag` 来自其独立 stub 接线，仓库搜索未显示它调用本 Rust 模块。
- Go 直接使用 `sessionapi.Session`、`mustExecute` 和日志；Rust 用 `MockBootstrapRuntime` 泛化 SQL/sleep，保留错误返回且不记录日志。
- Go callback 接收 Session，`TestCallback.Cnt` 是原子计数器；Rust callback 不带 Session，`Cnt` 为 `Option<usize>` 且本模块不操作它，闭包要求 `Send`。
- Go 的 `versionedUpgradeFunction` 持有可执行函数；Rust 只持有 `MockUpgradeAction`，需要外层自行分派 Full/Simple。
- Go 的升级主链已经调用追加函数；Rust 生产升级主链尚未接入。因此 Go 集成测试证明真实 DDL/并发行为，Rust 测试目前只证明本文件的纯控制流与录制副作用。

Go 测试还在 `TestUpgradeWithPauseDDL` 中检查最终 `mysql.mock_sys_t` 和分区表 schema，覆盖了完整 DDL 的组合结果；Rust 独立测试只核对数量、顺序端点和回调次数，未执行真实数据库 schema 校验。

## 扩展指南

- 新增或调整完整迁移步骤时，修改 `allDDLs`；若步骤需要前置对象，再同步修改私有 `FULL_SETUP_SQL`。同时对照 `pkg/session/mock_bootstrap.go`，并更新 `pkg/session/test/bootstraptest/bootstrap_upgrade_test.rs` 的数量、顺序、等待和回调断言；真实 schema 变化还应同步 Go 集成测试中的最终 schema 断言。
- 改动精简路径时，直接修改 `mockSimpleUpgradeToVerLatest` 的 SQL 数组，并扩展 `simple_upgrade_executes_real_schema_transition_sequence_once`。不要为了加速而改变完整路径的行为契约。
- 新增动作类型时，需要同时扩展 `MockUpgradeAction`、`addMockBootstrapVersionForTest` 的选择逻辑以及外层动作分派；当前文件没有统一执行入口，遗漏分派会产生“成功追加但无法执行”的条目。
- 若把本模块接入 Rust 升级主链，应在 `pkg/session/upgrade_run.rs` 的版本函数构造/遍历处做最小接线，并提供真实 Session 的 `MockBootstrapRuntime` 适配器；还需补独立测试验证条目只追加一次、错误传播、版本更新时机和事务语义。不要仅凭 Go 调用边假设 Rust 已完成接入。
- 若并行测试会切换全局开关，应增加串行化保护或显式状态 guard，在析构时恢复 `WithMockUpgrade` 与 `MockUpgradeToVerLatestKind`；仅在测试末尾手工恢复不能覆盖 panic 路径。
- 若要求与 Go 的并发 DDL 测试等价，需要让 callback 获得足够的 Session/runtime 上下文，或另设上下文 trait；当前无参数 callback 无法直接执行 Go 测试中的查询与异步 DDL。

兼容风险主要来自 DDL 顺序、系统表最终结构、版本门槛和全局状态泄漏；性能风险集中在完整模式的多条 DDL 与逐条 20 ms 阻塞等待。测试应继续放在独立的 `*_test.rs` 文件中，不应嵌入生产源文件。

## 验证依据

- 源码与模块边界：`pkg/session/mock_bootstrap.rs`、`pkg/session/lib.rs::mock_bootstrap`、`pkg/session/Cargo.toml`（crate 名 `astersql-session`、根 `lib.rs`）。目标包没有 `pkg/session/doc.go`，因此无可读取的包级 Go 契约文件。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query` 分别定位 Rust/Go 的 `addMockBootstrapVersionForTest`、`mockUpgradeToVerLatest`、`mockSimpleUpgradeToVerLatest`、`modifyBootstrapVersionForTest`、`RegisterMockUpgradeFlag`；`node --file pkg/session/mock_bootstrap.rs --symbols-only` 列出本文件全部 trait、struct、enum 和函数。图的宽泛 `explore` 还确认 `execute_sql` 被完整/精简路径调用、`sleep` 被完整路径调用、三个 callback 方法被相应流程调用。按文件限定的精确 callers/callees 命令在本地索引上超时，未把其缺失结果当作结论。
- Rust 测试：`pkg/session/test/bootstraptest/bootstrap_upgrade_test.rs` 验证版本短路、四条精简 SQL、完整模式 11 条准备 SQL加 `allDDLs`、每条 20 ms sleep、callback 次数、开关/版本改写及 Simple 动作；其独立测试 crate 在 `pkg/session/test/bootstraptest/Cargo.toml` 以 dev-dependency 依赖 `astersql-session`。`pkg/session/upgrade_backfill_test.rs::canonical_mock_upgrade_retargets_only_supported_versions` 验证双重 HTTP 版本门槛。
- Go 对照：`pkg/session/mock_bootstrap.go` 提供原始语义；`pkg/session/upgrade_run.go::upgrade` 是真实上游；`pkg/session/test/bootstraptest/bootstrap_upgrade_test.go::TestUpgradeWithPauseDDL` 验证 callback 驱动的并发 DDL 顺序和最终系统表 schema。
- 全仓库 Rust 引用搜索只发现上述测试、`pkg/session/lib.rs` 的模块导出，以及 `cmd/tidb-server` 自身的同名 stub；未发现本模块的非测试 Rust 生产调用方。该搜索是“Rust 生产主链尚未接入”结论的直接依据。
- 本任务为纯文档分析，按任务约束未运行 Cargo。交付结构检查使用任务规定的命令，确保本文档存在且恰含 11 个固定二级标题。
