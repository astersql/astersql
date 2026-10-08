# `pkg/plugin/errors.rs`

## 文件定位

[`errors.rs`](errors.rs) 是 `astersql-plugin` crate 的统一错误模型。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod errors` 声明该模块，并通过 `pub use errors::*` 将 `PluginError` 与 `PluginErrorKind` 暴露到 crate 根；因此同 crate 的加载、校验、审计事件解析和 SPI 回调都能使用同一种 `Result<_, PluginError>` 边界。

该文件只定义错误分类、错误载荷和标准 trait 适配，不执行插件加载，也不持有插件状态。实际错误产生点主要位于 [`helper.rs`](helper.rs)、[`plugin.rs`](plugin.rs) 和 [`audit.rs`](audit.rs)。所属 crate 由 [`Cargo.toml`](Cargo.toml) 定义为 `astersql-plugin`，库入口为 `lib.rs`；本文件只依赖 Rust 标准库 `std::fmt`，没有 feature 条件或外部运行时依赖。

## 核心职责

- `PluginErrorKind` 给调用方一个可直接比较的失败类别，覆盖插件 ID、manifest、名称、版本、重复注册、环境版本校验、查找、Ready 状态、Flush 能力和通用后端失败。
- `PluginError` 同时保存机器可判别的 `kind` 和面向日志/调用方的 `message`，避免仅靠字符串区分错误。
- `PluginError::new` 是所有明确分类错误的统一构造入口；`PluginError::backend` 是 `Backend` 类错误的简写。
- `fmt::Display` 只输出 `message`，使 `to_string()`、格式化日志和上层断言看到原始可读消息；`std::error::Error` 实现使该类型可进入标准 Rust 错误链容器。

## 主要符号

### `pub enum PluginErrorKind`

该枚举派生 `Clone + Copy + Debug + PartialEq + Eq`，没有附带数据，适合按值复制和在测试或调用方中直接比较。十个成员的当前语义如下：

| 成员 | 当前生产代码中的典型来源 |
| --- | --- |
| `InvalidPluginId` | `Id::decode` 找不到最后一个 `-` 分隔符（`helper.rs`） |
| `InvalidPluginManifest` | `load_one` 无可用 loader，模拟动态库打开失败（`plugin.rs`） |
| `InvalidPluginName` | manifest 导出的名称与插件 ID 名称不同（`plugin.rs`） |
| `InvalidPluginVersion` | 非静态插件的 manifest 版本与 ID 版本文本不同（`plugin.rs`） |
| `DuplicatePlugin` | 静态注册表重名，或一次 `load` 中出现重复插件名（`plugin.rs`） |
| `RequiredVersionCheckFailed` | `Plugin::validate` 发现组件实际版本低于 manifest 要求（`plugin.rs`） |
| `PluginNotFound` | Flush 前按名称找不到插件（`plugin.rs::supports_flush`） |
| `PluginNotReady` | Flush 前插件状态不是 `State::Ready`（`plugin.rs::supports_flush`） |
| `FlushUnsupported` | Flush 前插件没有 watcher（`plugin.rs::supports_flush`） |
| `Backend` | 锁中毒、回调/存储后端失败的便捷包装，以及无法归入专门类别的错误；`audit.rs::FromStr` 的非法事件字符串当前也归入此类 |

枚举没有稳定整数表示或序列化实现；成员顺序不应被当作协议编号。

### `pub struct PluginError`

结构体公开 `kind: PluginErrorKind` 与 `message: String` 两个字段，并派生 `Clone + Debug + PartialEq + Eq`。公开字段允许调用方同时检查类别和消息，但也意味着当前没有构造器级不变量能阻止调用方直接组合任意类别与文本。

### `PluginError::new`

签名为 `pub fn new(kind: PluginErrorKind, message: impl Into<String>) -> Self`。它保留传入分类，并把 `&str`、`String` 等输入一次转换为自有 `String`；不添加前缀、错误码、来源、调用栈或 cause。

### `PluginError::backend`

签名为 `pub fn backend(message: impl Into<String>) -> Self`，等价于 `PluginError::new(PluginErrorKind::Backend, message)`。`plugin.rs` 用它转换全局/静态注册表 `RwLock` 中毒，测试也用它构造消息为 `EOF` 的回调失败。

### `Display` 与 `Error`

`Display::fmt` 调用 `Formatter::write_str(&self.message)`，输出与 `message` 完全一致。空实现的 `std::error::Error` 没有覆盖 `source()`，所以本类型自身不暴露下游错误链。

## 执行流程

1. 业务入口先在 `helper.rs`、`plugin.rs` 或 `audit.rs` 识别失败条件。
2. 有明确领域分类时调用 `PluginError::new(kind, message)`；锁或通用后端失败可调用 `PluginError::backend(message)`。
3. 构造器取得消息所有权，生成只包含 `kind` 与 `message` 的值，不做 I/O、记录日志或修改全局状态。
4. 错误通过 `Result<T, PluginError>` 和 `?` 沿调用链上抛。例如 `Id::decode -> load_one -> load`，以及 `supports_flush -> notify_flush/change_disable_flag_and_flush`。
5. 若上层格式化错误，`Display` 原样写出消息；若按领域处理，则直接读取或比较公开的 `kind`。

`load` 有一个重要上层分支：`Config::skip_when_fail` 为真时，部分加载或校验错误会被跳过/转成禁用状态，而不是继续返回；这是 `plugin.rs` 的策略，不是本文件对错误做了吞并。

## 数据与状态

本文件唯一运行时数据是每个 `PluginError` 值中的枚举和拥有所有权的字符串。`PluginErrorKind` 是无负载、可复制的值；`PluginError` 因含 `String` 只能克隆，克隆会复制消息内容。

这里没有全局变量、缓存、注册表、环境读取或隐式状态。类型没有时间戳、插件 ID 独立字段、错误码、backtrace、下游 source 或重试标记；这些信息若需要保留，目前只能编码在 `message` 中或由上层另行维护。

## 依赖与调用关系

上游模块通过 `lib.rs` 的公开重导出消费该类型：

- `helper.rs::Id::decode` 产生 `InvalidPluginId`；`load_plugin_for_test` 继续传播加载/初始化错误。
- `plugin.rs` 将 `PluginError` 用作 `TestLoadHook`、`PluginLoader`、`KeyValueClient`、生命周期回调、加载/初始化/遍历/Flush API 的共同错误类型。主要构造点位于 `Plugin::validate`、`register_static_plugin`、`load`、`load_one` 和 `supports_flush`。
- `audit.rs::general_event_from_string` / `FromStr for GeneralEvent` 在无法解析事件字符串时返回 `Backend`。
- `spi.rs` 的初始化回调类型返回 `Result<(), PluginError>`；示例插件 `conn_ip_example.rs` 也沿用该公共边界。

下游依赖只有 `std::fmt`：`Display` 使用 `fmt::Formatter`/`fmt::Result`，随后实现标准 `std::error::Error`。RustCodeGraph 能精确定位 `PluginErrorKind` 与 `PluginError` 节点及目标文件源码，但对类型引用没有返回 call edge；上述直接使用关系由目标目录内的符号引用检索补证，不能把图中泛名 `new`/`Error` 的跨仓库结果当作本类型调用边。

## 错误处理与边界

- 分类和值都没有自动校验：直接结构体字面量可让类别与消息不一致，安全扩展应优先使用构造器并在生产点选择准确分类。
- `Display` 有意不显示 `kind`；只断言字符串的调用方无法区分相同消息的不同类别。需要分支处理时应检查 `kind`。
- `PluginError` 不包装原始错误，`Error::source()` 默认为 `None`。当前 `map_err` 多把锁中毒等原因压缩成固定消息，原始类型和调用栈不会保留。
- `Backend` 是兜底类别，覆盖范围较宽。新增稳定、可恢复的领域失败若仍放入 `Backend`，会削弱上层精确处理能力。
- 三个 Flush 前置类别只描述本地检查结果；真正的 `KeyValueClient::put` 错误由实现直接返回，类别由后端实现决定。
- 该文件没有 panic 路径。`notify_flush` 中检查后的 `expect` 属于 `plugin.rs` 的内部不变量，不是错误类型自身的行为。

## 并发与资源生命周期

`errors.rs` 不创建线程、任务、锁、通道、文件句柄或网络连接，错误值也不借用外部资源，因此没有清理顺序和异步生命周期。其字段由普通值组成；文件没有显式 `Send`/`Sync` 实现或负实现，线程传递能力由 Rust 对 `PluginErrorKind` 和 `String` 的自动 trait 推导决定。

并发相关失败来自调用者：`plugin.rs` 在全局插件集合、静态注册表或测试 hook 的 `RwLock` 中毒时构造 `Backend`。构造错误不会恢复锁，也不会更改插件生命周期；上层决定返回、跳过、禁用或终止当前操作。

## 与 Go 版本的对应关系

Go 对照文件 [`errors.go`](errors.go) 使用 `dbterror.ClassPlugin.NewStd(errno.*)` 定义六个包内标准错误：非法 ID、manifest、名称、版本、重复插件和版本要求检查失败。Rust 的前六个专门枚举成员与这些使用场景逐项对应；`helper.go::ID.Decode`、`plugin.go::Plugin.validate`、`plugin.go::Load/loadOne/loadManifestByGoPlugin` 是相应生产点。

两侧错误表示并不等价：Go 标准错误带 TiDB errno、模板参数，并可通过 `GenWithStackByArgs` 生成带栈实例；Rust 只保存类别和最终字符串，没有 errno、模板参数和栈。Rust 的 `PluginNotFound`、`PluginNotReady`、`FlushUnsupported` 对应 Go `plugin.go::supportsFlush` 中三个 `errors.Errorf` 分支，而不是 `errors.go` 中的标准错误变量。Rust 的 `Backend` 还统一承接 Rust 锁中毒、测试/回调错误及其他后端失败，Go 没有同名的本地标准错误。

因此，语义对齐应比较失败条件和用户可见消息，而不能宣称 Rust 已保留 Go `dbterror` 的错误码/错误栈身份。当前 Rust `load_one` 在没有 loader 时直接构造 `InvalidPluginManifest` 并模拟 `plugin.Open` 文案，而 Go 的真实 `gplugin.Open` 失败直接传播；这是调用实现层差异，错误模型仅承载结果。

## 扩展指南

- 新增稳定错误场景时，先判断它是否需要调用方分支处理；需要时在 `PluginErrorKind` 增加专门成员，并同步修改所有生产点，避免继续扩大 `Backend`。
- 若要求与 Go errno 身份严格兼容，应显式设计错误码/参数字段及转换规则，而不是从 `message` 反解析；这会影响 `PluginError` 公共结构、构造器、`Display` 和调用方，属于兼容性变更。
- 修改既有消息前应搜索字符串断言和外部日志消费者。当前 `plugin_test.rs` 明确断言模拟 `plugin.Open` 文案以及 `EOF` 的 `Display` 透传。
- 测试应放在独立文件，不能嵌入 `errors.rs`。可新增同目录 `errors_test.rs` 并在 `lib.rs` 以 `#[cfg(test)] #[path = "errors_test.rs"]` 挂载；若只验证现有生产路径，则扩展 `helper_test.rs` 或 `plugin_test.rs` 更贴近行为所有者。新增测试应覆盖构造器分类、`backend` 固定分类、`Display` 原样消息和新增成员的真实生产分支。
- 增加字段会影响公开结构体字面量、`Clone/PartialEq/Eq` 语义和潜在内存成本；增加错误链时需决定 clone/equality 如何处理 source。当前错误不在热循环中主动分配额外数据，但每次构造消息都会形成一个拥有所有权的 `String`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/plugin` 列出 31 个 Go/Rust 文件；`query PluginError --json` 精确得到 `pkg/plugin/errors.rs:50:struct:PluginError`，`query PluginErrorKind --json` 得到 `pkg/plugin/errors.rs:25:enum:PluginErrorKind`；`node --file pkg/plugin/errors.rs --offset 1 --limit 180` 核对了完整 78 行源码。对这两个类型节点执行 `callers`/`callees` 没有返回类型引用边，因此使用目录内精确符号检索补充调用证据。
- Rust 源与配置：`pkg/plugin/errors.rs`、`pkg/plugin/lib.rs`、`pkg/plugin/Cargo.toml`、`pkg/plugin/helper.rs`、`pkg/plugin/plugin.rs`、`pkg/plugin/audit.rs`、`pkg/plugin/spi.rs`、`pkg/plugin/conn_ip_example/conn_ip_example.rs`。
- Go 对照：`pkg/plugin/errors.go`、`pkg/plugin/helper.go`、`pkg/plugin/plugin.go`；其中 `errors.go` 证明六个标准 `dbterror` 定义，`plugin.go::supportsFlush` 证明另外三个 Flush 前置失败是格式化错误。
- 独立测试：`pkg/plugin/helper_test.rs::test_decode` 与 `pkg/plugin/helper_test.go::TestDecode` 验证无分隔符 ID 失败；`pkg/plugin/plugin_test.rs` 的加载用例覆盖错误传播、模拟打开失败文案和 `PluginError::backend("EOF")` 的显示结果，对照 `pkg/plugin/plugin_test.go`。同目录不存在独立 `errors_test.rs`，现有测试也未逐项断言 `PluginErrorKind`。
- 本任务是只新增说明文档的静态分析；按计划不运行 Cargo。交付前使用任务指定命令确认本文恰有 11 个固定二级标题，并人工复核所有现状结论均能回指上述符号或文件。
