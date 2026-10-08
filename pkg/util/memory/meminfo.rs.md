# `pkg/util/memory/meminfo.rs`

## 文件定位

本文件是 `astersql-util-memory` crate 的主机/cgroup 内存探针实现，由 [`pkg/util/memory/lib.rs`](lib.rs) 以 `pub mod meminfo` 暴露。它把“机器或容器可用的内存上限”“当前系统已用内存”和“当前 AsterSQL 进程堆占用”统一为字节数查询接口，供服务启动、全局内存仲裁、优化器、内存告警和缓存淘汰逻辑使用。

crate 边界由 [`pkg/util/memory/Cargo.toml`](Cargo.toml) 定义：本文件直接使用 `astersql-util-cgroup`（别名 `cgroup_crate`）读取 cgroup 状态，使用 `sysinfo` 读取宿主机内存，并通过同 crate 的 [`memstats.rs`](memstats.rs) 取得进程堆统计。文件没有条件编译项，也不受 `mem-arbitrator` feature 控制。

## 核心职责

1. 以 `MemTotal`、`MemUsed` 两个可热替换的函数指针提供当前生效的系统内存探针；默认指向普通宿主机实现。
2. 为总量、已用量和进程堆用量分别维护惰性全局缓存，降低系统信息与 cgroup 文件的读取频率。
3. 提供普通环境与 cgroup 环境的探针实现；cgroup 版本取 cgroup 值和物理机值的较小者，避免向上层报告超过任一约束的容量或用量。
4. 在 `InitMemoryHook` 中根据容器检测结果或“非零 cgroup 限制小于物理内存”条件选择探针，并在非容器路径上预热总量、用量查询。
5. 提供 `GetMemTotalIgnoreErr` 的降级语义以及 `InstanceMemUsed` 的进程堆查询，分别服务于允许零值兜底的决策路径和实例级内存守卫。

## 主要符号

- `MemoryInfoResult = Result<u64, Box<dyn Error + Send + Sync>>`：公共返回类型。错误对象可跨线程传递和共享；数值统一为字节。
- `MemoryInfoProbe = fn() -> MemoryInfoResult`：无捕获函数指针类型，只允许安装普通函数，不能直接安装携带状态的闭包。
- `MemTotal` / `MemUsed`：`pub static RwLock<MemoryInfoProbe>`。初值分别是 `MemTotalNormal` 和 `MemUsedNormal`；调用者需要先取得读锁再调用，初始化逻辑通过写锁切换实现。
- `CacheValue { value, updated }`：私有缓存条目。`updated: None` 表示尚无有效值；`Clone + Copy` 让读取时可在持锁区间内复制快照后立即释放锁。
- `total_cache` / `used_cache` / `process_cache`：用 `OnceLock` 惰性创建三个彼此独立的 `RwLock<CacheValue>`。总量 TTL 为 60 秒，另两类 TTL 为 500 毫秒。
- `cached` / `update`：统一执行 TTL 判断和整条缓存替换。判断使用单调时钟 `Instant`，不受系统墙钟回拨影响。
- `call_total` / `call_used`：复制当前函数指针后释放探针锁，再执行函数；因此系统调用期间不会长期持有 `MemTotal`/`MemUsed` 的读锁。
- `get_mem_total_ignore_err_with`：可注入探针的公共辅助函数，把任意 `Result<u64, E>` 的错误折叠成 `0`。`GetMemTotalIgnoreErr` 用它包装当前 `MemTotal`。
- `system_memory`：新建 `sysinfo::System`、刷新内存并返回 `(total_memory, used_memory)`；该依赖版本的刷新接口不返回错误。
- `MemTotalNormal` / `MemUsedNormal`：普通宿主机探针，分别使用 60 秒和 500 毫秒缓存。
- `MemTotalCGroup` / `MemUsedCGroup`：cgroup 感知探针。缓存失效后先读取 cgroup，再读取物理机值，最后缓存两者最小值。
- `MemoryHookKind::{Normal, CGroup}` 与 `select_memory_hook`：crate 内可见的可测试选择核心。容器分支直接选择 `CGroup`；非容器分支按顺序求 cgroup limit 和物理总量，仅当 limit 非零且更小时选择 `CGroup`。
- `InitMemoryHook`：公共初始化入口。选择 cgroup 时同时替换两个函数指针；容器分支随后直接返回 `Ok(0)`，非容器分支依次调用总量和已用量探针。
- `InstanceMemUsed`：公共进程堆探针，读取 `ReadMemStats().heap_alloc` 并缓存 500 毫秒。

## 执行流程

### 启动时选择探针

1. `InitMemoryHook` 调用 `InContainer()`。
2. `select_memory_hook` 在容器内直接返回 `CGroup`，不会执行传入的 cgroup-limit 或物理内存闭包；[`meminfo_test.rs`](meminfo_test.rs) 的 `container_selection_matches_go_and_skips_fallible_probes` 明确验证了这一惰性保证。
3. 非容器路径先调用 `GetMemoryLimit`，再调用 `mem_total_normal_uncached`。后者读取物理内存并更新总量缓存。任一步失败都会立即返回，不继续选择或预热。
4. limit 非零且小于物理总量时，依次把 `MemTotal`、`MemUsed` 改为 cgroup 版本；否则保留默认普通版本。
5. 容器路径完成函数指针切换后直接返回 `Ok(0)`。非容器路径先 `call_total()`，成功后再 `call_used()`，将最后一次用量查询的数值作为 `InitMemoryHook` 的成功返回值；调用者通常只关心成功或错误。

Rust 启动调用证据包括 [`cmd/tidb-server/main.rs`](../../../cmd/tidb-server/main.rs) 的 `memory::InitMemoryHook()`、[`br/cmd/br/cmd.rs`](../../../br/cmd/br/cmd.rs) 的初始化与后续总量/用量读取，以及 [`lightning/cmd/tidb-lightning/main.rs`](../../../lightning/cmd/tidb-lightning/main.rs) 的错误处理。

### 普通与 cgroup 查询

1. 探针先用 `cached` 检查对应缓存；命中则不触碰系统或 cgroup。
2. 普通探针调用 `system_memory`，取元组中的目标字段，再用 `update` 更新时间戳。
3. cgroup 探针先执行 `GetMemoryLimit` 或 `GetMemoryUsage`；错误通过 `?` 原样向上传播，且不会写缓存。
4. cgroup 成功后读取物理内存，取两者最小值并缓存。总量与已用量共享各自的缓存，不按“普通/cgroup 来源”再分层。

### 实例堆查询

`InstanceMemUsed` 先检查 500 毫秒的 `process_cache`。未命中时读取 [`memstats.rs`](memstats.rs) 中 `ReadMemStats()` 返回的 `heap_alloc`，写缓存后返回。上游 [`pkg/util/kvcache/simple_lru.rs`](../kvcache/simple_lru.rs) 用该值判断是否需要清空或淘汰 LRU；[`pkg/executor/internal/applycache/lib.rs`](../../executor/internal/applycache/lib.rs) 也将它再导出。

## 数据与状态

- 全局可变状态分成两类：两个“当前探针”锁和三个“最近值”缓存锁。它们都是进程级单例，生命周期覆盖整个进程。
- `MemTotal` 和 `MemUsed` 独立写入，不构成原子二元更新。当前唯一生产写入者 `InitMemoryHook` 连续更新两者；若初始化期间已有并发读者，理论上可短暂观察到一新一旧的组合，因此应在启动阶段、业务并发开始前调用。
- `total_cache` 同时服务 `MemTotalNormal` 和 `MemTotalCGroup`，`used_cache` 也同时服务两种已用量探针。切换函数指针不会清空缓存：例如非容器路径的 `mem_total_normal_uncached` 会先写入物理总量，若随后选择 cgroup，第一次 `MemTotalCGroup` 仍可能在 60 秒 TTL 内返回这份物理总量。Go 实现使用同一个 `memLimit`，具有相同的共享缓存特征；扩展时不能假定切换探针会立即切换缓存来源。
- 缓存没有“正在刷新”状态。多个线程同时发现过期时可以并行读取系统/cgroup 并先后覆盖缓存；结果最终一致于最后完成的探测，代价是过期边界可能发生重复 I/O。
- `0` 有两种含义：`GetMemTotalIgnoreErr` 的错误兜底值，以及不支持平台中 cgroup limit/usage 的合法返回值。调用者必须结合所用 API 判断，不能仅凭零值恢复错误原因。

## 依赖与调用关系

下游直接依赖如下：

- `cgroup_crate::{InContainer, GetMemoryLimit, GetMemoryUsage}`：选择运行环境及读取 cgroup 限额/用量。RustCodeGraph 显示 `GetMemoryLimit` 由 `MemTotalCGroup` 和 `InitMemoryHook` 调用，`GetMemoryUsage` 由 `MemUsedCGroup` 调用。
- `sysinfo::System`：`system_memory` 每次缓存未命中时新建实例并刷新物理总量/已用量。
- `crate::memstats::ReadMemStats`：`InstanceMemUsed` 的堆分配量来源；其自身还带有 memstats 层缓存，因此这里的 500 毫秒缓存是外层节流。
- 标准库 `OnceLock`、`RwLock`、`Instant`、`Duration`：负责惰性初始化、同步和 TTL。

主要 Rust 上游包括：

- `cmd/tidb-server/main.rs`：启动时初始化探针，并在设置全局变量时读取总量。
- `br/cmd/br/cmd.rs` 与 `lightning/cmd/tidb-lightning/main.rs`：工具进程启动时初始化；BR 还计算 `MemTotal - MemUsed` 风格的可用容量。
- `pkg/util/memory/global_arbitrator.rs`：`SetGlobalMemArbitratorWorkMode` 等路径调用 `GetMemTotalIgnoreErr` 计算限制。
- `pkg/planner/core/optimizer_runtime.rs`：`ShouldSkipReuseChunkForPhysicalPlan`、`ShouldSkipReuseChunkForPointGet` 以总内存决定复用策略。
- `pkg/util/memoryusagealarm/memoryusagealarm.rs`：直接读取 `MemTotal`/`MemUsed` 锁内的函数指针生成告警记录。
- `pkg/util/kvcache/simple_lru.rs`：调用 `InstanceMemUsed` 实施实例内存保护。

这些调用边由 RustCodeGraph 的 `node`/Trail 结果和仓库直接引用搜索共同核验；图索引还显示 `GetMemTotalIgnoreErr` 的直接调用者包括上述优化器和全局仲裁器路径。

## 错误处理与边界

- cgroup 读失败会从 `MemTotalCGroup`、`MemUsedCGroup` 或 `InitMemoryHook` 传播为 boxed error；实现不会用物理值掩盖失败，也不会把失败值写入缓存。
- 普通系统探针通过当前 `sysinfo` API 没有可传播的刷新错误，因此 `MemTotalNormal`、`MemUsedNormal` 的签名虽可返回错误，当前实现只会返回 `Ok`，锁中毒除外。
- 所有锁都通过 `expect(...)` 处理中毒。持写锁线程 panic 后，后续查询会 panic，而不是返回 `MemoryInfoResult::Err`；这是明确的进程级故障边界。
- `GetMemTotalIgnoreErr` 有意丢弃错误并返回 `0`，只适用于上层已有安全保守策略的路径。需要区分“探测失败”和“确实为零”的代码应直接调用锁中的探针。
- `select_memory_hook` 保证非容器路径的求值顺序：limit 失败时不读物理内存，物理内存失败时传播该错误；[`meminfo_test.rs`](meminfo_test.rs) 的 `non_container_selection_propagates_errors_and_compares_nonzero_limit` 覆盖 limit 错误、较小 limit、零 limit 和较大 limit，但当前没有覆盖物理闭包失败、缓存 TTL、锁中毒或真实系统/cgroup I/O。
- `min(cgroup, physical)` 防止 cgroup 返回异常偏大的值；在不支持 cgroup 的平台实现返回 `0` 时，显式调用 cgroup 探针也会得到 `0`。正常选择逻辑用 `cgroup_value != 0` 避免在非容器环境选中这种值。

## 并发与资源生命周期

- `OnceLock` 保证每个缓存锁只初始化一次；没有后台线程、异步任务、通道、事务或需要显式释放的句柄。
- 缓存读只在复制 `CacheValue` 时持读锁，系统探测在锁外完成；写入只在替换缓存条目时短暂持写锁。该结构避免慢 I/O 阻塞所有缓存读者，但允许并发重复刷新。
- `call_total`/`call_used` 先复制函数指针再释放探针读锁，因此探针执行过程中 `InitMemoryHook` 仍可获得写锁。已复制旧指针的调用会完成旧探针，之后的新调用才观察到切换。
- `Instant` 只在进程内使用，不被序列化；缓存随进程退出销毁。没有主动失效 API，值只会在 TTL 到期后的下一次访问时刷新。
- 初始化设计假定 `InitMemoryHook` 在服务启动阶段调用一次。重复调用通常可工作，但 `Normal` 分支不会主动把已经切到 cgroup 的函数指针恢复为普通版本；因此它不是通用的双向重配置接口。

## 与 Go 版本的对应关系

Rust 基本沿用 [`meminfo.go`](meminfo.go) 的公共名称、TTL、最小值规则、三个缓存和 `InitMemoryHook` 判定：总量缓存 60 秒，用量与实例堆缓存 500 毫秒；非容器时仅在非零 cgroup limit 更小时切换；实例内存读取 `HeapAlloc`/`heap_alloc`。

实现层面的对应与差异如下：

- Go 的 `MemTotal`/`MemUsed` 是可赋值函数变量；Rust 用 `RwLock<fn()>` 保留热替换能力并提供线程安全访问。
- Go 通过包级 `init()` 根据容器状态设置函数变量、初始化缓存并强制预读；Rust 用静态默认值和惰性 `OnceLock`，必须由进程入口显式调用 `InitMemoryHook` 才会切换到 cgroup。Rust 的容器分支会在返回前显式安装 cgroup 指针，以补偿没有 Go `init()` 的差异。
- Go `InitMemoryHook` 在切换时还调用 `sysutil.RegisterGetMemoryCapacity` 并记录选择日志；Rust 当前没有这两条接线，不能声称已经同步外部 sysutil 容量回调或日志行为。
- Go 的 `GetMemTotalIgnoreErr` 含 `GetMemTotalError` failpoint；Rust 只提供可注入的 `get_mem_total_ignore_err_with` 辅助函数，没有同名 failpoint。
- Go `gopsutil/mem.VirtualMemory()` 可返回错误；Rust `sysinfo::System::refresh_memory()` 当前不返回 `Result`，所以普通探针没有对应的 I/O 错误分支。
- Go 缓存结构为每项一个带 `*sync.RWMutex` 的 `memInfoCache`；Rust 用 `OnceLock<RwLock<CacheValue>>`，锁与数据共同由静态单例持有。

## 扩展指南

- 新增或更换系统内存来源时，优先在 `system_memory` 或新增独立探针中接入，保持所有值的字节单位，并确认 `sysinfo` 与 cgroup 的统计口径可比较。
- 新增探针种类需要同步扩展 `MemoryHookKind`、`select_memory_hook`、`InitMemoryHook` 的函数指针更新，以及缓存来源切换策略。若要求切换立即生效，应先设计显式缓存失效，避免沿用旧来源值。
- 若要允许带状态的运行时探针，`MemoryInfoProbe = fn()` 不够，需要改为 trait object 或其他所有权模型；这会影响静态锁、测试替换和所有直接读取 `MemTotal`/`MemUsed` 的调用方。
- 修改错误策略时要区分三类消费者：启动入口需要错误中止或记录，内存告警需要保留错误，全局仲裁与优化器使用 `GetMemTotalIgnoreErr` 的零值降级。不要全局吞掉 cgroup 错误。
- 修改 TTL 或并发刷新方式时，应评估系统调用成本、告警时效和惊群；若引入单飞刷新，不能在持缓存写锁期间执行慢 I/O。
- 测试必须继续放在独立的 [`meminfo_test.rs`](meminfo_test.rs)，不要嵌入生产文件。至少应同步覆盖选择顺序、错误传播、缓存命中/过期、探针切换及并发读取；涉及 Go 语义变更时同时核对 [`meminfo.go`](meminfo.go) 和其调用方预期。
- 若补齐 Go 的 `RegisterGetMemoryCapacity`、日志或 failpoint 行为，应先确认 Rust 对应基础设施与真实消费者，避免为了表面对齐引入无调用方的桩。

## 验证依据

- 源文件：[`pkg/util/memory/meminfo.rs`](meminfo.rs)，逐项核对了 2 个类型别名、2 个公共静态探针、`CacheValue`、3 个缓存入口、缓存辅助函数、普通/cgroup 探针、选择函数、初始化函数和实例堆函数；文件无 `cfg` 项。
- crate 与模块：[`pkg/util/memory/Cargo.toml`](Cargo.toml)、[`pkg/util/memory/lib.rs`](lib.rs)，确认 crate 名、模块公开方式、`cgroup_crate`/`sysinfo` 依赖和独立测试挂接。
- RustCodeGraph：`status` 显示索引覆盖本仓库；`files --filter pkg/util/memory` 收录目标源和测试；`node --file ...` 核对完整源码；`query MemTotal`/`query MemUsed`/`query MemAvailable` 确认符号集合且本文件没有 `MemAvailable`；`node InitMemoryHook`、`node MemTotalCGroup`、`node MemUsedCGroup`、`node InstanceMemUsed`、`node GetMemTotalIgnoreErr` 取得上述调用 Trail。一次 `callers InitMemoryHook` 查询长时间无输出后中止，调用方改由索引文件信息和仓库直接引用搜索核验。
- Go 对照：[`pkg/util/memory/meminfo.go`](meminfo.go)，核对公共 API、TTL、缓存、cgroup 选择、错误顺序、failpoint、日志和 sysutil 回调差异。
- 独立测试：[`pkg/util/memory/meminfo_test.rs`](meminfo_test.rs)，确认容器分支不求值闭包，以及非容器 limit 错误与三类比较结果；同目录没有专门的 `meminfo_test.go`，Go 直接证据来自生产对照文件与调用方测试。
- 上下游直接证据：[`pkg/util/cgroup/cgroup_memory.rs`](../cgroup/cgroup_memory.rs)、[`pkg/util/cgroup/cgroup_cpu_linux.rs`](../cgroup/cgroup_cpu_linux.rs)、[`pkg/util/memory/memstats.rs`](memstats.rs)，以及启动、仲裁器、优化器、告警和 LRU 调用路径。本文只说明当前实现，没有运行 Cargo，也没有把未覆盖行为表述为已验证。
