# `pkg/bindinfo/binding_cache.rs`

## 文件定位

本文件属于 `astersql-bindinfo` crate；`pkg/bindinfo/lib.rs` 以私有模块 `binding_cache` 装配它，并通过 `pub use binding_cache::*` 把公开接口重新导出。它位于绑定持久化与 SQL 匹配路径之间：全局路径由 `binding_handle.rs::NewBindingHandle` 创建 `BindingCacheUpdater`，写操作由 `binding_operator.rs` 触发重新加载；会话路径由 `session_handle.rs::NewSessionBindingHandle` 直接创建不设实际上限的 `BindingCache`。

`pkg/bindinfo/Cargo.toml` 声明该 crate 的 Go 对照包为 `pkg/bindinfo`。缓存文件自身只直接使用标准库集合与同步原语；SQL 解析、摘要和绑定结构由同 crate 的 `binding.rs` 提供，持久化边界和使用信息回写由 `utils.rs` 提供。

## 核心职责

1. 定义 `BindingCache`，提供按完整 SQL digest 精确访问、按 no-DB digest 跨库匹配、容量调整、占用统计和关闭清理。
2. 用 `bindingCache` 实现一个由 `HashMap`、插入顺序队列和双向摘要索引组成的内存缓存；容量超限时按插入顺序淘汰，而不是按读取热度更新的严格 LRU。
3. 用 `digestBiMap`/`digestBiMapImpl` 维护 `noDBDigest -> 多个 sqlDigest` 与 `sqlDigest -> 一个 noDBDigest` 的一致映射，为跨库候选检索服务。
4. 定义 `BindingCacheUpdater`，并由 `bindingCacheUpdater` 把持久化存储的全量或增量记录合并进缓存、推进更新时间水位线、回写绑定使用信息。

该文件不负责创建、删除或校验业务绑定，也不直接执行优化器匹配主链；这些职责分别位于 `binding_operator.rs`、`binding.rs` 和调用缓存的会话/运行时模块。

## 主要符号

- `bindingCacheTestKey: &str`：测试上下文键的 Rust 对照符号；当前 Rust 缓存实现没有读取它，因而不具备 Go 的淘汰回调注入语义。
- `BindingCacheUpdater: BindingCache`：全局缓存的扩展接口。`LoadFromStorageToCache(fullLoad, fromRemote)` 负责加载，`UpdateBindingUsageInfoToStorage` 负责回写，`LastUpdateTime` 暴露增量水位线。
- `bindingCacheUpdater`：组合 `Arc<dyn BindingCache>`、`Arc<dyn BindingStore>` 与 `Mutex<BindingTime>`。字段 `memQuota: Mutex<i64>` 在当前文件中仅初始化、没有被读取或更新。
- `NewBindingCacheUpdater(store, max_cost)`：全局缓存构造入口；由 `binding_handle.rs::NewBindingHandle` 调用。
- `digestBiMap`、`DigestMaps`、`digestBiMapImpl`、`newDigestBiMap`：双向摘要索引接口、锁内状态、实现和构造器。`Add` 幂等并能迁移已经属于另一 no-DB digest 的 SQL digest；`Del` 同步删除正反两个方向。
- `BindingCache`：缓存公共抽象；要求实现 `Send + Sync`，所有读取返回共享的 `Arc<Binding>`。
- `CacheState`：一把 `RwLock` 下的主表 `bindings`、FIFO 队列 `insertion_order`、估算占用 `usage` 与容量 `capacity`。
- `bindingCache`、`newBindingCache(maxCost)`：默认缓存及构造器；负容量被钳制为 0。
- `bindingCache::evict_to_capacity`：循环弹出最早插入的 digest，扣减 `Binding::size().ceil()` 后清理双向索引，直到 `usage <= capacity`。

本文件没有条件编译项；独立单元测试由 `lib.rs` 中的 `#[cfg(test)] mod binding_cache_test` 接入，而不是内嵌在生产源文件中。

## 执行流程

全局缓存初始化链为 `binding_handle.rs::NewBindingHandle -> NewBindingCacheUpdater -> newBindingCache -> newDigestBiMap`。操作器完成持久化写入后，`binding_operator.rs::{CreateBinding, DropBinding, SetBindingStatus}` 调用 `LoadFromStorageToCache(false, false)` 刷新缓存。

加载流程如下：

1. `fullLoad == true` 时以 `BindingTime::default()` 为边界，否则读取 `lastUpdateTime`。
2. 调用 `BindingStore::read_bindings_since(boundary)`；读取失败立即返回，缓存和水位线保持现状。
3. 对每条记录先用其 `UpdateTime` 推进本批局部最大值，再按 `SQLDigest` 读取旧缓存。
4. `binding.rs::pickCachedBinding` 在旧值和新值中选择最大更新时间的非 `deleted` 记录；得到记录则 `SetBinding`，否则 `RemoveBinding`。
5. 全批成功后才把 `lastUpdateTime` 设为本批最大值。任一 `SetBinding` 解析失败会提前返回，因此此前条目的缓存修改可能已经生效，但水位线不会推进。

写入流程 `SetBinding` 先调用 `noDBDigestFromBinding` 解析 `BindSQL` 并计算去库名摘要。只有解析成功才取得写锁并修改状态：覆盖时先扣旧值占用、移除旧队列位置，再加新值占用并把 digest 放到队尾；随后更新摘要索引并同步淘汰。重复设置同一 digest 因而不增加条数或累计占用，同时会把该项视为最新插入项。

匹配流程 `MatchingBinding` 先从双向索引取得某 no-DB digest 的所有完整 digest，在同一个缓存读锁下收集仍存在的绑定，然后交给 `binding.rs::crossDBMatchBindings`。后者只考虑启用绑定，并按表名、库名/通配符规则选择候选。精确读取、全量枚举、删除、容量调整和关闭分别由其同名 trait 方法完成；`GetAllBindings` 为确定性输出按 `SQLDigest` 排序。

## 数据与状态

缓存有两个必须同步维护的状态面：`CacheState.bindings` 是权威条目集合，`digestMap` 是跨库检索索引。`SetBinding`、`RemoveBinding`、容量淘汰和 `Close` 都会同步更新两者。`insertion_order` 不允许同一 digest 重复；覆盖会先 `retain` 清除旧位置。`usage` 是各当前绑定 `size().ceil() as i64` 的和，并不包含哈希表、队列、`Arc` 或索引自身的内存开销。

容量始终非负。单个绑定大于容量时，它在写入后会立即被 FIFO 淘汰；容量缩小时 `SetMemCapacity` 同步淘汰到限额内。读取不会改变顺序，因此这里的“新旧”是最近写入顺序，不是最近访问顺序。

`Arc<Binding>` 允许缓存、调用者与存储同步逻辑共享绑定。`GetBinding`、`GetAllBindings` 和匹配结果均克隆 `Arc`，不是深拷贝；关闭或淘汰只移除缓存引用，外部已持有的 `Arc` 仍可存活。更新时间水位线单独由 `lastUpdateTime: Mutex<BindingTime>` 保存。

## 依赖与调用关系

上游直接关系由 RustCodeGraph 确认：

- `binding_handle.rs::NewBindingHandle` 调用 `NewBindingCacheUpdater`，并把结果同时交给全局 `BindingHandle` 和 `BindingOperator`。
- `session_handle.rs::NewSessionBindingHandle` 调用 `newBindingCache(i64::MAX)`；创建、删除、匹配、状态编码和关闭均通过 `BindingCache` 接口完成。
- `binding_operator.rs` 在创建、逻辑删除和状态变更持久化成功后调用更新器的增量加载方法。
- `binding_cache_test.rs` 直接调用 `newBindingCache`、`newDigestBiMap` 和各缓存方法验证局部不变量。

主要下游关系为：`BindingStore::read_bindings_since` 提供持久化记录；`pickCachedBinding` 处理版本/墓碑；`noDBDigestFromBinding` 解析绑定 SQL 并生成索引键；`crossDBMatchBindings` 完成候选语义匹配；`updateBindingUsageInfoToStorage` 按批次及写入节流规则调用 `BindingStore::save_usage`。标准库的 `HashMap`/`VecDeque` 保存数据，`Arc`/`Mutex`/`RwLock` 提供共享所有权与同步。

## 错误处理与边界

- 可恢复错误使用 crate 的 `Result<T, BindError>` 传播。存储读取错误、绑定 SQL 解析错误、使用信息回写错误都会返回给调用者。
- `SetBinding` 在任何状态修改前计算 no-DB digest，所以无效 `BindSQL` 不会污染主表、队列、占用或摘要索引；`binding_cache_test.rs::invalid_binding_sql_is_rejected_without_mutating_cache` 明确覆盖此边界。
- 锁中毒采用 `expect(...)`，会 panic，而不是转换为 `BindError`。摘要索引与缓存状态分别加锁；代码固定先持有缓存写锁再调用摘要索引写操作，当前文件中没有相反的嵌套顺序。
- `fromRemote` 在 Rust 实现中以 `_fromRemote` 接收但完全忽略；`memQuota` 也未参与动态配额同步。不得据此宣称已经具备 Go 的远端日志或运行时配额刷新行为。
- `pickCachedBinding` 当前仅取最大 `UpdateTime` 后遇到的首个非删除记录；虽然 Go 查询按 `update_time, create_time` 排序，Rust 函数没有用 `CreateTime` 显式打破同更新时间并列。
- 加载不是整体事务式缓存替换：中途失败可能留下部分已应用条目；水位线只在全批成功后推进，因此重试会重新读取并幂等合并这些记录。

## 并发与资源生命周期

`BindingCache`、`digestBiMap`、`BindingStore` 均要求 `Send + Sync`。主缓存状态由一个 `RwLock<CacheState>` 保护，使条目、顺序、占用和容量的单次更新保持一致；摘要双向表由另一把 `RwLock<DigestMaps>` 保护，使两个方向的映射在一次 `Add`/`Del` 内一致。更新水位线由独立 `Mutex` 保护，存储读取期间不持有该锁。

这意味着多个加载调用可以同时读取同一水位线、分别应用结果，最后各自写回水位线；本文件没有把完整加载流程串行化。缓存级操作本身是线程安全的，但跨多条记录的加载不提供快照原子性。会话级调用者另在 `session_handle.rs` 用 `operation_lock` 保证批量会话操作的原子性。

`Close` 同步清空主表、队列和占用，并遍历摘要索引逐项删除；它没有“已关闭”标志，关闭后仍可再次写入和使用。`bindingCacheUpdater`、`bindingCache` 以及返回条目均由 `Arc` 管理，不创建后台线程、通道或异步任务，离开最后一个引用时由 Rust 自动释放。锁中毒是唯一显式的异常生命周期边界。

## 与 Go 版本的对应关系

接口形状和主链与 `pkg/bindinfo/binding_cache.go` 对齐：两者都有更新器、摘要双向映射、缓存抽象、跨库匹配、存储加载与使用信息回写；Rust 的独立测试也复刻了 Go 的跨库索引、重复写入、容量淘汰等用例意图。

实现存在以下已验证差异：

- Go 底层使用 Ristretto，带异步缓冲、准 LRU/准入策略、淘汰/拒绝回调、指标和关闭标志；Rust 使用同步 `HashMap + VecDeque` FIFO，写入操作完成时结果已可见，无淘汰日志或指标副作用，`Close` 后可复用。
- Go 增量加载会读取全局动态配额、应用 10 秒时钟偏差容忍、记录日志与 Prometheus 指标；Rust直接以精确水位线调用抽象存储，`fromRemote` 与 `memQuota` 未发挥作用。
- Go 的使用信息回写检查全局开关并捕获 panic；Rust直接调用 `utils.rs::updateBindingUsageInfoToStorage`，没有本文件级开关或 panic 恢复。
- Go 的 Ristretto 淘汰可能让摘要映射暂时含有已经不存在的缓存键，并在匹配时跳过 miss；Rust 的同步淘汰立即清理索引，目标是不保留这类不一致窗口。
- Rust `digestBiMapImpl::Add` 在已有 SQL digest 改映射时会从旧 bucket 移除它，比 Go 当前只覆盖反向映射更严格地维护双向一致性；`All` 和 `GetAllBindings` 还额外排序以提供确定性结果。
- 构造参数也不同：Go 更新器接受会话池并从全局变量取配额，Rust接受 `BindingStore` 与显式容量；这是 Rust 持久化适配边界的设计，不应与 Go 的会话池实现混用。

因此该 Rust 文件保留了核心绑定缓存语义，但不是 Go 运行时、观测性和淘汰算法的逐项等价实现。

## 扩展指南

- 修改淘汰策略时应集中调整 `CacheState`、`SetBinding` 与 `evict_to_capacity`，并在独立的 `binding_cache_test.rs` 增加访问顺序、覆盖、超大单项、容量为零和缩容用例；若要声称 LRU，必须让读取更新热度并验证并发下顺序。
- 修改摘要索引时必须保持两个方向、主缓存和淘汰路径一致；重点扩展 `cross_db_digest_map_tracks_add_remove_and_deduplicates`，加入 digest 从一个 no-DB bucket 迁移到另一个 bucket 的断言。
- 修改加载水位线或多节点时间语义时，应同步审查 `BindingStore::read_bindings_since`、`pickCachedBinding` 和 `binding_operator.rs` 的刷新调用；补充更新器专用测试，覆盖墓碑、同时间戳、读失败、批次中途解析失败、并发加载与水位线不回退。
- 若补齐 Go 的动态配额、时钟容忍、指标、日志、使用信息开关或关闭语义，应明确新增适配接口，而不是复用当前未使用的 `memQuota` 字段制造隐式行为；同时核对 Go 测试 `TestBindCache`、`TestBindingCacheEvictLog` 的原始意图。
- 调整返回类型或可变性时要审查所有 `Arc<Binding>` 持有者。现有 API 返回共享对象，不能假设淘汰或关闭会使调用者手中的绑定立即失效。
- 测试继续放在同目录独立文件 `pkg/bindinfo/binding_cache_test.rs`，并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入，不要把测试逻辑放回生产源文件。

## 验证依据

- RustCodeGraph 状态：索引覆盖 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/bindinfo` 找到目标 Rust/Go 源与独立测试。
- RustCodeGraph 文件/符号证据：完整读取 `binding_cache.rs` 451 行；`explore` 确认 `NewBindingCacheUpdater <- NewBindingHandle`、`newBindingCache <- NewBindingCacheUpdater/NewSessionBindingHandle`，以及 `SetBinding`、`RemoveBinding`、`MatchingBinding`、`evict_to_capacity` 的内部调用关系。
- crate 与装配证据：`pkg/bindinfo/Cargo.toml`、`pkg/bindinfo/lib.rs`。
- 下游语义证据：`pkg/bindinfo/binding.rs::{noDBDigestFromBinding, crossDBMatchBindings, pickCachedBinding}`，`pkg/bindinfo/utils.rs::{BindingStore, updateBindingUsageInfoToStorage}`。
- 上游入口证据：`pkg/bindinfo/binding_handle.rs::NewBindingHandle`、`pkg/bindinfo/binding_operator.rs`、`pkg/bindinfo/session_handle.rs::NewSessionBindingHandle`。
- Rust 测试证据：`pkg/bindinfo/binding_cache_test.rs` 覆盖双向索引增删与去重、重复写入占用不增长、非法 SQL 原子拒绝、缩容 FIFO 淘汰和关闭清空。
- Go 对照证据：`pkg/bindinfo/binding_cache.go` 全文件及 `pkg/bindinfo/binding_cache_test.go::{TestCrossDBBindingCache, TestDuplicatedBinding, TestBindCache, TestBindingCacheEvictLog}`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行任务指定的 11 章节结构检查并人工复核上述事实。
