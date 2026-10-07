# `pkg/lightning/common/util.rs`

## 文件定位

`util.rs` 是 `astersql-lightning-common` crate 的综合工具实现。crate 入口 `pkg/lightning/common/lib.rs` 以私有模块 `mod util` 纳入它，并通过 `pub use util::*` 把这里的公开类型、trait、常量和函数暴露给依赖 crate；仓库根门面又在 `pkg/lib.rs` 的 `lightning::common` 下再导出整个 crate。`pkg/lightning/common/Cargo.toml` 指定 `lib.rs` 为库入口，并以 `package.metadata.porting.go-package = "pkg/lightning/common"` 标明 Go 对照包。

文件位于 Lightning 公共层，横跨六类边界：MySQL 连接抽象、SQL 重试与事务、SQL 文本转义、表/索引元数据辅助、TiDB 配置查询，以及 RowID/KV 辅助。它没有自己的进程入口，也不拥有导入任务调度；上层应通过再导出的 API 调用它。

当前接线需要如实区分“可公开调用”与“已由生产代码调用”。RustCodeGraph 将该文件索引为 104 个符号，并显示 `ConnectMySQL -> MySQLConnectParam::Connect`、`Retry -> SQLWithRetry::perform` 等文件内调用边；仓库文本搜索未找到 `BuildAddIndexSQL`、`SkipReadRowCount`、`GetAutoRandomColumn`、`IsRaftKV2` 等 Rust 实现在独立生产文件中的直接调用。多个 Cargo crate 依赖 `astersql-lightning-common`，但目前主要使用该 crate 的其他公共能力。`lightning/pkg/importer`、`lightning/pkg/checkpoints` 等目录中存在同名 `common` 本地桩，不能据名称把它们误认成这里的直接调用者。

## 核心职责

1. 定义数据库边界。`SQLValue`、`QueryRows`、`Transaction` 和 `DBExecutor` 把驱动行为压缩为可注入接口；`MySQLConnector`、`MySQLConfig`、`MySQLConnectParam` 在不绑定具体 Rust MySQL 驱动的情况下描述连接参数和建连过程。
2. 提供失败可重试的 SQL 包装。`Retry` 统一最多三次尝试、生产三秒/测试十毫秒间隔和错误分类；`SQLWithRetry::{QueryRow, QueryStringRows, Transact, Exec}` 在该策略上承载查询、事务与执行。
3. 生成安全且与 MySQL 语法兼容的文本。`EscapeIdentifier`/`WriteMySQLIdentifier` 处理反引号，`InterpolateMySQLString` 处理单引号，`SprintfWithIdentifiers`/`FprintfWithIdentifiers` 只替换 `%s` 并先转义标识符，`UniqueTable` 组合限定名。
4. 查询目标库状态。`TableExists`、`SchemaExists`、三个会话变量读取函数、`IsRaftKV2` 和若干错误分类函数将 DB 返回值归一化为布尔值、字符串或 `CommonError`。
5. 表达导入所需的表结构子集。`CIStr`、`ColumnInfo`、`IndexColumn`、`IndexInfo`、`SchemaState`、`SchemaTableInfo` 只保留自动 ID、索引 DDL 与行数判定所需字段；它们不是完整 TiDB schema model。
6. 处理导入相关派生数据。`EncodeIntRowID` 生成与 Go `codec.EncodeComparableVarint` 对应的紧凑字节，`GetAutoRandomColumn` 定位 AUTO_RANDOM 列，`GetDropIndexInfos` 划分可删除索引，`BuildDropIndexSQL`/`BuildAddIndexSQL` 生成索引 DDL，`SkipReadRowCount` 决定能否省略精确行数读取。

## 主要符号

- 连接与执行接口：`SQLValue` 表示空值、整数、无符号整数、字符串、字节串和布尔参数；`QueryRows` 保存列名、字符串行及延迟迭代错误；`Transaction: Send` 暴露 `Commit`/`Rollback`；`DBExecutor: Send + Sync` 暴露 ping、关闭、连接池配置、查询、开事务和执行，前三项中除查询/事务/执行外有默认空实现。
- 连接配置：`MySQLConfig` 是驱动侧形状，`MySQLConnectParam` 是 Lightning 侧形状，`MySQLConnector` 是线程安全、可共享的工厂闭包。`ToDriverConfig` 强制 `charset=utf8mb4`，用单引号包装 `sql_mode` 与 `Vars`，默认网络为 `tcp`，并用 `join_host_port` 正确括起裸 IPv6 地址。`Connect` 建连后将最大空闲连接数设为 `available_parallelism()`，不可取得时退化为 1。
- 建连入口：`tryConnectMySQL` 要求已注入 `Connector`，连接后必须 `Ping`，ping 失败会尽力 `Close`；`ConnectMySQL` 仅在首次错误码为 1045 时尝试把密码按标准 base64 解码并重连，最终失败仍返回第一次错误。`decode_base64` 是私有严格解码器，检查长度、字符与 padding 位置。
- 重试入口：`Retry`、私有 `is_not_found`、`SQLWithRetry` 及私有 `perform`。`HideQueryLog` 保留了 Go 数据形状，但本 Rust 文件没有 logger 字段，也没有读该标志，因此当前不产生查询日志。
- 文件与上下文辅助：`IsDirExists`、`IsEmptyDir`、`IsContextCanceledError`、按平台条件编译的 `KillMySelf`。
- SQL 文本辅助：`UniqueTable`、私有 `escapeIdentifiers`、`SprintfWithIdentifiers`、`FprintfWithIdentifiers`、`EscapeIdentifier`、`WriteMySQLIdentifier`、`InterpolateMySQLString`。格式化器只认识 `%s` 和 `%%`；标识符不足时保留未匹配 `%s`，多余标识符被忽略。
- 数据库探测：`TableExists`、`SchemaExists`、私有 `getSessionVariable`/`tidb_opt_on`，以及 `GetBackoffWeightFromDB`、`GetPDEnableFollowerHandleRegion`、`GetExplicitRequestSourceTypeFromDB`、`IsRaftKV2`。
- 数据与 schema 类型：`KvPair`；`CIStr`；`ColumnInfo`；`IndexColumn`（默认长度为 `UnspecifiedLength = -1`）；`SchemaState::{None, Public}`；`IndexInfo`；`SchemaTableInfo`；位标志 `AUTO_INCREMENT_FLAG`、`PRI_KEY_FLAG`。
- 表结构算法：私有 `table_has_auto_row_id`、`TableHasAutoID`、`GetAutoRandomColumn`、`GetDropIndexInfos`、`BuildDropIndexSQL`、`BuildAddIndexSQL`、私有 `output_format`、`SkipReadRowCount`。
- 错误分类：`IsDupKeyError` 识别 1061/1068/1062，`IsFunctionNotExistErr` 依赖消息片段，`IsAccessDeniedNeedConfigPrivilegeError` 要求 1227 且消息含 `CONFIG`。
- 状态协议：`ChunkFlushStatus` 只有 `Flushed(&self) -> bool`，作为“刷盘是否完成”的最小观察接口；本文件未提供实现。

## 执行流程

连接主流程从 `MySQLConnectParam::Connect` 开始：先由 `ToDriverConfig` 构造配置，再调用 `ConnectMySQL`。后者第一次通过 `tryConnectMySQL` 调用注入工厂并 ping；成功立即返回。若失败是 1045 且密码能被严格 base64 解码为不同字节，则原地改写 `config.Passwd` 并只重试一次。二次失败不会替换首次错误。连接成功后，`Connect` 配置空闲连接数并返回共享的 `Arc<dyn DBExecutor>`。

SQLWithRetry 主流程由私有 `perform` 统一承载。`perform` 把有返回值的闭包包装成 `Retry` 所需的 `Result<(), CommonError>`，成功值暂存后取出。`Retry` 第一次立即调用；后续尝试前休眠；成功即返回；`Kind == "not-found"` 或 `ID == "NotFound"` 立即停止；只有 `IsRetryableError` 判定为真才继续，最多三次。最终错误的消息前置 `"<purpose> failed: "`。

`SQLWithRetry::Transact` 的每次重试都会重新 `BeginTx`。开始失败会附加 `begin transaction failed`；业务闭包失败时尽力 rollback、忽略 rollback 错误并保留业务错误；业务成功后 commit，提交失败附加 `commit transaction failed`。因此调用方闭包必须能够承受整个事务被再次执行，不能在事务外产生不可重复副作用。

索引 DDL 流程以 `SchemaTableInfo` 摘要为输入。`GetDropIndexInfos` 保留非 Public 索引、聚簇主键以及任何包含自增列的索引，其余归入可删除集合。`BuildAddIndexSQL` 按小写名跳过当前表已有索引，依主键/唯一/普通选择 ADD 形式；隐藏列输出生成表达式，普通列可带前缀长度；随后追加 INVISIBLE 和经 `output_format` 处理的注释。结果同时提供一条合并 ALTER 与逐索引 ALTER 列表；无新增索引时两者都为空。

行数跳过流程由 `SkipReadRowCount` 完成：缺少表信息、存在隐式 RowID 或 AUTO_RANDOM 时不能跳过；之后检查主键唯一索引及主键列上的 AUTO_INCREMENT 标志；只有这些都会安全时才返回 `true`。这对应“自动生成键可能依赖精确已有行数”的导入约束，而不是一般性的表行数优化器。

## 数据与状态

配置状态有两层。`MySQLConnectParam` 保留用户/Lightning 输入，`ToDriverConfig` 复制为 `MySQLConfig`；但 `ConnectMySQL(&mut MySQLConfig)` 在 base64 回退路径会原地把 `Passwd` 改为解码后的文本。调用它的 `MySQLConnectParam::Connect` 修改的是局部配置副本，因此不会改写原参数对象。

数据库对象通过 `Arc<dyn DBExecutor>` 跨调用共享；`DBExecutor` 要求 `Send + Sync`，连接工厂也要求 `Send + Sync`。`QueryRows` 把驱动迭代生命周期拍平成拥有所有权的 `Columns`/`Rows`，并用 `IterationError` 显式模拟 rows 迭代结束时才出现的错误。

schema 数据是刻意缩减的值对象。`CIStr` 同时保存原始值 `O` 与 Unicode 小写值 `L`；索引去重比较 `L`。`IndexColumn::Offset` 被当作 `SchemaTableInfo::Columns` 下标；部分函数用安全 `get`，但 `BuildAddIndexSQL` 直接索引，因此要求 offset 合法。`ColumnInfo::Flag` 使用本文件定义的主键/自增位。`SchemaState` 只建模 `None` 和 `Public`，不能表达 Go `model.SchemaState` 的完整状态集。

`KvPair` 拥有 key/value/row-id 三段字节；`EncodeIntRowID` 不保留外部缓冲区，始终新建 `Vec<u8>`。`ChunkFlushStatus` 只规定查询动作，不规定状态存放位置、同步机制或完成通知方式。

## 依赖与调用关系

直接 Rust 依赖来自同 crate 的 `CommonError`、`Context`、`IsRetryableError`、`TLSConfig`，以及标准库的集合、I/O、共享指针、线程和时间。`pkg/lightning/common/Cargo.toml` 仅声明 `astersql-lightning-log` 与 `libc` 为普通依赖，但本文件不直接引用二者；它也没有直接第三方 MySQL/base64 依赖，驱动由 `MySQLConnector` 注入，base64 由私有函数处理。

经 RustCodeGraph 确认的关键下游边包括：`MySQLConnectParam::Connect -> ToDriverConfig -> ConnectMySQL -> tryConnectMySQL/decode_base64`；`SQLWithRetry::{QueryRow, QueryStringRows, Transact, Exec} -> perform -> Retry -> IsRetryableError/is_not_found`；`BuildDropIndexSQL -> SprintfWithIdentifiers -> EscapeIdentifier`；`BuildAddIndexSQL -> EscapeIdentifier/output_format`；`GetAutoRandomColumn` 和 `SkipReadRowCount` 读取 schema 摘要。

经图和文本检索确认的上游现状是：`pkg/lightning/common/lib.rs` 对所有公开符号再导出；`pkg/lightning/common/util_test.rs` 直接调用连接、重试、转义、AUTO_RANDOM、索引 DDL 和行数判定入口；`pkg/lightning/common/key_adapter_test.rs` 使用 `EncodeIntRowID` 验证 key adapter。当前未发现这些 util API 在该 crate 之外的 Rust 生产调用边，因此不能声称它们已经接入完整 Rust Lightning 主链。Go 对照函数则有真实生产调用，例如 RustCodeGraph 显示 Go `SkipReadRowCount` 被 `pkg/executor/importer/table_import.go::PopulateChunks` 调用；这只证明 Go 主链位置，不等同于 Rust 已接线。

## 错误处理与边界

所有数据库路径用 `CommonError`。`TableExists`/`SchemaExists` 把 `Kind == "no-rows"` 转成 `false`，其他错误增加操作上下文；因此驱动适配层必须把无行错误规范化为这个 kind。`getSessionVariable` 传播查询/迭代错误，行少于两列时返回 `sql` 错误；零行时返回空串，多行时以最后一行值为准。`IsRaftKV2` 同样传播 `IterationError`，但不足四列的行被安全忽略。

连接回退有明确安全边界：只对 1045 尝试解码，只重试一次，严格拒绝长度非四倍数、非法字符、中间 padding 和 `x=yz` 形式的错误 padding。若解码字节不是 UTF-8，`String::from_utf8_lossy` 会替换非法序列，这与 Go 直接 `string([]byte)` 可保留任意字节不同，是需要兼容性测试关注的差异。回退失败返回第一错误，有助于保留用户最初看到的认证原因。

格式化辅助不是通用 SQL 参数绑定替代品。`SprintfWithIdentifiers` 仅用于标识符模板，`InterpolateMySQLString` 仅按单引号加倍；普通查询值仍应通过 `SQLValue` 参数传递。`BuildAddIndexSQL` 信任 `IndexColumn::Offset`，非法 offset 会 panic；隐藏列的 `GeneratedExprString` 被当作已可信 SQL 表达式直接嵌入。`IsFunctionNotExistErr` 和取消错误识别依赖 kind/message 文本，错误规范变化可能造成误判。

Unix `KillMySelf` 通过外部 `kill -INT <pid>` 命令发送信号，命令启动或退出失败都会成为 `signal` 错误；非 Unix 编译为固定“不支持”错误。目录判断把不存在、权限失败和其他 metadata/read_dir 错误都折叠为 `false`。

## 并发与资源生命周期

`DBExecutor: Send + Sync` 和 `Arc<dyn DBExecutor>` 允许连接句柄跨线程共享；`Transaction: Send` 允许事务对象在线程边界移动，但没有 `Sync` 要求，调用期间以 `&mut dyn Transaction` 独占访问。连接工厂闭包也被 `Arc` 包裹并要求 `Send + Sync`，便于测试和生产适配器共享。

资源关闭采用尽力而为策略：ping 失败时 `tryConnectMySQL` 调用 `Close` 并丢弃关闭错误；事务业务失败时 `Transact` 调用 `Rollback` 并丢弃 rollback 错误；成功路径必须显式 `Commit`。`SQLWithRetry` 本身不拥有关闭逻辑，释放或关闭共享 DB 仍由上层负责。

重试用 `std::thread::sleep` 阻塞当前 OS 线程，不是异步等待，也不检查 `Context` 是否在休眠期间取消；取消能否立即停止只取决于底层动作返回何种 `CommonError` 以及 `IsRetryableError` 分类。测试构建把间隔缩为 10ms，生产构建为 3s。

文件没有全局可变状态、锁、channel、后台任务或 unsafe 代码。主要可变状态局限在局部配置、结果缓存与事务句柄；真正的并发安全与连接池生命周期由 `DBExecutor` 的具体实现负责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/common/util.go`，Go 测试是 `pkg/lightning/common/util_test.go`。Rust 保留了多数公开名称、三次重试、三秒生产间隔、1045 后 base64 密码回退、标识符/字符串转义、INFORMATION_SCHEMA 探测、表索引算法、会话变量读取、raft-kv2 探测和 `ChunkFlushStatus` 协议。Rust 独立测试 `pkg/lightning/common/util_test.rs` 复刻了 Go 的主要用例，并额外覆盖非法 base64 padding 与索引注释转义。

Rust 不是对 Go 类型的逐字替换：Go 直接使用 `database/sql`、`mysql.Config`、完整 `model.TableInfo` 和日志对象；Rust 以 `DBExecutor`/`Transaction`、自有配置和缩减 schema 类型隔离依赖。Go `SQLWithRetry` 包含 `Logger`，能按 `HideQueryLog` 控制日志；Rust 结构仅保留 `DB` 与 `HideQueryLog`，当前不记录重试/SQL 日志。Go 查询行通过 `Rows.Close` 管理游标，Rust `QueryRows` 是已物化数据，没有游标关闭动作。

语义差异还包括：Go `WriteMySQLIdentifier` 按 UTF-8 字节复制，Rust 按 Unicode `char` 遍历，但对合法 UTF-8 字符串生成相同文本；Go `EncodeIntRowID` 委托 `codec.EncodeComparableVarint`，Rust手写同目标格式，当前 `util_test.rs` 没有直接逐边界验证它，相关证据主要来自 `pkg/lightning/common/key_adapter_test.rs`；Go `KillMySelf` 用进程信号 API，Rust Unix 版本启动 `kill` 子进程；Go schema state 更丰富，而 Rust 摘要只有两态。

迁移状态应描述为“公共能力已实现并由 crate 再导出，但部分能力尚未发现 Rust 生产调用”。不能仅凭 Go 已接入或 Rust 单测通过，就断言完整 Rust 导入链已经使用这些函数。

## 扩展指南

- 扩展真实数据库支持时，实现 `DBExecutor` 与 `Transaction`，并通过 `MySQLConnector` 注入。必须保持 no-rows、MySQL errno、迭代错误和 retryable 错误的 `CommonError` 规范，否则存在/重试判断会改变。测试应放在独立的 `pkg/lightning/common/util_test.rs`，不要嵌入源文件。
- 修改重试策略时，以 `Retry`/`SQLWithRetry::perform` 为唯一接入点；同步验证最大次数、首次不等待、not-found/取消不重试、事务每次重新开始及业务闭包幂等性。若加入取消感知或异步等待，要评估阻塞线程与 Go 日志语义差异。
- 增加 schema 字段或索引能力时，先扩展 `SchemaTableInfo`/`ColumnInfo`/`IndexInfo`，再调整 `GetAutoRandomColumn`、`GetDropIndexInfos`、`BuildAddIndexSQL` 或 `SkipReadRowCount`。必须同步 Go `model.TableInfo` 语义，并为非法 offset、非 Public 状态、表达式索引、前缀、可见性和注释转义补独立测试。
- 修改 SQL 生成时，区分标识符、字符串字面量和绑定值；不要把 `SprintfWithIdentifiers` 扩展成不受约束的 printf。新增格式语法需要覆盖标识符不足/过量、`%%`、反引号和多字节字符。
- 修改 `EncodeIntRowID` 时，应增加与 Go `codec.EncodeComparableVarint` 的跨语言边界向量，至少覆盖负数边界、`-1`、`0`、`239`、`240`、字节长度跃迁和 `i64::{MIN,MAX}`，并同步 `key_adapter_test.rs`。
- 若把当前公开 API 接入新的生产调用链，应在调用 crate 的 `Cargo.toml` 明确依赖并用实际 crate 名导入；先确认没有命中 `lightning/pkg/*/stubs.rs` 中的同名本地模块。兼容性风险主要是错误分类、DDL 文本和编码字节；性能风险主要是同步 sleep、查询全量物化及合并 ALTER 过长。

## 验证依据

- RustCodeGraph 索引状态：项目共 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/lightning/common/util.rs` 确认目标已索引且含 104 个符号。
- RustCodeGraph 源码与结构：读取 `pkg/lightning/common/util.rs` 1–959 行、`pkg/lightning/common/lib.rs` 1–93 行、`pkg/lightning/common/util_test.rs` 1–699 行；`node ConnectMySQL`、`node Retry`、`node BuildAddIndexSQL`、`node SkipReadRowCount`、`node EncodeIntRowID`、`node GetAutoRandomColumn`、`node IsRaftKV2` 用于核对调用 trail。
- crate 与上游边界：读取 `pkg/lightning/common/Cargo.toml`；搜索所有 Cargo manifest 中的 `astersql-lightning-common` 依赖；读取 `pkg/lib.rs` 的 `lightning::common` 门面；用 `rg` 区分真实 crate 引用和 `lightning/pkg/*/stubs.rs` 同名模块。
- Go 对照：通过 RustCodeGraph 读取 `pkg/lightning/common/util.go` 1–722 行与 `pkg/lightning/common/util_test.go` 1–307 行，并核对 Go `SkipReadRowCount <- pkg/executor/importer/table_import.go::PopulateChunks` 调用边。
- 测试证据：`pkg/lightning/common/util_test.rs` 覆盖目录判断、连接/base64 回退、取消识别、表名/字符串转义、三次重试、AUTO_RANDOM、ADD INDEX 与 SkipReadRowCount；`pkg/lightning/common/key_adapter_test.rs` 使用 `EncodeIntRowID`。本任务按计划是纯文档分析，未运行 Cargo 或代码测试。
- 人工复核结论：本文区分了已实现 API、crate 再导出和实际 Rust 生产接线；明确列出了重试/事务/资源生命周期、Go 差异、未验证接线及安全扩展入口，没有把 Go 调用关系推断成 Rust 现状。
