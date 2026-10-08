# `pkg/util/logutil/test_main.rs`

## 文件定位

本文件是 `astersql-util-logutil` crate 的自定义测试进程适配器，只在 `#[cfg(test)]` 下参与编译。`pkg/util/logutil/lib.rs` 关闭默认测试 harness（`Cargo.toml` 的 `[lib] harness = false`），包含构建脚本生成的测试注册表，并由自定义 `main()` 调用 `test_main::run(registered_tests())`。因此它位于“收集相邻 `*_test.rs` 测试”与“启动整个 crate 测试进程”之间，不参与日志工具的生产运行路径。

当前实现与文件头注释所描述的完整目标并不一致：`run` 目前只执行公共测试初始化，没有执行传入的测试列表，也没有调用本文件已经实现的平台线程枚举函数。文档以下均以这一当前源码事实为准，而把完整 TestMain 行为标为待接线能力。

## 核心职责

- `run(tests)` 是 crate 测试可执行文件的公开入口；当前唯一生效的职责是调用 `testsetup::SetupForCommonTest()`，依据 `log_level` 环境变量配置全局测试日志级别。
- 三个互斥的 `extra_threads()` 实现为 Linux、macOS 和 Windows 提供进程原生线程快照，并排除运行调用的当前/主线程。它们为线程泄漏检查准备了底层能力，但当前没有调用者。
- 对 Linux、macOS、Windows 之外的平台使用 `compile_error!`，使不具备原生线程枚举实现的平台在测试编译期明确失败，而不是静默跳过检查。

本文件当前**不**承担以下已在注释或测试中表达、但尚未接线的职责：调用 `libtest_mimic` 执行 `tests`、保留测试失败状态、等待短暂收尾线程、对成功套件报告泄漏线程。`pkg/util/logutil/main_test.rs` 中的测试表达了这些预期，但不能作为这些行为已经实现的证据。

## 主要符号

- `pub fn run(tests: Vec<libtest_mimic::Trial>)`：自定义 harness 入口。它接收 `build.rs` 生成的 `registered_tests()`，但当前参数未使用；函数只调用 `testsetup::SetupForCommonTest()`，正常路径返回 `()`，非法日志级别则由下游直接终止进程。
- `fn extra_threads() -> Result<Vec<u64>, String>`（Linux）：读取 `/proc/self/task`，把目录项名称解析为线程 ID，并过滤等于 `std::process::id()` 的主线程 ID。目录读取、目录项读取或 ID 解析失败均转成 `String` 返回。
- `fn extra_threads() -> Result<Vec<u64>, String>`（macOS）：调用 Mach 的 `task_threads` 获取当前 task 的线程端口，再用 `thread_info(THREAD_IDENTIFIER_INFO)` 获得稳定的 `thread_id`，过滤 `pthread_threadid_np` 返回的当前线程 ID。
- macOS 局部类型 `Snapshot { task, ports, count }` 及其 `Drop`：拥有 `task_threads` 返回的端口数组；析构时逐一 `mach_port_deallocate`，最后 `vm_deallocate` 数组内存，覆盖成功和提前返回路径。
- macOS 局部 FFI 声明 `mach_port_deallocate(...)`：补充 `libc` crate 未导出的 Mach API。
- `fn extra_threads() -> Result<Vec<u64>, String>`（Windows）：通过 ToolHelp `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)` 枚举系统线程，仅保留属于当前进程且不等于 `GetCurrentThreadId()` 的线程 ID。
- Windows 局部类型 `Snapshot(HANDLE)` 及其 `Drop`：拥有 ToolHelp snapshot handle，并在离开作用域时调用 `CloseHandle`。
- `use std::time::Duration`：当前未使用；它只表明原计划可能包含等待收尾线程的重试窗口，不能据此推断已有等待逻辑。

## 执行流程

当前真实流程如下：

1. `pkg/util/logutil/build.rs::main` 扫描当前目录的 `*_test.rs`，将无返回值的 `#[test]` 函数转换成 `libtest_mimic::Trial`，并生成 `registered_tests()`。
2. `pkg/util/logutil/lib.rs` 在测试配置下包含生成文件，并由自定义 `main()` 调用 `test_main::run(registered_tests())`。
3. `run` 调用 `testsetup::SetupForCommonTest()`。该函数读取 `log_level`；为空或未设置时保持默认 Info，合法值会更新全局 logger，非法值打印 `applyOSLogLevel failed:` 后以 `-1` 退出。
4. `run` 随即返回。传入的 `Vec<Trial>` 被丢弃，`libtest_mimic` runner 没有被调用，三个 `extra_threads` 版本也没有进入执行路径。

如果未来补齐完整适配器，顺序必须保持为“公共初始化 → 执行全部 Trial → 仅在测试成功后等待/检查残留线程 → 以测试或泄漏结果决定退出状态”。这来自 Go `TestMain` 的顺序及 `main_test.rs` 的断言意图，不是当前行为。

## 数据与状态

- `tests: Vec<libtest_mimic::Trial>` 拥有构建脚本注册的测试闭包；当前在 `run` 返回时直接释放，闭包不会执行。
- Linux 快照是一次性的 `/proc/self/task` 目录遍历结果；没有基线集合或忽略名单，若接线后会把枚举时仍存活的全部非主线程视为候选。
- macOS 快照拥有内核分配的线程端口数组及每个端口的 send right，所有权由局部 `Snapshot::drop` 集中回收。
- Windows 快照拥有一个 `HANDLE`，由局部 `Snapshot::drop` 回收；结果只包含 `th32OwnerProcessID` 等于当前 PID 的线程。
- 公共初始化的实际全局状态位于 `pkg/testkit/testsetup/bridge.rs`：`INSTALL_LOGGER: Once` 保证 logger 只安装一次，`CONFIGURED_LEVEL: AtomicUsize` 与 `log::set_max_level` 保存有效日志级别。本文件本身没有可变静态状态。

线程 ID 只适合诊断当前快照，不能当作跨时刻永久身份；线程可在枚举与检查之间退出，macOS 源码已把这种竞态作为 `thread_info` 失败返回，而不是忽略。

## 依赖与调用关系

上游调用链是 `pkg/util/logutil/lib.rs::main` → `test_main::run`。测试列表来自 `pkg/util/logutil/build.rs` 生成的 `registered_tests()`；构建脚本把相邻 `*_test.rs` 中受支持的测试函数包装成 `libtest_mimic::Trial`。

`run` 当前唯一的下游调用是开发依赖 `astersql-testkit-testsetup` 的 `SetupForCommonTest`。`Cargo.toml` 将 `libtest-mimic`、`libc`、`testsetup` 声明为 dev-dependencies，并仅在 Windows 测试目标上启用带 `Win32_Foundation`、`Win32_System_Diagnostics_ToolHelp`、`Win32_System_Threading` feature 的 `windows-sys`。

平台枚举的下游分别为：Linux 标准库文件系统 API 与 `/proc`；macOS 的 `libc`/Mach/Pthread API；Windows 的 ToolHelp、Foundation 和 Threading API。RustCodeGraph 对 `run`、`extra_threads` 的精确 callers/callees 查询没有给出额外调用边；`rg` 也只找到 `lib.rs` 对 `run` 的调用以及函数自身的三个定义，支持“线程枚举尚未接线”的判断。

## 错误处理与边界

- `SetupForCommonTest` 不返回错误：非法 `log_level` 会向 stderr 输出原因并调用 `std::process::exit(-1)`，因此测试闭包不会开始执行。
- Linux 将 `read_dir`、单个目录项以及线程 ID 解析错误原样字符串化；因为 `collect()` 的目标是 `Result<Vec<_>, _>`，任一项失败会使整个快照失败。
- macOS 明确检查 `task_threads`、`pthread_threadid_np` 和每次 `thread_info` 的返回码。线程在快照后退出可能让 `thread_info` 失败，当前选择返回错误；注释建议调用方重试整个快照，但调用方尚不存在。
- Windows 在 snapshot 创建失败时返回 `last_os_error()`；枚举结束后只有 `ERROR_NO_MORE_FILES` 被视为正常终止，其他 `GetLastError()` 值会返回诊断字符串。
- 不支持的平台在编译期失败。Linux 依赖 `/proc/self/task`，因此即使目标 OS 为 Linux，受限容器或不可用的 procfs 仍可能产生运行时错误。
- 当前最重要的行为边界是：`run` 返回 `()` 且没有 runner/退出码传播，所以任何关于“测试失败优先于泄漏错误”或“成功时检查泄漏”的结论都尚未由生产接线实现。

## 并发与资源生命周期

本文件不创建线程；它只提供线程快照函数。Linux 的 `ReadDir` 和收集结果由 RAII 回收。macOS 的 `Snapshot` 保证线程端口和 VM 数组在正常返回、查询失败及其他提前返回路径上释放；Windows 的 `Snapshot` 同样保证 handle 关闭。这两个局部所有权守卫是修改 FFI 代码时必须保留的不变量。

线程枚举天然存在竞态：线程可能在取快照后结束，也可能在快照后新建。Linux 和 Windows 返回单次观察值；macOS 会在查询已消失线程时返回错误。若实现等待收尾线程，必须使用有界重试和 `Duration`，避免无限等待，同时应只在测试执行成功后检查，以免泄漏诊断覆盖原测试失败。当前源码没有任何等待、重试或泄漏判定逻辑。

## 与 Go 版本的对应关系

Go 对照入口是 `pkg/util/logutil/main_test.go::TestMain`：先调用 `testsetup.SetupForCommonTest()`，再构造四个 `goleak.IgnoreTopFunction`，最后调用 `goleak.VerifyTestMain(m, opts...)`。该调用负责执行 `testing.M` 并在套件结束时验证 goroutine 泄漏。

Rust 已对齐公共日志初始化，并准备了跨平台“原生线程而非 goroutine”枚举。这四个 Go 忽略项分别属于 glog、rules_go、httprc 和 lumberjack 的 Go-only worker，当前 crate 没有同名 Rust worker；Rust 源码也没有按名称忽略或记录基线线程。

关键差异是 Rust 的 runner 与泄漏检查尚未接线：`tests` 未被执行，`extra_threads` 未被调用，而 Go `VerifyTestMain` 已同时执行套件和完成泄漏检查。`pkg/util/logutil/main_test.rs` 中 `testmain_rejects_invalid_level_before_running_tests`、`testmain_detects_detached_threads`、`testmain_accepts_joined_and_finishing_threads`、`testmain_preserves_test_failure`、`testmain_initializes_all_go_log_levels` 描述了期望的移植语义；在当前 `run` 下，除初始化相关分支外，这些测试意图不能成立。

## 扩展指南

补齐 TestMain 时，最小修改点应集中在 `run`：调用 `libtest_mimic` 执行传入 Trial，保存其成功/失败状态，并仅在成功后通过有界等待调用 `extra_threads()`。不要把测试实现嵌入本文件；应同步修改同目录独立测试 `pkg/util/logutil/main_test.rs`，覆盖初始化失败、测试 panic/失败优先级、永久泄漏、已 join 线程、即将退出线程和各平台枚举错误。

扩展时需要特别守住以下约束：

- 保持 Go 顺序，初始化失败必须发生在任何 Trial 之前；原测试失败不能被泄漏错误替换。
- 明确 `libtest_mimic` 的参数解析、退出状态与 panic 处理方式，不能只遍历闭包来冒充完整 runner。
- 为收尾线程设置短且有界的等待；不得把平台快照中的瞬时线程一律永久判漏。
- 保留 macOS 端口/VM 内存和 Windows handle 的 RAII 回收；新增 FFI 分支必须逐项检查返回码。
- 若要引入基线或白名单，必须有 Rust 侧真实 worker 证据，不能机械复制 Go 的函数名忽略项。
- 新平台必须新增独立 `extra_threads` 实现及测试，不能移除现有 `compile_error!` 后静默放行。

兼容性风险主要是自定义 harness 的命令行/退出码是否与 Cargo 预期一致；正确性风险是竞态导致误报、遗漏外部 native worker 或覆盖测试失败；性能风险较低，但过长轮询会直接增加每次 crate 测试耗时。

## 验证依据

- 目标源码：`pkg/util/logutil/test_main.rs`；确认 `run` 当前只有一次初始化调用，三个 `extra_threads` 由平台 `cfg` 互斥选择，其他平台使用 `compile_error!`。
- crate 接线：`pkg/util/logutil/lib.rs`；确认测试配置包含生成注册表、声明 `mod test_main`，并由自定义 `main()` 调用 `test_main::run(registered_tests())`。
- 构建与依赖：`pkg/util/logutil/build.rs`、`pkg/util/logutil/Cargo.toml`；确认 Trial 的生成方式、`harness = false` 及平台 dev-dependency 边界。
- Rust 测试意图：`pkg/util/logutil/main_test.rs`；确认初始化、失败保留、泄漏、join/收尾线程和日志级别用例。该文件是期望行为证据，不是当前生产接线已完成的证据。
- Go 对照：`pkg/util/logutil/main_test.go`、`pkg/testkit/testsetup/bridge.go`；确认 `SetupForCommonTest` 与 `goleak.VerifyTestMain` 的调用顺序及四个 Go-only 忽略项。
- Rust 公共初始化：`pkg/testkit/testsetup/bridge.rs`、`pkg/testkit/testsetup/bridge_test.rs`；确认合法/非法日志级别、全局 logger 状态及 `warning` 别名。
- RustCodeGraph：索引状态为 11,467 个文件；目标目录列出 23 个已索引源码文件。对目标文件的 `node --file` 显示 150 行和全部 12 个符号；精确 `callers/callees` 没有发现 `extra_threads` 调用边。使用 `rg` 补查未返回的边后，仅发现 `lib.rs` 调用 `test_main::run`，没有 `extra_threads` 调用或 `libtest_mimic::run`。
- 人工复核结论：本文件存在于自定义测试 harness 边界，当前能完成日志初始化与提供未接线的跨平台线程枚举；安全扩展必须把 Trial 执行、状态传播和有界泄漏检查接入 `run`，并继续将测试放在同目录独立测试文件中。
