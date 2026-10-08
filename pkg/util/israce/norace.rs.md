# `pkg/util/israce/norace.rs`

## 文件定位

`norace.rs` 是 `astersql-util-israce` crate 的非 race 构建变体，源码入口由同目录的 `lib.rs` 声明为 `pub mod norace`。当 crate 未启用 Cargo feature `race` 时，`lib.rs` 通过 `#[cfg(not(feature = "race"))] pub use norace::*` 将本文件的 `RaceEnabled` 重导出为 crate 根 API。

crate 边界由 `pkg/util/israce/Cargo.toml` 定义：库入口是 `lib.rs`，默认 feature 集为空，另有一个无附加依赖的 `race` feature。因此普通/default 构建选择本文件的 `false` 变体；启用该 feature 时则由相邻的 `israce.rs` 提供 `true` 变体。

## 核心职责

本文件只承担一个职责：在 `race` feature 未启用的编译配置中提供公开布尔常量 `RaceEnabled = false`，使调用方无需自行编写条件编译分支，就能把当前构建模式写入版本文本或启动日志。

它不是竞态检测器，也不启动线程检查、采样或运行时探针。根据当前 `Cargo.toml`，`race` 只是本 crate 的显式 feature 开关；仓库 Cargo 清单搜索未发现为该依赖启用 `race` 的配置，所以不能仅凭此常量推断 Rust 工具链确实执行了竞态检测。

## 主要符号

- `#[cfg(not(feature = "race"))] pub const RaceEnabled: bool = false`：本文件唯一的生产符号。`pub` 使它可被模块外访问，类型和值都在编译期确定；`cfg` 保证它只在非 race feature 配置中存在。
- 同名互斥符号位于 `pkg/util/israce/israce.rs`：其条件为 `#[cfg(feature = "race")]`，值为 `true`。两个文件的定义经 `lib.rs` 中互斥的重导出条件形成同一个 crate 根接口，而不会在同一配置下发生名称冲突。

RustCodeGraph 对 `pkg/util/israce/norace.rs` 的文件节点报告 1 个符号、0 个直接使用文件；这反映图索引没有把条件模块重导出归为本文件的直接调用边。实际接线由 `pkg/util/israce/lib.rs` 的模块声明和重导出语句确认，调用方则通过 crate 根或上层门面引用该常量。

## 执行流程

1. Cargo 解析 `pkg/util/israce/Cargo.toml` 的 feature 集；默认配置不含 `race`。
2. 编译 `lib.rs` 时，`norace` 模块被声明；在 `not(feature = "race")` 条件下，本文件的常量定义有效。
3. `lib.rs` 在相同条件下执行 `pub use norace::*`，将 `RaceEnabled` 暴露为 `astersql_util_israce::RaceEnabled`。
4. 调用方读取这个编译期常量并格式化输出。例如 `br/pkg/version/build/info.rs` 的 `LogInfo` 和 `Info` 分别写入欢迎日志与版本文本；`pkg/util/printer/printer.rs` 经其 `israce` 门面在 `PrintTiDBInfo` 和 `GetTiDBInfo` 中输出该值。

本文件本身没有函数调用、控制流或运行时初始化；选择发生在编译期，消费发生在调用方格式化版本信息时。

## 数据与状态

唯一数据是不可变的 `bool` 常量 `RaceEnabled`。它不占有堆资源，不依赖环境变量或全局可变状态，也不会在进程运行期间改变。对一个既定构建产物而言，该值是固定的；若要改变它，必须以不同 feature 配置重新编译。

常量采用 Go 风格的公开名称而非 Rust 通常要求的大写蛇形命名。`pkg/util/israce/lib.rs` 在 crate 级允许 `non_upper_case_globals`，保留了与 Go API `RaceEnabled` 的命名一致性。

## 依赖与调用关系

下游依赖仅为 Rust 条件编译机制和内建 `bool` 类型，本文件没有 `use`、外部 crate 调用或被调用函数。

直接装配关系如下：

- `pkg/util/israce/lib.rs` 声明 `pub mod norace`，并在 `not(feature = "race")` 下重导出其全部公开符号。
- `pkg/util/israce/Cargo.toml` 声明空的默认 feature 集和 `race = []`，决定两个变体的选择条件。
- `br/pkg/version/build/info.rs` 以 `use astersql_util_israce::RaceEnabled` 直接消费 crate 根重导出，并将值用于 `LogInfo` 与 `Info`。
- `pkg/util/printer/printer.rs` 通过该 crate 在上层 `israce` 门面中的重导出读取 `israce::RaceEnabled`，用于 TiDB 启动日志和诊断版本文本。

相关测试位于独立文件而非生产源文件中：`pkg/util/israce/migration_aster_unit_test.rs` 验证 feature 开关与布尔值一致；`br/pkg/version/build/parity_test.rs` 验证该值进入版本文本和日志。目标目录没有 Go 单测文件。

## 错误处理与边界

本文件没有返回值、错误类型、panic 路径、I/O 或解析过程，因此没有运行时错误传播。它的主要边界是编译配置：

- 未启用 `race` 时，本定义存在且值必须为 `false`。
- 启用 `race` 时，本定义被 `cfg` 排除，crate 根改为重导出 `israce.rs` 的 `true` 定义。
- 如果未来改变 feature 名称、只修改定义侧而未同步 `lib.rs` 的重导出条件，可能导致符号缺失或变体语义错误。
- `race` feature 当前只是声明式标记；把它描述为自动检测编译器或运行时状态会超出源码证据。

## 并发与资源生命周期

该常量是编译期值，只读且无内部可变性，任意线程读取都不需要锁或原子操作。文件不创建线程、任务、通道、锁、事务、文件句柄或网络连接，也不存在初始化和清理阶段。

名称中的 `race` 描述构建标签/feature 状态，并不意味着此模块参与并发调度或竞态检测生命周期。资源和并发风险只可能来自消费该值的其他模块，本文件没有此类所有权。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/util/israce/norace.go`：它以 `//go:build !race` 选择非 race 构建，并声明 `const RaceEnabled = false`。Rust 版本用 `#[cfg(not(feature = "race"))]` 和 Cargo feature 模拟同一二选一契约，常量名称、类型语义和值一致。

配对的 Go 文件 `pkg/util/israce/israce.go` 使用 `//go:build race` 并提供 `true`；Rust 的 `israce.rs` 与之对应。差异在于 Go 的 `race` build tag 通常由 Go race 构建方式提供，而 Rust 侧从当前代码可确认的机制仅是 Cargo feature；二者的选择机制不能视为自动等价。

消费者也保持用途对齐：Go 的 `br/pkg/version/build/info.go` 和 `pkg/util/printer/printer.go` 将 `israce.RaceEnabled` 写入版本/启动信息，Rust 对应文件执行相同类别的展示。目标目录未发现 Go 单测，因此 Rust 的 `migration_aster_unit_test.rs` 是迁移后新增的独立契约测试。

## 扩展指南

- 若只需新增一个消费者，应继续从 crate 根使用 `astersql_util_israce::RaceEnabled`，不要直接依赖 `norace` 模块，以免绕过变体抽象。
- 若调整构建选择机制，必须同步检查 `norace.rs`、`israce.rs`、`lib.rs` 和 `Cargo.toml` 的条件是否互斥且完备，并核对 Go 的 `!race`/`race` 语义是否仍被保留。
- 若希望反映真实 Rust 竞态检测状态，需要先建立工具链或构建系统到 Cargo feature 的可靠接线；不能仅修改本常量值。该接线应有独立构建配置验证。
- 修改常量契约时，应同步更新独立测试 `pkg/util/israce/migration_aster_unit_test.rs`，并检查 `br/pkg/version/build/parity_test.rs` 的输出断言；不要把测试嵌入 `norace.rs`。
- 兼容风险集中在公开名称和值：上层版本信息依赖它。性能风险近乎为零，因为读取会被编译为常量；错误 feature 接线则会造成诊断信息与实际构建方式不一致。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件、307,296 个节点；`files --filter pkg/util/israce` 列出目标源、配对源、模块入口和独立测试。
- RustCodeGraph `node --file pkg/util/israce/norace.rs`：确认文件共 27 行、唯一生产定义为第 27 行的 `RaceEnabled`，并报告无直接文件使用边。
- RustCodeGraph `node --file pkg/util/israce/lib.rs`：确认 `norace` 模块声明、条件重导出以及独立测试装配；`node --file pkg/util/israce/migration_aster_unit_test.rs` 确认默认分支断言 `!super::RaceEnabled`、race 分支断言其为真。
- crate 与装配证据：`pkg/util/israce/Cargo.toml`、`pkg/util/israce/lib.rs`、根 `Cargo.toml` 的 workspace 成员和 `facade_util_israce` 路径依赖。
- Rust 调用证据：`br/pkg/version/build/info.rs`、`br/pkg/version/build/parity_test.rs`、`pkg/util/printer/printer.rs`。
- Go 对照证据：`pkg/util/israce/norace.go`、`pkg/util/israce/israce.go`、`br/pkg/version/build/info.go`、`pkg/util/printer/printer.go`；同目录文件清单和 `rg` 搜索均未发现 Go 测试文件。
- Cargo 清单搜索未发现针对 `astersql-util-israce`/`facade_util_israce` 启用 `race` feature 的配置；因此本文只确认显式 feature 语义，不声称它与某种 Rust race 工具自动联动。
