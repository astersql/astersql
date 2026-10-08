# `pkg/util/cgroup/cgroup_cpu_linux.rs`

## 文件定位

该文件是 `astersql-util-cgroup` crate 的 Linux CPU 平台层。crate 入口 `pkg/util/cgroup/lib.rs` 仅在 `target_os = "linux"` 时声明并公开重导出 `cgroup_cpu_linux`；非 Linux 构建改用 `cgroup_cpu_unsupport.rs`。它不负责解析 cgroup 文件格式，而是把平台无关解析器 `cgroup_cpu.rs` 包装成面向真实根文件系统 `/` 的公开 API，并额外提供容器环境探测。

crate 边界由 `pkg/util/cgroup/Cargo.toml` 确定：包名为 `astersql-util-cgroup`，库入口是 `lib.rs`，生产依赖仅有 `anyhow`。本文件通过 `use super::*` 使用同 crate 的 `CPUUsage`、`CPUQuotaStatus`、proc 路径常量以及 `getCgroupCPU`、`getCgroupCPUPeriodAndQuota` 等符号。

## 核心职责

- `GetCgroupCPU` 将平台无关探测器固定到真实根目录 `/`，取得当前进程 cgroup 的 CPU 周期、配额及用户态/内核态累计用量，并用 `std::thread::available_parallelism` 填充 `CPUUsage::NumCPU`。
- `CPUQuotaToGOMAXPROCS` 把 `CPUUsage::CPUShares()` 向上取整为建议并行度，并按调用者给定的正下限钳制结果，同时返回能够区分“使用配额”与“使用下限”的状态。
- `GetCPUPeriodAndQuota` 提供只读取 period/quota 的窄接口，避免需要完整 CPU 用量的调用约束。
- `InContainer` 以 `/proc/self/cgroup` 中的常见运行时标记，或 `/proc/self/mountinfo` 中根挂载的 overlay 文件系统，做启发式容器判断。

这些职责使上层无需了解 cgroup v1/v2 的文件布局：真实解析和 v2 优先、v1 回退逻辑位于 `pkg/util/cgroup/cgroup_cpu.rs` 与 `pkg/util/cgroup/cgroup.rs`。

## 主要符号

- `pub fn GetCgroupCPU() -> anyhow::Result<CPUUsage>`：公开 Linux 采样入口。先调用 `getCgroupCPU(Path::new("/"))`；成功后将可见并行度写入 `NumCPU`。查询系统并行度失败时回退到 `1`，不会因该查询单独返回错误。
- `pub fn CPUQuotaToGOMAXPROCS(minValue: i32) -> anyhow::Result<(i32, CPUQuotaStatus)>`：调用 `GetCgroupCPU`，对 `CPUShares()` 使用 `ceil`。仅当 `minValue > 0 && max < minValue` 时返回 `(minValue, CPUQuotaMinUsed)`；其余情况返回 `(max, CPUQuotaUsed)`。
- `pub fn GetCPUPeriodAndQuota() -> anyhow::Result<(i64, i64)>`：把真实根目录传给 `getCgroupCPUPeriodAndQuota`，原样传播结果。
- `pub fn InContainer() -> bool`：按短路或顺序检查 `procPathCGroup` 与 `procPathMountInfo`。
- `pub(crate) fn inContainer(path: &Path) -> bool`：读取单个 proc 文件；读取失败返回 `false`，读取成功后交给纯内容函数。
- `pub(crate) fn in_container_content(path: &Path, content: &[u8]) -> bool`：不要求 UTF-8 的容器标记匹配器，也是可用伪内容独立测试的最小边界。

文件没有自定义类型、trait、常量或 `impl`；数据类型和份额计算分别定义在 `pkg/util/cgroup/cgroup.rs` 的 `CPUUsage`/`CPUQuotaStatus` 与 `pkg/util/cgroup/cgroup_cpu.rs` 的 `CPUUsage::CPUShares`。

## 执行流程

1. 完整 CPU 采样从 `GetCgroupCPU` 进入，以 `/` 调用 `getCgroupCPU`。后者从 `/proc/self/cgroup` 定位 `cpu,cpuacct` 控制路径，再从 mountinfo 定位挂载；混合挂载优先读取 v2，失败后回退 v1，并组装 `CPUUsage`。
2. 平台解析成功后，`GetCgroupCPU` 查询当前进程可用并行度。查询成功使用非零 `usize`，失败使用 `1`，随后转换为 `i32` 写入 `NumCPU` 并返回。
3. `CPUQuotaToGOMAXPROCS` 在上述结果上调用 `CPUShares`：正的 period/quota 使用 `quota / period`，无有效限制则使用 `NumCPU`。份额向上取整后，再决定是否采用正的 `minValue` 下限。
4. `GetCPUPeriodAndQuota` 走较窄的 `cpu` 控制器探测路径，仅返回 `(period, quota)`；`pkg/util/cgmon/cgmon.rs` 把它作为全局 cgroup 监控器的探针，每 10 秒刷新资源信息。
5. `InContainer` 先读 cgroup 路径文件；内容包含字节串 `docker`、`kubepods` 或 `containerd` 即返回 `true`。否则再读 mountinfo，逐行按空格切字段，仅当字段数大于 8、字段 4 为 `/`、字段 8 为 `overlay` 时返回 `true`。

## 数据与状态

本文件不保存全局可变状态。每次调用都即时读取 proc/cgroup 文件，结果装入值类型 `CPUUsage`；字段包括 `Stime`、`Utime`、`Period`、`Quota` 和本文件补充的 `NumCPU`。period/quota 的单位沿底层内核文件语义，累计用量在 cgroup v1/v2 间可能分别为纳秒/微秒，调用者不能在未经版本归一化的情况下假定统一单位。

`CPUQuotaStatus` 是结果标签而非状态机：本文件只产生 `CPUQuotaUsed` 和 `CPUQuotaMinUsed`。`CPUQuotaUndefined` 由非 Linux 占位实现或其他层的契约使用；Linux 路径探测失败直接返回 `Err`，不会以该枚举值替代错误。

容器探测只处理调用期间读到的 `Vec<u8>`，不缓存内容。字节级搜索保留 Go 字符串可容纳任意字节的语义，因而即使 proc 内容不是合法 UTF-8，运行时标记仍可被识别。

## 依赖与调用关系

下游调用边经 RustCodeGraph 核对：`GetCgroupCPU -> getCgroupCPU`，`CPUQuotaToGOMAXPROCS -> GetCgroupCPU`，`GetCPUPeriodAndQuota -> getCgroupCPUPeriodAndQuota`，`InContainer -> inContainer`，`inContainer -> in_container_content`。底层探测器再依赖 `detectControlPath`、`getCgroupDetails` 以及 v1/v2 配额和用量解析函数。

上游精确搜索显示：

- `pkg/util/cpu/cpu.rs` 的 `Observer::Start` 先用 `GetCgroupCPU` 判断是否支持采样，`observe` 周期性再次采样并用 `CPUShares` 归一化 CPU 使用率。
- `pkg/util/cgroup/cgroup.rs::SetGOMAXPROCS` 调用 `CPUQuotaToGOMAXPROCS(1)`，将建议值保存到模块内的 `MAX_PROCS`，并返回恢复闭包；Rust 并未直接修改进程调度器。
- `pkg/util/cgmon/cgmon.rs` 将 `GetCPUPeriodAndQuota` 注入进程级 `GLOBAL_MONITOR`。
- `pkg/util/memory/meminfo.rs::InitMemoryHook` 用 `InContainer` 选择 cgroup 或普通内存探针；`pkg/util/cpu/cpu_test.rs` 也用它决定真实容器测试是否执行。

RustCodeGraph 对目标公开符号未返回 callers，因此上述上游列表由限定 Rust 源文件的精确 `rg` 搜索补齐；没有证据表明它列出了动态函数值传播后的所有间接调用。

## 错误处理与边界

`GetCgroupCPU` 和 `GetCPUPeriodAndQuota` 使用 `?` 原样传播底层 `anyhow::Error`，包括 proc 文件不可读、找不到控制器、mountinfo 无匹配、cgroup 值格式非法或未知版本等错误。`CPUQuotaToGOMAXPROCS` 也会在换算前传播完整采样错误。与 Go 版本不同，Rust `GetCgroupCPU` 在底层失败时无法同时返回一个已经填好 `NumCPU` 的部分 `CPUUsage`；其 `Result` 直接为 `Err`。

系统并行度探测是例外：`available_parallelism` 失败被安全降级成 `1`。从 `usize` 到 `i32` 使用 `as` 转换；在现实 CPU 数范围内可用，但代码没有显式检查超过 `i32::MAX` 的理论溢出边界。

容器判断有意吞掉文件读取错误并返回 `false`，因此它表达“未从这些启发式规则识别为容器”，并不证明进程一定运行在裸机。标记集合并不覆盖所有容器运行时，overlay 也不是唯一存储驱动；mountinfo 规则依赖字段位置与精确空格切分。任一路径得到 `true` 后都会短路，不再读取下一文件。

配额换算使用浮点除法和向上取整：例如 `quota/period = 2.5` 得到 `3`。无效或无限配额由 `CPUShares` 回退到 `NumCPU`，但极端的底层数值转换边界没有在本文件额外校验。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道或长期句柄。`fs::read` 在单次调用内打开、读取并关闭文件；所有路径和缓冲区均为局部值，因此多个线程并发调用这些函数时没有本文件级共享状态竞争。

并发影响来自调用方而非本文件：`pkg/util/cpu/cpu.rs` 的后台线程约每 100ms 调用 `GetCgroupCPU`，`pkg/util/cgmon/cgmon.rs` 的全局监控器周期性调用 `GetCPUPeriodAndQuota`。每次采样均重新访问 proc/cgroup 文件，扩展时应避免在这些热路径加入无界扫描、阻塞等待或跨调用持锁。

独立真实环境测试 `pkg/util/cgroup/cgroup_cpu_test.rs` 在 10 个空转线程存在时执行一次采样，并通过原子退出标记和 `join` 清理线程；这验证并发负载下的读取，但不等价于对本文件进行高并发竞态压力测试。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/cgroup/cgroup_cpu_linux.go`，四个公开入口及容器判定顺序基本一一对应：真实根目录采样、`CPUShares` 向上取整与下限状态、窄 period/quota API，以及 cgroup 标记/根 overlay 判断均保持一致。

已确认的差异如下：

- Go 文件由 `//go:build linux` 选择，Rust 由 `lib.rs` 的 `#[cfg(target_os = "linux")]` 选择。
- Go `GetCgroupCPU` 含 `GetCgroupCPUErr` failpoint，并且即便底层返回错误仍会给局部结果写入 `runtime.NumCPU()`；本 Rust 函数自身没有 failpoint，且 `?` 使错误立即返回。Rust CPU 观测器在 `pkg/util/cpu/cpu.rs::Observer::Start` 另有同名 failpoint 兼容入口，但它不改变直接调用本函数的行为。
- Go `CPUQuotaToGOMAXPROCS` 的错误返回还携带 `-1` 与 `CPUQuotaUndefined`；Rust 用 `Result` 的 `Err` 表达失败，不携带这两个占位值。
- Go `runtime.NumCPU()` 直接返回整数；Rust 使用 `available_parallelism()`，查询失败时明确降级为 `1`。
- Go 把文件内容转换成可含任意字节的字符串再搜索；Rust 直接在字节切片上搜索。`pkg/util/cgroup/cgroup_cpu_linux_test.rs` 的非 UTF-8 回归用例确认两者在该边界上语义一致。

`pkg/util/cgroup/cgroup_cpu_test.go` 与 Rust 的 `cgroup_cpu_test.rs` 都在容器中、并发空转线程存在时读取 CPU 信息，并对旧内核缺控制器做兼容处理。Rust 迁移补充测试 `cgroup_cpu_linux_1_aster_unit_test.rs` 进一步用临时根验证 v1/v2 解析、v2 端到端份额 `2.5` 和异常格式错误上下文。

## 扩展指南

- 新增或调整公开 Linux API 时，应在本文件做真实根目录的薄包装，把可注入 root、可独立测试的解析逻辑放在 `cgroup_cpu.rs`/`cgroup.rs`，并检查 `lib.rs` 的 Linux 导出及 `cgroup_cpu_unsupport.rs` 的跨平台契约。
- 扩展容器运行时标记或 mountinfo 规则，应优先修改 `in_container_content`，保留字节输入以覆盖非 UTF-8 数据；同步扩展独立的 `cgroup_cpu_linux_test.rs`，而不是把测试内嵌进生产源文件。
- 修改配额舍入、下限或状态语义时，应同时核对 `CPUUsage::CPUShares`、`SetGOMAXPROCS`、CPU 观测器归一化逻辑，并补充独立 Rust 测试和对应 Go 测试语义。特别注意零/负 period、无限 quota、非正 `minValue`、小数份额和超大值。
- 修改错误契约前需评估 Rust/Go 差异：直接调用者可能依赖 `Err`，而 Go 调用者可能仍检查部分结果或 `CPUQuotaUndefined`。若要补齐 failpoint，也应验证它对所有直接调用方的影响，而不是只覆盖 CPU 观测器。
- 性能上，这些 API 可能在 100ms 采样循环中反复执行；新增工作应保持有界，避免全文件系统遍历、额外线程或跨采样缓存陈旧的 cgroup 路径而没有失效策略。

## 验证依据

- RustCodeGraph 索引状态：项目共索引 11,467 个文件；目标文件被识别为 82 行、11 个图节点。读取并核对了 `cgroup_cpu_linux.rs`、`cgroup_cpu.rs`、`cgroup.rs`、`lib.rs`、`cgroup_cpu_test.rs`、`cgroup_cpu_linux_test.rs`、`cgroup_cpu_linux_1_aster_unit_test.rs`、`pkg/util/cpu/cpu.rs`、`pkg/util/cgmon/cgmon.rs` 与 `pkg/util/memory/meminfo.rs`。
- 图查询确认的主要调用边：`GetCgroupCPU -> getCgroupCPU`、`CPUQuotaToGOMAXPROCS -> GetCgroupCPU`、`GetCPUPeriodAndQuota -> getCgroupCPUPeriodAndQuota`、`InContainer -> inContainer`；`in_container_content` 没有下游函数调用。
- 配置与 Go 对照：读取了 `pkg/util/cgroup/Cargo.toml`、`pkg/util/cgroup/cgroup_cpu_linux.go` 和 `pkg/util/cgroup/cgroup_cpu_test.go`；精确 `rg` 搜索补齐了图未返回的 Rust/Go 上游调用点及 `GetCgroupCPUErr` 差异。
- 测试证据：`cgroup_cpu_linux_test.rs` 覆盖非 UTF-8 标记；`cgroup_cpu_test.rs` 覆盖容器真实采样与旧内核边界；`cgroup_cpu_linux_1_aster_unit_test.rs` 覆盖临时根上的控制器匹配、v1/v2 文件、v2 端到端份额和错误上下文。
- 本任务是只新增说明文档的静态分析，按计划不运行 Cargo。交付前使用任务规定的命令验证目标文档存在且恰有十一个固定二级章节，并人工复核源码链接、符号名称、范围和 Go 差异。
