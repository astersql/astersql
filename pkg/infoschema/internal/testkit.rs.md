# `pkg/infoschema/internal/testkit.rs`

## 文件定位

本文件属于独立 crate `astersql-infoschema-internal`。crate 入口
[`pkg/infoschema/internal/lib.rs`](./lib.rs) 将 `testkit` 声明为公开模块，并把本文件的公开项再导出；根门面
[`pkg/lib.rs`](../../lib.rs) 又通过 `pkg::infoschema::internal` 暴露该 crate。它是 InfoSchema 测试夹具，不在 SQL 请求、DDL 执行或 InfoSchema 构建的生产主链上。

[`pkg/infoschema/internal/Cargo.toml`](./Cargo.toml) 表明该 crate 的常规依赖只有
`astersql-store-mockstore`。上层 `astersql-infoschema` 对它的依赖位于
`[target.'cfg(any())'.dependencies]`；`cfg(any())` 永远为假。因此，当前 Rust 代码中真实执行本文件逻辑的直接证据仅见同 crate 的独立测试
[`pkg/infoschema/internal/testkit_test.rs`](./testkit_test.rs)，不能把 Go 测试中广泛使用同名 helper 的事实等同为 Rust 已完成接线。

## 核心职责

本文件提供两类彼此独立的测试能力：

1. `PrepareSlowLogfile` 与 `SLOW_LOG_SAMPLE` 生成两条固定慢日志记录，供慢日志解析场景复用。
2. `TestStore` 组合真实的 `MockStorage` 句柄和一份进程内 `MetadataState`，并在其上提供 ID 分配、最小化库表模型、资源组和 Placement Policy 的构造及增删改 helper。

这里的元数据 helper 是 Go `pkg/infoschema/internal/testkit.go` 的轻量 Rust 对照层。它保留测试所需的状态变化、存在性检查和部分幂等语义，但没有把数据写入底层 mock TiKV，也没有调用 Rust 的生产 Meta/InfoSchema 实现。它的主要价值是让迁移中的 Rust 测试可以显式验证夹具自身的契约，而不是替代生产元数据层。

## 主要符号

- `SLOW_LOG_SAMPLE: &str`：包含 2019 和 2021 两条慢日志记录，覆盖事务耗时、Coprocessor、缓存命中、资源组、RU、CPU 和存储来源等字段。
- `PrepareSlowLogfile(path) -> std::io::Result<()>`：以创建、只写和 `0600` 模式打开路径，写入完整样例并刷新缓冲区；错误原样通过 `?` 返回。
- `CiString { original, lower }` 与 `CiString::new`：缓存原始名称及 `to_lowercase()` 结果，模拟 Go `ast.CIStr` 的测试所需部分。
- `SchemaState::{None, Public}`、`FieldType::LongLong`：只表达当前夹具用到的可见性和列类型。
- `ColumnInfo`、`TableInfo`、`DbInfo`、`ResourceGroupInfo`、`PolicyInfo`、`PolicyRefInfo`、`Table`：Go `model`/`table` 对象的最小化值类型。`TableInfo::revision` 是 Rust 测试用的更新计数，不是 Go `model.TableInfo` 中的对应字段。
- `MetadataState`：私有状态，保存 `global_id` 以及按 ID 建索引的库、表、资源组、策略映射；表键是 `(db_id, table_id)`。
- `TestStore(Arc<TestStoreInner>)`：可克隆句柄。`TestStoreInner` 同时持有 `MockStorage` 和 `Mutex<MetadataState>`；`storage()` 只读暴露 mock 存储引用。
- `TestStore::transaction`：获取元数据互斥锁、克隆进入操作前的完整快照；闭包返回错误时整体恢复快照。
- `TestkitError(String)`：夹具统一错误，提供 `Display` 和 `std::error::Error` 实现。
- `AutoIdRequirement`、`MockAutoIdRequirement`、`CreateAutoIDRequirement`、`CreateAutoIDRequirementWithStore`：封装 `TestStore`；远端 AutoID 客户端在当前模型中固定为 `None`。
- `GenGlobalID`：在事务内递增 `MetadataState::global_id`，再按 Go 测试约定给返回值加 `100`；默认状态的首次结果是 `101`。
- `MockDBInfo`、`MockTableInfo`、`MockTable`、`MockResourceGroupInfo`、`MockPolicyInfo`、`MockPolicyRefInfo`：构造公开状态的最小测试对象。表固定含一列公开的 `BIGINT` 语义列 `a`。
- `AddTable`、`UpdateTable`、`DropTable`、`AddDB`、`DropDB`、`UpdateDB`：操作库表内存索引。
- `AddResourceGroup`、`UpdateResourceGroup`、`DropResourceGroup`、`CreatePolicy`、`UpdatePolicy`、`DropPolicy`：操作资源组和放置策略内存索引。

所有上述函数和类型都没有条件编译标注。测试模块由 `lib.rs` 使用
`#[cfg(test)]` 和 `#[path = "testkit_test.rs"]` 单独接入，符合源文件与测试文件分离的仓库约束。

## 执行流程

典型库表夹具流程如下：

1. `CreateAutoIDRequirement(options)` 调用 `TestStore::new`，后者通过 `NewMockStore` 创建底层 `MockStorage`，并初始化空的 `MetadataState`。
2. 调用者从 `AutoIdRequirement::store()` 取得 `TestStore`。
3. `MockDBInfo` 调用一次 `GenGlobalID`，创建 `Public` 数据库；`MockTableInfo` 先为列、再为表各分配一个 ID，创建含单列 `a` 的 `Public` 表。
4. `AddDB` 把数据库写入 `databases`；`AddTable` 先确认库存在，再把表写入以 `(db_id, table.id)` 为键的 `tables`。
5. `UpdateTable` 先确认目标表存在，然后对调用者传入的 `TableInfo::revision` 做 wrapping 加一，并把克隆值覆盖到映射；不存在时既返回错误，也不改变调用者的 revision。
6. `DropTable` 删除精确键且在缺失时出错；`DropDB` 无论数据库是否存在都成功，并通过 `retain` 级联移除该库的所有表。

资源组和策略使用相同的事务包装。新增操作拒绝零 ID，并借助“先 `insert`、发现旧值后返回错误、随后快照回滚”保证重复 ID 不会覆盖旧记录。更新通常要求记录已存在；唯一例外是 `UpdateResourceGroup` 对 ID `1` 的默认资源组允许直接插入，因为 Go 侧默认资源组可能由系统合成而未持久化。删除资源组、策略和数据库采用幂等语义。

慢日志路径不经过 `TestStore`：`PrepareSlowLogfile` 直接打开文件、写入 `SLOW_LOG_SAMPLE` 并 `flush`。

## 数据与状态

`MetadataState` 是夹具逻辑的唯一可变元数据源。`TestStore` 的多个克隆共享同一个
`Arc<TestStoreInner>`，因此也共享一个 `global_id` 计数器和四张映射。状态没有公开查询 API；当前只能通过操作返回值、重复操作结果以及传入对象的变化间接观察。

`TestStore::transaction` 每次都克隆整个 `MetadataState`。这给单次 helper 调用提供原子提交/回滚效果，但成本与当前库、表、资源组、策略总量线性相关，适合小规模测试夹具，不适合作为生产元数据事务实现。互斥锁覆盖快照、闭包执行和可能的恢复全过程，因此同一 `TestStore` 上的元数据操作被串行化。

底层 `MockStorage` 与 `MetadataState` 没有同步关系：`storage()` 返回的存储可被其他代码使用，但本文件的 `AddDB`、`AddTable` 等只更新内存映射。`MockAutoIdRequirement::auto_id_client()` 的 `Option<()>` 只是“没有远端客户端”的占位表达。

名称比较所需的小写值在 `CiString::new` 时一次性生成；公开字段允许调用者之后分别修改 `original` 和 `lower`，类型本身不强制两者持续一致。

## 依赖与调用关系

上游边界：

- `pkg/infoschema/internal/lib.rs` 声明并再导出 `testkit`，同时在测试配置下引入 `testkit_test.rs`。
- 根 `Cargo.toml` 以 `facade_infoschema_internal` 注册该 crate，`pkg/lib.rs` 把它再导出到 `pkg::infoschema::internal`。
- 当前 Rust 源码的路径限定搜索未发现独立测试之外对 `PrepareSlowLogfile`、AutoID helper 或元数据增删改 helper 的调用；集群慢日志 Rust 测试只在注释中引用 Go helper，并维护自己的等价夹具。

下游边界：

- `TestStore::new` 调用 `astersql_store_mockstore::NewMockStore`，并保存其 `MockStorage`。
- 所有元数据 helper 最终只调用 `TestStore::transaction` 和标准库 `HashMap` 操作。
- `PrepareSlowLogfile` 只依赖标准库文件 API。

Go 调用面明显更广：`pkg/infoschema/infoschema_v2_test.go`、`infoschema_test.go`、
`builder_test.go` 等通过同路径 Go 包构造真实 `model` 对象并写入 `meta.Mutator`；
`pkg/infoschema/test/clustertablestest/{tables_test.go,cluster_tables_test.go}` 调用 Go
`PrepareSlowLogfile`。这些路径是移植语义的证据，不是 Rust helper 已被这些 Go 测试调用的证据。

## 错误处理与边界

- `TestStore::new` 把 `NewMockStore` 的错误转成只保留文本的 `TestkitError`，不保留结构化错误类型或 source 链。
- 元数据锁使用 `expect("metadata lock poisoned")`；任一持锁线程 panic 导致锁中毒后，后续操作也会 panic，而不是返回 `TestkitError`。
- `GenGlobalID` 对内部计数的 `+1` 使用 `checked_add` 并在溢出时回滚；之后的返回偏移 `+100` 未使用 checked 运算。正常从零开始的测试不会接近该边界，但这不是完整的整数溢出防护。
- `AddDB`、`AddTable`、`AddResourceGroup`、`CreatePolicy` 对重复 ID 返回错误并依赖事务快照恢复被 `insert` 暂时替换的旧值。
- `AddTable` 还要求数据库已登记；`UpdateTable`、`UpdateDB`、普通资源组更新和策略更新要求目标存在。
- `DropTable` 对缺失目标报错；`DropDB`、`DropResourceGroup` 和 `DropPolicy` 对缺失目标成功。`DropTable` 的 `_table_name` 参数只为签名对齐保留，当前不参与校验。
- `MockTable` 忽略 `_store`，仅克隆 `TableInfo`；不会执行 Go `tables.TableFromMeta` 的验证或 allocator 装配。
- `PrepareSlowLogfile` 没有设置 `truncate(true)`。若目标已经存在且旧内容更长，写入后可能残留尾部数据；这与 Go 版本使用 `O_CREATE|O_WRONLY` 而不含 `O_TRUNC` 的行为一致。函数显式 `flush`，文件关闭由 Rust 的 RAII 在离开作用域时完成。
- 文件无条件导入 `std::os::unix::fs::OpenOptionsExt`，所以当前实现是 Unix 专用；Cargo 中列出的 Windows 条件依赖并不能消除这一源码级编译边界。

## 并发与资源生命周期

`TestStore` 通过 `Arc` 共享所有权，通过 `Mutex` 独占保护元数据。每个 helper 在同步临界区中完成整次操作，不跨线程启动任务，不使用异步运行时、通道或后台资源。多个线程对同一 store 的操作不会并行修改状态；不同 `TestStore::new` 实例之间完全隔离。

闭包不能把 `&mut MetadataState` 带出 `transaction`，锁守卫也在函数返回时释放。失败恢复发生在释放锁之前，因此其他线程看不到中间状态。需要注意：若操作闭包 panic，当前函数没有 `catch_unwind`，不会执行显式快照恢复，并会使互斥锁中毒；现有闭包均由本文件控制，主要 panic 风险来自算术或分配等非 `Result` 路径。

`MockStorage` 的生命周期与 `Arc<TestStoreInner>` 一致；最后一个 `TestStore` 克隆被丢弃时释放。慢日志文件句柄在 `PrepareSlowLogfile` 返回前完成刷新，并在局部变量析构时关闭。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/infoschema/internal/testkit.go`](./testkit.go)：

- 两边的慢日志样例字段和两条记录保持一致；Go helper 通过 `testing.T` 立即断言失败，Rust helper 返回 `std::io::Result`。
- Go `mockAutoIDRequirement` 实现生产 `autoid.Requirement`，持有 `kv.Storage` 和可空的 `*autoid.ClientDiscover`；Rust 定义局部 `AutoIdRequirement` trait，持有 `TestStore`，客户端类型简化为 `Option<()>`。
- Go `GenGlobalID` 在带 `InternalTxnDDL` 来源的真实 KV 新事务中调用
`meta.NewMutator(txn).GenGlobalID()`；Rust 只在互斥锁保护的内存计数器上递增。两边都把结果偏移 `100`。
- Go 模型来自 `pkg/meta/model`、`pkg/parser/ast` 和 `pkg/types`；Rust 在本文件中定义最小副本，只覆盖当前夹具需要的字段。
- Go `MockTable` 通过 `tables.TableFromMeta(autoid.Allocators{}, tblInfo)` 构造生产表对象；Rust 仅克隆元数据到轻量 `Table`。
- Go 的增删改在独立 KV 事务内调用 `meta.Mutator`；Rust 对内存状态做快照事务。Rust 的 `TableInfo::revision` 更新用于证明存在性检查发生在调用者状态变化之前，是本地测试契约，并非 Go helper 的逐字段翻译。
- 独立 Rust 测试 `metadata_helpers_match_meta_mutator_existence_rules` 固化了 Go 元数据层的幂等删除、默认资源组 ID `1` 可直接更新、资源组和策略零 ID 非法等边界；`update_table_advances_revision_only_after_existence_checks` 固化了表更新顺序。

因此，新增对照逻辑时应先确认 Go helper 背后的 `meta.Mutator` 语义，再决定是扩展轻量模型还是接入真实 Rust 元数据 crate；不能仅凭函数同名认定两边具有完整等价性。

## 扩展指南

- 新增元数据字段时，先扩展相应最小类型及构造函数，并在独立的
`pkg/infoschema/internal/testkit_test.rs` 中添加行为测试；不要把 `#[test]` 或测试模块放回本源文件。
- 新增状态类别时，应同步扩展 `MetadataState`、相关增删改 helper 和失败回滚用例。若使用“先修改再报错”的实现方式，必须验证 `transaction` 恢复旧值。
- 修改存在性或幂等规则前，先核对 Go `pkg/infoschema/internal/testkit.go` 调用的具体
`meta.Mutator` 方法及其测试；默认资源组 ID `1`、零 ID 拒绝和库删除级联表是当前显式兼容点。
- 若 helper 需要参与真实 Rust InfoSchema 测试，应先改变 Cargo 接线并用真实调用点证明需求；这会把工作从“夹具文档/轻量模型”扩大到依赖和行为迁移，不能用当前内存映射假装生产持久化。
- 若增加可观察查询 API，保持锁持有时间最短，返回拥有所有权的克隆或受控快照，避免把锁守卫泄漏到调用方。
- 若要支持 Windows，应替换或条件封装 Unix 的 `OpenOptionsExt::mode`；同时验证文件权限语义，而不是只依赖 Cargo 的 Windows 条件依赖。
- 性能风险主要来自每次操作完整克隆 `MetadataState`。大规模夹具若出现明显成本，应设计变更日志或更细粒度回滚，但必须保留原子失败语义并补并发测试。
- 慢日志字段更新应同步检查 Go 常量及 Rust 集群慢日志测试中自行维护的样例，防止三份测试数据漂移。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件，其中 Rust 7,032 个；目标目录中识别到 `lib.rs`、`testkit.rs`、`testkit_test.rs` 及 Go 对照文件。
- RustCodeGraph `node --file pkg/infoschema/internal/testkit.rs`：读取目标文件全部 544 行，核对所有常量、类型、trait、函数、实现和内部调用。
- RustCodeGraph `node --file pkg/infoschema/internal/testkit_test.rs`：读取全部 75 行，核对两个独立测试覆盖的边界。
- RustCodeGraph `node --file pkg/infoschema/internal/lib.rs`：确认公开模块、再导出和独立测试接线。
- RustCodeGraph `query GenGlobalID --kind function` 与 `query AddTable --kind function --json`：确认 Go/Rust 同名定义及目标文件中的精确符号。路径限定的 `callers`/`callees` 查询在 30 秒内未返回，未将其当成调用关系证据。
- 直接读取 `pkg/infoschema/internal/Cargo.toml`、`pkg/infoschema/Cargo.toml`、根
`Cargo.toml` 与 `pkg/lib.rs`：确认 crate、常规依赖、永假上层依赖和门面再导出。
- 直接读取 `pkg/infoschema/internal/testkit.go`：核对 Go 的 KV 事务、Meta Mutator、生产模型和断言式错误处理。
- 路径限定 `rg`：核对 Rust 当前调用面，以及 Go
`infoschema_v2_test.go`、`infoschema_test.go`、`builder_test.go` 和集群慢日志测试中的使用位置。

结构校验使用任务规定的命令，目标是本文件恰好具有上述十一个固定二级标题。任务是纯文档分析，按计划不运行 Cargo；行为结论来自源码、调用点、Cargo 配置和既有测试，而不是本轮代码执行。
