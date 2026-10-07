# `br/pkg/streamhelper/config/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-streamhelper-config` 的 crate 根。[`Cargo.toml`](Cargo.toml) 通过 `[lib] path = "lib.rs"` 指定该入口，并以 `package.metadata.porting.go-package = "br/pkg/streamhelper/config"` 标明对应的 Go 包；根 [`Cargo.toml`](../../../../Cargo.toml) 将本目录列为 workspace 成员。

该文件是配置子系统的编译期门面，不直接实现配置计算。它公开挂载 [`command_conf.rs`](command_conf.rs)、[`tidb_conf.rs`](tidb_conf.rs)、[`types.rs`](types.rs)，再用三条 glob `pub use` 将其公开项扁平导出，使 Rust 调用方能够像使用 Go 单包 API 一样，从 crate 根取得 `Config`、`CommandConfig`、`TiDBConfig`、默认值、构造函数和 flag 辅助函数。三个独立测试文件仅在 `cfg(test)` 下挂载。

## 核心职责

- 用显式 `#[path]` 把三个生产模块纳入 crate，并公开模块路径，允许调用者按 `command_conf::...`、`tidb_conf::...` 或 `types::...` 精确访问。
- 用 `pub use command_conf::*`、`pub use tidb_conf::*`、`pub use types::*` 形成根级公共 API；本层不包装、不复制值，也不改变下游函数的错误或状态语义。
- 在测试构建中挂载 [`config_test.rs`](config_test.rs)、[`parity_test.rs`](parity_test.rs)、[`tidb_conf_test.rs`](tidb_conf_test.rs)，保持 Rust 生产逻辑与测试逻辑分文件。
- 通过 crate 级 `allow` 容纳 Go 风格的导出命名及迁移期未使用项；这属于 lint 兼容策略，不改变运行时行为。

真实逻辑分别位于：`types.rs` 的统一 `Config` trait 与并发默认值，`command_conf.rs` 的 CLI 配置/default/flag 解析，`tidb_conf.rs` 的 TiDB 嵌入配置与全局系统变量读取。

## 主要符号

- `pub mod command_conf`：导出五个 duration flag 名、五个默认 duration、`FlagSet`、`CommandConfig`、`DefineFlagsForCheckpointAdvancerConfig()`、`DefaultCommandConfig()` 及 `Config for CommandConfig`。
- `pub mod tidb_conf`：导出 `TiDBConfig`、`DefaultTiDBConfig()` 和兼容测试/调用方使用的 `AdvancerCheckPointLagLimitNanos`；`ADVANCER_LAG_LIMIT_TEST_LOCK` 为 `cfg(test)` 且 `pub(crate)`，不会成为生产公共 API。
- `pub mod types`：导出六方法 `Config` trait，以及以 `AtomicI32` 表示的 `DefaultMaxConcurrencyAdvance`。
- `config_test`、`parity_test`、`tidb_conf_test`：私有、仅测试模块，分别验证 Go 同名测试语义、根级公开契约及 TiDB 全局 lag-limit 的共享读取。
- 三条 glob 再导出：没有新建符号实现，但决定了 crate 根的兼容 API。未来子模块新增 `pub` 项会自动扩大根级公共面，并可能引发重名冲突。

`lib.rs` 自身不声明常量、结构体、trait、函数或 `impl`；唯一条件编译项是三个测试模块。

## 执行流程

1. Cargo 以 `lib.rs` 为 crate 根，依次加载三个生产模块；普通构建跳过三个 `cfg(test)` 模块。
2. 编译器将三个模块的公开项再导出到 crate 根；这一阶段没有运行时代码。
3. BR CLI 的 [`br/cmd/br/stream.rs`](../../../cmd/br/stream.rs) 从根路径调用 `DefineFlagsForCheckpointAdvancerConfig()`，为隐藏的 `br log advancer` 子命令注册默认值；执行该子命令时调用 `DefaultCommandConfig()`，再以 `CommandConfig::GetFromFlags()` 顺序覆盖五个字段。
4. [`br/pkg/streamhelper/advancer.rs`](../advancer.rs) 通过根路径导入 `CommandConfig`、`TiDBConfig`、`Config` 及两个默认构造器。`NewCommandCheckpointAdvancer()` 选择命令配置，`NewTiDBCheckpointAdvancer()` 选择 TiDB 配置；内部 `Cfg` 枚举把六个 trait 查询转发到当前变体。
5. 推进器运行时读取退避、tick 超时、正常/订阅错误轮询阈值、resolve-lock 间隔和 checkpoint lag 上限。TiDB 变体只有 lag 上限走进程级 `vardef.AdvancerCheckPointLagLimit`，其他方法委托内嵌 `CommandConfig`。

## 数据与状态

门面文件本身没有实例状态。再导出的 `CommandConfig` 持有五个 `Duration` 字段：退避、tick、主动推进阈值、checkpoint lag 上限及 ownership 轮转周期。默认值分别是 5 秒、12 秒、4 分钟、48 小时和 0；`FlagSet` 当前是 `HashMap<&'static str, Duration>` 的轻量 pflag 替代。

`TiDBConfig` 持有一份 `CommandConfig`。其 checkpoint lag 不读取内嵌字段，而是每次从 `astersql_sessionctx_vardef::AdvancerCheckPointLagLimit` 读取纳秒值，因而能观察系统变量更新。`AdvancerCheckPointLagLimitNanos` 只是指向同一 vardef 原子量的兼容视图，不是第二份状态。

`DefaultMaxConcurrencyAdvance` 是进程级 `AtomicI32`，默认 8；这保留 Go 可变包变量的运行时调整能力，同时避免 Rust 数据竞争。测试锁 `ADVANCER_LAG_LIMIT_TEST_LOCK` 只串行化会改动全局 lag-limit 的测试，生产构建不存在该锁。

## 依赖与调用关系

本 crate 的 manifest 只有一个仓库依赖：`astersql-sessionctx-vardef`，供 `tidb_conf.rs` 读取和写入 `AdvancerCheckPointLagLimit`。其余实现只使用标准库的 `Duration`、`HashMap` 和原子类型。

明确的 Rust 上游有两类：

- [`br/pkg/streamhelper/Cargo.toml`](../Cargo.toml) 依赖本 crate；其 [`advancer.rs`](../advancer.rs) 用 `Config` 抽象统一命令模式与 TiDB 模式，并在构造、轮询、resolve lock、超时和暂停判定路径消费配置。
- [`br/cmd/br/Cargo.toml`](../../../cmd/br/Cargo.toml) 依赖本 crate；其 [`stream.rs`](../../../cmd/br/stream.rs) 注册 advancer flags，将解析后的 `CommandConfig` 字段复制进任务层 `AdvancerCommandConfig`。

RustCodeGraph 的精确文件节点显示 `types.rs` 被 `advancer.rs` 和 `tidb_conf.rs` 使用，`command_conf.rs` 被 CLI、advancer 及测试等 8 个文件使用；`lib.rs` 的图级 “used by” 只报告 `tools/tazel/parity_test.rs`，说明索引不会把 Cargo crate 导入完整归并为 crate 根调用边。因此上游关系同时以 consumer manifests 和精确源码导入核验，未把宽泛同名查询结果当作证据。

## 错误处理与边界

`lib.rs` 不产生、捕获或转换错误。主要错误边界来自再导出的 `CommandConfig::GetFromFlags()`：五个键按固定顺序读取，首个缺失键即返回 `Result<(), String>`；此前已经赋值的字段不会回滚，所以失败不是事务性的。`parity_test.rs` 固化了缺 `try-advance-threshold` 时前两个字段已更新、后续字段保留原值的行为。

当前 Rust `FlagSet` 仅保存 duration，不等价于完整 `pflag.FlagSet`；Go 版还为每个 flag 保存帮助文本并隐藏 `ownership-cycle-interval`，Rust 门面及轻量 map 没有这些元数据能力。Rust 的 `DefaultCommandConfig()` 返回具体值，而 Go 返回 `Config` interface 中的指针；调用方不应据此假设跨语言对象身份或可空语义相同。

`Duration` 和纳秒转换还存在表示边界：`TiDBConfig::GetCheckPointLagLimit()` 将 vardef 的 `i64` 值转为 `u64` 后构造 duration，兼容视图的 `store(u64)` 又转为 `i64`。当前代码没有在此层校验负值或超范围转换，安全输入依赖系统变量路径及调用方约束。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道、事务或 I/O，也不拥有需要显式关闭的资源；模块声明和再导出完全发生在编译期。

再导出的普通 `CommandConfig`、`TiDBConfig` 和 `FlagSet` 由调用方按 Rust 所有权管理，离开作用域即释放。进程级并发状态只有两个原子量：`DefaultMaxConcurrencyAdvance` 和 vardef 的 `AdvancerCheckPointLagLimit`。后者的兼容 `load`/`store` 参数接受 `Ordering`，但实现有意忽略该参数并委托 vardef 的 `Load`/`Store`；不能从调用处传入的 ordering 推导更强同步保证。

测试对共享 lag-limit 的修改由 `ADVANCER_LAG_LIMIT_TEST_LOCK` 串行化并在断言后恢复默认值/原值。新增会写该全局量的独立测试也应复用此锁并恢复状态，避免并行测试串扰。

## 与 Go 版本的对应关系

Go 包没有独立 `lib.rs`；Rust 用本 crate 根模拟 Go 的包级命名空间。三个生产模块一一对应 [`command_conf.go`](command_conf.go)、[`tidb_conf.go`](tidb_conf.go)、[`types.go`](types.go)，根级 glob 再导出让调用方维持接近 `config.X` 的使用方式。

核心语义保持一致：五个默认 duration、六方法配置接口、订阅异常阈值 `TryAdvanceThreshold * 9 / 20`、resolve-lock 间隔 `TickDuration * 2`，以及 TiDB 配置从全局 vardef 动态读取 checkpoint lag。Go [`config_test.go`](config_test.go) 的 `TestCheckPointLimit` 和 `TestResolveLockInterval` 对应 Rust [`config_test.rs`](config_test.rs) 的两个测试；Rust [`tidb_conf_test.rs`](tidb_conf_test.rs) 直接证明读取的是共享 vardef 原子量，`parity_test.rs` 补充了公开 API、委托和错误副作用契约。

已确认的实现差异是：Go 使用真实 `pflag.FlagSet`、接口返回值和嵌入指针，Rust 使用 duration `HashMap`、具体值返回和具名字段委托；Go 通过 SQL `SET GLOBAL` 测试热更新，Rust 本 crate 因不拥有 session/domain，直接修改同一个 vardef 原子量来验证最终效果。不能把 Rust 单元测试写成已覆盖完整 SQL 系统变量链路。

## 扩展指南

- 新增配置项时，应在真实归属模块中加入字段、默认值和 trait 行为；若属于 CLI，还要同步 `DefineFlagsForCheckpointAdvancerConfig()` 与 `GetFromFlags()` 的读取顺序，并更新独立的 `config_test.rs`/`parity_test.rs` 及 Go 对照。
- 新增实现模块时，在本文件添加明确的 `#[path] pub mod`；是否 glob 再导出应审查根级命名冲突和公共 API 扩张。测试继续放在独立 `*_test.rs` 文件并以 `cfg(test)` 挂载，不能内嵌到生产文件。
- 修改 `Config` trait 会影响 `CommandConfig`、`TiDBConfig` 和 `advancer.rs` 的内部 `Cfg` 三处实现；还需检查所有 trait 消费点，不能用默认桩掩盖遗漏。
- 修改 lag-limit 热更新语义时，应保持 `TiDBConfig` 与 `CommandConfig` 的差异：前者读共享 vardef，后者保留自身字段；同步验证全局状态恢复和并行测试隔离。
- 若用真实 CLI parser 替换 `FlagSet` 桩，应保留 Go 的默认值、首错返回及逐字段副作用，并补齐帮助文本、隐藏 chaos flag 和非法 duration 的独立测试。
- 性能风险主要来自推进器调用频率而非门面：普通字段读取廉价，TiDB lag-limit 是原子读取。扩展时避免在 trait getter 中引入阻塞 I/O、长锁或分配，否则会进入 advancer tick 热路径。

## 验证依据

- RustCodeGraph：`status` 确认现有索引可用；`files --filter br/pkg/streamhelper/config` 定位目标目录；`node --file` 完整读取 `lib.rs`、`types.rs`、`command_conf.rs`、`tidb_conf.rs`，并读取 `advancer.rs` 与 `br/cmd/br/stream.rs` 的直接消费段。精确 `query` 定位 `CommandConfig`、`DefaultCommandConfig`、`DefineFlagsForCheckpointAdvancerConfig`、`DefaultTiDBConfig` 和 `GetCheckPointLagLimit`；精确 callers/callees 未返回边，故没有虚构图边。
- crate 边界：本目录 [`Cargo.toml`](Cargo.toml)、根 [`Cargo.toml`](../../../../Cargo.toml)、consumer manifests [`br/pkg/streamhelper/Cargo.toml`](../Cargo.toml) 与 [`br/cmd/br/Cargo.toml`](../../../cmd/br/Cargo.toml)。
- Rust 生产源码：[`lib.rs`](lib.rs) 的三个公开模块、三个 `cfg(test)` 模块和三条 glob 再导出；以及三个实现文件和 [`advancer.rs`](../advancer.rs)、[`br/cmd/br/stream.rs`](../../../cmd/br/stream.rs) 的直接接线。
- Rust 独立测试：[`config_test.rs`](config_test.rs)、[`parity_test.rs`](parity_test.rs)、[`tidb_conf_test.rs`](tidb_conf_test.rs)；另有上游 [`advancer_test.rs`](../advancer_test.rs) 使用根级 `CommandConfig`、`Config`、`DefaultCommandConfig`。
- Go 对照：[`command_conf.go`](command_conf.go)、[`tidb_conf.go`](tidb_conf.go)、[`types.go`](types.go)、[`config_test.go`](config_test.go)，以及 CLI 对照 `br/cmd/br/stream.go`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定结构命令，要求文档存在且恰有 11 个固定二级标题，并人工复核链接、当前接线、Go/Rust 差异和未验证边界。
