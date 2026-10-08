# `pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs`

## 文件定位

本文件实现统计信息缓存的内存配额版本：以物理表 ID（`i64`）为键、`Arc<Table>` 为值，在主缓存中使用 Moka 的加权 TinyLFU 策略，并在 `keySetShard` 中维护包含已淘汰条目的二级集合。crate 入口 `pkg/statistics/handle/cache/internal/lfu/lib.rs` 将本模块整体重新导出；上层 `pkg/statistics/handle/cache/statscacheinner.rs::NewStatsCacheWithCapacity` 在 `quota == true` 时调用 `cache_lfu::NewLFU(capacity)`，再把实例装箱为 `StatsCacheInner`。

crate 边界由同目录 `Cargo.toml` 给出：包名为 `astersql-statistics-handle-cache-internal-lfu`，直接依赖内部缓存 trait crate、缓存指标 crate、`statistics`、启用 `sync` feature 的 `moka 0.12` 以及读取系统内存的 `sysinfo 0.33`。本文件不是独立进程入口，而是统计信息缓存实现层。

## 核心职责

1. `NewLFU` 创建一个按 `Table::MemoryUsage().TotalTrackingMemUsage()` 加权的 TinyLFU 主缓存；容量传入 0 时，`adjustMemCost` 使用系统总内存的 20%。
2. `Put`、`Get`、`Del`、`Values`、`Len` 提供表统计的基本操作，并通过 `impl StatsCacheInner for LFU` 接入统一接口。
3. 主缓存因容量淘汰或拒绝条目时，`retainEvictedShell` 复制表并调用列、索引的 `DropUnnecessaryData`，在二级 `resultKeySet` 中保留仍可查询的轻量“壳表”。
4. `SharedState::addCost`、`LFUMetrics` 及四个指标桥接函数维护内存成本、淘汰/拒绝次数和 Prometheus 指标。
5. `SetCapacity`、`WaitForAsyncUpdates`、`Clear`、`Close` 管理容量变化、Moka 延迟维护队列和资源生命周期。

因此这里的“LFU”并非仅保存当前主缓存驻留项：`Len`/`Values` 面向二级集合，容量淘汰会丢弃统计大对象但保留表级可发现性。这一行为由 `TestCacheLen`、`TestLFUReject` 和并发小容量测试直接覆盖。

## 主要符号

- `type PrimaryCache = Cache<i64, Arc<Table>>`：Moka 同步主缓存别名。
- `LFUError(String)`：构造或容量规范化错误；实现 `Display` 和 `std::error::Error`。当前明确错误包括负容量、无法读取总内存、总内存换算超出 `i64`。
- `LFUMetrics`：四个原子累计量 `costAdded`、`costEvicted`、`evictions`、`rejections`，读取方法均使用 Acquire。
- `SharedState`：所有 `LFU` 克隆共享的状态，包含 `resultKeySet`、当前成本与容量原子量、关闭标志、准入互斥锁、按键频率表和指标。
- `SharedState::{addCost, recordFrequency, frequency}`：维护成本和本实现用于加权准入的频率。
- `SharedState::{retainEvictedShell, dropMemory, onRemoval}`：把完整表转换为壳表、完成成本交接，并处理 Moka 的 `RemovalCause`。
- `LFU { cache, state }`：公开缓存句柄。`cache` 是 `Arc<RwLock<PrimaryCache>>`，便于容量调整时整体替换；`state` 是共享的 `Arc<SharedState>`。
- `cacheWeight`：把表跟踪内存限制在 `[0, u32::MAX]`，满足 Moka weigher 的返回类型。
- `buildPrimary`：配置最大容量、`EvictionPolicy::tiny_lfu()`、权重函数和淘汰监听器。
- `NewLFU`、`adjustMemCost`：构造入口与容量规则。
- `LFU::{Get, Put, Del, Cost, Values, Len, Copy, SetCapacity, WaitForAsyncUpdates, Close, Clear, TriggerEvict}`：公开操作；`metrics` 暴露本实例累计指标。
- `impl StatsCacheInner for LFU`：逐项转发到固有方法，令上层可在 LFU 与 MapCache 之间切换。
- `setCostGauge`、`setCapacityGauge`、`incrementEvictCounter`、`incrementRejectCounter`：在全局指标句柄已初始化时更新指标。

本文件没有条件编译项；测试是在 `lib.rs` 中通过 `#[cfg(test)]` 和独立文件 `lfu_cache_test.rs` 接入。

## 执行流程

构造流程如下：`NewLFU(totalMemCost)` 先调用 `adjustMemCost`；负数返回 `LFUError`，正数原样使用，0 刷新系统内存并取 20%。随后更新容量 gauge，建立 `SharedState`，最后由 `buildPrimary` 创建带 TinyLFU、权重函数和 removal listener 的主缓存。

读取流程由 `Get(tid)` 执行：先通过 `recordFrequency` 增加访问频率，再在读锁保护下查询主缓存；主缓存未命中时调用 `resultKeySet.Get(tid)`。因此仍驻留的完整表优先于二级集合中的值，而已淘汰表仍可通过二级集合返回。

写入流程由 `Put(tblID, tbl)` 执行：

1. 已关闭时立即返回 `false`；否则计算跟踪内存成本并记录候选频率。
2. 先把完整表发布到 `resultKeySet` 并增加成本，保证主缓存准入尚未结束时读侧也能找到该键。
3. 获取 `admission` 互斥锁，将同一实例的准入与手工选 victim 串行化。
4. 若单表成本超过 `maxCost`，使旧驻留键失效，增加拒绝指标，并通过 `dropMemory` 把二级集合中的完整表改为壳表；该分支仍返回 `true`，与测试所要求的“键仍可读”语义一致。
5. 对非驻留候选，若当前加权大小加候选成本超过容量，则枚举主缓存条目，选择频率不高于候选的条目，按频率升序、权重降序排序并逐个失效；每个 victim 在二级集合保留壳表并计一次淘汰。此处是对 Moka 频率相同保留旧项行为的显式加权补偿。
6. 调用 `cache.insert`；若是驻留键替换，立即 `run_pending_tasks`，使替换监听器造成的成本扣减在返回前生效。

Moka 自身产生的移除由 `onRemoval` 接收。`Size`/`Expired` 会按条目是否大于当前容量分别计为拒绝或淘汰，然后执行完整表到壳表的成本交接；`Replaced`/`Explicit` 仅扣除旧完整表成本，因为显式操作或新写入已负责二级集合状态。

容量调整由 `SetCapacity` 在主缓存写锁下完成：排空旧维护任务、收集驻留项、发布新 `maxCost`、创建新主缓存；超过新容量的单项转成壳表，其余重新插入，最后排空新缓存的维护任务并替换旧缓存、更新 gauge。`WaitForAsyncUpdates` 是显式维护屏障；`TriggerEvict` 仅在当前跟踪成本超过上限时调用该屏障。

## 数据与状态

主缓存与二级集合承担不同职责：`PrimaryCache` 保存参与 TinyLFU 准入的完整统计表；`resultKeySet` 保存所有可见键，值可以是完整表，也可以是删除直方图 bucket、TopN 等非必要数据后的壳表。`Values` 和 `Len` 都遍历或统计二级集合，而非 Moka 主缓存。

`cost` 追踪当前由该两级模型计入的内存。写入先加完整表成本；替换、显式删除或淘汰监听器扣除旧完整表成本；容量淘汰还会先增加壳表成本再扣除完整表成本。`LFUMetrics` 的不变量由测试辅助函数 `assertMetricsMatchCost` 表达：在成本非负的测试场景中，`Cost() == CostAdded() - CostEvicted()`。

`maxCost` 是动态容量上限。`frequencies: Mutex<HashMap<i64, u64>>` 同时吸收 `Get` 和 `Put` 的访问，使用饱和加一避免溢出；`Del`、`Clear`、`Close` 会清理相应频率状态。频率表没有衰减逻辑，这是当前实现事实，不应推断为 Moka 内部 sketch 的镜像。

`retainEvictedShell` 使用 `CopyAs(CopyIntent::AllDataWritable)` 创建可写副本，仅对“统计已初始化且状态不是 `AllEvicted`”的列和索引调用 `DropUnnecessaryData`。源码注释明确指出该条件局部实现是为了对齐 Go 的 `HistColl.DropEvicted`，避免复用当前 `Table` 方法中的旧条件。

## 依赖与调用关系

上游主链是 `statscacheinner.rs::NewStatsCacheWithCapacity` → `cache_lfu::NewLFU` → 本文件 `NewLFU`。构造结果装入 `Mutex<Box<dyn StatsCacheInner>>`，随后 `StatsCache::{Get, Put, Values, Cost, SetCapacity, Close, TriggerEvict, WaitForAsyncUpdates}` 通过 trait 调用本实现。`StatsCache::CopyAndUpdate` 使用 `Copy` 后继续 `Put`/`Del`；由于本文件的 `Copy` 是 `self.clone()`，新装箱对象与原对象共享 `cache` 和 `state`。

下游依赖包括：

- `moka::sync::Cache`、`EvictionPolicy::tiny_lfu`、`RemovalCause`：主缓存、准入/淘汰和延迟维护。
- `statistics::{Table, CopyIntent, AllEvicted}`：统计表值、复制意图及淘汰状态判断。
- `key_set_shard::keySetShard`：并发安全的二级键值集合。
- `cache_internal::StatsCacheInner`：上层统一存储接口。
- `cache_metrics`：容量、成本、淘汰和拒绝的全局 Prometheus 指标。
- `sysinfo::System`：容量为 0 时读取物理内存。
- 标准库的 `Arc`、`RwLock`、`Mutex` 与原子类型：共享、整体替换、准入串行化和无锁标量状态。

RustCodeGraph 的本地索引包含该目录的 `lfu_cache.rs`（49 个符号）、Go 对照文件及测试；符号查询定位了 Rust `LFU`（第 198 行）和 `NewLFU`（第 229 行）。对跨 crate 调用未形成可用的直接 callers 输出，因此上游边以 `statscacheinner.rs` 中的实际构造表达式和 trait 转发为准，而不是推测调用图。

## 错误处理与边界

可恢复错误集中在构造期：`adjustMemCost` 对负容量报错；容量为 0 时若系统总内存为 0，或 20% 的结果无法转成 `i64`，返回 `LFUError`。`SetCapacity` 对容量规范化错误采用提前返回，没有把错误暴露给调用者，也没有更新旧容量。

锁中毒使用 `expect`，会 panic，涉及频率锁、准入锁和主缓存读写锁。指标访问位于 `unsafe` 块，因为依赖 crate 以可变静态句柄暴露指标；每个函数先检查 `Option`，未初始化时跳过更新。

权重会把负跟踪内存视为 0，并把超过 `u32::MAX` 的单项截断到 `u32::MAX`；但是超容量判断和成本账本仍使用原始 `i64` 跟踪值。调用者若依赖极大单表的精确 Moka 权重，需要同时审查这两种数值域。

`Put` 的布尔值不是“驻留于主缓存”的证明：超容量项也返回 `true`，但只在二级集合留下壳表。关闭后 `Put` 才返回 `false`。`Get` 会改变频率状态，因而不是纯只读操作。`WaitForAsyncUpdates` 只排空 Moka 当前维护任务，不提供跨所有调用者的事务隔离。

## 并发与资源生命周期

`LFU` 可克隆，两个字段均由 `Arc` 共享。主缓存外围 `RwLock` 允许普通操作并行持有读锁，`SetCapacity` 以写锁阻止其间使用旧缓存并原子替换整个 `Cache`。`admission: Mutex<()>` 串行化 `Put` 中的容量检查、victim 选择与插入，避免多个候选依据同一加权大小同时决策。频率表有独立互斥锁；成本、容量、关闭标志和指标使用原子操作。

Moka 的维护日志不是每次写入都同步执行：`WaitForAsyncUpdates` 调用 `run_pending_tasks`，是观察替换/淘汰和稳定成本前的屏障。测试在关键断言前显式调用它；驻留键替换路径则在 `Put` 内主动排空。

`Close` 通过 `closed.swap(true)` 保证幂等：首次调用使全部主缓存项失效、排空监听任务、清空二级集合和频率表；监听器和壳表保留函数看到 `closed` 后不再改账或重新发布条目。`Clear` 执行相同的内容清理但不设置关闭标志，因此之后仍允许 `Put`。独立测试 `shared_copy_and_close_are_idempotent` 证明克隆共享状态、重复关闭安全且关闭后写入失败。

并发测试覆盖 32 个写线程分片处理 1000 键、读写线程并发遍历同一键空间，以及小容量下反复写入并读取完整表/壳表；这些测试位于独立的 `lfu_cache_test.rs`，没有把测试逻辑嵌入生产文件。

## 与 Go 版本的对应关系

直接对照文件为同目录 `lfu_cache.go`，Go 测试为 `lfu_cache_test.go`。两版都实现两级模型：主缓存负责有容量约束的完整表，`resultKeySet` 保存全部键；`Put` 先发布二级集合，`Get` 先查主缓存再回落；淘汰时复制表、丢弃不必要统计数据并以“壳表成本减完整表成本”更新账本；`Len`/`Values` 基于二级集合；容量 0 默认取系统内存 20%；并提供 `Copy`、`SetCapacity`、等待维护、清理和关闭语义。

实现载体不同：Go 使用 Ristretto 及 `OnReject`、`OnEvict`、`OnExit` 回调，Rust 使用 Moka TinyLFU 的 removal listener。为弥合差异，Rust 对超容量单项显式执行拒绝路径，对频率平局增加“较大权重优先淘汰”的 victim 选择，并在驻留键替换时排空维护日志。Go 的 `triggerEvict` 会写入负随机键促使 Ristretto 工作；Rust 的 `triggerEvict` 只在超限时运行 Moka pending tasks。Rust `SetCapacity` 通过重建主缓存实现，Go 调用 `UpdateMaxCost`。

另有可见差异：Go 在测试环境下把容量 0 固定为 5 MB，而 Rust `adjustMemCost` 没有测试环境分支；Go 的 `adjustMemCost` 对非零负值没有本文件的显式拒绝；Rust 自建 `LFUMetrics` 供测试读取。Rust 源码关于 `Copy` 的注释称“浅拷贝”，实际也确实通过 `Arc` 共享状态；这与接口文件把 trait `Copy` 泛称为“深拷贝”的注释不一致，扩展时应以当前实现和共享状态测试为准。

Rust 测试移植了 Go 的基本写读删、更新成本、超大项、长度、并发、拒绝、容量缩减与重复更新场景，并额外用 `shared_copy_and_close_are_idempotent` 固化共享复制和关闭行为。Go 测试中的等待/Eventually 反映 Ristretto 异步特性；Rust 相应断言主要依靠 `WaitForAsyncUpdates`。

## 扩展指南

- 修改准入或 victim 规则时，从 `Put`、`recordFrequency`、`frequency` 和 `buildPrimary` 入手；必须保持“先发布二级集合”和超容量项仍可查询的契约，并扩展独立的 `lfu_cache_test.rs`，特别关注频率平局、不同权重和并发候选。
- 修改淘汰内容时，从 `retainEvictedShell` 入手，并与 Go `dropMemory`/`Table.DropEvicted` 的条件逐项核对；测试需要同时验证列与索引的 `AllEvicted`、TopN 和 histogram buckets。
- 修改成本账本时，成对审查 `Put` 的加成本、`onRemoval` 的扣成本、`dropMemory` 的完整表/壳表交接以及 `SetCapacity`；继续用 `CostAdded - CostEvicted` 不变量覆盖替换、删除、拒绝和淘汰。
- 新增 `StatsCacheInner` 方法时，需要同时更新 `pkg/statistics/handle/cache/internal/inner.rs`、本文件 trait impl、MapCache 实现和 `statscacheinner.rs` 上层门面；不要只增加固有方法。
- 改动生命周期时，应覆盖 `LFU` 克隆之间的可见性、`Clear` 后可重用、`Close` 幂等、关闭后 listener 不回填二级集合等竞争窗口。
- 修改容量为 0、负容量或系统内存读取规则时，同步核对 Go 差异及 `sysinfo` 单位，并为 `adjustMemCost` 增加独立测试；不要通过在生产源文件内嵌测试规避仓库的测试分离规则。
- 指标扩展应同时更新实例级 `LFUMetrics` 与 `cache_metrics` 桥接，并留意可变静态指标句柄的初始化边界和 `unsafe` 范围。

性能风险主要集中在 `Put` 超限时遍历并排序整个 Moka 缓存、频率表持续增长、`SetCapacity` 重建全部驻留项，以及频繁 `run_pending_tasks`；兼容性风险集中在与 Go/Ristretto 的准入、回调顺序和主缓存优先读取语义偏差。

## 验证依据

- 生产源码：`pkg/statistics/handle/cache/internal/lfu/lfu_cache.rs`，核对了全部类型、函数、trait 实现、锁/原子状态、Moka 配置、成本交接和指标函数。
- crate 与模块入口：`pkg/statistics/handle/cache/internal/lfu/Cargo.toml`、`pkg/statistics/handle/cache/internal/lfu/lib.rs`，核对包名、依赖、`sync` feature、公开重导出及独立测试接线。
- 直接上游与接口：`pkg/statistics/handle/cache/statscacheinner.rs`、`pkg/statistics/handle/cache/internal/inner.rs`，核对 `NewStatsCacheWithCapacity` 的构造条件和 `StatsCacheInner` 调用边界。
- Rust 独立测试：`pkg/statistics/handle/cache/internal/lfu/lfu_cache_test.rs`，核对基本操作、成本不变量、超容量保留、并发、壳表内容、容量调整、共享复制与关闭行为。
- Go 对照：`pkg/statistics/handle/cache/internal/lfu/lfu_cache.go`、`pkg/statistics/handle/cache/internal/lfu/lfu_cache_test.go`，核对 Ristretto 回调、二级集合、成本账本、容量和测试意图。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件和 4415 个 Go 文件；`files --filter pkg/statistics/handle/cache/internal/lfu` 显示目标 Rust/Go 源与测试均已索引；`query LFU --kind struct`、`query NewLFU --kind function` 定位本文件核心类型和构造入口。跨 crate callers/callees 未返回足以单独证明接线的边，故相应结论使用上述入口源码直接验证。
- 结构验证按任务文件的精确命令执行，要求目标文件存在且固定二级标题恰好为 11 个；本任务为纯文档分析，按要求未运行 Cargo。
