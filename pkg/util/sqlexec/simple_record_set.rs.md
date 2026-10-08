# `pkg/util/sqlexec/simple_record_set.rs`

## 文件定位

本文件实现 `astersql-util-sqlexec` crate 中最小的内存结果集 `SimpleRecordSet`。它把构造时已经全部可知的字段元信息和行值包装成 `RecordSet`，供不需要真实执行器、存储扫描或流式拉取的场景按 chunk 消费。模块在 `pkg/util/sqlexec/lib.rs` 中以私有模块 `simple_record_set` 挂载，并通过 `pub use simple_record_set::*` 对外导出。

该实现仅在默认启用的 `formal-crate` feature 下导入 `GoError`、`RecordChunk`、`RecordSet` 以及 `chunk`、`context`、`resolve`、`types`；feature 定义和所有直接依赖见 `pkg/util/sqlexec/Cargo.toml`。它位于 SQL 执行边界的结果集层，不负责解析、规划、执行 SQL，也不访问事务或存储。

## 核心职责

- 用 `ResultFields` 保存结果列描述，用 `Rows` 保存所有待返回的行，并用 `idx` 记录消费位置（`SimpleRecordSet`）。
- 实现 `RecordSet::Fields`，把构造时传入的字段切片原样暴露给调用方。
- 实现 `RecordSet::NewChunk`，按字段类型和 `MaxChunkSize` 创建 owned chunk，或从调用方提供的 allocator 取得 allocated chunk。
- 实现 `RecordSet::Next`，先清空目标 chunk，再从 `idx` 起逐行把动态值转换为 `Datum` 并追加，直到 chunk 满或所有行耗尽。
- 实现 `RecordSet::Close`，把 `idx` 重置为零，使同一个内存结果集可以再次从首行读取。

它不是惰性结果集：行的计算和所有权准备必须在 `SimpleRecordSet::new` 之前完成。文件也不实现 `Finish`、`TryDetach` 或 `OnFetchReturned`，因此使用 `RecordSet` 在 `restricted_sql_executor.rs` 中提供的默认行为。

## 主要符号

- `pub struct SimpleRecordSet`：结果集状态对象。
  - `pub ResultFields: Vec<resolve::ResultField>`：有序列元信息；其长度同时决定 `Next` 每行读取和追加的列数。
  - `pub Rows: Vec<Vec<Box<dyn Any>>>`：有序行数据；单元格以动态类型装箱，交给 `types::NewDatum` 识别和转换。
  - `pub MaxChunkSize: usize`：`NewChunk` 创建或租借 chunk 时使用的最大容量。
  - `idx: usize`：私有游标，指向下一条未消费行。
- `SimpleRecordSet::new(ResultFields, Rows, MaxChunkSize) -> Self`：取得字段和行的所有权，并把 `idx` 初始化为 `0`。
- `RecordSet::Fields(&self) -> &[resolve::ResultField]`：借用返回字段切片，不复制字段。
- `RecordSet::Next(&mut self, _, req) -> Result<(), GoError>`：推进游标并填充一次请求；当前实现没有可恢复错误分支，正常返回 `Ok(())`。
- `RecordSet::NewChunk(&self, alloc) -> RecordChunk`：从每个 `ResultField.column.FieldType` 构造字段类型数组，再选择 allocator 或 owned 分配路径。
- `RecordSet::Close(&mut self) -> Result<(), GoError>`：只重置游标，不释放字段、行或 chunk。

文件没有模块级常量、独立 trait、异步函数或额外条件编译分支；唯一条件编译项是 `formal-crate` 下的导入。

## 执行流程

1. 调用方先用 `SimpleRecordSet::new` 传入列描述、完整二维行数组和 chunk 上限；游标从零开始。
2. 消费方通常通过 `RecordSet::NewChunk` 取得请求缓冲区。方法遍历 `ResultFields`，要求每个字段都有 `column`，克隆其中的 `FieldType`；有 allocator 时调用 `Allocator::Alloc(&fields, 0, MaxChunkSize)` 并包装成 `RecordChunk::Allocated`，否则调用 `chunk::New(fields, MaxChunkSize, MaxChunkSize)` 并包装成 `Owned`。
3. 每次 `RecordSet::Next` 通过 `RecordChunk::with_chunk_mut` 取得底层 chunk 的可变访问权，并首先调用 `Reset`，所以调用方可以复用同一个请求缓冲区。
4. 当 `idx < Rows.len()` 时，先检查 `req.IsFull()`；已满则结束本批但保留当前 `idx`，下一次继续。未满则按 `0..ResultFields.len()` 访问当前行的单元格，用 `types::NewDatum` 转换，再调用 `AppendDatum(column, &datum)` 写入对应列。
5. 一整行追加完成后 `idx += 1`。所有行耗尽后方法返回成功；下一次调用仍先重置 chunk，最终以零行 chunk 表示 EOF。这正是 `restricted_sql_executor.rs::DrainRecordSet` 的终止条件。
6. 调用 `Close` 会把 `idx` 设回零；再次调用 `Next` 时从第一行重新开始。

## 数据与状态

`ResultFields`、`Rows` 和 `MaxChunkSize` 在构造后不会由本文件修改；唯一变化的持久状态是 `idx`。`Next` 的不变量是：`idx` 只在当前行的全部字段成功追加后增加，因此每次成功调用后它等于已经完整发出的行数。`Close` 是唯一允许游标倒退的操作。

字段数由 `ResultFields.len()` 决定，而不是当前行长度。由此产生的输入契约是每一行至少包含同样数量的单元格；额外单元格不会被读取。字段描述还必须包含 `column`，否则 `NewChunk` 会触发带固定消息的 `expect`。这些约束没有在 `new` 中预校验。

`Rows` 中的 `Box<dyn Any>` 由结果集独占；`Next` 仅借用单元格传给 `types::NewDatum`，不会从行中移走值。`Fields` 返回借用切片。`NewChunk` 会克隆字段类型，chunk 因而不借用结果字段。`MaxChunkSize` 同时作为 owned chunk 的初始容量与最大容量；allocator 路径以初始容量 `0`、最大容量 `MaxChunkSize` 请求缓冲区。

## 依赖与调用关系

上游边界由同 crate 的 `pkg/util/sqlexec/restricted_sql_executor.rs::RecordSet` 定义；`SimpleRecordSet` 实现其四个必需方法。`DrainRecordSet` 通过 trait 对象调用 `NewChunk`、反复调用 `Next`，并以 `RecordChunk::NumRows() == 0` 判断耗尽；`DrainRecordSetAndClose` 与 `ExecSQL` 则在消费后调用 `Close`。

下游直接依赖如下：

- `resolve::ResultField` 提供字段元信息，并经其 `column.FieldType` 决定 chunk 列类型。
- `types::NewDatum` 把动态单元格转换为类型系统的 `Datum`。
- `chunk::Chunk::{Reset, IsFull, AppendDatum}` 完成批缓冲区复用、容量控制和逐列追加。
- `chunk::Allocator::Alloc` 与 `chunk::New` 分别支持回收分配和直接拥有两条资源路径。
- `RecordChunk::with_chunk_mut` 统一访问 owned 与 allocated chunk；allocated 路径的锁定细节位于 `restricted_sql_executor.rs`。

RustCodeGraph 的文件级引用显示，当前 Rust 直接使用者包括 `pkg/util/sqlexec/migration_aster_unit_test.rs`、`pkg/server/conn_test.rs`，以及定义 trait/辅助函数的 `restricted_sql_executor.rs`。`conn_test.rs` 只构造空 `SimpleRecordSet` 作为 server resultset/cursor 测试的底层源。生产版 `pkg/session/nontransactional.rs` 目前定义自己的领域内 `SimpleRecordSet`，没有使用此通用实现；因此不能把 Go 侧非事务 DML 的生产接线误写成 Rust 侧已完成接线。

## 错误处理与边界

`Next` 和 `Close` 的签名服从 `RecordSet` 的 `GoError` 边界，但本实现始终返回 `Ok(())`。它没有 I/O、取消检查或业务错误传播；传入的 `_ctx` 明确未使用。结果耗尽不是错误，而由下一次 `Next` 返回空 chunk 表示。

两个结构错误会以 panic 表现，而不是 `GoError`：`NewChunk` 遇到缺少 `ResultField.column` 时执行 `expect("SimpleRecordSet result field has no column")`；`Next` 遇到某行列数少于字段数时会发生索引越界。单元格能否被 `types::NewDatum` 正确表达取决于该转换函数支持的动态类型，本文件不做类型与 `FieldType` 一致性检查。安全扩展时应保持 Go 版本的这些契约，若要改为显式错误，必须同时评估 trait、调用方和兼容行为，不能只在本文件静默改变。

`MaxChunkSize == 0` 的行为没有专门测试；其实际效果取决于 chunk 的容量语义。本文不宣称该值受支持。`Close` 不清空 `Rows`，所以它代表可重读的逻辑关闭，而不是销毁数据。

## 并发与资源生命周期

`SimpleRecordSet` 需要 `&mut self` 执行 `Next` 和 `Close`，游标没有原子量或内部锁；本文件不提供并发消费保证，也没有后台任务、通道、事务或异步生命周期。若外层把实例放进同步容器，并发顺序仍由外层负责。

owned chunk 由 `RecordChunk::Owned` 独占；allocator 路径返回 `RecordChunk::Allocated`，底层 `ChunkRef` 的互斥访问由 `RecordChunk::with_chunk_mut` 获取锁。`SimpleRecordSet` 本身不持有 allocator 或 chunk，因此 allocator/chunk 的归还和释放遵循各自类型的所有权生命周期。结果集被 drop 时字段、行及其中的装箱值自然释放；`Close` 只重置游标，既不释放内存也不归还外部缓冲区。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/sqlexec/simple_record_set.go`。Rust 保留了 Go `SimpleRecordSet` 的四个字段及核心算法：`Fields` 原样返回字段，`Next` 先 `Reset`、按字段逐列 `NewDatum`/`AppendDatum`、在 chunk 满时保留游标，`NewChunk` 支持 allocator 与普通分配，`Close` 把游标归零。

语言映射上的差异主要是所有权表达：Go 使用 `[]*resolve.ResultField`、`[][]any` 和 `*chunk.Chunk`；Rust 使用拥有的 `Vec<ResultField>`、`Vec<Vec<Box<dyn Any>>>`，并以 `RecordChunk::{Owned, Allocated}` 统一两种 chunk 所有权。Go 直接取 `field.Column.FieldType`；Rust 因类型模型与所有权需要先检查 `column` 再克隆 `FieldType`。Go 没有显式构造函数，通常使用结构体字面量；Rust 增加 `new` 来初始化私有 `idx`。

Go 的真实生产调用证据位于 `pkg/session/nontransactional.go::buildDryRunResults` 和 `buildExecuteResults`，用于返回非事务 DML 的说明性结果。当前 Rust 的 `pkg/session/nontransactional.rs` 使用另一套 `ResultValue`、`ResultField` 和同名领域结构，因此通用 sqlexec 版本在 Rust 侧主要由迁移测试和 server cursor 测试使用；后续若要统一两者，需要单独设计类型转换与接线，不能仅替换同名类型。

## 扩展指南

- 若修改批处理或游标行为，首要入口是 `RecordSet::Next`；必须保持“每次先重置请求”“满 chunk 不跳行”“耗尽后返回空 chunk”三个与 `DrainRecordSet` 配合的不变量。
- 若新增字段类型或单元格表示，检查 `types::NewDatum` 支持范围以及 `ResultField.column.FieldType` 与实际值的一致性；不要只扩展 `Rows` 的动态类型而遗漏 chunk 列编码。
- 若调整分配策略，修改 `NewChunk` 时同时覆盖 owned 和 allocator 两条路径，并确认 `RecordChunk` 的所有权/锁语义；性能风险主要是每次建 chunk 时克隆字段类型和逐单元格动态转换。
- 若改变 `Close` 为释放型语义，会破坏现有“关闭后可重读”测试和 Go 对齐行为；这属于兼容性变更。
- 应在独立文件 `pkg/util/sqlexec/migration_aster_unit_test.rs` 中扩展回归测试，不要把测试内嵌进生产源文件。现有测试 `simple_record_set_chunks_rows_and_close_restarts` 覆盖分批、EOF、字段访问和重读，`simple_record_set_uses_the_supplied_chunk_allocator` 覆盖 allocator 分支。可补充的边界包括空字段/空行、行列数不匹配、缺失 `column`、零 chunk 大小以及更多 `Any` 类型；其中 panic 契约测试应明确说明是否保持 Go 兼容。
- 若把本类型接入非事务 DML Rust 主链，需要同时处理 `pkg/session/nontransactional.rs` 当前领域类型与 `resolve::ResultField`/`Box<dyn Any>` 的转换，并在该模块独立测试中验证生产接线；这不是本文件现状。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；目标 `pkg/util/sqlexec/simple_record_set.rs` 已索引。
- RustCodeGraph `files --filter pkg/util/sqlexec`：确认目标、模块入口、Go 对照和相关独立测试位于同一 crate 目录。
- RustCodeGraph `node --file pkg/util/sqlexec/simple_record_set.rs --offset 1 --limit 240`：核对 `SimpleRecordSet`、`new` 及四个 trait 方法的完整实现，并得到目标文件的直接使用文件。
- RustCodeGraph 读取 `pkg/util/sqlexec/restricted_sql_executor.rs`：核对 `GoError`、`RecordChunk`、`RecordSet`、`DrainRecordSet`、`DrainRecordSetAndClose` 与 `ExecSQL` 的接口和调用关系。
- RustCodeGraph 读取 `pkg/util/sqlexec/lib.rs` 与直接读取 `pkg/util/sqlexec/Cargo.toml`：核对模块再导出、默认 `formal-crate` feature、crate 边界和依赖来源。
- RustCodeGraph 读取 `pkg/util/sqlexec/simple_record_set.go`、`pkg/session/nontransactional.go` 对应构造点，以及 `pkg/session/nontransactional.rs` 的同名领域结构：核对 Go 算法、生产用途和 Rust 当前接线差异。
- `rg -n "SimpleRecordSet|DrainRecordSet|RecordSet" ...` 与 RustCodeGraph 读取 `pkg/util/sqlexec/migration_aster_unit_test.rs`、`pkg/server/conn_test.rs`：定位并核对独立 Rust 测试，确认分批、EOF、重置、allocator 和 server cursor 使用证据。
- 本任务是纯文档分析，按计划未运行 Cargo；最终仅执行任务指定的 11 章节结构校验。
