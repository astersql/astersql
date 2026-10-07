# `lightning/pkg/errormanager/errormanager.rs`

## 文件定位

本文件是 `astersql-lightning-pkg-errormanager` crate 的主体实现，crate 边界由同目录的 `Cargo.toml` 定义，`lib.rs` 先导出 `stubs.rs` 提供的迁移期外围类型，再以 `pub use errormanager::*` 暴露本文件 API。它对应 Go 包 `lightning/pkg/errormanager` 的 `errormanager.go`，在 Lightning 导入链中负责把可容忍的数据错误和唯一键冲突转换为三类结果：原子计数、任务信息库中的明细表、面向用户的摘要。

当前 Rust 接线并不等于 Go 生产链已经全部移植完成。RustCodeGraph 显示本文件被 `lightning/pkg/importer/import.rs`、`lightning/pkg/errormanager/parity_test.rs` 和 `lightning/cmd/tidb-lightning-ctl/stubs.rs` 使用；其中 `import.rs` 的 `Controller.errorMgr` 会构造 `ErrorManager`，`outputErrorSummary` 会调用 `Output()`，但本仓库 Rust 生产代码中没有找到 `RecordTypeError`、两类 `Record*ConflictError`、`RecordDuplicate*` 或 `ReplaceConflictKeys` 的直接调用。它们的行为目前主要由独立 Rust 测试保护。因此，本文件是真实实现而非纯门面，但 SQL、KV、表编码、worker pool 等下游仍是同 crate `stubs.rs` 所定义的迁移边界。

## 核心职责

1. `New` 根据 backend、冲突策略、预检查开关和 task-info schema 是否启用，建立错误管理器的初始状态。
2. `Init` 按实际启用的错误类别创建 schema、错误表和统一冲突视图；没有 DB 或没有任何允许记录的错误时不产生 DDL。
3. `RecordTypeError`、`RecordDataConflictError`、`RecordIndexConflictError`、`RecordDuplicate*` 先消费原子配额，再选择是否把可查询明细写入 SQL 表，并把阈值越界或落库失败返回给调用方。
4. `ReplaceConflictKeys` 实现 `replace-on-duplicate` 的冲突清理：依据错误表中的历史 KV 和下游最新 KV，识别真正应删除的输家行/索引，同时保护仍属于胜者行的 KV。
5. `HasError`、`LogErrorDetails` 和 `Output` 从“配置初值 - 剩余配额”计算累计错误，分别用于判断、日志告警和终端表格摘要。

这些职责以 `ErrorManager` 为统一状态边界。SQL 表名和模板常量（例如 `ConflictErrorTableName`、`DupRecordTableName`、`ConflictViewName`）既是内部持久化协议，也是用户排障时可见的兼容接口。

## 主要符号

- `ErrorManager`：核心状态对象。`db`/`schema` 决定是否保存明细；`taskID` 标识任务；`configError` 与 `configConflict` 保存初始阈值；`remainingError`、`conflictErrRemain`、`conflictRecordsRemain` 是运行期剩余额度；`conflictV1Enabled`/`conflictV2Enabled` 控制冲突表版本；`recordErrorOnce` 是一次性写入门闩；`logger` 负责诊断；`encode_map: Arc<Mutex<...>>` 是当前桩编码器与 replace 测试共享的编码映射。
- `DataConflictInfo { RawKey, RawValue, KeyData, Row }`：批量记录数据/索引冲突时同时携带机器可用原始 KV 和面向人的解码文本。
- `New(Option<sql::DB>, &config::Config, log::Logger) -> ErrorManager`：公开构造器。仅当 `TaskInfoSchemaName` 非空时保留传入 DB；local backend 且策略非 `NoneOnDup` 启用 V1，local+预检查或 TiDB backend 启用 V2。
- `Init(&self, Context) -> Result<()>`：条件创建 `syntax_error_v2`、`type_error_v2`、`conflict_error_v4`、`conflict_records_v2` 和 `conflict_view`。视图会按 V1/V2 开关选择单表投影或 `UNION ALL`。
- `RecordTypeError(...)`：每次先 `remainingError.Type.Dec()`。越界时返回原编码错误（正阈值时附加 `max-error.type` 说明）；额度内且 DB 存在时写 `type_error_v2`。
- `RecordDataConflictError(...)` / `RecordIndexConflictError(...)`：空批次直接成功；非空批次一次扣除批量大小并在单事务中拼接多值 INSERT。即使扣减后越界，仍尝试保存本批明细；但若 SQL 事务失败，当前实现用 SQL 错误替换先前保存的阈值错误。
- `ReplaceConflictKeys(...)`：接受表元数据、动态 worker pool、读最新 KV 回调 `fnGetLatest` 和批量删 key 回调 `fnDeleteKeys`，分索引 KV、数据 KV、附加行清理三个阶段执行。
- `RecordDuplicateCount`：只扣冲突总额度，不写明细。
- `RecordDuplicate`：先扣冲突总额度，再扣可记录行数；总额度越界返回错误，明细额度耗尽则跳过 INSERT 但保留总计数变化。
- `RecordDuplicateOnce`：以 `CompareAndSwap(false, true)` 保证本实例只尝试写第一条重复记录；写失败仅告警，不向调用者返回错误。`RecordErrorOnce` 只是非原子快照读取。
- `TypeErrorsRemain`、`ConflictErrorsRemain`、`ConflictRecordsRemain`：暴露剩余额度。
- `HasError`、`LogErrorDetails`、`Output`：汇总入口。`Output` 无错误时返回空串，有错误时生成红色表格；字符集错误目前没有对应明细表。

## 执行流程

典型生命周期从 `New` 开始。构造器复制配置阈值，创建原子计数器，推导 V1/V2 开关；若 task-info schema 为空，则故意丢弃 DB，使整个管理器退化为“只计数、不落库”。随后 `Init` 检查 DB，按剩余额度和冲突开关组装 DDL 列表。列表只有 schema 一项时说明没有可记录类别，直接返回；否则按顺序建 schema/表，最后建立与启用版本相符的冲突视图。

记录类型错误时，`RecordTypeError` 先消费额度。剩余值小于零代表超过容忍上限，函数不会再落库，而是把原始编码错误返回；额度内才借助 `SQLWithRetry` 写一行，并在日志上下文中附带 offset、经脱敏的行文本和错误消息。如果 INSERT 失败，通过 `multierr::Append` 同时保留编码错误与数据库错误。

记录 V1 冲突时，两种 `Record*ConflictError` 都先对整个批次扣减 `conflictErrRemain`，再在事务内构造单条批量 INSERT。数据冲突固定使用 `PRIMARY` 并从 `RawKey` 判断 `kv_type`；索引冲突还需要与 `indexNames`、`rawHandles`、`rawRows` 同下标取值。调用者必须保证这些平行切片长度与 `conflictInfos` 一致，否则 Rust 索引会 panic；函数自身没有长度校验。

`ReplaceConflictKeys` 的主流程是：

1. DB 不存在时直接成功；否则以 `rowLimit = 1000` 和 `[start,end)` ID 区间驱动 `WorkerPool::RunDynamic`，大区间由 `splitRemaining` 二分后继续调度。
2. 索引 KV 阶段查询 `kv_type = 0` 的冲突。若错误表中的索引值仍等于下游最新值，说明它仍有效，跳过；若已被覆盖，则读取其 `rawHandle` 对应最新行、重新编码整行。只有重编码结果仍包含原冲突 `(rawKey, rawValue)` 时，才删除该输家行 key，并把被删行以 `kv_type = 2` 重新写入冲突表供下一阶段处理。
3. 数据 KV 阶段查询 `kv_type <> 0`。同一 `rawKey` 只取一次下游最新行并重编码为 `mustKeepKvPairs`；对每个历史行重新编码，逐个检查候选 KV 在下游是否仍是同值，并排除属于最新行的保护集合，余下 key 才交给删除回调。
4. 最后事务性执行 `DELETE ... WHERE kv_type = 2 LIMIT 1000`，循环到 `RowsAffected == 0`，清除为 replace 中间过程追加的行。

结束阶段以 `errorCount` 将剩余计数钳制到不小于零后计算累计数，避免越界后的负剩余额度把显示计数放大。`Output` 仅列出实际计数为正的类别；V1/V2 同时启用也只展示一行统一的 `conflict_view`。

## 数据与状态

持久化对象分为四张表和一个视图：语法错误表保存文件位置、错误与上下文；类型错误表保存位置、错误和原始行；`conflict_error_v4` 保存可重放的 raw key/value/handle/row、解码文本及 `kv_type`；`conflict_records_v2` 保存预检查阶段面向用户的重复行；`conflict_view` 对 V1/V2 统一列形。当前 Rust 文件没有专门写入语法/字符集错误的方法，但 `remainingError.Syntax` 与 `remainingError.Charset` 仍参与建表、计数和摘要，供与 Go 配置/共享状态契约保持一致。

计数有两个相互独立的维度：`conflictErrRemain` 控制任务可容忍的冲突总数，`conflictRecordsRemain` 只限制能写入明细表的重复记录行数。后者耗尽不会阻止前者继续统计。所有计数都可能变为负数，用负值表达“已经越界”；展示累计数时再把剩余值按零钳制。

`encode_map` 不是 Go `ErrorManager` 的业务字段，而是当前 Rust 迁移版为 `NewBaseKVEncoderWithMap` 桩和测试提供的共享映射。它以 `Arc<Mutex<_>>` 跨 worker 使用，说明目前 replace 编码证据来自可控映射，而不是完整 TiDB 编码栈。

## 依赖与调用关系

上游方面，`lightning/pkg/importer/Cargo.toml` 以路径依赖引入本 crate；`NewImportControllerWithPauser` 把 importer 配置桥接成 errormanager 的桩配置、创建内存 DB 并调用 `New`，`Controller::outputErrorSummary` 调用 `Output()`。RustCodeGraph 还记录了 parity 测试和 Lightning 控制端桩的引用。当前没有图或文本证据证明其他公开记录方法已被 Rust 生产路径调用，不能把 Go 调用面直接视为 Rust 已接线状态。

下游方面：

- `common::SQLWithRetry`、`sql::DB`/事务/查询行承担 DDL、批量 INSERT、查询与清理事务；标识符通过 `SprintfWithIdentifiers`/`FprintfWithIdentifiers` 转义，值通过 `SqlValue` 参数绑定。
- `atomic::{Int64,Bool}` 承担跨调用配额和一次性门闩。
- `tablecodec`、`tables`、`kv`、`encode`、`tidbtbl`、`types` 负责 row key 解码、行数据解码及重新编码。
- `util::WorkerPool::RunDynamic` 负责 replace 区间工作调度；`fnGetLatest` 和 `fnDeleteKeys` 把真实存储读取/删除留给调用方。
- `redact::NeedRedact` 控制 SQL 查询日志隐藏，`redact::Value` 保护行文本；`log`/`zap`/`logutil` 记录诊断；`pretty_table` 渲染用户摘要。
- `errors`、`multierr`、`tikverr` 分别提供错误包装/合并以及 `ErrNotFound` 分类。

## 错误处理与边界

- `db == None` 是受支持模式：`Init`、`ReplaceConflictKeys` 和明细落库路径会无操作返回，但额度仍可被记录方法消费；这使关闭 task-info schema 时仍能执行阈值判断。
- 阈值判断采用“扣减后 `< 0` 才越界”，所以阈值为 1 时第一条把剩余降到 0 并成功，第二条失败。冲突批量 API 即使本批跨过阈值仍尝试记录该批，保证小阈值不会导致完全没有诊断明细。
- `RecordTypeError` 合并原始编码错误和 INSERT 错误；冲突批量 API 则在事务失败时用事务错误覆盖既有阈值错误。调用方不应假设所有路径都能同时保留两个原因。
- `RecordDuplicateOnce` 吞掉落库错误并只写 warning，而且 CAS 在落库之前置位；第一次落库失败后不会自动重试第二条。这是与 Go 对齐的可观察语义。
- replace 中 `tikverr::IsErrNotFound` 是可忽略的历史状态：索引/行已经消失或候选索引已删除时继续处理；其他读取错误、解码错误、重编码错误、SQL 行扫描/关闭错误和删除回调错误都会立即传播。
- 数据 KV 阶段用 `Option<Vec<u8>>` 区分“存在但值为空”和“不存在”。比较时又用空切片模拟 Go 的 `bytes.Equal(empty, nil) == true`；且下一行 `ErrNotFound` 时保留上一行的 `mustKeepKvPairs`。这两个看似反直觉的分支都有 Rust 回归测试，修改时不可按直觉清空状态。
- `RecordIndexConflictError` 对三个平行数组直接以 `i` 索引，长度不一致会 panic；这是当前 API 前置条件和潜在健壮性风险。
- 字符集错误可计数和显示，但没有明细表名；`LogErrorDetails` 会格式化空表名，`Output` 的该列为空。

## 并发与资源生命周期

额度和 `recordErrorOnce` 使用原子类型，因此多个记录调用可并发扣减且“一次写入”门闩是 CAS；但 `RecordErrorOnce()` 与后续 CAS 不是一个原子复合操作，只适合观察，不能用于先检查再执行的同步协议。Go 注释还明确这些计数不在多个 Lightning 实例之间共享；Rust 同样只维护进程内对象状态。

`ReplaceConflictKeys` 把 worker 数量交给传入的 `WorkerPool`，任务按 ID 半开区间动态拆分。每个 worker 创建自己的编码器和查询行句柄；查询结束显式检查 `Rows.Err()` 并调用 `Close()`。SQL 修改通过 `SQLWithRetry::Transact` 收束到事务，外部 KV 删除则由回调执行，因此 SQL 与 KV 之间不存在一个跨系统原子事务：失败重试必须保持读取、删除和补写操作的幂等/可恢复性。

`encode_map` 的 `Arc<Mutex<_>>` 允许 worker 共享桩编码数据；锁行为属于当前 stub 实现的资源约束。`ErrorManager` 本身不拥有后台线程，也没有 `Close` 方法；传入/克隆的 DB 生命周期由外层管理，测试会在操作后显式 `DB::Close()` 验证资源可释放。

## 与 Go 版本的对应关系

`errormanager.rs` 的公开常量、`ErrorManager` 主要字段、构造开关、DDL/INSERT 模板、阈值扣减、三阶段 replace 算法和摘要顺序都直接对应 `errormanager.go`。`errormanager_test.rs` 对应 `errormanager_test.go`，`resolveconflict_test.rs` 对应 `resolveconflict_test.go`；`parity_test.rs` 额外把 Rust 公共契约分为 normal、boundary、error、resource cleanup 四组检查。

需要注意的 Rust 适配点：Go 的 `*sql.DB == nil` 变成 `Option<sql::DB>`；Go 的 goroutine/channel/errgroup 区间调度被 `WorkerPool::RunDynamic` 表达；Go 的 `nil []byte` 与空 slice 差异由 `Option<Vec<u8>>` 显式承载；Go 编码器在 Rust 迁移阶段由带 `encode_map` 的桩实现替代。Rust `Output` 通过本地 `pretty_table::render` 复刻 Go `go-pretty` 输出，相关测试对完整 ANSI 表格字符串做断言。

测试证据显示 Rust 还特意保护了两个 Go 细节：下游最新行存在但值为空时，不得删除属于该行的索引；下一组行读取返回 `ErrNotFound` 时，沿用前一组 `mustKeepKvPairs`。此外，非聚簇整数/字符串主键、唯一索引、多输家、已提前删除索引以及清理循环都由独立 replace 测试覆盖。当前 crate 的 `[dependencies]` 为空且依赖均由 `stubs.rs` 本地提供，说明其运行边界仍是迁移期模拟环境，不应宣称已等同连接真实 TiDB/TiKV 的 Go 包。

## 扩展指南

- 新增错误类别时，需要同时扩展配置/剩余额度、建表 SQL、记录入口、`errorCount`/`HasError`、`LogErrorDetails`、`Output`，并决定它是否有用户可查询表。同步更新独立的 `errormanager_test.rs` 和 `parity_test.rs`，不要把测试内嵌进生产文件。
- 修改表名、列或视图时，需把常量、DDL、INSERT/SELECT、Go 对照、用户摘要与 SQL 日志断言作为同一兼容协议处理；历史任务 schema 的向后兼容风险高于普通内部重构。
- 扩展 `RecordIndexConflictError` 时，优先为平行切片增加显式长度校验并在 Rust/Go 两侧补等价回归；否则错误输入会在事务构造时 panic。
- 修改 replace 算法时，以 `errormanager_test.rs` 的空值/缺失值保护用例和 `resolveconflict_test.rs` 的四类非聚簇主键用例为最低回归面。新增真实存储接线时，应把 `fnGetLatest`/`fnDeleteKeys` 的幂等性、取消传播、SQL 与 KV 部分成功后的恢复策略写入测试。
- 若将 `stubs.rs` 替换为 canonical crate，实现必须保持 `ErrNotFound` 分类、row key/row data 解码、非聚簇主键补 handle、编码器 KV 集合和 SQL 参数顺序；性能上重点关注每行重编码、逐 KV 最新值查询及 1000 行分页带来的读放大。
- 若扩展多实例运行，当前进程内原子计数不足以提供全局阈值，需先定义跨实例一致性协议；不能只把原子类型换成锁就声称解决。
- 生产接线新增调用者时，应同时验证 importer 是否调用 `Init`、所有记录入口和 `LogErrorDetails`，并避免继续使用独立内存 DB 桥接后却宣称错误明细已写入任务数据库。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter lightning/pkg/errormanager` 确认目标、Go 对照和独立测试集合；`node --file lightning/pkg/errormanager/errormanager.rs` 分段核对全部 1312 行；`explore` 给出本文件被 importer、parity test 和控制端桩使用，以及 `recordDuplicate`、计数/摘要方法的内部调用边。对重名严重的 `New`/`Output`，最终以上述文件限定结果和源码调用点为准。
- crate/入口：`lightning/pkg/errormanager/Cargo.toml`、`lightning/pkg/errormanager/lib.rs`、`lightning/pkg/errormanager/stubs.rs`；该包没有 `doc.go`。
- Rust 上游：`lightning/pkg/importer/Cargo.toml` 和 `lightning/pkg/importer/import.rs`（`Controller.errorMgr`、`NewImportControllerWithPauser`、`outputErrorSummary`）。
- Go 对照：`lightning/pkg/errormanager/errormanager.go`，核对构造、建表、记录、replace、计数与输出语义。
- Rust 测试：`lightning/pkg/errormanager/errormanager_test.rs`（初始化、两类基本 replace、空值与缺失值、摘要）、`resolveconflict_test.rs`（非聚簇整数/字符串主键和唯一索引）、`parity_test.rs`（公共契约、阈值、一次写入、资源清理）。
- Go 测试：`lightning/pkg/errormanager/errormanager_test.go`、`resolveconflict_test.go`，用于核对原测试意图和 case 形状。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的结构命令确认文档存在且恰有 11 个固定二级标题，并人工检查本文明确区分当前 Rust 接线、stub 边界和 Go 完整语义。
