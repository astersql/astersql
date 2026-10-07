# `br/pkg/restore/internal/prealloc_db/lib.rs`

## 文件定位

该文件是 Cargo crate `astersql-br-pkg-restore-internal-prealloc-db` 的库入口，而不是 PreallocDB 的业务实现文件。`br/pkg/restore/internal/prealloc_db/Cargo.toml` 以 `[lib] path = "lib.rs"` 指向它，根 `Cargo.toml` 又将该 crate 列入 workspace members。它通过 `#[path = "db.rs"] pub mod db` 挂载同目录的真实实现，再以 `pub use db::*` 提供扁平的 crate 级 API。

在 Go 包布局中没有对等的 `lib.rs` 门面；对应实现直接位于 `br/pkg/restore/internal/prealloc_db/db.go`。Cargo 元数据的 `go-package = "br/pkg/restore/internal/prealloc_db"` 明确了这一对应关系。

## 核心职责

`lib.rs` 只承担三项结构性职责：

1. 定义 crate 根和兼容性 lint 边界：`#![allow(...)]` 允许保留 Go 式命名及移植期未使用项。
2. 把 `db.rs` 声明为公开子模块，并将其公开符号重新导出到 crate 根；因此消费者可直接使用 `NewDB`、`DB`、`Glue`、`Session` 等，不必经过 `db::`。
3. 只在 `cfg(test)` 下挂载 `parity_test.rs` 和 `db_test.rs`，遵守“Rust 源码与测试逻辑分文件”的仓库约定。

业务行为均在 `db.rs` 中：创建 session、设置 SQL/placement mode、注册预分配 ID、建库建表、恢复 sequence/auto ID、处理 placement policy 以及关闭 session。

## 主要符号

- `pub mod db`：使 `br/pkg/restore/internal/prealloc_db/db.rs` 成为公开子模块。显式 `#[path]` 保持源文件与 Go 包布局的对照。
- `pub use db::*`：全量再导出 `db.rs` 的公开项。当前重要 API 包括 `NewDB(&dyn Glue, Storage, &str) -> Result<(Option<DB>, bool)>`、`DB`、`Glue`、`Session`、`BatchCreateTableSession`、`Context`、`Storage`以及用于移植的 `model`/`metautil` 类型。
- `mod parity_test`：测试配置下引入 `parity_test.rs`，关注 Go/Rust 公开契约、边界、错误和 `Close` 行为。
- `mod db_test`：测试配置下引入更细的 Go 对等单元测试，覆盖建库建表、ID 改写、policy、TTL、sequence、DDL 和批量/单表路径。

`lib.rs` 本身不定义常量、struct、trait、函数或 `impl`；这些符号的所有权仍在 `db.rs`，只是由 crate 根暴露。

## 执行流程

编译生产 library 时，Rust 先解析 `lib.rs` 的 lint 属性，再按 `#[path = "db.rs"]` 编译 `db` 模块，最后将其公开符号合并到 crate 根 API。`cfg(test)` 为假，两个测试模块不进入生产构建。

通过门面进入的典型业务流程由 `db.rs` 定义：`NewDB` 通过 `Glue::CreateSession` 取得 session，清空 `sql_mode`，并尝试设置 `tidb_placement_mode`；调用方之后用 `RegisterPreallocatedIDs` 注入 ID 映射，再调用 `CreateDatabase`、`CreateTables`/`CreateTable` 或 `ExecDDL`，最后以 `DB::Close` 释放 session。

但是当前 Rust 仓库的直接引用搜索只找到本 crate 的 `parity_test.rs` 和 `db_test.rs` 调用 `NewDB`，未找到其他 Rust 生产 crate 对包名或 `prealloc_db::` 的依赖；因此上述是已实现、已测试的 crate 内流程，不应表述为已接入 Rust `SnapClient` 生产主链。

## 数据与状态

`lib.rs` 无运行时字段、全局变量或缓存；它只决定符号可见性和测试编译边界。经它暴露的关键状态存放在 `db.rs` 的 `DB { se, prealloced_ids }` 中：`se` 拥有一个 `Box<dyn Session>`，`prealloced_ids` 在建表前由 `RegisterPreallocatedIDs` 设置。

placement policy 的共享集合由调用方以 `Mutex<HashMap<String, PolicyInfo>>` 传入；`ensurePlacementPolicy` 在锁内移除条目，锁外执行创建，使同一 policy 最多被该映射消费一次。表 ID 与分区 ID 由唯一 Cargo 依赖 `astersql-br-pkg-restore-internal-prealloc-table-id` 的 `PreallocIDs::RewriteTableInfo` 改写。

## 依赖与调用关系

- 下游模块：`lib.rs -> db.rs`，是唯一无条件模块边；`db.rs -> prealloc_table_id` 是 `Cargo.toml` 声明的唯一 crate 依赖。
- 测试边：`lib.rs -> parity_test.rs` 和 `lib.rs -> db_test.rs` 只在 `cfg(test)` 成立。测试以 `use super::db::{...}` 直接使用实现符号，同时验证 crate 内契约。
- Go 主链：`br/pkg/restore/snap_client/client.go::InitConnections` 调用 Go `tidallocdb.NewDB`创建主 DB 和 DB pool；随后注册预分配 ID，在库、表和 DDL 恢复路径上调用 `CreateDatabase`、`CreateTables`/`CreateTable`、`ExecDDL`，并在 `SnapClient.Close` 中关闭。
- Rust 现状：RustCodeGraph 对 `lib.rs` 的文件级结果主要反映模块挂载，对 `db.rs::NewDB` 则识别到 `Glue::CreateSession`、`Session::Execute`、`Context::Background`、`ErrUnknownSystemVar` 和错误透传等下游边。仓库文本搜索未发现本 crate 的 Rust 生产调用方，所以它目前更像已迁移并经独立测试的可复用边界。

## 错误处理与边界

`lib.rs` 不产生或处理运行时错误。其结构性边界是：`db.rs` 缺失会在模块解析阶段失败，再导出的破坏会在下游编译期显现，测试文件不会进入非测试构建。

经门面暴露的 `db.rs` 契约包含以下关键边界：raw-kv 模式返回空 session 时 `NewDB` 返回 `(None, false)`；目标不识别 `tidb_placement_mode` 时仅禁用 policy 支持，其他 session 错误立即返回；`CreateTables` 在未注册 ID 时返回 `preallocedIDs is nil`，而单表 `CreateTable` 将“必须先注册”表达为 `expect` 不变量；已存在的数据库是可恢复结果，其他建库建表错误向上传播。`parity_test.rs` 和 `db_test.rs` 对这些分支都有直接证据。

## 并发与资源生命周期

`lib.rs` 不启动线程、task 或 channel，也不持有资源。`db.rs` 明确把 `DB` 标记为“not thread-safe”：它通过 `&mut self` 串行使用内部 session。需要并行 DDL 时，Go `SnapClient` 在外层构建多个 `DB` 的 pool，而不是共享单个 `DB`。

Rust `Context` 以 `Arc<Mutex<Option<Error>>>` 表达可克隆的取消状态；policy map 使用 `Mutex` 保护“查找并移除”，且不在持锁时调用 session。session 的显式生命周期是 `NewDB` 创建、`DB` 独占、`DB::Close` 调用 `Session::Close`；`parity_test.rs` 的资源清理用例验证了 `Close` 传递。

## 与 Go 版本的对应关系

Rust `lib.rs` 是 Go 包在 Cargo 中额外需要的 crate 门面；它的 `db` 模块对应 `db.go`。`db.rs::NewDB`、`DB::RegisterPreallocatedIDs`、`ExecDDL`、`CreatePlacementPolicy`、`CreateDatabase`、`CreateTablePostRestore`、`CreateTables`、`CreateTable`、`Close` 均能在 `db.go` 找到同名或同职责实现。

Rust 为避免在当前 Darwin 迁移阶段引入 `kv/domain/kvproto/grpcio`，在 `db.rs` 中定义了本地 `Glue`/`Session`/model 边界；这是 `Cargo.toml` 注释记录的刻意差异。Rust 的 `Option<DB>` 对应 Go 的 `*DB == nil`，`Mutex<HashMap<...>>` 对应 Go `sync.Map`，trait 方法 `as_batch_create_table_session` 对应 Go 的 batch-session interface assertion。

`parity_test.rs::go_rust_public_contract_matches` 集中锁定公开契约；`db_test.rs` 对照 `db_test.go` 的更完整用例。相反，Go `snap_client/client.go` 的生产调用链目前只能作为预期集成位置证据，不能证明 Rust crate 已被 Rust SnapClient 接线。

## 扩展指南

- 新增业务行为应修改 `db.rs` 的相应类型或 `impl DB`，不要把实现写入 `lib.rs`。只有需要新的子模块或改变 crate 级导出面时才应改这个门面。
- 增删 `pub use db::*` 可见的符号会改变公开 API；优先在 `db.rs` 保持与 Go 同名/同语义，并同步更新独立的 `parity_test.rs` 和 `db_test.rs`，不应把测试内联到生产文件。
- 添加新模块时需决定它是公开 API 还是 crate 内部实现，并评估 glob 再导出的名称冲突风险。
- 把该 crate 接入 Rust 恢复主链时，应在真实上游 Cargo manifest 增加依赖，实现真实 `Glue`/`Session` 适配，复现 Go `SnapClient` 的 DB pool、ID 注册、DDL 与 `Close` 顺序；这是未来集成工作，不是本文件已有事实。
- 兼容风险集中在 Go/Rust 错误分类、空 session、policy 降级、预分配 ID 不变量和 sequence/TTL 恢复顺序；性能风险则主要来自 batch-session 选择、DB pool 并行度和 policy map 锁竞争。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件；`node --file br/pkg/restore/internal/prealloc_db/lib.rs` 确认该文件共 31 行、只包含 lint、`db` 模块、glob 再导出和两个测试模块。
- RustCodeGraph `node --file .../db.rs` 与 `node NewDB`：确认 `DB`、`NewDB`、trait 边界、ID 改写、建库建表、policy、sequence 和关闭流程，并识别 `NewDB -> Glue::CreateSession/Session::Execute/ErrUnknownSystemVar` 等下游边。
- Cargo 证据：根 `Cargo.toml` 的 workspace member 条目，以及 `br/pkg/restore/internal/prealloc_db/Cargo.toml` 的 `[lib]`、porting metadata 和唯一依赖。
- Go 对照：`br/pkg/restore/internal/prealloc_db/db.go`、`db_test.go`以及 `br/pkg/restore/snap_client/client.go` 中的 `InitConnections`、ID 注册、库/表/DDL 创建与关闭调用。
- Rust 测试：`br/pkg/restore/internal/prealloc_db/parity_test.rs` 和 `db_test.rs`。两者由 `lib.rs` 以 `cfg(test)` 独立挂载，没有将测试逻辑放入生产源文件。
- 调用面搜索：`rg` 对 crate 名、`prealloc_db::`、`NewDB` 和主要 DB 方法的查询，用于区分“Rust 测试已覆盖”与“Go 生产主链已接线”。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另执行任务指定的 11 章结构检查、链接/路径存在性检查和 diff 自审。
