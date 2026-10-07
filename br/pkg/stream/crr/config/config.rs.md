# `br/pkg/stream/crr/config/config.rs`

## 文件定位

本文件是 `astersql-br-pkg-stream-crr-config` crate 的唯一实现模块，由同目录 `lib.rs` 通过 `#[path = "config.rs"]` 挂载并扁平再导出。`Cargo.toml` 将该 crate 标记为 Go 包 `br/pkg/stream/crr/config` 的 Rust 移植，它依赖 CRR checkpoint 内部 crate 与 CRR service crate，但不直接执行 checkpoint 计算、网络访问或服务循环。

它的职责位于命令行和服务配置之间：定义四个 CRR checkpoint 标志，将标志值转换为 `astersql_br_pkg_stream_crr_service::Config`，并从下游 crate 导出的常量组装默认值。当前 Rust 生产 CLI 尚未依赖该 crate；`br/pkg/task/operator/config.rs` 实际通过 `stubs.rs` 中的 `CRRServiceConfig` 和 `DefineCRRFlags` 接线。因此，该文件是已实现、有独立测试的配置 crate，不能说成已经由当前 Rust `br operator crr-checkpoint` 主链调用。

## 核心职责

- `DefaultConfig` 建立唯一的默认值来源：`PollInterval` 和 `MetaReadConcurrency` 来自 checkpoint crate，`RetryInterval` 来自 service crate，任务名由 `CheckpointCalculatorConfig::default()` 保持为空。
- `DefineFlags` 把上述默认值注册成 `task-name`、`retry-interval`、`calc.poll-interval` 和 `calc.meta-read-concurrency` 四个长选项。
- `FlagSet` 提供本 crate 内的 pflag 替身：它保存标志定义和用户覆盖，并接受 `--name value` 与 `--name=value` 两种形式。
- `Config::Parse` 先重置整份配置，再按固定顺序取出四个标志。这保证重复解析不会遗留上一次的字段。
- `parse_duration` 将 Go `time.ParseDuration` 在本地所需的非负子集映射为 `std::time::Duration`，包括复合单位、小数和 Go `int64` 纳秒上界。

## 主要符号

- `flagTaskName`、`flagRetryInterval`、`flagCalcPollInterval`、`flagCalcMetaReadConcurrency`：与 Go 版本完全同名的标志键。它们是命令行兼容面，改名会破坏既有参数。
- `Config { inner: ServiceConfig }`：CLI 层包装。Go 用匿名嵌入 `service.Config`，Rust 则以显式 `inner` 字段持有相同载荷。
- `Config::TaskName`、`RetryInterval`、`PollInterval`、`MetaReadConcurrency`：只读访问器，将外层调用者隔离于 `inner.CalculatorConfig` 的嵌套布局。
- `DefaultConfig() -> Config`：显式默认构造器；`impl Default for Config` 委托给它，避免两套默认值漂移。
- `FlagSet`：三组覆盖 map 与三组默认值 map，按 `String`、`Duration`、`Int` 分类。它是自有内存对象，不持有系统句柄。
- `FlagSet::{String, Duration, Int}`：注册标志及默认值；`_help` 仅保留签名兼容，当前不存储帮助文本。
- `FlagSet::{SetString, SetDuration, SetInt}` 和 `GetString`、`GetDuration`、`GetInt`：分别写入覆盖值和按“覆盖优先，默认其次”读取。
- `FlagSet::Parse(&[&str]) -> Result<(), String>`：本地 argv 解析器，负责长选项形式、类型转换和未知标志错误。
- `parse_duration(&str) -> Result<Duration, String>`：私有的 duration 词法与边界实现。
- `DefineFlags(&mut FlagSet)` 与 `Config::Parse(&FlagSet)`：分别完成“定义”和“落地配置”两阶段。

本文件没有 trait、enum、异步函数或条件编译项。所有标志常量、配置类型和 `FlagSet` 方法均为公开 API；只有 `parse_duration` 是模块私有实现。

## 执行流程

1. 调用者创建 `FlagSet::new()`，然后必须调用 `DefineFlags`。`DefineFlags` 先通过 `DefaultConfig` 取得下游 crate 的当前默认值，再注册四个标志。
2. 调用者可用 `FlagSet::Parse` 解析 argv，或在测试/适配层中直接调用 `Set*`。解析器跳过位置参数，遇到 `--` 立即结束标志解析，拒绝未注册的双横线标志和任何单横线短选项。
3. 对 duration 标志，`parse_duration` 循环读取“整数/小数 + 单位”分量，按 `ns`、`us`/`µs`/`μs`、`ms`、`s`、`m`、`h` 换算为纳秒并累加，每步检查算术溢出和 Go 最大正 `time.Duration`。
4. 调用者创建 `Config`并调用 `Config::Parse`。该方法首先用 `DefaultConfig` 覆盖旧状态，再按任务名、重试间隔、轮询间隔、meta 读并发度的顺序写入 `inner`。
5. 服务构建层理论上可把 `Config::inner` 交给 CRR service；但当前 Rust operator 主链使用的是 `br/pkg/task/operator/stubs.rs` 的平行类型，此连接尚未在 Cargo 依赖中建立。

## 数据与状态

`Config` 本身只是值对象。其 `inner.RetryInterval` 控制 service 层单轮失败后的重试休眠；`inner.CalculatorConfig` 中的 `TaskName` 选择上游日志备份任务，`PollInterval` 控制下游同步检查轮询，`MetaReadConcurrency` 控制 backupmeta 读并发度。这些字段的运行时校正与消费发生在 checkpoint/service crate，不在本文件。

`FlagSet` 为每种类型分别保存“已定义默认值”和“已设置覆盖值”。`Get*` 不会修改 map，`Parse`/`Set*` 才会改变覆盖状态。同名标志被重复定义或设置时，`HashMap::insert` 保留最后一个值；这一简化行为没有模拟 pflag 的全部重复定义诊断。

duration 中间值以 `u128` 累加，最终仅接受 `0..=i64::MAX` 纳秒，然后安全转为 `Duration::from_nanos(u64)`。负 duration 因 `std::time::Duration` 不可表示而明确拒绝；单独的 `0` 是无单位输入的唯一特例。

## 依赖与调用关系

直接下游依赖只有两个：

- `astersql_br_pkg_stream_crr_internal_checkpoint::{CheckpointCalculatorConfig, DefaultPollInterval, DefaultMetaReadConcurrency}` 提供计算器配置形状及默认值。
- `astersql_br_pkg_stream_crr_service::{Config, DefaultRetryInterval}` 提供服务配置载荷和重试默认值。

crate 内上游是 `lib.rs` 的 `pub use config::*`，以及 `config_test.rs`/`parity_test.rs` 对 `DefaultConfig`、`DefineFlags`、`FlagSet::Parse` 和 `Config::Parse` 的直接调用。RustCodeGraph 的文件级反向索引报告了 21 个“used by”文件，但精确 `callers` 查询未在 30 秒内返回结果；因此本文不把其中的广泛符号边当成可靠的业务调用边。

生产接线方面，Go 路径是 `br/cmd/br/operator.go::newCRRCheckpointCommand` → `br/pkg/task/operator/config.go::DefineFlagsForCRRCheckpointConfig` → `crrconfig.DefineFlags`，随后 `CRRCheckpointConfig.ParseFromFlags` 调用 `cfg.CRRConfig.Parse`。Rust 对应 CLI 也有 `newCRRCheckpointCommand` 和 `DefineFlagsForCRRCheckpointConfig`，但后者当前调用 `stubs.rs::DefineCRRFlags`，`CRRCheckpointConfig.CRRConfig` 也是桩类型。对所有 `Cargo.toml` 的检索只找到本 crate 自身的 package 声明，未找到消费者依赖。

## 错误处理与边界

`FlagSet` 以 `Result<_, String>` 返回可读诊断，没有定义专用错误类型。主要失败分支是：单横线短选项、未注册长选项、缺少参数、非法 `i32`、非法 duration，以及对未定义标志的 `Get*`。`--` 之后的全部内容不再解析；普通位置参数被忽略。单个 `"-"` 不符合短选项分支，因而作为位置参数跳过。

duration 边界包括：接受前置 `+`、无整数部分的 `.5s`、复合 `1m30s` 和三种微秒写法；拒绝空串、负值、缺失数字、未知单位、算术溢出，以及超过 `2562047h47m16.854775807s` 的 Go 正 duration 上界。小数换算使用 `f64` 并截断为整数纳秒；若新需求依赖超长小数的逐位 Go 舍入一致性，必须先增加针对性 parity 用例，不应假定当前浮点算法在所有极端输入上逐位相同。

`Config::Parse` 在读取任意一项失败前已经把 `self` 重置并可能写入了前面的字段，所以错误不提供事务性回滚；这与 Go `Config.Parse` 的顺序赋值语义一致。调用者必须在 `Err` 时废弃该配置，不能继续使用部分值。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、网络连接或文件句柄。`Config` 和 `FlagSet` 都是 `Clone` 的自有值，由 Rust RAII 在作用域结束时释放 `String` 和 `HashMap`。

`FlagSet::Parse` 需要 `&mut self`，`Config::Parse` 也需要 `&mut self`，所以编译期借用规则防止同一实例的无同步并发写入。若上层需要跨线程共享或热更新，应由上层在调用本 API 之外选择 `Mutex`/`RwLock` 或配置快照；不应在此无 IO 的配置模块中引入隐式全局同步。运行时轮询、重试、锁和取消生命周期属于 checkpoint/service 实现，而非本文件。

## 与 Go 版本的对应关系

Rust 的四个标志名、`DefaultConfig` 字段来源、`DefineFlags` 帮助文本和 `Config::Parse` 赋值顺序都直接对应 `br/pkg/stream/crr/config/config.go`。Go `Config` 匿名嵌入 `service.Config`；Rust 为了明确所有权使用 `inner`，并增加四个访问器来恢复常用字段的扁平读取体验。

Go 直接使用 `spf13/pflag.FlagSet`，其 argv 语法和重复定义行为更完整。Rust 则在本文件定义一个最小 `FlagSet`，仅覆盖本包需要的 string/duration/int 长选项。Go `time.Duration` 是有符号 64 位纳秒，Rust `Duration` 是非负值；所以 Rust 支持 Go 的非负语法并主动施加 `i64::MAX` 上界，但不支持负 duration。

`config_test.go::TestDefaultConfig` 和 `TestParse` 在 `config_test.rs` 中有直接对应用例。Rust 还额外测试等号形式、复合 duration、未知标志、非法数值、`--` 终止、短选项拒绝、Go duration 正上界以及 `.5s`/`+1s`。`parity_test.rs::go_rust_public_contract_matches` 另外锁定默认任务名、纯默认解析和未定义 getter 错误。

最重要的移植差异是接线状态：Go operator 直接导入并消费本 Go 包；Rust operator 仍使用自己的桩结构和标志定义。扩展时必须把“目标 crate 本身行为正确”与“整体 Rust CLI 已切换到此 crate”分开验证。

## 扩展指南

- 增加新的 CRR 配置项时，应同步更新标志常量、`ServiceConfig`/`CheckpointCalculatorConfig` 载荷、`DefaultConfig`、`DefineFlags` 和 `Config::Parse`，并在独立的 `config_test.rs` 与 `parity_test.rs` 增加默认值、覆盖值、错误路径测试。不应把 Rust 测试内嵌回 `config.rs`。
- 修改标志名、帮助文本或默认值时，要对照 `config.go`、`config_test.go`及 checkpoint/service 中的导出常量，避免 CLI 兼容性和跨 crate 默认值漂移。
- 扩充 `FlagSet::Parse` 前要区分“本 crate 必需的 pflag 语义”与“完整 pflag 克隆”。引入 bool、短选项、重复定义检查或位置参数保留时，先补 Go/Rust 对照用例。
- 修改 `parse_duration` 要重点保护复合单位、Unicode 微秒、分数精度、`i64::MAX` 边界与负值拒绝。这是兼容性风险最高的纯计算部分，不应为了简化而删减 Go 已支持的语法。
- 若要让当前 Rust operator 主链真正使用本 crate，需在 `br/pkg/task/operator/Cargo.toml` 增加依赖，用本 crate 的 `Config`/`DefineFlags` 替换 `stubs.rs::CRRServiceConfig`/`DefineCRRFlags`，并核对 `br/cmd/br/operator.rs` 到真实 service 的类型转换。这是跨 crate 的生产接线任务，不属于本文档任务，也不应仅靠删除桩就宣称完成。
- 性能上本模块只解析少量参数，`HashMap` 分配和字符串克隆不是热点。更值得关注的风险是配置默认值漂移、Go/Rust 解析差异和 operator 接线重复。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/stream/crr/config` 确认该目录的 Rust/Go 实现与测试集合。
- RustCodeGraph `node --file br/pkg/stream/crr/config/config.rs --offset 1 --limit 500`：读取全部 353 行，核对 31 个索引符号、字段、分支与边界。
- RustCodeGraph `query DefaultConfig --kind function --json` 和 `query DefineFlags --kind function --json`：区分目标 Rust 符号与 Go/其他同名符号。针对精确 Rust 符号的 `callers`/`callees` 命令在 30 秒限制内未产生输出，因此用下述 Cargo 和文本接线检索补证，未据此猜测调用边。
- RustCodeGraph 全文节点：`config_test.rs`、`parity_test.rs`、`lib.rs`、`br/pkg/stream/crr/service/service.rs`、`br/pkg/stream/crr/internal/checkpoint/calculator.rs`、`br/pkg/task/operator/config.rs`、`br/cmd/br/operator.rs` 和 `br/pkg/task/operator/stubs.rs`。它们分别支撑公开出口、测试边界、下游配置语义和当前 Rust CLI 接线状态。
- 直接读取：`br/pkg/stream/crr/config/Cargo.toml`、`config.go`、`config_test.go`、`br/pkg/task/operator/config.go` 和 `br/cmd/br/operator.go`，用于核对 crate 边界、Go 实现、Go 测试与生产调用链。
- `rg` 检索所有 Rust/Go/Cargo 引用：确认 Rust 测试的直接调用，确认 Go operator 使用 `crrconfig`，并确认其他 Cargo manifest 尚未声明 `astersql-br-pkg-stream-crr-config` 依赖。
- 结构验证按任务规定检查文件存在且恰有十一个固定二级标题。本任务只生成文档，按计划不运行 Cargo。
