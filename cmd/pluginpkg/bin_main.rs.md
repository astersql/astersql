# `cmd/pluginpkg/bin_main.rs`

## 文件定位

`cmd/pluginpkg/bin_main.rs` 是 `astersql-cmd-pluginpkg` 可执行程序的最外层进程入口。`cmd/pluginpkg/Cargo.toml` 的 `[[bin]]` 将二进制名 `astersql-cmd-pluginpkg` 映射到此文件；同一 manifest 的 `[lib]` 则将 `lib.rs` 定义为库 crate。这个拆分使操作系统启动的二进制可以复用库 crate 中可由测试调用的同一条命令执行链。

该文件不是插件打包算法的实现位置。实际层次为 `bin_main.rs::main` → `astersql_cmd_pluginpkg::main`（`lib.rs`）→ `pluginpkg::main`（`pluginpkg.rs`）→ `run_with`。因此，本文件应被视为二进制薄包装，而不是 `manifest.toml` 解析、Go 源码生成或 `go build` 调用的所有者。

## 核心职责

本文件只有一项职责：在进程启动时调用 `astersql_cmd_pluginpkg::main()`，把控制权交给库 crate。它刻意不解析参数、不构造文件系统或子进程实现，也不处理返回值，从而避免二进制入口与测试使用两套打包流程。

这种结构也确定了职责边界：命令行接线位于 `cmd/pluginpkg/pluginpkg.rs::main`，可注入的核心流程位于 `cmd/pluginpkg/pluginpkg.rs::run_with`，文件系统、参数、时钟和命令执行边界定义在 `cmd/pluginpkg/stubs.rs`。修改打包行为时通常不应改动本文件。

## 主要符号

- `fn main()`：私有的 Rust 二进制入口，签名为无参数、无显式返回值。函数体仅调用 `astersql_cmd_pluginpkg::main()`。
- `astersql_cmd_pluginpkg`：由 Cargo 包名 `astersql-cmd-pluginpkg` 转换下划线后得到的库 crate 名。其 `lib.rs::main` 再调用 `pluginpkg::main()`。

本文件没有常量、类型、trait、`impl`、条件编译项或公开 API。RustCodeGraph 将其识别为 9 行文件和一个函数符号 `cmd/pluginpkg/bin_main.rs::main`；“used by 0 files”符合二进制根由运行时启动、而非被其他源码模块引用的角色。

## 执行流程

1. Cargo 构建的 `astersql-cmd-pluginpkg` 二进制被操作系统启动，进入本文件的 `main`。
2. `main` 无条件调用库入口 `astersql_cmd_pluginpkg::main()`。
3. `cmd/pluginpkg/lib.rs::main` 无条件转发到 `pluginpkg::main()`。
4. `cmd/pluginpkg/pluginpkg.rs::main` 重置 flag 状态，从进程环境读取参数，解析命令行，并装配 `OsFs`、`ProdRunner`、`SystemClock`、标准错误和标准输出。
5. `pluginpkg::main` 调用 `run_with`；后者读取 `manifest.toml`、生成临时 `*.gen.go`、执行 `go build -buildmode=plugin`、输出生成路径和 manifest，并在成功返回时清理临时文件。

本文件没有分支或循环。所有帮助输出、参数错误、构建失败和成功清理分支都发生在下游。

## 数据与状态

本文件不声明也不保存任何数据，没有静态变量、线程局部变量或配置缓存，也不接触命令行参数本身。调用过程中唯一可观察的本地状态是普通同步函数调用栈。

下游 `pluginpkg.rs` 使用线程局部的 `PKG_DIR`、`OUT_DIR`、`PGO_FILE` 和 `NEXT_GEN` 保存与 Go 包级 flag 变量对应的状态，但这些状态不属于本文件。入口每次最终由 `pluginpkg::main` 调用 `init_flags` 重置，再把解析结果传给 `run_with`。

## 依赖与调用关系

上游没有普通 Rust 调用者：本文件由 Cargo `[[bin]]` 和进程运行时选作入口。`cmd/pluginpkg/Cargo.toml` 同时声明 `[lib] path = "lib.rs"`，使本文件能通过 crate 名引用共享库入口；workspace 根 `Cargo.toml` 也把 `cmd/pluginpkg` 列为成员。

直接下游仅有 `astersql_cmd_pluginpkg::main()`。源码核验得到后续调用边 `lib.rs::main` → `pluginpkg.rs::main` → `pluginpkg.rs::run_with`；`pluginpkg.rs::main` 还调用 `init_flags`、`stubs::args_from_env`、`stubs::try_parse_flags` 和 `usage`，并构造生产环境依赖。

RustCodeGraph 对 `cmd/pluginpkg/bin_main.rs::main` 报告无 callees，说明当前图索引没有解析这条跨 crate 调用；这不是“入口不执行任何逻辑”的依据。跨 crate 边由目标文件第 8 行、`lib.rs` 第 33–35 行以及 Cargo 的 `[lib]`/`[[bin]]` 声明共同确认。

本文件本身不直接使用 `serde`、`serde_json` 或 `toml`；这些是同一包内库实现的依赖。它也不直接依赖 TiDB 的重型存储 crate，符合 `Cargo.toml` 关于本地 stubs 覆盖 Go 工具链和文件边界的说明。

## 错误处理与边界

`bin_main.rs::main` 没有 `Result` 返回值、错误匹配、日志或恢复逻辑；下游入口的正常返回会直接结束进程，下游 panic 或进程退出语义也不会在此被拦截。

参数解析失败由 `pluginpkg.rs::main` 写入标准错误并调用 `usage`；缺少必要路径、路径绝对化失败、manifest 读取/解码失败、名称不匹配、模板生成失败和 `go build` 失败由 `run_with` 处理。测试中的 `fatal_exit` 以可捕获 panic 模拟 Go 的 `os.Exit(1)`，生产边界则保留立即终止语义。

本入口没有直接测试。`pluginpkg_test.rs` 和 `parity_test.rs` 绕过进程入口调用 `run_with`，分别覆盖模板失败后的部分文件、清理顺序、非字符串字段、成功构建参数、参数边界、错误路径和临时文件生命周期。若入口未来增加自己的行为，应新增独立入口测试，而不是把测试代码嵌入本文件。

## 并发与资源生命周期

本文件同步调用库入口，不创建线程、异步任务、锁、通道或事务，也不直接持有文件和子进程句柄。进程生命周期与这次调用一致：下游返回后，顶层 `main` 随即返回。

资源规则全部由下游控制。`run_with` 在成功路径输出结果后删除生成的 `*.gen.go`；对齐 Go 的 `defer` 加 `os.Exit` 行为，模板或构建失败时临时文件可能保留。`parity_test.rs::contract_resource_cleanup` 和 `pluginpkg_test.rs::deferred_remove_runs_after_manifest_output` 是这项生命周期约束的直接测试证据。

## 与 Go 版本的对应关系

Go 对照文件是 `cmd/pluginpkg/pluginpkg.go`。Go 使用单文件 `package main`：`init` 注册 flags，`main` 直接完成参数解析、路径归一化、manifest 读取、模板生成、`go build`、结果输出和延迟清理。

Rust 保留相同的外部命令角色，但为可测试性将 Go `main` 拆为三层：本文件提供二进制入口，`lib.rs::main` 提供共享库入口，`pluginpkg.rs::main` 装配生产依赖并进入 `run_with`。因此，本文件对应 Go `main` 的“进程入口身份”，而不是逐语句对应 Go `main` 的完整函数体。

Go/Rust 语义对齐由 `parity_test.rs` 和 `pluginpkg_test.rs` 验证，重点包括 flag 拼写、`go build` 参数与环境、模板真值规则、manifest 类型断言、输出格式，以及 `os.Exit` 跳过 `defer` 所导致的失败路径临时文件保留。当前没有单独验证二进制符号能启动的集成测试。

## 扩展指南

- 新增或修改打包步骤：优先修改 `pluginpkg.rs::run_with` 或其 helper，并同步 `parity_test.rs`/`pluginpkg_test.rs`；不要把业务逻辑放进此入口。
- 修改真实环境接线：修改 `pluginpkg.rs::main`，例如新增可注入边界或参数来源，同时补充独立测试覆盖解析和接线契约。
- 修改库/二进制边界或可执行名：同时核对 `Cargo.toml` 的 `[lib]`、`[[bin]]`、workspace 成员和任何发布脚本；保持本文件的 crate 引用与包名转换一致。
- 只有在确需进程级初始化且不能放入库入口时才扩展 `bin_main.rs::main`。这会降低直接单元测试能力，并可能让库调用与真实二进制行为分叉，因此应配套新增独立的进程级测试。
- 兼容风险主要来自改变入口委托顺序、吞掉下游退出行为或在调用前后增加有副作用的初始化；性能风险当前可忽略，因为本文件只增加一次普通函数调用，主要耗时在下游文件 I/O 与 `go build`。

## 验证依据

- `cmd/pluginpkg/bin_main.rs`：完整读取，确认只有版权注释、模块说明和单一 `main` 转发调用。
- `cmd/pluginpkg/Cargo.toml`：确认包名、`[lib] path = "lib.rs"`、`[[bin]] name/path`、二进制迁移元数据及直接依赖边界。
- `cmd/pluginpkg/lib.rs`：确认公开 `main` 仅转发到 `pluginpkg::main`，并确认测试模块以独立文件挂载。
- `cmd/pluginpkg/pluginpkg.rs`：确认生产 `main` 的依赖装配和 `run_with` 的完整打包流程、错误路径及资源清理。
- `cmd/pluginpkg/pluginpkg.go`：确认 Go 版 `init`/`main` 的原始控制流和 `go build -buildmode=plugin` 行为。
- `cmd/pluginpkg/parity_test.rs`、`cmd/pluginpkg/pluginpkg_test.rs`：确认成功、参数边界、失败、输出和临时文件生命周期契约；未发现直接调用私有 `bin_main::main` 的测试。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter cmd/pluginpkg` 定位相关源码和测试；`node --file cmd/pluginpkg/bin_main.rs` 确认文件及符号；`callees` 对同名 `main` 的结果显示目标函数无已解析 callee，故跨 crate 边改由源码与 Cargo 声明验证。
- 未运行 Cargo 或代码测试：本任务仅新增说明文档，按计划使用结构检查代替运行时验证。
