# `dumpling/cmd/dumpling/bin_main.rs`

## 文件定位

本文件是 Cargo 包 `astersql-dumpling-cmd-dumpling` 的原生二进制壳层。`dumpling/cmd/dumpling/Cargo.toml` 的 `[[bin]]` 将二进制名 `astersql-dumpling-cmd-dumpling` 的入口明确指向 `bin_main.rs`；同一包的 `[lib]` 则指向 `lib.rs`。因此，操作系统启动二进制后首先进入本文件的私有 `main()`，随后跨越“二进制 target → 库 target”的边界调用 `astersql_dumpling_cmd_dumpling::main()`。

这个拆分使 Cargo 二进制入口只负责启动，而可复用、可测试的控制流留在库中。实际 CLI 流程不在本文件：`dumpling/cmd/dumpling/lib.rs::main()` 继续转发到 `entry::main()`，其实现位于 `dumpling/cmd/dumpling/main.rs`。

## 核心职责

本文件只有一个职责：把进程级 Rust 入口接到同包库公开的 `main()`。它不解析参数、不创建导出配置、不连接数据库，也不直接管理日志、指标或退出码。

这种薄壳保证实际入口逻辑只有一份：二进制执行和库内测试均可通过 `astersql_dumpling_cmd_dumpling` 的公开接口抵达相同控制流。源码第 3 行注释也明确说明，该层用于避免把主流程硬编码在 Cargo bin 壳层。

## 主要符号

- `fn main()`（`bin_main.rs:4`）：私有、无参数、无返回值的 Rust 二进制入口；这是本文件唯一函数，也是唯一业务符号。
- `astersql_dumpling_cmd_dumpling::main()`（调用点 `bin_main.rs:5`）：由 Cargo 包名 `astersql-dumpling-cmd-dumpling` 转换下划线后得到的库 crate 路径。公开函数定义在 `lib.rs:39`，再调用 `entry::main()`。

本文件没有模块级常量、类型、trait、`impl`、条件编译项或公开 API。其 `main()` 的可见性符合 Rust 二进制入口约定，不供其他 Rust 模块直接调用。

## 执行流程

1. Cargo 根据 `Cargo.toml` 的 `[[bin]].path = "bin_main.rs"` 编译并生成 `astersql-dumpling-cmd-dumpling` 二进制。
2. 操作系统启动该二进制，Rust 运行时调用本文件的 `fn main()`。
3. `main()` 无条件且恰好一次调用 `astersql_dumpling_cmd_dumpling::main()`，不在本层添加前置或后置步骤。
4. `lib.rs::main()` 转发到 `entry::main()`；后者读取 `std::env::args()`，由 `run()`/`run_with_factory()` 完成参数解析、配置构造、指标注册、Dumper 创建、导出和关闭。
5. `entry::main()` 仅在返回码非零时调用 `std::process::exit(code)`；正常、帮助和版本路径自然返回，本文件随之返回并结束进程。

重要不变量是本层必须保持透明转发。若在这里另行解析参数或初始化全局状态，二进制路径将与库测试所覆盖的路径分叉。

## 数据与状态

本文件不声明也不保存任何数据。它没有局部配置、静态变量、缓存、环境变量读取或全局状态写入；唯一可观察动作是函数调用。

命令行参数和进程环境直到下游 `dumpling/cmd/dumpling/main.rs::main()` 才被读取。`Config`、`FlagSet`、Prometheus gatherer、`Dumper` 和 logger 的状态均由下游模块持有，不能归因于本文件本身。

## 依赖与调用关系

- 上游：进程启动机制通过 Cargo 的 `[[bin]]` 元数据选择本文件；源码仓库中没有普通 Rust 调用者，因为二进制 `main` 是运行时入口。
- 直接下游：`bin_main.rs::main → astersql_dumpling_cmd_dumpling::main`（调用点 `bin_main.rs:5`，定义 `lib.rs:39`）。
- 后续调用：`lib.rs::main → entry::main`（`lib.rs:41`），再由 `main.rs::main → run`（`main.rs:55`）。
- crate 边界：`Cargo.toml` 同时声明 `[lib] path = "lib.rs"` 和 `[[bin]] path = "bin_main.rs"`，并依赖 `astersql-dumpling-cli`、`astersql-dumpling-export`、`astersql-dumpling-log`。这些依赖由库的后续模块使用，本文件不直接导入它们。
- 构建系统对照：`dumpling/cmd/dumpling/BUILD.bazel` 当前描述 Go 的 `dumpling` binary；Rust 二进制归属由 Cargo manifest 证明，不能从该 Bazel 文件推导 Rust 调用边。

RustCodeGraph 能定位 `dumpling/cmd/dumpling/bin_main.rs::main` 及其两行函数体，但本次 `callers/callees` 对常见名称 `main` 发生全仓库同名消歧退化，并对这个跨 crate 调用报告“无 callees”。因此这里的直接边以目标源码和 Cargo crate 声明交叉验证，不把缺失的图边误写成“没有下游”。

## 错误处理与边界

本文件没有 `Result`、显式错误分支、panic 处理或退出码转换。若库入口正常返回，本层也正常返回；若下游 panic，默认 panic 行为向进程边界传播；若下游执行 `std::process::exit`，控制流不会返回本层。

具体边界由 `main.rs` 管理：命令行语法错误返回 2，配置、位置参数、Dumper 创建或 Dump 失败返回 1，帮助、版本与成功路径返回 0。`entry::main()` 只对非零值调用 `process::exit`。`parity_test.rs` 的 `command_line_parse_errors_use_go_pflag_exit_code` 和 `contract_error_paths` 固定了这些契约，但它们测试的是本壳层之后的可复用入口，而不是直接启动子进程测试本文件。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件、网络连接或其他资源，也不存在本层清理顺序。它的栈帧只覆盖一次同步函数调用。

资源生命周期属于下游：`main.rs::run_with_factory()` 创建 Dumper，在 `Dump()` 之后无论导出成功与否都调用 `Close()`，并忽略 Close 错误以对齐 Go 行为；Prometheus 默认 gatherer 是测试可重置的全局状态。`parity_test.rs::contract_resource_cleanup_close_and_gatherer` 以原子计数器/标志验证 Dump 失败后仍 Close，并验证 gatherer 可被清理。扩展本壳层时不得截断这些下游清理路径，尤其不应在转发前后增加提前退出。

## 与 Go 版本的对应关系

Go 对照文件是 `dumpling/cmd/dumpling/main.go`。Go 的 `func main()` 直接包含完整 CLI 流程：安装 usage、注册和解析 flag、打印版本、校验参数、注册运行时 collectors、替换默认 gatherer、创建 Dumper、Dump、Close、记录结果并在失败时 `os.Exit(1)`。

Rust 版没有删减这条主流程，而是将 Go 单函数拆成三层：本文件提供真实二进制入口，`lib.rs::main()` 提供可复用门面，`main.rs` 承载 Go `main.go` 的控制流。由此，本文件只对应 Go `func main()` 的“进程入口身份”，行为语义需要沿调用链到 `main.rs` 才完整。`parity_test.rs::go_rust_public_contract_matches` 集中核对帮助/版本、默认参数、失败退出码和资源关闭等 Go/Rust 公共契约。

仍需注意实现形态差异：Go 通过包级 `pflag.CommandLine` 和 `os.Exit` 驱动流程；Rust 下游先以整数表达退出码，再由 `entry::main()` 统一退出，从而允许测试直接调用 `run()`。这不是本壳层自行实现的行为。

## 扩展指南

- 新增或修改 CLI 行为时，应优先修改 `dumpling/cmd/dumpling/main.rs` 的 `run()`/`run_with_factory()`，参数定义与转换分别落在 `config_flags.rs`，不要把业务逻辑塞入本文件。
- 若要改变库门面，应同步检查 `lib.rs::main()`；只有二进制 target 的启动接线变化才应修改 `bin_main.rs::main()`。
- 保持透明、单次转发，避免在壳层复制参数解析、错误映射、日志或资源清理，否则 `parity_test.rs` 直接测试库入口时无法覆盖新增差异。
- 相关 Rust 测试应继续放在独立文件：入口与 Go 行为契约扩展到 `parity_test.rs`，flag 细节扩展到 `config_flags_test.rs`；不要把测试嵌入 `bin_main.rs`。
- 若确需验证二进制壳本身（例如名称、环境或真实退出状态），应新增独立的进程级测试，并通过 Cargo 提供的二进制路径启动它；当前同目录没有针对 `bin_main.rs` 的直接子进程测试。
- 兼容风险主要是入口目标、二进制名称和退出语义漂移；性能风险极低，因为该层只有一次同步转发。任何新增初始化都应评估启动延迟、重复初始化和下游清理是否仍可达。

## 验证依据

- 目标源码：`dumpling/cmd/dumpling/bin_main.rs:1-6`，确认唯一符号及直接转发调用。
- crate 配置：`dumpling/cmd/dumpling/Cargo.toml`，确认包名、`[lib]`、`[[bin]]`、二进制名、入口路径与三个直接 Cargo 依赖；根 `Cargo.toml:82` 确认该包属于 workspace。
- Rust 后续入口：`dumpling/cmd/dumpling/lib.rs:16-42` 与 `dumpling/cmd/dumpling/main.rs:53-145`，确认模块接线、转发链、参数来源、退出码和 Dump/Close 顺序。
- Go 对照：`dumpling/cmd/dumpling/main.go:29-86`，确认完整原始入口流程及错误、指标、资源关闭语义。
- 独立测试：`dumpling/cmd/dumpling/parity_test.rs`，确认帮助/版本、解析错误码、配置错误、Dumper 创建/导出失败及失败后 Close；`config_flags_test.rs` 只覆盖参数转换细节，不直接覆盖本壳层。
- 构建元数据：`dumpling/cmd/dumpling/BUILD.bazel`，确认其为 Go target 描述，Rust target 证据来自 Cargo。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`node --file dumpling/cmd/dumpling/bin_main.rs --offset 1 --limit 240` 和 `node dumpling/cmd/dumpling/bin_main.rs::main` 均确认第 4-6 行函数体。`query 'bin_main.rs::main'` 定位到精确符号 ID `dumpling/cmd/dumpling/bin_main.rs:4:function:main`；`callers/callees` 的同名消歧与跨 crate 边缺失限制已在“依赖与调用关系”中明确说明。
- 本任务只新增说明文档，按总计划不运行 Cargo；结构检查用于验证固定章节数量，人工复核用于确认没有把下游行为误归属给薄壳入口。
