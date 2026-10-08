# `pkg/util/memory/utils.rs`

## 文件定位

该文件是 `astersql-util-memory` crate 的内部基础工具集合。crate 根文件 [`lib.rs`](./lib.rs) 通过 `pub mod utils { include!("utils.rs"); ... }` 把它编译进 `utils` 模块，并在同一模块补充 `prime64`、`initHashKey` 与 `baseQuotaUnit`；随后 `pub(crate) use utils::*` 让 crate 内的仲裁器、heap profile 和 tracker 可直接使用其中的 crate-private 符号。crate 边界和依赖由 [`Cargo.toml`](./Cargo.toml) 定义，默认 feature 为空，另有 `mem-arbitrator` feature；本文件直接使用标准库、`crossbeam-utils` 和 `memory-stats`。

它不是 SQL 请求入口，而是内存治理链的叶层组件：当前 Rust 生产路径中，会话运行时用 digest builder 生成内存画像键，内存仲裁器和 heap profile 用千分比运算，global arbitrator 用进程内存采样结果更新风险状态。文件中的可复用链表、通知器、分片函数和若干兼容工具目前只在本 crate 的独立测试中出现；不能据 Go 版的接线推断它们已经接入 Rust 仲裁器生产路径。

## 核心职责

1. `wrapList<V>` 以槽位数组、活跃下标队列和空闲下标栈实现可复用 FIFO/LRU 风格容器，避免删除后反复分配节点。
2. `Notifer`（名称保留 Go 原拼写）以容量为 1 的同步通道和原子 `awake` 标志合并多生产者唤醒，由单消费者 `Wait` 消费。
3. `DigestIDBuilder`、`HashEvenNum`、`shardIndexByUID` 和 `getQuotaShard` 提供无歧义 digest、UID 分片及按配额数量级分桶。
4. `nowUnixMilli`、`nowUnixSec`、`nextPow2`、`calcRatio`、`multiRatio` 和 `intoRatio` 提供仲裁器常用的时间、容量和千分比换算。
5. `SampleRuntimeMemStats` 与 `IntoRuntimeMemStats` 将 Rust 进程采样或显式 `RustMemStats` 映射为 Go 风格的 `RuntimeMemStats` 字段集合。

## 主要符号

- 字节与比例常量：`byteSize`、`byteSizeKB`、`byteSizeMB`、`byteSizeGB`、`kilo`。其中 `baseQuotaUnit = 4 * byteSizeKB` 在 `lib.rs` 的 `utils` 模块内定义；`kilo = 1000` 是比例整数化基数。
- `wrapList<V>` 保存 `slots: Vec<Option<V>>`、`active: VecDeque<usize>`、`free: Vec<usize>` 和活跃计数 `num`。`pushBack` 优先复用 `free` 槽；`remove`/`popFront` 清空槽并回收下标；`moveToFront` 改变活跃顺序。`wrapListElement` 是含 `Option<usize>` 的可复制句柄，`slot()` 对无效句柄 panic。
- `Notifer { C, receiver, awake }`：`NewNotifer()` 创建容量为 1 的 `sync_channel`；`Wake`/私有 `wake` 只在 `awake` 从 0 变 1 时发送；`WeakWake` 先观察状态；`Wait` 串行接收后清零。
- `HashStr(&str) -> u64`：按 Unicode 标量值迭代，用 `prime64` 乘法和异或累积。当前仓库未找到 Go 同名实现或 Rust 生产调用，只有黄金值测试。
- `DigestIDBuilder`：`NewDigestIDBuilder` 从 `initHashKey` 开始；`AddString` 先混入字节长度，再按 8 字节小端块及不足 8 字节的尾块混入，因而 `("ab", "c")` 与 `("a", "bc")` 不会仅靠拼接而碰成同一输入；`Sum64` 做 MurmurHash3 风格 avalanche，并把保留值 `InvalidDigestID == 0` 改为 1。
- `HashEvenNum` 先混合低 8 位再混合其余高位；`shardIndexByUID` 用哈希值与 `shardsMask` 做按位与，调用者必须提供适合该算法的掩码（通常是 `2^n - 1`）。`getQuotaShard` 以 `quota / baseQuotaUnit` 的位长选择对数桶，并钳制到最后一个桶。
- `nextPow2` 用逐级位扩散返回不小于输入的最小 2 的幂，`0` 特判为 `1`。`calcRatio`、`multiRatio` 显式使用 wrapping 乘法以复现 Go `int64` 溢出行为；`intoRatio` 把浮点比例截断为千分比整数。
- `cpuCacheLinePad<T> = CachePadded<T>` 是减少伪共享的类型别名，当前文件外没有 Rust 引用。
- `RuntimeMemStats` 是对外统计视图；`RustMemStats` 是显式转换输入。`SampleRuntimeMemStats` 从 `memory_stats::memory_stats()` 读取物理/虚拟内存，`IntoRuntimeMemStats` 则逐字段转换并用 wrapping 减法计算累计释放量和堆外量。
- `gcTrackerState`、静态 `gcTracker` 与 `approxLastGCTime` 是 Go GC 跟踪结构的预留映射；当前 Rust 采样不更新这些原子量，仓库中也未发现文件外调用。

## 执行流程

会话内存画像键的真实路径是 `pkg/session/runtime.rs::build_mem_arbitrator_digest_id`：规范化 SQL 为空时返回 `InvalidDigestID`；否则依次向 builder 加入固定域标签 `"db"`、小写数据库名和规范化 SQL，最后调用 `Sum64`。生成的非零 ID 传入 tracker，`pkg/util/memory/tracker.rs::InitMemArbitratorWithSharedKiller` 仅在 ID 非零且没有显式预留量时查询 digest profile cache。

比例工具的真实生产路径有两类。`pkg/util/memory/arbitrator.rs` 在更新内存放大系数、判断风险和计算软容量时调用 `calcRatio`；`pkg/util/memory/heap_profile.rs::Snapshot::from_arbitrator` 用 `multiRatio` 计算捕获阈值，`try_capture` 用 `calcRatio` 计算当前使用率，再决定重置、普通捕获或紧急捕获状态。

运行时采样路径是 `pkg/util/memory/global_arbitrator.rs::sample_runtime_mem_stats` 调用 `SampleRuntimeMemStats`，再把 `HeapAlloc`、`HeapInuse`、`MemOffHeap` 和 `TotalFree` 转成 `ArbitratorRuntimeStats`。采样失败时返回全零默认值；采样成功时物理驻留量同时作为 `HeapAlloc`/`HeapInuse`，虚拟量减物理量（饱和到零）作为 `MemOffHeap`。

容器和通知器的内部流程由独立测试直接验证：`wrapList` 插入时复用空闲槽、活跃下标入队；移除或弹出时释放槽；`moveToFront` 改写顺序。`Notifer` 的多个 `Wake`/`WeakWake` 在第一次发送后保持 `awake = 1`，直到 `Wait` 从通道取走单个令牌并清零，之后才能开启下一轮唤醒。

## 数据与状态

`wrapList` 的关键不变量是：`num` 等于 `active` 中有效元素数；每个活跃下标对应 `slots[index] = Some(V)`；`free` 中的下标对应空槽且不在 `active` 中。已分配槽位总数可由测试专用的 `allocated_len` 观察。句柄只保存下标且实现 `Copy`，容器不会自动使调用者持有的副本失效；元素被移除/弹出并且槽位复用后，旧句柄可能指向新值，因此调用方必须自行遵守“不再使用已删除句柄”的生命周期约束。

`Notifer.awake` 只取 0/1，使用 `SeqCst` 保证跨生产者和消费者的全序观察；`receiver` 用 `Mutex` 保证同时最多一个 `Wait` 真正接收。公开字段 `C` 与内部 `awake` 是两份关联状态，正常调用必须经 `Wake`/`WeakWake`，否则直接发送可能破坏“通道中至多一个与 awake 对应的令牌”的约束。

digest builder 仅维护一个 `u64`，各算术步骤都显式 wrapping，结果与 debug/release 溢出检查无关。`InvalidDigestID` 是控制语义而非哈希失败码：零表示禁用画像查询和更新，正常 `Sum64` 保证不返回零。

内存采样没有缓存和后台任务，每次调用都读取当前进程快照。`gcTracker` 虽使用原子字段，但 Rust 采样目前固定返回 `NumGC = 0`、`TotalFree = 0`，也不更新 `lastGCTime`/`lastNumGC`。

## 依赖与调用关系

直接下游依赖如下：

- `std::collections::VecDeque` 支撑 `wrapList` 活跃顺序；`std::sync::mpsc::sync_channel`、`Mutex` 与原子类型支撑通知器和预留 GC 状态；`SystemTime` 支撑 Unix 时间函数。
- `crossbeam_utils::CachePadded` 只用于 `cpuCacheLinePad` 别名。
- `memory_stats::memory_stats` 是 `SampleRuntimeMemStats` 的唯一系统采样入口；依赖在本 crate 的 `Cargo.toml` 中声明为 `memory-stats = "1"`。
- `prime64`、`initHashKey`、`baseQuotaUnit` 来自包裹 `include!` 的 `lib.rs::utils` 模块，而不是在本文件独立定义。

已核实的 Rust 上游包括：`pkg/session/runtime.rs::build_mem_arbitrator_digest_id` → `NewDigestIDBuilder`/`AddString`/`Sum64`；`pkg/util/memory/arbitrator.rs` → `calcRatio`；`pkg/util/memory/heap_profile.rs` → `calcRatio`/`multiRatio`；`pkg/util/memory/global_arbitrator.rs::sample_runtime_mem_stats` → `SampleRuntimeMemStats`；`pkg/util/memory/tracker.rs` → `InvalidDigestID`。RustCodeGraph 将目标文件标为被 18 个文件使用，但精确文本引用表明许多符号只通过测试或模块内再导出触达，故这里不把文件级“使用”全部解释为生产调用。

## 错误处理与边界

本文件不返回 `Result`。系统内存采样不可用时静默降级为全零 `RuntimeMemStats`，上游必须把零值理解为“没有可用快照”，而不是证明进程没有内存占用。虚拟内存小于物理内存时用 `saturating_sub` 避免下溢；相反，`IntoRuntimeMemStats` 为对齐 Go 无符号算术使用 wrapping 减法，若输入违反 `TotalAlloc >= Alloc` 或 `Sys >= HeapSys`，会得到很大的回绕值。

`Notifer::Wait` 在 mutex 中毒或所有 sender 断开时 panic；`wake` 在 receiver 断开时 panic。正常 API 没有超时或关闭协议。`wrapListElement::slot` 对 `reset` 后句柄 panic，而 `remove` 对“槽位当前不在 active”安静返回；超出 `slots` 范围的伪造句柄无法由公开构造产生。

`getQuotaShard` 隐含要求 `maxQuotaShard > 0`；传入 0 会计算 `maxQuotaShard - 1` 并在 debug 构建触发溢出。负 quota 先转换为 `u64`，会落入高位桶；现有测试和预期调用只覆盖非负配额。`calcRatio` 在 `y == 0` 时 panic。`nextPow2` 对大于 `2^63` 的非幂输入最终 wrapping 为 0；现有测试范围是 `0` 和最高到 `2^62` 的邻域。

## 并发与资源生命周期

`wrapList` 本身没有锁和原子操作，要求由拥有者串行访问或在外层加锁。`approxSize`/`approxEmpty` 在 Rust 中与精确读取完全相同，并没有 Go `//go:norace` 的特殊运行时含义。

`Notifer` 面向多生产者、单消费者：生产者可共享 `&Notifer` 调用唤醒；消费者通过 mutex 串行 `Wait`。强唤醒保证在 `awake` 从 0 到 1 时写入一个令牌。`WeakWake` 的“先读再尝试”允许与消费并发时丢失弱信号，这与 Go 注释中的弱语义一致。通知器没有显式 close；最后一个 sender 和对象一同析构，但阻塞中的 `Wait` 是否被唤醒取决于 sender 生命周期并可能以断连 panic 结束。

`SampleRuntimeMemStats` 是同步快照调用，不持有跨调用资源。静态 `gcTracker` 生命周期覆盖整个进程，但当前只是零初始化的预留状态。digest 和数值工具均为调用方局部状态，不创建线程、任务、锁或 I/O 资源。

## 与 Go 版本的对应关系

Go 对照文件是 [`utils.go`](./utils.go)，核心命名和算术意图被保留，但实现并非完全等价。Go `wrapList` 使用 `container/list` 和尾部哨兵复用 `list.Element`；Rust 使用 `Vec<Option<V>> + VecDeque<usize> + free`，保持顺序与槽位复用语义，却增加了“旧下标句柄在槽位复用后可能别名”的 Rust 特有边界。Go `Notifer` 使用容量 1 的 `chan struct{}` 和 `atomic.Int32`；Rust 用 `sync_channel(1)`、`Mutex<Receiver>` 与 `AtomicI32` 映射相同的合并唤醒协议。

`DigestIDBuilder` 与当前 Go 版逐步对应：长度、8 字节小端块、尾块和最终 avalanche 常量一致，并同样避开零 ID。`HashEvenNum`、配额分桶、幂和千分比运算也保留 Go 算法；Rust 的 wrapping 运算明确固定了 Go 整数回绕语义。当前 Go `utils.go` 中没有 `HashStr`，所以只能确认 Rust 测试黄金值，不能声称它与当前 Go 同名实现逐行对应。

最大差异在运行时内存统计。Go `SampleRuntimeMemStats` 通过 `runtime/metrics` 读取堆对象、unused/free/released、总内存、累计 frees 和 GC cycles，使用 `sync.Pool` 复用采样数组，并在 GC 次数增长时更新近似 GC 时间。Rust 版用 `memory-stats` 的进程物理/虚拟内存近似，固定 `TotalFree`/`NumGC` 为零且不更新 `gcTracker`，因此字段形状对齐但信息精度和 GC 语义尚未对齐。Rust 的 `RustMemStats` 是为测试与显式映射增加的本地输入类型，Go 直接接收 `runtime.MemStats`。

Go 的生产仲裁器已经把 `wrapList`、`Notifer`、`shardIndexByUID`、`getQuotaShard` 和 `nextPow2` 接入任务队列、分片表和构造流程；当前 Rust 精确引用只证明这些工具受独立测试覆盖，未证明同样的生产接线。该迁移差异应保持显式，后续若接线需以 Rust 仲裁器现有结构为准。

## 扩展指南

新增容器操作时，应同时维护 `slots`、`active`、`free` 和 `num` 四者不变量，并在独立文件 [`utils_5_aster_unit_test.rs`](./utils_5_aster_unit_test.rs) 中覆盖顺序、重复删除、槽位复用和旧句柄行为；不要把测试内嵌回生产源文件。若要让句柄安全跨删除使用，应先引入 generation 等防陈旧机制，而不是仅依赖下标。

扩展通知协议时，优先在 `Notifer` 内封装发送端，避免直接操作公开 `C` 导致状态分裂；任何 close、超时或多消费者设计都必须重新定义 `awake` 清理时点，并增加并发交错测试。保持 `WeakWake` 可能丢信号的契约，除非同步修改 Go 对照和全部调用方。

修改 digest 算法会改变画像 cache 键，是跨版本兼容风险；应同步核对 `pkg/session/runtime.rs::build_mem_arbitrator_digest_id`、tracker 的 `InvalidDigestID` 分支和 [`go_merge_30_test.rs`](./go_merge_30_test.rs) 的组件边界测试。若需要版本化，宜新增域标签或版本前缀，而不是无提示地改变 `AddString`/`Sum64`。

改进 `SampleRuntimeMemStats` 时，应明确是进程 RSS 还是 allocator/堆语义，并同步检查 `global_arbitrator.rs` 风险判断和 heap profile 阈值；若补 GC 数据，还要真正更新 `gcTracker` 并为失败/回退路径增加测试。数值工具扩展需补零除数、负配额、最大整数和 `nextPow2` 溢出边界；现有回绕语义由 [`utils_test.rs`](./utils_test.rs) 专门验证，不应改成饱和计算而不评估 Go 兼容性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/memory` 确认目标、Go 对照和独立测试均已索引；`node --file pkg/util/memory/utils.rs` 读取全部 445 行并报告目标被 18 个文件使用；`query` 核对了 `wrapList`、`NewNotifer`、`HashStr`、`DigestIDBuilder`、`HashEvenNum`、`shardIndexByUID`、`getQuotaShard`、比例函数和内存采样符号。批量 `callers/callees` 曾超时，随后以精确文件节点和引用搜索收敛调用边，未把不完整图结果当作事实。
- 生产源码：[`utils.rs`](./utils.rs)、[`lib.rs`](./lib.rs)、`pkg/session/runtime.rs`、[`arbitrator.rs`](./arbitrator.rs)、[`heap_profile.rs`](./heap_profile.rs)、[`global_arbitrator.rs`](./global_arbitrator.rs)、[`tracker.rs`](./tracker.rs)。
- crate/Go 对照：[`Cargo.toml`](./Cargo.toml)、[`utils.go`](./utils.go) 和 [`arbitrator.go`](./arbitrator.go)。
- 测试证据：[`utils_5_aster_unit_test.rs`](./utils_5_aster_unit_test.rs) 覆盖槽位复用、通知合并、哈希/分片/比例/幂、时间与内存字段映射；[`utils_test.rs`](./utils_test.rs) 覆盖 Go `int64` 回绕；[`go_merge_30_test.rs`](./go_merge_30_test.rs) 覆盖 digest 组件边界与非零稳定性；[`arbitrator_test.rs`](./arbitrator_test.rs) 提供内嵌 Go 对齐用例；Go [`arbitrator_test.go`](./arbitrator_test.go) 的 `TestBasicUtils` 覆盖原版 digest、分片、通知、链表、时间和幂行为。
- 人工复核结论：文档区分了已接入 Rust 生产路径与仅测试覆盖的工具，列出了输入前提、panic/回绕/降级边界、并发生命周期、Go 内存采样差异以及安全扩展所需的独立测试位置；没有运行 Cargo，因为任务和总计划明确排除代码构建测试。
