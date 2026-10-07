# `br/pkg/streamhelper/config/types.rs`

## 文件定位

本文件是 `astersql-br-pkg-streamhelper-config` crate 的配置契约层。crate 入口 `br/pkg/streamhelper/config/lib.rs` 以 `pub mod types` 挂载本模块，并用 `pub use types::*` 扁平导出其中的 `Config` 和 `DefaultMaxConcurrencyAdvance`。`br/pkg/streamhelper/config/Cargo.toml` 将该目录声明为 library crate，并以 `br/pkg/streamhelper/config` 标记其 Go 对照包；本文件本身只依赖 Rust 标准库的 `AtomicI32` 与 `Duration`，不直接使用该 crate 唯一的外部工作区依赖 `astersql-sessionctx-vardef`。

它位于日志备份检查点推进器与具体配置来源之间：`command_conf.rs` 提供命令行配置实现，`tidb_conf.rs` 提供 TiDB 进程内配置实现，`br/pkg/streamhelper/advancer.rs` 再通过内部枚举 `Cfg` 屏蔽两种来源的差异。这里不是配置解析器，也不包含推进算法。

## 核心职责

本文件承担两个职责：

1. 用 `Config` trait 规定检查点推进循环所需的六类时间参数，使推进器不必知道参数来自命令行字段还是 TiDB 全局变量。
2. 用进程级原子整数 `DefaultMaxConcurrencyAdvance` 表达 Go 包级可变并发默认值，并把初始值固定为 8。

该抽象刻意只暴露读取方法。字段校验、flag 解析、阈值计算和 TiDB 系统变量读取均由实现者完成；调用方得到的统一结果类型是 `Duration`。

## 主要符号

- `pub static DefaultMaxConcurrencyAdvance: AtomicI32 = AtomicI32::new(8)`：公开的进程级并发默认值。使用原子类型保留 Go 可变包变量的运行时修改能力，同时要求读写方显式选择内存序。当前 Rust 生产代码未读取它；`config/parity_test.rs::go_mutable_concurrency_default_and_value_equality_match` 验证其 `load`/`store` 行为。
- `pub trait Config`：公开的配置读取契约，没有关联类型、默认实现或错误返回。
  - `GetBackoffTime`：两次重试之间的等待时间。
  - `TickTimeout`：单轮推进允许占用的最长时间。
  - `GetDefaultStartPollThreshold`：订阅正常时，检查点落后到需要主动轮询 TiKV 的阈值。
  - `GetSubscriberErrorStartPollThreshold`：订阅异常时更积极开始轮询的阈值。
  - `GetResolveLockInterval`：检查点持续不变多久后尝试解锁。
  - `GetCheckPointLagLimit`：允许的最大检查点滞后；具体调用方将零值解释为禁用滞后限制。

命名沿用 Go 导出方法，因此不符合 Rust 的 snake_case 习惯；`config/lib.rs` 在 crate 级允许 `non_snake_case` 与 `non_upper_case_globals`。

## 执行流程

本文件没有可执行函数，运行时流程由实现与调用方共同形成：

1. `DefaultCommandConfig` 或 `DefaultTiDBConfig` 构造具体配置；二者分别在 `command_conf.rs` 和 `tidb_conf.rs` 实现 `Config`。
2. `advancer.rs` 的 `Cfg::{Command, TiDB}` 保存具体配置，并再次实现 `Config`，每个方法按枚举分支转发给对应实现。
3. `CheckpointAdvancer` 在需要参数时锁住其 `cfg: Mutex<Cfg>` 并调用契约方法。当前生产代码直接使用 `GetResolveLockInterval`、`GetDefaultStartPollThreshold`、`GetSubscriberErrorStartPollThreshold` 和 `GetCheckPointLagLimit`；其中前三者还可能被推进器自身的原子覆盖值替代。
4. `isCheckpointLagged` 把 `GetCheckPointLagLimit` 返回的时长换算为 TSO 物理部分的跨度；零时长直接关闭该检查。其余阈值分别控制 resolve-lock 和轮询时机。

`GetBackoffTime` 与 `TickTimeout` 已由三个 Rust 实现层提供并由测试覆盖，但在当前 `advancer.rs` 生产流程中只完成了转发，尚未发现实际读取点。相对地，Go `advancer.go` 已在监听重试、tick 超时和写入超时路径使用二者。

## 数据与状态

`Config` 自身无字段、无缓存，也不规定实现是否可变。全部返回值都是按值复制的 `Duration`，因此调用完成后不借用实现对象。

`DefaultMaxConcurrencyAdvance` 是本文件唯一持久状态，类型为有符号 32 位原子整数，初始值为 8。类型系统并未限制后续写入为正数，也没有在本文件内执行范围校验或单位转换。当前测试使用 `Ordering::SeqCst`，但静态值本身不强制调用者采用某种内存序。

具体状态归实现者所有：`CommandConfig` 从结构体字段计算六个结果；`TiDBConfig` 将五个方法委托给内嵌 `CommandConfig`，但 `GetCheckPointLagLimit` 每次读取 `astersql_sessionctx_vardef::AdvancerCheckPointLagLimit`，因而可观察系统变量热更新。

## 依赖与调用关系

- 上游定义与导出：`config/lib.rs` 声明并再导出本模块；`br/pkg/streamhelper/Cargo.toml`、`br/cmd/br/Cargo.toml` 和 `br/pkg/utiltest/crr/Cargo.toml` 以路径依赖使用该配置 crate。
- 直接实现者：`config/command_conf.rs::impl Config for CommandConfig`、`config/tidb_conf.rs::impl Config for TiDBConfig`、`advancer.rs::impl Config for Cfg`。
- 主要生产调用者：`advancer.rs::CheckpointAdvancer::{getResolveLockInterval,getDefaultStartPollThreshold,getSubscriberErrorStartPollThreshold,isCheckpointLagged}`。
- 下游标准库依赖：`std::time::Duration` 统一所有时间结果；`std::sync::atomic::AtomicI32` 保存可变并发默认值。
- `DefaultMaxConcurrencyAdvance` 的 Go 对照值在 `advancer.go` 创建子范围与 resolve-lock worker pool 时被读取；仓库搜索表明 Rust 侧除定义和 parity 测试外没有读取点，因此当前它是已导出但未接入生产 worker-pool 容量的兼容符号。

RustCodeGraph 的文件节点将 `types.rs` 标为被 `config/tidb_conf.rs` 和 `streamhelper/advancer.rs` 使用；由于精确 `callers Config --file ...` 查询未产出调用边，方法级引用由上述文件和 `rg` 结果补充核验。

## 错误处理与边界

六个 trait 方法均直接返回 `Duration`，契约层没有 `Result`、回退策略或合法性检查。负时间无法由 `Duration` 表示，但零值仍合法，其含义取决于调用方；已确认 `isCheckpointLagged` 把零滞后上限视为不启用检查。

实现计算仍可能遇到边界问题。例如 `CommandConfig` 使用持续时间乘除计算订阅错误阈值和 resolve-lock 间隔，本 trait 不约束溢出或舍入策略。`DefaultMaxConcurrencyAdvance` 允许写入零或负数，且从 `AtomicI32` 转为 worker 数量时如何处理不属于本文件；新增生产读取点前必须明确这一边界。

本文件不会产生 I/O 错误、锁中毒或解析错误。命令行缺 flag 的错误由 `CommandConfig::GetFromFlags` 返回，TiDB 全局量转换由 `tidb_conf.rs` 负责。

## 并发与资源生命周期

`DefaultMaxConcurrencyAdvance` 具有进程级静态生命周期，原子读写避免对该整数本身的数据竞争；调用者仍需自行约定修改时机、内存序和恢复策略。`config/parity_test.rs` 在测试中保存原值、以 `SeqCst` 修改并恢复，但该测试没有专用锁，因此未来增加并发访问测试时应避免共享全局状态互相污染。

`Config` 没有要求 `Send`、`Sync` 或 `'static`，也没有资源释放方法。当前推进器不是保存 `dyn Config`，而是将具体 `Cfg` 放在 `Mutex` 内，因此线程安全和配置替换生命周期由 `CheckpointAdvancer` 的锁管理，不是 trait 保证。所有方法都是同步读取；本文件不会启动任务、持有通道、打开文件或管理网络连接。

## 与 Go 版本的对应关系

Go 对照文件为 `br/pkg/streamhelper/config/types.go`。Rust 的六个 trait 方法与 Go `Config` interface 的六个方法逐项同名，返回值也都表达 `time.Duration`/`Duration`。`DefaultMaxConcurrencyAdvance` 的初始值同为 8。

两处语言适配值得注意：

- Go 使用可直接赋值的包变量 `int`；Rust 使用 `AtomicI32`，修改必须通过 `load`/`store` 等原子操作。`parity_test.rs` 明确验证了这项可变语义。
- Go 推进器已用 `DefaultMaxConcurrencyAdvance` 构建容量为该值及其四倍的 worker pool；当前 Rust 端没有对应读取点。因此“符号和可变性已移植”成立，但“生产并发上限已接线”不成立。

具体实现语义由邻近文件保持对齐：`CommandConfig` 返回字段值，并以 `TryAdvanceThreshold * 9 / 20` 和 `TickDuration * 2` 推导两个阈值；`TiDBConfig` 除滞后上限外均委托给命令配置，滞后上限读取共享 vardef 原子量。`config_test.rs`、`tidb_conf_test.rs` 和 `parity_test.rs` 覆盖这些对应关系。

## 扩展指南

新增推进器配置项时，应先判断它是否真的是两种配置来源共同需要的运行时契约。若是：

1. 在 `Config` 增加返回语义明确的方法。
2. 同步更新 `CommandConfig`、`TiDBConfig` 和 `advancer.rs::Cfg` 三个实现，不能只给某一实现加固有方法。
3. 在 `CheckpointAdvancer` 的实际决策点接入读取，并明确零值、极值、热更新与锁粒度。
4. 同步核对 `types.go` 的 interface；如果 Go/Rust 有意不同，在文档和 parity 测试中记录原因。
5. 测试应继续放在独立文件：通用契约放 `config/parity_test.rs`，Go 同名行为放 `config/config_test.rs`，TiDB 全局量行为放 `config/tidb_conf_test.rs`，推进器效果放 `streamhelper/advancer_test.rs`，不要把测试嵌入 `types.rs`。

若要把 `DefaultMaxConcurrencyAdvance` 接入 Rust 生产路径，应先设计非正值处理、整数转换、读取时机以及运行中修改是否影响已创建 worker；还应为实际并发容量增加独立回归测试，而不能仅依赖原子值可读写测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/streamhelper/config` 列出目标、Go 对照及三个独立 Rust 测试文件；`node --file br/pkg/streamhelper/config/types.rs` 确认文件共 21 行、一个静态值和一个 trait，并标出 `advancer.rs`、`tidb_conf.rs` 使用关系。
- 源码与入口：`br/pkg/streamhelper/config/types.rs`、`br/pkg/streamhelper/config/lib.rs`、`br/pkg/streamhelper/config/command_conf.rs`、`br/pkg/streamhelper/config/tidb_conf.rs`、`br/pkg/streamhelper/advancer.rs`。
- crate 边界：`br/pkg/streamhelper/config/Cargo.toml`，以及三个消费者的 Cargo manifest 搜索结果。
- Go 对照：`br/pkg/streamhelper/config/types.go`、`command_conf.go`、`tidb_conf.go`、`br/pkg/streamhelper/advancer.go`。
- 独立测试：`br/pkg/streamhelper/config/config_test.rs`、`parity_test.rs`、`tidb_conf_test.rs` 与 `br/pkg/streamhelper/advancer_test.rs`；Go 行为参考 `config/config_test.go`。
- 仓库引用搜索确认：Rust 生产代码目前仅通过 `Cfg`/具体配置实现 trait，四个阈值方法进入推进器决策；`DefaultMaxConcurrencyAdvance` 在 Rust 中仅由 parity 测试读取，而 Go 推进器存在两个 worker-pool 读取点。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证恰有 11 个固定二级章节。
