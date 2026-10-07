# `pkg/infoschema/sieve.rs`

## 文件定位

本文件实现 `astersql-infoschema` crate 内的通用、线程安全 SIEVE 缓存。模块由 [`lib.rs`](lib.rs) 以 `pub mod sieve` 装配，并重导出 `Sieve`、`SieveStatusHook`、`EmptySieveStatusHook` 和 `newSieve`。直接生产使用方是 [`infoschema_v2.rs`](infoschema_v2.rs)：`Data::table_cache` 以 `(table_id, schema_version)` 为键缓存 `Table`，服务于 InfoSchema V2 按名称或 ID 读取表元数据的路径。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，库入口为 `lib.rs`。本实现自身只使用 Rust 标准库；它通过公开钩子 trait 与 [`metrics.rs`](metrics.rs) 的 Prometheus 指标实现衔接。

## 核心职责

- `Sieve<K, V>` 在字节容量约束下保存键值，并以 SIEVE 的 `visited` 位和扫描指针选择淘汰对象。命中不会重排插入顺序，只给予条目一次“第二次机会”。
- `Mutex<State<K, V>>` 串行化容量、映射、顺序队列、扫描指针和计数的修改，使一个缓存实例可跨线程共享。
- `SieveStatusHook` 把命中、未命中、淘汰、用量变化和容量变化暴露给指标层；默认 `EmptySieveStatusHook` 为空操作。
- `SetCapacityAndWaitEvict` 提供同步缩容语义；`Set` 则保留 Go 版“插入前检查”的行为，因此新条目可以让总大小暂时超过容量，直到后续写入或显式同步缩容。

该文件不负责加载表元数据，也不决定缓存键；这些职责在 `infoschema_v2.rs`。它只提供缓存算法和观测接口。

## 主要符号

- `SieveStatusHook: Send + Sync`：公开回调协议。五个方法都有默认空实现，具体实现可以只覆盖关心的事件。
- `EmptySieveStatusHook`：构造时安装的默认钩子，保证无指标绑定时不需要空值分支。
- `Entry<V>`：内部条目，保存 `value`、`visited` 和创建时计算的 `size`。
- `State<K, V>`：锁内状态，包括 `count`、`size`、`capacity`、`HashMap<K, Entry<V>>`、以队头表示最新插入的 `VecDeque<K>`，以及可选的 `hand`。
- `Sieve<K, V>`：公开缓存类型。键要求 `Eq + Hash + Clone`，值要求 `Clone`；克隆是返回拥有值、保存顺序键和移动扫描指针所必需的。
- `new` / `newSieve`：分别为 Rust 风格和 Go 风格构造入口；后者只是前者的别名。
- `SetStatusHook`、`SetCapacity`、`SetCapacityAndWaitEvict`、`Capacity`：配置与容量管理 API。
- `Set`、`Get`、`Remove`、`Contains`、`Peek`：数据访问 API。`Get` 会置 `visited` 并记 hit/miss；`Contains` 与 `Peek` 不改变访问热度也不记命中指标。
- `Size`、`Len`、`Purge`、`Close`、`entry_size`：状态查询、清理及测试容量计算 API。
- `previous_key`、`remove_entry`、`evict`：私有顺序导航、统一删除和淘汰核心。

## 执行流程

1. `Sieve::new` 建立空 `HashMap`/`VecDeque`，计数与大小归零，`hand` 为空，并安装空钩子。
2. `Set` 先克隆当前钩子，再锁住状态。已有键只更新值并将 `visited` 置为 `true`，不改变顺序、大小或计数。
3. 新键写入前，`Set` 在当前 `size > capacity` 时最多调用 `evict` 十次；随后计算固定条目大小、把键压入队头并插入映射，最后调用 `on_update`。由于判断发生在插入前，插入本身可以造成超限。
4. `Get` 命中时把条目标为已访问、调用 `on_hit` 并克隆返回值；缺失时调用 `on_miss`。`Peek` 只克隆返回，`Contains` 只检查映射。
5. `evict` 从既有 `hand` 开始；若为空则从队尾（最旧插入）开始。遇到 `visited` 条目就清位并向更旧方向移动，越过队尾时绕回。首个未访问条目被删除，`hand` 保存其前驱，再调用 `on_evict`。
6. `Remove` 在目标恰为 `hand` 时先把指针移到前驱，再经 `remove_entry` 同步映射、队列、大小、计数和 `on_update`。
7. `SetCapacityAndWaitEvict` 先更新容量并记 `on_update_limit`，然后持有状态锁反复按每批最多十次淘汰，直到用量不超限或缓存为空。
8. `Purge` 对快照出的全部键逐项执行统一删除，再清空顺序队列和 `hand`；`Close` 仅调用 `Purge`，后续仍可再次 `Set`。

在应用主链中，`infoschema_v2::Data::add` 会把新表写入缓存；`infoschemaV2::TableByName` 和 `TableByID` 先 `Get`，未命中时从版本化元数据记录取表并 `Set`；`TableIsCached` 使用 `Contains`，`EvictTable` 使用 `Remove`，`Data::SetCacheCapacity` 使用同步缩容入口。

## 数据与状态

核心不变量是 `items` 与 `order` 对同一组键保持一一对应，`count` 表示条目数，`size` 等于各 `Entry::size` 之和。所有改变这些字段的路径都在 `state` 锁内，删除统一经过 `remove_entry`；减法使用 `saturating_sub`，避免计数或大小在异常状态下下溢。

`order` 的队头是最新插入键、队尾是最旧插入键。更新已有键和读取命中均不移动键，热度只体现在 `visited`。`hand` 不是一个独立节点，而是一个克隆键；删除 hand 指向的键时必须先推进它，以免后续扫描引用已移除条目。

容量和大小的单位都是字节，但 Rust 当前的 `entry_size` 是 `size_of::<K>() + size_of::<V>() + size_of::<Entry<V>>()`，属于编译期固定的浅层估算，不计算 `String`、`Vec`、`Arc` 等堆上内容。它也可能重复计入 `V`（`Entry<V>` 本身已包含 `V`）；因此该值应视为当前移植约定，而不是精确驻留内存。

## 依赖与调用关系

上游关系：

- `pkg/infoschema/lib.rs` 声明并重导出本模块的公开类型。
- `pkg/infoschema/infoschema_v2.rs` 的 `Data::new` 以 1 GiB 默认容量构造 `table_cache`；`Data::add`、`infoschemaV2::TableByName`、`TableByID` 写入或读取它，容量配置、缓存判断和主动淘汰也经该类型完成。
- `pkg/infoschema/metrics.rs` 实现 `SieveStatusHook`，把五类事件映射到 Prometheus counter/gauge。

下游关系仅包括标准库的 `HashMap`、`VecDeque`、`Arc`、`Mutex`、`Hash` 和 `size_of`。本文件不依赖异步运行时、后台任务、I/O 或 Cargo feature；`pkg/infoschema/Cargo.toml` 中的 `prometheus` 依赖由指标模块使用，而不是由 `sieve.rs` 直接使用。

RustCodeGraph 的索引查询识别了 `pkg/infoschema/sieve.rs` 中的 `Sieve`、`SieveStatusHook`、`newSieve`，以及 Go 对应符号和 `metrics.rs` 的钩子实现。精确的 Rust 生产调用位置由 `rg` 核实为 `infoschema_v2.rs` 中的构造、容量调整、`Set`、`Get`、`Contains` 和 `Remove`。

## 错误处理与边界

- 公开 API 不返回内部错误。任一 `Mutex` 被污染时使用 `expect` 立即 panic，错误文本区分状态锁与钩子锁。
- `evict` 若扫描键不存在于映射，或已访问扫描期间队列意外为空，会 panic。这些分支用于暴露 `items`/`order` 不变量被破坏，而不是静默恢复。
- `Remove` 对缺失键返回 `false`；`Get`/`Peek` 对缺失键返回 `None`；空缓存上的淘汰直接返回。
- 容量可以为零。新键仍会在写入前的检查后插入，因而可能短暂存在；下一次 `Set` 或 `SetCapacityAndWaitEvict(0)` 才会驱逐。
- 单次 `Set` 最多淘汰十项，不能保证返回时一定低于容量。同步缩容会重复批次直到满足容量，但其耗时与需要扫描/删除的条目数成正比。
- 钩子由调用方提供。回调在状态锁持有期间执行；钩子实现若阻塞会延长全部缓存操作，若回调重入同一个 `Sieve` 则会死锁，因此实现必须短小且不可重入。

## 并发与资源生命周期

`Sieve` 可安全放入 `Arc` 供多线程共享：`SieveStatusHook` 强制 `Send + Sync`，缓存的可变状态和当前钩子分别由两个 `Mutex` 保护。大多数数据操作先短暂锁住 `hook` 并克隆 `Arc`，释放钩子锁后再锁状态，因此普通路径不会同时持有两把锁；`SetCapacity` 则先完成状态锁更新，再获取钩子锁通知容量变化。

所有 `Get` 都需要写锁，因为命中会修改 `visited`；`Contains`、`Peek`、`Size`、`Len` 和 `Capacity` 也使用同一个互斥锁，读流量不会并行。值在锁内克隆后返回，不把内部引用泄露到锁外。

本实现没有后台线程、异步任务或通道。`Close` 不改变永久状态、不设置关闭标志，也不取消任务；它与 `Purge` 等价。钩子 `Arc` 的替换和克隆保证正在进行的操作可以继续使用旧钩子，新操作使用新钩子。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/sieve.go`，测试对照为 `sieve_test.go` 与 `sieve_test.rs`。Rust 保留了 Go 的核心行为：映射加插入链、命中只置 `visited`、从尾部/hand 反向扫描、第二次机会清位、已有键更新不重排、插入前批量淘汰、同步缩容，以及删除/淘汰/容量变化的钩子通知。

已确认的差异如下：

- Go `entry.Size()` 使用 `internal.Sizeof` 并缓存测量值；Rust 使用固定浅层 `size_of` 公式。涉及堆分配值时，两端容量含义不完全等价。
- Go `Sieve` 保存 `context.Context`/`CancelFunc`，`Close` 会清空并 cancel；Rust 没有后台上下文，`Close` 只清空。Rust 独立测试 `test_set_after_close` 明确验证关闭后仍允许写入。
- Go `Get` 有 `skipGet` failpoint；Rust 没有对应 failpoint，因此不能通过同名注入路径强制未命中。
- Go 的钩子字段与缓存状态受同一把 `mu` 保护；Rust 使用独立钩子锁和状态锁，使替换钩子的同步方式不同，但事件仍在状态更新的临界区内触发。
- Go 条目直接持有链表元素指针；Rust 以 `VecDeque<K>` 和克隆键表达顺序及 hand，`remove_entry` 使用 `retain` 删除键，因此删除复杂度为线性而非链表常数时间。

这些差异是当前源码事实，不应在扩展时假定 Rust 已具备 Go 的深度计量、failpoint 或取消语义。

## 扩展指南

- 修改淘汰策略时，集中调整 `evict`、`previous_key` 与 `Set` 的预插入检查，并保持 `items`、`order`、`hand` 三者不变量；同步更新独立文件 `pkg/infoschema/sieve_test.rs`，不得把测试嵌回生产源码。
- 新增命中类型或指标时，扩展 `SieveStatusHook`，同时更新 `EmptySieveStatusHook` 的默认行为、`pkg/infoschema/metrics.rs` 的生产实现和 `pkg/infoschema/infoschemav2_cache_test.rs` 的 `CountingHook`。新增必需方法前要评估下游 trait 实现的兼容性。
- 若要使容量代表真实内存，应先定义跨类型、跨平台的计量契约，再替换 `entry_size`/`Set` 的计算；必须与 Go `internal.Sizeof` 行为、InfoSchema V2 容量配置和测试容量构造一起校准。
- 若要优化高并发读取，不能简单换成读写锁，因为 `Get` 会修改 `visited`；需要同时设计访问位的原子性、淘汰扫描与删除的同步关系。
- 若引入真正的关闭状态或后台资源，需明确 `Close` 的幂等性、关闭后 `Set/Get` 的行为，并同步 Go 对应语义及 `test_set_after_close`。
- 性能风险主要在 `VecDeque::retain` 的线性删除、值/键克隆以及单锁串行访问；正确性风险主要在 hand 环绕、删除 hand、零容量和钩子重入。

## 验证依据

- 生产源码：[`pkg/infoschema/sieve.rs`](sieve.rs)，逐项核对 trait、内部状态、全部公开方法与三个私有辅助函数。
- crate 与模块边界：`pkg/infoschema/Cargo.toml`、`pkg/infoschema/lib.rs`。
- 直接生产调用：`pkg/infoschema/infoschema_v2.rs` 的 `Data::new`、`Data::add`、`Data::SetCacheCapacity`、`Data::SetStatusHook`、`infoschemaV2::TableIsCached`、`EvictTable`、`TableByName`、`TableByID`。
- 指标实现：`pkg/infoschema/metrics.rs` 的 `sieveStatusHookImpl` 及其 `SieveStatusHook` 实现。
- Rust 独立测试：`pkg/infoschema/sieve_test.rs` 覆盖基本存取、缺失删除、热条目第二次机会、存在性、重复更新大小、清空和关闭后写入；`pkg/infoschema/infoschemav2_cache_test.rs` 通过生产 `Data` 验证 hit/miss/evict/update/update-limit 事件序列。
- Go 对照：`pkg/infoschema/sieve.go`、`pkg/infoschema/sieve_test.go`，并以 `pkg/infoschema/infoschema_v2.go` 的生产接线核对缓存用途。
- RustCodeGraph：`status` 显示索引覆盖本仓库；`query Sieve --kind struct`、`query newSieve --kind function`、`query SieveStatusHook --kind trait` 识别 Rust/Go 对应符号及指标实现。`explore` 与文件级 `node` 本次无输出，因此调用边改用精确 `rg` 和源码读取核验。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另执行任务规定的 11 章节结构检查并人工复核上述事实。
