# `pkg/util/cgroup/cgroup.rs`

## 文件定位

`cgroup.rs` 是 `astersql-util-cgroup` crate 的共享基础层。crate 入口 `pkg/util/cgroup/lib.rs` 将本文件声明为私有模块并重导出其公开项；Linux 与非 Linux 的 CPU、内存实现均复用这里的数据类型、cgroup 路径常量、挂载探测和文件解析函数。crate 的 `Cargo.toml` 只为生产代码引入 `anyhow`，说明本文件直接依赖的能力集中在标准库文件系统/I/O与带上下文的错误传播，而不是 SQL、存储或网络子系统。

该文件本身不决定完整的 CPU 或内存策略。`pkg/util/cgroup/cgroup_cpu.rs` 负责把这里的探测原语组合成 CPU 采样，`pkg/util/cgroup/cgroup_memory.rs` 负责组合内存查询，`pkg/util/cgroup/cgroup_cpu_linux.rs` 再提供面向应用的 Linux 公开入口。因而它位于“读取 `/proc` 和 cgroup 控制文件”与“上层资源配额决策”之间。

## 核心职责

1. 定义跨平台公开数据契约：`CPUQuotaStatus`、`CPUUsage` 和 `Version`。
2. 用 `detectControlPath` 从 `/proc/self/cgroup` 找到进程所属的控制组路径，并用 `getCgroupDetails` 从 `/proc/self/mountinfo` 找到相应 v1/v2 挂载点。
3. 解析 cgroup v1 的 `cpu.cfs_*`、`cpuacct.usage_*`，以及 v2 的 `cpu.max`、`cpu.stat`；为内存实现提供 `readInt64Value` 和共享文件名/键名。
4. 通过 `SetGOMAXPROCS` 保留 Go API 的“设置并返回撤销闭包”可观察契约。Rust 没有 Go 运行时对应的进程级调度器旋钮，所以这里只更新模块内原子值 `MAX_PROCS`，并不改变 Rust 执行器或操作系统调度并行度。

## 主要符号

- `CPUQuotaStatus::{CPUQuotaUndefined, CPUQuotaUsed, CPUQuotaMinUsed}`：描述配额不可用、直接采用配额、或被最小值钳制三种结果；由平台实现 `CPUQuotaToGOMAXPROCS` 产生，`SetGOMAXPROCS` 消费。
- `CPUUsage { Stime, Utime, Period, Quota, NumCPU }`：一次 CPU 采样的数据载体。v1 的累计用量来自纳秒文件，v2 字段名为 `*_usec`；本结构不做单位归一化。`Period`/`Quota` 用于 `cgroup_cpu.rs::CPUUsage::CPUShares`，无有效正配额时回退到 `NumCPU`。
- `Version::{Unknown, V1, V2}`：内存查询等公开 API 返回的协议版本标记；文件同时重导出三个枚举成员。
- `SetGOMAXPROCS() -> Result<Box<dyn FnOnce() + Send>>`：尊重已有 `GOMAXPROCS` 环境变量；否则调用平台的 `CPUQuotaToGOMAXPROCS(1)`，必要时原子替换 `MAX_PROCS`，返回只能调用一次且可跨线程传递的恢复闭包。
- `join_root(root, child)`：仅拼接 `child` 的普通路径分量，忽略根目录、前缀、`.` 与 `..` 等非普通分量，使绝对风格的 proc/cgroup 路径可安全映射到测试临时根目录。
- `controllerMatch(field, controller)`：允许控制器字段的顺序和额外项不同，例如 `cpuacct,cpu` 或 `rw,cpuacct,cpu` 均可覆盖请求的 `cpu,cpuacct`。
- `detectControlPath(path, controller)`：逐行解析三段式 cgroup 记录；v1 精确匹配控制器后立即返回，并用 `fields[2..].join(":")` 恢复路径中的冒号；若无 v1 匹配则返回记录过的 unified v2 路径。
- `detectCgroupVersion(fields, controller)`：定位 mountinfo 可选字段后的 `-` 分隔符；识别带目标控制器的 `cgroup` 为 v1，任意 `cgroup2` 为 v2。
- `getCgroupDetails(path, cgroup_root, controller)`：扫描 mountinfo，最多保留最后一个匹配的 v1 和 v2 挂载；v1 会按 namespace root 计算相对路径，包含 `..` 的 root 被忽略；返回顺序固定为 v1 后 v2及相应版本数组。
- `detectCPUQuotaInV1` / `detectCPUUsageInV1`：通过私有泛型 `parse_file` 读取并解析两组单值文件，分别返回 `(period, quota)` 与 `(stime, utime)`。
- `detectCPUQuotaInV2`：解析 `cpu.max` 的一个或两个字段；`max` 映射为 `quota = -1`，省略 period 时返回 `0`。
- `detectCPUUsageInV2`：遍历 `cpu.stat`，只接收恰有两个字段的 `user_usec` 和 `system_usec`，忽略其他统计项，缺失项保持 `0`。
- `readInt64Value`：读取首行无符号整数；`max` 映射为 `i64::MAX as u64`，空文件按 Go 命名返回值行为映射为 `0`。

## 执行流程

CPU 完整采样的直接上游是 `cgroup_cpu.rs::getCgroupCPU`。它先调用 `detectControlPath(..., "cpu,cpuacct")`，再调用 `getCgroupDetails`。单一挂载按版本选择本文件的 v1 或 v2 配额/用量解析器；混合挂载则先读 v2，单项失败时回退 v1，最后填充 `CPUUsage`。只取配额的 `getCgroupCPUPeriodAndQuota` 使用相同定位流程，但控制器参数为 `cpu`，且不读取 usage 文件。

内存路径由 `cgroup_memory.rs::memory_location` 以 `"memory"` 控制器调用相同的控制路径和挂载探测。其 v1/v2 usage 与 v2 limit 最终调用 `readInt64Value`；memory.stat 的键值扫描则在内存文件中实现，但使用本文件声明的文件名和键常量。

`SetGOMAXPROCS` 的流程是：先检查进程环境是否显式设置 `GOMAXPROCS`；未设置时向当前平台实现请求最小值为 1 的建议并行度；状态为 `CPUQuotaUndefined` 时返回空操作闭包；其余状态用 `SeqCst` 原子交换记录新值，并把旧值捕获进恢复闭包。调用者必须执行该闭包才会恢复模块内先前状态。

## 数据与状态

绝大多数函数是无状态的同步解析器，输入为 `Path`、字符串切片或 mountinfo 字段，输出为值或 `anyhow::Result`。文件级常量集中声明 Linux cgroup 控制文件名和 `/proc/self/{cgroup,mountinfo}` 路径，供同 crate 的 CPU/内存模块复用。

唯一可变全局状态是 `MAX_PROCS: AtomicUsize`，初始值为 `0`。它仅由 `SetGOMAXPROCS` 及其撤销闭包以 `Ordering::SeqCst` 访问；源码没有公开读取器，因此它不直接影响其他模块的调度行为。`getCgroupDetails` 的 `Vec<String>` 与 `Vec<i32>` 保持位置对应，且同时发现两代挂载时约定索引 0 为 v1、索引 1 为 v2，这是直接调用者构造路径和回退顺序所依赖的不变量。

## 依赖与调用关系

- crate 边界：`pkg/util/cgroup/Cargo.toml` 将 `lib.rs` 设为库入口，生产依赖仅 `anyhow = "1"`；测试另用 `libc`、`regex`、`tempfile`。
- 模块装配：`pkg/util/cgroup/lib.rs` 的 `mod cgroup; pub use cgroup::*;` 暴露公开类型和函数，并按目标 OS 选择 Linux 或 unsupported CPU/内存模块。
- 直接上游：`cgroup_cpu.rs::{getCgroupCPU,getCgroupCPUPeriodAndQuota}` 调用控制路径、挂载和四个 CPU 解析器；`cgroup_memory.rs` 与 `cgroup_memory_unsupport.rs` 调用路径/挂载探测、`readInt64Value` 及内存常量。
- 直接下游：标准库的 `fs::File`、`fs::read_to_string`、`BufReader::lines`、`Path` 操作、环境变量与原子操作；错误附加上下文使用 `anyhow::{Context, anyhow}`。
- 应用方向：Linux 的 `GetCgroupCPU`、`CPUQuotaToGOMAXPROCS`、`GetCPUPeriodAndQuota` 和内存 `Get*` API 通过 `lib.rs` 重导出。目标文件不直接进入 SQL 请求链，而是为资源观测与并行度建议提供底层数据。

RustCodeGraph 的文件节点确认 `cgroup.rs` 被 crate 内 CPU、内存实现及测试使用；精确符号查询同时定位到 Go/Rust 的 `SetGOMAXPROCS`、`detectControlPath`、`getCgroupDetails` 和 `detectCPUQuotaInV1` 对应定义。调用图命令未返回可用的精确边，因此上述直接调用边以相邻 Rust 源中的调用点补证，而没有把模糊的全仓搜索结果当成结论。

## 错误处理与边界

- proc 或控制文件打开、读取、数值解析失败均通过 `?` 传播，并用路径、版本、控制器或字段名补充上下文；`getCgroupDetails` 在没有任何匹配挂载时主动报错。
- `detectControlPath` 跳过少于三段的异常行；没有匹配不是错误，而是返回 unified 路径或空字符串。是否把空字符串解释成“无控制器”由 CPU/内存上层决定。
- `detectCgroupVersion` 对短行、缺少 `-`、或分隔符后字段不足返回 `(0, false)`；v1 必须在 super options 中包含目标控制器，v2 不做控制器级过滤。
- v1 namespace root 含字面 `..` 时整行忽略；`strip_prefix` 失败也不报错，而是继续扫描其他行。这避免用不可安全映射的路径构造 cgroup 根。
- `detectCPUQuotaInV2` 拒绝空字段或超过两个字段；配额/周期非法数值报错。`detectCPUUsageInV2` 忽略无关或格式不符的行，但目标字段一旦存在且数值非法就报错；两个目标字段均缺失时返回 `(0, 0)`。
- `readInt64Value` 只取首行；空文件返回 `0` 是为对齐当前 Go 实现中 scanner 未产生 token 时保留命名 `uint64` 零值的实际行为。`max` 只映射到有符号 64 位最大值，而不是 `u64::MAX`。
- `join_root` 有意丢弃 `..`，因此它是测试根映射和路径收敛工具，不是保留任意输入路径语义的通用 join。

## 并发与资源生命周期

所有文件句柄都由 Rust RAII 在函数返回或错误传播时关闭；逐行迭代不产生后台任务、通道、锁或异步生命周期。返回的字符串、向量和 `CPUUsage` 都拥有数据，不借用打开的文件。

并发相关的唯一机制是 `MAX_PROCS` 原子值。`SeqCst` 保证多个线程能看到全序的交换/恢复操作，但撤销闭包并不实现栈式或引用计数管理：若多个调用者交错执行 `SetGOMAXPROCS` 和 undo，最终值取决于闭包的调用次序。因此安全扩展时应保持“一次设置对应一次及时恢复”的作用域纪律，不能把它误认为真正的 Rust runtime 全局并行度控制器。

## 与 Go 版本的对应关系

同路径 `pkg/util/cgroup/cgroup.go` 是主要语义基准。Rust 保留了 Go 的公开枚举/结构字段、控制器集合匹配、cgroup 路径冒号恢复、mountinfo v1/v2 识别、namespace 相对路径、CPU 文件格式、`max` 哨兵以及错误上下文的大体契约。`pkg/util/cgroup/cgroup_mock_test.go` 与 Rust 的 `cgroup_mock_test.rs` 都以表驱动覆盖缺文件、缺控制器、namespace 映射、控制器顺序反转、v1/v2 和混合挂载。

关键差异是 `SetGOMAXPROCS`：Go 调用 `runtime.GOMAXPROCS`，记录日志，并在新值与旧值相同时返回 no-op；Rust 因无等价进程级调度器旋钮，只维护不可公开读取的 `MAX_PROCS`，也不记录日志，且有效配额下无论值是否相同都会交换并返回恢复闭包。因此它保留 API 形状和撤销动作，但不能宣称改变了 Rust 应用实际并行度。

另一处需明确的当前事实是 v2 usage 的单位：Go 结构注释笼统称 `Stime/Utime` 为纳秒，但 `cpu.stat` 提供 `*_usec`，Rust 与 Go 都直接保存原始数值而未换算。Rust `detectCPUUsageInV2` 的解析错误消息还写有 “cgroup v1”，这是与 Go 现有消息一致的兼容文本，不表示实际读取 v1。

## 扩展指南

- 新增 cgroup 控制文件：先在本文件增加文件名/键常量和最小解析原语，再由 `cgroup_cpu.rs` 或 `cgroup_memory.rs` 组合；同步独立测试文件，不要把测试嵌入 `cgroup.rs`。
- 修改 `/proc` 或 mountinfo 规则：重点审查 `controllerMatch`、`detectControlPath`、`detectCgroupVersion`、`getCgroupDetails` 四处，并扩展 `cgroup_mock_test.rs` 与 Go 的 `cgroup_mock_test.go` 对应案例，特别保持控制器顺序、冒号路径、namespace root 与混合挂载行为。
- 修改数值格式：分别覆盖 v1 的带符号 quota、v2 `max`、缺失 period、空文件、非法整数和未知 `cpu.stat` 键；相关聚焦测试位于 `cgroup_test.rs`、`cgroup_cpu_linux_1_aster_unit_test.rs` 和 `cgroup_mock_test.rs`。
- 若要真正控制 Rust 并行度，不能只扩展 `MAX_PROCS`；需要先确定实际 runtime/线程池的拥有者，并显式接线。该变化会超出此基础解析文件的当前职责，且需评估全局兼容性、嵌套恢复和并发竞态。
- 性能上这些 API 每次都会同步打开并扫描 proc/cgroup 文件；若新增高频调用或缓存，必须同时定义 cgroup 动态更新后的失效策略，不能默认路径、配额或挂载在进程生命周期内恒定。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/util/cgroup/cgroup.rs`（46 个符号）；`files --filter pkg/util/cgroup` 确认同目录 Rust/Go 文件集合；`node --file pkg/util/cgroup/cgroup.rs --offset 1/240` 阅读全部 373 行；`query` 精确核对 `SetGOMAXPROCS`、`detectControlPath`、`getCgroupDetails`、`detectCPUQuotaInV1` 的 Rust/Go 定义。`callers/callees` 查询未给出可用输出，直接边改由下列相邻源码调用点验证。
- Rust 源与装配：`pkg/util/cgroup/cgroup.rs`、`lib.rs`、`cgroup_cpu.rs`、`cgroup_cpu_linux.rs`、`cgroup_memory.rs`、`cgroup_memory_unsupport.rs`。
- crate 声明：`pkg/util/cgroup/Cargo.toml`。
- Go 对照：`pkg/util/cgroup/cgroup.go`、`cgroup_mock_test.go`、`cgroup_cpu_test.go`。
- 独立 Rust 测试：`pkg/util/cgroup/cgroup_test.rs` 验证 v2 三段字段与空单值文件；`cgroup_cpu_linux_1_aster_unit_test.rs` 验证控制器匹配、v1/v2 CPU 文件、端到端 v2、`max` 与错误上下文；`cgroup_mock_test.rs` 验证 CPU/内存路径、失败和混合挂载矩阵。
- 本任务是纯文档分析，按计划未运行 Cargo；交付前另执行固定 11 章节结构检查和文档 diff 自审。
