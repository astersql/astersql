# `br/pkg/stream/crr/config/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-stream-crr-config` 的 crate 根。`Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定它为库入口，并用 `package.metadata.porting.go-package = "br/pkg/stream/crr/config"` 标记对应的 Go 包。该文件不实现配置算法；它把 [`config.rs`](config.rs) 挂为公开模块并将模块中的公开 API 扁平再导出，使 crate 使用者能够从根路径访问 `Config`、`FlagSet`、`DefaultConfig` 和 `DefineFlags`。

根 workspace 的 `Cargo.toml` 将本目录列为成员，因此这个 crate 会被 workspace 识别。当前仓库搜索没有发现其他 Rust `Cargo.toml` 声明 `astersql-br-pkg-stream-crr-config` 依赖，也没有生产 Rust 文件导入该 crate；所以它目前是可独立构建和测试的移植单元，而不是 Rust BR 命令主链上已接线的配置入口。Go 版本则由 `br/pkg/task/operator/config.go` 直接导入并组合进 `CRRCheckpointConfig`。

## 核心职责

- `#[path = "config.rs"] pub mod config` 固定实现模块的物理文件，并将模块本身公开。
- `pub use config::*` 将 `config.rs` 的全部公开项再导出到 crate 根，维持接近 Go 单包公开面的调用体验。
- 两个 `#[cfg(test)]` 模块分别挂载 [`parity_test.rs`](parity_test.rs) 与 [`config_test.rs`](config_test.rs)，保证测试逻辑与生产源文件分离；非测试构建不会编译这两个模块。
- crate 级 `allow` 列表容纳 Go 风格命名（如 `DefaultConfig`、`DefineFlags`、`TaskName`）及迁移期尚未被生产链消费的公开面。

这里没有默认值计算、参数解析、错误转换或资源管理逻辑；这些行为均位于 `config.rs` 及其依赖的 checkpoint/service crate 中。

## 主要符号

- `config`：公开子模块，真实实现位于 `config.rs`。其公开面包括四个 flag 名常量、`Config`、`FlagSet`、`DefaultConfig()` 和 `DefineFlags()`。
- `pub use config::*`：公开再导出，不创建包装函数或新状态。测试以 `use crate::{Config, DefaultConfig, DefineFlags, FlagSet};` 验证这些名称确实可从 crate 根解析。
- `parity_test`：仅测试构建可见的私有模块，锁定 Go/Rust 默认值、覆盖解析、未定义 flag 错误和无外部资源等公共契约。
- `config_test`：仅测试构建可见的私有模块，对应 Go `config_test.go`，并额外覆盖长选项形式、复合/边界 duration、未知 flag、非法整数和单短横线输入。

本文件不声明常量、结构体、trait、函数或 `impl`，也没有 feature 条件；唯一条件编译项是两个 `cfg(test)` 测试模块。

## 执行流程

1. Cargo 以 `lib.rs` 作为 crate 根加载。
2. 编译器按显式 `#[path = "config.rs"]` 解析并编译 `config` 模块。该实现从 checkpoint crate 取得 `DefaultPollInterval`、`DefaultMetaReadConcurrency`，从 service crate 取得 `ServiceConfig`、`DefaultRetryInterval`。
3. `pub use config::*` 将实现模块的公开项加入 crate 根命名空间；它不执行运行时代码。
4. 使用者若调用 `DefaultConfig()`，实现会组合两个下游 crate 的默认值；若调用 `DefineFlags()`，会向本地 `FlagSet` 注册四个选项；随后 `Config::Parse()` 先恢复默认配置，再依次读取 task name、retry interval、poll interval 和 meta read concurrency。
5. 仅在 `cargo test` 一类测试构建中，编译器再加载 `parity_test.rs` 和 `config_test.rs`。本任务按计划不运行 Cargo，上述行为由源码和测试结构核验。

## 数据与状态

`lib.rs` 自身没有静态可变状态、缓存或实例字段。再导出的 `Config` 持有一个 `astersql_br_pkg_stream_crr_service::Config`，其计算器子配置保存任务名、轮询间隔和 meta 读取并发度，外层保存重试间隔。

再导出的 `FlagSet` 使用六个自有 `HashMap<String, ...>`，分别保存字符串、`Duration`、`i32` 三种类型的默认值和覆盖值。读取时覆盖值优先；未定义的键返回错误。`Config::Parse()` 会先用 `DefaultConfig()` 整体替换旧值，因此重复解析不会保留上一次未覆盖字段。所有权均局限于调用者持有的普通 Rust 值。

## 依赖与调用关系

下游依赖由 `Cargo.toml` 明确给出：

- `astersql-br-pkg-stream-crr-internal-checkpoint`：提供 `CheckpointCalculatorConfig`、`DefaultPollInterval` 和 `DefaultMetaReadConcurrency`。
- `astersql-br-pkg-stream-crr-service`：提供嵌入 `Config.inner` 的服务配置及 `DefaultRetryInterval`；该 service crate 又依赖 checkpoint crate。
- Rust 标准库：`HashMap` 保存 flag 状态，`Duration` 表示 Go `time.Duration` 的非负可表示子集。

本 crate 的内部调用链是 `Default for Config -> DefaultConfig`、`DefineFlags -> DefaultConfig -> Config` 访问器，以及 `Config::Parse -> FlagSet::GetString/GetDuration/GetInt`。`FlagSet::Parse` 按注册类型调用 `SetString`、`SetDuration`、`SetInt`，duration 分支再调用私有 `parse_duration`。

RustCodeGraph 将 `lib.rs` 识别为 1 个符号的门面文件，将 `config.rs` 识别为实现文件；精确源码节点显示 `lib.rs` 只含模块声明和再导出。由于 `config`、`Config` 等名称在仓库中高度重名，宽泛 explore 的调用结果混有其他包；本说明没有把这些歧义结果当作真实调用者。结合 Cargo manifest 与精确文本搜索，当前没有生产 Rust 上游。相邻的 Rust BR CLI 位于 `br/pkg/task/operator/config.rs`，但它使用该 crate 自己 `stubs` 中的 `CRRServiceConfig`，并非此 crate 的 `Config`。

## 错误处理与边界

门面层不捕获或转换错误，所有错误契约来自再导出的实现：

- 未注册、未知或缺值 flag 返回 `Result<_, String>` 错误；单短横线 shorthand 在没有对应定义时被拒绝，`--` 停止 flag 解析，普通位置参数被忽略。
- 整数使用 `i32::parse`；非法文本失败，但实现没有在此层额外限制并发度必须为正。
- duration 只接受 Rust `Duration` 可表示的非负 Go duration 子集，支持组合值、小数和 `ns/us/µs/μs/ms/s/m/h`；负值、未知单位、语法错误、算术溢出和超过 Go `int64` 纳秒上限的值失败。
- `Config::Parse()` 在读取任何 flag 前先重置为默认值；若后续某项读取失败，调用者会收到错误，但 `Config` 可能已完成默认重置及前面字段的赋值，因此不具备失败时保持旧值的事务性保证。这与 Go 源码的逐字段赋值顺序一致。
- `pub use config::*` 会自动暴露 `config.rs` 后续新增的所有 `pub` 项；新增同名根符号时可能产生命名冲突或意外扩大公共 API。

## 并发与资源生命周期

本文件不启动线程、异步任务、通道、锁、事务或 I/O。`Config`、`FlagSet` 及其容器都由调用者拥有，离开作用域后按普通 RAII 释放；测试中的显式 `drop(flags)` 只是确认没有外部句柄生命周期。

`FlagSet` 的修改接口要求 `&mut self`，读取接口使用 `&self`；类型没有内部同步原语。是否跨线程共享取决于调用者自行提供同步，门面层不承诺并发修改安全。两个测试模块只在测试编译阶段存在，不影响生产二进制大小和运行时生命周期。

## 与 Go 版本的对应关系

Go `config.go` 的包级公开面由 Rust `config.rs` 实现，再由本 `lib.rs` 模拟 Go 包根：Go 的嵌入 `service.Config` 对应 Rust `Config { inner: ServiceConfig }` 加访问器；Go 的 `pflag.FlagSet` 对应本地轻量 `FlagSet`；`DefaultConfig`、`DefineFlags`、`Config.Parse` 的默认值来源和字段赋值顺序保持一致。

`config_test.go` 的 `TestDefaultConfig`、`TestParse` 分别对应 Rust `config_test.rs` 的 `test_default_config`、`test_parse`，`parity_test.rs` 再从 crate 根验证相同公共契约。Rust 测试还覆盖 Go `pflag`/`time.ParseDuration` 的若干边界，以防本地替代实现漂移。

接线状态并不对等：Go `br/pkg/task/operator/config.go` 导入 `br/pkg/stream/crr/config`，在 `DefineFlagsForCRRCheckpointConfig` 中调用其 `DefineFlags`，并在 `CRRCheckpointConfig.ParseFromFlags` 中调用其 `Config.Parse`；Rust 同路径的 operator 实现目前使用内部 stubs 类型和 `DefineCRRFlags`。因此不能仅凭本门面和测试断言 Rust 主命令已经消费此 crate。

## 扩展指南

- 修改配置字段、默认值、flag 语义或 duration 解析时，应改 `config.rs`，而不是在 `lib.rs` 增加重复包装；同时更新独立的 `config_test.rs`、`parity_test.rs`，并核对 Go `config.go`、`config_test.go`。
- 新增实现模块时，先在本文件添加明确的 `#[path] mod`/`pub mod`，再决定是否根级再导出；避免与现有 glob 再导出名称冲突。
- 新增测试必须继续放在独立 `*_test.rs`/`parity_test.rs` 文件，通过 `cfg(test)` 挂载，不把测试写进生产源文件。
- 若要把该 crate 接入 Rust BR 主链，需要在实际消费 crate 的 `Cargo.toml` 增加依赖，并用这里的 `Config`/`DefineFlags` 替换或适配 operator 的 stubs 配置；这属于独立接线工作，应同步验证 CLI 注册、必填项校验、服务构造和清理路径，不能只改门面。
- 保持 Go 风格 API 时需继续关注 crate 级 lint 例外；若迁移到 Rust 命名，应评估所有根级调用者和跨语言契约测试的兼容性。
- 性能风险主要在未来扩大 flag 集或频繁克隆 `HashMap`/字符串；当前门面没有热路径开销，再导出本身是编译期行为。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/stream/crr/config` 找到本目录的 Go/Rust 实现与测试；`node --file` 核验了 `lib.rs`、`config.rs`、`config_test.rs`、`parity_test.rs`、`config.go` 和 `config_test.go` 的完整相关源码。
- Rust 源码：`lib.rs` 的 `pub mod config`、`pub use config::*` 和两个 `cfg(test)` 模块；`config.rs` 的 `Config`、`FlagSet`、`DefaultConfig`、`DefineFlags`、`Config::Parse`、`FlagSet::Parse`、`parse_duration`。
- crate 边界：`br/pkg/stream/crr/config/Cargo.toml`、根 `Cargo.toml`、`br/pkg/stream/crr/service/Cargo.toml`、`br/pkg/stream/crr/internal/checkpoint/Cargo.toml`。
- Go 对照：`br/pkg/stream/crr/config/config.go`、`br/pkg/stream/crr/config/config_test.go`、`br/pkg/task/operator/config.go`。
- Rust 接线对照：`br/pkg/task/operator/config.rs`、`br/pkg/task/operator/crr_checkpoint.rs`、`br/cmd/br/operator.rs`；仓库精确搜索未发现生产 Rust 文件或其他 Cargo manifest 引用 `astersql-br-pkg-stream-crr-config`。
- 结构检查使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题。纯文档任务按计划不运行 Cargo 或代码测试。
