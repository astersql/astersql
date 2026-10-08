# `pkg/structure/structure.rs`

源码：[structure.rs](./structure.rs)。

## 文件定位

`structure.rs` 是 `astersql-structure` crate 的核心状态门面。`pkg/structure/lib.rs` 将它包含在私有的 `structure_impl` 模块中，再以 `pub use structure_impl::*` 导出其中的构造器、类型和错误；同一 crate 的 `string.rs`、`list.rs`、`hash.rs`、`type.rs` 继续为这里定义的 `TxStructure` 增加数据操作与键编码方法。

它位于业务数据结构语义与底层 `astersql-kv` 抽象之间：调用者提供一个只读 `kv::Retriever`、一个可选的 `kv::RetrieverMutator` 和命名空间前缀，后续 String、List、Hash 操作据此完成编码、读取和写入。`pkg/structure/Cargo.toml` 表明该 crate 直接依赖 `astersql-kv`、`astersql-util-dbterror` 和 `astersql-util-codec`，没有 feature 条件；本文件自身也没有条件编译项。

## 核心职责

- 注册并导出 Structure 错误类中的四个标准错误：哈希键标志非法、列表下标非法、列表元数据非法、快照写入非法。
- 用 `NewStructure` 将读取器、可选写入器和键前缀聚合为 `TxStructure`。构造过程不访问 KV，也不校验前缀。
- 保存所有结构操作共享的三项状态：读路径、写路径能力和命名空间前缀。
- 用 `TxStructure::writer` 集中实施“只读快照不得写入”的能力检查，并向具体 String/List/Hash 写方法提供可变的 `RetrieverMutator` 引用。

本文件不实现键编码或具体数据结构算法；这些职责分别位于 `type.rs`、`string.rs`、`list.rs` 和 `hash.rs`。

## 主要符号

- `ErrInvalidHashKeyFlag: LazyLock<Box<errors::Error>>`：以 `dbterror::ClassStructure.NewStd(mysql::ErrInvalidHashKeyFlag)` 延迟构造。`type.rs` 在解码出错误的 String/Hash 类型标志时使用它。
- `ErrInvalidListIndex: LazyLock<Box<errors::Error>>`：对应 MySQL 错误码 `ErrInvalidListIndex`；`list.rs::LSet` 在非空列表下标越界时生成该错误。
- `ErrInvalidListMetaData: LazyLock<Box<errors::Error>>`：`list.rs::loadListMeta` 读取到长度不是 16 字节的列表元数据时返回该错误。
- `ErrWriteOnSnapshot: LazyLock<Box<errors::Error>>`：表示缺少写能力。`writer()` 直接生成此错误；List/Hash 的部分写入口还会在执行读取或空输入短路之前显式检查 `readWriter.is_none()`。
- `pub fn NewStructure(reader, readWriter, prefix) -> TxStructure`：公开工厂，取得三个参数的所有权并原样存入结构体。Rust API 返回值本身，而 Go `NewStructure` 返回 `*TxStructure`。
- `pub struct TxStructure`：公开类型；三个字段均为 `pub(crate)`，外部 crate 只能通过公开方法使用它。`reader` 是 `Box<dyn kv::Retriever>`，`readWriter` 是 `Option<Box<dyn kv::RetrieverMutator>>`，`prefix` 是 `Vec<u8>`。
- `pub(crate) fn writer(&mut self) -> Result<&mut dyn kv::RetrieverMutator, errors::SharedError>`：crate 内写能力闸门；`Some` 时借用写入器，`None` 时返回新生成的 `ErrWriteOnSnapshot`。

## 执行流程

1. 上游准备实现 `kv::Retriever` 的读对象。可写事务再提供 `kv::RetrieverMutator`；快照读将第二个参数设为 `None`。
2. `NewStructure` 保存这两个 trait object 和 `prefix`，不产生底层读写或锁操作。
3. 具体方法先由 `type.rs` 使用 `prefix`、业务键和类型标志生成物理 KV 键。
4. 读方法通过 `reader` 执行 `GetValue`、正向迭代或反向迭代；因此即使 `readWriter` 为 `None`，读取仍可工作。
5. String 写方法（例如 `Set`、`Inc`、`Clear`）直接调用 `writer()?`；List/Hash 写方法通常先检查 `readWriter`，再在实际变更处调用 `writer()?`。任一方式都会阻止快照写入。
6. 写入器返回的错误、读取器错误或编码/解析错误由相邻实现向上传播；本文件只生成写快照错误。

当前 Rust 接线证据需要区分 crate 内部与应用级入口：RustCodeGraph 能确认 `TxStructure` 被相邻四个实现文件扩展，能确认 `NewStructure` 的直接 Rust 调用来自 `migration_aster_unit_test.rs` 和 `structure_test.rs`；没有检出生产 Rust 调用者。`pkg/tablecodec` 依赖该 crate，但其生产代码只使用 `HashData` 和 `TypeFlag`，不是本文件的构造器。故不能仅凭 Go 版本的使用范围断言 Rust 应用主链已经广泛接入该工厂。

## 数据与状态

`TxStructure` 自身不缓存业务值，也不持有显式事务生命周期对象；它持有的是动态分发的 KV 接口对象：

- `reader` 始终存在，是所有读路径的唯一数据源。
- `readWriter` 的 `Some/None` 同时编码运行能力：`Some` 表示允许变更，`None` 表示只读快照。该状态在构造后没有公开替换接口。
- `prefix` 被 `type.rs` 复制到每个编码键的开头，用来隔离不同结构实例的键空间。构造器不复制外部切片，而是直接接收已拥有的 `Vec<u8>`。

三个字段都不是公开 API。具体实现只能在当前 crate 内访问它们，外部调用者无法绕过方法直接替换 reader/writer 或前缀。`LazyLock` 错误对象按进程延迟初始化一次；每次实际报错再通过 `FastGenByArgs` 或 `GenWithStack` 生成错误实例。

## 依赖与调用关系

- 装配上游：`pkg/structure/lib.rs::structure_impl` 导入 `dbterror`、`errors`、`kv`、`mysql` 后 `include!("structure.rs")`，并公开再导出本文件符号。
- 类型依赖：`kv::Retriever` 负责读取和迭代；`kv::RetrieverMutator` 叠加写能力；`errors::SharedError` 是统一传播类型；`dbterror::ClassStructure` 与 `mysql::*` 错误码建立 SQL 错误映射。
- crate 内下游：`string.rs` 的 `Set`、`Inc`、`Clear` 调用 `writer()`；`list.rs` 的 push/pop/set/clear 以及 `hash.rs` 的 update/delete/clear 路径调用 `writer()`。`type.rs`、`string.rs`、`list.rs`、`hash.rs` 都读取 `prefix` 或 `reader`。
- 外部依赖：`pkg/tablecodec/Cargo.toml` 以 `structure-dependency` 引入本 crate，`tablecodec.rs` 使用该 crate 导出的 `TypeFlag`/`HashData` 来维持行键编码兼容；这条边说明 crate 的编码常量被复用，但不是 `TxStructure` 的运行时调用边。
- 已验证调用者：RustCodeGraph 对 `NewStructure` 给出的调用边来自 `migration_aster_unit_test.rs::{writable,string_round_trip_iteration_and_snapshot_errors_match_go,list_push_pop_index_set_and_clear_match_go,hash_crud_integer_and_reverse_iteration_match_go}`；`structure_test.rs::TestListAndHashSnapshotWritesFail` 也直接构造只读实例。

## 错误处理与边界

- `writer()` 不会 panic：写入器存在即返回可变借用；不存在即返回带 Structure 类和 `ErrWriteOnSnapshot` 错误码的 `SharedError`。
- `NewStructure` 不验证 `reader` 与 `readWriter` 是否指向同一底层事务，也不验证 `prefix` 唯一性或非空。这些是调用者必须维持的组合约束；错误组合可能造成读写视图不一致或键空间冲突。
- 四个静态错误只是错误类别入口。实际 List 元数据长度、下标范围和编码标志检查在相邻实现中完成，不能把这些校验归因于构造器。
- String 写入口依靠 `writer()` 统一报错；List/Hash 的若干入口会提前显式检查 `readWriter`，从而保证即使参数为空或后续读取本可短路，快照写调用仍返回写入错误。扩展写 API 时应保持这一可观察顺序与 Go 测试意图一致。
- `structure_test.rs::TestError` 验证四个错误都能转换为非 `ErrUnknown` 的 SQL 错误码，且转换后的 code 与注册 code 相同。

## 并发与资源生命周期

本文件没有创建线程、异步任务、通道、锁、文件句柄或网络连接。`Box<dyn ...>` 的释放遵循 `TxStructure` 的所有权生命周期；`writer()` 返回的可变借用受 `&mut self` 限制，借用期间不能通过同一实例并行再借用写入器。

这里没有声明 `Send`、`Sync` 或内部同步保证；能否跨线程共享取决于 KV trait 与具体实现的约束，调用者不应从 `TxStructure` 本身推断线程安全。测试用 `MemoryStore` 内部使用共享锁只是测试存储的实现细节，不是该类型承诺。

迭代器资源由 `string.rs`/`hash.rs` 等具体方法创建并关闭，而不是由本文件管理。底层事务或快照的提交、回滚和失效也由传入对象及其拥有者负责；`TxStructure` 没有显式 commit/rollback API。

## 与 Go 版本的对应关系

`pkg/structure/structure.go` 是直接语义基线：四个错误变量、`NewStructure` 的三个输入以及 `TxStructure` 的 `reader/readWriter/prefix` 字段逐项对应。Rust 使用 `LazyLock<Box<errors::Error>>` 表达延迟初始化的包级错误，用 trait object 表达 Go 接口，并用 `Option` 明确表达 Go `nil` 写入器。

主要差异如下：

- Go 构造器返回 `*TxStructure`，Rust 返回拥有所有权的 `TxStructure`；Rust 调用方需要可变变量才能执行写方法。
- Go 结构体字段是包内不可导出字段；Rust 字段是 `pub(crate)`，可供同 crate 的多个 `include!` 实现模块访问，但仍不向外部 crate 开放。
- Go 文件没有单独的 `writer()`；Go 的各写方法直接检查或使用 `readWriter`。Rust 增加该 crate 内辅助方法来统一安全地取得可变写入器，同时保留相同的快照写错误。
- Go 的 `nil` 同时可能出现在接口和值语义中；Rust 的 `Option<Box<dyn RetrieverMutator>>` 将“无写能力”显式化，但构造器仍不验证 reader 与 writer 的底层身份。

Go `structure_test.go` 覆盖 String/List/Hash 的可写事务与 nil writer 快照分支；Rust 的 `migration_aster_unit_test.rs` 移植主干行为，`structure_test.rs` 补充快照写和错误码验证。两边的核心不变量都是：读路径可在快照上使用，所有实际写路径必须失败且使用注册错误。

## 扩展指南

- 新增数据结构类型时，应继续复用 `TxStructure` 的 `reader`、`writer()` 和 `prefix`，把键编码放到独立实现文件，不要把算法堆入本文件。
- 新增写方法时，先决定快照检查的可观察顺序。若 Go 方法在空参数或读取前就拒绝快照，Rust 也应在同一阶段检查 `readWriter`；实际写入仍通过 `writer()`，避免 `unwrap` 或直接假设 `Some`。
- 新增 Structure 错误时，需要同步 dbterror/errno 定义、此处的延迟静态值、Go 同路径错误变量和独立 Rust 错误码测试。
- 修改字段类型或构造签名会影响 `string.rs`、`list.rs`、`hash.rs`、`type.rs` 以及测试辅助构造器 `migration_aster_unit_test.rs::writable`。应特别评估 trait object 所有权、对象安全和 reader/writer 是否仍共享一致视图。
- 修改前缀语义必须同步审查 `type.rs` 的全部编码/解码方法和 `pkg/tablecodec/tablecodec.rs` 对 `HashData`/`TypeFlag` 的兼容使用；风险包括已有元数据不可读和键范围扫描越界。
- 测试逻辑应继续放在独立文件 `structure_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。至少覆盖可写与只读构造、四类错误映射及新增 API 的 Go 对齐边界。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/structure` 列出本模块 16 个已索引文件。
- RustCodeGraph 源码/符号查询：`node --file pkg/structure/structure.rs --offset 1 --limit 260`；`query TxStructure --limit 20`；`node pkg/structure/structure.rs::TxStructure`；`node pkg/structure/structure.rs::writer`；`node NewStructure`。查询确认本文件 4 个静态错误、1 个工厂、1 个结构体、1 个内部方法，并给出 `NewStructure` 的测试调用边。
- crate 与装配：`pkg/structure/Cargo.toml`、`pkg/structure/lib.rs`。
- Rust 直接实现证据：`pkg/structure/structure.rs`、`pkg/structure/type.rs`、`pkg/structure/string.rs`、`pkg/structure/list.rs`、`pkg/structure/hash.rs`。
- Rust 独立测试证据：[structure_test.rs](./structure_test.rs)、[migration_aster_unit_test.rs](./migration_aster_unit_test.rs)；其中 `writable` 构造共享 reader/writer，快照测试以 `None` 构造只读门面。
- Go 对照证据：[structure.go](./structure.go)、[structure_test.go](./structure_test.go)，并参考同目录 `string.go`、`list.go`、`hash.go`、`type.go` 的方法职责。
- 外部直接依赖证据：`pkg/tablecodec/Cargo.toml`、`pkg/tablecodec/tablecodec.rs`。当前证据只支持编码常量复用，不支持声称生产 Rust 已调用 `NewStructure`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求文档存在且恰有 11 个固定二级标题。
