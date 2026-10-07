# `pkg/meta/meta_autoid.rs`

## 文件定位

`pkg/meta/meta_autoid.rs` 属于 `astersql-meta` crate，由 [`pkg/meta/lib.rs`](lib.rs) 的 `mod meta_autoid` 挂载并通过 `pub use meta_autoid::*` 对外再导出。crate 边界由 [`pkg/meta/Cargo.toml`](Cargo.toml) 定义：库入口是 `lib.rs`，此文件直接使用同 crate 的 `Mutator`、键编码函数、`errors` 与 `model::AutoIdGroup`。在元数据主链中，它位于 `Mutator` 与底层 `TxStructure` hash 操作之间，为单表的 RowID、AUTO_INCREMENT、AUTO_RANDOM 和 Sequence 水位提供类型化访问面。

直接上游入口是 [`pkg/meta/meta.rs`](meta.rs) 中的 `Mutator::get_auto_id_accessors(db_id, table_id)`，它调用本文件的 `new_auto_id_accessors`。[`pkg/meta/reader.rs`](reader.rs) 中 `Reader::get_auto_id_accessors` 的 `Mutator` 实现只是再委托给该入口。

## 核心职责

- 将 `(database_id, table_id, ID 种类)` 映射为 meta hash 的 `(DB:<database_id>, field)`，其中 field 为 `TID:<table_id>`、`IID:<table_id>`、`TARID:<table_id>`、`SID:<table_id>` 或 `SequenceCycle:<table_id>`。键选择分别由 `row_id`、`increment_id`、`random_id`、`sequence_value` 和 `sequence_cycle` 完成。
- 在不校验 schema/table 当前是否存在的前提下，提供单类 ID 的读取、覆盖、自增、跨表复制和删除。这是 rename/drop 与 ID 分配并发时的有意设计，依据是 `AutoIdAccessorImpl::inc` 与 Go `autoIDAccessor.Inc` 中的完整 schema/table ID 保留规则。
- 对 RowID、独立 AUTO_INCREMENT 和 AUTO_RANDOM 提供 `AutoIdGroup` 级别的顺序 `get`/`put`/`del`。Sequence 是 picker 可选字段，不属于 `AutoIdGroup`，因此不在这三个批量操作中。
- 保留表元数据 V5 的兼容分界：`table_version < TABLE_INFO_VERSION_5` 时 RowID 与 AUTO_INCREMENT 共用 `TID` field，V5 起 AUTO_INCREMENT 改用独立 `IID` field。

## 主要符号

- `pub trait AutoIdAccessor`：单个 ID field 的协议，定义 `get(&self)`、`put(&mut self, value)`、`inc(&mut self, step)`、`copy_to(&mut self, database_id, table_id)` 和 `del(&mut self)`，所有 IO 失败通过 `Result<_, errors::Error>` 上传。
- `pub type IdEncodeFn = fn(i64) -> Vec<u8>`：将 table ID 编码为 hash field 的函数指针；这是组访问器复用单一实例的切换点。
- `pub struct AutoIdAccessorImpl<'a>`：具体访问器，持有 `&'a Mutator`、创建时的 `database_id`/`table_id` 以及当前 `id_encode_fn`。其字段在当前 Rust 实现中均为 `pub`。
- `pub trait AutoIdAccessors`：`AutoIdGroup` 的批量读、写、删协议。
- `pub trait AccessorPicker`：按 ID 类型选择并返回 `&mut dyn AutoIdAccessor`的协议。`increment_id(table_version)` 是唯一依赖表版本的选择器。
- `const SEP_AUTO_INC_VER`：等于 `model::TABLE_INFO_VERSION_5`，用于区分共享 `TID` 与独立 `IID` 布局。
- `pub struct AutoIdAccessorsImpl<'a>`：整组控制器，内部只保存一个 `AutoIdAccessorImpl`，同时实现 `AccessorPicker` 和 `AutoIdAccessors`。
- `pub fn sequence_cycle_key(id)`：生成 `SequenceCycle:<id>` 字节键，与 Go `Mutator.sequenceCycleKey` 的格式相同。
- `pub fn new_auto_id_accessors(mutator, database_id, table_id)`：构造函数，初始编码函数为 `auto_table_id_key`；每个 picker 都会在返回前显式设置所需函数。

## 执行流程

1. 调用方通过 `Mutator::get_auto_id_accessors` 传入库 ID 和表 ID，最终由 `new_auto_id_accessors` 构造一个绑定该 `Mutator` 和完整 ID 对的 `AutoIdAccessorsImpl`。
2. 调用 `row_id`/`random_id`/`increment_id`/`sequence_value`/`sequence_cycle` 时，picker 先改写内部 `access.id_encode_fn`，再返回同一个访问器的可变 trait-object 引用。
3. `get`、`put`、`inc` 或 `del` 使用 `db_key(database_id)` 生成 hash 键，并用当前 `id_encode_fn(table_id)` 生成 field，然后分别调用 `hget_i64`、`hset`、`hinc` 或 `hdel`。
4. `copy_to` 先从源 field 读取当前值；值为 0 时立即成功返回，非 0 时使用相同 ID 类型的编码函数写入目标 `(database_id, table_id)`。
5. 组级 `get` 按 RowID → V5 IncrementID → RandomID 读取并组装 `AutoIdGroup`；`put` 和 `del` 也保持这一顺序。任一步返回错误时，`?` 使后续步骤不再执行。

## 数据与状态

按 `AutoIdAccessorImpl` 的键组合，一个值存在 `DB:<database_id>` hash 下的某个 field 中。底层以十进制 ASCII 字节保存 `i64`：`put` 使用 `value.to_string().as_bytes()`，`get` 由 `hget_i64` 解析，缺失 field 在当前 `TxStructure` 实现中返回 0。

`AutoIdAccessorsImpl` 的可变状态不是 ID 值本身，而是 `id_encode_fn`。因为所有 picker 共享同一 `access`，每次选择都覆盖上次类型；返回的 `&mut dyn AutoIdAccessor` 的借用期又阻止调用者同时重新选择另一类 field。`database_id` 和 `table_id` 在访问器生命期内不变，以保留 rename 前的原始标识。

`AutoIdGroup` 在当前 crate 内由 [`pkg/meta/harness.rs`](harness.rs) 的 `model` 模块提供，包含 `row_id`、`increment_id`、`random_id` 三个 `i64`。虽然 `pkg/meta/Cargo.toml` 声明了 `astersql-meta-model`，本文件的 `crate::model` 实际由 `lib.rs` 再导出的 harness 模型满足；这是当前 crate 接线事实，不应误写为已直接使用完整 `astersql-meta-model` 表模型。

## 依赖与调用关系

- 上游：`Mutator::get_auto_id_accessors` → `new_auto_id_accessors`；`Reader for Mutator::get_auto_id_accessors` → `Mutator::get_auto_id_accessors`。`Mutator::drop_sequence` 会取得组访问器并调用 `del`，然后另行删除 sequence value field。
- 下游：`AutoIdAccessorImpl::{get,put,inc,del,copy_to}` → `Mutator.txn::{hget_i64,hset,hinc,hdel}`；键生成依赖 `db_key`、`auto_table_id_key`、`auto_increment_id_key`、`auto_random_table_id_key`、`sequence_key` 和本文件的 `sequence_cycle_key`。
- 模型：组读写依赖 `model::AutoIdGroup`，版本分界依赖 `model::TABLE_INFO_VERSION_5`。
- crate 接线：`pkg/meta/lib.rs` 把本模块公开项再导出；`pkg/meta/Cargo.toml` 声明了 `anyhow`、KV、codec、model、serde 等 crate 级依赖，但本文件的直接 IO 仍经由 `Mutator.txn` 封装。
- RustCodeGraph 对 `new_auto_id_accessors`、`sequence_cycle_key` 的精确 `callers/callees` 查询返回空边；因此上述直接调用关系是用索引源码 `node` 与文本引用搜索双重核验，没有借用同名 Go 符号的全局图结果。

## 错误处理与边界

- 本文件不创建业务错误变体；所有底层 `TxStructure` 错误均通过 `Result` 和 `?` 原样向上传。数值不是有效 UTF-8/十进制 `i64`时，错误由 `hget_i64`/`hinc` 的解析阶段产生。
- 缺失 field 视为 0；删除不存在的 field 在当前底层实现中成功返回。这使 `del` 可重复执行，也意味着 `get == 0` 无法区分“键不存在”与“显式写入 0”。
- `inc` 故意不检查库或表存在性；Go `pkg/meta/meta_test.go` 明确覆盖了不存在表 ID 和库 ID 仍可自增的行为。
- `copy_to` 中的零值短路是兼容性保护：在高版本 BR 恢复到低版本 TiDB 等 rename 时序中，源值可能已被删除，不应用 0 覆盖目标已有水位。
- 组级 `put`/`del` 不是本文件自行开启的原子批处理；前一 field 成功、后一 field 失败时，本层不回滚已完成的写/删。是否由更外层事务回滚取决于 `Mutator` 所绑定的实际交易实现，本文件未单独保证。
- 组级方法始终以 V5 选择独立 IncrementID field；老表共享 field 需调用者显式使用 `increment_id(actual_table_version)`。

## 并发与资源生命周期

`AutoIdAccessorImpl<'a>` 借用 `&'a Mutator`，因此不拥有事务，也不负责 commit、rollback 或关闭资源；访问器不能比 `Mutator` 活得更久。picker 需要 `&mut self`，并返回内部访问器的 `&mut` 引用，这使“切换类型函数”和紧随的 field 操作在 Rust 借用范围内不会被另一 picker 调用交叉更改。

本文件不创建线程、异步任务、通道或锁。当前 `pkg/meta/harness.rs` 的 `TxStructure` 使用共享 `Arc<Mutex<State>>`：`hinc` 在一次锁持有期内读-改-写，因而在该内存实现中是原子的；`hset`/`hdel` 也在内部加锁。这些是底层实现的并发性质，不是 `meta_autoid.rs` 自行提供的跨 field 原子性。由 snapshot 构造的只读 `Mutator` 可执行 `get`，而 `put`/`inc`/`del`/copy 的目标写入会由底层 `ensure_writable` 拒绝为 `write on snapshot`。

## 与 Go 版本的对应关系

Rust 文件按 [`pkg/meta/meta_autoid.go`](meta_autoid.go) 的同序结构移植：`AutoIdAccessor` 对应 `AutoIDAccessor`，`AutoIdAccessorImpl` 对应 `autoIDAccessor`，`AutoIdAccessors`/`AutoIdAccessorsImpl` 对应 `AutoIDAccessors`/`autoIDAccessors`，`AccessorPicker` 同名，`SEP_AUTO_INC_VER` 对应 `sepAutoIncVer`，`new_auto_id_accessors` 对应 `NewAutoIDAccessors`。两者的键选择、V5 分界、组操作顺序、rename 时保留完整 ID 以及 `CopyTo` 的零值保护一致。

已核实的差异有：

- Go `AutoIDAccessors` interface 显式嵌入 `AccessorPicker`；Rust `AutoIdAccessors` trait 没有以 supertrait 约束 `AccessorPicker`。当前具体 `AutoIdAccessorsImpl` 同时实现两者，但单独的 `dyn AutoIdAccessors` 类型面不自动包含 picker 方法。
- Go 实现类型和字段为包内私有；Rust 实现结构及其字段是 `pub`，对外暴露面更宽。
- Go `Del` 对 `HDel` 错误调用 `errors.Trace`；Rust `del` 直接返回底层错误。当前 Rust `errors::Error` 是 `anyhow::Error` 别名，没有复制 Go 的 stack-trace 包装语义。
- Go 构造器返回 `AutoIDAccessors` interface；Rust 构造器返回具体 `AutoIdAccessorsImpl<'_>`，因此调用方可直接使用两个 trait 的方法。

## 扩展指南

- 新增 ID 种类时，先在键布局定义处添加唯一编码函数，再扩展 `AccessorPicker` 和其 `AutoIdAccessorsImpl` 实现。需明确它是否进入 `AutoIdGroup`；若进入，必须同步修改组级 `get`/`put`/`del` 的顺序、`model::AutoIdGroup` 及 Go 对照结构。
- 修改表版本兼容规则时，聚焦 `SEP_AUTO_INC_VER` 与 `increment_id`，并同时验证旧版本的 RowID/IncrementID 共键和新版本分键行为。不能只改组级方法中固定传入的版本。
- 修改 `copy_to` 时必须保留或用等价方案替代零值不覆盖保护，并重验跨版本 BR/TiDB rename 场景。
- 若要缩小公开 API 或使 Rust trait 层级与 Go 对齐，需先检查 `pub` 字段的外部构造用法，并评估将 `AutoIdAccessors: AccessorPicker` 设为 supertrait 对 trait object 和下游约束的兼容影响。
- 测试不应内嵌到本生产文件。直接 Rust 回归应继续放在同目录独立的 [`pkg/meta/migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 或 [`pkg/meta/meta_test.rs`](meta_test.rs)；Go 对照回归位于 [`pkg/meta/meta_test.go`](meta_test.go)。至少覆盖缺失键返回 0、正负 step、V4/V5 分键、零/非零复制、中途错误顺序以及 snapshot 写拒绝。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，`pkg/meta/meta_autoid.rs` 被索引为 215 行、34 个符号；通过 `node --file` 读取了目标文件、`pkg/meta/lib.rs`、`pkg/meta/meta.rs`、`pkg/meta/reader.rs`、`pkg/meta/harness.rs`、Rust 测试和 Go 对照文件。
- RustCodeGraph 符号核验：查询到 `AutoIdAccessors`、`AutoIdAccessorsImpl`、`AutoIdAccessorImpl`、`AccessorPicker`、`new_auto_id_accessors` 和 `sequence_cycle_key`；对后两者的精确 `callers/callees` 图查询为空，后续用 `meta.rs:311-313`、`reader.rs:185-187` 与 `meta.rs:1404-1407` 的源码引用补足调用证据。
- Rust 行为测试：[`pkg/meta/migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `auto_id_picker_copy_and_group_order_match_go` 验证组 put/get、RowID 自增、V4 共享 TID、V5 独立 IID、非零跨表复制、零值不覆盖和组删除；[`pkg/meta/meta_test.rs`](meta_test.rs) 的 `test_meta` 验证建表后组值读取与 RowID 自增。
- Go 行为对照：[`pkg/meta/meta_autoid.go`](meta_autoid.go) 提供逐符号实现依据；[`pkg/meta/meta_test.go`](meta_test.go) 覆盖 RowID 增读、删除后回到 0、不存在库/表仍可自增以及建表设置 AutoID。
- crate 与底层证据：[`pkg/meta/Cargo.toml`](Cargo.toml) 核对 crate 入口和依赖；[`pkg/meta/harness.rs`](harness.rs) 的 `TxStructure::{hget_i64,hset,hinc,hdel}` 核对缺失值、十进制存储、锁范围和只读 snapshot 错误。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验收使用任务指定的 `test -f` 与 11 个固定二级标题计数命令。
