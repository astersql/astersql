# `pkg/resourcegroup/runaway/syncer.rs`

## 文件定位

本文件实现 runaway 子系统的系统表读取与增量同步边界。它属于 `astersql-resourcegroup-runaway` crate；crate 入口在 [`lib.rs`](./lib.rs) 中以 `pub mod syncer` 暴露模块，并由 [`manager.rs`](./manager.rs) 的 `ManagerInner::syncer` 持有。数据源是 `mysql.tidb_runaway_watch` 与 `mysql.tidb_runaway_watch_done`，前者提供新增监视，后者提供已完成、应从节点本地监视列表移除的记录。

在应用链路中，`Manager::UpdateNewAndDoneWatch` 是增量扫描的直接上游：它先拉取 watch 并调用 `AddWatch`，再拉取 watch_done 并调用 `removeWatch`。`Manager::RemoveRunawayWatch` 和 `Manager::RemoveRunawayResourceGroupWatch` 则使用本文件的点查 API 查出待移动到 done 表的记录。因此本文件只负责“构造读取、执行读取、解码和维护扫描游标”，不负责本地列表去重、系统表写入或定时调度。

## 核心职责

1. `SystemTableReader` 为一张系统表缓存三种 SQL 模板，并保存半开扫描窗口 `[check_point, upper_bound)` 的游标状态。
2. `Syncer` 组合两套 reader：`new_watch_reader` 以 `start_time` 为分页键；`deletion_watch_reader` 以 `done_time` 为分页键。
3. `Syncer::scan` 通过 `RestrictedSqlExecutor::Execute` 执行有序、限量的时间窗口查询，把合法行解码成 `QuarantineRecord`，并根据有效记录数推进或保持 checkpoint。
4. `getWatchRecordByID` / `getWatchRecordByGroup` 提供不改变扫描状态的点查路径。
5. `decodeQuarantineRecord` 通过每张表独立的 `QuarantineColumns` 映射处理两表不同的列布局，并把数据库值转换为 runaway 枚举和记录字段。
6. `SystemTableCatalog` 抽象系统表存在性探测，使 manager 能在表尚未创建时跳过同步。

文件不执行周期循环。`WATCH_SYNC_INTERVAL_MICROS` 只是与 Go 的一秒同步周期对齐并用于推导三秒重叠窗口；实际何时调用 `UpdateNewAndDoneWatch` 由文件外的运行时负责。

## 主要符号

- `WATCH_SYNC_INTERVAL_MICROS = 1_000_000`：名义同步周期，一秒。
- `WATCH_SYNC_OVERLAP_MICROS = 3 * WATCH_SYNC_INTERVAL_MICROS`：非空且非满批扫描后回退的三秒重叠窗口。
- `WATCH_SYNC_BATCH_LIMIT = 2048`：单次查询上限；与 Go 的 `maxWatchRecordChannelSize * 2` 数值一致。
- `SqlRow(Vec<SqlValue>)`：执行器返回的一行。私有辅助方法 `int`、`text`、`time`、`nullable_time` 进行带类型检查的列读取；`int` 允许可转换为 `i64` 的 `UInt`，`nullable_time` 把 SQL `NULL` 映射为时间戳 `0`（不过期）。
- `QuarantineColumns`：`QuarantineRecord` 字段到 `SELECT *` 结果列的映射。`WATCH_RECORD_COLUMNS` 使用 watch 表的 `id`；`WATCH_DONE_RECORD_COLUMNS` 跳过 done 表自身主键，使用索引 1 的原记录 ID，并把扫描键设为索引 11 的 `done_time`。
- `SystemTableCatalog::TableExists`：目录探测协议。`AllSystemTables` 是始终返回 `true` 的默认/测试实现，不代表生产目录真的存在这些表。
- `SystemTableReader::new`：初始化表名、分页键、列布局和三个预生成 SQL；所有时间状态初始为 `0`。
- `genSelectByIDStmt` / `genSelectByGroupStmt`：生成 watch 表点查语句及绑定参数。
- `genSelectStmt`：生成 `key >= check_point AND key < upper_bound ORDER BY key LIMIT 2048` 的窗口查询参数。
- `Syncer::new`：装配两张表的 reader、执行器与目录；watch 的分页键索引为 2，watch_done 的分页键索引为 11。
- `checkWatchTableExist` / `checkWatchDoneTableExist`：分别探测 `mysql.tidb_runaway_watch` 与 `mysql.tidb_runaway_watch_done`。
- `getNewWatchRecords` / `getNewWatchDoneRecords`：可变借用对应 reader 并进入共同的 `scan` 实现。
- `readQuarantineRecords`：点查的执行和过滤解码路径，不改 reader。
- `decodeQuarantineRecord`：公开的单行解码函数；必需列缺失或类型不符时返回 `None`。

## 执行流程

增量扫描按以下步骤运行（`Syncer::scan`）：

1. 以 `nowMicros()` 捕获本轮 `upper_bound`。查询窗口是 `[check_point, upper_bound)`，上界不包含，避免读取扫描开始之后才出现的未来记录。
2. `genSelectStmt` 绑定下界、上界和 2048 上限，`ExecutorRef::Execute` 执行查询。SQL 明确按分页键升序排列。
3. 对每一行先读取分页键，再调用 `decodeQuarantineRecord`。只有整条记录解码成功才加入结果；此时若分页键也合法，更新 `last_scan_key_time`。
4. 若有效记录数达到 2048，优先把 checkpoint 移到最后一个有效分页键。查询条件使用 `>=`，边界行可能在下一轮重复出现，重复消解由 manager 的 `AddWatch` / `removeWatch` 语义承担。
5. 满批但最后键没有超过旧 checkpoint 时，改为 `upper_bound - overlap`，避免大量相同微秒时间戳令扫描永远停在同一页。
6. 若是非空部分批次，checkpoint 同样设为 `upper_bound - overlap`，让下一轮重读窗口尾部，覆盖提交可见性延迟和节点时钟偏差。
7. 空结果或所有行均解码失败时不改变 checkpoint。执行器报错通过 `?` 原样返回；checkpoint 也不推进。

点查路径先由 `genSelectByIDStmt` 或 `genSelectByGroupStmt` 构造 SQL，再由 `readQuarantineRecords` 执行并 `filter_map` 掉坏行。它不写 `check_point`、`upper_bound` 或 `last_scan_key_time`，所以夹在同步轮次之间的手工删除操作不会扰动增量分页状态。

manager 接线顺序见 `Manager::UpdateNewAndDoneWatch`：先确认 watch 表存在并扫描新增项；首次处理 done 表前，将 done reader 的 checkpoint 初始化为 watch reader 的 `upper_bound - overlap`；再确认 done 表存在并扫描删除项。该顺序避免初次启动从时间戳零全量扫描历史 done 表。

## 数据与状态

`Syncer` 的持久状态只有两个 `SystemTableReader`、共享执行器/目录引用以及 `last_sync_time`。`last_sync_time` 在本文件中不参与扫描计算，由 `Manager::UpdateNewAndDoneWatch` 在每轮开始时写入。

每个 reader 的关键不变量是：

- `check_point` 是下一次查询的含边界下界；查询使用 `>=`，通常会重读最后键并产生重复。若单个时间戳内的行数超过批上限且没有稳定的次级排序键，则仍受后述同微秒极端限制约束。
- `upper_bound` 是当前轮捕获的排他上界，仅扫描路径更新。
- `last_scan_key_time` 只记录“成功解码记录且分页键类型合法”的最后键；点查不触碰它。
- SQL 模板在构造时生成，正常扫描只替换参数，避免每轮重复拼接字符串。

`QuarantineRecord` 的 `EndTime == 0` 表示永不过期。watch 数值 `1/2/3` 分别映射 `Exact/Similar/Plan`，其他值降级为 `None`；action 数值 `1/2/3/4` 分别映射 `DryRun/CoolDown/Kill/SwitchGroup`，其他值降级为 `NoneAction`。这是一种向未知枚举值容错的策略，不会把未知值当作解码错误。

两张表的字段差异完全由列映射隔离：done 表第 0 列是 done 行主键，第 1 列才是原 watch ID，第 11 列 `done_time` 仅用于分页，不进入 `QuarantineRecord`。

## 依赖与调用关系

上游调用关系（RustCodeGraph 与源码交叉核对）：

- `Manager::NewRunawayManager -> Syncer::new`：创建 manager 时注入 `ExecutorRef` 和 `SystemTableCatalog`。
- `Manager::UpdateNewAndDoneWatch -> checkWatchTableExist/getNewWatchRecords/checkWatchDoneTableExist/getNewWatchDoneRecords`：同步系统表到内存 watch 列表。
- `Manager::RemoveRunawayWatch -> getWatchRecordByID`，`Manager::RemoveRunawayResourceGroupWatch -> getWatchRecordByGroup`：为系统表事务删除取得完整记录。
- 独立测试直接调用 `Syncer::new`、扫描入口和 `decodeQuarantineRecord` 验证内部契约。

下游依赖关系：

- `Syncer::scan` 与 `readQuarantineRecords` 调用 crate 根定义的 `RestrictedSqlExecutor::Execute`，错误类型统一为 crate 的 `Result<T>`。
- 解码依赖 [`record.rs`](./record.rs) 的 `QuarantineRecord`、`SqlValue` 和两张系统表全名常量。
- 时间上界依赖 crate 根的 `nowMicros`；动作和匹配类型依赖 `RunawayAction`、`RunawayWatchType`。
- 目录检查通过 `Arc<dyn SystemTableCatalog>` 动态分派；SQL 执行通过 `ExecutorRef = Arc<dyn RestrictedSqlExecutor>` 共享。

[`Cargo.toml`](./Cargo.toml) 声明 crate 名为 `astersql-resourcegroup-runaway`、入口为 `lib.rs`，并记录 Go 包映射 `pkg/resourcegroup/runaway`。当前 manifest 的工作区依赖集中在 `cfg(windows)` 区段；本文件自身直接使用的只有标准库与同 crate 符号，没有新增第三方依赖。

## 错误处理与边界

- 执行器错误直接传播，不包装也不吞掉。`scan` 在执行前已经刷新 `upper_bound`，但错误时不会推进 `check_point` 或 `last_scan_key_time`；调用方若重试仍从原下界开始。
- 任一必需字段缺失或类型不匹配都会让 `decodeQuarantineRecord` 返回 `None`，坏行被静默过滤。这包括 ID、名称、起止时间、watch/action、文本、来源、切换组和超限原因；Rust 的防御范围比 Go 注释中强调的时间解析更广。
- `EndTime` 只接受 `Null` 或 `Time`；`Null` 转为 0。错误的非空类型会丢弃整行。
- 原始查询命中 2048 行、但部分行解码失败时，判断依据是“有效记录数”而非原始行数，因此走部分批次回退路径。这会重读尾部，优先避免漏读。
- done 行即使 `done_time` 类型错误，只要记录字段都合法仍可被返回；此时 `last_scan_key_time` 不更新。满批时防活锁分支会依状态决定是否回退。扩展解码规则时应特别保持“记录有效性”和“分页键有效性”之间的关系清晰。
- 同一微秒超过 2048 行且分页键无法前进时，回退窗口是现实负载下的防活锁折中；Go 源码明确记录了极端情况下可能跳过重叠窗口以外未返回同行键数据的 TODO。Rust 当前保留同一限制。
- 初始或非常早的 `upper_bound` 减去重叠量可能得到负时间戳；当前实现未做饱和减法。实际 Unix 微秒时钟远大于重叠量，因此正常运行不触发。
- `AllSystemTables` 无条件报告存在，只适合默认/测试或调用方明确接受该假设的场景；真实目录实现必须反映系统表生命周期。

## 并发与资源生命周期

`Syncer` 内部不创建线程、任务、通道或事务，也不自行加锁。两个扫描入口需要 `&mut self`，Rust 借用规则保证单个实例上的扫描状态不会被无同步并发修改。在实际 manager 中，`Syncer` 放在 `Mutex<Syncer>` 内，`UpdateNewAndDoneWatch`、按 ID 删除和按资源组删除都通过这把锁串行访问；锁中毒转换为 `Error::Poisoned`。

执行器和目录以 `Arc` 共享，生命周期至少覆盖 `Syncer`；本文件不拥有显式关闭动作。一次扫描只暂存执行器返回的 `Vec<SqlRow>` 与解码后的 `Vec<QuarantineRecord>`，容量按原始行数预分配，上限由 SQL 的 2048 限制控制。SQL 模板随 reader 生命周期复用。

重叠窗口会有意重复读取记录，因此并发正确性依赖下游幂等语义：`Manager::addWatchList` 对相同 ID 的重复记录保持原对象，`removeWatch` 仅在 key 与 ID 都匹配时移除。修改扫描边界时必须同时复核这些下游不变量。

## 与 Go 版本的对应关系

直接对照文件为 [`syncer.go`](./syncer.go)，Rust 保留了以下核心语义：两张表和各自列布局、`start_time`/`done_time` 分页键、半开时间窗口、2048 行上限、三倍同步周期重叠、满批推进到最后有效键、同键防活锁回退、部分批次回退、空/坏行/错误保持 checkpoint，以及点查不改变扫描游标。

主要实现差异如下：

- Go 通过 `util.SessionPool` 和 `ExecRCRestrictedSQL` 获得受限 session；Rust 将其收敛为 `RestrictedSqlExecutor` trait，直接返回 `SqlRow`。
- Go 用 `infoschema.InfoCache` 探测表；Rust 用 `SystemTableCatalog` trait，降低与 infoschema 的直接耦合。
- Go 的 `syncer` 自带互斥锁和 Prometheus interval/duration/checkpoint/counter 指标；Rust 文件没有这些指标字段，互斥由 `ManagerInner::syncer` 外置承担，`last_sync_time` 也由 manager 更新。不能据此宣称 Rust 已在此文件实现 Go 的指标观测。
- Go 使用 `time.Time`；Rust 用微秒 `i64`。Rust 的 `SqlValue` 显式校验列类型，未知枚举值映射到默认枚举。
- Go 的 reader 以返回 closure 的 `sqlGenFn` 表示点查；Rust 直接返回 `(String, Vec<SqlValue>)`，但调用语义相同。

Rust 独立测试 [`syncer_test.rs`](./syncer_test.rs) 的可执行部分覆盖 SQL 形状和列映射、点查游标保护、两表部分批次推进、满批推进、同键防活锁、空结果/坏行/执行错误保持 checkpoint，以及原始满批但有效记录不足时走部分批次。该文件前半还保存 Go 测试参考文本；真正的 Rust 断言位于 `RowExecutor` 之后的 `#[test]` 函数。Go 独立测试 [`syncer_test.go`](./syncer_test.go) 额外提供原实现的时间窗口、分页和点查意图依据。

## 扩展指南

- 新增或调整系统表列时，先以实际 DDL 顺序更新 `QuarantineColumns` 常量和 `decodeQuarantineRecord`，同时更新 `watch_row` / `watch_done_row` 构造器与列映射测试。不要让 watch 与 watch_done 共用未经核验的硬编码布局。
- 修改分页键、排序或查询窗口时，集中改 `SystemTableReader::new` / `genSelectStmt` / `Syncer::scan`，并保留半开窗口、边界重复可去重和 checkpoint 可前进三个不变量。若要解决同微秒超大批次问题，应考虑稳定的复合游标（时间加唯一 ID），而不是简单移除防活锁分支。
- 改变坏行策略时，应明确区分记录字段解码失败与分页键失败；同步更新 `invalid_tail_uses_partial_batch_checkpoint_rule`、`empty_invalid_and_error_scans_hold_checkpoint`，并补充 done_time 非法的独立用例。
- 新增点查方式时应复用 `readQuarantineRecords`，避免改写 `check_point`、`upper_bound`、`last_scan_key_time`；同步扩展 `point_queries_preserve_scan_cursor_state`。
- 接入真实目录或执行器应在 crate 外实现 `SystemTableCatalog` / `RestrictedSqlExecutor`，不要把 infoschema/session 细节重新硬编码进 reader。
- 若补齐 Go 的指标，应在 manager 调度边界测量同步间隔、耗时、成功/失败和两个 checkpoint，避免仅在 `scan` 内计时而遗漏表不存在或 done 阶段失败的分支。
- 所有 Rust 测试继续放在同目录独立的 [`syncer_test.rs`](./syncer_test.rs)，不要嵌入生产源文件；语义变化还应同步核对 [`syncer_test.go`](./syncer_test.go) 的原始意图。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust/Go 源与测试均在索引内。
- RustCodeGraph `node --file pkg/resourcegroup/runaway/syncer.rs` 核对了完整 320 行源码及模块符号；精确查询确认 `scan` 被两个增量入口调用，`readQuarantineRecords` 被两个点查入口调用，`decodeQuarantineRecord` 被扫描、点查及 Rust 测试调用。
- RustCodeGraph 对 [`manager.rs`](./manager.rs) 的读取确认生产接线：构造、增量同步顺序、首次 done checkpoint 初始化，以及两个删除 API 的点查调用。
- [`Cargo.toml`](./Cargo.toml) 核对 crate 名称、`lib.rs` 入口、Go 包映射和条件依赖边界；[`lib.rs`](./lib.rs) 核对模块导出、`Timestamp`、公共错误、枚举、`RestrictedSqlExecutor` 与 `ExecutorRef`。
- [`syncer.go`](./syncer.go) 与 [`syncer_test.go`](./syncer_test.go) 核对 Go 的扫描算法、重叠窗口理由、同键限制、列布局和回归意图；[`syncer_test.rs`](./syncer_test.rs) 核对 Rust 当前实际覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求文档存在，且上述十一个固定二级标题各出现一次。
