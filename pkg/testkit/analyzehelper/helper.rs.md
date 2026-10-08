# `pkg/testkit/analyzehelper/helper.rs`

## 文件定位

该文件是 `astersql-testkit-analyzehelper` crate 的行为实现，服务于 ANALYZE、统计信息与谓词列收集相关测试。crate 入口 `pkg/testkit/analyzehelper/lib.rs` 声明 `helper` 模块，并通过 `pub use helper::*` 将本文件的三个公开项重新导出；仓库总门面 `pkg/lib.rs` 又在 `testkit::analyzehelper` 下转出该 crate。

它不是 SQL 执行器或统计模块的生产实现，而是一个测试辅助边界：调用方提供“执行 SQL”和“将列统计用量写入 KV”的运行时能力，本文件负责以固定顺序编排这两个动作。`pkg/testkit/analyzehelper/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/testkit/analyzehelper`，并声明 `astersql-kv`、`astersql-session`、`astersql-testkit` 三个路径依赖；不过当前 `helper.rs` 本身没有直接引用这些 crate，真实会话/Domain 适配尚未在本文件中接线。

## 核心职责

- `TriggerPredicateColumnsCollection` 为每个指定列构造并执行一条等值谓词查询，使会话的列使用追踪路径观察到该列，然后在所有查询成功后请求一次持久化。
- `AnalyzeRuntime` 把具体 TestKit、Session、Domain 和统计句柄隔离在接口之后，使编排逻辑可以用可控替身独立测试。
- `AnalyzeError` 为适配层提供最小、可比较且实现标准错误接口的消息错误，并让底层执行或持久化失败原样向上返回。

本文件只触发列使用记录并要求运行时落盘，不负责创建表、执行 `ANALYZE TABLE`、读取直方图或验证统计结果；这些步骤属于调用测试。

## 主要符号

- `pub struct AnalyzeError(pub String)`：公开的单字段元组结构体。派生 `Clone`、`Debug`、`Eq`、`PartialEq`，便于适配器构造及测试精确比较；`Display` 直接输出内部消息，`std::error::Error` 采用默认实现。它与 `pkg/executor/analyze.rs` 等文件中的同名错误不是同一类型。
- `pub trait AnalyzeRuntime`：同步、可变运行时接口。
  - `execute(&mut self, sql: &str) -> Result<(), AnalyzeError>` 执行一条由 helper 生成的 SQL。
  - `dump_column_stats_usage_to_kv(&mut self) -> Result<(), AnalyzeError>` 将已经累积的列统计用量写入 KV。
- `pub fn TriggerPredicateColumnsCollection(runtime: &mut dyn AnalyzeRuntime, table_name: &str, columns: &[String]) -> Result<(), AnalyzeError>`：唯一编排入口。名称保留 Go 风格；`lib.rs` 用 `#![allow(non_snake_case)]` 允许这一命名。

文件没有常量、枚举、泛型、条件编译项或私有辅助函数。

## 执行流程

1. 调用方传入一个可变的 `dyn AnalyzeRuntime`、表名和保持调用方顺序的列名切片。
2. 函数顺序遍历 `columns`，针对每个 `column` 用 `format!` 生成 `SELECT * FROM {table_name} WHERE {column} = '1'`。
3. 每条 SQL 立即交给 `AnalyzeRuntime::execute`。成功后继续下一列；任意一次返回错误时，`?` 立即结束函数，因此剩余列不会执行，dump 也不会发生。
4. 全部查询成功后调用一次 `dump_column_stats_usage_to_kv`，并直接返回其结果。
5. 当 `columns` 为空时循环不执行，但第 4 步仍会发生，所以仍请求一次 dump。

独立测试 `pkg/testkit/analyzehelper/helper_aster_unit_test.rs` 用事件序列验证了查询顺序在 dump 之前、空列仍 dump、第二条查询失败会短路，以及 dump 失败发生在所有查询完成之后并被传播。

## 数据与状态

本文件自身不保存全局或跨调用状态。`table_name`、`columns` 和生成的 SQL 都仅在一次函数调用内使用；真正的会话状态、列使用缓存及 KV 写入状态由 `AnalyzeRuntime` 实现持有。

`columns: &[String]` 是只读借用，遍历不会排序、去重或规范化，因此重复列会产生重复查询，输出顺序严格等于输入顺序。`runtime: &mut dyn AnalyzeRuntime` 保证一次调用期间对适配器的独占可变访问；helper 不克隆运行时，也不缓存其引用。每次 `format!` 会分配一条新的 SQL `String`，随后仅以 `&str` 借给 `execute`。

## 依赖与调用关系

上游边界如下：

- `pkg/testkit/analyzehelper/lib.rs` 声明并再导出本模块；`pkg/lib.rs` 提供聚合门面。
- 本 crate 的直接 Rust 单元测试位于 `pkg/testkit/analyzehelper/helper_aster_unit_test.rs`，由 `lib.rs` 的 `#[cfg(test)] mod helper_aster_unit_test;` 纳入，测试逻辑没有内嵌到源文件。
- 多个 Rust 测试 crate 在各自 `Cargo.toml` 中把 `astersql-testkit-analyzehelper` 声明为 dev-dependency，例如 `pkg/statistics/handle/updatetest/Cargo.toml`、`pkg/statistics/handle/handletest/Cargo.toml` 与 `pkg/executor/test/analyzetest/Cargo.toml`。

当前接线并不完整：`rg` 没有找到上述外部 Rust 测试对本文件 CamelCase 函数的直接调用；`pkg/statistics/handle/updatetest/update_test.rs`、`pkg/statistics/handle/handletest/analyze/analyze_test.rs` 和 `pkg/statistics/handle/handletest/handle_test.rs` 仍直接执行逐列 SQL并调用 Domain 的 dump 方法。`pkg/statistics/handle/storage/gc_test.rs` 调用了 `analyzehelper::trigger_predicate_columns_collection`，但本文件及 `lib.rs` 没有定义该蛇形名称，且该依赖位于 `cfg(any())` 的禁用区段，不能把它视为当前已工作的调用边。

下游只有 `AnalyzeRuntime::execute` 和 `AnalyzeRuntime::dump_column_stats_usage_to_kv` 两个动态分派调用。Cargo 中声明的 KV、Session、TestKit 依赖表达预期适配边界，但本实现没有直接调用它们。

Go 侧的真实上游调用广泛分布于统计与执行器测试，例如 `pkg/statistics/integration_test.go`、`pkg/statistics/handle/updatetest/update_test.go`、`pkg/statistics/handle/storage/gc_test.go`、`pkg/statistics/handle/handletest/analyze/analyze_test.go` 和 `pkg/executor/test/analyzetest/analyze_test.go`。

## 错误处理与边界

函数遵循首错返回：查询失败时保留此前运行时已经产生的副作用，不回滚，也不会尝试 dump；dump 失败时所有查询已经完成，函数只把错误交给调用方。`AnalyzeError` 不保存错误类别、来源链或上下文，适配器若需要诊断信息，必须在构造其消息字符串时加入。

SQL 通过字符串插值生成，`table_name` 与 `column` 没有标识符转义、引用、语法检查或参数绑定。该 API 只能接收测试代码控制的可信 SQL 标识符/表达式，不能暴露给用户输入；特殊字符、保留字或恶意文本可能导致语法错误或 SQL 注入。固定比较值为字符串字面量 `'1'`，具体类型转换语义由 SQL 引擎决定。

helper 不检查空表名、空列名、重复列或不存在的对象，相关失败由 `execute` 返回。空列集合是明确支持的边界：不执行 SQL，但仍 dump 一次。

## 并发与资源生命周期

该逻辑完全同步，不创建线程、异步任务、锁、通道或事务，也没有超时与取消机制。`&mut dyn AnalyzeRuntime` 使同一个适配器在调用期间不能被普通安全 Rust 代码并发可变访问，但 trait 本身没有 `Send` 或 `Sync` 约束。

SQL 字符串在单次循环迭代中创建，并在 `execute` 返回后释放；所有会话、事务、统计缓存和 KV 资源的开启、提交、刷新与关闭都由运行时实现负责。若未来适配器要求事务原子性或并发隔离，应在适配器中实现并补充测试，不能从当前 helper 推断这些保证。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/testkit/analyzehelper/helper.go`。两版共有的核心语义是：保持列顺序；每列执行 `SELECT * FROM <table> WHERE <column> = '1'`；所有查询成功后只调用一次列使用落盘；即使列集合为空也会落盘。

接口层存在有意差异。Go 函数直接接收 `*testing.T`、`*testkit.TestKit` 和 `kv.Storage`，先通过 `session.GetDomain(store)` 获取 Domain，再取 `StatsHandle`；SQL 用 `tk.MustExec` 执行，错误通过 `require.NoError`/测试失败终止。Rust 版不依赖测试框架终止流程，而是要求调用者实现 `AnalyzeRuntime`，并以 `Result<(), AnalyzeError>` 显式传播错误。这让顺序与失败路径可独立测试，但也意味着 Rust 文件尚未包含 Go 版的真实 TestKit/Storage/Domain 适配和 `GetDomain` 失败分支。

Go 版使用可变参数 `columns ...string`，Rust 版使用 `&[String]`。Go 的测试调用点已经覆盖统计集成、统计句柄与 executor analyze 场景；Rust 的同类迁移测试目前多以内联步骤完成，不能据此声称 helper 已在所有对应场景复用。

## 扩展指南

- 若要接入真实 Rust TestKit/Domain，应新增独立适配实现文件或在合适的 test-support crate 中实现 `AnalyzeRuntime`，避免把会话与存储细节塞回编排函数；同时核对 `Cargo.toml` 中当前未使用的三个依赖是否因此真正需要。
- 若要统一命名，可在保持 `TriggerPredicateColumnsCollection` 兼容性的前提下增加明确的蛇形别名，并修正 `pkg/statistics/handle/storage/gc_test.rs` 的调用；不要静默删除 Go 风格 API。
- 若改变 SQL 模板、列顺序、空输入行为或 dump 时机，必须同步更新 `pkg/testkit/analyzehelper/helper_aster_unit_test.rs`，并抽查依赖谓词列收集的 statistics/executor 对照测试。
- 若允许不可信标识符，应引入仓库既有的 SQL 标识符转义方案，并为数据库名、限定表名、保留字、反引号和注入文本添加独立回归测试；直接拼接用户输入不安全。
- 若改成异步或并行执行，需要先定义查询顺序、首错语义、部分成功后的 dump 策略和运行时线程安全约束。并行化还可能改变列使用记录的可观察顺序，当前测试契约不允许无依据地这样做。
- Rust 测试必须继续放在独立的 `helper_aster_unit_test.rs`，不要内嵌到 `helper.rs`。

## 验证依据

- 源码与模块边界：`pkg/testkit/analyzehelper/helper.rs`、`pkg/testkit/analyzehelper/lib.rs`、`pkg/testkit/analyzehelper/Cargo.toml`、`pkg/lib.rs`。
- 独立 Rust 测试：`pkg/testkit/analyzehelper/helper_aster_unit_test.rs`，覆盖生成 SQL与事件顺序、空列、查询失败短路和 dump 失败传播。
- Go 对照与调用证据：`pkg/testkit/analyzehelper/helper.go`；使用 `rg` 找到 `pkg/statistics/integration_test.go`、`pkg/statistics/handle/**` 与 `pkg/executor/test/analyzetest/**` 中的调用。
- Rust 迁移现状：使用 `rg` 检查 `TriggerPredicateColumnsCollection` 与 `trigger_predicate_columns_collection`，并读取 `pkg/statistics/handle/updatetest/update_test.rs`、`pkg/statistics/handle/handletest/analyze/analyze_test.rs`、`pkg/statistics/handle/handletest/handle_test.rs`、`pkg/statistics/handle/storage/gc_test.rs` 的相关片段。
- RustCodeGraph：`status` 显示本地索引包含 11,467 个文件、307,296 个节点；`query TriggerPredicateColumnsCollection --kind function` 同时定位 Go `helper.go:28` 与 Rust `helper.rs:45`；`query AnalyzeRuntime --kind trait` 定位本文件 trait；`query AnalyzeError --kind struct` 区分本文件类型与仓库其他同名错误。`explore/files/node` 与后续 `callers/callees/node` 组合未返回可用输出或超时，因此调用点以 `rg` 和原始文件读取补证，没有把缺失图边解释成“无调用者”。
- 结构验收应执行任务指定命令，确认文件存在且恰好包含十一个固定二级标题。本任务是纯文档分析，按计划不运行 Cargo。
