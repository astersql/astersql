# `pkg/ddl/mock.rs`

源文件：[`mock.rs`](mock.rs)

## 文件定位

`mock.rs` 属于 `astersql-ddl` crate；[`Cargo.toml`](Cargo.toml) 的 `[lib]` 指向 `lib.rs`，[`lib.rs`](lib.rs) 以 `pub mod mock` 公开本模块，并仅在 `cfg(test)` 下装配独立的 [`mock_test.rs`](mock_test.rs)。文件提供 DDL 测试辅助 API：一个可修改的 delete-range 批大小测试值、一套空操作 delete-range 管理器接口，以及从完整 `CREATE TABLE` AST 构造 `TableInfo` 的便捷函数。

它不在 DDL owner、job 持久化或后台 GC 的生产主链上。精确引用搜索显示，批大小 API 只由 [`tests/serial/main_test.rs`](tests/serial/main_test.rs) 使用，`mock_table_info` 只由 `mock_test.rs` 使用，而 `new_mock_delete_range_manager` 和本文件的 `DeleteRangeManager` trait 没有文件外 Rust 调用者。生产侧另有 [`delete_range.rs`](delete_range.rs) 的具体 `DeleteRangeManager` 和独立常量；两者与本文件的同名接口/状态并未接线。

## 核心职责

- 用 `BATCH_INSERT_DELETE_RANGE_SIZE`、`set_batch_insert_delete_range_size` 和 `batch_insert_delete_range_size` 保存、修改并读取测试进程内的批大小配置。
- 定义最小的 `DeleteRangeManager` trait，并用无状态 `MockDeleteRange` 提供全部成功或空操作的实现，方便未来调用者以 trait object 注入替身。
- 用 `new_mock_delete_range_manager` 构造 `Box<dyn DeleteRangeManager>`，隐藏具体 mock 类型。
- 用 `mock_table_info` 复用正式的 `BuildTableInfoFromAST` 元数据构造路径，只在成功后覆盖调用者指定的表 ID。
- 明确隔离测试辅助与生产行为：本文件不提交 DDL job、不写 delete-range 系统表、不启动后台任务，也不更新 schema version。

## 主要符号

- `static BATCH_INSERT_DELETE_RANGE_SIZE: AtomicUsize`：初始值为 `256` 的进程级原子量。它与 `delete_range.rs::BATCH_INSERT_DELETE_RANGE_SIZE` 是两个不同符号；当前生产删除范围代码使用后者的常量，而不是这里的可变值。
- `set_batch_insert_delete_range_size(size: usize)`：以 `Ordering::SeqCst` 写入测试值，不做范围校验，也不返回旧值。
- `batch_insert_delete_range_size() -> usize`：以 `Ordering::SeqCst` 读取测试值，主要用于测试断言和恢复现场。
- `trait DeleteRangeManager: Send`：声明 `add_delete_range_job`、`remove_from_gc_delete_range`、`start`、`clear` 四个可变方法。作业接口被简化为 `job_id: i64`，错误被简化为 `String`。
- `MockDeleteRange`：零字段、`Default` 的空实现；两个作业方法始终返回 `Ok(())`，生命周期方法不改变任何状态。
- `new_mock_delete_range_manager() -> Box<dyn DeleteRangeManager>`：返回装箱后的默认 `MockDeleteRange`。`Send` 约束允许所有权跨线程移动，但不代表对象可被并发共享。
- `mock_table_info<C, E>(context, statement, table_id)`：调用 `BuildTableInfoFromAST`，传播解析/元数据校验错误，成功时把 `TableInfo.ID` 改成指定值后返回。

## 执行流程

批大小测试流程由 `tests/serial/main_test.rs` 展示：先读取原值，`setup_test_environment` 调用 setter 写入 `2`，测试再通过 getter 断言，最后显式恢复原值。这个流程只验证测试配置自身可写、可读；由于 `delete_range.rs` 使用独立的 `pub const BATCH_INSERT_DELETE_RANGE_SIZE: usize = 256`，它目前不会改变真实 delete-range 分批逻辑。

mock 管理器流程很短：调用 `new_mock_delete_range_manager` 得到 trait object；调用 `add_delete_range_job` 或 `remove_from_gc_delete_range` 时忽略 job ID 并立即成功；`start`、`clear` 均为空操作。当前仓库没有外部 Rust 调用边，因此这只是可注入接口，不应推断为已替代生产管理器。

表元数据构造流程如下：

1. 调用者先把 SQL 解析成 `ast::CreateTableStmt`，并准备 `metabuild::Context<C, E>`。
2. `mock_table_info` 把上下文与完整 AST 原样传给 `create_table.rs::BuildTableInfoFromAST`。
3. 正式 builder 通过 `build_table_info_with_check` 处理列、索引、约束、外键、表选项和校验；任一错误用 `?` 原样提前返回。
4. 只有 builder 成功后才覆盖 `table.ID = table_id`，其他元数据保持正式构建结果，随后返回 `TableInfo`。

## 数据与状态

本文件唯一持久到进程生命周期的可变状态是 `AtomicUsize`。默认值 `256` 与 Go 的 `batchInsertDeleteRangeSize` 默认值一致，但 Rust 状态当前只对本模块 getter 可见。setter 接受包括 `0` 在内的任意 `usize`；本文件没有消费该值，因此也没有除零、零步长或批处理死循环，但未来接线到分批循环前必须定义并验证 `0` 的行为。

`MockDeleteRange` 不保存 pending/completed 队列、started 标志或 job ID，所有实例行为等价。它与 `delete_range.rs::DeleteRangeManager { pending, completed, started }` 不是同一类型，也不模拟后者的任务生成、重试、归档或清理状态。

`mock_table_info` 不克隆输入 AST，也不修改 `Context`；它拥有 builder 返回的 `TableInfo`，只覆写 `ID`。泛型 `C: ?Sized + 'static, E: 'static` 由 `metabuild::Context` 和正式 builder 的接口决定，本文件不对二者施加额外业务约束。

## 依赖与调用关系

直接下游依赖为：标准库 `AtomicUsize/Ordering`；Cargo 中的 `astersql-meta-metabuild`、`astersql-meta-model`、`astersql-parser`、`astersql-parser-ast`；以及同 crate 的 `BuildTableInfoFromAST`。`mock_table_info -> BuildTableInfoFromAST -> build_table_info_with_check` 是本文件唯一进入正式 DDL 元数据逻辑的调用边。

已核实的上游引用为：

- `pkg/ddl/mock_test.rs -> mock::mock_table_info`，覆盖完整建表语义和错误传播。
- `pkg/ddl/tests/serial/main_test.rs -> set_batch_insert_delete_range_size / batch_insert_delete_range_size`，覆盖设置、读取和恢复测试值。
- `pkg/ddl/lib.rs -> pub mod mock`，构成 crate 的公开模块边界。

RustCodeGraph 识别该文件 18 个符号，并能定位上述主要定义；精确 `callers/callees` 命令在本次查询窗口内未返回结果，因此调用关系又用限定于 `pkg/ddl` 的精确引用搜索交叉核验。搜索未发现 `new_mock_delete_range_manager`、`MockDeleteRange` 或本文件 trait 的文件外使用，也未发现本文件原子量被 `delete_range.rs` 消费。

## 错误处理与边界

批大小 setter/getter 不返回错误，且不校验 `size > 0`。测试如果在断言失败、panic 或并发执行期间未走到显式恢复，会把全局值留给后续测试；`SeqCst` 只保证原子可见性，不能提供测试级作用域恢复。

mock 管理器刻意吞掉全部业务效果：它不验证 job ID 是否存在或为正，不保存任务，也无法模拟插入失败、移除失败、启动顺序、重复操作或清理行为。其 `Result<(), String>` 允许未来替身报告错误，但当前实现永远成功；不能用它证明真实 delete-range 系统表或 GC 路径正确。

`mock_table_info` 的失败边界来自正式 builder，错误类型为 `parser::errors::Error`。在失败路径中没有 `TableInfo` 可被覆写，指定的 `table_id` 不会掩盖 builder 错误。`mock_test.rs` 用大小写重复列 `a`/`A` 验证错误传播；成功测试验证主键 handle、生成列、唯一索引、外键、检查约束、字符集、排序规则和注释，说明它不是仅拼列名的简化构造器。

## 并发与资源生命周期

原子批大小使用最强的 `SeqCst` 顺序，多个线程读写不会发生数据竞争；但它是全局共享配置，两个并行测试仍会发生逻辑覆盖，且“读取旧值—写临时值—恢复旧值”不是一个原子事务。当前调用位于串行测试目录并显式恢复，这降低但没有从 API 层消除污染风险。

`MockDeleteRange` 没有锁、线程、异步任务、channel、事务、数据库连接或析构动作。trait 只要求 `Send`，未要求 `Sync`；`&mut self` 又要求调用者独占访问。`start` 不启动后台 worker，`clear` 也没有资源需要释放。

`mock_table_info` 是同步内存计算。它借用 context 与 AST，返回拥有所有权的 `TableInfo`；本文件不持有借用、不创建会话或任务，也没有取消、重试、checkpoint 和故障恢复语义。

## 与 Go 版本的对应关系

Go 对照文件是 [`mock.go`](mock.go)。Rust `set_batch_insert_delete_range_size` 对应 `SetBatchInsertDeleteRangeSize` 的测试配置意图，但接线并不等价：Go setter 直接修改 `delete_range.go` 分批循环读取的包变量；Rust setter 修改 `mock.rs` 私有原子量，而 `delete_range.rs` 的真实逻辑使用独立编译期常量 `256`。因此 Rust 当前只保留测试可观察值，没有改变生产批量大小。

Rust `MockDeleteRange`/trait 对应 Go `mockDelRange`/`delRangeManager` 的空实现意图。Go 的 `ddl.newDeleteRangeManager(mock)` 在 mock 分支真实调用 `newMockDelRangeManager`，随后调用 `start`；Go 方法还接收 `context.Context` 和完整 `*model.Job`。Rust 只接收 job ID，仓库未找到对应构造链调用，且生产 `delete_range.rs` 使用另一个具体 struct，而不是实现本 trait。

Rust `mock_table_info` 与 Go `MockTableInfo` 的共同点是从 `CreateTableStmt` 构造表元数据并最后赋指定 ID。实现路径有差异：Go 函数显式调用 `buildColumnsAndConstraints`、`BuildTableInfo`、`setTableAutoRandomBits` 和 `handleTableOptions`；Rust 复用更完整的 `BuildTableInfoFromAST` 正式入口。Rust 测试证明该入口同时保留索引、外键、check constraint、生成列及表选项，并传播正式 builder 校验错误，因此在当前覆盖范围内没有为测试另造简化语义。

## 扩展指南

- 若要让测试批大小真正影响 Rust delete-range 分批，应先统一 `mock.rs` 与 `delete_range.rs` 的配置来源，明确禁止或定义零值，并为修改前失败、修改后通过的独立回归测试提供真实分批观察点。还应使用 RAII guard 或串行化机制确保 panic 时恢复，避免全局测试污染。
- 若要接入 mock manager，应让生产抽象和 mock 共用同一个 trait，保留 Go 所需的完整 job/context/error 语义，并在 DDL 构造路径验证 mock 分支、`start` 和 `clear` 生命周期；不要让同名但互不兼容的 `delete_range.rs::DeleteRangeManager` 与本 trait 长期并存。
- 扩展 `mock_table_info` 时优先继续复用 `BuildTableInfoFromAST`，不要手写一套弱化的列或表选项构造逻辑。新增 builder 行为应在正式 builder 的独立测试与 [`mock_test.rs`](mock_test.rs) 中同步验证成功元数据和错误传播。
- 测试逻辑必须继续放在独立 `*_test.rs` 文件。批大小/manager 行为适合扩展串行测试或新增同目录独立测试；建表辅助继续扩展 `mock_test.rs`。若需求涉及用户可见 DDL 行为，还需在真实 executor/job/delete-range 路径增加相应测试，不能以 mock 测试替代。
- 兼容性风险主要是 Go 与 Rust 接线、错误载荷和 job 参数差异；并发风险主要是全局测试值互相覆盖；性能风险当前很低，真正接入生产分批后才需评估过小批次造成的系统表写入次数。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ddl/mock.rs` 报告目标文件有 18 个符号；`node --file pkg/ddl/mock.rs --offset 1 --limit 400` 读取全部 91 行；`query` 核对了两个批大小函数、trait、mock struct、构造器、`mock_table_info` 和 `BuildTableInfoFromAST` 的签名。精确 `callers/callees` 查询超时且无输出，未把无输出解释为不存在调用。
- crate 与模块证据：读取 `pkg/ddl/Cargo.toml`，核对依赖和 `[lib] path = "lib.rs"`；读取 `pkg/ddl/lib.rs`，核对 `pub mod mock` 与独立 `mock_test` 装配。
- Rust 实现证据：读取 `pkg/ddl/mock.rs`、`pkg/ddl/create_table.rs::BuildTableInfoFromAST` 和 `pkg/ddl/delete_range.rs`；后者证明真实 manager/state 与批大小常量独立存在。
- Rust 测试证据：读取 `pkg/ddl/mock_test.rs`，核对完整 CREATE TABLE 元数据和错误传播；读取 `pkg/ddl/tests/serial/main_test.rs`，核对全局批大小设置、读取和恢复；限定引用搜索确认没有其他直接 Rust 使用者。
- Go 对照证据：读取 `pkg/ddl/mock.go`，核对 setter、`mockDelRange` 与 `MockTableInfo`；读取 `pkg/ddl/ddl.go::newDeleteRangeManager` 和 `pkg/ddl/delete_range.go` 的引用，确认 Go mock manager 与批大小已接入实际路径。
- DDL 契约证据：读取 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`，并以源码确认本文件不实现 job 持久化、owner 状态机、schema sync 或后台 GC。
- 本任务为纯文档分析，按计划未运行 Cargo。最终以任务指定命令检查固定十一个二级标题，并人工复核“为何存在、如何运行、如何安全扩展”均可由上述源码、测试或调用证据回溯。
