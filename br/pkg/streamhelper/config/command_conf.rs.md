# `br/pkg/streamhelper/config/command_conf.rs`

## 文件定位

本文件属于独立库 crate `astersql-br-pkg-streamhelper-config`。crate 入口是同目录的 `lib.rs`，它以 `pub mod command_conf` 挂载本模块并用 `pub use command_conf::*` 扁平导出公开项；`Cargo.toml` 指定 `lib.rs` 为库入口。文件自身只使用标准库的 `HashMap`、`Duration` 以及本 crate `types.rs` 中的 `Config` trait，不直接使用该 crate 声明的 `astersql-sessionctx-vardef` 依赖；后者由相邻的 `tidb_conf.rs` 使用。

它是 BR stream checkpoint advancer 的“命令行配置实现”。CLI 入口 `br/cmd/br/stream.rs::newStreamAdvancerCommand` 注册本模块的标志，`streamCommand` 在 `StreamCtl` 分支把标志解析成 `CommandConfig`，再复制到任务层的 `AdvancerCommandConfig`。运行时另一条直接使用路径位于 `br/pkg/streamhelper/advancer.rs::NewCommandCheckpointAdvancer`，该构造函数把 `DefaultCommandConfig()` 包装为内部 `Cfg::Command`。

## 核心职责

- 定义五个 advancer duration 标志名及默认值：重试退避、tick 周期、主动轮询阈值、检查点最大滞后、owner 轮转周期。
- 用轻量 `FlagSet = HashMap<&'static str, Duration>` 注册默认值并读取覆盖值。它是 Rust 端当前 CLI 框架与 Go `pflag.FlagSet` 之间的适配桩，不是完整命令行解析器。
- 以 `CommandConfig` 保存五项配置，并提供默认构造与按固定顺序从 `FlagSet` 回填的入口。
- 实现 `types.rs::Config`，把原始字段转换为推进器所需的六项查询：其中订阅异常轮询阈值派生为 `TryAdvanceThreshold * 9 / 20`，resolve-lock 间隔派生为 `TickDuration * 2`。

本文件不负责启动推进循环、解析字符串形式的 duration、同步配置或执行重试；这些职责分别位于 CLI 框架、`advancer.rs` 及其环境实现中。

## 主要符号

- `flagBackoffTime`、`flagTickInterval`、`flagTryAdvanceThreshold`、`flagCheckPointLagLimit`、`flagOwnershipCycleInterval`：五个键名，均为 `&str` 常量。最后一项供 chaos 场景使用。
- `DefaultTryAdvanceThreshold`（4 分钟）、`DefaultCheckPointLagLimit`（48 小时）、`DefaultBackOffTime`（5 秒）、`DefaultTickInterval`（12 秒）、`DefaultOwnershipCycleInterval`（0）：默认 `Duration`；ownership 的零值表示不主动轮转。
- `FlagSet`：`HashMap<&'static str, Duration>` 类型别名。键要求静态生命周期，值已是解析后的 duration。
- `DefineFlagsForCheckpointAdvancerConfig(&mut FlagSet)`：无条件插入五个默认项；同名键会被默认值覆盖。
- `CommandConfig`：公开、可克隆且支持 `Debug`、值相等比较的配置结构；五个字段均公开，便于 CLI/task 转接和测试局部改写。
- `defaultCommandConfig() -> CommandConfig`：以本文件五个默认常量逐字段构造。虽然注释称内部默认构造，当前符号实际声明为 `pub`，并会被 `lib.rs` 的模块公开路径访问；`tidb_conf.rs::DefaultTiDBConfig` 正是直接调用者。
- `DefaultCommandConfig() -> CommandConfig`：公共默认入口，仅委托 `defaultCommandConfig`。Rust 返回具体值；Go 对照返回 `Config` 接口中的指针。
- `CommandConfig::GetFromFlags(&mut self, &FlagSet) -> Result<(), String>`：按 Backoff、Tick、TryAdvance、LagLimit、Ownership 的顺序取值并原地赋值。
- `impl Config for CommandConfig`：`GetDefaultStartPollThreshold`、`GetCheckPointLagLimit`、`TickTimeout`、`GetBackoffTime` 直接返回字段；`GetSubscriberErrorStartPollThreshold` 和 `GetResolveLockInterval` 执行比例派生。

## 执行流程

1. `br/cmd/br/stream.rs::newStreamAdvancerCommand` 创建隐藏的 `advancer` 子命令，并调用 `DefineFlagsForCheckpointAdvancerConfig(command.AdvancerFlags())`。五个键因此获得本模块默认值。
2. 命令进入 `streamCommand(..., StreamCtl)` 后，先解析公共 stream 标志，再调用 `DefaultCommandConfig()` 获得完整默认配置。
3. `GetFromFlags` 按固定顺序读取五个键。所有键存在时，配置被 flag 值完整覆盖；任何键缺失时立即返回对应的 `missing ...` 字符串。
4. CLI 将五个公开字段复制到 `astersql_br_pkg_task::AdvancerCommandConfig`。这是配置跨 crate 进入任务层的接线点。
5. streamhelper 自身的命令行推进器由 `advancer.rs::NewCommandCheckpointAdvancer` 以相同默认入口初始化，并通过内部 `Cfg` 枚举按 `Config` trait 查询值。
6. 推进期间，`GetCheckPointLagLimit` 用于判断是否因全局检查点滞后过大而暂停任务；tick/轮询/resolve-lock/退避路径读取其余 trait 方法。`advancer.rs::refreshLogBackupFlushInterval` 还可把实际 flush 间隔写入原子覆盖值：覆盖值非零时，resolve-lock 与轮询阈值优先使用运行时值，而不是本文件字段。

## 数据与状态

`CommandConfig` 是普通拥有型值，所有状态都是五个 `Duration` 字段，没有引用、全局可变状态或内部可变性。默认构造每次产生独立值；`Clone` 是字段复制，`PartialEq/Eq` 是五字段逐值比较。

关键不变量与约定如下：

- 默认值集合同时由 `DefineFlagsForCheckpointAdvancerConfig` 和 `defaultCommandConfig` 引用同一批常量，避免正常默认路径发生漂移。
- `OwnershipCycleInterval == Duration::ZERO` 表示默认关闭；本文件不解释非零值如何触发 owner 轮转。
- 订阅正常时的轮询阈值就是 `TryAdvanceThreshold`；订阅异常时使用其 `9/20`。整数 duration 运算会按 `Duration` 的整数比例计算，不使用浮点数。
- resolve-lock 间隔始终由当前 `TickDuration * 2` 派生，所以修改 tick 会同步改变该间隔。
- `GetFromFlags` 不是事务式更新：在后续键缺失前已成功读取的字段会保留新值。`parity_test.rs::get_from_flags_preserves_values_and_first_error_side_effects` 固化了该行为。

## 依赖与调用关系

上游直接证据：

- `br/cmd/br/stream.rs::newStreamAdvancerCommand` → `DefineFlagsForCheckpointAdvancerConfig`。
- `br/cmd/br/stream.rs::streamCommand` → `DefaultCommandConfig` → `CommandConfig::GetFromFlags`，随后复制五字段到任务配置。
- `br/pkg/streamhelper/advancer.rs::NewCommandCheckpointAdvancer` → `DefaultCommandConfig`，并保存为 `Cfg::Command`。
- `br/pkg/streamhelper/config/tidb_conf.rs::DefaultTiDBConfig` → `defaultCommandConfig`；`TiDBConfig` 除 lag-limit 外，把 trait 查询委托给其内嵌的 `CommandConfig`。
- 测试辅助 `br/pkg/streamhelper/basic_lib_for_test.rs` 使用默认配置的 resolve-lock 间隔；`advancer_test.rs` 和 `export_test.rs` 通过公开字段或测试扩展修改活动配置。

下游依赖：

- `std::collections::HashMap` 承载已解析的 flag 值；`std::time::Duration` 承载所有时间量。
- `crate::types::Config` 定义推进器消费的统一查询接口。
- `advancer.rs` 的 `Cfg` 为 Command/TiDB 两种具体实现做枚举分派；它会读取配置，但锁、原子覆盖、tick 和任务生命周期均由该文件管理。

RustCodeGraph 的文件节点报告本文件被 8 个文件使用，并明确列出 `br/cmd/br/stream.rs`、`br/pkg/streamhelper/advancer.rs`、`advancer_test.rs`、`basic_lib_for_test.rs` 和 `config/tidb_conf.rs` 等；精确 `query` 也同时定位了 Rust/Go 的 `DefineFlagsForCheckpointAdvancerConfig`、`DefaultCommandConfig`、`GetFromFlags` 对照符号。图的 `callers/callees` 查询在本次分析中超时，故具体调用边以上述已索引文件节点和 `rg` 的逐处引用核验为准。

## 错误处理与边界

`GetFromFlags` 是本文件唯一显式失败入口。每次 `HashMap::get` 失败都会通过 `ok_or(...)?` 立即返回固定英文错误；错误顺序与读取顺序一致，因此同时缺多个键时只报告第一个。CLI 在 `streamCommand` 中用 `Error::new` 包装该字符串并中止命令配置。

由于原地逐字段赋值，失败不会回滚此前更新；调用者若要求原子更新，应先对临时 `CommandConfig` 调用，成功后再替换活动配置。当前 CLI 正是从新建默认值开始解析，因此错误对象不会进入任务配置。

本文件不校验零值、超大值或字段间关系。`Duration` 排除了负值，但 `TickDuration == 0`、阈值为零等输入仍可存入；其业务效果由下游推进器决定。`Duration * 2` 等算术对极端值可能触发标准库的溢出行为，因此引入不受信任或超大 duration 时应在 flag 解析边界增加范围验证，而不是依赖本实现兜底。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄或网络资源。`CommandConfig` 本身没有同步原语；共享与并发更新策略由持有者负责。

在真实运行路径中，`advancer.rs::CheckpointAdvancer` 将 `Cfg` 放入 `Mutex`，trait 查询和配置替换在锁内完成；flush 派生的 resolve-lock/try-advance 覆盖值则存于原子整数。因而本文件返回的 `Duration` 是一次读取的值，不携带锁守卫，也不管理覆盖值的生命周期。CLI 路径在启动前构建配置，随后按值复制到任务配置，不共享 `FlagSet`。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/streamhelper/config/command_conf.go`。五个键名、五个默认值、`CommandConfig` 字段、读取顺序，以及六个 `Config` 方法的计算都与 Go 对齐，特别是异常订阅阈值 `* 9 / 20` 与 resolve-lock 间隔 `* 2`。

已确认的实现形态差异：

- Go 使用 `pflag.FlagSet`，注册 duration 时附带帮助文本，并把 ownership flag 标记为隐藏；Rust `FlagSet` 只是 duration `HashMap`，没有帮助文本、字符串解析或单项 hidden 元数据。当前隐藏的是整个 advancer 子命令，不能据此声称 Rust 已实现 Go 的 flag 级隐藏行为。
- Go `CommandConfig` 字段带 TOML/JSON tag；Rust 结构没有 `serde` 派生或重命名属性，因此本文件不提供同等序列化契约。
- Go `defaultCommandConfig` 返回指针，`DefaultCommandConfig` 返回 `Config` 接口；Rust 两者均按值返回具体 `CommandConfig`，再由调用方通过静态 trait 分派或 `Cfg` 枚举使用。
- Go 的 flag 常量是包内私有；Rust 常量为 `pub`，且经 crate 入口再导出，测试和 CLI 可直接引用。
- Go `GetDuration` 可返回解析/类型相关错误；Rust map 的值已经是 `Duration`，当前只可能因键缺失失败。

Go 测试 `config_test.go` 与 Rust 独立测试 `config_test.rs` 都证明默认 lag-limit 为 48 小时、Command 配置不跟随 TiDB 全局 lag-limit 热更新，以及 resolve-lock 间隔随 tick 保持二倍关系。Rust 的 `parity_test.rs` 额外覆盖全部默认 trait 值、flag 回填、缺键错误的部分更新副作用和值相等语义。

## 扩展指南

新增配置字段时，至少应同步以下位置，避免“注册了但未消费”或默认值漂移：

1. 在本文件增加 flag 键、默认常量和 `CommandConfig` 字段，并同时修改 `DefineFlagsForCheckpointAdvancerConfig`、`defaultCommandConfig`、`GetFromFlags`。
2. 若推进器需要统一读取，修改 `types.rs::Config`、本文件的 trait 实现、`tidb_conf.rs` 的委托实现以及 `advancer.rs::Cfg` 分派；若只是 CLI 到任务层传递，还要修改 `br/cmd/br/stream.rs::streamCommand` 与任务层 `AdvancerCommandConfig`。
3. 与 `command_conf.go` 核对键名、默认值、读取顺序和派生公式。若 Rust 有意不同，文档与独立 parity 测试必须明确差异，不能把适配桩描述为完整 pflag 等价物。
4. 测试逻辑保持在独立文件：默认/trait 语义放入 `config_test.rs` 或 `parity_test.rs`，CLI 接线放入 `br/cmd/br/stream_test.rs`（若该场景已有对应测试），不要把 `#[cfg(test)]` 测试内嵌到生产源文件。
5. 对缺键行为的任何调整都要决定是否保留当前部分更新语义；若改为事务式更新，需要同步 Go 行为依据及 `get_from_flags_preserves_values_and_first_error_side_effects`。

兼容风险主要是 flag 键和默认值改变会影响命令行行为，字段/trait 改动会影响两个配置实现及 advancer 分派；性能风险很小，正常读取仅为字段复制和常数时间算术。若把轻量 `HashMap` 替换为真实解析器，还需评估帮助文本、隐藏标志、重复注册和错误文本兼容性。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点；`node --file br/pkg/streamhelper/config/command_conf.rs` 读取完整 115 行并报告 8 个使用文件；`query` 定位 Rust/Go 的三个关键入口。`explore` 及精确 `callers/callees` 本次在 30 秒限制内未返回，未把它们当作调用关系证据。
- 生产源码：`br/pkg/streamhelper/config/command_conf.rs`、`types.rs`、`tidb_conf.rs`、`lib.rs`、`br/pkg/streamhelper/advancer.rs`、`br/cmd/br/stream.rs`。
- crate 边界：`br/pkg/streamhelper/config/Cargo.toml`；直接消费者依赖由 `br/pkg/streamhelper/Cargo.toml` 与 `br/cmd/br/Cargo.toml` 核对。
- Go 对照：`br/pkg/streamhelper/config/command_conf.go`；Go 回归证据为 `config_test.go`，并参考 `br/pkg/streamhelper/advancer_test.go` 对活动配置的使用。
- Rust 独立测试：`br/pkg/streamhelper/config/config_test.rs`、`parity_test.rs`；调用侧测试证据包括 `br/pkg/streamhelper/advancer_test.rs` 与 `export_test.rs`。
- 本任务是只读逻辑分析加文档输出，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工检查本文能回答文件存在原因、CLI/advancer 执行链、错误副作用及安全扩展位置。
