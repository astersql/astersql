# `pkg/session/tidb.rs`

## 文件定位

`pkg/session/tidb.rs` 属于 `astersql-session` crate，并由 [`pkg/session/lib.rs`](lib.rs) 的 `pub mod tidb` 暴露。它不是完整的会话实现，而是把 Go 版本 [`pkg/session/tidb.go`](tidb.go) 中几组可独立描述的规则抽成 Rust 边界：Domain 缓存与初始化、SQL 解析警告转发、语句结束时的事务处理、事务语句数限制，以及测试用结果集读取。

该文件直接依赖 crate 根的 `SessionError`/`SessionResult`，并通过 `astersql-domain-serverinfo::SyncerOption` 表达 Domain server-info 注册选项；后者在 [`pkg/session/Cargo.toml`](Cargo.toml) 中声明为 `astersql-domain-serverinfo` 路径依赖。其余数据库对象没有在这里绑定具体实现，而由五组 runtime trait 注入。因此本文所述的是当前 Rust 文件已有规则，不能等同于 Go `session` 包全部主链均已接入。

## 核心职责

1. `domainMap` 按 `StorageRuntime::UUID` 缓存已初始化的 `DomainRuntime`，串行化同一映射中的查找/创建，并在 Domain 关闭时移除缓存项。
2. `Parse` 调用 `ParseRuntime::ParseSQL`，无论成功还是解析器能在失败后提供警告，都把警告追加回会话侧；解析错误本身保持不变。
3. `finishStmt` 协调连接存活检查、重试历史、statement 级提交/回滚、事务级自动提交/回滚、pending 事务清理和语句数上限。
4. `StmtHistory` 保存可重试写语句的 `Arc<dyn StatementRuntime>`；`GetHistory` 为可选历史提供惰性初始化。
5. `GetRows4Test` 与 `ResultSetToStringSlice` 提供测试辅助：按 chunk 耗尽结果集，并将 `NULL` 稳定表示为 `<nil>`。

## 主要符号

- 常量与全局状态：`StoreBootstrappedKey` 是清理 store bootstrap 标志所用键；`minConnectionAliveCheckBeforeCommitDuration` 是自动提交 DML 的一秒连接检查阈值；`ErrForUpdateCantRetry` 仅保留错误文案；`DOMAP: OnceLock<domainMap>` 只允许安装一次全局映射；`STATS_LEASE_SECONDS: AtomicI64` 是本文件内测试开关状态。
- Domain 边界：`StorageRuntime` 提供 UUID 和选项清理，`DomainRuntime` 提供初始化、关闭及关闭回调，`DomainFactory` 负责构造 Domain 和记录初始化失败。`domainMap::{new, Get, GetOrCreateWithFilter, getDomainForGlobalVarInit, getWithEtcdClient, Delete}` 实现缓存生命周期；`InstallDomainMap`、`ResetStoreForWithTiKVTest`、`DisableStats4Test` 暴露进程或测试级操作。
- 解析边界：`StatementKind` 区分收尾关心的语句类型；`ParsedStatement` 保存原文、类型和只读标志；`ParseRuntime` 抽象解析与 warning sink；`Parse` 是规则入口。`TakeWarningsAfterError` 的默认空实现保留既有适配器兼容性。
- 收尾边界：`StatementRuntime` 暴露只读性和种类；`FinishSessionRuntime` 汇集事务状态、连接检测、提交/回滚、指标、历史和新事务能力。`finishStmt` 是总入口，`autoCommitAfterStmt` 与 `checkStmtLimit` 也可分别复用；`recordAbortTxnDuration`、`isLoadDataLocal`、`shouldCheckConnectionAliveBeforeCommit` 是内部辅助。
- 结果集边界：`CellValue::{Null, Text}`、`Row`、`RecordSetRuntime` 构成最小结果集模型；`GetRows4Test` 负责拉取，`ResultSetToStringSlice` 负责转换并关闭。
- 历史：`StmtHistory::{new, Add, Count}` 维护有序语句向量，只暴露追加和计数，不在本文件执行回放。

## 执行流程

Domain 获取从 `domainMap::Get`、`GetOrCreateWithFilter` 或 `getDomainForGlobalVarInit` 进入 `getWithEtcdClient`。函数先持有 `domains` 互斥锁；无 store 时返回任意缓存 Domain，空缓存则报错；有 store 时先按 UUID 命中缓存。未命中会在 `max_retries` 次内调用 `DomainFactory::NewDomainWithEtcdClient` 和 `DomainRuntime::Init`。失败路径先 `Close` 再 `LogInitFailure`，成功路径安装一个捕获弱引用与 UUID 的 `SetOnClose` 回调、插入缓存并返回。全局变量初始化专用入口额外传入 `systemDBFilter` 和 `WithoutStatusEndpointClaim`。

解析从 `Parse` 进入。成功时依次 `AppendWarning`，再返回语句；失败时先用 `TakeWarningsAfterError` 提取解析器保留的警告并追加，最后原样返回 `SessionError`。因此 warning 的存在不改变成功/失败结论。

语句收尾从 `finishStmt` 进入：先计算只读性；对无既有错误、非只读、自动提交且不在显式事务中的 INSERT/UPDATE/DELETE，在语句持续至少一秒（或没有开始时间）时检查连接，检查失败成为本次语句错误。随后，成功且可重试的写语句会加入历史，但 `LoadDataLocal` 改为 `DisableRetry`；有效事务按错误与否执行 `StmtRollback(false)` 或 `StmtCommit`。然后 `autoCommitAfterStmt` 处理事务级动作，pending 状态统一转 invalid，最后仅在前述结果成功时执行 `checkStmtLimit(session, true)`。

`autoCommitAfterStmt` 遇到错误时：非显式事务总是回滚；显式事务只有 shared-lock-lost，或悲观事务死锁，才整事务回滚；两种回滚都记录 abort 指标，原错误继续返回。无错误且不在显式事务中时提交；若 COMMIT 语句提交失败，错误会附加前一条语句。显式事务的正常语句不做事务级提交。

`checkStmtLimit` 用历史长度作为已完成语句数，预检查（`is_finish == false`）额外计入当前语句。未超限直接成功；超限且未启用 batch commit 时回滚并报错；启用 batch commit 的预检查先放行，完成阶段则 `NewTxn`，随后无论创建结果如何都保持 `InTxn=true` 并返回创建结果。

结果读取由 `GetRows4Test` 循环复用同一个 `Vec<Row>`：每轮先清空，再调用 `Next`，空批次表示结束，否则克隆并累积行。`ResultSetToStringSlice` 只在成功读完后调用 `Close`，随后把文本原样输出、把空值转成 `<nil>`。

## 数据与状态

`domainMap.domains` 是 `Arc<Mutex<HashMap<String, Arc<dyn DomainRuntime>>>>`。强引用保证缓存期间 Domain 存活；关闭回调只持有 `Weak`，避免 Domain、映射和回调形成强引用环。`max_retries.max(1)` 保证即使构造参数为零也至少尝试一次。当前实现持锁覆盖 Domain 创建和 `Init`，以换取简单的唯一创建语义，但也意味着不同 store 的初始化会相互串行。

`DOMAP` 是一次赋值的进程级槽位；`InstallDomainMap` 重复调用返回错误，不替换原值。`STATS_LEASE_SECONDS` 使用 Release store 写入 `-1`，但本文件没有读取者；实际生产接线是否消费它必须由外部实现确认。`StmtHistory` 持有语句的共享所有权，`Count` 只反映记录数。`ParsedStatement`、`Row` 和 `CellValue` 是轻量值模型，不含 Go AST、字段类型或 chunk 的完整语义。

## 依赖与调用关系

RustCodeGraph 的 `node --file pkg/session/tidb.rs` 将该文件识别为 503 行业务实现，并报告被 9 个文件使用，列出的使用方包括 `pkg/session/runtime.rs`、`pkg/session/runtime/planning.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs` 和若干测试。精确符号文本检索显示，当前仓库内清晰的直接使用包括：`br/cmd/br/{backup,restore,abort}.rs` 调用 session 层 `DisableStats4Test`；[`pkg/session/main_test.rs`](main_test.rs) 调用 `ResultSetToStringSlice`；[`pkg/session/test/tidb_test.rs`](test/tidb_test.rs) 调用 `Parse` 与 `GetRows4Test`；[`pkg/session/tidb_test.rs`](tidb_test.rs) 直接验证 `Parse`、`autoCommitAfterStmt` 和 Domain 选项。

下游依赖由 trait 方法定义，而非具体类型：Domain 路径下调用 `UUID`、工厂构造、`Init`/`Close`/`SetOnClose`；解析路径下调用 `ParseSQL`、`AppendWarning`、`TakeWarningsAfterError`；收尾路径下调用 `FinishSessionRuntime` 的状态与事务操作；结果集路径下调用 `Next` 和 `Close`。这使规则能独立测试，但接入新 runtime 时必须完整实现 trait 所声明的不变量。

## 错误处理与边界

- `Mutex::lock` 使用 `expect("domain map lock poisoned")`：锁中毒会 panic，而不是转成 `SessionError`。
- nil store 只查找已有 Domain，不创建；映射为空时返回 `can not find available domain for a nil store`。HashMap 的“任意一个”没有稳定选择顺序。
- Domain 每次初始化失败都会先关闭临时对象并记录失败；全部失败后返回最后一个错误。工厂若异常地没有留下错误，才使用兜底 `domain initialization failed`。
- `Parse` 不吞解析错误；只有覆写 `TakeWarningsAfterError` 的 runtime 才能保留失败同时产生的 warnings。
- `finishStmt` 的连接检查仅覆盖指定 DML、自动提交、非显式事务和慢语句；`StatementKind::Other` 及无法在适配层识别的 prepared statement 不会自动获得 Go 版的解析能力。
- `LoadDataLocal` 因客户端文件流不可安全重放而禁用 retry，其他写语句才进入历史。
- `GetRows4Test(None)` 返回空向量。`Next` 失败立即返回，`ResultSetToStringSlice` 此时不会调用 `Close`；`Close` 失败也直接传播。Rust 文本单元格已经是 `String`，没有 Go 版按字段类型把 datum 转字符串时可能出现的转换错误。
- `ErrForUpdateCantRetry` 当前只是字符串常量，弱于 Go 中带 errno 分类的 `dbterror` 值；调用者不能据此获得同等的错误类别判定。

## 并发与资源生命周期

Domain 映射通过单个 `Mutex` 防止重复创建。成功 Domain 的关闭回调升级 `Weak`，映射仍存活才加锁删除对应 UUID；映射已释放时回调安全地无操作。显式 `Delete` 只移除缓存引用，不调用 `DomainRuntime::Close`。相反，初始化失败路径明确调用 `Close`，由具体 runtime 负责停止后台任务和释放资源。

`OnceLock` 使全局 Domain map 安装线程安全且不可替换；`AtomicI64` 避免测试禁用 stats 时的数据竞争。语句收尾和解析 trait 接口使用 `&mut self` 表示单会话状态的顺序修改，文件本身不派生异步任务。`StatementRuntime`、Domain 和工厂要求 `Send + Sync`，并用 `Arc` 穿越所有权边界；`StmtHistory` 因此能保存共享语句对象。

结果集读取是同步、串行且以空 chunk 为 EOF。转换函数拥有关闭责任，但仅覆盖成功排空之后的路径；若适配的结果集需要“错误也必须关闭”，调用层应提供守卫或扩展接口，不能依赖当前函数。

## 与 Go 版本的对应关系

Domain 的基本形状与 [`pkg/session/tidb.go`](tidb.go) 一致：按 store UUID 缓存、nil store 复用已有 Domain、初始化失败关闭后重试、成功后用 OnClose 删除。Rust 通过 `DomainFactory` 隐藏了 Go 中 lease、session factory、cross-keyspace factory、DDL injector 和 external workload manager 等构造细节；重试次数可注入，但未在本文件表达 Go 的重试间隔。全局变量临时 Domain 保留 `systemDBFilter` 和 `WithoutStatusEndpointClaim` 的关键语义。

`Parse` 对齐 Go 的“warnings 追加到 statement context、错误继续返回”。Rust 额外用 `TakeWarningsAfterError` 解决 `Result` 不能同时携带 warnings 和 error 的接口差异；默认实现为空。

`finishStmt`、`autoCommitAfterStmt` 与 `checkStmtLimit` 保留 Go 的主要分支：慢自动提交 DML 提交前检查连接、LOCAL LOAD DATA 禁重试、statement commit/rollback、pending 状态清理、自动提交、shared-lock-lost/悲观死锁回滚、COMMIT 错误附带前一语句，以及 batch commit 超限换事务。Rust 当前没有 Go 文件中的 failpoint、SQL killer 信号归一化、日志和按真实事务创建时间记录 duration；指标由 `ObserveAbortTxn` 抽象为事件，连接检查也依赖 `StatementKind`，没有 Go 的 prepared-statement 解析。

结果集辅助保留 chunk 复用、空 chunk 终止、`<nil>` 输出和成功后的关闭。Rust 行为模型比 Go 的 `chunk.Row`/field type 简化。Go `StmtHistory` 的完整回放上下文不在本文件；Rust 这里只保存 `StatementRuntime` 对象和计数。因此后续对齐不能仅凭签名相似认定完整等价。

测试证据中，[`pkg/session/test/tidb_test.go`](test/tidb_test.go) 的 `TestParseErrorWarn` 与 `TestSharedLockLostRollsBackTransaction` 分别对应解析警告/错误和 shared-lock-lost 回滚；Rust 的可执行对应项位于 [`pkg/session/test/tidb_test.rs`](test/tidb_test.rs) 与 [`pkg/session/tidb_test.rs`](tidb_test.rs)。后者还验证全局变量初始化 Domain 不抢占 serving status endpoint。

## 扩展指南

- 扩展 Domain 创建参数时，优先修改 `DomainFactory::NewDomainWithEtcdClient` 及 `domainMap::getWithEtcdClient` 的透传，并同步 `getDomainForGlobalVarInit` 的特殊约束；测试应放在独立的 [`pkg/session/tidb_test.rs`](tidb_test.rs)，不要内嵌到生产文件。需评估持锁初始化带来的跨 store 阻塞以及缓存命中时首次选项被固化的兼容性。
- 增加语句种类或 prepared-statement 支持时，修改 `StatementKind`、`shouldCheckConnectionAliveBeforeCommit` 或 `isLoadDataLocal`，并同步收尾 runtime 适配。必须保持 LOCAL 客户端流不可重试的不变量。
- 改动错误回滚条件时，在 `FinishSessionRuntime` 增加最小必要能力并修改 `autoCommitAfterStmt`；至少覆盖 autocommit、显式乐观/悲观事务、deadlock、shared-lock-lost 和普通错误，避免扩大整事务回滚范围。
- 改动语句上限时同时验证 `is_finish` 两阶段行为、batch commit、新事务失败后 `InTxn` 状态，以及错误消息兼容性。
- 增强结果集模型时，修改 `CellValue`/`Row`/`RecordSetRuntime` 和两个辅助函数，并同步 [`pkg/session/main_test.rs`](main_test.rs) 与 [`pkg/session/test/tidb_test.rs`](test/tidb_test.rs)。若要求错误路径也关闭资源，应显式设计 RAII/guard，而不是只在调用点补一次 `Close`。
- 新增生产接线前应重新运行限定到该文件的 RustCodeGraph callers/callees，并核对 runtime 是否真正实现 trait；当前测试可证明规则本身，不足以证明所有规则都已进入 SQL 服务主链。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/session/tidb.rs --offset 1 --limit 500` 与后续尾段读取覆盖目标文件全部 503 行，并给出文件使用方；对 `finishStmt`、`autoCommitAfterStmt`、`checkStmtLimit`、`Parse`、`ResultSetToStringSlice`、`GetRows4Test`、`InstallDomainMap` 做了精确符号查询。限定文件的 callers/callees 查询在本地未及时返回结果，因此调用关系又以索引文件使用方和精确源码引用交叉核对，未据此臆造具体调用边。
- 生产源码与装配：[`pkg/session/tidb.rs`](tidb.rs)、[`pkg/session/lib.rs`](lib.rs)、[`pkg/session/Cargo.toml`](Cargo.toml)。包下不存在 `doc.go`。
- Go 对照：[`pkg/session/tidb.go`](tidb.go)，重点核对 `domainMap`、`Parse`、`finishStmt`、`autoCommitAfterStmt`、`checkStmtLimit`、`GetHistory`、`GetRows4Test`、`ResultSetToStringSlice` 和 `ErrForUpdateCantRetry`。
- Rust 独立测试：[`pkg/session/tidb_test.rs`](tidb_test.rs) 的 shared-lock-lost、解析 warning/error 与 Domain server-info 选项；[`pkg/session/test/tidb_test.rs`](test/tidb_test.rs) 的解析及 chunk 排空；[`pkg/session/main_test.rs`](main_test.rs) 的 NULL 转换与成功关闭。
- Go 测试：[`pkg/session/test/tidb_test.go`](test/tidb_test.go) 的 `TestParseErrorWarn`、`TestSharedLockLostRollsBackTransaction`，以及 [`pkg/session/tidb_test.go`](tidb_test.go) 的 `TestDomapHandleNil` 等同路径会话测试。
- 本任务是纯文档分析，按任务要求不运行 Cargo。交付前用任务指定的 `rg` 命令验证固定二级标题恰好为 11，并人工复核文档能回答文件存在原因、主要流程、边界和安全扩展位置。
