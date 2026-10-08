# [`pkg/plugin/const.rs`](./const.rs)

## 文件定位

`pkg/plugin/const.rs` 属于 `astersql-plugin` crate。crate 入口 `pkg/plugin/lib.rs` 以 `pub mod r#const` 声明本模块，并通过 `pub use r#const::*` 将其中的 `Kind` 和 `State` 重导出到 crate 根，因此同 crate 的 `plugin.rs`、`spi.rs`、`helper.rs` 以及外部使用者都可以写成 `crate::Kind` / `crate::State`。

这个文件只定义插件框架共享的两个值域及其显示文本，不负责装载动态库、保存全局插件集合或执行回调。`pkg/plugin/Cargo.toml` 表明它所在的包名为 `astersql-plugin`、库入口为 `lib.rs`，并通过 `[package.metadata.porting] go-package = "pkg/plugin"` 指向对应 Go 包。

## 核心职责

1. `Kind`（`const.rs:24`）把插件清单划分为审计、认证、Schema 和守护任务四类，供清单声明、按类存储及按类分发使用。
2. `State`（`const.rs:56`）描述插件实例的四个生命周期状态：未初始化、就绪、即将退出、禁用。
3. 两个枚举各自通过 `as_str` 和 `Display` 提供稳定、区分大小写的协议文本，使日志、状态值和测试与 Go 版本的 `String()` 结果一致。
4. `#[repr(u8)]` 固定枚举判别值的底层表示：`Kind` 从 1 开始，依次为 1–4；`State` 从 0 开始，依次为 0–3。当前文件没有提供整数到枚举的解析或不安全 FFI 转换接口。

## 主要符号

- `pub enum Kind`：公开插件种类。派生 `Clone`、`Copy`、`Debug`、相等/排序和 `Hash`；其中 `Hash` 是 `pkg/plugin/plugin.rs:282` 的 `HashMap<Kind, Vec<Plugin>>` 能按类型索引插件的直接要求。`Audit = 1` 显式锁定首值，后续 `Authentication`、`Schema`、`Daemon` 按声明顺序递增。它没有 `Default`，调用者必须明确选择插件类别。
- `Kind::as_str(self) -> &'static str`：纯匹配函数，分别返回 `"Audit"`、`"Authentication"`、`"Schema"`、`"Daemon"`。返回值来自静态字符串，不分配内存。
- `impl Display for Kind`：把 `as_str()` 写入格式化器；`to_string()` 等标准格式化入口因此复用同一映射。
- `pub enum State`：公开生命周期状态。派生 `Clone`、`Copy`、`Debug`、`PartialEq`、`Eq` 和 `Default`；`#[default] Uninitialized` 使默认状态明确为未初始化。它不派生 `Hash` 或排序，因为当前代码仅比较和展示状态，不把它作为集合键。
- `State::as_str(self) -> &'static str`：返回 `"Uninitialized"`、`"Ready"`、`"Dying"`、`"Disable"`。
- `impl Display for State`：将上述静态文本交给格式化器。`pkg/plugin/plugin.rs:245` 的 `Plugin::state_value` 通过该实现生成 `{State}-enable|disable`。

## 执行流程

这两个类型没有主动执行入口；它们作为数据被插件主流程消费：

1. 清单创建时，`pkg/plugin/spi.rs:129` 的 `Manifest::new(kind, name, version)` 保存调用者给出的 `Kind`。`Manifest::default` 在 Rust 中以 `Kind::Audit` 作为空清单占位值。
2. 装载时，`pkg/plugin/plugin.rs:292` 的 `Plugins::add` 从 `Plugin::kind()` 取得清单种类，并将实例追加到 `by_kind: HashMap<Kind, Vec<Plugin>>`。
3. `Plugin::new`（`plugin.rs:213`）总是把新实例状态设为 `State::Uninitialized`。`load` 的校验失败分支可将其设为 `Disable`（`plugin.rs:387`）。
4. `init` 执行初始化回调和可选 watcher 初始化；可跳过的失败变为 `Disable`，成功路径最终变为 `Ready`（`plugin.rs:467–495`）。
5. `foreach_plugin` 和 `is_enabled` 先按 `Kind` 选择分组，再只接受 `State::Ready` 且原子禁用标志不为 1 的实例（`plugin.rs:541–573`）。例如 `pkg/plugin/integration_test.rs:164` 以 `Kind::Audit` 分发通用审计事件。
6. `shutdown` 取走全局集合，把每个实例置为 `Dying`，取消 watcher 并调用关闭回调（`plugin.rs:501–521`）。这里不会再次进入 `Ready`。

因此，`Kind` 决定“从哪一组查找/分发”，`State` 决定“该实例当前能否参与工作”；二者不是同一维度，也不替代单独的运行时禁用原子标志。

## 数据与状态

`Kind` 和 `State` 都是无载荷的 `Copy` 枚举，不持有堆内存、引用或外部资源。它们的判别值和文本是兼容面：改变成员顺序、显式数值或大小写会影响与 Go 的对应关系、状态输出以及潜在的 ABI/持久化使用者。

实际状态存储在 `pkg/plugin/plugin.rs:197` 的 `Plugin` 中：`manifest.kind` 保存种类，`state` 保存生命周期状态，`disabled: Arc<AtomicU32>` 保存可动态切换的启用标志。`Disable` 表示装载/校验/初始化失败后被框架禁用；`disabled == 1` 则是可由 watcher 更新的运行时开关。`Plugin::state_value` 会组合两者，例如 `Ready-enable` 或 `Ready-disable`，所以不能把 `State::Disable` 与原子禁用标志混为一谈。

`State::default()` 是 `Uninitialized`；`Kind` 刻意没有默认值。虽然 `Manifest::default()` 为满足 Rust 空值构造选择了 `Kind::Audit`，`pkg/plugin/spi.rs:149–152` 明确说明这只是稳定占位，与 Go 的零值 `Kind(0)` 不等价。

## 依赖与调用关系

本文件的下游依赖只有 Rust 标准库格式化接口：两个 `Display::fmt` 都调用 `Formatter::write_str`，两个 `as_str` 都只做穷举匹配。没有第三方 crate、feature gate 或条件编译项。

RustCodeGraph 将 `pkg/plugin/const.rs` 标为被五个文件使用：`pkg/plugin/helper.rs`、`pkg/plugin/integration_test.rs`、`pkg/plugin/plugin.rs`、`pkg/plugin/plugin_test.rs`、`pkg/plugin/spi.rs`。直接关系包括：

- `spi.rs`：`Manifest.kind: Kind` 以及 `Manifest::new` 的入参。
- `plugin.rs`：`Plugin.state: State`、`HashMap<Kind, Vec<Plugin>>`、生命周期迁移、按类查询和就绪门控。
- `helper.rs:104`：测试装载辅助构造 `Kind::Audit` 清单。
- `integration_test.rs:164`：通过 `foreach_plugin(Kind::Audit, ...)` 触发审计回调。
- `const_test.rs` 与 `plugin_test.rs`：验证文本映射、分类行为和生命周期相关路径。

RustCodeGraph 对两个枚举能定位定义与源码，但精确 `callers/callees` 查询没有返回更细的符号边；上述直接使用点因此由索引给出的文件级使用关系和这些文件中的源码引用共同核验。

## 错误处理与边界

`as_str` 对闭合枚举做穷举匹配，不返回 `Option`/`Result`，也没有未知值分支；在安全 Rust 中，调用者不能构造枚举声明之外的判别值。`Display::fmt` 唯一可能传播的是格式化器的 `std::fmt::Result`，本文件不产生插件领域错误。

与之相邻但不属于本文件的错误边界是：装载、校验和初始化失败由 `pkg/plugin/plugin.rs` 返回 `PluginError`，并根据 `skip_when_fail` 决定中止或把状态设为 `Disable`；全局锁中毒也在该文件转换为后端错误。`foreach_plugin` 的回调错误会立即向上传播。

不要通过 `unsafe` 把任意 `u8` 转成 `Kind` 或 `State`；无效判别值会破坏 Rust 枚举有效性约束。若协议层需要接收整数，应新增显式、可失败的转换并为未知值添加独立测试，而不是依赖 `repr(u8)` 进行转置。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务或资源句柄；枚举值本身是不可变的 `Copy` 数据，可随包含它们的类型在线程间传递（取决于包含类型的约束）。

并发生命周期由 `pkg/plugin/plugin.rs` 管理：全局插件表放在 `OnceLock<RwLock<Option<Plugins>>>` 中，读路径按 `Kind` 查组并比较 `State`；运行时禁用位通过 `Arc<AtomicU32>` 以 Acquire/Release 顺序读写；`init` 可启动 watcher 线程；`shutdown` 在取走全局集合后将状态改为 `Dying` 并取消 watcher。`State` 本身不是原子类型，当前安全性依赖所有集合内状态修改经全局写锁进行，而读取经读锁或读取克隆快照。扩展状态迁移时必须保留这一锁边界，不能把普通 `State` 字段当作可无锁并发修改的数据。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/plugin/const.go`，对应测试是 `pkg/plugin/const_test.go`：

- Go `type Kind uint8` 的 `Audit = 1 + iota`、`Authentication`、`Schema`、`Daemon` 对应 Rust `#[repr(u8)] enum Kind` 的 1–4。
- Go `type State uint8` 的 `Uninitialized = iota`、`Ready`、`Dying`、`Disable` 对应 Rust `#[repr(u8)] enum State` 的 0–3。
- Go 的两个 `String()` 与 Rust 的 `as_str`/`Display` 返回相同英文文本。`pkg/plugin/const_test.rs:test_const_to_string` 与 Go `TestConstToString` 对这些合法值逐项核对。
- Go 的命名整数类型允许构造未知数值，其 `String()` 对未命中值返回空串；Rust 使用闭合枚举，安全代码中不存在未知成员，因而没有对应的空串分支。这是类型安全差异，不应误写成 Rust 支持未知值兼容。
- Go `Kind` 的零值是未命名的 `0`；Rust `Kind` 没有默认值。Rust 的 `Manifest::default` 选择 `Audit` 仅是清单往返测试需要的占位策略。

Go `plugin.go` 与 Rust `plugin.rs` 的关键迁移语义一致：校验/初始化可将插件置为 `Disable`，成功初始化置为 `Ready`，关闭置为 `Dying`，分发和 flush 前检查 `Ready`。本文件只是这些状态名称和数值的共同定义处。

## 扩展指南

新增插件种类时，应在 `Kind` 末尾增加显式考虑过兼容性的成员，并同步：`Kind::as_str`、`pkg/plugin/const.go` 的常量与 `String()`、`pkg/plugin/const_test.rs:test_const_to_string`、`pkg/plugin/const_test.go:TestConstToString`，以及该种类对应的 Manifest/SPI、装载声明和真实分发入口。仅增加枚举成员而没有消费者会形成“可声明但不可运行”的类别。若数值用于外部 ABI，应保留已有判别值，避免插入成员造成后续值漂移。

新增生命周期状态时，应先定义允许的迁移和门控含义，再同步 `State::as_str`、Go 常量/String、两侧测试，以及 `plugin.rs` 中 `load`、`init`、`shutdown`、`foreach_plugin`、`is_enabled`、`supports_flush` 等所有状态判断。尤其要决定新状态是否允许回调、是否仍持有 watcher、是否与 `disabled` 原子位正交。

若新增整数解析或序列化，应使用 `TryFrom<u8>`（或等价的显式失败 API），覆盖全部合法判别值和未知值，测试继续放在独立的 `pkg/plugin/const_test.rs`，不要把测试嵌入生产源文件。若仅修改显示文本，也必须视为 Go/Rust 兼容变更并同步两侧测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/plugin` 显示 `const.rs` 有 5 个符号；`node --file pkg/plugin/const.rs` 读取完整 84 行并报告五个使用文件；精确 `query`/`node` 确认 `Kind` 位于第 24 行、`State` 位于第 56 行。枚举级 `callers/callees` 未返回更细边，故没有据此臆造符号调用。
- 生产源码：`pkg/plugin/const.rs`、`pkg/plugin/lib.rs`、`pkg/plugin/spi.rs`、`pkg/plugin/plugin.rs`、`pkg/plugin/helper.rs`。
- crate 与移植边界：`pkg/plugin/Cargo.toml`、`pkg/plugin/README.md`。
- Go 对照：`pkg/plugin/const.go`，并以 `pkg/plugin/plugin.go` 的状态写入与检查点核对生命周期语义。
- 独立测试：`pkg/plugin/const_test.rs`、`pkg/plugin/const_test.go`；此外 `pkg/plugin/plugin_test.rs` 和 `pkg/plugin/integration_test.rs` 提供分类索引、就绪过滤和审计分发的使用证据。
- 按任务约束，本次是纯文档分析，未运行 Cargo 或代码测试；交付验证仅执行固定 11 章节的结构命令，并人工核对本文没有把相邻模块行为归因给常量文件。
