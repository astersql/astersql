# `pkg/util/cgroup/cgroup_cpu.rs`

源文件：[cgroup_cpu.rs](cgroup_cpu.rs)。本文只描述当前实现；底层文件解析由相邻的 `cgroup.rs` 提供，平台公开门面由 `cgroup_cpu_linux.rs` 或 `cgroup_cpu_unsupport.rs` 提供。

## 文件定位

`cgroup_cpu.rs` 位于 `astersql-util-cgroup` crate 内，是 cgroup CPU 探测的“平台无关编排层”。它不直接解析 `/proc` 或 cgroup 文件，而是把 `cgroup.rs` 中的路径探测、挂载版本识别以及 v1/v2 文件解析函数组合成一份 `CPUUsage`，并为 `CPUUsage` 提供配额到 CPU 份额的换算。

模块入口 `lib.rs` 以私有模块 `mod cgroup_cpu` 装载本文件，并通过 `pub(crate) use cgroup_cpu::*` 供 crate 内的平台门面使用。Linux 上，[`cgroup_cpu_linux.rs`](cgroup_cpu_linux.rs) 的 `GetCgroupCPU` 和 `GetCPUPeriodAndQuota` 把真实根目录 `/` 传给本文件；非 Linux 由 [`cgroup_cpu_unsupport.rs`](cgroup_cpu_unsupport.rs) 直接返回降级结果，不进入本文件的文件系统探测流程。

## 核心职责

- `getCgroupCPU`：从可注入的根目录下定位本进程的 CPU cgroup，识别 v1、v2 或混合挂载，读取 period、quota、system time 和 user time，并组装 `CPUUsage`。
- `getCgroupCPUPeriodAndQuota`：执行相同的定位和版本分派，但只读取 period/quota，供轻量监控使用。
- `CPUUsage::CPUShares`：把有效配额换算为 `quota / period`；没有正数 period/quota 时退回 `NumCPU`。

可注入的 `root: &Path` 是测试边界：生产门面传 `/`，独立测试传临时目录模拟 `/proc` 与 `/sys/fs/cgroup`。因此文件本身不保存全局配置，也不决定 `NumCPU`；Linux 门面读取 `std::thread::available_parallelism()` 后补入该字段。

## 主要符号

- `pub(crate) fn getCgroupCPU(root: &Path) -> anyhow::Result<CPUUsage>`：crate 内入口。控制器查询使用 `"cpu,cpuacct"`，因为完整采样同时需要 CPU 配额和 cpuacct 用量。成功时填写 `Period`、`Quota`、`Stime`、`Utime`；`NumCPU` 保持默认值，留给平台门面填写。
- `pub(crate) fn getCgroupCPUPeriodAndQuota(root: &Path) -> anyhow::Result<(i64, i64)>`：只查询 `"cpu"` 控制器并返回 `(period, quota)`。它不读取 `cpu.stat` 或 `cpuacct.usage_*`，所以即使用量文件缺失，只要配额文件完整仍可成功。
- `impl CPUUsage { pub fn CPUShares(&self) -> f64 }`：公开方法。`Period > 0 && Quota > 0` 时返回浮点除法结果，否则返回 `NumCPU as f64`。这使 v1 的 `quota = -1`、v2 的 `max`（下层映射为 `-1`）以及异常的非正 period 都表示“无有效限制”。

本文件没有自定义常量、trait、结构体或条件编译项。`CPUUsage`、`procPathCGroup`、`procPathMountInfo` 和所有探测函数均通过 `use super::*` 从 crate 的相邻模块进入作用域；错误类型和 `anyhow!` 来自唯一运行时依赖 `anyhow`。

## 执行流程

`getCgroupCPU` 的主流程如下：

1. 用 `join_root(root, procPathCGroup)` 得到可测试的 `/proc/self/cgroup` 路径，调用 `detectControlPath(..., "cpu,cpuacct")` 取得本进程的控制组相对路径；空路径立即报“未探测到 CPU controller”。
2. 用 `getCgroupDetails(join_root(root, procPathMountInfo), path, "cpu,cpuacct")` 获得平行的 `mounts` 与 `versions`。下层约定单挂载返回一个元素，混合 v1/v2 返回 `[v1, v2]` 和 `[1, 2]`。
3. 若 `mounts.len() == 2`，按该约定构造 v1 根目录与附加控制组路径的 v2 根目录。配额和用量分别先读 v2；每一类读取失败时各自用 `.or_else(...)` 回退 v1。因此可能出现“配额来自 v2、用量来自 v1”的合法组合。
4. 单挂载时，v1 直接使用挂载路径，v2 再拼接控制组相对路径；随后依据 `versions[0]` 调用对应的 quota/usage 解析器。版本不是 1 或 2 时返回错误。
5. 全部读取成功后才把四个局部结果写入默认构造的 `CPUUsage` 并返回。

`getCgroupCPUPeriodAndQuota` 重用相同的探测、混合挂载优先级和单挂载分派，但省略用量读取。Linux 的 `GetCPUPeriodAndQuota` 直接调用它；`pkg/util/cgmon/cgmon.rs` 将该公开门面注入全局 cgroup monitor 的 CPU quota 探针。

`CPUShares` 是后续换算阶段：Linux `CPUQuotaToGOMAXPROCS` 对结果取 `ceil()` 并应用最小值；`pkg/util/cpu/cpu.rs` 的采样流程用 shares 归一化用户态与内核态 CPU 速率。

## 数据与状态

`CPUUsage` 定义在 [`cgroup.rs`](cgroup.rs)，本文件涉及的字段为：

- `Period: i64`、`Quota: i64`：v1 来自 `cpu.cfs_period_us` / `cpu.cfs_quota_us`，v2 来自 `cpu.max`。v2 的 `max` 被下层转换为 `Quota = -1`。
- `Stime: u64`、`Utime: u64`：v1 来自 `cpuacct.usage_sys` / `cpuacct.usage_user`，v2 来自 `cpu.stat` 的 `system_usec` / `user_usec`。当前结构不统一单位：v1 文件值按纳秒语义，v2 按微秒语义；本文件只原样聚合，调用方不可假定两者单位相同。
- `NumCPU: i32`：本文件不设置。真实 Linux 门面成功探测后用可用并行度填充；测试直接调用内部函数时通常保持 0。

所有状态均为单次调用的栈上局部值。`CPUShares` 只读 `&self`，没有缓存或隐藏状态。混合挂载场景中的 v2→v1 回退按“配额”和“用量”两条独立数据链进行，不要求四个字段来自同一 cgroup 版本。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 声明：crate 名为 `astersql-util-cgroup`，库入口是 `lib.rs`，运行时依赖只有 `anyhow = "1"`；`libc`、`regex`、`tempfile` 仅用于测试。

直接上游：

- RustCodeGraph 的 `node pkg/util/cgroup/cgroup_cpu.rs::getCgroupCPU` 显示 [`cgroup_cpu_linux.rs::GetCgroupCPU`](cgroup_cpu_linux.rs) 调用内部 `getCgroupCPU`。
- `cgroup_cpu_linux.rs::GetCPUPeriodAndQuota` 调用 `getCgroupCPUPeriodAndQuota`；其公开函数被 `pkg/util/cgmon/cgmon.rs` 的 `GLOBAL_MONITOR` 作为探针使用。
- `cgroup_cpu_linux.rs::CPUQuotaToGOMAXPROCS` 以及 `pkg/util/cpu/cpu.rs::cpu_share_from_result` 调用 `CPUUsage::CPUShares`；`pkg/util/cpu/cpu.rs::Observer::Start/observe` 通过公开 `GetCgroupCPU` 间接进入本文件。

直接下游均在 [`cgroup.rs`](cgroup.rs)：`join_root`、`detectControlPath`、`getCgroupDetails`、`detectCPUQuotaInV1/V2`、`detectCPUUsageInV1/V2`。RustCodeGraph 未为 `use super::*` 引入的这些调用生成 callee 边，以上关系由函数体与相邻定义直接核验。

## 错误处理与边界

- 所有 I/O、文本解析和挂载探测错误使用 `anyhow::Result` 及 `?` 原样向上传播；具体路径上下文由 `cgroup.rs` 的下层函数附加。
- 控制器路径为空时，本文件主动产生 `no cpu controller detected`。独立 Linux 集成测试允许内核版本不高于 4.7 的容器出现该错误，新内核则视为失败。
- 单挂载分支假定 `getCgroupDetails` 成功时 `mounts[0]` 与 `versions[0]` 存在；这一不变量由该函数只在至少找到一个挂载时返回 `Ok` 保证。不要绕过该函数直接构造空列表。
- `mounts.len() == 2` 是当前混合模式判据，而不是一般的“至少两个”。下层当前最多返回 v1、v2 各一个；若未来允许多个挂载，必须同步修改本文件，避免把长度大于 2 的结果错误送入单挂载分支。
- 混合模式只在 v2 读取失败时回退 v1；若 v2 成功，即使其值表示无限制，也不会选 v1。两种读取都失败时返回 v1 的最终错误，因为 `.or_else` 会以回退调用结果替换首个错误。
- `CPUShares` 不校验 `NumCPU` 的正值。默认 `CPUUsage` 会得到 `0.0`，而 `pkg/util/cpu/cpu.rs` 在探测错误时确实以默认值调用该方法；扩展调用方需要自行理解零 shares 的后续除法风险，不能把该方法当成总会返回正数的 API。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道或长期持有的文件句柄。每次调用独立读取当前文件系统快照；下层 `File`/`BufReader` 在调用结束时由 RAII 释放，所以多个线程并发调用不会共享本文件内的可变状态。

快照不是事务性的：定位 `/proc`、读取 quota 和读取 usage 分多次 I/O 完成，期间 cgroup 挂载或成员关系可能变化；混合模式下甚至允许配额与用量来自不同版本。这是当前 Go/Rust 共同的尽力探测语义。需要强一致快照的新功能不能仅在此文件加锁，因为内核文件本身仍会独立变化。

测试中的并发只出现在 [`cgroup_cpu_test.rs`](cgroup_cpu_test.rs)：它启动工作线程制造 CPU 活动，采样后用原子退出标记停止并 `join`；该生命周期验证公开探测在活动负载下可用，不代表本文件创建后台工作。

## 与 Go 版本的对应关系

直接对照文件是 [`cgroup_cpu.go`](cgroup_cpu.go)。三个 Rust 符号与 Go 的 `getCgroupCPU`、`getCgroupCPUPeriodAndQuota`、`CPUUsage.CPUShares` 一一对应：控制器字符串、v1/v2 路径拼接、混合模式 v2 优先并回退 v1、未知版本错误，以及非正 quota/period 回退 `NumCPU` 的规则均保持一致。

语法层差异不改变主流程：Go 通过具名返回值逐字段写 `CPUUsage`，Rust 先取得元组再写结构体；Go 使用包级 `errNoCPUControllerDetected` 便于直接比较，Rust 在本文件现场构造同文案的 `anyhow` 错误，测试和调用方只能按错误内容/传播结果判断；Go 的 `filepath.Join` 对绝对片段有自身语义，Rust 使用 `join_root` 丢弃根目录/父目录等非普通分量，以便把绝对风格 fixture 安全地放进临时根。

平台层也基本对齐：Go/Rust Linux 门面都补 `NumCPU`、公开 period/quota，并以 `ceil(CPUShares)` 推导建议并行度。当前 Rust 没有 Go runtime 的真实 `GOMAXPROCS` 旋钮，相关差异由 `cgroup.rs::SetGOMAXPROCS` 的模块内状态模拟承担，不属于本文件职责。

## 扩展指南

- 新增 CPU 指标字段时，先在 `cgroup.rs::CPUUsage` 和对应 v1/v2 解析器定义单位与缺失值语义，再在 `getCgroupCPU` 的两个版本分支中组装；同时扩展独立的 `cgroup_mock_test.rs`，不要把测试嵌入生产源文件。
- 修改控制器选择、挂载优先级或回退策略时，要同时更新 `getCgroupCPU` 与 `getCgroupCPUPeriodAndQuota`，并与 Go `cgroup_cpu.go` 保持增量语义一致。尤其应覆盖纯 v1、纯 v2、混合模式、控制器顺序反转、namespace 相对挂载、v2 失败回退 v1。
- 修改 `CPUShares` 时必须同步检查 `cgroup_cpu_linux.rs::CPUQuotaToGOMAXPROCS` 和 `pkg/util/cpu/cpu.rs::observe`；舍入、零值和无限制语义会直接影响并行度与 CPU 利用率。性能上该方法本身是常数时间，主要成本来自每次探测的多次文件 I/O。
- 若要暴露新的公开 API，应在平台门面中接线而不是把基于 `root` 的测试辅助入口公开到 crate 外；非 Linux 的 `cgroup_cpu_unsupport.rs` 也必须提供语义匹配的降级实现。
- 兼容性风险集中在内核文件格式、v1/v2 单位差异和错误文本。保持 `anyhow` 路径上下文，并同步 Go 测试 fixture 与 Rust 独立测试，可避免只在一种 cgroup 布局上“通过”。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/cgroup` 确认目标及相邻 Go/Rust 文件已索引；`node --file pkg/util/cgroup/cgroup_cpu.rs` 列出全文和“used by `cgroup_cpu_linux.rs`”；`query getCgroupCPU`、`query getCgroupCPUPeriodAndQuota` 定位 Rust/Go 对照符号；精确 `node pkg/util/cgroup/cgroup_cpu.rs::getCgroupCPU` 给出到 Linux 门面的 caller trail。图未解析通配导入的内部 callee，故用源码核验补齐。
- 已读生产路径：`pkg/util/cgroup/cgroup_cpu.rs`、`cgroup.rs`、`cgroup_cpu_linux.rs`、`cgroup_cpu_unsupport.rs`、`lib.rs`、`Cargo.toml`；直接上游另核验 `pkg/util/cpu/cpu.rs` 与 `pkg/util/cgmon/cgmon.rs`。
- 已读 Go 对照：`pkg/util/cgroup/cgroup_cpu.go`、`cgroup_cpu_linux.go`、`cgroup_cpu_unsupport.go`。
- 已读独立测试：`pkg/util/cgroup/cgroup_cpu_test.rs`、`cgroup_mock_test.rs`、`cgroup_cpu_linux_1_aster_unit_test.rs`，并对照 `cgroup_cpu_test.go`、`cgroup_mock_test.go`。测试覆盖真实容器探测、v1/v2 与混合 fixture、错误路径、控制器顺序反转、无限配额及 `CPUShares() == 2.5`。
- 本任务只新增说明文档，不运行 Cargo。交付前按任务给定命令校验本文恰有 11 个固定二级标题，并人工检查所有运行时结论均能回指上述符号、调用点或测试。
