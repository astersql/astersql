# `pkg/executor/show_next_row_id.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根 `pkg/executor/lib.rs:215-216` 以公开模块 `show_next_row_id` 导出它，`pkg/executor/Cargo.toml` 则把 crate 根指定为 `lib.rs`。它实现的是 `SHOW TABLE ... NEXT_ROW_ID` 结果行的编码核心：把上游提供的分配器快照写入 `astersql_util_chunk::Chunk`，而不是自行访问 domain、information schema、表对象或持久化的 AutoID 存储。

当前接线必须按事实区分：`pkg/executor/builder.rs:2406,2495-2497` 会把 `Plan::ShowNextRowId` 降为通用的 `ExecutorKind::ShowNextRowId` 叶节点，但全仓 Rust 引用搜索未找到 `ShowNextRowIDExec`、`NextRowIdSource` 或 `AllocatorSnapshot` 在本文件之外的构造/实现。因此，本文件的公开 API 已存在，尚无直接证据表明这个泛型执行器已经被 Rust builder 实例化并进入运行时执行主链。

## 核心职责

- `NextRowIdSource::allocators` 隔离“按 schema/table 获取分配器状态”与“格式化 SHOW 结果”两部分，使执行器只依赖一批完整的 `AllocatorSnapshot`。
- `ShowNextRowIDExec::Next` 每次先清空调用方的输出 chunk；首次成功调用把每个快照编码为五列：库名、表名、列名、下一全局 ID、ID 类型；后续调用只返回空 chunk。
- `AllocatorType` 到展示列的映射与 Go `pkg/executor/show_next_row_id.go:57-84` 对齐：RowId、AutoIncrement、AutoRandom、Sequence 分别产生 `_TIDB_ROWID`、`AUTO_INCREMENT`、`AUTO_RANDOM`、`SEQUENCE` 标签。
- 本文件不负责计算下一 ID，也不承诺快照的顺序、一致性或新鲜度；这些属性由 `NextRowIdSource` 的实现负责，而当前仓库未找到实现。

## 主要符号

- `pub enum AllocatorType`（`show_next_row_id.rs:27-32`）：本地闭合枚举，表示四种允许展示的分配器类型。它与 `astersql_meta_autoid::AllocatorType` 名称相似但不是同一个类型，当前也没有转换实现。
- `pub struct AllocatorSnapshot`（`:36-44`）：一条分配器查询快照。`next_global_id` 是待展示数值；`primary_key_is_handle`、`auto_increment_column` 和 `primary_key_name` 为列名选择提供元数据。
- `pub trait NextRowIdSource`（`:47-55`）：同步、可变的数据源边界。关联类型 `Error` 原样成为执行器错误；`allocators(&mut self, schema, table)` 一次返回完整向量。
- `pub struct ShowNextRowIDExec<S>`（`:58-63`）：持有数据源、库表名和一次性执行标记 `done`。四个字段均公开，调用方可直接组装，也必须自行保证初始状态通常为 `done = false`。
- `pub fn Next<C>(&mut self, _ctx: C, req: &mut Chunk)`（`:67-107`）：唯一行为入口。泛型上下文参数未使用；命名保留 Go 风格，因此文件以 `#![allow(non_snake_case)]` 放宽命名 lint。

## 执行流程

1. `Next` 无条件调用 `req.Reset()`（`:68`），保证复用的 chunk 不残留上一批数据。
2. 若 `done` 已为真，立即返回 `Ok(())`（`:69-71`）；所以标准拉取协议中的第二次及后续调用得到空批次。
3. 以保存的 `schema_name`、`table_name` 调用 `source.allocators`（`:73-75`）。错误通过 `?` 原样返回，此时尚未写行，且 `done` 仍为假，调用方可以重试。
4. 顺序遍历数据源给出的快照（`:76`），按 `allocator_type` 选择 `column_name` 和固定 `id_type`（`:78-97`）：
   - RowId 或 AutoIncrement：若 `primary_key_is_handle`，使用 `auto_increment_column.unwrap_or_default()`；否则使用隐藏列名 `_tidb_rowid`。
   - AutoRandom：使用 `primary_key_name`。
   - Sequence：列名为空字符串。
5. 对每个快照依次写入 chunk 的 0..4 列（`:99-103`）。循环保持源向量顺序，不排序、不去重。
6. 全部快照编码完成后设置 `done = true` 并成功返回（`:105-106`）；空向量同样会完成执行器。

## 数据与状态

`AllocatorSnapshot` 是值语义快照：字符串归执行器此次调用所有，匹配分支会按需移动它们；文件不缓存快照。`next_global_id: i64` 被不加变换地写入第 3 列，因此符号位、取值合法性和“下一可用”语义都由数据源保证。

`ShowNextRowIDExec` 唯一跨调用状态是 `done`。成功获取并遍历快照后它从假变真；数据源失败时保持原值。`schema_name`、`table_name` 在一次执行器生命周期内不被修改。输出 schema 是硬编码的五列位置约定，调用方必须提供与 `AppendString/AppendInt64` 类型和列数相容的 chunk（相关 API 位于 `pkg/util/chunk/chunk.rs:411,640,668`）。

列名存在两个值得保留的边界：PK handle 分支中缺少 `auto_increment_column` 时不会报错，而是展示空字符串；AutoRandom 同样直接信任 `primary_key_name`。这是当前 Rust API 的显式行为，不应擅自推断为已验证的元数据不变量。

## 依赖与调用关系

直接下游依赖只有 `astersql_util_chunk::Chunk`（导入见 `show_next_row_id.rs:23`，Cargo 路径依赖见 `pkg/executor/Cargo.toml` 的 `astersql-util-chunk`）。`Next` 调用 `Chunk::Reset`、`AppendString` 和 `AppendInt64`，并通过调用方实现的 `NextRowIdSource::allocators` 获取数据。

可确认的上游模块关系是 `pkg/executor/lib.rs:216` 的公开导出。RustCodeGraph 能索引本文件及主要符号，但对泛型 `Next` 的精确 callers/callees 查询没有返回静态调用边；`rg` 的全仓精确符号搜索也只命中定义本身。`pkg/executor/builder.rs:2406,2495-2497` 的通用叶节点是同一计划类型的相邻入口证据，不是对本结构体的调用证据。

Go 主链更完整：`pkg/executor/builder.go:413-418` 从 `ShowNextRowID` 计划创建具体 `ShowNextRowIDExec`，其 `Next` 再访问 domain/infoschema、表分配器及 `NextGlobalAutoID`。Rust 文件把这些下游动作收敛在尚待实现的 `NextRowIdSource` 边界之后。

## 错误处理与边界

唯一可表达的运行时错误来自 `S::Error`。`source.allocators` 失败会短路返回；由于 chunk 已先清空且循环尚未开始，不会留下本次调用的部分结果，`done` 也不会被置真。快照到 chunk 的转换路径本身没有返回错误。

Rust 的 `AllocatorType` 是闭合枚举，所以 `match` 穷尽四种类型，不存在 Go 版本 `default -> ErrInvalidAllocatorType` 的运行时分支。代价是将来给枚举增加变体会形成编译期遗漏，必须同步更新映射和测试。

本文件不检查目标表是否存在、访问权限、schema/table 空字符串、重复分配器、负数 ID 或输出 chunk schema。与 Go 版本相比，表查找错误、AutoID 读取错误及未知 allocator 错误只能由未来的数据源适配层表达或预先消解。

## 并发与资源生命周期

文件不创建线程、任务、锁、通道、事务或外部资源。`Next` 需要 `&mut self` 和 `&mut Chunk`，安全 Rust 会阻止同一个执行器或输出 chunk 被两个调用同时可变借用；但 `S` 是否在内部共享或加锁完全由数据源实现决定。

资源生命周期是单批、一次性：数据源借用仅持续到 `allocators` 返回，快照向量在本次 `Next` 内消费，chunk 由调用方持有，执行器以 `done` 终止后不再访问数据源。没有显式 `Close`/`Drop` 协议，因此未来若数据源持有事务、网络连接或锁，必须由 `S` 自己的 RAII/`Drop` 语义处理，或扩充接口而不能假定本执行器会清理。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/executor/show_next_row_id.go`。两版共同保留了：每次先 Reset、`done` 的一次性语义、逐分配器输出五列、四种标签、RowId/AutoIncrement 对 PK handle 的列名选择、AutoRandom 主键名与 Sequence 空列名。

主要差异如下：

- Go 结构嵌入 `exec.BaseExecutor` 并保存 `*ast.TableName`；Rust 结构保存字符串和泛型 source，不实现仓库通用 Executor trait。
- Go `Next` 在执行时从 domain 的 InfoSchema 查表，再读取 `tbl.Allocators` 和每个 allocator 的 `NextGlobalAutoID`；Rust 仅消费数据源预制的快照。
- Go 对缺失的自增列信息保持空列名，Rust 用 `Option::unwrap_or_default` 对齐这一结果；Go 未知 allocator 返回 `ErrInvalidAllocatorType`，Rust 以闭合枚举在类型层排除该分支。
- Go 的 `context.Context` 参与表查找；Rust 的泛型 `_ctx` 完全未使用，所以当前文件本身没有取消或超时传播。

直接行为回归位于 `pkg/executor/test/seqtest/seq_executor_test.go:774-872` 的 `TestAdminShowNextID`/`HelperTestAdminShowNextID`：它验证普通隐藏 RowID、PK handle、分离 AUTO_INCREMENT、非聚簇自增主键、AUTO_RANDOM、SEQUENCE、表重命名，以及 cache/rebase 后的下一 ID。未发现对应的独立 Rust 测试或对本文件类型的 Rust 测试引用，因此这些 Go 用例是语义基准，并非 Rust 路径已经通过测试的证据。

## 扩展指南

- 若接入真实 Rust 执行链，优先为 table/autoid 适配层实现 `NextRowIdSource`，并在 builder 的 `ExecutorKind::ShowNextRowId` 物化路径中构造本执行器；同时明确上下文取消、权限/表查找错误及 allocator 读取错误如何映射到 `S::Error`。
- 若新增 allocator 类型，必须同时修改 `AllocatorType`、`Next` 的映射、Go 对照语义（若 Go 已有对应类型）和独立 Rust 测试；特别确认列名、标签、输出顺序与未知类型策略。
- 若调整五列布局或类型，必须同步 planner 的结果 schema、chunk 写入索引以及 SQL 回归期望。不能只改 `Append*` 顺序，否则会产生静默错列或类型不匹配。
- 建议新增同目录独立测试文件（例如 `pkg/executor/show_next_row_id_test.rs`，并在 `lib.rs` 以 `#[cfg(test)] mod show_next_row_id_test;` 装配），用可注入 mock source 覆盖四个映射、第二次调用为空、空 allocator 列表、source 错误后 `done == false`、错误前清空旧 chunk，以及 PK handle 缺列名的空字符串行为。测试逻辑不要内嵌回生产源文件。
- 性能上当前一次收集完整 `Vec<AllocatorSnapshot>` 并一次写完所有行，适合单表少量 allocator；若未来扩展为大量对象，需重新评估批量上限和流式接口，但不得在没有测量数据时改变当前一次性协议。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、索引时间戳 `1791342965170`；`node --file pkg/executor/show_next_row_id.rs --offset 1 --limit 260` 完整读取 108 行源码；`query` 确认 `ShowNextRowIDExec`、`NextRowIdSource`、`AllocatorSnapshot`、本地 `AllocatorType` 的定义位置。针对全限定泛型符号的 `callers/callees` 未返回边，因此没有把缺失的静态边写成已接线事实。
- Rust 源与装配：`pkg/executor/show_next_row_id.rs:23-107`、`pkg/executor/lib.rs:215-216`、`pkg/executor/builder.rs:2406,2495-2497`、`pkg/util/chunk/chunk.rs:411,640,668`。
- crate 配置：`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`、`[package.metadata.porting] go-package = "pkg/executor"`、`astersql-util-chunk` 路径依赖；该模块不受 `nextgen` feature 条件控制。
- Go 对照与接线：`pkg/executor/show_next_row_id.go:28-91`、`pkg/executor/builder.go:413-418`。
- 行为测试：`pkg/executor/test/seqtest/seq_executor_test.go:774-872`。全仓 `rg` 未找到本文件公开类型在其他 Rust 文件中的使用，也未找到 `seqtest` 中对应的 Rust `NEXT_ROW_ID` 用例。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前另行执行任务指定的 11 章节结构检查，并人工复核本文将已实现逻辑、Go 语义基准和未接线限制分别陈述。
