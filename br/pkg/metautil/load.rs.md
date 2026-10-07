# `br/pkg/metautil/load.rs`

## 文件定位

`load.rs` 属于 `astersql-br-pkg-metautil` library crate。crate 根 `br/pkg/metautil/lib.rs` 通过 `pub mod load` 挂载它，并以 `pub use load::*` 再导出公开符号；`br/pkg/metautil/Cargo.toml` 的 `[package.metadata.porting]` 则把该 crate 对应到 Go 包 `br/pkg/metautil`。本文件位于备份元数据读取和恢复/检查消费之间：输入是已经构造好的 `MetaReader`，输出是按数据库原始名称聚合的 `HashMap<String, Database>`。

当前 Rust 接线需要与 Go 主链分开描述。RustCodeGraph 的文件关系只列出 `br/pkg/metautil/load_test.rs` 直接使用本文件，仓库内 Rust 搜索也未发现 `LoadBackupTables` 的生产调用；因此它是已经实现并由 crate 公开的移植 API，但尚不能据此断言 Rust 恢复命令已经走到该实现。Go 版本的直接生产入口包括 `br/pkg/restore/snap_client/client.go` 的 `SnapClient.LoadSchemaIfNeededAndInitClient`，以及 `br/cmd/br/debug.go` 的 checksum 和 backupmeta 校验命令。

## 核心职责

- `Database` 把一个 `model::DBInfo`、该库下的 `Vec<Table>` 和一个私有 PITR 复用标记组织成恢复侧的库级视图。
- `Database::GetTable` 按 `Table.Info.Name.O` 做区分大小写的线性查找，返回借用而不复制表。
- `Database::SetReusedByPITR` / `IsReusedByPITR` 管理库级布尔状态；新聚合出的库默认未复用。
- `LoadBackupTables` 根据 `loadStats` 选择是否传入 `SkipStats`，在后台调用 `MetaReader::ReadSchemasFiles`，前台接收 `Table` 并按 `table.DB.Name.O` 归桶。
- 本文件不解析 schema JSON、不遍历 v1/v2 元数据叶子、不计算 SST 到 physical table ID 的归属；这些职责在 `br/pkg/metautil/metafile.rs` 的 `MetaReader::ReadSchemasFiles`、`parseSchemaFile`、`readSchemas` 和 `readDataFiles` 中。

## 主要符号

- `fn trace_err(err: SharedError) -> SharedError`：私有错误包装器，调用 `astersql_errors::Trace`，模拟 Go `errors.Trace` 的传播形态。`expect("trace")` 表示这里假定对现有错误做 Trace 必然仍得到错误；若该假设被依赖实现破坏，会 panic。
- `pub struct Database`：公开字段 `Info: model::DBInfo` 和 `Tables: Vec<Table>` 允许上层直接读取 schema 与表集合；`reusedByPITR: bool` 私有，只能经方法修改或查询。与 Go 的指针字段不同，Rust 的 `Info` 是拥有所有权的值，表也存为值。
- `Database::SetReusedByPITR(&mut self)`：需要独占可变借用，将标记永久置为 `true`；没有对应的清零方法。
- `Database::IsReusedByPITR(&self) -> bool`：只读返回当前标记。
- `Database::GetTable(&self, name: &str) -> Option<&Table>`：逐项检查 `Table.Info`，名称相等时返回首个匹配项；没有匹配时返回 `None`。若遍历到 `Info == None` 的库级占位 `Table`，会以 `"table info must not be nil"` panic，这是对 Go nil 解引用行为的显式保持。
- `LoadBackupTables(ctx, reader, loadStats) -> Result<HashMap<String, Database>, SharedError>`：本文件的主入口。它克隆 `Context` 与 `MetaReader` 给后台线程，使用一个表通道和一个错误通道与前台协调。

本文件没有模块级常量、trait、泛型公开接口或条件编译项。其并发轮询间隔直接写为 `Duration::from_millis(10)`。

## 执行流程

1. `LoadBackupTables` 创建 `Vec<ReadSchemaOption>`；仅当 `loadStats == false` 时加入函数指针 `SkipStats`。它不传 `SkipFiles`，所以数据文件仍会被读取并挂到表上。
2. 函数创建无界的 `std::sync::mpsc` 表通道 `(tx, rx)` 和错误通道 `(errTx, errRx)`，再克隆 `ctx` 与 `reader`。
3. 后台线程调用 `reader.ReadSchemasFiles(&reader_ctx, callback, &opts)`。每次回调通过 `tx.send(table)` 交付一张表；若接收端已释放，回调构造 `BrokenPipe("schema reader output closed")`。
4. `ReadSchemasFiles` 自己负责更深层流水线：读取内嵌和索引 schema、用固定 8 个 worker 解析、按需读取文件并按 physical ID 关联，最后分批执行这里提供的回调。`SkipStats` 会先清除 schema 的 `stats` 和 `stats_index`，避免后续解析。
5. 若后台读取返回错误，线程尝试将错误送入 `errTx`；随后闭包结束，持有的 `tx`/`errTx` 被释放，通道最终断开。
6. 前台先检查原始 `ctx.is_cancelled()`，再非阻塞检查 `errRx.try_recv()`，之后以 10 ms 超时调用 `rx.recv_timeout()`。超时只用于重新检查取消与错误，不代表读取失败。
7. 收到 `Table` 时，以 `table.DB.Name.O.clone()` 为键。首次出现该库时克隆 `table.DB` 建立 `Database`、初始化空表向量和 `reusedByPITR = false`，然后追加当前表；后续同名库沿用第一次收到的 `DBInfo`。
8. 表通道断开表示后台不再生产。前台 join 后台线程，再检查一次错误通道；有错误则 Trace 后返回，否则返回完整映射。

由于 `ReadSchemasFiles` 内部存在多 worker、批内 `HashMap::into_values()`，本文件收到表的顺序不应被视为稳定协议；因此 `Database.Tables` 的顺序也不适合作为业务语义或持久化顺序。

## 数据与状态

聚合结果的顶层键使用 `DBInfo.Name.O`，即 `CIStr` 的原始大小写形式。这与 Go 的 `Name.String()` 在当前模型中的意图一致，但不会自动以小写名合并：大小写不同的 `O` 值会形成不同键。`Database.Info` 来自该键下首张到达的表，之后同键表携带的 `DBInfo` 不会覆盖它；调用方和元数据生产者应保证同名表的 DB 信息一致。

`Database.Tables` 只追加、不去重。更深层的 `ReadSchemasFiles` 会在单批中按表 ID 建立 `table_map`，但跨批、同名异 ID或异常重复输入仍不由本文件消除。`GetTable` 按名称返回第一个匹配项，时间复杂度为 O(n)，适合恢复阶段按库持有的顺序集合，而不是高频索引。

`reusedByPITR` 是普通 `bool`，没有原子性或内部可变性。新建 `Database` 时固定为 `false`，调用 `SetReusedByPITR` 后为 `true`。修改需要 `&mut Database`，并发共享时必须由上层提供锁或其他同步。

## 依赖与调用关系

直接下游依赖如下：

- `astersql_objstore_storeapi::Context`：提供可克隆的取消状态；取消检查同时发生在本文件和 `MetaReader::ReadSchemasFiles` 内部。
- `crate::metafile::{MetaReader, ReadSchemaOption, SkipStats, Table}`：承载实际元数据读取、选项配置和表结果。
- `astersql_meta_model::DBInfo`：组成 `Database.Info`，其 `Name.O` 同时作为聚合键。
- `astersql_errors::{SharedError, Trace}`：统一线程和存储读取错误的返回类型与 Trace 包装。
- 标准库 `HashMap`、`mpsc`、`thread`、`Duration`：分别承担归桶、线程通信、后台任务和取消轮询。

Rust 侧上游事实是：`br/pkg/metautil/lib.rs` 将 API 公开到 crate 根；直接调用证据只出现在 `br/pkg/metautil/load_test.rs`。`br/pkg/restore/snap_client/client.rs` 当前使用的是其相邻 `stubs.rs` 内定义的独立 `metautil::Database`（其中 PITR 标记为 `Arc<AtomicBool>`），不是本文件的 `Database`，两者不可混同。

Go 主链则更完整：`SnapClient.LoadSchemaIfNeededAndInitClient` 调用 `metautil.LoadBackupTables` 填充恢复客户端数据库集合；`br/cmd/br/debug.go` 调用它生成 checksum 与 key-range 检查所需的表/文件集合。该 Go 证据说明 API 的设计位置，但不构成 Rust 生产接线已完成的证据。

## 错误处理与边界

- 调用开始时若 `ctx` 已取消，前台很快返回 `Interrupted("context canceled")`；独立测试 `load_backup_tables_returns_context_cancellation` 覆盖该分支。
- 运行中取消通过 10 ms 的 `recv_timeout` 周期被观察，因此不是严格的零延迟唤醒。取消分支立即返回，不 join `handle`；后台线程靠克隆的已取消 Context 和通道关闭自行收敛。
- `ReadSchemasFiles` 的错误由独立通道传播。前台在每轮收表前检查一次，并在表通道断开、join 后再检查一次，以覆盖“错误稍晚到达”的竞态；错误经 `trace_err` 返回。
- 后台线程 panic 时，`join()` 的 `Err` 被丢弃。若 panic 导致表通道断开且没有错误消息，正常断开分支会返回当前已聚合的映射；因此扩展时若要把线程 panic 变成业务错误，必须显式处理 join 结果。
- `GetTable` 对正常缺失返回 `None`，但对遍历中的 `Table.Info == None` 明确 panic。`load_test.rs` 的 `get_table_panics_for_missing_table_info_like_go` 固化了该兼容边界，不应擅自改成静默跳过。
- 聚合不会验证同一名称下各表的 `DBInfo` 是否一致，也不会拒绝空结果；空备份会正常返回空 `HashMap`。
- 通道是无界通道；如果读取速度远高于聚合速度，队列可能增长。当前聚合操作很轻，但大规模或昂贵回调演进时需要重新评估内存背压。

## 并发与资源生命周期

外层每次调用创建一个具名 `JoinHandle` 对应的后台读取线程。正常完成和通过错误通道完成时，前台会 join；取消时为了及时响应不会 join，线程成为暂时分离的后台任务。函数没有修改传入 `MetaReader`，而是依赖其 `Clone`：底层 storage 是 `Arc<dyn Storage + Send + Sync>`，备份元数据和 cipher 被克隆后跨线程使用。

发送端的析构是完成信号。后台闭包退出会 drop 表发送端，使 `recv_timeout` 得到 `Disconnected`；错误发送端同理。错误发送使用 `let _ = errTx.send(err)`，如果前台已经因取消返回，错误会被有意丢弃。表发送失败则被转换为 `BrokenPipe`，继而尝试经错误通道上报。

本文件的 `Database` 本身没有锁、原子变量或内部共享所有权。相比之下，`ReadSchemasFiles` 内部另有 schema 读取线程、固定 8 个解析 worker、文件读取线程和多个通道；这些线程的启动、批处理与错误收束属于 `metafile.rs`，不要在扩展本文件时重复实现那套流水线。

## 与 Go 版本的对应关系

`br/pkg/metautil/load.go` 是直接对照源。结构和语义对应关系如下：

- Go `Database.Info *model.DBInfo` / `Tables []*Table` 对应 Rust 拥有值的 `DBInfo` / `Vec<Table>`；Rust 消除了 `Database.Info` 自身为 nil 的状态，但保留 `Table.Info: Option<_>`。
- Go 的私有 `reusedByPITR bool` 与 Rust 普通 `bool` 对齐；Rust setter 因借用规则要求 `&mut self`。
- Go `GetTable` 返回 `*Table` 或 nil；Rust 返回 `Option<&Table>`。两者都按原始表名匹配，Rust 通过 `expect` 保持遇到 nil table info 时失败的行为。
- Go goroutine、`chan *Table`、`chan error` 和 `select` 对应 Rust 后台线程、两个 `mpsc` 通道和 10 ms 轮询循环。Rust 正常/错误路径显式 join，Go 不暴露 join；Rust 取消路径则同样优先返回。
- Go 在 goroutine 内构建 `opts`，Rust 在 spawn 前构建并 move；最终效果相同：`loadStats == false` 时只传 `SkipStats`。
- Go 用 `table.DB.Name.String()` 作键，Rust使用 `table.DB.Name.O`；现有 Rust 测试也用 `.O` 查找，体现当前移植约定。
- Go 错误在生产 goroutine和接收点调用 `errors.Trace`；Rust 在 `ReadSchemasFiles` 返回后把原错误送入通道，在接收点调用 `trace_err`。

Rust 测试 `test_load_backup_meta` 和 `test_load_backup_meta_partition_table` 对照 Go 同名测试，验证普通表/分区表的 SST 归属；三个 `benchmark_load_backup_meta_*` 把 Go benchmark 的 64、1024、10240 表规模收敛为单次功能冒烟。文件归属判断本身是 `metafile.rs` 的行为，本文件负责把其输出收集成库映射。

## 扩展指南

- 若新增加载选项，应优先扩展 `ReadSchemaOption`/`readSchemaConfig` 并在 `LoadBackupTables` 只做参数到选项的接线；不要把 schema、stats 或文件解析复制进本文件。同步更新 `br/pkg/metautil/metafile.rs`、独立测试 `br/pkg/metautil/load_test.rs`，并与 `metafile_test.rs` 的底层行为测试分工。
- 若要改变数据库键的大小写规范、重复库合并或表去重，修改点是 `LoadBackupTables` 的 `databases.entry(...)` 分支。必须先核对 Go `load.go`、所有按 `Name.O` 查找的 Rust 调用方，并增加包含大小写冲突和重复元数据的独立测试。
- 若要让 `GetTable` 容忍库级占位表，必须把这视为兼容性变化；现有 should-panic 测试明确要求保持 Go 式失败，不能只为便利改成 `filter_map`。
- 若要加强并发安全或允许共享状态下设置 PITR 标记，需要决定是维持 `&mut self`，还是采用锁/原子内部可变性。相邻 snap-client stub 的 `Arc<AtomicBool>` 只能作为当前迁移差异证据，不能未经主链设计确认直接照搬。
- 若要严格传播后台 panic，处理 `handle.join()` 的错误并定义新的 `SharedError`；同时覆盖“读线程 panic、已有部分结果、无 errTx 消息”的回归测试。
- 若要加入背压，可把无界 `mpsc::channel` 改为有界机制，但需验证 `ReadSchemasFiles` 回调、取消和 join 不形成互相等待。性能测试应继续放在独立 `load_test.rs`，不要把测试逻辑嵌入生产源文件。
- Rust 生产接线时，应从恢复初始化或 debug 命令的真实入口引入 crate 导出的 `LoadBackupTables`，并避免与 `snap_client/stubs.rs` 的同名 `Database` 混用；接线属于调用方任务，不应在本文件文档任务中假定已经完成。

## 验证依据

本说明基于以下本地证据人工复核：

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/metautil` 确认目标、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file br/pkg/metautil/load.rs --offset 1 --limit 260`：读取完整 150 行目标文件，并得到“used by 1 file: br/pkg/metautil/load_test.rs”。
- RustCodeGraph `query LoadBackupTables/GetTable/SetReusedByPITR/IsReusedByPITR/ReadSchemasFiles --json`：核对 Rust/Go 同名符号、签名和定义位置。
- RustCodeGraph `node` 读取 `br/pkg/metautil/metafile.rs:350-777`、`br/pkg/metautil/load_test.rs:1-422`、`br/pkg/restore/snap_client/client.rs:880-979` 与 `stubs.rs:1190-1259`：核对读取流水线、测试边界以及 canonical/stub 类型差异。精确 `callers`/`callees` 查询在本地 30 秒窗口内超时且无输出，因此没有把它误判为“无调用边”，而是用已索引文件关系和 `rg` 补证。
- 直接读取 `br/pkg/metautil/Cargo.toml`、`lib.rs`、`load.go`、`load_test.go`、`metafile.go`，并搜索 `br/**/*.rs` 与 `br/**/*.go` 的调用点：核对 crate 边界、公开再导出、Go 生产入口和 Rust 当前接线。目标目录不存在 `doc.go`，因此没有可额外读取的包契约文件。
- 任务为纯文档分析，按计划未运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题，并确认唯一生产物是 `br/pkg/metautil/load.rs.md`。
