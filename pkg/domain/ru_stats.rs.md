# `pkg/domain/ru_stats.rs`

源码：[ru_stats.rs](./ru_stats.rs)；独立 Rust 测试：[ru_stats_test.rs](./ru_stats_test.rs)；Go 对照：[ru_stats.go](./ru_stats.go) 与 [ru_stats_test.go](./ru_stats_test.go)。

## 文件定位

该文件属于 `astersql-domain` crate；crate 入口 `pkg/domain/lib.rs` 以 `pub mod ru_stats` 公开此模块。它提供 RU（Request Unit）历史统计的领域模型、后端抽象和同步写入/清理算法，目标数据语义对应系统表 `mysql.request_unit_by_group`（`RuStatsRow`、`generate_sql`）。

当前 Rust 实现是一个可注入 `RuStatsBackend` 的核心算法，并未像 Go 版那样直接持有 PD client、InfoSchema、KV storage 和系统 session pool。RustCodeGraph 与仓库文本检索均未发现非测试 Rust 生产调用者；现有直接使用来自 `pkg/domain/ru_stats_test.rs`。因此它目前是公开但尚未接入 Rust `Domain` 后台生命周期的能力，不能把文件前半部注释保存的 Go 风格伪代码当作已运行实现。

`pkg/domain/Cargo.toml` 将 crate 根指定为 `lib.rs`，没有为本模块设置 feature 或平台条件。实际可编译代码只依赖标准库的 `BTreeMap`、`Duration`、`SystemTime` 和 `UNIX_EPOCH`；crate 的 PD、KV、InfoSchema 等依赖并未被本文件直接使用。

## 核心职责

- 用 `GroupRuStats`、`DailyRuStats`、`RuStats` 表示资源组累计 RU 快照，并用 `RuStatsRow` 表示一个对齐区间内待落库的增量。
- 通过 `RuStatsBackend` 隔离去重查询、快照读取、资源组拉取、快照持久化、历史行写入和过期行删除，使 `RuStatsWriter<B>` 不依赖具体数据库或网络客户端。
- 在 `RuStatsWriter::write` 中执行“校验区间—检查重复—读取/刷新快照—计算增量—写行”的主流程。
- 在 `generate_rows` 中只对同名且同 ID 的前后快照做差，过滤小于 1 或非有限的增量。
- 在 `RuStatsWriter::gc_outdated_records` 中按 92 天保留窗口和 1000 行批次持续清理。
- `generate_sql` 可把行转换成 `REPLACE INTO` SQL，并转义资源组名中的单引号；但 `write` 本身调用的是 `backend.insert_rows`，不会调用该 SQL 生成器。

## 主要符号

- `RU_STATS_INTERVAL`：默认及允许的最大写入间隔，24 小时。
- `RU_STATS_GC_DURATION`：保留窗口，92 天。
- `GC_BATCH_SIZE`：单次删除上限，1000 行。
- `GroupRuStats { id, name, read_ru, write_ru }`：PD 风格的资源组累计读写 RU；ID 用于防止同名资源组被删除重建后错误相减。
- `DailyRuStats { end_time, groups }`：某一对齐结束时刻的全量累计快照。
- `RuStats { previous, latest }`：最多保存相邻两期快照；首期没有 `previous`。
- `RuStatsRow { start_time, end_time, resource_group, total_ru }`：区间增量结果。
- `RuStatsBackend`：六个同步、可失败的后端操作。错误统一为 `String`，实现者负责把真实存储或网络错误转换进来。
- `RuStatsWriter<B> { interval, start_time, backend }`：写入器；泛型约束仅在 `impl<B: RuStatsBackend>` 上施加。
- `RuStatsWriter::write() -> Result<Vec<RuStatsRow>, String>`：核心入口，同时返回实际生成的行，便于调用方观察或测试。
- `RuStatsWriter::gc_outdated_records(end) -> Result<usize, String>`：GC 入口，返回累计删除行数。
- `get_last_expected_time(now, interval)`：按 UNIX 纪元对秒级区间向下取整。
- `generate_rows(stats, interval)`：纯增量计算函数。
- `generate_sql(rows)`：纯 SQL 文本生成函数；总 RU 以 `as i64` 截断后写入。

文件没有条件编译项。第 23—284 行均为注释化的 Go 迁移草稿，不声明 Rust 符号。

## 执行流程

`RuStatsWriter::write` 的顺序及短路点如下：

1. 拒绝零区间和大于 24 小时的区间。
2. `get_last_expected_time(start_time, interval)` 计算不晚于参考时间的对齐终点，再以 `checked_sub` 得到起点；下溢立即报错。
3. 调用 `backend.is_inserted(start, end)`。已存在任意对应区间时直接返回空向量，不再读取、拉取或写入。
4. 调用 `backend.load_latest()`。若缓存的 `latest.end_time == end`，复用整份状态，避免重复拉取和持久化；否则调用 `fetch_groups` 形成新 `DailyRuStats`，把旧状态的 `latest` 移到 `previous`，再调用 `persist_latest`。
5. `generate_rows` 为每个最新资源组计算 `read_ru + write_ru`。只有上期存在同名、同 ID 项时才减去上期累计值，否则把当前累计值视为本期增量。
6. 非有限值或小于 1 的增量被丢弃。若仍有行，调用 `backend.insert_rows`；没有行则跳过后端插入。
7. 返回生成的行；任一后端错误均立即中止并原样传播。

`gc_outdated_records` 先计算 `end - 92 天`，随后循环调用 `delete_before(cutoff, 1000)`。每次把返回数累加；当某批少于 1000 行时认为已清完并返回。后端必须遵守“返回实际删除数且不超过批大小”的契约，否则循环终止性无法保证。

`generate_sql` 对每行把时间转换为 UNIX 秒、把资源组名中的 `'` 替换成 `''`、把浮点 RU 截断为 `i64`，最后用逗号拼接为单条 `REPLACE INTO`。空输入返回空字符串。

## 数据与状态

所有统计对象都是拥有所有权的值类型；快照中的组以 `Vec` 保留原顺序和重复项。`generate_rows` 对上期组建立按名称排序的 `BTreeMap<&str, &GroupRuStats>`：上期同名重复项以后出现者覆盖先出现者；最新快照则逐项输出，因此最新同名重复项不会去重。`pkg/domain/ru_stats_test.rs::duplicate_latest_groups_and_integer_sql_match_go` 固定了这一行为。

`RuStatsWriter` 自身不缓存可变执行状态；`start_time` 是构造时提供的参考时钟，`write(&self)` 不会推进它。重试或下一周期必须由外层调用者更新或重新构造 writer。真正的持久状态位于后端：一份 `RuStats` 快照以及历史表行。

快照持久化发生在历史行插入之前。如果 `persist_latest` 成功而 `insert_rows` 失败，重试时因缓存终点相同会复用快照并再次生成相同行；是否幂等由后端插入语义保证。`generate_sql` 选择 `REPLACE INTO`，但 trait 的 `insert_rows` 契约没有强制具体后端采用该语句。

## 依赖与调用关系

上游接线为 `pkg/domain/lib.rs -> pub mod ru_stats`。RustCodeGraph 对 `generate_rows`、`generate_sql`、`get_last_expected_time` 的调用查询和仓库检索表明，当前明确调用边来自 `pkg/domain/ru_stats_test.rs`；没有发现 `RuStatsWriter` 被 Rust `Domain` 生产生命周期构造或调度。调用方若要使用主流程，需自行实现 `RuStatsBackend` 并安排周期与 owner 协调。

核心下游边为：

- `RuStatsWriter::write -> get_last_expected_time -> RuStatsBackend::{is_inserted, load_latest, fetch_groups, persist_latest, insert_rows}`；
- `RuStatsWriter::write -> generate_rows`；
- `RuStatsWriter::gc_outdated_records -> RuStatsBackend::delete_before`；
- `generate_rows -> BTreeMap` 与 `SystemTime::checked_sub`；
- `generate_sql -> SystemTime::duration_since(UNIX_EPOCH)` 和字符串转义/拼接。

与 Go 版相比，PD 查询时的 `InfoSchema` 过滤、KV meta 读写、restricted SQL 以及 owner 定时循环全部被移到 trait 实现或尚未接入的外层。`pkg/domain/Cargo.toml` 虽声明了相关 workspace 依赖，不能据此推断已经存在具体 `RuStatsBackend` 实现。

## 错误处理与边界

- `write` 显式拒绝 `interval == 0` 或 `interval > 24h`；`get_last_expected_time` 还拒绝亚秒区间（`as_secs() == 0`）。
- `SystemTime` 早于 UNIX 纪元、区间起点下溢或 GC 截止点下溢都会返回字符串错误。
- 六个后端方法均使用 `?` 立即传播错误，没有重试、日志、错误包装或补偿事务。
- `generate_rows` 将 `NaN`、正负无穷和小于 1 的 delta 静默过滤；累计值回退产生的负 delta 也因此跳过。恰好 1.0 会保留。
- 当前值和旧值先分别相加再相减；任何一步导致非有限结果都被过滤。同名但 ID 不同视为新资源组，不减旧累计。
- `generate_rows` 的 `checked_sub(interval)?` 位于 `filter_map` 内：时间下溢只会丢弃对应行，不会向调用者报告错误。不过 `write` 已先用同一终点和区间做过下溢校验，因此经主入口调用时不会触发该静默分支；直接调用该公开函数时会。
- `generate_sql` 对早于纪元的时间使用 `unwrap_or_default()`，降级为 0；它不返回错误。资源组名单引号被 SQL 标准方式加倍，但其他 SQL 合法性和数值范围由调用环境承担。
- GC 假设 `delete_before` 返回不大于 `batch_size` 的实际删除数；代码没有校验恶意或错误实现，也没有最大迭代次数。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、timer 或事务；所有 API 都是同步调用。`RuStatsWriter::write(&self)` 和 GC 使用共享引用，但能否跨线程共享完全取决于泛型后端是否实现 `Send`/`Sync`，trait 本身没有这些上界。

一次 `write` 没有覆盖“快照持久化 + 历史行插入”的原子事务边界，调用方应依靠区间去重和幂等插入处理失败重试。多个 writer 并发执行时，`is_inserted` 与 `insert_rows` 之间存在竞态；该层没有锁或 compare-and-set。生产后端和外层 owner 机制需负责单写者约束，或让目标表键与插入操作具备幂等性。

GC 会持续占用当前调用线程，直到后端返回少于一批；每批后端资源的获取、事务提交和释放都由 `delete_before` 实现管理。本层不保留连接或句柄。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/domain/ru_stats.go`，测试为 `pkg/domain/ru_stats_test.go`。

保持的核心语义包括：24 小时默认间隔、92 天保留期、1000 行 GC 批量、前后累计值做差、只对同名同 ID 资源组相减、增量小于 1 时跳过，以及 SQL 中把 RU 截断为整数。Rust 独立测试覆盖首期累计写入、同 ID 差值、重复最新组、单引号转义、重复区间跳过和常见区间对齐。

重要差异与迁移状态：

- Go `requestUnitsWriterLoop` 在 `Domain` 启动时注册，只由 DDL owner 执行，失败最多重试并等待退出或下一周期；Rust 没有对应生产循环、owner 判断、timer、退出信号、日志或重试。
- Go writer 直接从 PD 拉取并按最新 InfoSchema 过滤已存在资源组；Rust 只调用 `fetch_groups`，过滤责任未写入 trait 契约，当前仓库也未发现生产后端实现。
- Go 使用本地日界线，并在 UTC 中加 interval 以兼容 DST；Rust `get_last_expected_time` 始终按 UNIX 纪元秒数取整，不接受时区。UTC 和普通整除案例一致，但本地日界线及 DST 语义并不等价。Go 测试覆盖 `Asia/Shanghai` 与 `Australia/Lord_Howe`，Rust 测试仅覆盖 UTC 纪元算术。
- Go `isLatestDataInserted` 固定使用 24 小时计算查询起点，即使 writer 的 `Interval` 被测试修改；Rust 使用实际 `self.interval`，接口更一致但行为不完全相同。
- Go GC 先 `count(*)` 并据此执行有限批次数；Rust 重复删除直到不足一批，减少一次计数查询，但更依赖后端准确报告删除数。
- Go 直接拼接日期时间 SQL；Rust 主流程提交结构化 `RuStatsRow`，另提供尚未接入主流程的 UNIX 秒 SQL 生成器。因此 `generate_sql` 不是 Go 生产 SQL 的格式等价实现。
- Go 对缺失 `RUConsumption` 的组记录警告并跳过；Rust 数据结构用非可选浮点字段，不能表达该缺失状态。

## 扩展指南

接入真实 Rust Domain 时，优先新增独立生产后端类型实现 `RuStatsBackend`，在其中明确完成 PD 拉取与 InfoSchema 过滤、meta 快照读写、系统表区间去重、幂等批量插入和限量删除；不要把网络、KV 和 session 细节塞回 `generate_rows`。随后在 Domain 生命周期层增加 owner-only 的周期调度、退出处理、有限重试与日志，并补独立测试文件，不能把测试内嵌到 `ru_stats.rs`。

修改增量规则时应集中调整 `generate_rows`，并同步 `pkg/domain/ru_stats_test.rs` 中 ID 匹配、小增量、重复组和异常浮点边界。修改 SQL 表达时调整 `generate_sql` 及其转义/截断测试；若生产后端不使用它，应明确其定位或统一接线，避免两套序列化规则漂移。修改时间对齐时必须先决定要保持 Go 的本地时区/DST 合约还是确立 UTC 合约，并增加 DST 独立测试。修改 GC 时应验证整批、尾批、零删除、后端错误和错误返回超批大小的行为。

兼容风险主要是时间边界和资源组重建后的差值；正确性风险主要是并发重复写、两阶段持久化失败及非有限数据；性能风险主要是资源组全集复制、每次增量构造 `BTreeMap`，以及 GC 后端持续返回满批时的长循环。任何生产接线都应保留结构化后端边界，并用集成测试证明系统表与 meta 状态在失败重试后仍一致。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；使用 `node --file pkg/domain/ru_stats.rs` 阅读了完整 499 行，并对 `RuStatsWriter`、`generate_rows`、`generate_sql`、`get_last_expected_time` 及调用方/被调用方执行查询。
- 源码：`pkg/domain/ru_stats.rs`，重点为 `RuStatsBackend`、`RuStatsWriter::{write,gc_outdated_records}`、`get_last_expected_time`、`generate_rows`、`generate_sql`。
- crate 边界：`pkg/domain/Cargo.toml` 与 `pkg/domain/lib.rs`；后者公开 `ru_stats` 并仅在 `cfg(test)` 下装入 `ru_stats_test`。
- Rust 独立测试：`pkg/domain/ru_stats_test.rs`，覆盖同 ID 增量与 SQL 转义、重复最新组和整数 SQL、写入/去重主流程、UTC 区间对齐。
- Go 对照：`pkg/domain/ru_stats.go`、`pkg/domain/ru_stats_test.go`，以及 `pkg/domain/domain.go` 中 `do.wg.Run(do.requestUnitsWriterLoop, "requestUnitsWriterLoop")` 的生产启动边。
- 仓库检索未发现 Rust 生产代码调用上述入口；此结论表示“当前未检出接线”，不表示未来或动态调用不可能存在。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前另运行固定 11 章节结构检查并人工核对：本文区分了真实 Rust 行为、注释草稿、Go 对照和未接线能力。
