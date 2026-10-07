# [`br/pkg/streamhelper/config/tidb_conf.rs`](tidb_conf.rs)

## 文件定位

本文件属于独立 library crate `astersql-br-pkg-streamhelper-config`；crate 入口是同目录的 `lib.rs`，它公开 `tidb_conf` 模块并通过 `pub use tidb_conf::*` 扁平再导出本文件的公开符号。`Cargo.toml` 表明该 crate 只直接依赖 `astersql-sessionctx-vardef`，对应的 Go package 是 `br/pkg/streamhelper/config`。

它提供检查点推进器运行在 TiDB 进程内时使用的配置实现。Rust 上游 `br/pkg/streamhelper/advancer.rs::NewTiDBCheckpointAdvancer` 调用 `DefaultTiDBConfig`，将结果装入 `Cfg::TiDB`；推进器随后只通过 `types.rs::Config` trait 读取配置。该文件不是通用 TiDB server 配置，也不负责解析命令行或注册系统变量。

## 核心职责

1. 用 `TiDBConfig` 包装一份 `command_conf.rs::CommandConfig`，复用退避、tick、轮询和 resolve-lock 的默认策略。
2. 实现 `types.rs::Config`：除检查点滞后上限外，全部调用都委托给内嵌 `CommandConfig`。
3. 让 `GetCheckPointLagLimit` 动态读取 `astersql_sessionctx_vardef::AdvancerCheckPointLagLimit`，使 `SET GLOBAL tidb_advancer_check_point_lag_limit=...` 的进程级更新无需重建 `TiDBConfig` 即可生效。
4. 提供 `AdvancerCheckPointLagLimitNanos` 兼容视图，供现有 Rust 测试以原子量风格读写同一个 vardef 值；它不是第二份配置存储。

## 主要符号

- `AdvancerCheckPointLagLimitNanosValue`：无字段的公开兼容类型。`load(&self, Ordering) -> u64` 调用 vardef 的 `Load()`，`store(&self, u64, Ordering)` 调用 vardef 的 `Store(i64)`；传入的 `Ordering` 参数被有意忽略，因为真正的同步语义由 vardef 包装类型负责。
- `AdvancerCheckPointLagLimitNanos`：上述零大小类型的公开静态实例。`config_test.rs` 和 `br/pkg/streamhelper/export_test.rs` 用它模拟 Go 系统变量更新或测试导出方法。
- `ADVANCER_LAG_LIMIT_TEST_LOCK`：仅在 `cfg(test)` 下存在的 crate 内静态 `Mutex<()>`，串行化会改写进程级滞后上限的测试，避免测试彼此污染。
- `TiDBConfig { pub CommandConfig: CommandConfig }`：公开、可克隆且可调试打印的 TiDB 嵌入模式配置。字段名保留 Go 风格；它拥有而非借用 `CommandConfig`。
- `DefaultTiDBConfig() -> TiDBConfig`：用 `defaultCommandConfig()` 构造默认实例。它返回具体类型，之后通过 `Config` trait 或推进器内部 `Cfg` 枚举使用。
- `impl Config for TiDBConfig`：`GetBackoffTime`、`TickTimeout`、`GetDefaultStartPollThreshold`、`GetSubscriberErrorStartPollThreshold`、`GetResolveLockInterval` 委托给 `CommandConfig`；`GetCheckPointLagLimit` 是唯一覆盖点，返回 vardef 原子值对应的 `Duration`。

## 执行流程

TiDB 内嵌模式的主流程如下：

1. `br/pkg/streamhelper/advancer.rs::NewTiDBCheckpointAdvancer` 调用 `DefaultTiDBConfig`。
2. `DefaultTiDBConfig` 调用 `command_conf.rs::defaultCommandConfig`，得到默认 5 秒退避、12 秒 tick、4 分钟轮询阈值、48 小时静态滞后字段和零 ownership 周期，并将其放入 `TiDBConfig`。
3. 推进器把实例保存为受互斥锁保护的 `Cfg::TiDB`。读取一般参数时，`Cfg` 转发到 `TiDBConfig`，后者再转发到内嵌 `CommandConfig`。
4. 当 `advancer.rs::importantTick` 上传全局检查点后，它调用 `isCheckpointLagged`；该方法经 `Cfg::GetCheckPointLagLimit` 到达本文件的覆盖实现。
5. 覆盖实现每次调用 `AdvancerCheckPointLagLimit.Load()`，将纳秒数转为 `Duration`。因此 `pkg/sessionctx/variable/sysvar_builtins.rs` 中该全局系统变量的 `SetGlobal` 回调执行 `Store(duration)` 后，既有配置对象下一次检查便能看到新值。
6. `isCheckpointLagged` 将允许时长换算为 TSO 物理时间跨度；实际跨度超过上限时，`importantTick` 暂停备份任务并返回 `check point lagged too large`。

测试兼容流程更短：调用者通过 `AdvancerCheckPointLagLimitNanos.store` 写入纳秒值，兼容层直接写同一个 vardef 原子量，随后 `DefaultTiDBConfig().GetCheckPointLagLimit()` 从该位置读回。

## 数据与状态

`TiDBConfig` 自身只有一份拥有所有权的 `CommandConfig`。该字段中的 `CheckPointLagLimit` 仍会被构造为默认 48 小时，但 TiDB 模式的 trait 方法不会读取它；该静态字段主要随其余命令配置一起保留，并可在测试辅助代码中原位修改其他字段。

真正的 TiDB 模式滞后上限是 vardef crate 中的进程级 `AdvancerCheckPointLagLimit: AtomicI64Value`。其默认值 `DefTiDBAdvancerCheckPointLagLimit` 是 48 小时的纳秒数，系统变量注册限定输入为 1 秒到 365 天，并在 setter 中解析 Go duration 后写入原子值。所有 `TiDBConfig` 实例共享这一个值，因此更新不是实例级操作。

`AdvancerCheckPointLagLimitNanosValue` 与其静态实例都不保存数据。测试锁也不保护生产读取；它只协调同一测试二进制中会改写共享全局值的用例。

## 依赖与调用关系

- 上游构造者：`br/pkg/streamhelper/advancer.rs::NewTiDBCheckpointAdvancer`；RustCodeGraph 将它列为 `DefaultTiDBConfig` 的生产调用者，另一调用者是 `tidb_conf_test.rs`。
- 上游消费链：`advancer.rs::Cfg` 为 `Config` 实现分派，`CheckpointAdvancer::isCheckpointLagged` 调用 `GetCheckPointLagLimit`，`importantTick` 根据结果暂停任务。`UpdateConfigTiDB` 可替换推进器持有的 TiDB 配置，但全局滞后上限仍不来自实例字段。
- crate 内下游：`command_conf.rs::defaultCommandConfig`、`CommandConfig` 的五个 trait 方法，以及 `types.rs::Config`。
- 跨 crate 下游：`astersql_sessionctx_vardef::AdvancerCheckPointLagLimit`。对应系统变量的注册与热更新位于 `pkg/sessionctx/variable/sysvar_builtins.rs`。
- crate 装配：同目录 `lib.rs` 负责模块声明及再导出；`br/pkg/streamhelper/Cargo.toml` 以路径依赖引入本 crate，`br/cmd/br` 与 `br/pkg/utiltest/crr` 也声明了该配置 crate 的路径依赖。

## 错误处理与边界

本文件的所有公开方法都不返回 `Result`，也不自行记录日志。委托方法只是读取内存字段；vardef 的读取和写入也被视为不可失败。系统变量字符串的解析、1 秒至 365 天范围检查及错误返回发生在 `sysvar_builtins.rs`，不在本文件重复执行。

需要特别注意整数边界：生产 `GetCheckPointLagLimit` 把 vardef 的 `i64` 直接转换为 `u64`，兼容层 `store` 又把 `u64` 直接转换为 `i64`。正常系统变量路径保证值为正且不超过 365 天，因而不会触发符号翻转；绕过 setter 的内部或测试调用若写入负数或大于 `i64::MAX` 的值，会按 Rust `as` 转换规则回绕，可能产生与输入意图不符的超大 `Duration`。扩展代码不应把兼容静态量当成无约束公共配置接口。

传给兼容层 `load/store` 的 `Ordering` 不影响底层操作，调用者不能据此要求额外的 happens-before 保证。测试获取 `ADVANCER_LAG_LIMIT_TEST_LOCK` 时使用 `unwrap()`；若持锁测试 panic 导致 mutex poisoned，后续测试也会 panic，这是测试隔离策略而非生产错误路径。

## 并发与资源生命周期

生产热更新依赖 vardef 的原子包装，因此多个读取者可无锁观察进程级值；每次调用 `GetCheckPointLagLimit` 都重新加载，而非在 `TiDBConfig` 中缓存。可见性和内存顺序由 `AtomicI64Value::Load/Store` 的实现决定，本文件不接收或传递显式 ordering。

`TiDBConfig` 没有线程、异步任务、通道、文件句柄或析构逻辑；克隆它只复制内嵌 `CommandConfig`，不会复制全局 vardef 状态。配置对象的生命周期由 `CheckpointAdvancer.cfg: Mutex<Cfg>` 管理，而全局滞后上限贯穿进程生命周期。测试必须在持有 `ADVANCER_LAG_LIMIT_TEST_LOCK` 时保存并恢复原值；`tidb_conf_test.rs` 恢复进入测试前的值，`config_test.rs` 恢复默认值。

## 与 Go 版本的对应关系

Go 的 `tidb_conf.go` 令 `TiDBConfig` 匿名嵌入 `*CommandConfig`，因此自然继承其方法，并只显式覆盖 `GetCheckPointLagLimit`；Rust 无结构体方法提升，故 `impl Config` 明确写出五个委托方法和一个覆盖方法。两边的关键语义一致：默认构造包含 `defaultCommandConfig()`，而滞后上限读取 `vardef.AdvancerCheckPointLagLimit` 的实时值。

接口形态有三点差异。第一，Go `DefaultTiDBConfig() Config` 返回装箱在接口后的指针，Rust 返回拥有数据的具体 `TiDBConfig`，再由调用方通过 trait/枚举分派。第二，Rust 的 `AdvancerCheckPointLagLimitNanosValue`、`AdvancerCheckPointLagLimitNanos` 和测试 mutex 是为移植测试提供的兼容设施，Go 文件中没有对应生产符号。第三，Rust 用整数纳秒承接 Go `time.Duration`；正常路径单位一致，但 Rust 的显式有符号/无符号转换具有前述边界风险。

Go `config_test.go::TestCheckPointLimit` 证明 `SET GLOBAL` 后 TiDB 配置从 48 小时变为 100 小时，而命令配置仍保持 48 小时；Rust `config_test.rs::test_check_point_limit` 通过同一 vardef 原子量复现该区别。Go `TestResolveLockInterval` 与 Rust `test_resolve_lock_interval` 都证明 TiDB 默认 resolve-lock 间隔来自内嵌命令配置的两倍 tick。

## 扩展指南

- 新增所有模式共享的配置项时，先扩展 `types.rs::Config` 和 `command_conf.rs::CommandConfig`，再在本文件的 `impl Config for TiDBConfig` 中明确委托；同时更新独立的 `config_test.rs`，不要把测试内嵌进生产 `.rs` 文件。
- 新增由 TiDB 系统变量热更新的配置项时，应像 `GetCheckPointLagLimit` 一样直接读取 canonical vardef 原子状态，并同步核对 `pkg/sessionctx/vardef` 的默认值、`pkg/sessionctx/variable/sysvar_builtins.rs` 的解析/范围/setter/getter，以及同路径 Go 实现。不要同时维护实例缓存和全局值，否则会产生双重真相来源。
- 若修改滞后上限的数值类型或单位，必须同时检查 `isCheckpointLagged` 的 `Duration -> 毫秒 -> TSO` 换算、测试兼容层的纳秒转换，以及 `i64/u64` 边界；兼容性风险包括已有系统变量取值、48 小时默认值和 365 天上限。
- 若增加会修改全局 vardef 的测试，复用 `ADVANCER_LAG_LIMIT_TEST_LOCK`，在测试结束前恢复原值，并同步 `tidb_conf_test.rs`、`config_test.rs` 以及必要的 Go 测试。不要依赖测试执行顺序。
- 本文件的热路径只是少量字段读取和一次原子加载；新增解析、锁或 I/O 会改变推进 tick 的成本，应放在 setter/初始化路径而不是 getter 中。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标文件被识别为 76 行、20 个符号。
- RustCodeGraph 源码与调用查询：`node --file br/pkg/streamhelper/config/tidb_conf.rs`；查询 `TiDBConfig`、`DefaultTiDBConfig`；`explore` 确认 `DefaultTiDBConfig -> NewTiDBCheckpointAdvancer`、`GetCheckPointLagLimit -> isCheckpointLagged` 及对应测试调用边。
- 直接读取的实现与装配：`br/pkg/streamhelper/config/tidb_conf.rs`、`command_conf.rs`、`types.rs`、`lib.rs`、`Cargo.toml`，以及 `br/pkg/streamhelper/advancer.rs` 的 `Cfg`、`NewTiDBCheckpointAdvancer`、`isCheckpointLagged`、`importantTick`。
- Go 对照：`br/pkg/streamhelper/config/tidb_conf.go`、`command_conf.go`、`types.go`、`config_test.go`，以及 `br/pkg/streamhelper/advancer.go` 的构造和滞后判断调用点。
- Rust 测试：`br/pkg/streamhelper/config/tidb_conf_test.rs` 验证读取共享 vardef 值；`config_test.rs` 验证默认值、全局热更新与 Command/TiDB 模式隔离；`br/pkg/streamhelper/export_test.rs` 验证测试辅助更新路径。
- 全局值证据：`pkg/sessionctx/vardef/tidb_vars.rs` 定义默认 48 小时和 `AtomicI64Value`；`pkg/sessionctx/variable/sysvar_builtins.rs` 注册全局 duration 系统变量、限制 1 秒至 365 天并在 setter/getter 中访问同一原子量。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前使用任务指定命令验证固定的 11 个二级章节。
