# `pkg/util/workloadrepo/housekeeper.rs`

## 文件定位

本文件属于 `astersql-util-workloadrepo` crate 的分区维护层。crate 根 `pkg/util/workloadrepo/lib.rs` 以私有模块 `housekeeper` 装配本文件，再通过 `pub use housekeeper::*` 导出其中的公开函数和 `worker` 方法；`pkg/util/workloadrepo/Cargo.toml` 表明该 crate 仅直接依赖 `chrono = "0.4"`，并声明对应 Go 包为 `pkg/util/workloadrepo`。

在当前 Rust 应用链中，`pkg/util/workloadrepo/table.rs::worker::createAllTables` 创建缺失目标表后调用 `createAllPartitions`，确保已有表也补足未来分区。另一方面，`startHouseKeeper` 尚未被 Rust 生产代码调用，只在 `pkg/util/workloadrepo/worker_test.rs::TestHouseKeeperThread` 中直接取得并执行一次闭包。因此，本文件目前既承载已接线的“建表时补分区”能力，也承载尚未接入持续后台调度的 housekeeper API。

## 核心职责

- `calcNextTick` 计算从给定本地时间到下一个本地 02:00 的间隔；当天 02:00 已到或已过时，目标移到次日。
- `createPartition` 查询目标表已有分区，借助 `generatePartitionRanges` 只生成所需的未来分区，并通过 `execRetry` 执行 `ALTER TABLE ... ADD PARTITION`。
- `dropOldPartition` 逐个解析目标表分区名，并删除年龄达到保留天数阈值的分区。
- `createAllPartitions` 和 `dropOldPartitions` 把单表操作扩展到 `worker.workloadTables` 的整张表清单。
- `getHouseKeeper` 形成一次性维护闭包：只有后端报告当前节点为 owner 时才先补分区、再按当前保留期清理。
- `startHouseKeeper` 当前只是 `getHouseKeeper` 的同义入口，不负责线程、定时器或循环调度。

这些职责以 `RepositoryBackend` 抽象数据库与 owner 状态，不直接依赖 TiDB session/infoschema 类型；后端契约定义在 `pkg/util/workloadrepo/worker.rs::RepositoryBackend`。

## 主要符号

- `pub fn calcNextTick(now: DateTime<Local>) -> Duration`：用 `Local.with_ymd_and_hms(..., 2, 0, 0).single().unwrap()` 构造当天 02:00，并返回严格大于零且不超过一天的间隔。它没有被当前 Rust 的 `startHouseKeeper` 调用，现阶段主要由测试和外部调用者使用。
- `pub fn createPartition(backend, table, now) -> Result<(), String>`：读取 `backend.partitions(table.destTable)`；当 `generatePartitionRanges` 返回 `false` 时执行带 schema/table 标识符反引号的 `ADD PARTITION` DDL。
- `pub fn dropOldPartition(backend, table, now, retention) -> Result<(), String>`：把每个 `pYYYYMMDD` 名称交给 `parsePartitionName`，当 `(now - partition_time).num_days() >= retention` 时执行带分区标识符反引号的 `DROP PARTITION` DDL。
- `pub fn worker::createAllPartitions(&self, now) -> Result<(), String>`：持有 `workloadTables` 互斥锁并依次调用 `createPartition`；遇到第一项错误立即返回。
- `pub fn worker::dropOldPartitions(&self, now, retention) -> Result<(), String>`：`retention == 0` 时禁用清理；否则持锁遍历全部表，收集所有单表错误，最后以 `"; "` 合并返回。
- `pub fn worker::getHouseKeeper(&self, now) -> impl FnOnce() -> Result<(), String>`：捕获 `&self` 与固定 `now` 的一次性闭包；owner 节点先执行 `createAllPartitions`，成功后读取 `self.intervals().2` 并执行清理。
- `pub fn worker::startHouseKeeper(&self, now) -> impl FnOnce() -> Result<(), String>`：直接返回 `getHouseKeeper(now)`；名称中的 “start” 不代表当前实现会生成线程或安排未来 tick。

本文件没有自定义常量、结构体、trait 或条件编译项。所有符号均为公开函数/方法，但模块本身由 `lib.rs` 私有声明后统一再导出。

## 执行流程

1. 建表路径从 `pkg/util/workloadrepo/worker.rs::worker::startRepository` 进入 `pkg/util/workloadrepo/table.rs::worker::createAllTables`；后者创建缺失表后调用 `createAllPartitions(now)`。
2. `createAllPartitions` 在表清单上逐项调用 `createPartition`。单表函数读取已有分区名，`pkg/util/workloadrepo/utils.rs::generatePartitionRanges` 以列表最后一个分区为基准，按需生成 `now` 之后第 1、2 天的 RANGE 分区定义。
3. 如果范围均已覆盖，`generatePartitionRanges` 返回 `true`，不发 DDL；否则 `createPartition` 拼接 `ALTER TABLE \`WORKLOAD_SCHEMA\`.\`<destTable>\` ADD PARTITION (...)`，交由 `pkg/util/workloadrepo/worker.rs::execRetry` 最多执行五次。
4. 显式执行 `startHouseKeeper(now)()` 或 `getHouseKeeper(now)()` 时，闭包先检查 `RepositoryBackend::is_owner`。非 owner 直接成功返回且不访问表清单。
5. owner 先运行 `createAllPartitions`；只有全部建分区操作成功，才从 `worker::intervals()` 取三元组的第三项 `retentionDays` 并调用 `dropOldPartitions`。
6. `dropOldPartitions` 对 `retention == 0` 立即返回。否则每张表调用 `dropOldPartition`：分区名解析为本地零点，达到阈值的分区逐一执行 `DROP PARTITION`；跨表错误不会中断剩余表，但最终返回合并错误。

当前 Rust 流程到“一次闭包执行”即结束，没有再次调用 `calcNextTick`、等待到 02:00 或自动安排下一轮。

## 数据与状态

- 时间统一使用 `chrono::DateTime<Local>` 与 `chrono::Duration`。分区日期由 `parsePartitionName` 解析为本地零点；保留判断使用整日数 `num_days()`，边界为“大于等于 retention 即删除”。
- 表元数据来自 `worker.workloadTables: Mutex<Vec<repositoryTable>>`。本文件只读取 `repositoryTable.destTable`，用它查询后端分区并生成目标表 DDL。
- 后端由 `worker.backend: Arc<dyn RepositoryBackend>` 持有，本文件经共享引用调用 `partitions`、`execute`（经 `execRetry`）和 `is_owner`。
- 保留天数存于 `worker.state: Mutex<WorkerState>`；`getHouseKeeper` 在完成建分区后通过 `intervals().2` 读取执行当下的值，而不是在构造闭包时复制它。
- SQL 没有参数值，`execRetry` 的参数数组恒为 `&[]`。schema 使用常量 `workloadSchema`，表名和分区名以反引号包裹；当前拼接没有调用 `table.rs::identifier` 的内部反引号转义逻辑。

## 依赖与调用关系

上游调用关系：

- `pkg/util/workloadrepo/table.rs::worker::createAllTables -> worker::createAllPartitions` 是当前 Rust 生产路径中的直接调用边。
- `pkg/util/workloadrepo/worker_test.rs` 直接调用 `createAllPartitions`、`dropOldPartitions`、`calcNextTick` 和 `startHouseKeeper`；`pkg/util/workloadrepo/housekeeper_test.rs` 直接调用 `dropOldPartition`。
- RustCodeGraph 对 `housekeeper.rs` 建立了 10 个符号节点；精确 `callers` 查询在本次分析的有界等待内未返回，因此上述上游边以已索引源码和仓库内直接引用检索核验。

下游调用关系：

- `createPartition -> RepositoryBackend::partitions -> generatePartitionRanges -> execRetry`。
- `dropOldPartition -> RepositoryBackend::partitions -> parsePartitionName -> execRetry`。
- `createAllPartitions -> createPartition`；`dropOldPartitions -> dropOldPartition`。
- `getHouseKeeper -> RepositoryBackend::is_owner -> createAllPartitions -> worker::intervals -> dropOldPartitions`。
- `startHouseKeeper -> getHouseKeeper`。

跨文件实现分别位于 `worker.rs`（后端、状态、重试）、`utils.rs`（分区解析与生成）、`table.rs`（建表入口）。`Cargo.toml` 没有 feature 门控，本文件也没有 `cfg` 分支。

## 错误处理与边界

- 所有数据库/解析错误以 `Result<_, String>` 传播。`createPartition` 和 `createAllPartitions` 使用 `?`，任何失败都会立即中断当前调用链；housekeeper 因此不会在建分区失败后继续删除旧分区。
- `dropOldPartition` 一旦查询分区、解析任意分区名或执行某次 DDL 失败，就停止处理该表。外层 `dropOldPartitions` 会继续处理后续表并合并跨表错误，但不合并同一表内后续分区的潜在错误。
- `execRetry` 对每条 DDL 最多尝试五次；全部失败时用换行连接五次错误。`dropOldPartitions` 再用分号空格连接不同表的错误，两层错误聚合格式不同。
- `retention == 0` 是显式禁用值。负数未在本文件校验；按照当前比较式，通常会使所有可解析分区满足删除条件，合法范围应由上层系统变量校验保证（`utils.rs::setRetentionDays` 的注释也声明范围由 sysvar 层校验）。
- `calcNextTick` 对本地 02:00 使用 `.single().unwrap()`；如果时区在该时刻出现不存在或重复的本地时间，可能 panic。当前代码没有为夏令时歧义提供错误返回。
- 分区列表被当作按日期升序排列：`generatePartitionRanges` 只查看最后一项。后端若返回无序列表，可能错误判断覆盖范围。
- 表名和分区名来自内部元数据并使用反引号，但本文件没有转义名称内部的反引号；若未来允许不可信名称进入 `repositoryTable` 或分区列表，需要统一复用安全标识符格式化。

## 并发与资源生命周期

- `createAllPartitions` 与 `dropOldPartitions` 在整个表遍历及后端 DDL 调用期间持有 `workloadTables` 的 `MutexGuard`。这保证清单不会并发变化，但慢查询/重试会延长锁持有时间；新增动态表管理时应评估先 clone 快照再释放锁。
- `getHouseKeeper` 返回 `FnOnce + 'a`，借用 `worker`，没有产生 `'static` 任务，也没有克隆 `Arc<worker>`。闭包执行一次后即被消费。
- owner 检查只在闭包开头发生一次；若执行期间 owner 身份变化，当前轮不会再次确认。DDL 幂等性与 owner 交接安全依赖后端及 SQL 行为。
- 当前 Rust 版本不创建线程、timer、channel、async task 或取消令牌，也不持有专用数据库 session。资源生命周期仅包括同步闭包、互斥锁 guard 和单条 DDL 的同步重试。
- Go 版循环持有 session、监听 `context.Done()`、管理 `time.Timer` 并每轮重置；这些生命周期机制尚未出现在本 Rust 文件，扩展时不能假设已经存在。

## 与 Go 版本的对应关系

对应实现是 `pkg/util/workloadrepo/housekeeper.go`，Rust 保留了以下核心语义：下一个 02:00 的计算、缺失分区生成、owner-only 门控、先创建后清理、保留期为 0 时禁用、单表创建失败立即返回，以及清理阶段跨表收集错误。`pkg/util/workloadrepo/housekeeper_test.rs::drop_partition_quotes_partition_identifier_like_go` 还专门验证 Rust 的 DROP SQL 与 Go 一样引用分区标识符。

主要差异与迁移状态如下：

- Go `getHouseKeeper(ctx, fn)` 返回持续循环函数：构造 timer、从 session pool 取 session、响应取消、每次 tick 获取最新 infoschema，并在成功后 reset timer。Rust `getHouseKeeper(now)` 只执行固定时间点的一轮同步工作，不使用 `calcNextTick`，也没有上下文、timer、session 或循环。
- Go `startHouseKeeper(ctx)` 注入 `calcNextTick` 并由 `worker.go` 的 goroutine 管理启动；Rust `startHouseKeeper(now)` 仅转发到一次性闭包，Rust 生产代码当前没有对应启动调用边。
- Go 从 infoschema 取得表与分区元数据并输出带上下文的日志/错误；Rust 把这些能力压入 `RepositoryBackend::partitions`，错误为裸 `String`，本文件不记录日志。
- Go 的 `dropOldPartition` 显式检查表元数据和分区信息非空，并对错误添加表/分区上下文；Rust 后端直接返回分区名列表，无法在本层区分“无分区表”和“空分区列表”。
- Go 的 housekeeper 在某些失败分支 `continue` 前没有重置 timer，按当前源码会等待不到下一次 timer 事件；本文只陈述源码事实，不把该行为作为 Rust 应复制的目标。

Go 测试 `pkg/util/workloadrepo/worker_test.go::TestCreatePartition` 覆盖从旧分区、当天、明天、后天和存在日期缺口等输入生成分区；`TestDropOldPartitions` 覆盖保留边界与“不能删除最后分区”的数据库错误；`TestHouseKeeperThread` 覆盖后台运行和保留期变化；`TestCalcNextTick` 覆盖 02:00 前后纳秒边界。Rust 测试覆盖了主要结果，但定时循环和大部分边界仍主要由 Go 测试提供对照证据。

## 扩展指南

- 若补齐后台调度，应修改 `getHouseKeeper`/`startHouseKeeper` 并在 `worker` 生命周期中建立明确启动、取消和 join 机制；需要决定同步线程还是 async task，并同步移植 Go `TestHouseKeeperThread` 的多轮运行、owner 变化与动态 retention 场景到独立测试文件 `pkg/util/workloadrepo/housekeeper_test.rs` 或 `worker_test.rs`，不要把测试写进生产源文件。
- 若调整分区前瞻窗口，入口在 `pkg/util/workloadrepo/utils.rs::generatePartitionRanges`，同时检查 `table.rs::checkTableExistsByIS` 的“至少覆盖明天之后”不变量，以及 Rust `TestCreatePartition` 和 Go 同名测试。
- 若改变清理阈值，入口是 `dropOldPartition` 的 `num_days() >= retention`；需要补测恰好到期、差一秒、DST 日、零值和非法负值，并确保系统变量层的合法范围与本层假设一致。
- 若增强错误可诊断性，应在 `createPartition`/`dropOldPartition` 为表名、分区名和操作类型添加上下文，同时保持 `dropOldPartitions` 的跨表继续策略，并更新错误聚合测试。
- 若表清单可运行时变更，可考虑在持锁期间 clone `Vec<repositoryTable>` 后释放锁再执行 DDL；代价是本轮使用清单快照。应增加并发修改清单与长时间重试的测试。
- 若接受外部可控标识符，应将 ADD/DROP DDL 都接入统一转义函数，覆盖表名和分区名中反引号的回归测试。
- 与 Go 对齐时应保留实际行为，但不应机械复制 Go timer 失败分支；先明确失败后是否必须重新调度，再以独立测试固定契约。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 7,032 个 Rust 文件；`files --filter pkg/util/workloadrepo` 找到 22 个该模块文件；`node --file pkg/util/workloadrepo/housekeeper.rs --offset 1 --limit 240` 读取了本文件 118 行全貌；`query HouseKeeper`/`query housekeeper` 确认本文件的 7 个函数/方法节点及 Go 对照符号。精确 `callers housekeeper.rs::startHouseKeeper` 在两次 30 秒有界等待内无输出，随后中止，故未把缺失的图结果当作“无调用者”的唯一证据。
- 已读生产文件：`pkg/util/workloadrepo/housekeeper.rs`、`lib.rs`、`Cargo.toml`、`worker.rs`、`table.rs`、`utils.rs`，以及 Go 对照 `housekeeper.go`。
- 已读测试：`pkg/util/workloadrepo/housekeeper_test.rs`、`worker_test.rs` 的相关测试段，以及 `worker_test.go` 中 `TestCreatePartition`、`TestDropOldPartitions`、`TestHouseKeeperThread`、`TestCalcNextTick`。
- 直接引用检索：`rg` 确认 Rust 生产侧 `table.rs:119` 调用 `createAllPartitions`；其余 housekeeper 入口的 Rust 直接引用位于上述独立测试。Go 侧 `worker.go` 启动 `startHouseKeeper`，形成与 Rust 当前接线程度的对照。
- 本任务是纯文档分析，未运行 Cargo 或代码测试。交付前按任务验证命令检查目标文件存在且恰好具有 11 个固定二级章节，并人工复核本文件为何存在、当前如何运行及安全扩展入口。
