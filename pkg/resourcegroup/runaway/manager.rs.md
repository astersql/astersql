# `pkg/resourcegroup/runaway/manager.rs`

## 文件定位

`manager.rs` 是 `astersql-resourcegroup-runaway` crate 的状态中枢：它把单查询检查器需要的资源组目录、进程内 quarantine watch 列表、三类待持久化记录队列，以及对 `tidb_runaway_watch` / `tidb_runaway_watch_done` 的增量同步器集中在一个可克隆的 `Manager` 句柄中。crate 入口在 `pkg/resourcegroup/runaway/lib.rs` 中公开 `manager` 模块，并定义本文件使用的 `ResourceGroupCatalog`、`RestrictedSqlExecutor`、动作/匹配枚举和统一 `Error`。

当前 Rust 应用链路是“可用组件加局部接线”，不是 Go 后台管理器的完整等价物。`pkg/domain/domain.rs::bind_runaway_manager` 可发布一个 `Arc<Manager>`，`pkg/session/runtime/scan_adapter_runtime.rs::RunawayBeforeExecutor` 会取出它并调用定义于 `checker.rs` 的 `Manager::DeriveChecker`。但是 Go `manager.go` 中的 `RunawayRecordFlushLoop`、`RunawayWatchSyncLoop` 在本文件没有对应后台任务；Rust 的手工 ADD/DROP QUERY WATCH 入口在 `pkg/executor/internal/querywatch/query_watch.rs` 仍是注释化迁移草稿。因此本文件提供同步、排队、drain 和手工持久化 API，本身不会定时调用它们。

crate 清单 `pkg/resourcegroup/runaway/Cargo.toml` 指定 `lib.rs` 为入口，并用 `package.metadata.porting.go-package = "pkg/resourcegroup/runaway"` 声明 Go 对照目录。清单中的本地依赖和 dev-dependencies 当前都受 `cfg(windows)` 限制；本文件本身只直接使用标准库和同 crate 类型。

## 核心职责

1. `Manager::NewRunawayManager` 组装目录、受限 SQL 执行器、系统表目录、节点 ID、空 watch/计数/队列，以及一个 `Syncer`。
2. `markQuarantine`、`AddWatch`、`addWatchList`、`removeWatch` 维护以 `"资源组/匹配文本"` 为键的进程内 watch 列表，同时维护每个资源组的活跃 watch 原子计数。
3. `examineWatchList` 为 `Checker::BeforeExecutor` 提供热路径查询；`resourceGroup` 和活跃计数则供 `Checker` 校验切换目标、判断是否需要派生检查器。
4. `markRunaway` 和 `markQuarantine` 生成记录并写入有界内存队列；三个 `drain*Records` 把所有权交给外部刷盘驱动。达到容量或锁中毒时，热路径静默丢弃记录。
5. `UpdateNewAndDoneWatch` 通过 `Syncer` 扫描两个系统表，吸收新增 watch、移除已完成 watch，并用重叠时间窗初始化删除游标。
6. `AddRunawayWatch`、`RemoveRunawayWatch`、`RemoveRunawayResourceGroupWatch` 提供手工持久化 API；移除路径复用 `record.rs::handleRunawayWatchDone`，把 watch 行转入 done 表。
7. `Stop`/`stopped` 保存生命周期标志，`checkpointGaugeValue` 和 `flushThreshold` 暴露与 Go 指标、刷盘阈值对齐的辅助值。

## 主要符号

- `ManualSource = "manual"`：系统表中的手工来源标志。`AddWatch` 看到它时启用强制替换；相同 ID 仍被视为重复扫描而保留原对象。
- `MaxWaitDurationMillis = 30_000`：与 Go `MaxWaitDuration` 的 30 秒数值对齐，供 crate 外资源控制配置使用；本文件内部不读取它。
- `MAX_WATCH_LIST_CAP = 10_000`：仅在插入全新 key 时限制 watch map；替换既有 key 不受此限制。
- `MAX_WATCH_RECORD_CHANNEL_SIZE = 1024`：三类 `Vec` 队列的独立容量。`flushThreshold()` 返回其一半，即 512。
- `WatchEntry { record, expires_at }`：缓存一条 `QuarantineRecord` 及其过期微秒时间；过期不是后台定时驱逐，而是在同 key 再次加入时检查。
- `ManagerInner`：实际共享状态，含 `CatalogRef`、`ExecutorRef`、`server_id`、四个 `Mutex` 状态域、`Syncer` 和 `AtomicBool`。
- `Manager(Arc<ManagerInner>)`：廉价克隆句柄；所有 clone 观察同一 watch、队列、同步游标和停止标志。
- `loadOrStoreActiveCounter` / `getActiveWatchCount`：按资源组惰性创建 `Arc<AtomicI64>` 并读取计数。前者会报告是否已有条目，后者在 map 锁中毒或缺失时返回 0。
- `queue<T>`：内部通用有界入队函数；只有成功取得锁且长度小于 1024 才 push。
- `markQuarantine` / `markRunaway`：分别构造 quarantine 与 runaway 记录。前者立即加入本地 watch，再排队持久化；后者从 `Checker` 快照 SQL、digest、动作和原因。
- `addWatchList` / `AddWatch` / `removeWatch`：实现重复、手工覆盖、过期、冲突和 done 删除规则。
- `GetWatchList` / `examineWatchList`：分别返回全量克隆快照和单 key 动作结果。
- `drainRunawayRecords` / `drainQuarantineRecords` / `drainStaleRecords`：用 `mem::take` 原子地取走当前批次并留下空 `Vec`。
- `UpdateNewAndDoneWatch`：串行驱动新增表和 done 表扫描。
- `AddRunawayWatch` / `RemoveRunawayWatch` / `RemoveRunawayResourceGroupWatch`：直接系统表管理接口。
- `Stop` / `stopped`：Release 写、Acquire 读停止标志；当前没有其他方法据此拒绝工作。
- `checkpointGaugeValue`：将微秒时间戳转为毫秒浮点，0 保持 0。

## 执行流程

查询执行主链如下：

1. 上层先通过 `Domain::bind_runaway_manager` 发布管理器；`RunawayBeforeExecutor` 在资源控制启用时从 Domain 取出它。
2. `Manager::DeriveChecker`（实现在 `checker.rs`）调用本文件的 `resourceGroup` 和 `getActiveWatchCount`。没有 plan digest，或既没有 runaway 设置又没有活跃 watch 时，不创建 `Checker`。
3. `Checker::BeforeExecutor` 依次用原始 SQL、SQL digest、plan digest 调用 `examineWatchList`。命中后按记录动作执行 Kill、CoolDown 或 SwitchGroup，并通过本文件的 `markRunaway` 排队审计记录。
4. 查询运行中第一次超过耗时、RU 或 processed-keys 阈值时，`Checker` 以 CAS 保证只标记一次，再调用 `markRunaway`；若规则包含 watch 且目录中的设置快照仍一致，还调用 `markQuarantine`。
5. `markQuarantine` 用 `saturating_add` 计算有限 TTL 的 `EndTime`，`addWatchList(..., false)` 使本节点立即生效，再把记录放入 quarantine 队列。落库前其 `ID` 为 0；之后同步到持久化行时，同 key、非零 ID 记录可替换它。

watch 同步流程如下：

1. 外部驱动调用 `UpdateNewAndDoneWatch`；函数持有 `syncer` mutex，更新 `last_sync_time`，先检查 watch 表是否存在。
2. `Syncer::getNewWatchRecords` 扫描 `[check_point, upper_bound)`，每条记录交给 `AddWatch`。已过期记录直接进入 stale 队列；`Source == ManualSource` 的记录走强制覆盖。
3. 首次 done 同步时，删除 reader 的 checkpoint 被设为新增 reader 上界减 `WATCH_SYNC_OVERLAP_MICROS`，避免扫描无意义的全部历史 done 行，同时覆盖两次快照之间的并发移动。
4. done 表存在时，`getNewWatchDoneRecords` 的每条记录交给 `removeWatch`；只有 key 和 ID 都匹配当前缓存项才删除，避免旧 done 记录误删较新的替代 watch。

手工管理流程中，`AddRunawayWatch` 生成单行 INSERT、调用执行器，并最多读取三次 `LastInsertId`；始终为 0 时返回 `Error::Storage`。按 ID 删除先要求查询结果恰好一条；按组删除逐条处理。两条删除路径都调用 `handleRunawayWatchDone`，其下游依次执行 begin、插入 done、删除 watch、commit。

## 数据与状态

`watch_list` 是 `HashMap<String, WatchEntry>`，键由 `QuarantineRecord::getRecordKey` 生成。这个键不包含 watch 类型或 ID，因此同一资源组与文本只有一个当前条目。`EndTime == 0` 表示不过期；有限 TTL 以绝对微秒时间保存。`GetWatchList` 的结果无稳定顺序，并且是记录克隆，不会暴露内部锁或可变引用。

`active_group` 的 map 值是独立 `AtomicI64`。新 key 插入 watch 时加一，缓存项被过期清理或 ID 匹配删除时减一；同 key 替换不会改变计数。map 不回收归零 counter，因此它是按曾出现过的资源组增长的辅助索引。`DeriveChecker` 只把计数是否非零作为“没有规则但仍需检查 watch”的快速门控。

三个记录队列分别保存：`runaway_records`（查询审计）、`quarantine_records`（待插入 watch）、`stale_records`（待清理的过期/冲突持久化 watch）。它们不是 channel，也没有内部消费者。每个队列最多 1024 条，`drain*` 返回调用瞬间已排队的完整批次；调用之后到达的新记录进入新 `Vec`。

`syncer` 包含新增和 done 两个 reader 的 checkpoint/upper-bound 状态，必须在同一把 mutex 下推进。`stopped` 只是一个跨线程可见标志；当前 `queue`、同步和存储 API 都不检查它，Drop 也不会自动 Stop。

## 依赖与调用关系

上游生产接线：

- `pkg/session/runtime/scan_adapter_runtime.rs::RunawayBeforeExecutor` 调用 `Manager::DeriveChecker`；后者位于 `checker.rs`，读取本文件的目录和活跃计数。
- `pkg/resourcegroup/runaway/checker.rs::BeforeExecutor` 调用 `examineWatchList`；`Checker::markQuarantine` 和内部 `markRunaway` 分别调用本文件同名方法。
- `pkg/domain/domain.rs` 保存/返回 `Arc<Manager>`；`pkg/domain/runaway.rs::ControllerResourceGroupCatalog` 把 TiKV controller 查询结果转换成该 crate 的 `ResourceGroup`。

当前未发现 Rust 生产调用的公开接口包括 `UpdateNewAndDoneWatch`、三个 `drain*`、手工 Add/Remove API 和 `Stop`；它们主要由相邻测试覆盖，或等待后台循环、SQL executor 的后续接线。`pkg/executor/internal/querywatch/query_watch.rs` 中对应 Add/Drop 调用仍被注释，不能视为已运行路径。

主要下游：

- `checker::Checker` 提供 `markRunaway` 所需的 SQL、digest、plan digest 与资源组字段。
- `record::QuarantineRecord` 提供 key、切换组名和 INSERT/DELETE SQL；`record::handleRunawayWatchDone` 实现 done 表移动事务。
- `syncer::Syncer` 检查表存在性、按窗口或 ID/资源组读记录，并维护两个扫描游标。
- `CatalogRef` 抽象资源组查询；`ExecutorRef` 抽象系统表 SQL 和最后插入 ID，因此 manager 不依赖具体 session 类型。

RustCodeGraph 对目标文件的 `node --file` 给出了 38 个符号和完整源码；精确图查询确认 `manager.rs::markQuarantine` 的被调用函数为 `addWatchList`。图对 Rust 方法调用的 callers 覆盖不完整，因此上述上游关系又用精确 `rg` 和相邻索引源码补证，而不是从文件名推断。

## 错误处理与边界

- 会向外传播的错误主要来自资源目录、SQL 执行器、Syncer 和 syncer mutex 中毒；统一类型定义在 `lib.rs::Error`。
- `watch_list`、`active_group` 和三个队列的锁中毒被降级处理：部分方法提前返回，查询快照/计数/drain 返回空或 0，队列记录被丢弃。这保证热路径不 panic，但会隐藏状态或审计数据缺失。
- 容量边界有两层：watch map 达到 10,000 时拒绝新的 key，但仍允许替换既有 key；每个记录队列达到 1,024 时丢弃新记录。当前都没有日志或返回值提示丢弃。
- `addWatchList` 对相同 ID 无条件早退，包括 `force` 路径；不同持久化 ID 的普通冲突把传入记录送 stale，手工强制替换则把旧记录送 stale。ID 为 0 的本地临时项可被持久化项替换。
- `AddWatch` 会把已过期记录送 stale；不过 ID 为 0 的 stale 也会进入队列，真正的外部刷盘方应像 Go loop 一样跳过不可删除的 ID 0。
- 过期项不会仅因时间流逝自动从 `HashMap` 消失；当前只在同 key 的 `addWatchList` 路径清理。若没有外部扫描或访问触发，该项仍可能被 `examineWatchList` 命中。这是相对于 Go `ttlcache` 自动过期的重要当前实现边界。
- `RemoveRunawayWatch` 把零条和多条查询结果都映射为 `Error::NotFound`；按组删除遇到第一条事务错误即停止，前面已成功移动的行不会被整体回滚。
- `AddRunawayWatch` 与 Go 不同：Rust 没有在该方法内显式 BEGIN/COMMIT、退避、取消等待或执行 `SELECT LAST_INSERT_ID()`，而依赖执行器的 `LastInsertId()`。因此事务性和 ID 获取能力由具体执行器契约决定，不能宣称已完整复刻 Go 行为。
- `handleRunawayWatchDone` 在 insert/delete 失败时尝试 rollback 并返回原错误；commit 错误被忽略，这是 `record.rs` 明确记录的 Go 未命名返回值兼容行为。
- `checkpointGaugeValue(0) == 0.0`，避免把“尚未同步”显示成极大的负 Unix 时间；非零值以微秒除 1000 转为毫秒。

## 并发与资源生命周期

`Manager` 的 `Arc` 使 Checker、Domain 和外部后台驱动可以共享状态。watch map、active-group map、三个队列和 Syncer 使用互相独立的 `Mutex`，减少不相关操作间的竞争；每资源组 counter 使用 Acquire/Release 或 AcqRel 原子序。

锁顺序需要保持：`addWatchList`/`removeWatch` 先持有 `watch_list`，随后可能取得 `active_group`；新增代码不要反向在持有 `active_group` 时调用会锁 watch 的方法。`UpdateNewAndDoneWatch` 持有 `syncer` 后会进入 `AddWatch`/`removeWatch`，后者不重新获取 syncer，因此当前没有递归锁。手工 Remove 也在持有 syncer 时执行 SQL，保证点查与同步游标操作串行，但可能长时间占锁。

队列操作在单个 mutex 临界区内检查长度并 push，drain 也在同一锁下 `mem::take`，因此不会丢失锁获取成功后的并发写；锁中毒时则按设计丢弃或返回空。队列没有条件变量、发送阻塞或唤醒机制，消费周期完全由外部调用者决定。

`Stop` 只设置 `AtomicBool`，不会清空状态、停止线程、唤醒消费者或阻止新记录；本文件也不创建线程。与 Go `Stop` 停止 `ttlcache` 不同，Rust 当前生命周期应由持有者管理。最后一个 `Manager` clone 被释放时 `ManagerInner` 及其中状态自然析构。

## 与 Go 版本的对应关系

对应基线主要是 `pkg/resourcegroup/runaway/manager.go`，手工增删持久化逻辑位于 `record.go`：

- 常量数值、三类队列、watch key、手工来源覆盖、ID 冲突处理、活跃资源组计数、同步先 new 后 done、首轮 done checkpoint overlap、按 ID 防误删，以及 flush threshold 512 均保留了 Go 意图。
- Go 用容量 10,000 的 `ttlcache`，带 insertion/eviction 回调和自动到期；Rust 用 `Mutex<HashMap>` 手动更新计数和 stale 队列，只在同 key 插入时发现到期。自动 TTL 驱逐尚不等价。
- Go 用三个有界 channel 和一个 select loop；Rust 用三个有界 `Vec` 加 drain API。Go 热路径的 runaway/quarantine channel send 是 non-blocking default，Rust也在满时静默丢弃；但 Go 某些 stale send 是阻塞式，而 Rust stale 队列始终不会阻塞。
- Go `RunawayRecordFlushLoop` 会去重/合并并定期写表、GC 历史记录；Rust `manager.rs` 不创建该循环。相邻 `flusher.rs::BatchFlusher` 和 `record.rs` 提供部分构件，但目标文件未把它们装配起来。
- Go `RunawayWatchSyncLoop` 每秒调用 `UpdateNewAndDoneWatch` 并记录指标；Rust 只提供一次同步方法，不含 ticker、日志或同步成功/失败/耗时指标。
- Go `UpdateNewAndDoneWatch` 分为带指标的外层和 `doSync`；Rust合并为一个方法，只更新 `last_sync_time`。同步的数据顺序与 overlap 语义一致。
- Go `AddRunawayWatch` 在事务中 INSERT 并用带退避的 `SELECT LAST_INSERT_ID()` 获取 ID；Rust 直接 Execute 后轮询执行器属性，没有事务或延迟。Go Remove 与 Rust Remove 都在 syncer 锁下点查并转入 done；Rust 的底层事务 helper 对 commit 错误保持 Go 的实际返回语义。
- Go `Stop` 停止 TTL cache；Rust只置位。`MaxWaitDurationMillis` 也只是数值对应，单位通过名称表达为毫秒。

Rust 独立回归 `manager_test.rs::duplicate_and_manual_watch_replacement_match_go` 专门固定了重复 ID、手工相同 ID 和手工不同 ID 替换的 Go 语义。`checker_test.rs` 覆盖 watch 动作、切组校验、规则超限后 runaway/quarantine 排队和并发阈值状态；Go `checker_test.go::TestActiveGroupCounterOrdering` 还覆盖 counter 正反顺序及并发加减，而 Rust 目标测试目前没有等量的 manager counter 并发用例。

## 扩展指南

- 接入后台服务时，应在明确的 Domain 生命周期位置周期调用 `UpdateNewAndDoneWatch`，并消费三个 drain 队列；停止顺序必须先停止生产/定时任务、完成最后批次处理，再释放 manager。不要仅依赖当前 `Stop` 标志。
- 补齐 flush loop 时复用 `flusher.rs::BatchFlusher` 和 `record.rs` 的批量 SQL：runaway 以 `RecordKey` 合并并递增 `Repeats`，quarantine 以 record key 去重，stale 以非零 ID 去重；同时同步 Go 的定时周期、失败丢批和 GC 行为。
- 若要宣称 TTL 与 Go 等价，需在本文件引入主动过期机制，或至少让 `examineWatchList` 在返回前校验/驱逐过期条目；同时保持计数只减一次、持久化 ID 才进入 stale 的不变量。相关测试应放在独立 `manager_test.rs`，不要内嵌进生产文件。
- 修改覆盖规则时重点维护四类情况：同 ID 重扫、ID 0 本地项被持久化项替换、不同非零 ID 冲突、`ManualSource` 强制替换。同步更新 `manager_test.rs::duplicate_and_manual_watch_replacement_match_go`，并对照 `manager.go::addWatchList`。
- 修改锁或计数时保持 `watch_list → active_group` 的锁序，增加 Rust 独立并发测试，对齐 Go `checker_test.go::TestActiveGroupCounterOrdering`；若改用无锁 map，需要证明 `DeriveChecker` 不会在添加 watch 后漏建 Checker。
- 扩展手工 Add/Remove 时应先决定 `RestrictedSqlExecutor` 的事务与 last-insert-ID 契约，再补 `record_test.rs`/`syncer_test.rs` 中的执行序列和失败注入测试；不要只让 NoopExecutor 路径通过。
- 改动同步窗口时同步审查 `syncer.rs::scan`、`WATCH_SYNC_OVERLAP_MICROS` 和 `syncer_test.rs`，保持窗口边界、满批推进、防活锁及并发 done 不扰动新增游标的约束。
- 对外增加状态或指标时优先提供只读快照，避免暴露 mutex guard；队列丢弃、watch 容量拒绝和锁中毒目前不可观测，是最值得补的运维信号。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/resourcegroup/runaway` 列出目标源、Go 对照、五个 Rust 独立测试文件及相邻实现。
- 目标源码：RustCodeGraph `node --file pkg/resourcegroup/runaway/manager.rs --offset 1 --limit 260` 与 `--offset 253 --limit 220`，覆盖全部 422 行及 38 个目标符号。
- 符号/调用查询：`query NewRunawayManager`、`query UpdateNewAndDoneWatch`、`query markQuarantine`、`query AddRunawayWatch` 精确区分 Rust/Go 定义；`callees manager.rs::markQuarantine` 得到 `manager.rs::addWatchList`。图的 Rust callers 结果为空或不完整，随后以精确 `rg` 补查调用点，此限制已纳入结论。
- Rust 直接证据：`pkg/resourcegroup/runaway/lib.rs`、`checker.rs`、`record.rs`、`syncer.rs`、`flusher.rs`、`manager_test.rs`、`checker_test.rs`、`syncer_test.rs`，以及应用接线 `pkg/domain/domain.rs`、`pkg/domain/runaway.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`。
- Go 对照：`pkg/resourcegroup/runaway/manager.go`、`record.go`、`checker_test.go`。其中 `manager.go` 给出构造、两个后台循环、watch 替换/同步/停止语义，`record.go` 给出手工 Add/Remove 事务与 ID 获取路径。
- crate 边界：`pkg/resourcegroup/runaway/Cargo.toml` 的 package、lib path、porting metadata 和 target 条件依赖。
- 接线范围补查：`rg` 只发现 `NewRunawayManager` 的 Rust 实际构造位于独立测试，Domain 的生产结构提供 manager 绑定槽位，session 生产代码消费已绑定 manager；`UpdateNewAndDoneWatch`、drain、手工增删暂无 Rust 生产调用，querywatch 中对应代码仍为注释。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并检查 git diff 仅包含本文档和任务文件删除。
