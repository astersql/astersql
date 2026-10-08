# `pkg/util/cgroup/cgroup_cpu_unsupport.rs`

源文件：[cgroup_cpu_unsupport.rs](cgroup_cpu_unsupport.rs)。本文描述非 Linux 目标上的 CPU cgroup 兼容实现；Linux 的真实探测位于 [`cgroup_cpu_linux.rs`](cgroup_cpu_linux.rs)，共享数据类型位于 [`cgroup.rs`](cgroup.rs)。

## 文件定位

`cgroup_cpu_unsupport.rs` 属于 `astersql-util-cgroup` crate，是非 Linux 平台的 CPU cgroup 公开门面。crate 入口 [`lib.rs`](lib.rs) 只在 `#[cfg(not(target_os = "linux"))]` 下声明并重导出本模块；Linux 构建改为声明和重导出 `cgroup_cpu_linux.rs`，所以两份平台文件提供相同名称的公开 API，却不会同时进入一个构建产物。

本文件不读取 `/proc`、cgroup v1/v2 控制文件，也不进入 [`cgroup_cpu.rs`](cgroup_cpu.rs) 的平台无关探测编排。它保留 `GetCgroupCPU`、`GetCPUPeriodAndQuota`、`CPUQuotaToGOMAXPROCS`、`InContainer` 四个调用契约，让上层 crate 无需为非 Linux 单独改写导入和控制流；返回值明确表示“只有主机并行度可用，cgroup CPU 配额和容器判断不受支持”。

## 核心职责

- 为非 Linux 构建提供与 Linux 门面同形的四个公开函数，维持跨平台编译边界。
- `GetCgroupCPU` 尽力取得当前进程可用的逻辑并行度，只填写 `CPUUsage::NumCPU`，其余采样字段使用默认零值。
- `GetCPUPeriodAndQuota` 以 `(-1, -1)` 表示没有可读取的 period/quota。
- `CPUQuotaToGOMAXPROCS` 忽略最小值参数，以 `(-1, CPUQuotaUndefined)` 告诉 `SetGOMAXPROCS` 不应应用 cgroup 推导值。
- `InContainer` 固定返回 `false`，使依赖容器判断的上层逻辑走非容器降级路径。

这是兼容桩而非尚待补齐的 Linux 文件解析器：目标 OS 的条件编译决定这里不应尝试 Linux 专有的 `/proc/self/cgroup` 和 mountinfo 探测。

## 主要符号

- `pub fn GetCgroupCPU() -> anyhow::Result<CPUUsage>`：调用 `std::thread::available_parallelism()`；成功时取 `NonZeroUsize::get()`，失败时回退为 `1`，转换为 `i32` 后写入 `NumCPU`。`CPUUsage { ..Default::default() }` 使 `Stime`、`Utime`、`Period`、`Quota` 均为 `0`。当前函数保留 `Result` 以匹配 Linux API，但内部所有分支都返回 `Ok`。
- `pub fn GetCPUPeriodAndQuota() -> anyhow::Result<(i64, i64)>`：始终返回 `Ok((-1, -1))`。两个 `-1` 是“不支持/无配额”的哨兵，不是读取到的调度周期和配额。
- `pub fn CPUQuotaToGOMAXPROCS(_: i32) -> anyhow::Result<(i32, CPUQuotaStatus)>`：参数以 `_` 明确丢弃；始终返回 `Ok((-1, CPUQuotaStatus::CPUQuotaUndefined))`，不会采用或钳制调用方给出的最小值。
- `pub fn InContainer() -> bool`：始终为 `false`。

本文件没有模块级常量、自定义类型、trait 或 `impl`。`CPUUsage`、`CPUQuotaStatus` 由 `use super::*` 从 crate 父模块作用域取得，`Result` 来自 crate 唯一的生产依赖 `anyhow`；`#![allow(non_snake_case)]` 保留与 Go API 一致的符号命名。

## 执行流程

非 Linux 构建时，`lib.rs` 装载本文件并将四个函数重导出到 `astersql-util-cgroup` crate 根。之后各入口彼此独立：

1. 完整 CPU 快照请求进入 `GetCgroupCPU`，先向标准库查询可用并行度；成功则使用该正整数，查询失败则使用保守值 `1`，最后返回仅设置 `NumCPU` 的 `CPUUsage`。
2. 轻量配额请求进入 `GetCPUPeriodAndQuota`，不执行 I/O，直接返回两个 `-1`。
3. 并行度建议请求进入 `CPUQuotaToGOMAXPROCS`，不读取 CPU 快照，直接声明配额未定义。上层 [`cgroup.rs::SetGOMAXPROCS`](cgroup.rs) 看到 `CPUQuotaUndefined` 后返回空操作撤销闭包，不修改其模块内 `MAX_PROCS`。
4. 容器判断进入 `InContainer`，固定返回 `false`。例如 `pkg/util/memory/meminfo.rs::InitMemoryHook` 因而继续比较 cgroup 内存限制与物理内存，而不会走“已在容器中”的提前返回分支。

`pkg/util/cpu/cpu.rs::Observer::Start` 和 `observe` 通过 crate 重导出调用 `GetCgroupCPU`。在非 Linux 上，启动探测会成功，后续 CPU share 由 `CPUUsage::CPUShares` 回退到 `NumCPU`，即用主机可用并行度归一化进程 CPU 用量，而不是按容器配额归一化。

## 数据与状态

函数返回的 `CPUUsage` 定义在 `cgroup.rs`。本实现只设置：

- `NumCPU`：来自 `available_parallelism`，其 API 保证成功值非零；查询失败时显式取 `1`。转换使用 `as i32`，当前实现没有对理论上大于 `i32::MAX` 的 `usize` 做饱和或错误处理。
- `Stime = 0`、`Utime = 0`：没有 cgroup CPU 累计用量数据。
- `Period = 0`、`Quota = 0`：完整快照采用 `CPUUsage::default()` 的零值；单独的 period/quota API 则返回 `(-1, -1)`。调用方必须按具体 API 的契约理解两组不同哨兵，不能把完整快照的零值解释成真实采样。

本文件没有静态可变状态、缓存或配置。输入参数仅有 `CPUQuotaToGOMAXPROCS` 的 `i32` 最小值，但该平台实现有意忽略它。每次 `GetCgroupCPU` 都重新查询标准库可见的并行度。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 声明：包名是 `astersql-util-cgroup`，库入口为 `lib.rs`，生产依赖只有 `anyhow = "1"`；`libc`、`regex`、`tempfile` 都是测试依赖。本文件实际还使用标准库 `std::thread::available_parallelism`。

直接装配关系是 `lib.rs` 的 `#[cfg(not(target_os = "linux"))] mod cgroup_cpu_unsupport;` 与同条件的 `pub use cgroup_cpu_unsupport::*;`。RustCodeGraph 能定位本文件的四个符号，但对精确符号执行 `callers`/`callees` 没有生成边；因此调用关系以模块重导出和实际调用点补证：

- [`cgroup.rs::SetGOMAXPROCS`](cgroup.rs) 通过 `super::CPUQuotaToGOMAXPROCS(1)` 消费本平台实现。
- `pkg/util/cpu/cpu.rs::Observer::Start` 与 `observe` 通过其 crate 内的 `cgroup` 再导出调用 `GetCgroupCPU`。
- `pkg/util/memory/meminfo.rs::InitMemoryHook` 导入并调用 `InContainer`。
- `pkg/util/cgmon/cgmon.rs` 把 `GetCPUPeriodAndQuota` 配置为全局监控器探针，但 `StartCgroupMonitor`/`StopCgroupMonitor` 用 `cfg!(target_os = "linux")` 限制实际启动；因此正常非 Linux 流程不会周期调用这个 `(-1, -1)` 探针。

工作区依赖声明还显示 `pkg/util/cpu`、`pkg/util/memory`、`pkg/util/cgmon` 三个 crate 直接依赖 `astersql-util-cgroup`。本文件不直接参与 SQL、事务或存储调用链，而是这些资源观测/初始化模块的跨平台底层边界。

## 错误处理与边界

- 四个公开 API 中有三个返回 `anyhow::Result`，但当前 unsupported 实现不会构造或传播 `Err`。`GetCgroupCPU` 吞掉 `available_parallelism` 的错误并降级为单核；其余两个 `Result` API 直接返回哨兵值。
- `GetCgroupCPU` 的零 `Period`/`Quota` 与 `GetCPUPeriodAndQuota` 的 `-1/-1` 都表示不可用，但形式不同。扩展调用方应优先使用 `CPUQuotaStatus` 或正值检查，不应假设两个 API 的无配额表示完全相同。
- `InContainer = false` 只表示本实现无法/不尝试用 Linux cgroup 判定容器，并不证明进程不处于任何非 Linux 容器或隔离环境。
- `CPUQuotaToGOMAXPROCS` 的 `minValue` 在此平台没有效果；依赖“结果至少等于 minValue”的调用方必须先确认状态不是 `CPUQuotaUndefined`。
- 标准库返回的 `usize` 通过 `as i32` 转换，极端超大并行度会发生截断；现实硬件通常远低于该边界，但若未来加强健壮性，应与 Linux 门面和 Go 行为一起评估，不能只改单个平台。

## 并发与资源生命周期

本文件不创建线程、锁、原子变量、通道、异步任务或文件句柄。`available_parallelism` 是一次同步查询，所有返回数据均为拥有所有权的值；多个线程可并发调用这些函数，本文件内部没有共享可变状态或清理动作。

上层生命周期不由本文件拥有：CPU `Observer` 自己创建、停止并 join 采样线程；cgroup monitor 自己管理后台 worker，且在非 Linux 上不会启动；`SetGOMAXPROCS` 返回的空操作闭包由调用者决定何时消费。为本文件新增缓存或后台探测会改变这一无状态契约，需要同时定义同步、刷新和关闭规则。

## 与 Go 版本的对应关系

直接语义基准是 [`cgroup_cpu_unsupport.go`](cgroup_cpu_unsupport.go)，其 `//go:build !linux` 与 Rust `lib.rs` 的 `#[cfg(not(target_os = "linux"))]` 对应。四个 API 的主要降级结果一致：主机 CPU 数、`(-1, -1)`、`(-1, CPUQuotaUndefined)`、`false`。

存在两点当前差异：

- Go `GetCgroupCPU` 使用 `runtime.NumCPU()`；Rust 使用 `std::thread::available_parallelism()`，失败时显式回退 `1`。两者目的都是取得非零主机/进程可用并行度，但具体平台和亲和性语义由各自运行时决定。
- Go unsupported 入口包含 `GetCgroupCPUErr` failpoint，可按测试注入错误；Rust 本文件没有 failpoint。Rust 的 `pkg/util/cpu/cpu.rs::Observer::Start` 在调用 cgroup 前实现了同名 failpoint，用于 CPU 观察器级测试，但直接调用 Rust `GetCgroupCPU` 仍不会因该 failpoint 返回错误。

Rust 还使用 `anyhow::Result` 和元组承载 Go 的多返回值。API 形状虽有语言差异，`CPUQuotaUndefined` 才是上层判断不应应用配额的稳定语义；`-1` 数值不应单独替代状态判断。

## 扩展指南

- 增加或修改公开 CPU cgroup API 时，必须同时维护 `cgroup_cpu_linux.rs` 与本文件的同名签名，并检查 `lib.rs` 的互斥条件重导出；否则只会在某一目标 OS 暴露编译错误。
- 若新增可移植的 CPU 信息，优先使用标准库或明确支持目标平台的依赖，并给出无法查询时的哨兵/错误策略；不要在本文件直接假设 Linux `/proc` 或 cgroup 文件存在。
- 修改 `GetCgroupCPU` 的字段默认值时，同步审查 `cgroup_cpu.rs::CPUUsage::CPUShares` 与 `pkg/util/cpu/cpu.rs::observe`，避免产生零除、负 share 或把未知值当成真实用量。性能风险主要在把当前常数时间/一次系统查询的桩变成频繁 I/O。
- 修改容器判断时，同步审查 `pkg/util/memory/meminfo.rs::InitMemoryHook` 的分支语义；返回 `true` 会绕过后续的 cgroup/物理内存比较，并改变缓存探针选择。
- 回归测试应放在独立测试文件中。当前 [`cgroup_cpu_test.rs`](cgroup_cpu_test.rs) 会在 `InContainer()` 为 false 时提前返回，只间接确认非容器分支不执行 Linux 断言；仓库没有专门验证四个 non-Linux 哨兵的 Rust 测试。新增覆盖宜建立独立 `*_test.rs` 并由 `lib.rs` 挂接，同时对照 Go unsupported 行为；不要把测试嵌入生产源文件。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/util/cgroup` 确认目标、平台实现与独立测试均已索引；`node --file pkg/util/cgroup/cgroup_cpu_unsupport.rs` 阅读全部 45 行；`query` 分别定位 `GetCgroupCPU`、`GetCPUPeriodAndQuota`、`CPUQuotaToGOMAXPROCS`、`InContainer` 的 Rust/Go 定义。对四个目标符号执行精确 `callers`/`callees` 均无输出，故条件编译重导出边及上游调用点由源码与 Cargo 声明补证。
- 已读 Rust 生产/装配路径：`pkg/util/cgroup/cgroup_cpu_unsupport.rs`、`lib.rs`、`cgroup.rs`、`cgroup_cpu.rs`，以及直接上游 `pkg/util/cpu/cpu.rs`、`pkg/util/memory/meminfo.rs`、`pkg/util/cgmon/cgmon.rs`。
- 已读 crate 声明：`pkg/util/cgroup/Cargo.toml`，并核对 `pkg/util/cpu/Cargo.toml`、`pkg/util/memory/Cargo.toml`、`pkg/util/cgmon/Cargo.toml` 的直接依赖。
- 已读 Go 对照：`pkg/util/cgroup/cgroup_cpu_unsupport.go`；全仓调用搜索另确认 Go 的 CPU、内存上游位于 `pkg/util/cpu/cpu.go` 与 `pkg/util/memory/meminfo.go`。
- 已读独立测试：`pkg/util/cgroup/cgroup_cpu_test.rs` 与 `cgroup_cpu_test.go`。二者面向 Linux 真实容器；Rust 测试在 `InContainer() == false` 时提前返回，因此不构成 unsupported 返回值的逐项断言。`lib.rs` 没有挂接专门的 `cgroup_cpu_unsupport_test.rs`，这是当前验证边界而非推测出的覆盖。
- 本任务仅新增说明文档，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核文档只陈述上述符号、条件编译、调用点和测试可支持的事实。
