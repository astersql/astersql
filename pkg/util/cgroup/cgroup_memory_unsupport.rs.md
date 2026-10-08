# `pkg/util/cgroup/cgroup_memory_unsupport.rs`

## 文件定位

本文件是 `astersql-util-cgroup` crate 的非 Linux 内存 cgroup 实现。`pkg/util/cgroup/lib.rs` 仅在 `cfg(not(target_os = "linux"))` 下声明并公开重导出本模块；Linux 构建改用同目录的 `cgroup_memory.rs`。crate 边界由 `pkg/util/cgroup/Cargo.toml` 定义，运行时依赖只有 `anyhow`。

它同时承担两种角色：面向 crate 使用者的四个 `Get*` API 是明确的“不支持平台”桩；带 `root: &Path` 参数的 `getCgroup*` 和 `detect*` 函数则保留真实的 v1/v2 文件解析能力，供临时文件树测试和跨平台语义复用。因此不能把整个文件概括为无行为的空实现。

## 核心职责

- `GetMemoryLimit`、`GetCgroupMemLimit`、`GetMemoryUsage`、`GetMemoryInactiveFileUsage` 在非 Linux 平台稳定返回零值，其中版本为 `Unknown`，且不访问文件系统。
- `location` 通过 `cgroup.rs` 的 `detectControlPath`、`getCgroupDetails` 和 `join_root`，从注入根目录下的 `/proc/self/cgroup` 与 `/proc/self/mountinfo` 解析 memory controller 的控制路径、挂载点和版本。
- `getCgroupMemUsage`、`getCgroupMemInactiveFileUsage`、`getCgroupMemLimit` 根据 v1/v2 或混合挂载选择对应探针；没有 memory controller 时返回零值，解析或读取失败时传播错误。
- `detectMemStatValue` 实现 `memory.stat` 键值扫描，并有意模拟 Go `bufio.Scanner` 的 64 KiB token 上限和终止错误被忽略后的“键不存在”结果。

## 主要符号

- `pub fn GetMemoryLimit() -> Result<u64>`：非 Linux 公共限额入口，恒为 `Ok(0)`。
- `pub fn GetCgroupMemLimit() -> Result<(u64, Version)>`：非 Linux 公共限额及版本入口，恒为 `Ok((0, Unknown))`。
- `pub fn GetMemoryUsage() -> Result<u64>` 与 `pub fn GetMemoryInactiveFileUsage() -> Result<u64>`：非 Linux 公共用量入口，均恒为 `Ok(0)`。
- `fn location(root: &Path) -> Result<Option<(String, Vec<String>, Vec<i32>)>>`：内部定位器；`None` 表示未检测到 memory controller，`Some` 中依次保存控制路径、挂载点和版本号。
- `getCgroupMemUsage`、`getCgroupMemInactiveFileUsage`、`getCgroupMemLimit`：可注入根目录的编排函数。限额函数将数值与 `Version::{V1,V2}` 配对。
- `detectMemUsageInV1/V2` 与 `detectMemLimitInV2`：委托 `cgroup.rs::readInt64Value` 读取单值文件。
- `detectMemLimitInV1`、`detectMemInactiveFileInV1/V2`：用固定文件名、键名和版本参数调用 `detectMemStatValue`。
- `detectMemStatValue(root, filename, key, version)`：打开文件、逐 token 扫描、校验十进制无符号整数并返回第一个匹配值。

本文件没有自定义类型、trait、impl、模块级可变状态或常量；文件名、键名和协议版本常量来自父模块 `cgroup.rs`。

## 执行流程

公共非 Linux 路径最短：`lib.rs` 通过条件编译选择本模块，调用任一 `Get*` 后直接取得零值，不会进入探测函数。上层例如 `pkg/util/memory/meminfo.rs` 的 `MemTotalCGroup`、`MemUsedCGroup` 会消费这些返回值；`pkg/util/cgmon/cgmon.rs` 也把 `GetMemoryLimit` 注册成监控探针，但该监控只在 Linux 启停。

可注入 root 的解析路径如下：

1. `getCgroup*` 调用 `location`，先解析 memory controller 控制路径，再解析匹配的挂载点和版本。
2. 若没有控制路径，返回用量 `0`，或限额 `(0, Unknown)`。
3. 若 `versions.len() == 2`，先读 v1；v1 失败后用 `.or_else` 回退到 v2。该分支的 v2 路径由第一个挂载点、控制路径与 root 拼接而成，这是当前 Rust 非 Linux实现的实际行为。
4. 单版本时，在 v2 路径后附加控制路径，v1 只使用挂载根，然后按版本 `1`/`2` 调用探针；其他版本立即报错。
5. 单值探针通过 `readInt64Value` 读取；统计探针由 `detectMemStatValue` 扫描恰好两个字段且首字段等于目标键的行。
6. 匹配值必须只含 ASCII 数字并能解析为 `u64`；否则返回带键名、版本和文件名的错误。扫描结束仍未匹配时返回 missing-key 错误。

## 数据与状态

所有状态均为函数局部值，没有缓存或跨调用共享状态。`path` 是进程在 cgroup 层次中的控制路径，`mounts` 与 `versions` 是 `getCgroupDetails` 返回的对应列表；代码假设非空结果至少有 `mounts[0]` 和 `versions[0]`，完整性由父模块的探测函数提供。

数据源对应为：v1 用量 `memory.usage_in_bytes`，v2 用量 `memory.current`，v1 限额是 `memory.stat` 的 `hierarchical_memory_limit`，v2 限额是 `memory.max`，v1/v2 inactive file 键分别为 `total_inactive_file` 与 `inactive_file`。版本通过 `Version::{Unknown,V1,V2}` 表达。

`detectMemStatValue` 每轮复用一个 `Vec<u8>`，单次读取最多 65536 字节。它使用损失性 UTF-8 转换后按 Unicode 空白分字段，但对数值字段再执行 ASCII 数字校验，避免 `+42`、负数或非 ASCII 数字被接受。

## 依赖与调用关系

上游装配是 `pkg/util/cgroup/lib.rs`：非 Linux 时声明 `cgroup_memory_unsupport` 并 `pub use` 其符号。仓库文本调用证据显示，公开入口的主要消费者包括 `pkg/util/memory/meminfo.rs`（读取限额和用量、选择 cgroup 内存 hook）以及 `pkg/util/cgmon/cgmon.rs`（把 `GetMemoryLimit` 作为监控探针）。

下游依赖分三层：父模块 `cgroup.rs` 提供路径常量、统计键、`Version`、`join_root`、`detectControlPath`、`getCgroupDetails` 和 `readInt64Value`；`anyhow` 提供 `Result`、`anyhow!` 与 `Context`；标准库提供 `File`、`BufReader`、受限读取和 `Path`。

精确的内部边包括：三个 `getCgroup* -> location`；`location -> detectControlPath/getCgroupDetails/join_root`；用量探针与 v2 限额探针 `-> readInt64Value`；v1 限额及两个 inactive-file 探针 `-> detectMemStatValue`。`pkg/util/cgroup/cgroup_mock_test.rs` 直接调用三个 `getCgroup*`，`cgroup_memory_unsupport_test.rs` 通过私有测试模块直接调用 `detectMemInactiveFileInV2`。

## 错误处理与边界

公共 `Get*` 桩从不返回错误。注入 root 的路径则保留上下文：无法打开 proc/mount 文件的错误来自父模块；未知版本错误为 `detected unknown cgroup version index: ...`；统计文件打开错误包含文件名和版本；非法或溢出的目标值包含键、版本和文件名。

没有 controller 与读取失败是不同语义：前者成功返回零值，后者返回错误。混合版本分支只在 v1 探针失败时回退 v2，因此 v1 的任何错误都会触发回退，且最终只保留 v2 的结果或错误。

统计扫描会忽略字段数不是 2 或键不匹配的行。达到 EOF、底层 `read_until` 出错，或遇到恰好占满 65536 字节且没有换行的 token，都会结束扫描并统一表现为 missing-key；这是为了对齐 Go Scanner 的终止行为。当前专门测试覆盖超长 token，其他打开/解析/版本/路径边界由 mock 表驱动测试覆盖。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或全局状态；各函数只进行同步文件 I/O，因此不同调用间没有本文件引入的共享数据竞争。性能与阻塞边界主要是 `/proc`、mountinfo 和 cgroup 文件的同步读取。

`fs::File`、`BufReader` 与临时字节缓冲区由 RAII 在函数返回时释放，无需显式关闭。`detectMemStatValue` 的每行读取被限制为 64 KiB，避免对无换行输入无限扩张；代价是超长行之后的有效键不会继续扫描，这正是保留的 Go 兼容行为。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/cgroup/cgroup_memory_unsupport.go`。四个公共 API 的非 Linux 语义一致：零限额、零用量、零 inactive-file 用量以及 `Unknown` 版本。Go 文件同样保留可注入 root 的真实 v1/v2 探测辅助和 `detectMemStatValue`，Rust 用 `Result<T>` 替代 Go 的多返回值错误。

Rust 的统计解析显式重现 Go `bufio.Scanner` 默认 64 KiB token 上限，并像 Go 实现一样不读取 `scanner.Err()`，所以超长 token 归并为 missing-key。Rust 对数值先做 ASCII 数字检查，再解析 `u64`，对应 Go `strconv.ParseUint(..., 10, 64)` 的关键接受范围。

需要注意两处当前实现差异。第一，Go 非 Linux 混合版本分支依据 `len(ver) == 2`，Rust相同，但 Rust v2 回退路径使用 `mounts[0]`；Linux Rust 实现和 Linux Go 实现对限额混合挂载会使用第二个挂载点。第二，Go 非 Linux 未知版本格式化整个 `ver` 切片，Rust只报告 `versions[0]`。本文只记录这些事实，不推断或修正其意图。

相关 Go 行为测试在 `pkg/util/cgroup/cgroup_mock_test.go`，Rust 对照测试在独立文件 `cgroup_mock_test.rs` 与 `cgroup_memory_unsupport_test.rs`，符合测试不内嵌生产源文件的仓库规则。

## 扩展指南

若新增公共内存指标，应同时决定 Linux 与非 Linux 的公开语义，在 `lib.rs` 两个条件编译模块中保持同名 API，并同步 `cgroup_memory.rs`、`cgroup_memory_unsupport.rs` 及 Go 对照文件。新增 v1/v2 文件或 `memory.stat` 键时，优先复用 `readInt64Value` 或 `detectMemStatValue`，并把协议文件名/键名常量放在共享的 `cgroup.rs`，避免两平台漂移。

修改路径选择或混合 v1/v2 回退时，应扩展 `cgroup_mock_test.rs` 的表驱动用例，并与 `cgroup_mock_test.go` 的挂载、namespace、缺文件、解析失败和版本断言逐项核对。修改扫描器时必须同步独立的 `cgroup_memory_unsupport_test.rs`，至少保留 64 KiB 无换行 token、非法数字、`u64` 边界和缺键行为；不要把测试写回生产文件。

兼容风险集中在错误文本、零值含义、混合挂载索引和 `memory.max` 的特殊值处理。性能风险较低，但不应去掉行长度上限或引入跨调用全局缓冲；若有意改变 Go Scanner 兼容行为，应同时更新 Go/Rust 对照证据和调用方对错误的预期。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本目录 22 个 Go/Rust 文件；`files --filter pkg/util/cgroup` 确认目标、平台实现与测试均已索引；`node --file pkg/util/cgroup/cgroup_memory_unsupport.rs --offset 1 --limit 220` 核对了完整 185 行源码及文件引用。对 `callers/callees --file` 的精确查询在本环境长时间无输出后被中止，因此调用边又以限定目录的 `rg` 结果和逐文件源码复核，未采用自然语言 `explore` 返回的同名噪声。
- 生产与装配：`pkg/util/cgroup/cgroup_memory_unsupport.rs`、`pkg/util/cgroup/lib.rs`、`pkg/util/cgroup/cgroup.rs`、`pkg/util/cgroup/Cargo.toml`。
- 直接调用者：`pkg/util/memory/meminfo.rs`、`pkg/util/cgmon/cgmon.rs`；限定 Rust 源码搜索还确认三个可注入 root 的函数只由同目录 mock 测试直接调用。
- Go 对照：`pkg/util/cgroup/cgroup_memory_unsupport.go`、Linux 语义参照 `pkg/util/cgroup/cgroup_memory.go`、表驱动测试 `pkg/util/cgroup/cgroup_mock_test.go`。
- Rust 测试证据：`pkg/util/cgroup/cgroup_mock_test.rs` 覆盖 v1/v2、namespace、无 controller、缺文件、非法值、版本与 `max`；`pkg/util/cgroup/cgroup_memory_unsupport_test.rs::unsupported_stat_scanner_stops_after_go_max_token_size` 覆盖 64 KiB token 边界；`pkg/util/cgroup/cgroup_memory_test.rs` 提供 Linux 实现的字节与扫描语义参照。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务文件指定的 11 章节结构命令，并人工核对本文能回答文件存在原因、执行路径、安全扩展位置和测试同步点。
