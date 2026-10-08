# `pkg/table/index.rs`

## 文件定位

`pkg/table/index.rs` 属于 `astersql-table` crate，由 `pkg/table/lib.rs` 以 `pub mod index` 挂载并通过 `pub use index::*` 重导出。它对应 Go 的 `pkg/table/index.go`，位于表层 DML 与 KV 索引编码之间：定义“索引实现必须提供哪些操作”，并提供把一行的普通或多值索引数据逐项转换成 key/value 的有状态生成器。

crate 边界由 `pkg/table/Cargo.toml` 确认：本文件直接依赖 `astersql-kv` 的 `Handle`/`Transaction`、`astersql-meta-model` 的 `IndexInfo`/`TableInfo`、`astersql-types` 的 `Datum`、`astersql-errctx`、`astersql-util-chunk`、`astersql-table-tblctx` 和 `chrono-tz`。它不实现具体字节编码或 KV 写入策略，而是将这些行为留给 `Index` trait 实现者。

## 核心职责

- 用 `IndexIterator` 描述从 KV 索引中读取“索引列值 + 行 handle”的迭代与关闭契约。
- 用 `DupKeyCheckMode`、`PessimisticLazyDupKeyCheckMode`、`commonMutateOpt`、`CreateIdxOpt` 和 `CreateIdxOption` 表达创建索引项时的重复键检查、KV 上下文、事务断言与 DDL backfill 来源。
- 用 `IndexMutateContext` 对 `tblctx_dependency::MutateContext` 做对象安全的窄化视图，仅暴露索引变更需要的表达式上下文、连接 ID 和变更缓冲。
- 用 `Index` trait 统一索引元数据查询、部分索引条件判断、增删/存在性检查、key/value 生成和索引列提取。
- 用 `IndexKVGenerator` 在不预先生成全部 KV 的情况下，逐组调用 `GenIndexKey` 和 `GenIndexValue`；普通索引产生一组，多值索引按 `all_index_values` 产生多组。

## 主要符号

- `IndexResult<T> = Result<T, errors_dependency::SharedError>`：本文件公开操作的统一错误类型。
- `IndexIterator::{Next, Close}`：`Next` 返回 `Vec<Datum>` 和动态 `Handle`，`Close` 是显式资源释放钩子。该 trait 只定义契约，本文件中无默认实现。
- `DupKeyCheckMode`：`DupKeyCheckInPlace` 为默认值，另有 `DupKeyCheckLazy` 和 `DupKeyCheckSkip`。`#[repr(u8)]` 固定了枚举判别值 0/1/2。
- `PessimisticLazyDupKeyCheckMode`：默认在 acquire-lock 阶段检查，可切换为 prewrite 阶段检查；同样以 `u8` 表示。
- `commonMutateOpt`：保存可选 `kv_dependency::Context` 与两种重复键模式。字段仅 crate 内可见，通过 `Ctx`、`DupKeyCheck`、`PessimisticLazyDupKeyCheck` 读取。
- `CreateIdxOpt` / `NewCreateIdxOpt`：在公共变更选项之上加入 `ignoreAssertion` 和 `fromBackFill`。构造函数从 `Default` 开始，按切片顺序调用每个 `CreateIdxOption::applyCreateIdxOpt`，后面的选项因此可以覆盖前面的状态。
- `WithIgnoreAssertion` / `FromBackfill`：静态零大小选项值，分别将两个布尔标志置为 `true`。`DupKeyCheckMode` 和 `PessimisticLazyDupKeyCheckMode` 自身也实现 `CreateIdxOption`。
- `IndexMutateContext`：定义 `GetExprCtx`、`ConnectionID`、`GetMutateBuffers`。对所有满足 `tblctx_dependency::MutateContext` 且关联 `ExprContext: Sized` 的类型提供 blanket impl，方法均直接委派。
- `Index`：核心 trait。`Meta`/`TableMeta` 暴露元数据；两个 `MeetPartialCondition*` 分别接受 Datum 切片和 chunk `Row`；`Create`/`Delete` 处理写路径；`Exist` 查重；`GenIndexKey`/`GenIndexValue` 负责实际编码；`FetchValues` 支持复用输出容器。`GenIndexKVIter` 返回借用当前实现的 `IndexKVGenerator<'index, Self>`，因而带 `Self: Sized` 约束，其余方法仍能通过 `dyn Index` 使用。
- `IndexKVGenerator<'index, I>`：借用索引实现，拥有错误上下文、时区、handle、handle 还原数据与索引数据，并以 `cursor` 维护迭代位置。
- `NewMultiValueIndexKVGenerator` / `NewPlainIndexKVGenerator`：分别初始化多组值与单组值路径。普通路径不存储多值列表，多值路径的 `index_values` 为空。
- `IndexKVGenerator::{Next, Valid}`：`Valid` 是调用 `Next` 前的状态检查；`Next` 选择当前值组，先生成 key 和 `distinct`，再以 `untouched = false` 生成 value，两步都成功后才增加游标。

## 执行流程

1. 写入方根据 AddRecord/UpdateRecord 或直接创建索引的语义组装 `CreateIdxOption` 列表。`NewCreateIdxOpt` 或 `AddRecordOpt::GetCreateIdxOpt` / `UpdateRecordOpt::GetCreateIdxOpt` 得到最终快照。
2. 调用者必须先使用 `MeetPartialCondition` 或 `MeetPartialConditionWithChunk` 判断当前行是否应该进入部分索引。`Index::Create`、`Delete` 和 `GenIndexKVIter` 的契约明确不内置该判断。
3. 对需要生成 KV 的行，具体 `Index` 实现的 `GenIndexKVIter` 根据索引类型调用 `NewPlainIndexKVGenerator` 或 `NewMultiValueIndexKVGenerator`，并转移行级输入。
4. 调用者以 `while generator.Valid()` 形式驱动生成器。每次 `Next` 将调用者给出的 key/value 缓冲区分别传入 `GenIndexKey` 和 `GenIndexValue`，使实现可以复用容量或保留前缀。
5. `GenIndexKey` 返回的 `distinct` 原样传给 `GenIndexValue`；`GenIndexValue` 还接收 handle、handle 还原数据和固定的 `untouched = false`。只有两个编码操作均成功，`cursor` 才前进。
6. 普通生成器初始 `cursor == 0`，一次成功后即无效；多值生成器在 `cursor < all_index_values.len()` 时有效，按保存顺序访问每组值。

## 数据与状态

`CreateIdxOpt` 是一次变更的值对象：公共部分可由 `table.rs` 的 `WithCtx`、`DupKeyCheckMode` 和 `PessimisticLazyDupKeyCheckMode` 写入，两个索引专用标志由零大小选项置位。`Ctx()` 返回 clone，而枚举和布尔值按值返回；外部不能直接改写内部字段。

`IndexKVGenerator` 把不变的行级环境与可变的游标放在一起。`index` 是带生命周期的借用，其余输入由生成器拥有，因此生成期间不需要从调用者借用 Datum 容器或 handle。普通和多值模式共用一个结构，由 `is_multi_value` 选择 `index_values` 或 `all_index_values[cursor]`。错误时不改变 `cursor`，故当前项仍可重试；但如果不先检查 `Valid`就在多值耗尽后再调用 `Next`，会因切片越界而 panic。

## 依赖与调用关系

上游接线中，`pkg/table/lib.rs` 重导出本文件 API；`pkg/table/table.rs` 导入 `CreateIdxOpt`、`CreateIdxOption`、两种重复键模式、`Index` 和 `IndexMutateContext`。其中 `AddRecordOpt::GetCreateIdxOpt` 与 `UpdateRecordOpt::GetCreateIdxOpt` 通过 `CreateIdxOpt::from_common` 传递公共状态，`CommonMutateOptFunc` 通过 `common_mutate_opt_mut` 同时适配 Add/Update/Create 三类选项，`MutateContext` 则以 `IndexMutateContext` 为父 trait。

下游调用由具体 `Index` 实现注入：`IndexKVGenerator::Next -> Index::GenIndexKey -> Index::GenIndexValue`，错误以 `?` 原样传播。`Index` 的写操作还依赖 `kv_dependency::{Transaction, Handle}`，部分索引条件路径依赖 `Datum` 或 `chunk_dependency::Row`，时间值编码通过 `chrono_tz::Tz` 显式传入时区。

RustCodeGraph 将 `pkg/table/index.rs` 标记为被 14 个索引文件使用，但精确符号搜索显示，当前 Rust 生产代码的直接连线主要是上述 `table.rs` 选项与上下文契约。`pkg/table/tables/index.rs` 定义了同名的具体 `Index` 结构及自有 `gen_index_key`/`gen_index_value` 方法，但在当前源码中没有为它实现本文件的 `table::Index` trait。因此不应把 Go `tables/index.go` 的完整生产调用链视为已经由这个 Rust trait 接通。

## 错误处理与边界

所有可失败的契约统一返回 `IndexResult`。生成器的 key 生成失败时不调用 value 生成；value 生成失败时已经发生的 key 生成调用不回滚，但游标不前进。这个语义要求实现者的两个编码函数不应在错误前产生不可重试的外部副作用。

`Create`/`Delete`/`GenIndexKVIter` 不替调用者检查部分索引谓词；遗漏前置 `MeetPartialCondition*` 会导致多写或多删索引项。`GenIndexKey` 的契约要求多值索引走 `GenIndexKVIter`，不应由上游手动把整个多值容器当作普通值组编码。`FetchValues` 的 `columns` 是可复用容器，实现必须尊重其容量/内容契约。

`Next` 不自行检查耗尽状态：普通路径在第二次调用时仍会重用 `index_values`，多值路径在耗尽后会越界 panic。正确 API 用法是始终以 `Valid` 作为循环守卫。`IndexIterator::Close` 没有 RAII 默认实现，迭代器实现和调用者必须明确约定谁负责关闭。

## 并发与资源生命周期

本文件不启动任务、不建立通道，也不自己持有锁。`IndexKVGenerator::Next` 需要 `&mut self`，游标更新因此在类型层面串行化；结构未声明 `Send`/`Sync` 边界，是否可跨线程取决于具体 `Index`、`Handle`、错误上下文和 Datum 类型的自动 trait。不应在没有进一步约束与审查时将同一生成器并发驱动。

生成器的 `'index` 生命周期保证它不能比底层索引实现活得更久；其他可变输入被转移进结构。key/value 缓冲区每次由调用者传值进入编码函数，成功后返回新所有权；这是容量复用点，也是避免每个索引项固定重新分配的性能接口。事务和变更缓冲的真实资源生命周期由 `Transaction` 实现和上游 `MutateContext` 所有者管理，本文件只借用它们。

## 与 Go 版本的对应关系

Rust 基本保留了 `pkg/table/index.go` 的名称和操作顺序：`IndexIterator`、`CreateIdxOpt`、`CreateIdxOption`、`Index`、`IndexKVGenerator`、两个构造函数以及 `Next`/`Valid` 都能一一对应。Go `NewCreateIdxOpt(opts ...CreateIdxOption) *CreateIdxOpt` 在 Rust 中是选项 trait-object 切片并按值返回；Go 的 interface/nil 语义则用 trait object、`Box<dyn Handle>` 和 `Option<Box<dyn Handle>>` 表达。Go `*time.Location` 在 Rust 中收窄为可拷贝的 `chrono_tz::Tz`。

`IndexKVGenerator::Next` 与 Go 顺序一致：先取当前值、再生成 key、再生成 value，只在都成功后增加游标；`Valid` 对普通索引仍以游标是否为 0 判断。`FromBackfill` 的语义也保留了 Go 注释的关键约束：backfill-merge 时 DML 产生的 KV 可被重定向到临时索引，DDL backfill worker 自身产生的 KV 不应被重定向。

差异和迁移边界是：Go 的完整生产具体实现位于 `pkg/table/tables/index.go`，其 `GenIndexKVIter` 可真正返回 `table.IndexKVGenerator`；Rust `pkg/table/tables/index.rs` 当前并未实现这里的 `Index` trait，而且使用了该模块自己的精简元数据与 Datum 类型。因此本文件的契约、选项组装和生成器逻辑有独立测试，但与生产具体索引实现的端到端接线尚不能从当前 Rust 源码证明。

## 扩展指南

- 新增 `Index` 能力时，先保持 `pkg/table/index.go` 契约对齐，再同步所有 Rust `impl Index`；当前可搜索到的直接 impl 为 `pkg/table/index_migration_aster_unit_test.rs::RecordingIndex` 和 `pkg/util/admin/admin_integration_test.rs::UniqueIndex`。不应为通过编译而给新方法填写无行为的桩。
- 要接通真实索引写路径，应在审查类型转换、元数据语义、KV 交易和部分索引求值后，为 `pkg/table/tables/index.rs` 的具体类型添加完整 trait 实现；不要只把现有精简 `gen_index_key`/`gen_index_value` 包一层就宣称 Go 语义已完成。
- 新增创建索引选项时，把字段放入 `CreateIdxOpt` 或 `commonMutateOpt`，实现 `CreateIdxOption`，并检查该选项是否也应通过 Add/Update 选项传递。通用状态应沿用 `CommonMutateOptFunc` 适配模式，避免三套设置逻辑漂移。
- 修改 `Next` 时必须保留 key-before-value、`distinct` 传递、`untouched = false`、输入缓冲复用与“全部成功后才前进”的不变量。如果计划将耗尽后的 panic 改为错误，这是与 Go 当前行为的显式 API 变更，需单独做兼容性决策。
- 索引契约与生成器回归应优先扩展独立测试 `pkg/table/index_migration_aster_unit_test.rs`；选项在 Add/Update/Create 间的传递同时更新 `pkg/table/table_test.rs` 和 `pkg/table/table_migration_aster_unit_test.rs`，并与 Go `pkg/table/table_test.go` 对照。真实实现接线后，还需在独立的 `pkg/table/tables/index_test.rs` 中覆盖编码、唯一/含 NULL、多值和部分索引边界；不要把测试内嵌到生产 `.rs` 文件。
- 性能审查重点是 Datum 集合的所有权转移、handle 动态分派和 key/value 缓冲复用；兼容性审查重点是重复键检查时机、backfill 重定向、时区编码、部分索引前置判断与错误后重试语义。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个符号、1848419 条边；`files --filter pkg/table` 确认目标、Go 对照和独立测试都在索引中。
- RustCodeGraph 源码查询：`node --file pkg/table/index.rs --offset 1 --limit 260` 与 `--offset 251 --limit 260` 覆盖全部 468 行；`query` 确认 Rust/Go 的 `NewCreateIdxOpt`、`NewMultiValueIndexKVGenerator`、`IndexKVGenerator` 和 `Index` 候选。`callers`/`callees` 对限定名称未返回边，因此又以精确符号搜索补证直接接线，未把缺失的图边当成“无调用者”。
- 读取的生产路径：`pkg/table/index.rs`、`pkg/table/lib.rs`、`pkg/table/Cargo.toml`、`pkg/table/table.rs`、`pkg/table/tables/index.rs`。其中 Cargo 声明和 `lib.rs` 证明 crate/重导出边界，`table.rs` 证明选项与上下文接线，`tables/index.rs` 证明当前具体 Rust 索引类型尚未实现本 trait。
- Go 对照：`pkg/table/index.go` 的 204 行全文验证公共契约、选项标志、生成次序和游标语义；`pkg/table/table_test.go:50-83` 验证默认值与 `WithCtx`/`WithIgnoreAssertion`/`FromBackfill` 组合。Go 生产 `pkg/table/tables/index.go::GenIndexKVIter` 仅作为完整实现对照，未将其调用链误记为 Rust 已接线。
- Rust 独立测试：`pkg/table/index_migration_aster_unit_test.rs` 验证选项默认/累积、`dyn Index` 对象安全、普通生成器一次性与缓冲前缀保留、多值顺序遍历，以及 key/value 失败时游标不前进。`pkg/table/table_test.rs:120-150` 与 `pkg/table/table_migration_aster_unit_test.rs:90-147` 验证公共变更选项向 `CreateIdxOpt` 传递。
- 精确 `rg` 补证：搜索 `impl Index for`、`GenIndexKVIter(`、`NewCreateIdxOpt`、两个 generator 构造函数与选项符号，用于弥补 RustCodeGraph 调用边输出为空的部分，并确认相关 Rust 测试位于独立文件。
- 本任务仅新增说明文档，按计划不运行 Cargo。文档结构用任务指定的 `test -f` + `rg -c` 命令验证；代码行为结论均来自上述源文件、符号查询和测试断言，未以理想架构代替当前迁移状态。
