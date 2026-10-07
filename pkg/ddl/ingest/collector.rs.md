# `pkg/ddl/ingest/collector.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate；crate 入口 `pkg/ddl/ingest/lib.rs` 以 `pub mod collector` 公开它。它保存 DDL ingest 临时索引操作的进程内统计模型：写入按“连接 ID → 表 ID”分层，扫描与合并按表 ID 汇总，并可生成与 Go 指标标签兼容的快照。

这里不是 DDL job、schema state 或 checkpoint 的持久化点，也不参与回填调度。当前 Rust 仓库中，它尚未像 `pkg/ddl/ingest/collector.go` 那样注册到 Prometheus 或替换 `pkg/metrics/ddl.rs` 的全局回调；除 `pkg/ddl/ingest/collector_test.rs` 外未检索到直接使用者。因此它目前是已公开、已单测的迁移组件，而不是完整接入运行主链的指标端点。

## 核心职责

- 用 `Collector::add_temp_index_write` 将尚未提交的单写或双写次数记入指定连接和表。
- 用 `Collector::commit_temp_index_write` 将一个连接下所有表的暂存次数累加到已提交总数；用 `rollback_temp_index_write` 丢弃暂存次数。
- 用 `reset_temp_index_write` 清除某张表跨所有连接的写统计及其 scan/merge 统计；用 `clear_temp_index_write` 清除一个连接的全部写状态。
- 用 `set_temp_index_scan_and_merge` 累加一张表的扫描、合并次数。
- 用 `collect` 把已提交写计数和 scan/merge 计数转换为 `MetricSample` 列表；待提交写计数不会暴露。
- 用内部 `lock` 在互斥锁中毒后取回内部值，避免指标采集因另一线程 panic 而新增 Rust 特有的持续失败路径。

## 主要符号

- `METRIC_NAME` / `METRIC_HELP`：保留 Go Prometheus 描述符的名称 `tidb_ddl_temp_index_op_count` 与帮助文本。当前文件只提供常量，没有创建或注册描述符。
- `LABEL_SINGLE_WRITE`、`LABEL_DOUBLE_WRITE`、`LABEL_MERGE`、`LABEL_SCAN`：对应 Go 指标 `type` 标签的四个取值。
- `MetricSample { operation, table_id, value }`：公开快照项。`operation` 是上述静态标签之一，`table_id` 保留有符号 `i64`，`value` 是累计 `u64`。
- `MergeAndScan`：私有的每表 scan/merge 累计状态。
- `TableCollector`：私有的每连接、每表写状态，分别保存 single/double 的暂存计数与已提交总数。
- `Collector`：公开聚合器；`write` 为 `Mutex<BTreeMap<u64, BTreeMap<i64, TableCollector>>>`，`read` 为 `Mutex<BTreeMap<i64, MergeAndScan>>`。
- `lock<T>`：私有锁辅助函数，正常获取 `MutexGuard`，锁中毒时通过 `PoisonError::into_inner` 恢复访问。
- `Collector` 的七个公开方法：`add_temp_index_write`、`commit_temp_index_write`、`rollback_temp_index_write`、`reset_temp_index_write`、`clear_temp_index_write`、`set_temp_index_scan_and_merge`、`collect`。

## 执行流程

1. 调用者以 `(connection_id, table_id, double_write)` 调用 `add_temp_index_write`。方法创建缺失的连接/表条目，并只增加 single 或 double 暂存计数。
2. 事务成功时，`commit_temp_index_write(connection_id)` 遍历该连接的全部表，把两类暂存计数分别累加进 total，然后把暂存值清零。未知连接直接返回。
3. 事务失败时，`rollback_temp_index_write(connection_id)` 只清零该连接全部表的暂存值，已提交 total 保持不变。未知连接同样直接返回。
4. ingest 扫描/合并阶段可调用 `set_temp_index_scan_and_merge(table_id, scan_count, merge_count)`，按参数顺序分别累加 scan 与 merge。
5. `collect` 先遍历所有连接，将相同表的已提交 single/double total 聚合；随后读取每表 merge/scan；最后依次输出 single、double、merge、scan 样本。`BTreeMap` 使各类别内部按 `table_id` 排序，但接口没有声明跨类别以外的稳定性契约。
6. 表生命周期结束时可调用 `reset_temp_index_write(table_id)` 同时清理写侧与读侧状态；连接生命周期结束时可用 `clear_temp_index_write(connection_id)` 清理写侧状态。

## 数据与状态

`write` 的外层键是 `u64 connection_id`，内层键是 `i64 table_id`。暂存字段表达当前尚未提交的事务写入，total 字段表达此前提交的累计写入。只要表条目仍存在，`collect` 就会为 single 和 double 各产出一个样本，即使 total 为零；这由 `collector_test.rs::write_counts_follow_go_commit_and_rollback_lifecycle` 验证。

`read` 只按 `table_id` 保存累计值，不按连接或事务分组，也没有 commit/rollback 阶段。`set_temp_index_scan_and_merge` 连续调用会累加；测试 `scan_and_merge_accumulate_with_go_label_order` 验证 `(2, 7)` 与 `(3, 11)` 最终产生 scan=5、merge=18。

所有计数使用 `wrapping_add`。达到 `u64::MAX` 后会按模 $2^{64}$ 回绕，而不会 panic、饱和或返回错误。状态只驻留内存，没有序列化、checkpoint 或重启恢复能力。

## 依赖与调用关系

本文件的实现依赖仅来自标准库：`BTreeMap`、`Mutex`、`MutexGuard`。虽然 `pkg/ddl/ingest/Cargo.toml` 声明了 `fs2`、`fail` 和 `astersql-util-dbterror` 等 crate 级依赖，但本文件没有使用它们；Prometheus crate 也不在该文件的依赖或实现中。

上游模块边为 `pkg/ddl/ingest/lib.rs -> pub mod collector`。RustCodeGraph 将本文件索引为 21 个符号，并确认 `Collector` 及主要方法位于本文件；仓库文本检索显示直接方法调用来自 `pkg/ddl/ingest/collector_test.rs`。下游调用只包括 `lock`、`BTreeMap` 的 entry/遍历操作和整数回绕加法，没有 DDL scheduler、backend、checkpoint、存储或网络调用。

Go 运行链则由 `pkg/ddl/ingest/collector.go::init` 把 `metrics.DDLAddOneTempIndexWrite` 等回调绑定到包级 `coll`，并通过 `prometheus.MustRegister(coll)` 注册。Rust 对应的 `pkg/metrics/ddl.rs` 仍定义空操作函数指针；本文件没有连接这些函数指针。扩展时必须把“模型已存在”与“生产接线已完成”区分开。

## 错误处理与边界

- 所有公开方法均不返回 `Result`。未知连接的 commit/rollback 是幂等空操作；删除不存在的连接或表也是空操作。
- `lock` 显式恢复 poisoned mutex 内部值，优先保持指标路径可用；它不修复可能因 panic 留下的业务层中间状态，因此调用者不能把指标精确性当作事务正确性的依据。
- `reset_temp_index_write` 先锁写侧、遍历并移除表项，写锁 guard 在该 `for` 语句结束后释放，再获取读锁；它不会同时持有两把锁。
- `collect` 先完成写侧聚合并释放写锁，再持有读锁组装样本，所以结果不是 write/read 两域同一瞬间的原子快照。监控使用者应接受并发采集时的弱一致性。
- `table_id` 可为负数；Rust 测试以 `-5` 验证其可作为键和样本值。计数输入只能是非负 `u64`。
- `collect` 不输出只有 read 状态的 single/double 样本，也不输出只有 write 状态的 merge/scan 样本。

## 并发与资源生命周期

`Collector` 通过两把独立 `Mutex` 支持多线程共享，通常以 `Arc<Collector>` 传递。写统计的所有连接共享一把 write 锁，read 统计共享另一把 read 锁；单次方法修改在对应域内互斥，但并发度不等同于 Go 的 `sync.Map` 加 per-counter atomic 实现。连接数、表数或写入频率很高时，Rust 的全局写锁可能成为竞争点。

`collector_test.rs::concurrent_connections_aggregate_without_losing_counts` 用 8 个线程、每线程 1000 次写入后提交，验证并发聚合不丢计数。`Collector` 没有后台任务、通道、文件句柄或显式 `Drop`；资源随实例及其 `Arc` 引用释放。若长期存活，调用方必须适时使用 reset/clear，避免连接和表条目持续增长。

## 与 Go 版本的对应关系

Rust 的状态维度、四个标签、commit/rollback/reset/clear 语义、scan/merge 参数顺序以及采集时只统计 total 的规则，都直接对应 `pkg/ddl/ingest/collector.go`。`collector_test.rs` 的前三个测试分别覆盖 Go 的事务生命周期、连接/表清理范围与 scan/merge 标签顺序。

存在以下明确差异：

- Go 有包级单例 `coll`、`init()` 自动注册、Prometheus `Describe`/`Collect` 实现，并改写 `pkg/metrics/ddl.go` 的函数变量；Rust 只提供可构造的 `Collector` 和 `Vec<MetricSample>` 快照。
- Go 使用 `sync.Map` 和 `atomic.Uint64`，不同连接/表更新可更细粒度并行；Rust 使用两把聚合级 `Mutex` 与普通 `u64`。
- Go 采集通过 channel 发出 `prometheus.Metric`，表 ID转成十进制标签字符串；Rust 样本保留 `i64 table_id`，没有构造 Prometheus metric。
- Go map 遍历顺序不保证；Rust 用 `BTreeMap`，使同类样本按表 ID 有序。这是实现特性，不应被上层当作指标语义。
- Go 原子加法与 Rust `wrapping_add` 都允许无错误返回的累计，但文档没有承诺溢出后的业务含义；扩展时应保持两端一致并补边界测试。

## 扩展指南

- 若新增操作类型，需同步更新标签常量、对应内部状态、变更方法、`collect` 输出以及独立测试 `pkg/ddl/ingest/collector_test.rs`；同时核对 Go `collector.go` 和 Grafana 查询是否要求兼容标签。
- 若完成生产接线，应在独立改动中实现 Prometheus collector/注册和 `pkg/metrics/ddl.rs` 回调绑定，并验证初始化顺序与单例生命周期；不能仅凭 `METRIC_NAME` 常量宣称已有指标暴露。
- 若改变 commit/rollback 语义，应保持“暂存计数不在 collect 中出现、commit 转入 total、rollback 不影响 total”的不变量，并增加跨多表、重复 commit/rollback 的测试。
- 若优化锁粒度，应特别验证 reset 与 collect 并发、锁中毒策略和弱一致快照边界；避免形成 write/read 反向加锁导致死锁。
- 若引入错误返回或溢出策略，必须评估与 Go 无返回回调接口的兼容性，并添加接近 `u64::MAX` 的独立回归测试。
- Rust 测试必须继续放在 `collector_test.rs`，不要内嵌回生产源文件。

## 验证依据

- 源码：`pkg/ddl/ingest/collector.rs`（常量、`MetricSample`、内部状态、`lock`、`Collector` 七个方法）。
- crate 边界：`pkg/ddl/ingest/lib.rs` 的 `pub mod collector` 与 `#[cfg(test)] mod collector_test`；`pkg/ddl/ingest/Cargo.toml` 的包名、库入口和依赖声明。
- Go 对照：`pkg/ddl/ingest/collector.go` 的 `init`、`collector`、`newCollector`、`Describe`、`Collect`；回调声明位于 `pkg/metrics/ddl.go`，Rust 空操作声明位于 `pkg/metrics/ddl.rs`。
- 独立 Rust 测试：`pkg/ddl/ingest/collector_test.rs` 的 `write_counts_follow_go_commit_and_rollback_lifecycle`、`reset_and_clear_match_go_connection_and_table_scopes`、`scan_and_merge_accumulate_with_go_label_order`、`concurrent_connections_aggregate_without_losing_counts`。
- RustCodeGraph：`status` 显示索引覆盖 11467 个文件；`files --filter pkg/ddl/ingest` 列出目标 Rust/Go/测试文件；`query Collector` 定位 `collector.rs:57`，对主要方法的 query 定位 `add_temp_index_write:72`、`commit_temp_index_write:87`、`set_temp_index_scan_and_merge:130`。图查询未给出生产调用边，仓库 `rg` 检索仅发现 Rust 测试调用，因此本文将生产接线明确标为尚未验证/尚未实现。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前只执行固定 11 章节结构检查和文档差异自审。
