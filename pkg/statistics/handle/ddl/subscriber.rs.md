# `pkg/statistics/handle/ddl/subscriber.rs` 逻辑说明

## 文件定位

`subscriber.rs` 属于独立 crate `astersql-statistics-handle-ddl`；`pkg/statistics/handle/ddl/Cargo.toml` 将 `lib.rs` 设为 crate 入口，`lib.rs` 通过 `pub mod subscriber` 和 `pub use subscriber::*` 导出本文件的公开类型与函数。根 `Cargo.toml` 又以 `facade_statistics_handle_ddl` 引入该 crate，`pkg/lib.rs` 将其纳入统一 facade。

本文件位于 DDL 事件与统计存储之间：它把已经转换为 `SchemaChangeEvent` 的表、列和分区变更分发为统计元数据写入、版本推进、全局行数增量或统计 ID 迁移。相邻的 `ddl.rs::DdlHandler` 持有 `Subscriber<B>`，从有界队列取到事件后调用 `Subscriber::handle`；该层若失败只调用 `StatsBackend::warn_ignored_event_error` 并返回成功，保留 Go 统计 DDL 更新的 best-effort 语义。

当前生产接线与算法能力需要分开理解：仓库搜索只发现 `pkg/statistics/handle/ddl/ddl_test.rs::RecordingBackend` 实现 `StatsBackend`，所有 `DdlHandler::new` 调用也都在该测试文件。也就是说，本文件当前是已导出且有独立单元测试覆盖的抽象移植，但尚无真实 Rust 统计存储后端把它接入应用 DDL notifier 主链。

## 核心职责

- 定义 Rust 侧统计 DDL 领域模型：`SchemaChangeEvent`、`TableInfo`、`PartitionInfo`、`PartitionDefinition`、`ColumnInfo` 和 `MiniTableInfo`。
- 用 `StatsBackend` 隔离全局变量读取、统计缓存检查、持久化、锁表状态、时间戳、InfoSchema 名称查询与 best-effort 告警，使事件算法可由记录型测试后端验证。
- 在 `Subscriber::handle` 中按事件类型维护物理表统计：创建时插入伪统计，删除时只推进 `stats_meta` 版本以触发后续 GC，新增/修改列时插入列级伪统计。
- 维护分区表的全局统计：删/截断分区扣减全局行数，交换分区按普通表和原分区的 count/modify 差更新全局表，普通表与分区表互转时迁移全局统计 ID。
- 在历史统计开启、写入时间戳有效且缓存已初始化时记录历史统计元数据。
- 提供 `update_stats_with_count_delta_and_modify_count_delta` 和 `exchange_partition_log_fields` 两个公开辅助函数，分别封装锁表/非锁表增量写法和交换分区日志字段。

## 主要符号

- `Error(pub String)`：本 crate 的轻量错误包装，实现 `Display` 与标准 `Error`；后端错误、未知事件和交换分区缺少定义都沿此类型传播。
- `PartitionPruneMode::{Static, Dynamic}`：控制分区表物理 ID 集合。静态模式只处理分区 ID，动态模式还处理逻辑表 ID 对应的全局统计。
- `ColumnInfo`、`PartitionDefinition`、`PartitionInfo`、`TableInfo`、`MiniTableInfo`：从 Go 模型提炼出的最小数据结构；它们不包含完整 TiDB schema 元数据。
- `SchemaChangeEvent`：订阅者的输入协议，覆盖建表、截断、物化视图 cutover/元数据事件、删表、加列/改列、分区增删截断交换重组、分区化/去分区化、Flashback、加索引、删库和未知事件。
- `StatsBackend`：所有可观察副作用的边界。其方法分为配置/缓存查询、伪统计插入、版本与历史记录、统计读取、锁表与时间戳、绝对/增量写回、统计 ID 迁移、全量版本刷新、schema 名查询和告警。
- `Subscriber<B>`：只保存一个泛型后端；`new` 构造，`backend`/`backend_mut` 暴露测试和上层告警所需访问，`handle` 是事件主入口。
- `physical_ids`：把逻辑表转换为需要维护的物理 ID 列表；无分区表返回表 ID，分区表返回全部分区 ID，动态裁剪时末尾追加全局表 ID。
- `record_historical_stats_meta`：历史记录门控；`start_ts == 0`、历史统计关闭或缓存未初始化时均直接成功返回。
- `partition_count`、`update_global_stats_for_drop_partition`、`update_global_stats_for_truncate_partition`、`update_global_stats_for_exchange_partition`：分区事件的全局统计算法。
- `update_stats_with_count_delta_and_modify_count_delta`：锁表时写增量且允许结果为负；非锁表时读取当前值、叠加增量并把 count/modify_count 分别钳制到零以上。
- `SCHEMA_NOT_FOUND` 与 `exchange_partition_log_fields`：在 schema 查询失败时使用 `"Not Found"`，构造固定的交换分区诊断键值。

## 执行流程

`Subscriber::handle` 的主要分支如下：

1. `CreateTable` 通过 `physical_ids` 逐个调用 `insert_stats_for_physical_id`；`DropTable` 对同一集合逐个执行延迟删除。`TruncateTable` 与 `MViewRefreshOutOfPlaceCutover` 共用 `handle_truncate_like_event`，先初始化新表/新分区，再推进旧表/旧分区的统计版本。
2. `AddColumn` 为所有适用物理 ID 插入列伪统计；`ModifyColumn` 在 `analyzed == true` 时立即结束，否则执行相同初始化，避免重复覆盖 DDL 内已分析的结果。
3. `AddTablePartition` 只初始化新增分区。`ReorganizePartition` 初始化新增分区并延迟删除旧分区，不改全局统计，因为重组不改变总数据量且新分区行数难以即时拆分。
4. `TruncateTablePartition` 严格按“初始化新分区、从全局统计扣除旧分区行数、推进旧分区版本”执行。`DropTablePartition` 先扣全局统计，再推进被删分区版本。任一步失败都停止后续步骤，因此这里不是原子事务协调器。
5. `ExchangeTablePartition` 读取被交换分区和普通表的 `(count, modify_count)`，计算 `count_delta = table_count - partition_count`，以及 `modify_delta = table_count + partition_count - partition_modify + table_modify`；二者全为零时不写，否则更新全局表。
6. `AlterTablePartitioning` 初始化新分区后把旧单表统计 ID 迁移到新全局表 ID；`RemovePartitioning` 先把旧分区表全局统计迁移到新单表 ID，再延迟删除各旧分区统计。
7. `FlashbackCluster` 刷新全部统计版本。`AddIndex` 和五类只改物化视图元数据的事件明确为空操作。
8. `DropSchema` 对每张表先处理各分区、再处理表 ID；每次延迟删除的错误均被局部忽略，继续清理剩余对象。`Unknown` 则返回含 action 文本的错误。

伪统计写入、列统计写入和延迟删除最终都得到一个 `start_ts`，再统一进入历史记录门控。交换分区的通用写回先查询锁表集合和事务时间戳：锁表调用 `update_locked_delta`，非锁表读取现值并调用 `write_stats_meta`。

## 数据与状态

`Subscriber` 自身只有 `backend: B`，不缓存事件、表信息或事务状态；所有持久状态都由后端拥有。`&mut self` 和 `&mut StatsBackend` 方法把一次订阅处理限制为对同一后端的顺序可变访问。

统计对象以物理 ID 为键。普通表只有 `TableInfo::id`；分区表的每个 `PartitionDefinition::id` 都有独立统计，动态裁剪模式额外维护逻辑表 ID 的全局统计。`physical_ids` 保留输入分区顺序，并把全局 ID 放在末尾；代码没有去重或校验 ID 合法性，调用方必须提供规范化 schema 数据。

删除不是立即移除统计行：`delayed_delete_stats_for_physical_id` 调用 `update_stats_meta_version`，让后续统计 GC 依据版本清理。历史记录只在三个条件同时满足时写入：非零 `start_ts`、`historical_stats_enabled()` 为真、`cache_initialized(id)` 为真。这避免为无实际写入或尚无可用缓存快照的对象生成历史元数据。

`partition_count` 将缺失的统计行视为 `(0, 0)` 中的 count 0，并累加所有目标分区。删/截断分区若总 count 为 0，会跳过锁表查询、时间戳和全局写入。交换分区也把缺失的分区或普通表统计视为零，但要求 `PartitionInfo::definitions` 至少有一个元素，且只读取第一个定义。

非锁表增量写回使用 `(current + delta).max(0)` 防止绝对 count 和 modify_count 变负；锁表路径把原始增量交给后端，允许锁表 delta 记录为负。该差异是存储协议的一部分，扩展后端不能把两条路径合并成同一种写法。

## 依赖与调用关系

上游直接调用链是 `ddl.rs::DdlHandler::handle_ddl_event -> Subscriber::handle`。`DdlHandler::new` 用后端构造订阅者，`subscriber`/`subscriber_mut` 提供访问；其中可变访问用于在 `handle` 出错后调用 `warn_ignored_event_error`。`ddl.rs::update_stats_with_count_delta_and_modify_count_delta_for_test` 还直接转发到本文件同名公开辅助函数。

下游调用全部通过 `StatsBackend`：`handle` 及私有 helper 调用配置、存储、缓存、锁表、时间戳和迁移接口，本文件没有 SQL、事务或异步运行时依赖。源码层唯一标准库数据结构是 `HashSet`；`Cargo.toml` 没有声明直接依赖，说明当前 crate 的算法和领域模型完全由标准库及本地 trait 组成。

RustCodeGraph 将 `Subscriber::handle` 的下游边定位到 `physical_ids`、三个伪统计/删除 helper、三个全局分区更新 helper，以及后端的 `change_global_stats_id`、`update_all_stats_versions`。对 `update_stats_with_count_delta_and_modify_count_delta`，图确认其调用 `locked_tables`、`start_ts`、`stats_meta`、`update_locked_delta` 和 `write_stats_meta`。由于常见名称 `handle` 的图查询存在跨语言歧义，上游关系同时由 `ddl.rs` 的直接调用和仓库引用搜索核验。

crate 虽经根 facade 导出，但仓库中没有生产 `StatsBackend` 实现；因此不能把 Go 生产链的 notifier、session context、InfoSchema、storage、lockstats 和 history 依赖描述为当前 Rust 的真实运行依赖，它们只是被 trait 抽象出的待接线能力。

## 错误处理与边界

- `handle` 一般使用 `?` 首错返回，可能已完成此前物理 ID 的写入；本文件不开始事务、不回滚，也不保证跨 ID 原子性。生产后端必须决定单次调用的事务边界和重试幂等性。
- `DdlHandler::handle_ddl_event` 会吞掉 `handle` 错误并通过后端告警，因此上层返回成功不代表统计更新全部成功。这与 Go 的 best-effort 入口一致。
- `DropSchema` 更宽松：即使某个表或分区的版本推进失败也继续，并且当前 Rust 只丢弃错误，没有像 Go 实现那样逐项写错误日志；除非后端方法内部记录，否则这些错误不可见。
- `Unknown` 返回显式错误；与 Go 默认分支的测试断言加日志不同，Rust 不 panic。物化视图元数据事件和 `AddIndex` 是已识别的无操作，不会被视为未知。
- `ExchangeTablePartition` 对空 `definitions` 返回 `"exchange partition has no definition"`；Go 直接索引 `Definitions[0]`，依赖 notifier 保证非空。Rust 在这一输入上更防御。
- `stats_meta` 返回 `None` 时按零处理，可能掩盖“统计尚未加载”和“真实计数为零”的区别，这是与 Go `StatsMetaCountAndModifyCount` 默认结果相近的降级策略。
- `exchange_partition_log_fields` 只是纯字段构造器，本文件的更新流程当前没有调用它；真实日志输出也未接线。schema 名找不到时字段值固定为 `SCHEMA_NOT_FOUND`。
- 系统表过滤不在订阅者内。`ddl_test.rs::TestSystemTableDDLHasNoEvent_is_caller_policy` 明确要求调用方不要入队系统表事件；Go 的 `ddl_test.go::TestSystemTableDDLHasNoEvent` 验证生产 notifier 入队前过滤。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或事务。`Subscriber::handle(&mut self, ...)` 和后端大多数 `&mut self` 方法使同一实例上的处理天然串行；若未来在多个任务间共享，需要由外层提供互斥，并确认后端事务上下文不能交叉。

事件队列生命周期属于相邻的 `DdlHandler`：它使用容量 1000 的 `VecDeque`，但从队列取出事件与调用 `handle_ddl_event` 是两个独立动作。本文件只借用事件，不保存其引用；处理结束后事件可立即释放。

每次统计写入返回的 `start_ts` 只在当前调用栈内用于历史元数据，之后不缓存。全局增量路径单独获取 `start_ts`，但原子性完全依赖 `StatsBackend` 的实现。延迟删除也只推进版本，真实统计行的资源回收由后续 GC 负责。

循环均为串行、首错停止，唯一例外是 `DropSchema` 的逐项 best-effort。若后端持有数据库连接、事务或缓存句柄，应在一次 `DdlHandler::handle_ddl_event` 外层管理其生命周期；`Subscriber` 没有 `Drop` 实现或显式 close 钩子。

## 与 Go 版本的对应关系

Rust `Subscriber::handle` 对应 `subscriber.go::subscriber.handle`；`handle_truncate_like_event`、`insert_stats_for_physical_id`、`record_historical_stats_meta`、`delayed_delete_stats_for_physical_id`、`insert_stats_for_columns`、`physical_ids` 分别对应 Go 的 `handleTruncateLikeEvent`、`insertStats4PhysicalID`、`recordHistoricalStatsMeta`、`delayedDeleteStats4PhysicalID`、`insertStats4Col`、`getPhysicalIDs`。三个全局更新 helper 对应 Go 的 `updateGlobalTableStats4DropPartition`、`updateGlobalTableStats4TruncatePartition` 和 `updateGlobalTableStats4ExchangePartition`。

已保持的关键语义包括：动态裁剪追加全局表 ID；改列已分析时跳过初始化；截断类事件先建新统计再延迟删除旧统计；重组分区不改全局行数；删/截断分区按被删 count 给全局表施加负 delta；截断不单独扣 modify_count；交换分区的两条 delta 公式；锁表走允许负值的增量 upsert、非锁表结果钳零；普通表/分区表互转迁移统计 ID；历史记录的 start_ts、开关和缓存初始化三重门控；DropSchema 尽量清理所有对象；物化视图纯元数据事件为空操作。

当前 Rust 是结构化摘要模型，而非 Go 生产实现的一比一依赖移植：

- Go 直接接收 notifier 事件并从 `model.TableInfo`、session 全局变量和 InfoSchema 取值；Rust 要求上游先构造自有 `SchemaChangeEvent` 和精简模型。
- Go 直接调用 storage、history、lockstats、statistics cache 和日志包；Rust 将这些行为全部放进 `StatsBackend`，且尚无生产实现。
- Go 的更新方法携带 `context.Context` 和 `sessionctx.Context`，可在同一内部事务中运行；Rust 接口没有 context、取消、超时或显式事务协议。
- Go 的交换/截断分区路径记录成功和失败日志；Rust 仅提供交换日志字段构造器，没有调用日志设施，也没有对应的截断日志字段 helper。
- Go 默认未知事件执行测试断言并写日志后返回 nil；Rust `Unknown(String)` 返回错误，再由 `DdlHandler` 统一 best-effort 告警。
- Go 交换分区假设至少一个定义并直接取下标；Rust 对空定义返回错误。Go 的系统表过滤在生产调用方完成，Rust 测试只记录了同一契约，尚无生产调用方证明。

## 扩展指南

接入真实 Rust 生产链时，首要工作是实现 `StatsBackend`，并在 notifier 到 `SchemaChangeEvent` 的转换层完整映射表、列、分区和 `analyzed` 信息。后端必须明确每个方法是否共用同一内部事务，保证版本推进与历史记录的 start_ts 对应同次写入，并维持锁表增量与非锁表绝对写回的差异。接线测试应独立放置，不能把测试写入 `subscriber.rs`。

新增 DDL 类型时，应同时修改 `SchemaChangeEvent` 和 `Subscriber::handle`，判断它属于伪统计初始化、延迟删除、全局 delta、ID 迁移还是明确无操作；随后在 `pkg/statistics/handle/ddl/ddl_test.rs` 增加独立回归，并核对 Go `subscriber.go` 与 `ddl_test.go`。未知事件不应静默降级成无操作。

改变分区算法时，重点保护以下不变量：截断分区的三步顺序、重组不改变全局总量、交换 delta 公式、空统计按零处理、零 delta 避免写入、锁表允许负 delta、非锁表绝对值不小于零。性能风险集中在 `partition_count` 对每个分区逐次读取统计；若改为批量接口，需要同步 `StatsBackend` 和记录型后端测试，且保持错误与缺失行语义。

历史统计扩展应集中在 `record_historical_stats_meta`，避免三个调用点产生不同门控。若需要补齐日志，应让更新路径实际使用 `exchange_partition_log_fields`，并增加截断分区字段 helper；需测试 schema 缺失时 `"Not Found"`、成功/失败字段一致性以及敏感信息边界。

建议补充的独立 Rust 回归包括：静态分区裁剪不含全局 ID、`ModifyColumn { analyzed: true }` 无写入、历史开关关闭/缓存未初始化/start_ts 为零、`RemovePartitioning`、`ReorganizePartition`、空交换分区、非锁表负增量钳零、零 delta 无写入、DropSchema 单项失败后继续，以及 `Unknown` 经 `DdlHandler` 告警但入口返回成功。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/statistics/handle/ddl` 确认目标目录 11 个源文件均已索引，`subscriber.rs` 含 72 个符号。
- RustCodeGraph `node --file pkg/statistics/handle/ddl/subscriber.rs --offset 1 --limit 800`：读取目标文件 569 行全貌；`query` 定位 `Subscriber`、`update_stats_with_count_delta_and_modify_count_delta` 和 `exchange_partition_log_fields`。
- RustCodeGraph 调用边：`Subscriber::handle` 的 callees 包含 `physical_ids`、插入/延迟删除 helper、三个全局统计 helper、`change_global_stats_id` 和 `update_all_stats_versions`；增量辅助函数的 callees 包含 `locked_tables`、`start_ts`、`stats_meta`、`update_locked_delta`、`write_stats_meta`。常见名称 `handle` 的 callers/callees 查询出现跨语言歧义，因此上游边由 `ddl.rs` 和仓库引用搜索补证，未把空 callers 输出当作无调用者证据。
- crate 与入口：`pkg/statistics/handle/ddl/Cargo.toml`、`pkg/statistics/handle/ddl/lib.rs`、`pkg/statistics/handle/ddl/ddl.rs`、根 `Cargo.toml`、`pkg/lib.rs`；这些文件证明 crate 边界、facade 导出、队列入口和 best-effort 错误处理。
- Go 对照：`pkg/statistics/handle/ddl/subscriber.go` 与 `pkg/statistics/handle/ddl/ddl.go`；核对事件分支、物理 ID、历史记录、三类分区全局更新、锁表写入和日志字段。
- 独立测试：`pkg/statistics/handle/ddl/ddl_test.rs` 的 `RecordingBackend` 及建表、动态裁剪、截断、物化视图、删/截断/交换分区、DropSchema、加列、分区化、Flashback、锁表增量、队列容量和系统表调用方契约测试；Go `pkg/statistics/handle/ddl/ddl_test.go` 提供生产 SQL/notifier 行为对照。
- 接线搜索：全仓 `StatsBackend for` 只命中 `ddl_test.rs::RecordingBackend`，`DdlHandler::new` 也只在该测试文件使用；因此本文将生产接线标为尚未实现，而非推断已运行。
- 本任务是纯文档分析，按要求未运行 Cargo。交付前运行任务指定的结构命令，验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核没有把 trait 预期能力误写为当前生产接线。
