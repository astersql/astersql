# `br/pkg/restore/ingestrec/lib.rs`

源文件：[`lib.rs`](lib.rs)

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-restore-ingestrec` 的 crate 根；同目录 [`Cargo.toml`](Cargo.toml) 通过 `[lib] path = "lib.rs"` 明确指向它，`package.metadata.porting.go-package` 将其对应到 Go 包 `br/pkg/restore/ingestrec`。它本身是编译时装配门面：挂载 `model_stub.rs`、`foreign_key.rs`、`ingest_recorder.rs`，并将三者的公开项扁平再导出；它不自己记录 DDL job、扫描外键或生成修复信息。

在完整 BR 设计中，该包处于日志备份元数据重写与 ingest 索引修复之间：Go 的 `br/pkg/stream/rewrite_meta_rawkv.go` 从 DDL job 记录 ingest 索引，`br/pkg/task/stream.go` 重写表 ID，`br/pkg/restore/log_client/client.go` 补全最新 schema 并生成外键/索引修复 SQL。但仓库搜索未发现其他 Rust Cargo manifest 依赖本 crate，所以当前可证明的 Rust 状态是“独立迁移 crate 与测试已存在”，不是“已接入 Rust BR 生产主链”。

## 核心职责

- 用 `#[path = "..."] pub mod ...` 建立三个生产模块：`model_stub` 提供该迁移 crate 所需的 DDL/model/InfoSchema 边界，`foreign_key` 收集和裁剪修复索引涉及的外键，`ingest_recorder` 记录 ingest DDL job 并补全索引信息。
- 用三条 `pub use ...::*` 将子模块公开符号同时暴露在 crate 根，模拟 Go 同包符号的扁平可见性。
- 仅在 `cfg(test)` 下挂载 `export_test.rs`、`parity_test.rs`、`model_stub_test.rs`、`foreign_key_test.rs`、`ingest_recorder_test.rs`，保证测试与生产源文件分离。
- 在 crate 级允许死代码、Go 风格命名和迁移期未使用项。这些 `allow` 属性不改变运行时逻辑，也不能作为生产接线完成的证据。

## 主要符号

`lib.rs` 没有定义常量、struct、enum、trait、函数或 `impl`；RustCodeGraph 的文件视图也只识别到文件级门面。其公共 API 由下列模块和 glob re-export 决定：

- `model_stub::*`：导出 `CIStr`、`Job`、`ActionType`、`JobState`、`ReorgType`、`IndexInfo`、`TableInfo`、`FKInfo`、`InfoSchema` trait、`Context` 及 finished-args 解析/索引前缀判定辅助函数。这是本地轻量模型边界，不是 TiDB 完整 meta/infoschema 子系统。
- `foreign_key::*`：导出 `ForeignKeyRecordKey`、`ForeignKeyRecord`、`ForeignKeyRecordManager`、`TableForeignKeyRecordManager`、`NewForeignKeyRecordManager`、`NewForeignKeyRecordManagerForTables`。记录 key 以子库、子表和外键原始名称去重。
- `ingest_recorder::*`：导出 `IngestIndexInfo`、`IngestRecorder` 和包级 `New`。核心方法是 `TryAddJob`、`RewriteTableID`、`UpdateIndexInfo`、`Iterate`、`IterateForeignKeys`。
- 五个 `mod *_test`：都是私有测试模块，不扩大生产 API。`export_test` 只为同 crate 测试提供 FK map 访问器。

根级 glob re-export 意味着，任一子模块新增 `pub` 项都可能自动成为 crate 根 API，也可能与其他模块同名符号冲突。

## 执行流程

`lib.rs` 的直接工作发生在编译阶段：Cargo 以它为 crate 根，应用 lint 例外，按显式路径纳入三个生产模块，在测试构建中再纳入五个独立测试模块，最后建立扁平导出面。

由门面导出的典型运行时数据流是：

1. `New` / `IngestRecorder::New` 创建空录制器，其索引 map 为空，外键管理器尚未初始化。
2. 元数据重放阶段将 DDL `Job` 交给 `TryAddJob`。只接受 ingest reorg、`AddIndex`/`AddPrimaryKey`/`ModifyColumn`、主 job `Synced` 或子 job `Done` 的组合；此时只记录表/索引 ID 与主键标志，`Updated=false`。
3. 恢复规则就绪后，`RewriteTableID` 将旧表 ID 映射为新 ID，回调返回 `skip=true` 时丢弃该表记录；出错时因采用“先构建新 map，再整体替换”而保留旧 map。
4. `UpdateIndexInfo` 从最新 `InfoSchema` 查表和库，填充 schema/table 名、索引对象、列占位串和列参数，同时收集需在修复索引前处理的自身/被引用外键，成功项置 `Updated=true`。
5. 消费端先通过 `IterateForeignKeys` 获取外键记录，再通过 `Iterate` 只获取已补全的索引，从而生成删除/重建顺序。Go `log_client/client.go::generateRepairIngestIndexSQLs` 验证了这一消费顺序。

## 数据与状态

`lib.rs` 不保存任何运行时状态。它暴露的主状态容器是 `IngestRecorder`：`items` 是两层 `HashMap<table_id, HashMap<index_id, IngestIndexInfo>>`，`foreignKeyRecordManager` 在首次成功的 `UpdateIndexInfo` 之前为 `None`。`IngestIndexInfo` 持有库表名、索引列 SQL 模板、列参数、主键标志、可选 `IndexInfo` 快照和 `Updated` 门控。

`ForeignKeyRecordManager` 用 `HashMap<ForeignKeyRecordKey, ForeignKeyRecord>` 合并多表的外键和 referred FK；`TableForeignKeyRecordManager` 在合并前分开保存两类记录。`RemoveForeignKeys` 只在索引列前缀能安全覆盖 FK 列时删除相关记录；部分索引条件和 PK-handle 特例由 `model_stub` 的覆盖判定函数表达。

上述 map 的迭代顺序没有稳定保证，调用方不应依赖固定的表、索引或外键顺序。

## 依赖与调用关系

Cargo 边界很小：`br/pkg/restore/ingestrec/Cargo.toml` 的唯一直接依赖是路径 crate `astersql-errors`；没有 feature 开关。标准库提供 `HashMap` 和 `Instant`，业务模型由 `model_stub.rs` 在 crate 内部提供。

内部依赖方向是 `ingest_recorder -> foreign_key + model_stub`、`foreign_key -> model_stub`；`lib.rs` 负责把三者组成一个 crate。`UpdateIndexInfo` 调用 `NewForeignKeyRecordManagerForTables`，后者通过 `InfoSchema::{GetTableReferredForeignKeys, TableByName}` 查找被引用关系；`TryAddJob` 通过 `GetFinishedModifyIndexArgs` / `GetFinishedModifyColumnArgs` 解码完成参数。

RustCodeGraph `node --file br/pkg/restore/ingestrec/lib.rs` 确认了三个生产模块、五个测试模块和三条再导出，并将 `IngestRecorder` 定位到 Rust `ingest_recorder.rs` 与 Go `ingest_recorder.go`。精确 `callers`/`callees` 查询在 30 秒内未返回边，因此本文不将图工具的空输出解读为“无调用者”。Cargo 反向搜索只命中本 crate 自身 manifest；Rust 直接消费证据目前来自本 crate 的独立测试。Go 上游/下游调用则由 `rewrite_meta_rawkv.go`、`stream.go` 和 `log_client/client.go` 的精确方法引用验证。

## 错误处理与边界

`lib.rs` 不生成、包装或捕获错误；子模块的可失败 API 主要返回 `astersql_errors::SharedError`。关键边界如下：

- `TryAddJob(None, ...)` 以及非 ingest、非目标 action、未达目标状态的 job 均静默成功并不记录；通过过滤后缺少 finished args 则返回错误。
- `RewriteTableID` 给回调错误增加旧 table ID 语境，并在中途失败时不替换原 `items`。但多个旧 ID 被映射到同一新 ID 时，后插入者会覆盖先插入者，当前 API 不报冲突。
- `UpdateIndexInfo` 找不到表时跳过；表存在但找不到所属库时是硬错误。它依赖 `IndexColumn.Offset` 是有效列下标，实现使用直接索引；破坏该先决条件会 panic。
- `Iterate` 跳过 `Updated=false` 的粗粒度记录；`IterateForeignKeys` 在外键管理器未初始化时空成功。两者都将回调错误通过 `trace` 向上传播。
- `NewForeignKeyRecordManagerForTables` 查被引用外键时，子表查找失败会返回错误；找到子表但没有同名 FK 时不会伪造记录。

## 并发与资源生命周期

该门面和三个生产子模块都不创建线程、异步任务、channel、文件、网络连接或显式锁。`IngestRecorder` 的变更方法要求 `&mut self`，因此 Rust 借用规则要求调用方串行化单实例变更；只读 `Iterate`/`IterateForeignKeys` 使用 `&self`。类型没有自定义 `Drop`，所有 map、`String`、`Vec` 和 clone 的模型快照按 Rust 所有权自动释放。

`UpdateIndexInfo` 会在栈上建立新的全局 FK manager，扫描完后一次写入 recorder；方法中的 `Instant::now()` 目前只保留计时形状，没有上报或改变控制流。`RewriteTableID` 为了错误原子性会 clone 每张表的索引 map；扩展大规模恢复时需关注这一瞬时内存倍增，不应为减少 clone 而破坏错误时保留原状态的语义。

## 与 Go 版本的对应关系

Rust 没有与 `lib.rs` 一对一的 Go 文件，因为 Go package 自动聚合同目录源文件；Rust crate 根的模块声明与再导出正是对这种包级命名空间的显式模拟。`ingest_recorder.rs` 对应 `ingest_recorder.go`，`foreign_key.rs` 对应 `foreign_key.go`；`model_stub.rs` 是 Rust 迁移层额外的本地模型投影。

已验证的核心对齐点包括：主 job 要求 `Synced`而子 job 允许 `Done`；只记录 ingest 的 add-index/add-primary-key/modify-column；先记 ID，再用最新 InfoSchema 补全；隐藏生成列内联表达式，普通列使用 `%n` 参数且保留前缀长度；表缺失跳过、库缺失报错；修复索引前先处理外键；外键按索引前缀覆盖判定是否需移除。

表达差异包括：Go 直接使用 TiDB `model`、`infoschema`、`context.Context` 和 errors package，Rust 使用本地 `model_stub`、零字段 `Context` 与 `SharedError`；Go `New()` 返回指针，Rust 返回拥有值；Go 的 map 和 Rust `HashMap` 都不应被视为稳定迭代顺序。Rust 现有独立测试和 `parity_test.rs` 验证可观察契约，但这不代表本地 stub 已等价于完整 TiDB 生产类型栈。

## 扩展指南

- 新增生产模块时，在 `lib.rs` 中用明确的 `#[path] pub mod` 挂载；只有需维持 Go 包级 API 时才在 crate 根再导出，并先检查三个 glob 命名空间的冲突。
- 调整 job 过滤、表 ID 重写、列列表生成或迭代契约时，修改 `ingest_recorder.rs` 的对应方法，同步 `ingest_recorder_test.rs`、`parity_test.rs` 及 Go `ingest_recorder_test.go`；不要把测试内嵌到生产文件。
- 调整 FK 去重、被引用查找、PK-handle 或部分索引覆盖时，聚焦 `foreign_key.rs` 和 `model_stub.rs`，同步 `foreign_key_test.rs`、`model_stub_test.rs` 及 Go `foreign_key_test.go`。
- 若将本 crate 接入 Rust BR 生产主链，必须在真实上游 Cargo manifest 添加依赖，并对照 Go 的“重放记录 -> ID 重写 -> schema 补全 -> FK/索引修复”全链添加独立集成测试；单纯扩大再导出不构成接线。
- 引入真实 TiDB model/infoschema 或外部 Rust 依赖时，先定义 stub 到真实类型的转换与错误/取消边界；外部依赖还必须遵循仓库的上游移植、提交、打 tag 和统一 Git tag 引用要求。
- 性能修改要重验 `RewriteTableID` clone 的错误原子性、`UpdateIndexInfo` 的多表/FK 扫描成本和无序 map 的可重现性；兼容性修改要保留 Go 过滤条件、错误优先级和 SQL 列模板形状。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter br/pkg/restore/ingestrec` 列出 14 个 Rust/Go 生产与测试文件；`node --file br/pkg/restore/ingestrec/lib.rs` 核对了全部 52 行、模块边界、测试条件和再导出；`query IngestRecorder --kind struct` 定位 Rust/Go 对应定义。`callers`/`callees` 在 30 秒内未返回，因而调用边由精确源码/Cargo 搜索补证。
- Rust 生产源码：`br/pkg/restore/ingestrec/lib.rs`、`ingest_recorder.rs`、`foreign_key.rs`、`model_stub.rs`；Cargo 边界：同目录 `Cargo.toml`。仓库内对 crate 包名和 Rust 导入名的搜索未找到外部 Cargo 消费者。
- Go 对照与主链：`br/pkg/restore/ingestrec/ingest_recorder.go`、`foreign_key.go`、`export_test.go`；生产使用点 `br/pkg/stream/rewrite_meta_rawkv.go`、`br/pkg/task/stream.go`、`br/pkg/restore/log_client/client.go`。
- Rust 独立测试：`export_test.rs`、`parity_test.rs`、`model_stub_test.rs`、`foreign_key_test.rs`、`ingest_recorder_test.rs`；Go 对照测试：`foreign_key_test.go`、`ingest_recorder_test.go` 及上层 `br/pkg/restore/log_client/client_test.go`。它们覆盖 job 过滤、缺参错误、ID 重写与失败保留、列模板、表/库缺失、FK 覆盖/PK 特例和公开 API 串联。
- 本任务只新增说明文档，按计划不运行 Cargo。交付验证为规定的 11 章结构检查、`git diff --check` 和仅目标文档的差异自审；未验证仓库外调用者，也不声称运行时测试结果。
