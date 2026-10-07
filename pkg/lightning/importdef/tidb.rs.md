# `pkg/lightning/importdef/tidb.rs`

## 文件定位

本文件是 `astersql-lightning-importdef` crate 的 TiDB 导入元数据模型实现，源码入口由 `pkg/lightning/importdef/lib.rs` 的 `#[path = "tidb.rs"] mod tidb` 装配，并通过 `pub use tidb::*` 对 crate 使用者公开。根门面 `pkg/lib.rs` 又在 `lightning::importdef` 下再导出该 crate。`pkg/lightning/importdef/Cargo.toml` 声明其 Go 对照包为 `pkg/lightning/importdef`，唯一直接 Rust 依赖是 `astersql-meta-model`。

它位于 Lightning 导入流程的“库表描述”边界：只定义传递数据库、表以及当前/目标表结构所需的数据类型，不负责读取 TiDB、执行导入、持久化 checkpoint 或计算 checksum。Go 生产链会在 `lightning/pkg/importer/tidb.go::LoadSchemaInfo` 创建这些对象，再交给 importer、checkpoint 和 checksum 代码；当前 Rust 仓库中没有找到生产源码对本 crate 类型的直接引用，只有 crate/根门面的再导出、下游 Cargo 依赖声明及本 crate 的迁移测试。因此，Go 主链说明的是该模型的设计位置，而不是 Rust 主链已经完整接线的证据。

## 核心职责

1. 用 `DBInfo` 表示一次导入涉及的目标数据库，并维护“表名到表描述”的索引。
2. 用 `TableInfo` 同时携带当前 TiDB 表结构 `Core` 和导入完成后期望的表结构 `Desired`，支持“先去索引导数据、后恢复完整索引”的两阶段导入语义。
3. 用 `Option` 表达 Go 指针或 map 的 `nil`，用 `Arc<RwLock<_>>` 模拟 Go map/指针复制后的共享、可变语义。
4. 保持模型层轻量：没有构造函数、校验函数、序列化实现、错误类型或业务副作用；调用方必须自行建立并维护字段间的一致性。

## 主要符号

- `TableInfoRef = Arc<RwLock<TableInfo>>`：对应 Go 的 `*TableInfo`。多个 map 项持有者或结构副本可以共享并修改同一表描述。
- `TableInfoMap = HashMap<String, TableInfoRef>`：对应已初始化的 `map[string]*TableInfo`；键的约定是表名，但类型本身不验证键是否等于 `TableInfo.Name`。
- `TableInfoMapRef = Arc<RwLock<TableInfoMap>>`：为整张表 map 增加共享所有权和读写锁。`DBInfo::clone` 只克隆 `Arc`，不会深拷贝 map。
- `ModelTableInfoRef = Arc<RwLock<model::TableInfo>>`：对应 Go 的 `*model.TableInfo`。其中 `model::TableInfo` 经 `lib.rs::model` 从 `astersql-meta-model::group_1::TableInfo` 再导出。
- `DBInfo { ID, Name, Tables }`：公开、可克隆且可默认构造。`Tables: None` 对应 Go nil map；`Some(...)` 才表示已经初始化、可以加表的 map。
- `TableInfo { ID, DB, Name, Core, Desired }`：公开、可克隆且可默认构造。`Core` 是当前生效元数据，`Desired` 是最终目标元数据；两者均允许缺失，也允许指向同一个对象。

本文件没有模块级常量、trait、函数、固有 `impl` 或条件编译项。`Clone` 与 `Default` 均由 derive 生成；公开 API 是四个类型别名和两个结构体及其公开字段。

## 执行流程

本文件没有可执行入口，运行时流程由使用者围绕数据结构组织。根据源码、Go 对照和独立迁移测试，可归纳为：

1. 元数据发现阶段创建 `DBInfo`，写入 schema ID 和名称，并显式把 `Tables` 从 `None` 初始化为 `Some(Arc<RwLock<HashMap<...>>>)`。
2. 每张表创建 `TableInfo`，写入表 ID、库名、表名，并用 `Core` 保存 TiDB 当前表结构。
3. 普通导入可令 `Desired` 与 `Core` 共享同一个 `Arc`；需要分离数据和索引导入时，`Core` 可表示暂时去除索引的结构，而 `Desired` 保留完整结构。
4. 调用方取得 map 的读锁按表名查询，或取得写锁插入/更新表；取得 `Core`/`Desired` 的锁后读取或修改底层 `model::TableInfo`。
5. Go 侧 `lightning/pkg/importer/tidb.go::LoadSchemaInfo` 是上述创建流程的直接实现证据；`lightning/pkg/checkpoints/checkpoints.go::Initialize`、`lightning/pkg/importer/table_import.go` 与 `pkg/ingestor/ingestctrl/checksum.go::Checksum` 展示模型在 checkpoint、单表导入和校验和阶段的消费位置。Rust 侧尚未发现等价生产调用点。

## 数据与状态

`DBInfo::default()` 产生 `ID = 0`、空 `Name` 和 `Tables = None`；`TableInfo::default()` 产生 `ID = 0`、空 `DB`/`Name`、`Core = None`、`Desired = None`。这些值对齐 Go 结构体零值，但“默认可构造”不等于“业务上已就绪”。尤其是 nil map 对应的 `Tables = None` 不能直接写入。

共享状态分两层：`TableInfoMapRef` 让克隆后的多个 `DBInfo` 共享整张 map，`TableInfoRef` 和 `ModelTableInfoRef` 让 map 中的表描述以及 `Core`/`Desired` 共享各自对象。derive 的 `Clone` 只增加 `Arc` 强引用计数，因此任何一方经写锁做的修改会被其他副本观察到。独立测试 `migration_aster_unit_test.rs::database_keeps_tables_addressable_by_name` 和 `copied_and_dual_metadata_pointers_preserve_aliases` 明确验证了这两个不变量。

类型没有强制以下一致性：`DBInfo.Name` 与 `TableInfo.DB`、map 键与 `TableInfo.Name`、`DBInfo.ID`/`TableInfo.ID` 与底层模型 ID、`Core` 与 `Desired` 的结构关系。它们都是调用方协议。

## 依赖与调用关系

下游数据依赖只有 `crate::model::TableInfo`；`lib.rs` 将其绑定到 `model_dependency`（Cargo 包 `astersql-meta-model`）的 `group_1::TableInfo`。标准库依赖为 `HashMap`、`Arc` 和 `RwLock`。

装配链为 `tidb.rs` → `pkg/lightning/importdef/lib.rs` 的 `pub use tidb::*` → `pkg/lib.rs::lightning::importdef` 的门面再导出。根 `Cargo.toml` 用 `facade_lightning_importdef` 指向该 crate。`pkg/dxf/importinto`、`pkg/executor/importer`、`pkg/ddl/ingest` 和 `pkg/ingestor/ingestctrl` 的 Cargo manifest 声明了此依赖，但 `rg` 未在这些目录的 Rust 生产源码中找到 `astersql_lightning_importdef` 或门面类型的直接使用；不能据依赖声明推断已完成运行时接线。

RustCodeGraph 将 `DBInfo` 和 `TableInfo` 索引为 `pkg/lightning/importdef/tidb.rs` 中的结构体，并报告二者没有 callee；这是符合纯数据类型性质的结果。由于没有函数体，传统 callers/callees 图不能完整表达字段类型引用，实际引用以 crate 再导出、Cargo 声明和 `rg` 结果补充核验。

## 错误处理与边界

本文件没有 `Result`、显式错误或校验路径。主要失败边界来自调用方操作：

- `Tables = None` 表示未初始化 map；强行解包并写入会 panic。迁移测试用 `catch_unwind` 验证这一点，生产代码应先显式初始化，而不是依赖 panic。
- `RwLock::read`/`write` 在持锁线程 panic 后可能返回 poisoned error；本文件不提供恢复策略，调用方必须决定传播、重建还是终止。
- 类型允许 `Core = None` 或 `Desired = None`。只有明确允许缺失元数据的阶段才能接受该状态；需要表结构的调用方必须先检查。
- 所有字段公开，错误的库表名、ID 或 `Core`/`Desired` 组合可以被构造出来；模型层不会拒绝它们。
- `HashMap` 不保证遍历顺序。需要稳定 checkpoint、日志或测试输出时，调用方必须排序，不能依赖当前迭代次序。

## 并发与资源生命周期

`Arc` 以引用计数管理 map、表描述和底层模型的生命周期：最后一个强引用释放时对象销毁，不需要显式关闭。`RwLock` 允许多个并发读者或一个写者，锁的粒度分别是整张 `Tables` map、单个 `TableInfo` 或单个 `model::TableInfo`。

代码没有后台任务、channel、事务、文件句柄或网络连接。并发正确性仍由调用方负责：避免在持有 map 锁时再以不一致顺序取得表/模型锁，否则跨线程代码可能死锁；不要在长耗时或 I/O 操作期间持写锁；需要一致读取 `Core` 和 `Desired` 时，应定义快照或锁顺序，因为两个字段可以指向不同锁，也可能别名到同一个锁。当前独立测试验证共享可见性和别名关系，但没有验证多线程竞争、锁中毒或死锁规约。

## 与 Go 版本的对应关系

`pkg/lightning/importdef/tidb.go` 定义同名 `DBInfo` 与 `TableInfo`，字段集合和语义逐项对应：Rust `i64`/`String` 对应 Go `int64`/`string`，Rust `Option<TableInfoMapRef>` 对应可为 nil 的 Go map，Rust `Option<ModelTableInfoRef>` 对应可为 nil 的 Go 指针。

Rust 为保留 Go 引用语义引入了显式包装。Go map 和指针复制后天然共享底层对象；Rust 通过 `Arc` 保持共享所有权，通过 `RwLock` 提供内部可变性。由此产生两点语言差异：Rust 访问共享值必须处理加锁结果，Go 原类型则没有锁；Rust `Arc<RwLock<_>>` 自带线程安全能力，但并不意味着 Go 版本原本承诺并发安全。

`Core`/`Desired` 的注释与 Go 完全相同的业务意图：通常二者相同；分离索引和数据导入时，`Core` 可暂时不含索引，`Desired` 保留索引。Rust 测试还验证它们可以显式别名到同一个对象。当前 Rust 生产接线覆盖度低于 Go：Go 的 importer、checkpoint、backend、DDL ingest 和 checksum 路径有直接类型引用；Rust 侧能确认的是模型、再导出、依赖声明与迁移单测，不能宣称上述 Go 消费链均已移植到此类型。

## 扩展指南

- 新增或调整字段时，应同时核对 `tidb.go`，保持字段含义、nil/零值和共享语义一致；若底层表结构字段变化，还要核对 `lib.rs::model` 的再导出来源。
- 修改 `DBInfo`/`TableInfo` 的 clone、默认值或共享策略时，应扩展独立文件 `pkg/lightning/importdef/migration_aster_unit_test.rs`，不要把测试写回生产文件。至少覆盖零值、map 克隆共享、表指针共享以及 `Core`/`Desired` 相同和不同对象两种情况。
- 增加便利方法时，优先把“初始化 map”“按名查询”“获得当前/目标元数据”等协议封装成明确返回 `Result`/`Option` 的 API，避免各调用方直接 `unwrap`；同时明确 poisoned lock 的处理策略。
- 若把 Rust 类型接入生产流程，需从当前实际消费者开始验证，而不是只添加 Cargo 依赖：重点核对 schema 加载、checkpoint 初始化、单表 importer 与 checksum 边界，并补各自目录的独立测试。
- 调整锁或容器类型会影响克隆可见性、并发性能和潜在死锁顺序，属于兼容性较高风险的改动；调整字段名或公开类型也会破坏门面 API。序列化若未来加入，还需明确 map 顺序和 `Arc<RwLock<_>>` 的快照规则。

## 验证依据

- 目标源码：`pkg/lightning/importdef/tidb.rs`，核对四个类型别名、`DBInfo`、`TableInfo`、字段、derive 及不存在函数/trait/条件编译项。
- crate 边界：`pkg/lightning/importdef/Cargo.toml`、`pkg/lightning/importdef/lib.rs`；工作区与根门面：根 `Cargo.toml`、`pkg/lib.rs::lightning::importdef`。
- Go 对照：`pkg/lightning/importdef/tidb.go`；Go 调用证据包括 `lightning/pkg/importer/tidb.go::LoadSchemaInfo`、`lightning/pkg/checkpoints/checkpoints.go::Initialize`、`lightning/pkg/importer/table_import.go`、`pkg/ingestor/ingestctrl/checksum.go::Checksum`。
- Rust 独立测试：`pkg/lightning/importdef/migration_aster_unit_test.rs` 的 `zero_values_match_go_struct_behavior`、`database_keeps_tables_addressable_by_name`、`copied_and_dual_metadata_pointers_preserve_aliases`。
- RustCodeGraph：索引状态为 11,467 个文件；`files --filter pkg/lightning/importdef` 找到 `lib.rs`、`migration_aster_unit_test.rs`、`tidb.go`、`tidb.rs`；`query` 定位 `tidb.rs::DBInfo`（第 37 行）和 `tidb.rs::TableInfo`（第 48 行）；`callees` 对两个结构体均显示 `No callees found`。CLI 对高度重名符号的限定查询会展开同名候选，因此引用覆盖又以 `rg` 精确核验。
- 文本引用核验：Rust 侧仅发现 `migration_aster_unit_test.rs` 直接使用本文件类型，`pkg/lib.rs` 通过 facade 再导出；四个下游 manifest 声明依赖但其 Rust 源码没有命中 crate 名。该结果限定了本文关于“当前 Rust 尚未生产接线”的结论。
- 按任务要求未运行 Cargo；本次只新增说明文档，结构检查负责确认目标文件存在且恰有十一个固定二级章节。
