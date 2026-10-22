# `pkg/util/rlimit_windows.rs`

## 文件定位

Windows 分支不探测操作系统资源限制；该数值也与非 Windows 实现读取 `RLIMIT_NOFILE` 失败时的回退值一致。

## 核心职责

- 在 Windows 上提供公开函数 `GenRLimit`，使需要“进程可打开文件数上限”的上层代码具有可编译的统一接口。
- 固定返回 `1024`。Windows 分支不探测操作系统资源限制；该数值也与非 Windows 实现读取 `RLIMIT_NOFILE` 失败时的回退值一致。
- 接受但不解释 `source`，保持与 Go 函数 `GenRLimit(source string) uint64` 及非 Windows Rust 分支相同的参数形状。

当前 Rust 仓库中未检索到生产代码调用 `rlimit_windows::GenRLimit`；因此它目前是已公开但尚未接入 Rust 业务主链的平台兼容实现。Go 主链的两个实际用途分别是：`pkg/ddl/ingest/env.go` 初始化 `litRLimit`，再由 `pkg/ddl/ingest/config.go` 写入 `MaxOpenFiles`；`pkg/executor/importer/import.go` 直接将返回值写入 table import 后端配置的 `MaxOpenFiles`。

## 主要符号

### `pub fn GenRLimit(source: &str) -> u64`

这是文件中的唯一符号和唯一公开 API。

- 输入：借用的字符串切片 `source`。按照跨平台接口约定，它描述调用来源；Windows 实现通过 `let _ = source;` 显式丢弃该值。
- 输出：无条件返回 `1024_u64`。
- 可见性：`pub`，可由 `astersql_util::rlimit_windows::GenRLimit` 形式访问，但仅在 Windows 编译目标上存在。
- 副作用：无日志、系统调用、分配、全局状态读写或 I/O。

文件不定义常量、结构体、枚举、trait 或 `impl`。固定值 `1024` 目前是函数体内的字面量，而不是共享常量。

## 执行流程

1. Windows 目标编译 `pkg/util/lib.rs` 时，`rlimit_windows` 模块中的内容因 `#![cfg(windows)]` 生效。
2. 调用者把来源字符串以 `&str` 传给 `GenRLimit`。
3. 函数用 `let _ = source;` 表明参数有意未使用；不会解析来源，也不会输出来源相关日志。
4. 函数立即返回 `1024`，没有条件分支和失败路径。
5. 预期上层将该值作为打开文件数预算使用；Go 侧的真实接线证据是 DDL ingest 与 table import 的 `MaxOpenFiles` 配置。Rust 侧尚未发现等价生产接线，不能把 Go 调用关系描述成已经完成的 Rust 调用关系。

## 数据与状态

函数只处理一个临时的不可变借用 `&str` 和一个 `u64` 返回值。它不保存 `source`，不创建堆对象，也不读取或修改进程、线程、环境变量、文件描述符或 crate 级状态。

核心不变量是：在任意 Windows 调用中，无论 `source` 内容为空、很长或来自哪个子系统，返回值始终为 `1024`。这个值不是对 Windows 当前实际句柄上限的测量，而是跨平台兼容默认值。

## 依赖与调用关系

上游模块接线：

- `pkg/util/lib.rs`：以 `pub mod rlimit_windows;` 声明并公开模块。
- RustCodeGraph 对 `pkg/util/rlimit_windows.rs::GenRLimit` 的精确 callers 查询没有给出静态调用边；随后以 `rg` 搜索全部 Rust 源码，同样未发现生产调用者。

Go 版上游调用证据：

- `pkg/ddl/ingest/env.go`：`util.GenRLimit("ddl-ingest")` 初始化 `litRLimit`；`pkg/ddl/ingest/config.go` 将其转换为 `int` 并设置 `MaxOpenFiles`。
- `pkg/executor/importer/import.go`：`tidbutil.GenRLimit("table_import")` 的结果直接设置本地导入后端的 `MaxOpenFiles`。

下游依赖：

- `GenRLimit` 没有函数调用或系统 API 下游；RustCodeGraph 的精确 callees 查询没有返回调用边，这与函数体仅丢弃参数并返回字面量相符。
- `pkg/util/Cargo.toml` 的 Windows `windows-sys` 依赖由该 crate 的其他 Windows 能力使用；不能据此声称本文件访问 Win32 资源限制接口。

## 错误处理与边界

该 API 不返回 `Result`，也不会 panic；固定返回路径没有可传播的错误。它刻意不尝试查询 Windows 系统资源，因此不存在查询失败、权限不足或系统值转换失败的分支。

重要边界是数值语义而非输入校验：`1024` 是保守兼容值，不代表真实平台容量。调用方若把它转换为较小整数类型或用于扣减保留量，应自行确保不会溢出或下溢；当前 Go 调用方转换为 `int` 时，`1024` 可安全表示。本文件不负责验证后续配额计算。

## 并发与资源生命周期

函数是纯读取式、无状态实现，同一进程中的多个线程可以并发调用而无需锁或原子操作。每次调用只产生栈上的借用和返回值，不启动任务、不创建线程、不打开句柄、不持有文件描述符，也没有需要显式关闭或回收的资源。

由于返回值不会缓存系统状态，所以也不存在刷新周期或竞态；代价是它永远不会反映运行时 Windows 资源配置的变化。

## 与 Go 版本的对应关系

Rust 文件用 `#![cfg(windows)]` 表达相同的平台条件，用 `&str` 避免取得字符串所有权，并以 `let _ = source;` 显式保留未使用参数。

非 Windows 对照 `pkg/util/rlimit_other.go` 与 `pkg/util/rlimit_other.rs` 会读取 `RLIMIT_NOFILE` 的软限制，并在读取失败时记录警告、回退到 `1024`。

测试方面，`pkg/util/rlimit_other_test.rs` 验证了非 Windows Rust 辅助函数的成功值与失败回退，`pkg/util/cpu_posix_1_aster_unit_test.rs::printer_and_rlimit_are_operational` 只调用 `rlimit_other::GenRLimit`。仓库中没有针对 `rlimit_windows.rs` 的独立 Rust 测试，也未发现直接验证 Go Windows 函数的测试；现有非 Windows 测试不能证明 Windows 条件编译与固定返回契约。

## 扩展指南

若只需继续对齐 Go，优先保持 `GenRLimit` 的名称、参数与固定返回值不变。若 Go 版未来改为读取真实 Windows 限制，Rust 版应在同一提交范围内同步其可观察行为，并明确所用 Win32 API、错误回退和日志契约；不要仅因 `Cargo.toml` 已有 `windows-sys` 就假设存在可直接替代 POSIX `RLIMIT_NOFILE` 的接口。

建议新增独立测试文件 `pkg/util/rlimit_windows_test.rs`，并在 `pkg/util/lib.rs` 中以 `#[cfg(all(test, windows))]` 和 `#[path = "rlimit_windows_test.rs"]` 挂载，至少断言空字符串与典型来源均返回 `1024`。测试逻辑不应写入生产源文件。跨平台 API 若进一步统一，可考虑由一个条件编译门面选择 `rlimit_windows` 或 `rlimit_other`，但在新增 Rust 生产调用者前应先核对 Go 的两个真实消费点及配额转换逻辑。

兼容风险主要是改变固定值会改变导入/DDL 后端的文件句柄预算；过高可能放大系统资源压力，过低会限制并发或吞吐。当前函数本身为常数时间、零分配，任何真实系统探测都会引入新的平台依赖、失败模式和潜在启动成本，需要同步测试和文档。
