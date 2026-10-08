# `pkg/util/israce/israce.rs`

## 文件定位

本文件是 `astersql-util-israce` crate 的 **race 构建变体**。crate 根模块 [`pkg/util/israce/lib.rs`](./lib.rs) 始终声明 `israce` 与 `norace` 两个子模块，但只有在 Cargo feature `race` 启用时才通过 `pub use israce::*` 把本文件的公开项导出到 crate 根；未启用时改为导出 [`norace.rs`](./norace.rs) 的实现。

crate 边界由 [`pkg/util/israce/Cargo.toml`](./Cargo.toml) 定义：库入口为 `lib.rs`，默认 feature 集为空，`race` 是一个不附带依赖的空 feature。因此本文件不是运行时探测器，而是一个由编译配置选中的常量实现。

## 核心职责

本文件只承担一项职责：在 `race` feature 已启用的编译中提供 `RaceEnabled = true`。上层代码可读取同一个 crate 根符号，而不必自行重复条件编译判断。

它与 [`norace.rs`](./norace.rs) 组成互斥实现：前者在 `#[cfg(feature = "race")]` 下定义真值，后者在 `#[cfg(not(feature = "race"))]` 下定义假值。是否真的使用了某种竞态检测工具，不由本文件在运行时检测或验证；该布尔值只反映 Cargo feature 的选择。

## 主要符号

- `pub const RaceEnabled: bool = true`：本文件唯一的生产符号，也是唯一公开 API。其 `#[cfg(feature = "race")]` 属性使定义仅在 feature 打开时存在。类型是编译期常量 `bool`，没有初始化函数、参数、返回错误或可变状态。
- 文件内没有类型、trait、函数、`impl`、宏定义或其它条件编译分支。

名称沿用 Go 的导出标识符，因此不符合 Rust 通常要求常量全大写的命名习惯；crate 根的 `#![allow(non_upper_case_globals)]` 明确接受这一迁移兼容命名。

## 执行流程

1. Cargo 解析 `astersql-util-israce` 的 feature 集。
2. 若启用了 `race`，编译器保留本文件中带 `#[cfg(feature = "race")]` 的 `RaceEnabled = true`；同时 [`norace.rs`](./norace.rs) 中的反向条件定义被剔除。
3. [`lib.rs`](./lib.rs) 的 `#[cfg(feature = "race")] pub use israce::*` 将常量再导出为 `astersql_util_israce::RaceEnabled`。
4. 消费者把这个编译期布尔值写入版本信息或日志。例如 [`br/pkg/version/build/info.rs`](../../../br/pkg/version/build/info.rs) 的 `LogInfo` 将其格式化为 `race-enabled=...`，`Info` 将其格式化为 `Race Enabled: ...`。

整个流程在编译期完成选择；读取常量时没有分派、I/O 或运行时检测步骤。

## 数据与状态

本文件的数据模型只有一个不可变 `bool` 常量。它不占有堆资源，不维护全局可变状态，也没有缓存、配置读取或持久化数据。

关键不变量是：当本文件的定义参与编译时，`RaceEnabled` 必须为 `true`；crate 根在相同 feature 条件下必须只再导出这一变体。对应的非 race 值由独立文件维护，从而避免同一配置下在 crate 根暴露两个同名定义。

## 依赖与调用关系

下游依赖只有 Rust 编译器内建的 `cfg(feature = ...)` 条件处理；[`Cargo.toml`](./Cargo.toml) 没有声明第三方依赖。

模块接线为 `lib.rs -> israce.rs::RaceEnabled`，仅在 `race` feature 下成立。RustCodeGraph 的文件节点能够读取本文件，但报告文件级 `used by 0 files`，且精确 `query/callers/callees` 没有为这个条件常量建立可寻址定义；因此外部使用关系用源码搜索补证：

- [`br/pkg/version/build/info.rs`](../../../br/pkg/version/build/info.rs) 导入 `astersql_util_israce::RaceEnabled`，并由 `LogInfo`、`Info` 消费。
- [`br/pkg/version/build/parity_test.rs`](../../../br/pkg/version/build/parity_test.rs) 使用同一公开常量校验版本文本和欢迎信息。
- 根 [`Cargo.toml`](../../../Cargo.toml) 以 `facade_util_israce` 声明该路径依赖；BR 的 [`Cargo.toml`](../../../br/pkg/version/build/Cargo.toml) 也直接声明该依赖。当前这两个依赖声明均未请求 `race` feature，而根包自己的 `race = []` 也没有列出对依赖 feature 的转发。

[`pkg/util/printer/printer.rs`](../printer/printer.rs) 虽然也读取名为 `israce::RaceEnabled` 的符号，但它引用的是 [`pkg/util/printer/lib.rs`](../printer/lib.rs) 内部定义的固定 `false` 占位模块，不是本 crate，不能算作本文件的调用者。

## 错误处理与边界

常量定义本身不会失败，也没有 `Result`、panic 或错误传播路径。主要边界来自构建配置：

- 默认 feature 集为空，所以默认选择 `norace.rs` 的 `false`，而不是本文件的 `true`。
- 仅给工作区根包启用当前空的根 feature `race`，从现有 manifest 看不会自动转发到 `astersql-util-israce/race`；需要在依赖 feature 接线或该 crate 自身的构建参数中明确启用，才能选择本文件。
- `RaceEnabled = true` 只能证明 feature 被选中，不能独立证明二进制已经由竞态检测器插桩。调用方不应把它当作运行时能力探测结果。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或资源句柄。`RaceEnabled` 是编译期常量，所有线程观察到相同值，也不存在初始化顺序或销毁阶段。

尽管名称涉及 race，本文件本身既不执行竞态检测，也不改变同步行为；它只让上层根据构建选择记录或分支处理。当前直接 Rust 消费者仅把该值用于版本文本和日志。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/israce/israce.go`](./israce.go)：Go 通过 `//go:build race` 纳入文件，并定义 `const RaceEnabled = true`；Rust 通过 `#[cfg(feature = "race")]` 和 Cargo feature 表达同一二选一语义。两边的公开名称、布尔类型和 race 变体值一致。

Go 的配对文件 [`norace.go`](./norace.go) 使用 `//go:build !race` 并定义 `false`；Rust 的 [`norace.rs`](./norace.rs) 使用 `#[cfg(not(feature = "race"))]`。差异在于 Go 的 `race` 构建标签通常由 Go 的 race 构建模式设置，而 Rust 侧这里是仓库自定义 Cargo feature，当前文件没有把它与具体检测器或编译器选项绑定。

Go 的直接消费点包括 [`br/pkg/version/build/info.go`](../../../br/pkg/version/build/info.go) 和 [`pkg/util/printer/printer.go`](../printer/printer.go)。Rust 已接线 BR 版本信息；Rust printer 当前使用其自身占位模块，因此该部分尚不是对本 crate 的等价调用链。

## 扩展指南

- 若只需新增读取点，应继续从 crate 根导入 `astersql_util_israce::RaceEnabled`，不要直接依赖 `israce` 子模块，否则会绕过 `lib.rs` 的互斥再导出契约。
- 若改变 feature 名称、默认值或转发关系，需要同步检查 [`Cargo.toml`](./Cargo.toml)、根 workspace manifest、所有消费 crate 的 manifest、`lib.rs` 与 `norace.rs`，并确保两种配置仍恰好导出一个同名常量。
- 若改变公开语义，必须同时核对 Go 的 `israce.go`/`norace.go`；不能把“feature 已启用”无依据地扩写为“检测器已实际工作”。
- 测试逻辑应继续放在独立的 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，不要嵌入生产源文件。至少覆盖 `race` 与非 `race` 两种配置；上层展示契约则同步维护 BR 的独立 `parity_test.rs`。
- 若要让 `pkg/util/printer` 使用本 crate，需要单独处理其 crate 依赖和模块接线，并验证 Go/Rust 输出；这不是修改本常量本身即可完成的工作。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库；`files --filter pkg/util/israce` 确认 Rust/Go 配对文件与独立测试；`node --file pkg/util/israce/israce.rs`、`lib.rs`、`norace.rs`、`migration_aster_unit_test.rs` 核对全部源码和条件编译；`node astersql_util_israce` 确认 BR 生产文件及测试的导入节点。图对条件常量的精确 `query/callers/callees` 未建立定义，已在“依赖与调用关系”中明确记录该限制。
- crate 与 feature：读取 [`pkg/util/israce/Cargo.toml`](./Cargo.toml)、根 [`Cargo.toml`](../../../Cargo.toml) 和 [`br/pkg/version/build/Cargo.toml`](../../../br/pkg/version/build/Cargo.toml)。
- Go 对照：读取 [`israce.go`](./israce.go)、[`norace.go`](./norace.go)、[`br/pkg/version/build/info.go`](../../../br/pkg/version/build/info.go) 与 [`pkg/util/printer/printer.go`](../printer/printer.go)。
- Rust 调用与测试：读取 [`lib.rs`](./lib.rs)、[`norace.rs`](./norace.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)、[`br/pkg/version/build/info.rs`](../../../br/pkg/version/build/info.rs)、[`br/pkg/version/build/parity_test.rs`](../../../br/pkg/version/build/parity_test.rs) 以及 printer 的本地占位实现。
- 独立迁移测试 `migration_race_enabled_matches_go_build_tag` 在 `race` 配置断言 `super::RaceEnabled` 为真，在默认配置断言其为假；BR 契约测试则验证该值进入用户可见文本。按任务约束，本次纯文档分析没有运行 Cargo。
