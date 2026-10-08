# `pkg/util/memory/memstats.rs`

## 文件定位

[`memstats.rs`](memstats.rs) 是 `astersql-util-memory` crate 的进程堆内存采样与快照缓存层，由 [`lib.rs`](lib.rs) 以公开模块 `pub mod memstats` 暴露。它不读取主机或 cgroup 的总内存：那是相邻 [`meminfo.rs`](meminfo.rs) 的职责；本文件只把当前进程分配器可观测的“已分配字节”和“分配器持有字节”归一为 `MemStats`。

该文件位于运行时内存观测链的底部。上层的 `meminfo::InstanceMemUsed`、服务端内存限制、内存告警、heap profile 回退以及 GOGC/memory-limit 调谐器消费这里的快照。其结果是近似 Go `runtime.MemStats` 中 `HeapAlloc`/`HeapInuse` 的跨平台替代值，而不是 Rust 对 Go runtime 统计结构的完整复刻。

## 核心职责

- `sample_heap` 按目标平台选择真实采样后端：macOS malloc zones、glibc Linux `mallinfo2`，或其他平台的 `sysinfo` 进程内存。
- `ForceReadMemStats` 获取新快照并原子化地替换进程级缓存内容；`ReadMemStats` 优先返回缓存，缓存尚未建立时才采样。
- `LastReadTime` 暴露最近一次强制采样前记录的墙上时间，供需要判断快照新鲜度的调用方读取；当前仓库中未发现调用者。
- `mem_stats_from_allocator` 集中构造字段，既避免各平台分支交换 `heap_alloc`/`heap_inuse`，也为独立测试提供 crate 内验证入口。

本文件不负责按 `ReadMemInterval` 定时刷新。常量只是与 Go 保持一致的建议周期；当前 Rust 生产代码搜索未发现使用该常量的调度器，缓存只有在调用 `ForceReadMemStats`（或首次 `ReadMemStats`）时更新。

## 主要符号

- `pub const ReadMemInterval: Duration = 300ms`：与 Go 的刷新周期常量对齐，但本模块不会主动建立定时器。
- `pub struct MemStats { heap_alloc, heap_inuse }`：按字节保存快照。`heap_alloc` 表示分配器报告的正在使用量；`heap_inuse` 表示分配器持有/保留量。结构体实现 `Copy`，调用者拿到的是快照值而非缓存内部引用。
- `struct GlobalMemStats { timestamp, stats }`：缓存内部条目；时间戳与采样结果一起在写锁下替换。
- `cache() -> &'static RwLock<Option<GlobalMemStats>>`：通过 `OnceLock` 惰性创建唯一缓存，初始为 `None`。
- `read_unpoisoned` / `write_unpoisoned`：取得读写锁；若其他线程 panic 导致锁中毒，则取回 guard 并继续提供最后状态，而不是再次 panic。
- `pub(crate) const fn mem_stats_from_allocator(...)`：保持两个字段的直接映射，仅 crate 内可见。
- `MallocStatistics` 与 `malloc_zone_statistics`：仅 macOS 编译，映射 Darwin malloc 统计 ABI。
- `MallInfo2` 与 `mallinfo2`：仅 `target_os = "linux" && target_env = "gnu"` 编译，映射 glibc ABI。
- `sample_heap() -> MemStats`：私有平台分发入口。
- `pub fn ReadMemStats() -> MemStats`：读取缓存；仅在缓存为空时调用 `ForceReadMemStats`。
- `pub fn ForceReadMemStats() -> MemStats`：无条件采样、更新时间戳并覆盖缓存。
- `pub fn LastReadTime() -> Option<SystemTime>`：返回缓存时间戳；从未采样时返回 `None`。

## 执行流程

1. 调用方请求普通读取时，`ReadMemStats` 通过 `read_unpoisoned(cache())` 获取读锁。缓存为 `Some` 时复制并立即返回 `stats`，不会检查经过时间，也不会触发系统调用。
2. 缓存为 `None` 时，读锁随表达式结束释放，然后调用 `ForceReadMemStats`。多个线程可同时观察到空缓存并分别采样；结果都有效，最后取得写锁者成为缓存中的最新条目。
3. `ForceReadMemStats` 先记录 `SystemTime::now()`，再调用 `sample_heap`；这一顺序对齐 Go 版“先记时间、后 `runtime.ReadMemStats`”。
4. macOS 分支把空 zone 指针传给 `malloc_zone_statistics`，汇总进程的所有 malloc zone，以 `size_in_use` 作为 `heap_alloc`、`size_allocated` 作为 `heap_inuse`。
5. glibc Linux 分支调用 `mallinfo2`，以 `uordblks` 作为 `heap_alloc`，以 `arena.saturating_add(hblkhd)` 作为 `heap_inuse`，同时覆盖 arena 与 mmap 分配。
6. 其他平台构造并刷新 `sysinfo::System`，读取当前 PID 的进程内存；因后端没有同等的 allocator 字段，两个字段都退化为同一个 resident 值。PID 获取失败或进程条目缺失时返回零。
7. `ForceReadMemStats` 在写锁内把 `GlobalMemStats { timestamp, stats }` 整体写入缓存，再按值返回同一份快照。

## 数据与状态

全局可变状态只有 `OnceLock<RwLock<Option<GlobalMemStats>>>`。`OnceLock` 保证锁只初始化一次；`RwLock` 允许高频缓存读取并发进行，强制刷新时独占替换整个条目。由于 `GlobalMemStats` 和 `MemStats` 都是 `Copy`，锁不会随结果逃逸，调用者也无法修改缓存。

时间戳使用 `SystemTime` 而非单调时钟，表达的是采样发生的墙上时间。系统时间回拨可能使外部计算出的“缓存年龄”为负；本文件只存取时间，不计算持续时间。`ReadMemInterval` 也不参与状态转换。

字段含义受平台能力约束。macOS 与 glibc Linux 尽量区分已用和持有内存；其他平台只能以 resident memory 同时填充两者。这里统计的是分配器/进程层数据，不等于 cgroup 用量、整机用量，也不等于某个 SQL 会话的 tracker 计数。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 声明，crate 名为 `astersql-util-memory`，默认 feature 为空；本文件没有受 `mem-arbitrator` feature 控制。标准库提供 `OnceLock`、`RwLock`、`Duration` 和 `SystemTime`；`sysinfo` 仅在非 macOS、非 glibc Linux 的回退分支中使用。macOS 与 glibc Linux 则直接依赖系统 C allocator ABI。

已核对的 Rust 生产调用关系包括：

- [`meminfo.rs`](meminfo.rs) 的 `InstanceMemUsed` 调用 `ReadMemStats().heap_alloc`，并在其上再做 500ms 的局部缓存。
- `pkg/util/servermemorylimit/servermemorylimit.rs` 的内存限制检查读取 `ReadMemStats().heap_inuse`，用于更新峰值及判断是否超过服务端限制。
- `pkg/util/memoryusagealarm/memoryusagealarm.rs` 的告警检查在设置了 server memory limit 时使用 `heap_alloc`；其 heap profile 写入在 `memory-stats` 采样失败时调用 `ForceReadMemStats` 作为回退。
- `pkg/util/gctuner/mem.rs` 的 `readMemoryInuse` 每次调用 `ForceReadMemStats().heap_inuse`，为 GC 和 memory-limit 调谐提供新样本。
- `pkg/server/tidb_library_test.rs` 与 executor 的不稳定内存测试也直接强刷，用采样前后差值验证内存行为；这些是消费证据，不是本模块的独立单元测试。

RustCodeGraph 的文件节点还显示 `memstats.rs` 被 `pkg/util/gctuner/mem_test.rs`、`global_arbitrator_3_aster_unit_test.rs` 和 `memstats_test.rs` 使用。对精确 callers/callees 的命令在本次会话未产生可用输出，因此调用边又以符号搜索和上述调用点源码核验；未据此推断不存在其他动态或未来调用者。

## 错误处理与边界

公开 API 不返回 `Result`。macOS/glibc 的 FFI 被视为进程平台契约，调用失败没有可表达的 Rust 错误通道；其他平台的 PID 或进程查询失败则保守返回 `MemStats { 0, 0 }`。调用方若把零解释为“内存很低”，必须结合自身安全策略处理。

`arena + hblkhd` 使用饱和加法，避免极端统计值在 `usize` 上溢。锁中毒会恢复内部值继续服务，这提高监控路径的可用性，但也意味着触发中毒的 panic 不会由后续读者再次暴露。

FFI 结构体布局以 `#[repr(C)]` 固定，且只在匹配平台编译。其安全性依赖系统声明与本地结构保持 ABI 一致。macOS 传入的输出对象有效且可写；glibc 的 `mallinfo2` 无参数并按值返回。其他 libc 环境不会误用 glibc 接口，而会落入 `sysinfo` 回退。

与 Go 版不同，Rust `ReadMemStats` 没有 `ReadMemStats` failpoint 注入分支；依赖该注入点的 Go 测试语义不能直接假设在 Rust 中存在。Rust 也不承诺返回 Go runtime 的 GC、对象数量等其他字段。

## 并发与资源生命周期

缓存生命周期与进程一致，无显式销毁。首次访问初始化 `RwLock`；后续普通读取只短暂持有共享锁，强制刷新在执行平台采样时尚未持锁，采样完成后才短暂取得写锁。因此潜在较慢的系统/allocator 查询不会阻塞已有缓存读者。

并发强刷没有单飞控制或时间戳比较：两个刷新可以交错，后完成写入者覆盖先写入者；因为时间戳在采样之前记录，极端调度下缓存也可能被一个较早开始、较晚完成的采样覆盖。这是快照缓存的允许行为，不能把 `LastReadTime` 当作严格单调的提交序号。

本文件不分配后台线程、不建立 channel、不持有文件描述符。macOS/glibc 采样只读取 allocator 状态；回退分支的临时 `System` 在 `sample_heap` 返回时释放。`ReadMemStats` 返回值为复制值，不存在借用、释放或跨线程所有权要求。

## 与 Go 版本的对应关系

Go 对照文件为 [`memstats.go`](memstats.go)。两边都有 300ms 的 `ReadMemInterval`、惰性首次采样、显式强刷、采样时间戳和全局快照缓存。Go 使用 `atomic.Pointer<globalMstats>` 无锁发布不可变快照，Rust 使用 `RwLock<Option<GlobalMemStats>>` 并按值返回；两者都避免把可变缓存直接交给调用者。

Go 的数据源是 `runtime.ReadMemStats`，字段为完整的 `runtime.MemStats`；Rust 只能用平台 allocator 或 resident memory 近似映射两个必要字段。Go `ReadMemStats` 支持 failpoint 增加 `HeapInuse`，Rust 没有该注入语义。Go 的时间戳字段 `globalMstats.ts` 是包内状态；Rust 额外提供公开 `LastReadTime`，但当前没有仓内调用者。

Go 的 `pkg/domain/domain.go` 创建 `ReadMemInterval` ticker，并周期调用 `ForceReadMemStats`，所以普通读取能获得定期更新的缓存。当前 Rust 源码搜索只找到 `ReadMemInterval` 的定义，没有找到对应生产调度器；因此 Rust 的普通读取会一直返回最近一次显式强刷结果。该差异应在接入域生命周期时处理，不能通过让每次 `ReadMemStats` 都采样来悄然改变缓存契约和调用成本。

## 扩展指南

- 增加平台后端时，应在 `sample_heap` 添加互斥的 `cfg` 分支，并明确该平台如何区分 `heap_alloc` 与 `heap_inuse`；若只能取得 RSS，需继续显式说明语义退化。同步扩展独立的 [`memstats_test.rs`](memstats_test.rs)，不要把测试嵌入生产文件。
- 调整字段映射时，先修改 `mem_stats_from_allocator` 的调用参数或平台取值，并保留 `allocator_fields_preserve_heap_alloc_and_heap_inuse_semantics` 对字段顺序的回归覆盖。内存限制与告警分别依赖 `heap_inuse` 和 `heap_alloc`，交换字段会直接改变保护阈值。
- 若补齐 Go 的周期刷新，应在拥有应用生命周期和关闭信号的上层模块接入 `ReadMemInterval`/`ForceReadMemStats`，并验证任务退出；本文件不适合自行启动无法停止的全局线程。
- 若需要测试注入，优先设计显式、可恢复且仅测试启用的采样器边界，同时覆盖并发恢复；不要让生产缓存永久保留伪造值。
- 修改 FFI 结构或统计公式时，需要按目标平台核对系统头文件/文档，并运行对应平台测试。尤其要保留 macOS “所有 malloc zones”语义以及 Linux mmap 分配计入 `heap_inuse` 的语义。
- 性能方面应避免在 `ReadMemStats` 的缓存命中路径加入系统刷新；正确性方面需评估零值回退、时钟回拨及并发强刷；兼容性方面需保持 `MemStats` 字段名、单位和 Go 近似语义。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/memory` 确认目标、Go 对照与独立测试均已索引；`node --file pkg/util/memory/memstats.rs --offset 1 --limit 240` 读取完整 162 行及三处索引使用关系；`query MemStats`、`query ReadMemStats`、`query MemTotal`、`query MemUsed` 用于区分本文件快照 API 与系统/cgroup 内存 API。
- 直接读取：[`memstats.rs`](memstats.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`memstats.go`](memstats.go) 和 [`memstats_test.rs`](memstats_test.rs)。目标目录不存在 `doc.go`。
- 调用点核验：[`meminfo.rs`](meminfo.rs)、`pkg/util/servermemorylimit/servermemorylimit.rs`、`pkg/util/memoryusagealarm/memoryusagealarm.rs`、`pkg/util/gctuner/mem.rs`；Go 定时刷新证据来自 `pkg/domain/domain.go` 对 `ReadMemInterval` 和 `ForceReadMemStats` 的使用。
- 独立测试证据：[`memstats_test.rs`](memstats_test.rs) 验证 100MiB 活跃分配被统计、macOS 非默认 malloc zone 被统计，以及构造器不交换字段；`global_arbitrator_3_aster_unit_test.rs::memstats_cache_can_be_forced_and_read` 验证强刷结果随后可由缓存原样读出；`pkg/util/gctuner/mem_test.rs` 验证调谐器读取值与缓存的 `heap_inuse` 一致。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付前运行任务指定的 11 章节结构命令，并人工检查所有行为结论均指向上述源码、调用点或测试证据。
