# `pkg/util/topsql/collector/mock/mock.rs`

## 文件定位

本文件实现 `astersql-util-topsql-collector-mock` crate 的核心逻辑，是 TopSQL 测试链路中的内存型 collector，而不是生产环境的远端 reporter。crate 入口 `pkg/util/topsql/collector/mock/lib.rs` 通过 `mod mock; pub use mock::*;` 导出这里的公开 API；`pkg/util/topsql/collector/mock/Cargo.toml` 则声明它依赖真实的 `collector`、`stmtstats`、`parser` 和 `logutil` crate，因此测试使用的是正式接口和正式 SQL digest 算法，而非另造一套类型。

它位于“SQL/Plan 元数据注册及 CPU 采样结果”与“测试断言”之间：被测 TopSQL 链路可把统计送入 `collector::Collector::Collect`，测试随后通过 `GetSQLStatsBySQL*`、`GetSQLCPUTimeBySQL`、`GetSQL` 和 `GetPlan` 读取内存快照。当前 Rust 直接接线证据包括 `pkg/server/tests/servertestkit/testkit.rs` 对 `TopSQLCollector` 的使用，以及本 crate 的独立测试 `pkg/util/topsql/collector/mock/migration_aster_unit_test.rs`；Go 版本还广泛用于 `pkg/util/topsql/topsql_test.go`、`pkg/server/tests/commontest/tidb_test.go` 和 `pkg/executor/adapter_internal_test.go`。

## 核心职责

1. `NewTopSQLCollector` 构造可通过 `Arc` 跨组件和线程共享的空 collector。
2. `RegisterSQL`、`RegisterPlan` 保存 digest 到规范化 SQL/plan 文本的首次映射，供后续测试按采样记录反查元数据。
3. `Collect` 按 `SQLDigest || PlanDigest` 聚合 `SQLCPUTimeRecord::CPUTimeMs`，并记录每次收集调用；空批次也计数。
4. `GetSQLStatsBySQL`、`GetSQLStatsBySQLWithRetry` 和 `GetSQLCPUTimeBySQL` 把测试输入的 SQL 先经真实 parser 归一化并生成 digest，再筛选或汇总统计。
5. `Reset` 提供测试用例之间的状态隔离；`Start`、`Close`、`BindProcessCPUTimeUpdater`、`BindKeyspaceName` 和 `CollectStmtStatsMap` 以无副作用实现满足测试所需接口表面。

该文件有意模拟 Go mock 的可观察行为，包括任意字节 digest、首次注册获胜、超大 plan 被忽略、`u32` CPU 时间回绕以及无分隔符 digest 拼接可能产生的键碰撞；它不负责真实采样、后台上报、容量淘汰或持久化。

## 主要符号

- `CollectorState`：私有状态容器。`sql_map: HashMap<Vec<u8>, String>` 和 `plan_map` 保存元数据，`sql_stats_map: HashMap<Vec<u8>, collector::SQLCPUTimeRecord>` 保存聚合记录。键使用 `Vec<u8>`，保持 Go `string([]byte)` 可容纳任意字节的语义。
- `TopSQLCollector`：公开 collector 类型。`state: Mutex<CollectorState>` 串行保护三张关联表，`collect_cnt: AtomicI64` 独立记录 `Collect` 调用次数。
- `NewTopSQLCollector() -> Arc<TopSQLCollector>`：创建默认状态并直接返回共享所有权对象。
- `Collect(&self, Vec<SQLCPUTimeRecord>)`：核心写路径；按私有 `hash` 产生键，保留首条记录的两个 digest，只累加 CPU 时间。
- `RegisterSQL` / `RegisterPlan`：元数据写路径。两者都使用 `HashMap::entry(...).or_insert(...)` 保留首次值；`RegisterPlan` 在 `isLarge` 为真时不写入，`RegisterSQL` 当前忽略 `_is_internal`。
- `GetSQLStatsBySQL`：按 `GenSQLDigest(sql)` 匹配 SQL digest；`planIsNotNull` 为真时进一步要求相应 plan digest 已注册且文本非空。
- `GetSQLStatsBySQLWithRetry`：最长十秒轮询上述查询；无结果时调用 `WaitCollectCnt(1)` 等待至少一次新的收集。
- `GetSQLCPUTimeBySQL`：跨所有 plan 汇总同一 SQL 的 CPU 毫秒数，使用 `wrapping_add`。
- `GetSQL` / `GetPlan`：按原始字节 digest 查询文本，缺失时返回空字符串。
- `WaitCollectCnt` / `CollectCnt` / `Reset`：分别等待相对计数目标、读取当前计数、清空所有状态并归零。
- `impl collector::Collector` 与 `impl stmtstats::Collector`：将正式 trait 方法转发到同名固有方法；前者进入真实聚合逻辑，后者当前是空实现。
- `GenSQLDigest(&str) -> parser::Digest`：调用 `parser::NormalizeDigest` 并返回 digest。

文件没有模块级业务常量、条件编译项或自定义错误类型；唯一条件编译发生在 `lib.rs`，它仅在测试配置下挂载独立的 `migration_aster_unit_test.rs`。

## 执行流程

典型测试流程如下：

1. 测试调用 `NewTopSQLCollector`，把得到的 `Arc<TopSQLCollector>` 接入被测 TopSQL collector/reporter 边界。
2. 被测路径通过 `RegisterSQL` 和 `RegisterPlan` 注册 digest 元数据。重复 digest 不覆盖旧值；标记为大 plan 的数据直接丢弃。
3. CPU 采样批次经 `collector::Collector::Collect` 进入 trait 转发层，再调用固有 `TopSQLCollector::Collect`。
4. 非空批次持有 `state` 锁，逐条把 SQL digest 和 plan digest 无分隔拼接为键。首次出现时复制两个 digest 并将 CPU 初值设为零，随后用回绕加法累积本次 CPU 时间；同时从元数据表取 SQL 和 plan 存在性写一条背景日志。
5. `Collect` 在空批次时直接增加 `collect_cnt`；非空批次则先释放 map 锁，再增加计数。这与 Go 中延迟执行的计数操作和解锁顺序一致。
6. 断言侧可调用 `GetSQLStatsBySQLWithRetry`：它先查询现有聚合记录；若为空，则以当前计数为基准等待下一次 `Collect`，循环直至得到结果或外层十秒期限到达。
7. 测试结束后调用 `Reset`。`pkg/server/tests/servertestkit/testkit.rs::TidbTestTopSqlSuite::test_case` 明确在停止并回收后台执行线程之后执行该重置。

`GetSQLStatsBySQL` 返回的是记录克隆，调用方不能借返回值绕过锁修改 collector 内部状态。`HashMap` 遍历次序未定义，因此多 plan 查询不承诺结果顺序；现有测试只在结果唯一或不依赖顺序时做等值断言。

## 数据与状态

三张表和计数器的关系是：

- `sql_map[SQLDigest] = normalized SQL`；
- `plan_map[PlanDigest] = normalized plan`；
- `sql_stats_map[SQLDigest || PlanDigest] = {首次 SQLDigest, 首次 PlanDigest, 累计 CPUTimeMs}`；
- `collect_cnt` 是收集批次数，而不是记录条数。

重要不变量是注册表“首次值获胜”，统计表“相同拼接键共用一条记录”。因为聚合键没有长度前缀或分隔符，`("a", "bc")` 与 `("ab", "c")` 会碰撞；发生碰撞时保留第一条记录的 digest 对，并把后续 CPU 时间加到该记录。这不是理想哈希设计，而是对 Go `string(SQLDigest) + string(PlanDigest)` 的兼容，已由 `collect_preserves_go_digest_concatenation_hash_behavior` 明确验证。

`Reset` 在同一个 `Mutex` 临界区内用默认值整体替换 `CollectorState`，随后把原子计数归零。状态只存在于进程内存，collector 被释放或重置后不保留任何信息。

## 依赖与调用关系

`Cargo.toml` 给出的直接依赖及用途如下：

- `collector`（路径 `..`）：提供 `Collector`、`SQLCPUTimeRecord` 和 `ProcessCPUTimeUpdater`；本文件实现其 collector trait 并保存其记录类型。
- `stmtstats`（路径 `../../stmtstats`）：提供 `stmtstats::Collector` 和 `StatementStatsMap`；当前 trait 入口被接受但不存储。
- `parser`（路径 `../../../../parser`）：以 `digester_impl` 别名使用，`GenSQLDigest` 委托 `NormalizeDigest`。
- `logutil`（路径 `../../../logutil`）：`Collect` 通过 `BgLogger` 输出 SQL 文本和是否存在 plan 两个字段。
- Rust 标准库：`HashMap` 保存数据，`Arc` 共享实例，`Mutex` 保护复合状态，`AtomicI64` 管理计数，`Instant`/`Duration`/`thread::sleep` 实现超时轮询。

RustCodeGraph 的直接内部调用边为：`Collect -> hash`，trait `Collector::Collect -> TopSQLCollector::Collect`，`GetSQLStatsBySQLWithRetry -> GetSQLStatsBySQL`、`WaitCollectCnt`，`GetSQLStatsBySQL -> GenSQLDigest`，`GetSQLCPUTimeBySQL -> GenSQLDigest`，以及 `stmtstats::Collector::CollectStmtStatsMap ->` 固有同名空实现。

上游方面，`pkg/server/tests/servertestkit/testkit.rs` 依赖此 crate，并在 `TidbTestTopSqlSuite::test_case` 接受 `&TopSQLCollector` 后调用 `Reset`。工作区 Cargo 接线还见于根 `Cargo.toml`、`pkg/server/tests/servertestkit/Cargo.toml`、`pkg/server/tests/commontest/Cargo.toml` 和 `pkg/executor/Cargo.toml`。图索引没有给 Rust 版 `NewTopSQLCollector` 列出本 crate 测试之外的构造调用，因此不能据此声称所有 Go 集成场景已迁移到 Rust。

## 错误处理与边界

- 所有 `Mutex::lock` 都使用 `expect("mock TopSQLCollector state lock poisoned")`。持锁线程 panic 导致锁中毒后，后续访问也会 panic；作为测试辅助类型，它没有恢复或返回 `Result` 的路径。
- `GetSQL`、`GetPlan` 在缺失时返回空串；查询统计超时时返回空向量；`WaitCollectCnt` 超时时静默返回。调用方必须根据返回内容或计数自行判断是否满足预期。
- 两种等待都以十秒为上限并每十毫秒 sleep，没有条件变量通知。`GetSQLStatsBySQLWithRetry` 内部调用的 `WaitCollectCnt(1)` 自身也可等待十秒，所以外层 deadline 只在每轮开始检查，实际返回时间可能略超过十秒。
- `WaitCollectCnt(count)` 将“当前计数 + count”作为目标并用 `i64::wrapping_add` 计算；正常测试应传非负小值。负值会使条件立即满足，极端溢出也遵循回绕而不是报错。
- `CPUTimeMs` 的单条聚合和跨 plan 汇总均为 `u32::wrapping_add`，对齐 Go `uint32` 溢出行为，不提供饱和或溢出诊断。
- `RegisterPlan(..., isLarge = true)` 完全忽略输入；空 plan 即使被注册，在 `planIsNotNull = true` 查询中仍视为“无 plan”。
- 拼接键存在已知碰撞边界；改变它会改变 Go 兼容语义及现有回归测试，不能作为局部“修正”直接实施。

## 并发与资源生命周期

`Arc<TopSQLCollector>` 提供共享所有权；所有三张 map 由单个 `Mutex<CollectorState>` 保护，因此注册、聚合、查询和重置之间不存在未同步 map 访问。复合读取（例如统计记录与 `plan_map` 的过滤）也在同一锁内完成，可看到一致的临界区快照。`collect_cnt` 使用 `SeqCst` 原子顺序；它不需要持有状态锁即可被等待线程读取。

非空 `Collect` 的状态锁作用域被显式块限定，计数只在解锁后增加。这样，观察到新计数的等待方随后取得状态锁时，相关批次数据已写完。空批次没有状态写入，只增加计数。`Reset` 的 map 清空与计数归零不是对其他线程的单一原子事务：它先在锁内替换状态，再执行原子 store；因此测试应像 `TidbTestTopSqlSuite::test_case` 一样先停止并 join 生产线程，再重置，避免把并发写入跨用例混合。

等待机制采用主动轮询和线程 sleep，不创建后台任务、通道或异步 runtime 资源。`Start`/`Close` 不启动或回收资源，类型也没有自定义 `Drop`。真正的采样器、线程和 reporter 生命周期由调用方负责。

## 与 Go 版本的对应关系

同目录 `mock.go` 是直接语义基线，Rust 实现保留了其公开的 Go 风格命名（文件级 `#![allow(non_snake_case)]`）和主要行为：

- Go 的三张 map 加嵌入式 `sync.Mutex` 对应 Rust 的单个 `Mutex<CollectorState>`；Go `atomic.Int64` 对应 `AtomicI64`。
- Go 用字符串承载 digest 字节，Rust 用 `Vec<u8>`，避免非 UTF-8 digest 的有损转换；独立 Rust 测试以 `0x80/0xff` 等字节验证该差异仍保持语义等价。
- Go `defer c.collectCnt.Inc()` 使空批次也计数；Rust 在空/非空分支分别递增，并保持非空路径先解锁、后计数。
- Go map 的“先检查是否存在”对应 Rust `entry(...).or_insert(...)`；二者均保留首个 SQL/plan 文本。
- Go 的 `uint32` 自然回绕对应 Rust 显式 `wrapping_add`。
- Go 返回内部记录指针切片，Rust 返回克隆后的 `Vec<SQLCPUTimeRecord>`，所有权更安全，但测试观察到的字段值相同。
- Go `time.After` 加 sleep 对应 Rust `Instant` deadline 加 `thread::sleep`；两者都以约十秒为界且没有错误返回。
- Go `BindProcessCPUTimeUpdater` 接收接口值，Rust 接收 `Arc<dyn ProcessCPUTimeUpdater>`；二者当前均忽略参数。

Go 集成测试补充了用途证据：`pkg/util/topsql/topsql_test.go::TestTopSQLCPUProfile` 用重试查询检查 SQL/plan/CPU 链，`TestMaxSQLAndPlanTest` 检查大 plan 不注册；`pkg/server/tests/commontest/tidb_test.go` 用它验证运行中 SQL、不同 plan 及 CPU 时间比较；`pkg/executor/adapter_internal_test.go::TestObserveStmtBeginOnTopProfiling` 检查执行开始时的 SQL/plan 注册。Rust 当前独立测试覆盖 mock 自身的移植语义，但不能把这些 Go 端到端测试自动视为已在 Rust 侧全部复刻。

## 扩展指南

- 若新增 collector 可观察字段或统计维度，应先修改 `CollectorState` 和 `Collect`，再为查询提供返回克隆或聚合值的只读方法；保持所有关联状态在同一个锁内，避免跨表快照撕裂。
- 若扩展正式 `collector::Collector` 或 `stmtstats::Collector` trait，需同步本文件对应 impl。特别是实现语句级统计时，不能继续让 `CollectStmtStatsMap` 静默丢弃，并应增加独立测试文件中的并发、合并和重置覆盖。
- 若改变等待策略，可考虑条件变量以减少轮询，但必须保持“写完状态后再发布计数/通知”的 happens-before 关系，并验证超时、空批次和重置竞争。
- 若准备修复 digest 拼接碰撞，必须作为明确的 Go/Rust兼容行为变更，同时更新 `hash`、Go `mock.go` 及 `collect_preserves_go_digest_concatenation_hash_behavior`；仅在 Rust 侧加入分隔符会造成测试替身分叉。
- 若改变 SQL/plan 注册覆盖规则、大 plan 处理、CPU 溢出或空值语义，应同步 `pkg/util/topsql/collector/mock/migration_aster_unit_test.rs`，并核对 `pkg/util/topsql/topsql_test.go`、`pkg/server/tests/commontest/tidb_test.go` 与 `pkg/executor/adapter_internal_test.go` 的原始意图。
- Rust 单元测试必须继续放在独立文件，不要嵌入 `mock.rs`。新测试优先追加到现有 `migration_aster_unit_test.rs`，由 `lib.rs` 的 `#[cfg(test)]` 接线。
- 性能风险主要来自单全局 mutex、逐记录日志和十毫秒轮询；它们对测试 mock 通常可接受，但批量或高并发扩展前应量化，不应把该类型误用作生产 collector。

## 验证依据

本说明基于以下直接证据：

- 目标实现：`pkg/util/topsql/collector/mock/mock.rs`（RustCodeGraph 文件节点显示完整 307 行、24 个符号）。
- crate 边界：`pkg/util/topsql/collector/mock/Cargo.toml`；模块导出与测试接线：`pkg/util/topsql/collector/mock/lib.rs`。
- Rust 独立测试：`pkg/util/topsql/collector/mock/migration_aster_unit_test.rs`，覆盖二进制 digest、首次注册、大 plan、CPU 聚合、plan 过滤、空批次计数、拼接碰撞、reset、真实 digest 依赖和空接口方法。
- Go 对照：`pkg/util/topsql/collector/mock/mock.go`。
- Go 调用与测试：`pkg/util/topsql/topsql_test.go`、`pkg/server/tests/commontest/tidb_test.go`、`pkg/executor/adapter_internal_test.go`。
- Rust 上游接线：`pkg/server/tests/servertestkit/testkit.rs` 及其 Cargo 清单；工作区依赖搜索还命中根 `Cargo.toml`、`pkg/server/tests/commontest/Cargo.toml` 和 `pkg/executor/Cargo.toml`。
- RustCodeGraph 状态：索引覆盖 11,467 个文件，目标目录包含 `lib.rs`、`migration_aster_unit_test.rs`、`mock.go`、`mock.rs`；`node/query/callees` 核实了本文列出的核心符号和内部调用边。精确 `callers/callees` 对重名 `Collect` 的结果存在名称扩张，因此上游调用结论同时以 Cargo 和 `rg` 的精确 crate/符号引用核验，未把不相干同名结果计入。

本任务是纯文档分析，按计划不运行 Cargo。最终结构检查要求本文恰好包含规定的十一个二级标题；人工复核重点是所有“已支持”陈述均能回指上述源文件、调用边或测试，且未把尚未迁移的 Go 集成场景描述为 Rust 现状。
