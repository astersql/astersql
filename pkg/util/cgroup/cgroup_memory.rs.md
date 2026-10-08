# [`pkg/util/cgroup/cgroup_memory.rs`](./cgroup_memory.rs)

## 文件定位

本文件是 `astersql-util-cgroup` crate 在 Linux 上的 cgroup 内存探测实现。crate 入口 `pkg/util/cgroup/lib.rs` 通过 `#[cfg(target_os = "linux")] mod cgroup_memory` 选择本文件，并以 `pub use cgroup_memory::*` 暴露其接口；非 Linux 平台改用 `cgroup_memory_unsupport.rs`。`pkg/util/cgroup/Cargo.toml` 指定 crate 根为 `lib.rs`，运行时仅直接依赖 `anyhow`，并以 `package.metadata.porting.go-package = "pkg/util/cgroup"` 标明 Go 对照包。

生产调用链中，`pkg/util/memory/meminfo.rs` 导入 `GetMemoryLimit`、`GetMemoryUsage`：`MemTotalCGroup` 用前者与物理内存总量取较小值，`MemUsedCGroup` 用后者与物理已用量取较小值，`InitMemoryHook` 也通过 `GetMemoryLimit` 判断是否启用 cgroup 感知探针。因此本文件负责把 Linux `/proc` 和 cgroup 文件系统数据转换成上层内存管理可消费的字节数。

## 核心职责

- 提供四个以主机根目录 `/` 为入口的公开查询：内存上限、带版本的内存上限、当前用量、inactive file 用量（`GetMemoryLimit`、`GetCgroupMemLimit`、`GetMemoryUsage`、`GetMemoryInactiveFileUsage`）。
- 通过 `memory_location` 联合 `detectControlPath` 与 `getCgroupDetails`，从 `/proc/self/cgroup` 和 `/proc/self/mountinfo` 定位当前进程的 memory controller、挂载点及 cgroup v1/v2 版本。
- 在仅有 v1、仅有 v2、v1/v2 同时存在三类环境中选择对应文件，并在混合环境里先读 v1、失败后回退 v2。
- 解析单值文件（经 `cgroup.rs::readInt64Value`）及 `memory.stat` 键值文件（`detectMemStatValue`），同时保留 Go `bufio.Scanner`/`strconv.ParseUint` 的关键边界语义。

## 主要符号

- `GetMemoryLimit() -> Result<u64>`：以 `/` 调用 `getCgroupMemLimit`，只返回限制值。
- `GetCgroupMemLimit() -> Result<(u64, Version)>`：返回限制值和 `Version::{V1,V2,Unknown}`。
- `GetMemoryUsage() -> Result<u64>`：返回当前 cgroup 内存用量。
- `GetMemoryInactiveFileUsage() -> Result<u64>`：返回 `memory.stat` 中 inactive file 页的字节数，供上层判断可回收缓存。
- `memory_location(root) -> Result<Option<(String, Vec<String>, Vec<i32>)>>`：内部公共前置步骤；`String` 是控制路径，两个向量按 `getCgroupDetails` 的约定对应挂载点和版本，双挂载时顺序为 v1、v2。
- `getCgroupMemLimit`、`getCgroupMemUsage`、`getCgroupMemInactiveFileUsage`：可注入根目录的实现入口，生产传 `/`，测试可传临时文件树。
- `detectMemLimitInV1` / `detectMemLimitInV2`：分别读取 v1 `memory.stat` 的 `hierarchical_memory_limit` 和 v2 `memory.max`。
- `detectMemUsageInV1` / `detectMemUsageInV2`：分别读取 `memory.usage_in_bytes` 和 `memory.current`。
- `detectMemInactiveFileUsageInV1` / `detectMemInactiveFileUsageInV2`：分别查找 `total_inactive_file` 和 `inactive_file`。
- `detectMemStatValue(root, filename, key, version)`：逐行扫描统计文件，只接受“目标键 + 十进制无符号整数”两字段记录。

本文件没有自定义常量、类型、trait、`impl` 或文件内条件编译项；文件选择发生在 `lib.rs`，文件名和键名常量以及 `Version` 定义来自相邻的 `cgroup.rs`。

## 执行流程

1. 四个公开入口把固定根目录 `/` 传给相应 `getCgroup*` 函数。
2. `memory_location` 先对 `root` 和 `/proc/self/cgroup` 做安全拼接，调用 `detectControlPath(..., "memory")`。若没有 memory controller，打印警告并返回 `None`；调用方把它解释为数值 `0`，限制查询同时返回 `Unknown`。
3. 找到控制路径后，`getCgroupDetails` 扫描 `mountinfo`。仅 v1 时返回一个 v1 挂载点，仅 v2 时返回一个 v2 挂载点，二者并存时返回按 v1、v2 排列的两个挂载点及 `[1, 2]`。
4. 单版本路径把 v2 的控制路径追加到挂载点，v1 则直接使用 `getCgroupDetails` 已结合 namespace root 算出的路径；随后按版本调用相应读取器。未知版本立即报错。
5. 混合版本中，限制和 inactive file 查询先读 v1 挂载点，失败才读 `mounts[1]/path` 的 v2 文件。用量查询同样先读 v1，但当前 Rust 源码的 v2 回退使用 `mounts[0]/path`；这与 Go 的 `mount[1]/path` 不同，是当前实现事实而非本文推断出的设计意图。
6. 单值读取器 `readInt64Value` 读取第一行、去空白并解析；字面量 `max` 映射为 `i64::MAX as u64`。统计读取器 `detectMemStatValue` 最多按 64 KiB token 逐行读取，找到目标键后校验每个值字节均为 ASCII 数字，再解析为 `u64`。

## 数据与状态

本文件自身无全局可变状态、缓存或持久对象。一次查询中的状态仅存在于栈上：控制路径 `path`、平行的 `mounts`/`versions` 向量、组合后的 `PathBuf`，以及统计文件扫描所复用的 `Vec<u8>` 缓冲区。

关键数据契约由 `cgroup.rs` 提供：`Version` 的数值语义为 `Unknown=0`、`V1=1`、`V2=2`；`getCgroupDetails` 在双挂载时保证 v1 在索引 0、v2 在索引 1；内存文件名和统计键分别是 `memory.stat`、`memory.max`、`memory.usage_in_bytes`、`memory.current`、`hierarchical_memory_limit`、`total_inactive_file`、`inactive_file`。所有成功数值均以字节为单位返回 `u64`。

“未探测到 controller”使用 `(0, Unknown)` 或 `0` 表达，而不是错误；因此上游必须结合调用场景理解零值。与此不同，`memory.max` 中的 `max` 被 `readInt64Value` 映射为 `9_223_372_036_854_775_807`，不是零。

## 依赖与调用关系

上游直接调用证据：RustCodeGraph 将本文件标为被 `pkg/util/cgroup/cgroup_memory_test.rs` 和 `pkg/util/memory/meminfo.rs` 使用；后者在 `MemTotalCGroup`、`MemUsedCGroup`、`InitMemoryHook` 中分别调用 `GetMemoryLimit` 或 `GetMemoryUsage`。crate 外调用者通过 `lib.rs` 的公开再导出访问接口，而不是直接引用私有模块。

下游依赖均来自父模块 `use super::*`：`join_root` 负责把绝对形式的子路径按普通分量接入可注入根；`detectControlPath` 解析进程 cgroup；`getCgroupDetails` 解析挂载与版本；`readInt64Value` 读取单值文件；`Version` 与各文件名/键名常量定义协议。标准库提供文件、路径和缓冲读取，`anyhow::{Result, Context, anyhow}` 提供统一错误及上下文。

Cargo 边界很小：运行时只依赖 `anyhow = "1"`；`tempfile` 仅是 dev-dependency，并由独立 Rust 测试创建隔离目录。此实现直接读取本地 Linux 伪文件系统，不进行网络、SQL、RPC 或存储引擎调用。

## 错误处理与边界

- `/proc/self/cgroup` 或 `mountinfo` 打不开/无法解析时，底层错误通过 `?` 传播并保留上下文；找不到 memory controller 是可接受的零值结果，找不到匹配挂载则是错误。
- 当 `versions[0]` 不是 1 或 2 时，三个主查询返回 `detected unknown cgroup version index`。正常情况下 `getCgroupDetails` 只生成 1/2，但该分支保护向量契约。
- 混合环境的回退只由 v1 读取失败触发；成功读到 v1 即不访问 v2。回退成功时限制查询返回 `V2`，失败则传播 v2 错误；v1 的原始错误不会保留。
- `detectMemStatValue` 忽略非两字段行和非目标键行。目标值含 `+`、负号或非数字时返回解析类错误；超过 `u64::MAX` 时由 `parse` 返回带上下文错误。找不到目标键、底层读取中止、非 UTF-8 内容未形成目标键，最终都归并为 missing-key 错误。
- 为对齐 Go `bufio.Scanner`，单个未换行 token 达到 64 KiB 时停止扫描并返回 missing-key；`pkg/util/cgroup/cgroup_memory_test.rs::stat_byte_and_scanner_parity` 覆盖非 UTF-8 前缀、前导加号、`u64::MAX`、溢出、超长 token 与 CRLF。
- v2 `memory.max` 的层级最小限制/安全折扣并未在本文件实现。Go 文件中的 TODO 明确提醒，当前只读取当前 cgroup 的 `memory.max`，Kubernetes 层级限制和 OOM 余量可能需要额外策略。

## 并发与资源生命周期

所有入口都是同步、只读、无共享状态的函数；多个线程并发调用不会共享本文件内的缓冲区或文件句柄。每次调用都会重新读取 `/proc`、`mountinfo` 和目标 cgroup 文件，因此返回的是各次读取时刻的快照，跨文件读取并非原子操作，期间 cgroup 层级或数值可能变化。

`fs::File`、`BufReader`、`Vec<u8>` 和临时 `PathBuf` 由 Rust 所有权在函数返回或错误传播时自动释放，无显式关闭、后台任务、锁、channel 或事务。性能成本主要是每次查询的文件打开与线性扫描；缓存位于上游 `pkg/util/memory/meminfo.rs`（总量 60 秒、用量 500 毫秒），不属于本文件职责。

## 与 Go 版本的对应关系

主要语义逐项复刻 `pkg/util/cgroup/cgroup_memory.go`：相同的四个公开入口、可替换 `root` 的内部入口、controller/mount 探测顺序、v1 优先和 v2 回退、相同的文件名与统计键、零值表示无 controller，以及相同风格的错误文本。Rust 的 `Result` 取代 Go 的多返回值 `error`，`Option` 表示无 controller，RAII 取代 Go 的 `defer stat.Close()`。

`detectMemStatValue` 有意模拟 Go Scanner 的 64 KiB token 上限，并像 Go 实现一样不单独返回 Scanner 终止错误；Rust 使用 `from_utf8_lossy` 继续扫描包含无效 UTF-8 的其他行，并用 ASCII 数字预检确保 `+42` 不会被 Rust 的整数解析器意外接受。独立 Rust 测试专门固定这些兼容细节。

已验证的差异有两项。第一，混合 v1/v2 时，Go 的 `getCgroupMemUsage` 回退路径是 `mount[1]/path`，Rust 当前是 `mounts[0]/path`；限制和 inactive file 两条 Rust 路径均使用 `mounts[1]`。第二，Go 源码保留关于 v2 `memory.max`/`memory.current` 部署覆盖范围的 TODO，Rust 源码未复制该注释，但实际仍只是读取当前路径，没有新增层级遍历或折扣逻辑。扩展或修复时应以 Go 行为与部署约束为基准，不应把当前 Rust 差异默认为兼容要求。

Go 回归面主要位于 `pkg/util/cgroup/cgroup_mock_test.go` 的 `TestCgroupsGetMemoryUsage`、`TestCgroupsGetMemoryInactiveFileUsage`、`TestCgroupsGetMemoryLimit`，覆盖缺文件、无 controller、无挂载、v1、namespace v1、特殊 controller 字段、v2 缺失/非法/正常值及 `max`。Rust 独立测试 `pkg/util/cgroup/cgroup_memory_test.rs` 当前聚焦统计扫描器边界，不等同于 Go 三组端到端矩阵的完整迁移。

## 扩展指南

- 新增公开内存指标时，在本文件增加以 `/` 为根的公开入口和可注入 `&Path` 的内部入口，并在 `lib.rs` 的非 Linux 实现中提供同签名行为；不要把测试写进生产源文件，应同步扩展同目录独立测试文件。
- 新增 v1/v2 文件或 `memory.stat` 键时，优先把协议常量放入 `cgroup.rs`，复用 `readInt64Value` 或 `detectMemStatValue`，并明确无 controller、单版本、双版本及未知版本的行为。
- 修改双版本逻辑前，应先为 `mounts = [v1, v2]` 建立 Rust 回归测试，尤其覆盖 `getCgroupMemUsage` 的 v2 回退挂载索引；同时对照 Go 的 `mount[1]` 语义，避免继续传播错误路径。
- 修改统计解析器必须同步 `cgroup_memory_test.rs` 的 byte/scanner 边界，并对照 Go `cgroup_memory.go` 的 `bufio.Scanner` 与 `strconv.ParseUint`。若有意改变 64 KiB、非 UTF-8、前导加号或读错误行为，应明确记录兼容性影响。
- 若实现 v2 层级最小限制或安全折扣，需要单独设计遍历边界、无限制值和竞态读取策略，并同步评估 `meminfo.rs` 的 60 秒缓存、Go TODO 所指的 Kubernetes/OOM 兼容性及性能成本。
- 若扩大端到端覆盖，应将 Go `cgroup_mock_test.go` 的内存矩阵移植到独立 Rust 测试文件，保持源文件与测试逻辑分离。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/cgroup` 确认目标源、入口、Linux/非 Linux 实现及测试均被索引；`node --file` 读取了 `cgroup_memory.rs` 全部 193 行、`cgroup_memory_test.rs`、`lib.rs`、`cgroup.rs` 相关路径/版本/单值读取实现，以及生产上游 `pkg/util/memory/meminfo.rs`。文件关系结果指出本文件由 Rust 测试和 `meminfo.rs` 使用。
- crate 与模块证据：`pkg/util/cgroup/Cargo.toml`；`pkg/util/cgroup/lib.rs`；`pkg/util/cgroup/cgroup.rs`。
- Go 对照证据：`pkg/util/cgroup/cgroup_memory.go`；非 Linux 对照 `pkg/util/cgroup/cgroup_memory_unsupport.go`；端到端用例 `pkg/util/cgroup/cgroup_mock_test.go`。
- Rust 测试证据：`pkg/util/cgroup/cgroup_memory_test.rs`，包含 `stat_byte_and_scanner_parity` 与 `stat_read_error_matches_go_missing_key_error`。
- 生产调用证据：`pkg/util/memory/meminfo.rs` 的 `MemTotalCGroup`、`MemUsedCGroup`、`InitMemoryHook`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核以上符号、路径、差异与扩展入口均可回溯到源码或测试。
