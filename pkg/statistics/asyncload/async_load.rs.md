# `pkg/statistics/asyncload/async_load.rs`

## 文件定位

本文件实现 `astersql-statistics-asyncload` crate 的核心数据结构：一个进程级、分片加锁的“待异步加载统计项”集合。crate 入口 `pkg/statistics/asyncload/lib.rs` 通过 `mod async_load` 声明本模块，并以 `pub use async_load::*` 导出其公开接口；`pkg/statistics/asyncload/Cargo.toml` 则表明生产依赖只有 `astersql-meta-model`，数据类型直接复用其中的 `TableItemID` 与 `StatsLoadItem`。

它位于查询规划与统计加载之间，但不访问 KV，也不创建后台线程：Session/Planner/Domain 将尚未完整加载的列或索引登记到 `AsyncLoadHistogramNeededItems`，`Domain::load_needed_histograms` 再取得快照、加载统计并逐项删除。对应生产证据见 `pkg/session/runtime/control.rs::collect_predicate_columns_point`、`pkg/planner/core/optimizer_runtime.rs` 的同步加载失败回退，以及 `pkg/domain/domain.rs::{enqueue_async_load_items,load_needed_histograms}`。

## 核心职责

1. 以完整 `TableItemID` 为身份对待加载请求去重；键包含表 ID、列/索引 ID、是否索引和同步加载是否失败四个字段（`TableItemKey`）。
2. 保存每项的 `FullLoad` 请求，并保证同一键只会从部分加载升级为完整加载，不能被后续请求降级（`NeededStatsInternalMap::Insert` 中的 `*current |= full_load`）。
3. 将请求按统计项 ID 分到 128 个独立分片，缩小并发读写的锁竞争范围（`shardCnt`、`getIdx`、`NeededStatsMap`）。
4. 提供插入、精确删除、全量快照和计数接口，供生产链和独立测试使用（`NeededStatsMap::{Insert,Delete,AllItems,Length}`）。

本文件不负责判断某项是否需要加载、不负责调度或重试，也不负责解释/写回直方图；这些策略在调用方中实现。特别是 `AllItems` 只复制当前内容，并不会原子地“取走”或清空队列。

## 主要符号

- `pub use astersql_meta_model::{StatsLoadItem, TableItemID}`：同时作为内部数据模型和 crate 的公开再导出。`TableItemID` 描述物理表中的列/索引；`StatsLoadItem` 在其上附加 `FullLoad`。
- `pub static AsyncLoadHistogramNeededItems: LazyLock<NeededStatsMap>`：进程级全局集合，第一次访问时调用 `newNeededStatsMap` 完成惰性初始化。
- `TableItemKey`：私有、可哈希的内部键。`from_item` 完整复制 `TableItemID` 的四个字段，`into_item` 无损还原公开类型。
- `NeededStatsInternalMap`：单个分片；内部是 `RwLock<HashMap<TableItemKey, bool>>`，其中 `bool` 表示是否要求完整加载。
- `NeededStatsInternalMap::{AllItems,Insert,Delete,Length}`：分别在读锁或写锁下完成单分片快照、插入/升级、精确删除和计数。
- `const shardCnt: usize = 128`：固定分片数，与 Go 实现一致。
- `pub fn getIdx(&TableItemID) -> usize`：对 `ID` 使用 `unsigned_abs()` 后模 128。正负同绝对值会进入同一分片，但完整键仍能区分它们。
- `pub fn newNeededStatsMap() -> NeededStatsMap`：用 `std::array::from_fn` 构造 128 个空分片。
- `pub struct NeededStatsMap` 及其公开方法：对外隐藏分片数组；`Insert`/`Delete` 只锁一个分片，`AllItems`/`Length` 顺序访问所有分片。

文件级 `#![allow(non_snake_case, non_upper_case_globals)]` 保留 Go 风格公开名称，使迁移调用点仍可使用 `AllItems`、`Insert`、`AsyncLoadHistogramNeededItems` 等名称。

## 执行流程

典型异步路径如下：

1. Session 在 `tidb_stats_load_sync_wait = 0` 时收集谓词列、裁剪无关索引并调用 `Domain::enqueue_async_load_items`；同步加载失败的 Planner 路径也会把待处理项转存到本全局集合，并设置 `IsSyncLoadFailed = true`。
2. Domain 根据统计缓存状态为列或索引构造 `TableItemID`，调用全局 `Insert(item, full_load)`。
3. `NeededStatsMap::Insert` 用 `getIdx` 选定分片；分片取得写锁，以完整键查找或插入值，再用逻辑或合并 `FullLoad`。重复请求因此合并，而完整加载请求具有支配性。
4. `Domain::load_needed_histograms` 调用 `AllItems`。外层方法逐分片取得读锁，把每项转换回 `StatsLoadItem` 并拼接成快照。
5. Domain 按 `IsIndex` 选择索引或列加载逻辑。对应加载包装函数无论内部返回成功还是错误，都会调用 `Delete` 删除精确键；删除再次路由到同一分片并在写锁下执行。

`Length` 与 `AllItems` 都逐分片读取，因此适合观测或测试，但并不是跨 128 个分片的单一原子快照：并发修改时，各分片反映的时刻可能不同。

## 数据与状态

队列中一条记录的逻辑形态是 `(TableItemID, FullLoad)`。身份由 `TableID`、`ID`、`IsIndex`、`IsSyncLoadFailed` 四元组组成；值仅为 `FullLoad`。因此，同一表和 ID 的列项、索引项或不同同步失败状态是不同记录，删除也只影响完全匹配的一条。该不变量由 `migration_all_table_item_fields_participate_in_map_identity` 和 `migration_delete_removes_only_the_exact_item` 覆盖。

状态转换只有三类：不存在的键经 `Insert` 变为 `false` 或 `true`；已有 `false` 可升级到 `true`；`true` 不会降级；`Delete` 将精确键移除。没有过期时间、容量上限或持久化，进程退出即丢失。HashMap 的遍历顺序未定义，所以 `AllItems` 返回顺序也没有契约，调用方和测试不能依赖顺序。

分片只由 `ID` 决定，而非完整键。这是负载分散策略，不是身份规则；碰撞仅意味着共享一把锁。`migration_negative_and_positive_ids_share_a_shard_without_colliding` 验证 `17` 与 `-17` 同分片但不互相覆盖。

## 依赖与调用关系

直接下游依赖只有标准库的 `HashMap`、`LazyLock`、`RwLock` 和 `astersql-meta-model::{TableItemID,StatsLoadItem}`。`Cargo.toml` 没有 feature 条件，也没有条件编译分支。

直接生产调用关系如下：

- `pkg/session/runtime/control.rs::collect_predicate_columns_point` 收集查询谓词统计需求；零同步等待时经 Domain 入队，同步等待失败时使用带失败标记的入口。
- `pkg/domain/domain.rs::enqueue_async_load_items_for_physical_id` 检查缓存中的列/索引是否仍需加载，并调用 `AsyncLoadHistogramNeededItems.Insert`。
- `pkg/planner/core/optimizer_runtime.rs` 在同步统计加载失败且允许回退伪统计时，将 `StmtCtx` 中的待加载项转入全局集合，然后消费同步等待列表。
- `pkg/domain/domain.rs::load_needed_histograms` 调用 `AllItems`，索引总是完整加载，列按 `FullLoad` 选择载荷；`load_needed_column_histogram` 和 `load_needed_index_histogram` 在尝试结束后调用 `Delete`。

RustCodeGraph 对目标文件识别出 17 个符号，并定位了上述 Domain/Session 入口；其 `callers`/`callees` 对这种静态全局对象的方法调用没有给出精确边，因此生产引用又通过全仓 `AsyncLoadHistogramNeededItems` 精确搜索核验。Cargo 反向依赖还包括 Domain、Planner Core、statistics handle/storage 及若干测试 crate，但存在 manifest 依赖并不等于均有生产调用。

## 错误处理与边界

本文件的公开操作不返回 `Result`。所有 `RwLock` 获取都使用 `unwrap_or_else(|poisoned| poisoned.into_inner())`：线程 panic 导致锁中毒后仍继续访问其中数据，而不是再次 panic。这个选择保证队列可继续使用，但并不证明中毒前的复合操作已完整执行；若将来增加多步更新，应重新评估恢复后的数据不变量。

`Delete` 对不存在的键静默成功，重复删除无副作用。`getIdx` 使用 `i64::unsigned_abs`，包括 `i64::MIN` 在内都不会发生有符号取反溢出。`AllItems` 的容量初值是 128 而不是精确总项数，这只影响潜在扩容，不限制结果数量。

表、列或索引在入队后被删除，以及持久统计损坏等业务边界由 Domain 加载器处理，不由本集合识别。`async_load_test.rs` 的五个集成测试验证这些情况下加载入口仍返回且相关表项最终离队；这些是本队列与下游消费者共同形成的行为，不应误解为 `NeededStatsMap` 自身验证元数据或吞掉解析错误。

## 并发与资源生命周期

全局值由 `LazyLock` 安全地初始化一次，随后存活到进程结束。每个 `NeededStatsInternalMap` 有独立 `RwLock`：同分片的写操作串行，不同分片可并行；读操作允许同分片多个读者，但会与该分片写者互斥。`Insert` 在同一个写锁临界区内完成查找和按位或，因而并发升级不会丢失；`migration_concurrent_inserts_and_upgrades_are_not_lost` 用 8 个线程、每线程 100 项验证最终数量及全部升级状态。

外层聚合操作不同时锁住所有分片，从而避免长时间持有 128 把锁和固定锁序问题，但代价是弱一致快照。`AllItems` 返回拥有所有权的值，不携带锁守卫；锁在每个分片复制完成后立即释放。调用方处理快照期间，新请求可以继续进入，旧请求也可能已被其他线程删除。消费者因此必须把 `Delete` 视为幂等，并且不能把一次 `AllItems` 后的空结果当作永久静止状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/statistics/asyncload/async_load.go`。Rust 保留了 Go 的两层结构、128 分片、按 `abs(ID) % shardCnt` 路由、每分片读写锁、全量枚举、精确删除和长度汇总。Go 的 `Insert` 在旧值已经为 `true` 时提前返回，否则写入新值；Rust 的 `*current |= full_load` 与其真值表一致，表达得更紧凑。

主要实现差异是：Go 直接以 `model.TableItemID` 为 map 键，Rust 用私有 `TableItemKey` 获得 `Eq + Hash` 后在边界转换；Go 全局变量立即构造并持有指针，Rust 用 `LazyLock<NeededStatsMap>` 惰性构造；Go 对负 ID 手动取反，Rust 用不会在 `i64::MIN` 溢出的 `unsigned_abs`。这些差异不改变正常 ID 下的队列语义。

Go 集成测试 `pkg/statistics/asyncload/async_load_test.go` 的五个场景已在独立 Rust 集成测试 `async_load_test.rs` 中对应：表删除、列删除、索引所属表删除、索引删除、损坏桶数据。Rust 测试因本工作区加载职责由 Domain 持有而调用 `domain.load_needed_histograms()`，并显式驱逐分析后的缓存以复现 Go 的轻量缓存状态；这些适配在测试文件头部有说明。Rust 特有的结构级语义测试位于独立文件 `migration_aster_unit_test.rs`，没有把测试内嵌到生产源文件。

## 扩展指南

- 新增键字段时，必须同步修改 `TableItemKey`、`from_item`、`into_item`，并扩展 `migration_all_table_item_fields_participate_in_map_identity`；遗漏任一转换会造成错误去重或删除。
- 修改合并策略时，入口是 `NeededStatsInternalMap::Insert`。必须保留并发临界区的原子性，并同步验证重复插入、升级和并发测试；若允许降级，需要先审查 Domain/Planner 对“完整加载支配部分加载”的假设。
- 修改分片算法或数量时，入口是 `shardCnt` 与 `getIdx`。需评估热点 ID 分布、`i64::MIN`、正负 ID、内存和锁竞争；只要插入与删除共用同一算法，分片变化不应改变逻辑身份。
- 若要实现真正的 drain、批量领取、重试或状态机，不应仅把 `AllItems` 改名；需要定义快照与并发插入/删除的线性化语义，并同步调整 `Domain::load_needed_histograms` 的删除和失败策略。
- 若增加后台任务、容量限制或清理机制，应放在拥有调度生命周期的 Domain/统计处理层；本 crate 当前只是数据结构。新增测试仍应放在同目录独立测试文件：容器不变量放 `migration_aster_unit_test.rs`，跨 SQL/Domain 行为放 `async_load_test.rs`。
- 对公开 API 或依赖边界的改变要同步检查 `pkg/statistics/asyncload/lib.rs`、本 crate 的 `Cargo.toml` 以及 Domain/Planner 的 Cargo 依赖；不要假设所有 manifest 依赖都有直接调用。

## 验证依据

- 目标源码：`pkg/statistics/asyncload/async_load.rs`，核对了全部 179 行、17 个索引符号及公开/私有边界。
- crate 边界：`pkg/statistics/asyncload/{lib.rs,Cargo.toml}`；确认模块导出、生产依赖、集成测试声明和无 feature 条件。
- Go 对照：`pkg/statistics/asyncload/async_load.go`；逐项核对两层分片结构、锁、合并、删除、枚举、计数和路由算法。
- 测试证据：`pkg/statistics/asyncload/migration_aster_unit_test.rs` 覆盖空集合、完整键和值、单调升级、精确删除、正负 ID 碰撞和并发写；`async_load_test.rs` 与 `async_load_test.go` 覆盖五个 SQL/DDL/损坏统计场景。
- 生产调用证据：`pkg/domain/domain.rs::{load_needed_histograms,load_needed_column_histogram,load_needed_index_histogram,enqueue_async_load_items_for_physical_id}`、`pkg/session/runtime/control.rs::collect_predicate_columns_point`、`pkg/planner/core/optimizer_runtime.rs` 的同步失败回退分支。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/statistics/asyncload` 找到目标及 Go/Rust 测试；`node --file` 和 `query` 核对目标源码与符号。方法 `callers`/`callees` 未返回精确静态全局调用边，已用上述精确引用搜索补齐并在“依赖与调用关系”中披露限制。
- 结构检查要求：文档必须存在，且固定的 11 个二级标题各出现一次；本任务是纯文档分析，按计划不运行 Cargo。
