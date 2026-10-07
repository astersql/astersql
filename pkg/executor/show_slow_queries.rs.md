# `pkg/executor/show_slow_queries.rs`

## 文件定位

该文件属于 `astersql-executor` crate，由 `pkg/executor/lib.rs` 以公开模块 `show_slow_queries` 装配。它承载 `ADMIN SHOW SLOW` 结果物化与分批写入 `Chunk` 的 Rust 核心模型，对照实现是同目录的 `show_slow_queries.go`。

当前接线状态需要特别区分：`pkg/executor/builder.rs` 能把 `Plan::ShowSlow` 映射为 `ExecutorKind::ShowSlow` 并交给 `ExecutorBuildDependencies::build_executor`，但仓库内生产代码没有 `ShowSlowSource` 的实现，也没有把本文件的泛型 `ShowSlowExec<S>` 构造成 `ExecutorBox` 的代码；仅 `pkg/executor/show_slow_queries_test.rs` 提供两个测试数据源。因此，本文件已经实现并测试执行器核心，但不能据此宣称 Rust 主执行链已经接通 Go `Domain.ShowSlowQuery`。

## 核心职责

- `SlowQueryInfo` 定义本执行器消费的一条慢查询快照，覆盖 SQL、时间、执行详情、连接/事务、用户/库表、内部查询标志、digest、session alias 和 IA remote-read 扫描统计。
- `ShowSlowSource` 把环境相关行为隔离为打开数据源、按请求取全量结果、给出单批最大行数三个操作。
- `ShowSlowExec::Open` 先打开数据源，再按 `show_slow` 请求一次性取得并缓存结果。
- `ShowSlowExec::Next` 从 `cursor` 开始，把缓存记录按固定的 17 列布局写入调用方提供的 `Chunk`，直到结果耗尽或达到数据源报告的 chunk 上限。

文件不负责解析 `ADMIN SHOW SLOW` 语法、选择 recent/top/internal/all 的具体算法，也不维护慢查询 Top-N；这些策略应由请求类型与 `ShowSlowSource::show_slow_query` 的实现提供。

## 主要符号

- `SlowQueryInfo`：可克隆的数据传输结构。`start` 和 `duration` 已经是 SQL 层 `Time`/`Duration`，`detail` 已经格式化成字符串；`scan_detail: Option<ScanDetail>` 用于派生最后三列。它与 `pkg/domain/domain.rs`、`pkg/domain/topn_slow_query.rs` 中同名但字段不同的结构不是同一个类型。
- `ShowSlowSource`：公开 trait，关联类型 `Request` 和 `Error` 让调用方决定请求表示和打开阶段的错误类型。`show_slow_query` 返回 `Vec<SlowQueryInfo>`，接口本身不能返回错误。
- `ShowSlowExec<S>`：公开泛型执行器，持有 `source`、请求 `show_slow`、全量缓存 `result` 和下一行位置 `cursor`。这些字段均公开，构造与初始状态约束由外部负责。
- `ShowSlowExec::Open<C>`：上下文参数当前未使用；调用 `source.open()?`，成功后以 `source.show_slow_query(&show_slow)` 覆盖 `result`。它不重置 `cursor`。
- `ShowSlowExec::Next<C>`：上下文参数当前未使用；先 `Chunk::Reset`，再逐行追加 17 列并递增 `cursor`。返回类型与数据源错误类型保持一致，但当前函数体没有产生或传播错误的调用。
- `GetIARemoteReadSegmentStats`：来自 `astersql-util-execdetails`，把可选 `ScanDetail` 映射成 count、bytes、wait time；`None` 或默认详情得到三个零值。

## 执行流程

1. 上游应创建 `ShowSlowExec`，传入具体 `ShowSlowSource`、请求对象，并通常以空 `result`、`cursor = 0` 初始化。
2. `Open` 调用 `source.open()`。若失败，错误立即返回，原有 `result` 不会被本次调用覆盖；若成功，则同步调用 `show_slow_query` 并把返回向量整体存入 `result`。
3. 每次 `Next` 首先清空输出 `Chunk`，不会保留调用前的行。
4. 循环条件同时要求 `cursor < result.len()` 与 `req.NumRows() < source.max_chunk_size()`。每轮按固定序号写入：SQL(0)、开始时间(1)、耗时(2)、详情(3)、成功标志(4)、连接 ID(5)、事务时间戳(6)、用户(7)、数据库(8)、表 ID(9)、索引名(10)、内部查询标志(11)、digest(12)、session alias(13)、IA 远程读段数(14)、字节数(15)、等待秒数(16)。
5. 布尔值通过 `i64::from` 输出为 0/1；IA 等待时间通过 `as_secs_f64()` 输出为秒。每写完一行才递增 `cursor`。
6. 达到批大小后返回；下一次 `Next` 从保存的 `cursor` 继续。结果耗尽时，`Next` 只留下已重置的空 chunk 并返回成功。

## 数据与状态

执行器的持久状态是 `result` 与 `cursor`。`result` 是 `Open` 时的全量快照，后续数据源变化不会反映到已经打开的执行器；内存占用随匹配慢查询条数及字符串/详情大小线性增长。`cursor` 是跨 `Next` 调用的消费位置，不会被 `Open` 重置，这一点由 `open_preserves_cursor_like_go_executor` 明确测试；复用实例时，调用方必须自行决定是否清零。

输出 schema 是重要不变量：`Next` 无条件访问第 0 至 16 列，调用方必须提供至少 17 个类型兼容的列。源码自身不验证列数和字段类型。`max_chunk_size()` 也由数据源控制；若它返回 0，即使尚有结果，本次调用也不会推进游标。

`scan_detail` 只参与最后三列计算。`GetIARemoteReadSegmentStats` 对缺失详情返回默认零值，因此普通慢查询仍产生完整的 17 列，而不是空值。

## 依赖与调用关系

- crate 边界：`pkg/executor/Cargo.toml` 声明库入口为 `lib.rs`，并直接依赖 `astersql-types`、`astersql-util-chunk` 与 `astersql-util-execdetails`；本文件分别使用其 SQL 时间类型、列式结果容器和扫描详情转换函数。
- 模块入口：`pkg/executor/lib.rs` 公开 `show_slow_queries`，并仅在测试配置下装入 `show_slow_queries_test`。
- 规划/构建上游：`pkg/executor/builder.rs` 的 `Plan::ShowSlow` 分支调用 `buildShowSlow`，再以 `ExecutorKind::ShowSlow` 调用通用 `build_leaf`/依赖工厂。这证明 SHOW SLOW 有构建种类，但没有证明工厂使用本文件的泛型类型。
- 本文件直接下游：`ShowSlowExec::Open` 调用 trait 的 `open`、`show_slow_query`；`Next` 调用 `Chunk` 的 reset/append API、数据源的 `max_chunk_size` 和 `GetIARemoteReadSegmentStats`。
- 已确认调用者：RustCodeGraph 能定位本文件的三个核心符号及测试导入，但 callers/callees 未返回可用调用边；`rg` 复核显示 `ShowSlowExec`/`ShowSlowSource` 的 Rust 实例化仅出现在独立测试中。

## 错误处理与边界

`Open` 唯一可传播的错误来自 `ShowSlowSource::open`。发生错误时不会调用查询方法，也不会替换缓存；错误类型完全由具体数据源决定。相反，`show_slow_query` 被设计为无错误返回，无法表达查询阶段失败；若未来真实数据源需要失败语义，应先评估是否修改 trait 签名及所有实现。

`Next` 声明返回 `Result<(), S::Error>` 以保持执行器接口形态，但当前路径始终返回 `Ok(())`。越界列、错误字段类型或底层 `Chunk` 行为不在这里转成 `S::Error`。代码也不检查 `cursor` 是否因外部直接写入而异常，只要它大于等于长度就按结果耗尽处理。

时间转换边界已经前移到 `SlowQueryInfo` 的生产者：本文件不把系统时间转换为 SQL timestamp，也不设置 FSP。等待时间用浮点秒输出，需接受 IEEE 754 表示带来的常规精度限制。

## 并发与资源生命周期

本文件没有锁、原子量、任务或通道，也没有 `unsafe`。`Open` 和 `Next` 都要求 `&mut self`，同一实例的状态推进是串行的；是否跨线程共享由泛型数据源及外部同步策略决定，本文件没有声明或保证额外的并发语义。

数据源生命周期由 `ShowSlowExec` 所有；trait 只有 `open`，没有 `close`。查询结果以拥有所有权的 `Vec` 和 `String` 缓存在执行器中，随执行器释放。输出行被追加到调用方拥有的 `Chunk`，下一次调用会先 reset 该 chunk。Go 版本从 Domain 的消息通道同步取得快照，但 Rust 本文件没有对应通道接线，不能把 Go 的并发保证推定到 Rust trait 实现。

## 与 Go 版本的对应关系

`pkg/executor/show_slow_queries.go` 的 `ShowSlowExec.Open` 先打开 `BaseExecutor`，再通过 `domain.GetDomain(e.Ctx()).ShowSlowQuery(e.ShowSlow)` 取得结果；Rust 用 `ShowSlowSource` 抽象这两步，但当前没有生产适配器。两版 `Next` 的分页条件、cursor 递增时机、0/1 布尔编码和 17 列顺序一致，最后三列都由 `GetIARemoteReadSegmentStats` 得出。

表示层存在刻意差异：Go 在 `Next` 内把 `time.Time`/`time.Duration` 转成 TiDB SQL 时间类型并把结构化详情格式化为字符串；Rust 的 `SlowQueryInfo` 已持有 `astersql_types::datum::Time`、`Duration` 和 `String`，转换责任属于数据源。Go 的 `SlowQueryInfo.Detail` 同时携带扫描详情；Rust 将展示字符串 `detail` 与 `scan_detail` 分开保存。

Go `Open` 同样不显式重置 cursor，Rust 测试 `open_preserves_cursor_like_go_executor` 固化了这一行为。Go 测试 `TestAdminShowSlowIARemoteReadStats` 还覆盖 recent/top/internal/all 命令、17 列元数据、缺失统计为零和 `4/4096/0.015` 输出；Rust 测试目前只覆盖一行 IA 数据的最后三列和 Open 保留 cursor，尚未端到端覆盖请求筛选与 Domain 接线。

## 扩展指南

- 接入生产主链时，优先新增实现 `ShowSlowSource` 的适配器，并在执行器依赖工厂的 `ExecutorKind::ShowSlow` 分支构造本类型；不要把 Domain 或会话全局状态硬编码进分页逻辑。同步在独立测试文件中覆盖打开失败、请求转发、多 chunk 翻页、空结果和生产适配器。
- 新增/调整输出列时，必须同步修改 `ShowSlowExec::Next` 的列序号、规划器生成的 schema、Go 对照实现及 Rust/Go 独立测试。特别注意保持已公开列的顺序和类型，避免客户端兼容性破坏。
- 扩展 `SlowQueryInfo` 时先核对 Domain 中多个同名类型，明确转换归属；不要因名称相同而直接假设可互换。
- 若允许 `show_slow_query` 失败，需要修改 trait 返回类型、`Open` 的传播逻辑及所有数据源实现，并测试失败后 `result`/`cursor` 的状态。若数据量可能很大，可评估流式数据源，但这会改变当前“Open 全量物化、Next 只分页”的生命周期与内存特征。
- 测试应继续放在 `pkg/executor/show_slow_queries_test.rs`，不要内嵌到生产源文件。涉及 SQL 可见行为时还应同步 `pkg/executor/show_test.go` 或对应的独立 Rust 集成测试。

## 验证依据

- 源码与模块：`pkg/executor/show_slow_queries.rs`、`pkg/executor/lib.rs`、`pkg/executor/builder.rs`。
- crate 声明：`pkg/executor/Cargo.toml` 中 `[lib] path = "lib.rs"` 以及 `astersql-types`、`astersql-util-chunk`、`astersql-util-execdetails` 依赖。
- Rust 独立测试：`pkg/executor/show_slow_queries_test.rs` 的 `next_appends_ia_remote_read_stats_after_existing_columns` 与 `open_preserves_cursor_like_go_executor`。
- Go 对照与测试：`pkg/executor/show_slow_queries.go`、`pkg/executor/builder.go::buildShowSlow`、`pkg/executor/show_test.go::TestAdminShowSlowIARemoteReadStats`、`pkg/domain/domain.go::ShowSlowQuery`。
- 下游转换：`pkg/util/execdetails/execdetails.rs::GetIARemoteReadSegmentStats`，缺失 `ScanDetail` 时返回默认零统计。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`query ShowSlowExec --kind struct`、`query ShowSlowSource --kind trait`、`query SlowQueryInfo --kind struct` 均定位到目标符号及相关同名结构。文件过滤与 callers/callees 没有给出可用边，故调用关系用上述源码与 `rg` 结果交叉核验。
- 结构验收使用任务指定命令，只检查文档存在且恰有 11 个固定二级章节；本任务按计划不运行 Cargo。
