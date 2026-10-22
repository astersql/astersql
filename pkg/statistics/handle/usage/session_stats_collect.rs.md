# `pkg/statistics/handle/usage/session_stats_collect.rs`

## 文件定位

用 `collect_pending_stats_delta_table_ids` 为刷盘层选择待处理表 ID：空目标表示全部，非空目标只保留仍有 pending delta 的唯一 ID，最终始终升序输出。

## 核心职责

本文件承担四项职责，均为内存态聚合而非 KV 持久化：

1. 用 `TableDeltaMap` 按物理表 ID 累加行数变化 `delta`、修改次数 `count`、列大小 `col_size`，并保留该批变化最早的 `init_time`。
2. 用 `StatsUsage` 按 `TableItemId` 聚合列或索引的最近使用时刻，同一键只保留较晚时间。
3. 用 `SessionStatsItem` 隔离单个会话的本地变化；`SessionStatsList::sweep` 将所有会话本地状态原子取走后合并到全局汇总，并清除已删除会话。
4. 用 `collect_pending_stats_delta_table_ids` 为刷盘层选择待处理表 ID：空目标表示全部，非空目标只保留仍有 pending delta 的唯一 ID，最终始终升序输出。

这里没有 SQL、事务、schema 或存储实现，也没有后台任务调度。`ColStatsUsageEntry` 只是供相邻持久化层传递使用时间的扁平数据结构。

## 主要符号

- `TableItemId { table_id, id, is_index }`：列/索引使用量的可哈希键。`table_id` 标识表，`id` 标识列或索引，`is_index` 区分两类对象。
- `TableDelta { delta, count, col_size, init_time }`：单表累计变化。`Default` 产生全零字段和 `None` 时间；`Copy` 使持久化层能在失败路径重新放回同一个值。
- `TableDeltaMap`：`Mutex<HashMap<i64, TableDelta>>` 的线程安全包装。`update` 增量累加并在首次插入时设置当前时间；`merge` 累加三个数值字段并选取最早非空时间；`take` 通过 `mem::take` 交换出整个映射；`reset` 清空映射。
- `StatsUsage`：`Mutex<HashMap<TableItemId, SystemTime>>` 的线程安全包装。`merge`/`merge_raw` 对重复键取最大时间；`take` 交换出全部记录；`reset` 清空记录。`merge_raw` 额外断言输入不是索引，因为该入口对应谓词列采集。
- `SessionState`：私有会话状态，包含 `deleted`、`deltas` 和 `usage`。所有字段由同一个会话级互斥锁保护。
- `SessionStatsItem`：可克隆会话句柄，克隆共享同一个 `Arc<Mutex<SessionState>>`。`delete` 只设置删除标志；`update` 累加表变化；`update_column_usage` 合并最近使用时间；`clear_for_test` 重置本地测试状态。
- `SessionStatsList`：保存已注册的 `Vec<SessionStatsItem>` 以及全局 `TableDeltaMap`、`StatsUsage`。`new_item` 注册会话；`sweep` 汇总并清理；两个访问器暴露全局映射；`reset` 清除全部会话和汇总。
- `ColStatsUsageEntry { item, last_used_at }`：持久化边界使用的扁平条目。
- `collect_pending_stats_delta_table_ids(delta_map, targets)`：刷盘前的确定性 ID 选择器。

除 `SessionState` 外，上述类型或函数均为 `pub`；`SessionStatsItem::state`、`SessionStatsList` 的三个字段和各映射内部字段保持私有。文件没有 trait、模块级常量、宏或条件编译项。

## 执行流程

典型链路如下：

1. `StatsUsageImpl::new_session_stats_item` 调用 `SessionStatsList::new_item`。后者创建默认 `SessionStatsItem`，将共享状态的一个克隆放入列表，并把另一个克隆交给会话。
2. 会话发生 DML 时调用 `SessionStatsItem::update(id, delta, count)`；首次出现的表记录 `SystemTime::now()`，以后只累加 `delta` 与 `count`。谓词列被使用时调用 `update_column_usage(items, time)`，每个键保留较晚时间。
3. 会话结束时调用 `delete`。这一步不丢弃尚未汇总的数据，只把 `deleted` 置为 `true`。
4. 刷盘入口 `StatsUsageImpl::{dump_stats_delta_to_kv,dump_column_stats_usage_to_kv}` 调用 `SessionStatsList::sweep`。`sweep` 先锁住条目向量，再逐项锁住会话状态，以 `mem::take` 取走会话的 delta/usage，合并到全局映射；返回值 `!state.deleted` 决定是否保留该会话条目。因此已删除会话也是“先合并、后移除”。
5. delta 刷盘路径对 `table_delta().take()` 的结果调用 `collect_pending_stats_delta_table_ids`，按目标选择且排序；`predicate_column.rs` 再执行 schema、年龄和修改比例判断。未写入、表不存在或写入报错的值会被合并回 `TableDeltaMap`。
6. 列使用时间刷盘路径取走 `stats_usage()`，转成 `ColStatsUsageEntry` 并排序后交给 `UsageStore::save_column_usage`；失败时按“同键取较晚时间”的规则合并回去。

`take`/`mem::take` 的设计保证一次 sweep 后相同会话数据不会再次出现；`session_stats_collect_test.rs::canonical_session_sweep_merges_delta_usage_and_removes_deleted_session` 通过第二次 sweep 得到空映射验证了这一点。

## 数据与状态

状态分为两级：每个会话的 `SessionState` 和列表持有的全局 `table_delta`/`stats_usage`。会话层减少了不同 Session 更新时的锁竞争；只有 sweep 才把局部状态搬到全局层。

`TableDelta` 的合并不变量是：`delta`、`count`、`col_size` 分别相加，`init_time` 取两个非空值中更早者；一侧为空则保留另一侧。这样即使失败回并或重叠刷盘乱序，超时判断仍以最早未处理变化为准。`TableDeltaMap::update` 只接受正表 ID，并在第一次建立条目时初始化时间；`SessionStatsItem::update` 同样首次初始化时间，但当前没有显式的正 ID 断言。

`StatsUsage` 与 `SessionStatsItem::update_column_usage` 都执行 max-time 合并，旧时间不能覆盖新时间。`TableItemId::is_index` 是键的一部分，因此相同表/对象 ID 的列与索引在通用合并中是不同记录；但 `StatsUsage::merge_raw` 专用于谓词列并拒绝索引。

`collect_pending_stats_delta_table_ids` 的非空目标分支用本地 `HashSet` 去重，并过滤 `delta_map` 不存在的 ID；空目标直接读取映射全部键。两条分支最后都 `sort_unstable`，所以输出与哈希遍历顺序无关。

## 依赖与调用关系

RustCodeGraph 将本文件识别为 31 个符号，并报告它被 `pkg/statistics/handle/usage/predicate_column.rs`、同目录测试及其他模块文件引用。精确源码引用进一步确认主要边如下：

- 上游构造：`predicate_column.rs::StatsUsageImpl::new` 创建 `Arc<SessionStatsList>`；`StatsUsageImpl::new_session_stats_item` 调用 `SessionStatsList::new_item`。
- 上游触发：`StatsUsageImpl::dump_stats_delta_to_kv` 和 `dump_column_stats_usage_to_kv` 调用 `SessionStatsList::sweep`。
- 下游消费：delta 路径调用 `table_delta().take()` 与 `collect_pending_stats_delta_table_ids`，随后调用 `UsageStore::{stats_meta_count,update_delta}`；列路径调用 `stats_usage().take()`，构造 `ColStatsUsageEntry` 后调用 `UsageStore::save_column_usage`。
- 失败恢复：相邻持久化代码分别调用 `TableDeltaMap::merge` 和 `StatsUsage::merge`，将尚未成功持久化的数据放回。
- crate 出口：`usage/lib.rs` 再导出本文件所有公共 API；根 workspace 和 `pkg/statistics/handle/Cargo.toml`、`pkg/session/Cargo.toml` 声明了该 usage crate。

RustCodeGraph 的精确 `callers collect_pending_stats_delta_table_ids` 查询在本次分析中长时间无输出后被终止，因此调用边以其已索引文件关系、`node --file` 源码结果及上述直接引用搜索交叉确认；没有据此推断未见于源码的运行时调用。

## 错误处理与边界

本文件没有 `Result` 返回值和可恢复业务错误。边界失败表现为 panic：`TableDeltaMap::update` 对非正表 ID 执行 `assert!`；`StatsUsage::merge_raw` 对索引项执行 `assert!`；所有互斥锁均以带具体消息的 `expect` 处理 poisoned mutex。调用者若需要可恢复错误，必须在进入这些 API 前校验输入，或在本文件之外建立错误边界。

时间合并只比较 `SystemTime`，不进行时区格式化；持久化格式和时区属于 `UsageStore` 实现。`SystemTime::now()` 本身不返回错误。`collect_pending_stats_delta_table_ids` 对空映射、空目标、重复目标和不存在目标均安全返回；它不验证 ID 是否为正，因为其职责仅是从已有 pending map 中选择键。

`delete` 是延迟删除：若此后没有 sweep，条目及其中数据仍被列表持有。`reset` 则会直接清除所有会话条目和全局汇总，只适合测试或明确允许丢弃 pending 数据的生命周期边界。

## 并发与资源生命周期

`SessionStatsItem` 使用 `Arc<Mutex<SessionState>>`，所以句柄克隆可跨线程共享，所有会话更新、删除与 sweep 对同一会话串行化。`TableDeltaMap`、`StatsUsage` 和 `SessionStatsList::items` 各自有独立互斥锁；类型没有异步任务、通道、文件句柄或外部事务。

`SessionStatsList::sweep` 的锁顺序固定为 `items`，再到某个 `SessionState`，再在合并时短暂取得全局 map 锁。普通会话更新只取自己的 `SessionState`，不会反向获取 `items`，因此当前代码没有相反锁序。代价是整个 sweep 期间持有列表锁，并逐项获取会话锁；新会话注册会等待 sweep，长列表的暂停时间与会话数和 pending 键数相关。

`take` 通过交换空 `HashMap` 缩短锁内工作，调用者在锁外处理所有权已转移的数据；后续并发更新进入新的空映射，不会被本次刷盘误删。失败时 `merge` 把旧批次与期间的新变化相加或取较晚使用时间。已删除会话只有在 sweep 完成其最后一次搬运后才从向量移除；外部若仍持有克隆，仍可访问共享状态，因此调用方必须把 `delete` 当作“不应继续写入”的生命周期契约，而不是强制失效机制。

## 与 Go 版本的对应关系

实现结构存在已验证差异。Go 用带哨兵头节点的单链表，并在 sweep 中采用相邻节点锁接力，最多同时持有两个会话锁；Rust 用 `Mutex<Vec<SessionStatsItem>>`，sweep 持有向量锁并逐会话锁定。Go 的同一文件还包含刷盘阈值、SQL/KV 写入、批处理和指标逻辑；Rust 将这些职责拆到相邻 `predicate_column.rs` 的抽象存储实现中，因此不能把 Go 文件其余能力视为本文件已经实现。Go `TableDeltaMap::Update` 不设置 `InitTime`，而当前 Rust 的两个 update 路径会在首次写入时设置；这与 Rust `predicate_column.rs` 的年龄判断配套。

## 扩展指南

- 新增表 delta 字段时，应同时修改 `TableDelta`、`TableDeltaMap::merge`、会话 update 入口和持久化边界，明确该字段是求和、取最值还是覆盖；同步扩展 `session_stats_collect_test.rs` 的 merge 测试，并核对 Go `variable.TableDelta::MergeFrom`。
- 改变会话生命周期时，优先修改 `SessionStatsItem::{delete,clear_for_test}` 与 `SessionStatsList::{new_item,sweep,reset}`；必须保留“删除会话先搬运最后数据再移除”和“重复 sweep 不重复统计”的回归测试。
- 扩展列/索引使用量时，先决定是否应走通用 `update_column_usage` 还是只允许列的 `StatsUsage::merge_raw`；若允许索引进入后者，需要同时评估并更新 Go 侧断言，而不是仅删除 Rust 断言。
- 修改目标表选择策略时，集中在 `collect_pending_stats_delta_table_ids`，保持确定性排序，并在独立的 `session_stats_collect_test.rs` 增加空输入、重复、不存在键和顺序用例。
- 改变锁布局或为 sweep 优化并发时，应验证固定锁序、注册与 sweep 的竞争、delete 与最后一次 update 的竞争，以及 take 后并发新写入不会丢失。测试仍应放在同目录独立测试文件，不能内嵌进生产源文件。
- 涉及刷盘条件、错误回并、schema 或存储格式的需求，应修改 `predicate_column.rs` 及 `predicate_column_test.rs`，而不是把这些职责塞入本收集文件。需要与 Go 对齐时，还应检查 `session_stats_collect.go` 和 `session_stats_collect_test.go` 的相应语义。
