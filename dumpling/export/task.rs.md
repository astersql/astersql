# `dumpling/export/task.rs`

## 文件定位

本文件定义 Dumpling 导出流水线的“任务协议层”：生产者把数据库、表、视图、序列、placement policy 的 DDL，或某个表数据分块，包装成任务；消费者再按任务种类选择具体写出路径。它属于 Cargo package `astersql-dumpling-export`，由 [`dumpling/export/lib.rs`](lib.rs) 的 `include!("task.rs")` 直接拼入 crate 根作用域，而不是独立的 Rust 子模块。因此，同一 crate 中的 `dump.rs`、`writer.rs` 和 `ir.rs` 可直接使用这里的类型以及 `TableMeta`、`TableDataIR` 等相邻文件定义的协议。

对应的 Go 源文件是 [`dumpling/export/task.go`](task.go)。本文件不是门面或生成代码，也不执行查询或文件 I/O；它只承载任务数据、统一摘要接口以及静态分发枚举。crate 的边界和依赖由 [`dumpling/export/Cargo.toml`](Cargo.toml) 确认；本文件自身只直接依赖 crate 内 `ir.rs` 的两个 trait 和 Rust 标准库的 `String`、`Box`。

## 核心职责

1. `Task: Send` 规定所有导出任务必须能给出人类可读的 `Brief()`，并能在线程间安全转移。
2. 六个具体结构体保存写出某类对象所需的最小数据：五类元数据任务保存对象名和 DDL，`TaskTableData` 保存表元信息、行数据 IR 以及分块编号。
3. 六个 `NewTask*` 构造函数把调用参数原样装配进对应结构体；它们不验证 SQL、名称或分块范围。
4. 各结构体的 `Task::Brief` 实现产生与 Go 版本一致的日志/进度摘要。
5. `TaskEnum` 将六种任务组成闭合的 tagged union，供 `writer.rs::handleTask` 进行穷举、类型安全的写出分发。

该文件不负责决定要导出哪些对象、不负责把任务发送到通道，也不负责写文件。前两项由 `dump.rs::dumpDatabases`、`dumpSQL`、`dumpWholeTableDirectly` 等生产路径完成，最后一项由 `writer.rs::Writer::handleTask` 及其 `Write*` 方法完成。

## 主要符号

- `pub trait Task: Send { fn Brief(&self) -> String; }`：最小公共行为。`Send` 是跨线程传递约束；接口没有执行、取消或错误返回能力。
- `TaskDatabaseMeta { DatabaseName, CreateDatabaseSQL }`：一条 `CREATE DATABASE` 输出任务。
- `TaskTableMeta { DatabaseName, TableName, CreateTableSQL }`：一条普通表 schema 输出任务。
- `TaskViewMeta { DatabaseName, ViewName, CreateTableSQL, CreateViewSQL }`：同时携带视图对应的 table DDL 与 view DDL；writer 会写两个目标文件。
- `TaskSequenceMeta { DatabaseName, SequenceName, CreateSequenceSQL }`：sequence schema 输出任务。
- `TaskPolicyMeta { PolicyName, CreatePolicySQL }`：placement policy 输出任务，不带数据库名。
- `TaskTableData { Meta, Data, ChunkIndex, TotalChunks }`：数据导出任务。`Meta: Box<dyn TableMeta>` 提供库表名、列与 schema 信息；`Data: Box<dyn TableDataIR>` 提供可启动、迭代和关闭的行源。`ChunkIndex` 按当前实现从 `0` 开始。
- `NewTaskDatabaseMeta`、`NewTaskTableMeta`、`NewTaskViewMeta`、`NewTaskSequenceMeta`、`NewTaskPolicyMeta`：接受 `impl Into<String>`，取得并保存字符串所有权，返回具体值而不是堆指针。
- `NewTaskTableData`：取得两个 trait object 的所有权并记录分块位置；不启动数据源。
- `TaskEnum::{DatabaseMeta, TableMeta, ViewMeta, SequenceMeta, PolicyMeta, TableData}`：writer 通道的实际载荷类型。其 `Brief` 实现只委派给内部具体任务。

所有这些 API 都是 `pub`，但由于文件被 `include!` 到 crate 根，它们实际成为 crate 根符号，而不是 `task::TaskEnum` 这样的模块限定符号。命名保留 Go 风格，crate 根的 lint 配置在迁移阶段允许 `non_snake_case`。

## 执行流程

常规导出主链如下：

1. `dump.rs::Dump` 调用 `Dumper::dumpDatabases`。
2. `dumpDatabases` 查询对象元数据并按对象类型调用相应 `NewTask*`。TiDB placement policy 先生成 `PolicyMeta`；随后逐库生成 `DatabaseMeta`，逐表生成 `TableMeta`、`ViewMeta` 或 `SequenceMeta`。普通表在未禁用数据导出时继续进入数据任务路径。
3. `Dumper::newTaskTableData` 先增加 `metrics.totalChunks`，再调用本文件的 `NewTaskTableData`。`dumpSQL` 为匿名 `result` 表生成单个 `(0, 1)` 数据块；普通表路径也通过该包装器构造任务。
4. 生产者显式包成 `TaskEnum`，通过 `std::sync::mpsc::Sender<TaskEnum>` 发送。通道关闭时，`dump.rs` 在发送点产生 `"task channel closed"` 错误；本文件不参与该错误处理。
5. `writer.rs::Writer::handleTask` 对枚举变体穷举匹配：五类元数据任务分别调用 `WriteDatabaseMeta`、`WriteTableMeta`、`WriteViewMeta`、`WriteSequenceMeta`、`WritePolicyMeta`；数据任务调用 `WriteTableData`。
6. 数据任务写完后，若 `ChunkIndex + 1 == TotalChunks`，writer 调用整表完成回调。这个等式是当前代码判断最后一块的唯一条件，本文件不预先验证编号是否合法。

`Brief()` 与写出流程正交：具体任务或 `TaskEnum` 均可生成摘要，但它不触发 I/O，也不改变任务状态。

## 数据与状态

本文件没有全局变量、缓存、锁或可变静态状态。每个任务都是一次性拥有其字段的普通值：元数据任务拥有名称与 SQL 字符串；数据任务独占两个 boxed trait object。构造函数只完成所有权转移和字段赋值，不复制 IR 内部数据，也不做数据库访问。

`TaskTableData::Meta` 的协议来自 `ir.rs::TableMeta: Send + Sync`，其中 `DatabaseName()`、`TableName()` 被 `Brief()` 使用，其余列、DDL 和估算信息供 dump/writer 的其他阶段使用。`Data` 的协议来自 `ir.rs::TableDataIR: Send`；它要求可变调用 `Start`、`Rows`、`Close`、`RawRows`。因此 `TaskTableData` 的数据源只能由持有任务可变访问权的一方推进，而元信息允许线程安全共享语义。

`ChunkIndex`、`TotalChunks` 使用 `i32`。文件注释和生产调用表明索引从零开始，摘要直接显示原值，例如首块显示 `(0/N)`；这里没有把它转换成人类习惯的一基编号。`TaskEnum` 自身不额外保存状态，仅保留内部任务的所有权和变体标签。

## 依赖与调用关系

上游生产者的直接证据来自 RustCodeGraph：

- `dump.rs::dumpDatabases` 调用全部五个元数据构造函数，并将结果包装为对应 `TaskEnum` 变体。
- `dump.rs::Dumper::newTaskTableData` 调用 `NewTaskTableData`；该方法又被 `dumpSQL`、`dumpWholeTableDirectly` 等数据生成路径调用。
- `parity_test.rs::contract_normal` 直接调用数据库、表、policy 构造函数和 `Brief()`，固定对外可见摘要。
- `writer_test.rs::test_handle_task_runs_table_callback_only_for_last_chunk` 直接构造 `TaskEnum::TableData`，覆盖两块数据的最后一块回调条件。

下游消费者的直接证据来自 `writer.rs::Writer::handleTask`：它是 `TaskEnum` 的核心分发点，并调用六个对应的 `Write*` 方法。数据分支需要 `&mut TaskEnum`，因为 `TableDataIR::Start`、行迭代和 `Close` 都通过可变引用推进资源状态。

crate 层面，`lib.rs` 先公开 `stubs.rs`，随后依次 `include!` `prepare.rs`、本文件和 `ir.rs` 等文件；Rust 名称解析允许本文件引用稍后定义在同一 crate 作用域中的 `TableMeta` 与 `TableDataIR`。`Cargo.toml` 将该目录声明为 library crate，且未为 `task.rs` 设置独立 feature，故这些任务类型不受条件编译开关控制。

## 错误处理与边界

本文件的构造函数和 `Brief()` 都不返回 `Result`，没有显式错误分支。它们接受空名称、空 SQL、负数分块索引、零/负总块数或不一致的 `(ChunkIndex, TotalChunks)`；合法性由上游生成逻辑保证。安全扩展时不应误以为构造成功代表参数已校验。

`TaskTableData::Brief` 会同步调用 `Meta.DatabaseName()` 和 `Meta.TableName()`；trait 实现若内部 panic，摘要调用也会 panic，本文件不捕获。除此之外，格式化拥有的 UTF-8 `String` 不涉及编码转换错误。

真正可恢复的错误位于边界两侧：生产者发送失败由 `dump.rs` 映射为 `task channel closed`；writer 的模板渲染、IR 启动/迭代/关闭及存储写入错误通过 `Result` 向上传播。Rust 的 `TaskEnum` 是闭合枚举，所以与 Go `writer.go::handleTask` 不同，不存在未知动态任务类型的默认分支；新增任务若未补 writer match，编译器会报告非穷举匹配。

数据库摘要中的 `"dababase"` 是 Go 原实现保留的拼写错误。`task.rs` 明确保留它，`parity_test.rs` 也断言该字符串；修正拼写会改变已固定的兼容输出，应当视为有意的外部可见变更。

## 并发与资源生命周期

`Task: Send`、`TableMeta: Send + Sync`、`TableDataIR: Send` 共同保证任务载荷可以进入标准库 MPSC 通道并移交给 writer 线程。这里没有 `Clone` 实现：尤其是 `TaskTableData` 独占 `Data`，避免同一行迭代器被多个 writer 同时推进。枚举只做所有权封装，不创建线程，也不加锁。

数据资源的生命周期分为三段：`NewTaskTableData` 只接管尚未启动的 IR；`Writer::WriteTableData` 用 writer 持有的连接调用 `Start` 并消费行；写出路径随后关闭行迭代器和 IR。关闭失败可作为 `Result` 返回。元数据任务没有数据库连接或外部存储句柄，离开作用域时仅释放字符串。

任务完成语义由 writer 管理而不是 `Drop` 管理。`handleTask` 每次接收时更新计数；数据块成功写出且满足 `ChunkIndex + 1 == TotalChunks` 才触发整表回调。任务本身没有完成标志、重试计数、取消 token 或析构副作用，因此移动或丢弃一个未消费任务不会自动补发、关闭通道或报告失败。

## 与 Go 版本的对应关系

[`task.go`](task.go) 的 `Task` 接口、六个结构体、六个构造函数和六种 `Brief` 格式在 Rust 中均有直接对应。字段含义和摘要文本保持一致，包括 `dababase` 拼写以及数据摘要的零基分块值。Rust 的 `impl Into<String>` 比 Go 的 `string` 参数多接受了可转换的所有权形式，但落入结构体后仍是拥有的 `String`。

主要语言映射差异如下：

- Go 构造函数返回 `*Task...`；Rust 元数据构造函数返回具体值，只有 trait object 字段使用 `Box`。
- Go 结构体匿名嵌入 `Task` 接口字段，但实际摘要由指针接收者方法提供；Rust 不保存冗余接口字段，而是直接为每个结构体实现 `Task`。
- Go writer 接收 `chan Task` 并通过 type switch 分发，遇到未知实现会记录警告后返回 `nil`；Rust 通道传递 `TaskEnum`，由穷举 `match` 静态约束支持集合。
- Go 的 `TableMeta`、`TableDataIR` 是接口值；Rust 使用 `Box<dyn ...>` 显式表达堆分配与唯一所有权，并通过 trait 上的 `Send`/`Sync` 表达并发约束。
- Go 分块字段是平台宽度的 `int`，Rust 固定为 `i32`。当前生产路径和 writer 文件命名也使用 `i32`，但超大分块计数属于扩展时需审查的兼容边界。

Go 版 `dumpDatabases` 还包含更丰富的取消、collation 调整和部分兼容分支；这些差异属于 `dump.rs` 的生产策略，不应归因于本任务定义文件。就任务载荷协议而言，Rust 已覆盖 Go 的六种任务类型。

## 扩展指南

新增一种任务时，至少应同步修改本文件的具体结构体、构造函数（若需要）、`Task` 实现和 `TaskEnum` 变体，并修改 `writer.rs::Writer::handleTask` 及对应 `Write*` 路径。随后在 `dump.rs` 的真实生产入口接线，确认 MPSC 发送错误仍被传播。由于枚举是闭合的，先添加变体能借助编译错误定位遗漏的 match，但不能替代行为测试。

调整现有任务字段时，应同时核对 `dump.rs::dumpDatabases`/`newTaskTableData` 的构造参数与 `writer.rs::handleTask` 的消费方式。修改 `TaskTableData` 时还要同步 `ir.rs` 的 `TableMeta`、`TableDataIR` 合约、分块文件命名和最后一块回调规则。不要把 Rust 测试内嵌到本源文件；应扩展同目录独立的 `parity_test.rs` 或 `writer_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] #[path = ...]` 声明挂载。

建议补充的回归点包括：六种 `Brief` 全覆盖、`TaskEnum::Brief` 委派、空字符串与特殊字符格式、分块 `(0, 1)` 和多块边界、每个枚举变体到正确 writer 方法的映射。若打算修正 `dababase`、改变零基展示、扩大整数宽度或开放外部自定义任务，需明确评估日志兼容、Go 对齐、文件命名和通道 API 的破坏性影响。性能方面，本文件主要成本是字符串拥有与每次 `Brief()` 分配；避免在高频路径无必要地反复生成摘要。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标文件已索引；`files --filter dumpling/export` 确认 Rust/Go 实现及相邻独立测试均在图中。
- RustCodeGraph `node --file dumpling/export/task.rs`：核对了 190 行完整源码、23 个索引符号以及 `dump.rs`、`writer.rs`、`parity_test.rs`、`writer_test.rs` 四个直接使用文件。
- RustCodeGraph `query/node/callers/callees`：确认 `dump.rs::dumpDatabases -> NewTask* -> TaskEnum`、`Dumper::newTaskTableData -> NewTaskTableData`，以及 `Writer::handleTask -> Write*` 的调用边；`NewTaskTableData` 还由 `writer_test.rs` 直接调用。
- [`dumpling/export/Cargo.toml`](Cargo.toml) 与 [`dumpling/export/lib.rs`](lib.rs)：确认 library crate 边界、`include!` 组织方式、没有 task 专属 feature，以及独立测试模块的挂载位置。
- [`dumpling/export/ir.rs`](ir.rs)：确认 `TableMeta: Send + Sync` 和 `TableDataIR: Send` 的方法、可变性与资源协议。
- [`dumpling/export/dump.rs`](dump.rs) 与 [`dumpling/export/writer.rs`](writer.rs)：核对任务创建顺序、MPSC 载荷、发送错误、六路写出分发、IR 生命周期和最后一块回调。
- [`dumpling/export/task.go`](task.go) 与 [`dumpling/export/writer.go`](writer.go)：逐项核对 Go 的字段、构造函数、摘要文本、动态 type switch 以及未知任务分支。
- [`dumpling/export/parity_test.rs`](parity_test.rs)：`contract_normal` 固定数据库、表、policy 的摘要，包含 `dababase` 兼容拼写。
- [`dumpling/export/writer_test.rs`](writer_test.rs)：`test_handle_task_runs_table_callback_only_for_last_chunk` 证明两块任务均被处理，但整表回调只在第二块触发。同目录未发现 `task_test.rs`；任务协议的现有 Rust 回归分散在上述两个独立测试文件中。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前另以指定 shell 命令验证目标文件存在且恰好包含十一个固定二级标题，并人工复核文档没有把上游/下游行为误写成本文件自身实现。
