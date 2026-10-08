# `pkg/sessionctx/variable/varsutil.rs`

## 文件定位

`varsutil.rs` 属于 `astersql-sessionctx-variable` crate，是系统变量子系统的公共转换、校验和副作用适配层。crate 入口 `pkg/sessionctx/variable/lib.rs` 以私有模块 `mod varsutil` 装入本文件，再通过 `pub use varsutil::*` 将其中的公开项暴露给本 crate 及上层模块。`pkg/sessionctx/variable/Cargo.toml` 声明该 crate 对 `vardef`、`parser-charset`、`chrono` 和 `sysinfo` 的直接依赖，分别支撑变量常量、字符集/排序规则元数据、时间解析和主机内存探测。

它位于“字符串形式的系统变量值”与“`SessionVars`/进程级配置”之间：`pkg/sessionctx/variable/sysvar_builtins.rs` 将这里的校验器和 setter 接到具体 `SysVar`，而会话、DDL、表达式、HTTP 状态等模块直接复用 `BoolToOnOff`、`TiDBOptOn`、`TiDBOptOnOffWarn` 等无状态助手。本文件不负责 SQL `SET` 的语法解析、变量注册表查找或持久化事务。

## 核心职责

1. 规范化常见变量表示：布尔值与 `ON`/`OFF`、`true`/`false` 的互换，三态 `OFF`/`ON`/`WARN`、断言等级及整数/浮点数的容错解析。
2. 实现系统变量校验：字符集和排序规则、默认 `utf8mb4` 排序规则、只读兼容开关、隔离级别、ANALYZE 跳过列类型。
3. 解析带业务约束的容量：服务器内存限制和 Schema 缓存大小，包括百分比、二进制单位、下限/上限裁剪和语句警告。
4. 维护时间旅行读取状态：设置 `SnapshotTS`、`TxnReadTS`、`ReadStaleness`，执行互斥检查并清理关联状态。
5. 桥接进程级副作用：通过已注入钩子切换 DDL/统计 owner，通过 `GlobalVarsAccessor` 更新密码最小长度并同步原子配置。
6. 提供表达式索引 GA 函数与 ANALYZE 可跳过类型的白名单表示。

## 主要符号

- `BoolToOnOff(bool) -> String`、私有 `int32ToBoolStr`、`trueFalseToOnOff`、`OnOffToTrueFalse`：表示层转换；未知字符串保持原样。`TiDBOptOn` 仅把忽略 ASCII 大小写的 `ON` 和精确的 `"1"` 视为开启。
- `OffInt`、`OnInt`、`WarnInt` 与 `TiDBOptOnOffWarn`：将三态变量压缩为整数；除精确 `WARN`、`ON` 外均落到 `OFF`。`AssertionLevel`/`tidbOptAssertionLevel` 同样对未知输入回退到关闭。
- `tidbOptPositiveInt32`、`TidbOptInt`、`TidbOptInt64`、`TidbOptUint64`、`tidbOptFloat64`：解析失败时返回调用方提供的默认值；正整数版本还拒绝零和负数。
- `checkCollation`、`checkDefaultCollationForUTF8MB4`、`checkCharacterSet`：依赖 `parser_charset::charset` 返回注册元数据中的规范名称。默认 `utf8mb4` 排序规则只接受 `utf8mb4_bin`、`utf8mb4_general_ci`、`utf8mb4_0900_ai_ci`。
- `checkReadOnly`：开启 READ ONLY/OFFLINE MODE 时，按 session/global scope 检查 `tidb_enable_noop_functions`，在 `OFF` 时拒绝、`WARN` 时向 `StmtCtx` 添加警告。
- `checkIsolationLevel`：对 `SERIALIZABLE` 和 `READ-UNCOMMITTED` 默认报 `UnsupportedIsolationLevel`；开启 `tidb_skip_isolation_level_check` 后放行并警告。
- `getTiDBTableValue`/`setTiDBTableValue`：通过 `GlobalVarsAccessor` 兼容 `mysql.tidb` 中的旧式布尔存储。读取失败被有意转换成调用方默认值；写入错误向上传播。
- `parseMemoryLimit`、`parsePercentage`、`parseByteSize`：解析服务器内存限额。百分比必须在 1..99；容量支持无后缀字节数以及 KB/KiB、MB/MiB、GB/GiB、TB/TiB（大小写敏感）。非零结果低于 512 MiB 时被抬升并记录截断警告。
- `parseSchemaCacheSize`：复用 `parseByteSize`，将非零小值抬到 `SchemaCacheSizeLowerBound`（64 MiB），并把超过 `i64::MAX` 的值裁剪到该上限。
- `setSnapshotTS`、`parseTSFromNumberOrTime`、私有 `parseTSFromTime`、`setTxnReadTS`、`setReadStaleness`：解析并维护三类读取时间状态。时间字符串使用会话时区，物理毫秒左移 18 位生成 TSO。
- `switchDDL`、`switchStats`：调用 crate 中的可注入 enable/disable 钩子；具体 owner 生命周期不在本文件实现。
- `GAFunction4ExpressionIndex`、`collectAllowFuncName4ExpressionIndex`、`expression_index_function_map`：分别提供静态白名单、排序后的展示字符串和查找表。
- `analyzeSkipAllowedTypes`、`ValidAnalyzeSkipColumnTypes`、`ParseAnalyzeSkipColumnTypes`：校验/规范化或宽容解析 JSON、文本、BLOB 类列类型。
- `updatePasswordValidationLength`：先写全局系统变量，成功后才更新 `vardef::PasswordValidationLength`，避免持久化失败时发布不一致的进程内值。
- `secondsPerYear`、`initChunkSizeUpperBound`、`maxChunkSizeLowerBound`：供变量定义使用的边界常量；`appendDeprecationWarning` 统一向当前语句附加弃用告警。

## 执行流程

典型系统变量写入从 `sysvar_builtins.rs` 注册的 `SysVar::Validation` 或 setter 进入本文件。以 `tidb_server_memory_limit` 为例，注册闭包调用 `parseMemoryLimit`：先在允许探测总内存时尝试 `N%`，百分比无效或不可用再尝试字节容量；解析失败返回 `TruncatedWrongValue`；成功但低于 512 MiB 则写警告并抬升；调用方随后把字节值写入进程级原子量并保存规范字符串。`tidb_schema_cache_size` 走相同入口模式，但使用 64 MiB 下限和 `i64::MAX` 上限。

时间旅行变量直接改变会话状态：`setSnapshotTS` 对空串执行清理；非空时先拒绝已启用的 `ReadStaleness`，再按纯数字 TSO或本地时间解析。无论解析是否成功，它都会用解析结果（错误时为 0）更新 `SnapshotTS` 并清零 `TxnReadTS`。`setTxnReadTS` 只接受时间格式，解析成功后清除 snapshot 及其 infoschema；`setReadStaleness` 将有符号秒数乘以 10^9 存为纳秒，并在 snapshot 非零时拒绝设置。

只读和隔离级别校验属于“返回规范值并可能产生警告”的路径。`checkReadOnly` 只在目标值为开启时工作：session scope 读取 `NoopFuncsMode`，global scope 经 `GlobalVarsAccessor` 查询同名全局变量，其他 scope 原样返回 `original`。`checkIsolationLevel` 从 `SessionVars::system` 读取跳过开关，未跳过时报错，跳过则保留请求值并追加警告。

## 数据与状态

多数转换函数只处理借用字符串，不保存状态。持久状态集中在传入的 `SessionVars`：

- `StmtCtx` 收集弃用、截断、noop 功能和不支持隔离级别警告；调用方必须在当前语句生命周期内消费这些警告。
- `SnapshotTS`、`SnapshotInfoschema`、`TxnReadTS`、`ReadStaleness` 共同描述历史读视图。代码显式维护 snapshot 与 staleness、snapshot 与 txn-read-TS 的互斥，但 `setTxnReadTS` 本身不检查或清除 `ReadStaleness`。
- `memory_total_available` 和 `memory_total` 控制百分比内存解析：前者为假时禁用百分比基数；后者非零时优先使用缓存值，否则调用 `sysinfo::System::new_all().total_memory()`。
- `GlobalVarsAccessor` 是外部可变边界，承担 `mysql.tidb` 兼容值和全局变量的读写。
- `GAFunction4ExpressionIndex` 与 `analyzeSkipAllowedTypes` 是只读静态切片；查找集合/映射由调用时新建，不共享可变容器。
- `vardef::PasswordValidationLength` 是进程级原子配置，只有外部写入成功后才通过 `Store` 发布。

容量乘法采用不同溢出语义：百分比路径使用 `saturating_mul`，单位路径使用 `wrapping_shl`。因此扩展单位或调整上限时不能假设所有超大输入都会统一饱和；现有 Schema 缓存路径会在移位后再裁剪，服务器内存路径没有 `i64::MAX` 裁剪。

## 依赖与调用关系

RustCodeGraph 索引将 `varsutil.rs` 识别为含 56 个符号的文件，并报告有 20 个文件使用它。直接结构证据如下：

- 上游装配：`pkg/sessionctx/variable/lib.rs` 私有声明并公开再导出整个模块。
- 系统变量注册：`pkg/sessionctx/variable/sysvar_builtins.rs` 把 `checkIsolationLevel` 接到事务隔离变量，把 `checkReadOnly` 接到只读/离线变量，把 `parseMemoryLimit` 和 `parseSchemaCacheSize` 接到全局容量变量。
- 会话内消费：`pkg/sessionctx/variable/session.rs` 使用 `TidbOptInt64` 和 `TiDBOptOn` 更新字段；`variable.rs` 使用 `TiDBOptOn` 生成布尔 `Datum`。
- 跨 crate 消费：`pkg/server/http_status.rs`、`pkg/ddl/executor.rs`、`pkg/session/runtime/*`、`pkg/expression/exprstatic/*`、`pkg/util/gcutil/gcutil.rs` 等复用布尔或三态转换。
- 下游依赖：字符集校验调用 `parser_charset::charset::{GetCollationByName, GetCharsetInfo}`；时间解析调用 `chrono`；容量探测调用 `sysinfo`；持久化和警告均经 `SessionVars` 提供的接口完成。

RustCodeGraph 对精确同名符号能够区分 Rust/Go 节点（例如 Rust `parseMemoryLimit` 为 `varsutil.rs::parseMemoryLimit`，Rust `setSnapshotTS` 为 `varsutil.rs::setSnapshotTS`），但本次 `callers`/`callees` 命令未返回可用的逐边输出，因此上述调用边又以注册文件和全仓 `rg` 交叉核对；未把图工具未展示的边描述为已验证图边。

## 错误处理与边界

本文件统一返回 `VariableError`，但不同助手的容错契约不同：基础数值助手静默回退默认值；白名单、时间和容量校验返回错误；部分业务约束用 `StmtCtx` 警告后修正并成功返回。调用方扩展时应保持这三类契约，不应把警告式裁剪改成硬错误或反之。

需要特别保留的边界包括：空字符集按字符串 `NULL` 报错；未知排序规则映射为 `WrongValue`；百分比 0、100 及以上无效；容量单位大小写敏感且不接受尾随字符；内存/Schema 容量的零值不会触发最小值裁剪；负时间戳被拒绝；本地时间遇到 DST 歧义取第一个候选，遇到不存在的本地时间报错；`setSnapshotTS` 解析失败仍清零 snapshot/txn-read-TS；`getTiDBTableValue` 将任何 accessor 读取错误当作缺省值处理。

`ParseAnalyzeSkipColumnTypes` 是宽容读取器，不会 trim 分项，非法项（包括带前导空格的 `" text"`）被忽略；`ValidAnalyzeSkipColumnTypes` 才会 trim、拒绝任一非法项并输出逗号紧凑格式。两者不可互换。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁或事务。所有会话级修改要求调用方持有独占的 `&mut SessionVars`，因此 Rust 借用规则阻止同一会话对象被这些 setter 并发写入。只读转换和静态白名单可并发调用。

进程级资源通过边界接口管理：DDL/统计 owner 的实际启停由 crate 中注册的钩子执行，`switchDDL`/`switchStats` 只同步调用并传播错误；未注入钩子的安全行为由 `call_*_hook` 实现及独立测试确认。密码长度使用原子 `Store` 发布，但它与 accessor 持久化不构成跨资源事务；当前顺序保证写入失败不会更新原子值，写入成功后的进程崩溃恢复则取决于外部全局变量加载流程。

`sysinfo::System::new_all()` 是按调用创建的系统快照，可能有显著探测成本；`SessionVars::memory_total` 提供避免重复探测的缓存入口。`expression_index_function_map` 每次分配新 `HashMap`，高频调用方若出现应自行缓存，而不是引入全局可变映射。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/sessionctx/variable/varsutil.go`，Rust 保留了主要函数名、常量和流程，独立 Go 测试位于 `pkg/sessionctx/variable/varsutil_test.go`。已核对的对应关系包括 ON/OFF 助手、隔离级别/noop 校验、旧 `mysql.tidb` 兼容读写、容量解析、历史读状态、owner 开关、表达式索引白名单、ANALYZE 类型白名单和 Schema 缓存裁剪。

实现层差异必须视为兼容审查点：Go 排序规则走 `util/collate`，Rust 走 `parser-charset`；Go 时间解析使用 TiDB `types.ParseTime` 与 `oracle.GoTimeToTS`，Rust 使用 `chrono` 并手工左移 18 位，因此 SQL 时间语法、时区/DST 和错误分类不天然等价；Go 的 owner 函数指针为 `nil` 时直接成功，Rust 的等价行为委托给 `call_*_hook`；Go 表达式函数白名单以 AST 常量为键，Rust 当前保存字符串字面量；Go 内存总量来自 `memory.GetMemTotalIgnoreErr`，Rust 还受会话内存探测字段控制。

还存在可观察的实现细节差异：Rust `checkDefaultCollationForUTF8MB4` 显式列出三个允许值；Rust `parseByteSize` 以 `wrapping_shl` 计算，而 Go 的无符号左移按 Go 规则执行；Rust 的错误类型/消息是 `VariableErrorKind` 映射，不保证与 Go 的带错误码堆栈对象逐字相同。文档只陈述当前代码，未将这些差异判断为缺陷。

## 扩展指南

- 新增系统变量校验时，先确定它是纯规范化、警告式修正还是硬错误，再在 `sysvar_builtins.rs` 的对应 `SysVar` 上接入；同步扩展独立的 `varsutil_test.rs` 或 `tidb_vars_4_aster_unit_test.rs`，不要把测试写入生产文件。
- 修改历史读逻辑时，应把 `SnapshotTS`、`SnapshotInfoschema`、`TxnReadTS`、`ReadStaleness` 作为一个状态机审查，并新增成功、清空、解析失败和互斥冲突用例；同时对照 Go 的错误后副作用顺序。
- 新增容量单位或调整阈值时，应覆盖零值、大小写、尾随字符、移位溢出、百分比无系统内存基数、下限警告和上限裁剪，并明确规范化字符串的预期。
- 扩展 `GAFunction4ExpressionIndex` 时，应同步展示字符串和查找映射的测试，并核对 Go AST 常量对应的稳定名称；扩展 ANALYZE 类型时同时更新校验器和宽容解析器。
- 新增全局副作用应延续“外部写入成功后再发布进程内状态”的顺序，并评估失败原子性、重试和启动恢复；不要在本文件中实现 owner 的具体生命周期。
- 若修改公共符号，应检查 `lib.rs` 的整体再导出所形成的跨 crate API 面，以及 RustCodeGraph/全仓搜索列出的 `pkg/session/runtime`、`pkg/expression`、`pkg/server`、`pkg/ddl` 等调用方。

## 验证依据

- 源码与装配：`pkg/sessionctx/variable/varsutil.rs`（610 行、56 个索引符号）、`pkg/sessionctx/variable/lib.rs`、`pkg/sessionctx/variable/Cargo.toml`。
- Rust 调用与注册：`pkg/sessionctx/variable/sysvar_builtins.rs`、`pkg/sessionctx/variable/session.rs`、`pkg/sessionctx/variable/variable.rs`，以及全仓对公开助手的 Rust 引用搜索。
- Rust 独立测试：`pkg/sessionctx/variable/varsutil_test.rs` 覆盖开关、字符集/排序规则、带小数秒 timestamp 和错误后的时间状态；`pkg/sessionctx/variable/tidb_vars_4_aster_unit_test.rs` 覆盖容量、白名单、时间状态互斥和可注入 owner 钩子。
- Go 对照：`pkg/sessionctx/variable/varsutil.go` 与 `pkg/sessionctx/variable/varsutil_test.go`。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/sessionctx/variable/varsutil.rs` 找到目标文件；`node --file ...` 读取完整源码；`query --json` 区分了 Rust/Go 的 `parseMemoryLimit`、`setSnapshotTS`、`ValidAnalyzeSkipColumnTypes`、`collectAllowFuncName4ExpressionIndex` 节点。精确 `callers`/`callees` 未产生可用输出，调用关系因此由模块注册和文本引用补证。
- 按任务约束，本次是纯文档分析，不运行 Cargo；结构验证要求目标文件存在且恰好包含上述 11 个固定二级标题。
