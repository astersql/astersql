# `pkg/parser/terror/terror.rs` 逻辑说明

## 文件定位

`terror.rs` 是 `astersql-parser-terror` crate 的实现文件，提供与 Go `pkg/parser/terror/terror.go` 对齐的分类错误框架。crate 入口 `pkg/parser/terror/lib.rs` 将本文件全部公开项同时再导出到 crate 根和 `parser::terror` 命名空间；上层因此既可直接依赖 `astersql_parser_terror::*`，也可沿兼容路径访问。

该文件位于 SQL 解析器目录，但职责并不限于语法错误：`ClassDDL`、`ClassKV`、`ClassSession` 等 27 个内置 `ErrClass` 为多个子系统提供统一的 RFC 错误身份，并把规范化错误转换为 MySQL 协议 `SQLError`。根工作区通过 `facade_parser_terror` 引入本 crate，`pkg/parser/Cargo.toml`、`pkg/util/dbterror/Cargo.toml` 以及多个执行、会话、DDL、存储 crate 直接或间接依赖它。

## 核心职责

1. 用 `ErrClass` 和 `ErrCode` 表达“子系统类别 + 类内编号”，并以 `ERROR_CLASSES` 固定 27 个内置类别的编号和文本前缀。
2. 通过 `ErrClass::New`、`NewStdErr`、`NewStd` 在初始化阶段登记错误码，调用共享 `errors::Normalize` 生成同时携带 MySQL code 与 RFC code（如 `parser:1064`）的 `Error`。
3. 通过 `ErrClass::Synthesize` 构造来自 TiKV 等外部系统的错误，但不把其 code 写入本地已注册集合。
4. 通过 `EqualClass`、`GetErrClass` 和 `ErrorEqual` 提供类别识别、根因比较及兼容普通错误文本的相等性判断。
5. 通过 `ToSQLError` 将已登记的规范化错误映射到 MySQL 协议错误；类别或 code 未知时回退 `mysql::errcode::ErrUnknown`。
6. 提供 `RegisterFinish`、`MustNil`、`Call`、`Log` 这些初始化冻结、终止清理与日志辅助能力。

## 主要符号

- `pub type Error = errors::Error`：复用 `astersql-errors` 的规范化错误实体，不在本 crate 重建错误载荷或栈模型。
- `ErrCode(pub isize)`：类内错误编号。`CodeUnknown`、`CodeExecResultIsEmpty`、`CodeMissConnectionID`、`CodeResultUndetermined` 对齐 Go 常量。
- `ErrClass(pub isize)`：子系统类别。`ClassAutoid` 到 `ClassUtil` 的数值 1 到 27 与 `ERROR_CLASSES` 中的描述严格配对；`ClassOptimizer` 的文本前缀特意是 `planner`。
- `Code2ErrClassMap`、`newCode2ErrClassMap`、`Get`、`Put`：维护 RFC 文本前缀到类别的反向索引；未命中返回 `(ErrClass(-1), false)`。
- `RegisterErrorClass`：向 `errClass2Desc` 增加类别；重复编号 panic。内置类别不是逐项调用该函数，而由 `ERROR_CLASSES` 一次性填充。
- `RegisterFinish` / `frozen`：以 `registerFinish: AtomicU32` 宣告注册阶段结束；冻结后 `New`、`NewStdErr`、`NewStd` 经 `initError` panic。
- `ErrClass::String`：已注册类别返回描述，未注册类别返回十进制编号。内部 `registeredDescription` 则对未注册类别返回空串，以保持 Go map 直接索引语义。
- `ErrClass::New`：已弃用的自由消息构造器；登记 code 后附加 MySQL code 与 RFC code。
- `ErrClass::NewStdErr` / `NewStd`：分别接收标准 `ErrMessage`，或先从 `mysql::errname::MySQLErrName()` 按 code 取标准模板；同时保留脱敏参数位置。
- `ErrClass::Synthesize`：不调用 `initError`，不登记 code，允许冻结后构造外部错误。
- `getMySQLErrorCode` / `ToSQLError`：前者验证 RFC 类别与已登记 code，后者用验证结果和原消息调用 `mysql::error::NewErrf`。
- `ErrCritical` / `ErrResultUndetermined`：由 `LazyLock` 延迟构造的全局标准错误模板；对应 code 3 和 2。
- `ErrorEqual` / `ErrorNotEqual`：委托共享 `errors` 基座处理根 cause、规范化 ID 和普通文本比较。
- `MustNil`：有错误时按切片顺序执行所有清理回调，刷新日志并以状态码 1 结束进程。
- `Call` / `Log`：分别记录闭包返回错误或可选错误，不向调用方传播。
- `GetErrClass`：从 `Error::RFCCode` 的冒号前缀反查类别，无法识别时返回 `ErrClass(-1)`。

## 执行流程

注册型错误的主流程是：调用方选取 `ErrClass` 和 `ErrCode`，调用 `New`、`NewStdErr` 或 `NewStd`；`initError` 首先检查 `frozen()`，再把 code 插入 `ErrClassToMySQLCodes[class]`，把类别描述写入 `rfcCode2errClass`，最后返回 `<描述>:<编号>`；构造器把该 RFC code、MySQL code（以及标准消息的脱敏位置）传给 `errors::Normalize`。例如 `pkg/parser/yy_parser.rs` 的 `ErrSyntax`、`ErrParse` 通过 `ClassParser.NewStd(...)` 建立解析器错误模板，`pkg/parser/charset/charset.rs` 用同一路径建立字符集/排序规则错误。

转换流程从 `ToSQLError` 进入 `getMySQLErrorCode`：先解析 RFC code 的冒号前缀，再通过 `rfcCode2errClass.Get` 得到类别；随后读取 `ErrClassToMySQLCodes`，确认错误自身 `Code()` 在该类别集合内。两步均成功才返回原 code，否则记录 warn/debug 日志并使用 `ErrUnknown`。最终 `NewErrf(..., "%s", ..., error.GetMsg())` 保留用户可见消息。

类别判断流程由 `EqualClass` 先调用 `errors::Cause` 解开 Trace/包装，确认根因可下转为 `Error`，再解析 RFC 前缀并查反向表；`NotEqualClass` 只是其否定。`GetErrClass` 直接处理给定 `Error`，失败哨兵为 `-1`。

外部错误走 `Synthesize`：它只读取类别描述并调用 `Normalize`，不更新任何注册表。因此错误仍有 RFC/MySQL 元数据，但若 code 没有被其他注册构造器登记，`ToSQLError` 会安全回退为 `ErrUnknown`。

## 数据与状态

- `errClass2Desc: LazyLock<RwLock<HashMap<ErrClass, String>>>`：首次访问时由 `ERROR_CLASSES` 全量构造。动态 `RegisterErrorClass` 也写入该表。
- `rfcCode2errClass: LazyLock<Code2ErrClassMap>`：RFC 前缀反向索引。初始化时预置 `global -> ClassGlobal`，确保全局错误在任意公开调用前可识别；其他前缀由 `initError` 写入。
- `ErrClassToMySQLCodes`：类别到已登记 code 集合的公开全局表。初值预置 `ClassGlobal` 的 `CodeExecResultIsEmpty` 和 `CodeResultUndetermined`，之后由 `initError` 扩充。
- `registerFinish: AtomicU32`：值非零表示禁止继续登记错误码；它没有解冻 API，是进程级单向状态转换。
- `defaultMySQLErrorCode`：固定为 `mysql::errcode::ErrUnknown`。
- `ErrCritical` 和 `ErrResultUndetermined`：`LazyLock<Box<Error>>` 模板。调用生成方法时共享模板身份和 RFC code，不把每次派生消息视为新注册。

三个注册表保存进程级状态，测试新增类别/code 时会持续到该测试进程结束。测试因此使用互不冲突的高位类别编号，并用子进程隔离不可逆的 `RegisterFinish` 和 `MustNil`。

## 依赖与调用关系

`pkg/parser/terror/Cargo.toml` 声明三个运行时依赖：`astersql-errors` 提供 `Error`、`SharedError`、`Cause`、`Normalize` 和相等性；`astersql-parser-mysql` 提供标准错误码、消息表、`ErrMessage` 与 `SQLError`；`log` 提供告警、调试和终止辅助日志。`serde_json` 仅为测试依赖，用于验证共享 `Error` 的兼容序列化。

本文件内部主要调用边为：`New`/`NewStdErr -> initError -> ErrClassToMySQLCodes + rfcCode2errClass.Put`；`NewStd -> MySQLErrName + NewStdErr`；`EqualClass -> errors::Cause + rfcCode2errClass.Get`；`ToSQLError -> getMySQLErrorCode -> rfcCode2errClass.Get + ErrClassToMySQLCodes`；所有构造路径最终调用 `errors::Normalize`。

RustCodeGraph 将 `terror.rs` 识别为 72 个符号、被 35 个文件使用。直接源码证据包括：`pkg/parser/yy_parser.rs` 和 `pkg/parser/parser_semantic_support.rs` 调用 `NewStd` 建立解析/DDL 标准错误；`pkg/parser/charset/encoding_base.rs` 建立 `ClassParser` 错误；`pkg/server/internal/resultset/resultset.rs` 用 `Call` 记录 `RecordSet::Close` 失败；`pkg/tablecodec/tablecodec_test.rs`、`pkg/structure/structure_test.rs` 和 `pkg/domain/domain_utils_test.rs` 验证 `ToSQLError`；`pkg/parser/types/etc_test.rs` 与 `pkg/server/err/migration_aster_unit_test.rs` 验证 `RegisterFinish` 时序。

## 错误处理与边界

- 重复 `RegisterErrorClass` 会 panic，原描述不被覆盖。
- 冻结后调用注册型构造器会打印强制 backtrace 并 panic；`Synthesize` 不登记，因此不受冻结限制。
- `NewStd` 对标准消息表中不存在的 code 使用 `expect`，会 panic；调用方必须传入有效 MySQL 标准码。`code.0 as u16/i32` 也意味着扩展时必须审查负数或超范围转换。
- 未注册 `ErrClass` 的 `String()` 返回数字，但构造 RFC code 时使用 `registeredDescription()`，前缀为空，形成 `":<code>"`。这种错误不能由 `EqualClass`/`GetErrClass` 识别，转 SQL 时回退 `ErrUnknown`。
- RFC code 没有非空冒号前缀、前缀未登记、类别没有 code 集合或 code 未登记时，`getMySQLErrorCode` 均回退 `ErrUnknown`；未知类别记录 warn，未知 code 记录 debug。
- 所有 `RwLock` 获取都使用 `expect`；持锁线程 panic 导致锁中毒后，后续访问也会 panic，而不是恢复或忽略污染。
- `Call` 和 `Log` 是显式吞错辅助函数：只产生日志。它们不适用于需要重试、回滚或向上层返回失败的路径。
- `MustNil` 是进程终止边界；只有错误为 `Some` 才执行清理，并且清理回调 panic 会阻止后续回调及预期的退出流程。

## 并发与资源生命周期

Rust 实现用 `RwLock<HashMap<...>>` 保护类别描述、RFC 反向索引和类别-code 集合，使并发读写本身具备内存安全；`Code2ErrClassMap::Put` 在写锁作用域内原子替换同名前缀。`RegisterFinish` 使用 Release 写、`frozen` 使用 Acquire 读，使观察到冻结状态的线程也能观察初始化阶段此前完成的注册写入。

不过逻辑生命周期仍要求“先注册，后冻结”：调用者应在全局/模块初始化阶段触发所有 `New`/`NewStd*` 模板，再调用 `RegisterFinish`；运行期外部错误使用 `Synthesize`。`ErrCritical` 等 `LazyLock` 若直到冻结后才首次解引用，理论上会进入注册型构造器并 panic；本文件通过预置 global 映射/code 集合保留包初始化语义，相关跨 crate 测试还要求依赖方在冻结前完成其模板初始化。

`LazyLock` 保证每个全局表或模板只初始化一次，锁守卫离开作用域即释放；没有后台任务、通道或显式堆资源回收流程。`MustNil` 是唯一接收资源清理回调的 API，按输入顺序执行，而非栈式逆序。

## 与 Go 版本的对应关系

Rust `ErrCode`、`ErrClass`、27 个类别、四个 code 常量、构造器、类别判断、SQL 转换和日志辅助均逐项对应 `pkg/parser/terror/terror.go`。`pkg/parser/terror/terror_test.rs` 按 Go `terror_test.go` 的测试组覆盖常量、错误生成、JSON、相等性、日志和栈；`migration_aster_unit_test.rs` 额外锁定未注册类别、合成错误与转换边界。

主要实现差异如下：Go 的 `sync.Map` 在 Rust 中是封装 `RwLock<HashMap>` 的 `Code2ErrClassMap`；Go 的普通 map 注册表也在 Rust 中加锁，因此数据竞争被类型系统阻止。Go 包变量逐项调用 `RegisterErrorClass`，Rust 内置类别使用常量加 `ERROR_CLASSES` 一次性初始化，同时仍保留公开的动态注册函数。Go `atomic.Store/LoadUint32` 对应 Rust Release/Acquire 原子操作。Go `debug.PrintStack` 对应 Rust 强制捕获并输出 `Backtrace`。

Go `log.Fatal` 的副作用由 Rust `MustNil` 明确实现为 error 日志、logger flush 和 `process::exit(1)`；Go 的 zap 结构化字段与栈日志在 Rust 中简化成格式化消息，因此日志字段结构并非完全相同。Go `ToSQLError` 返回指针，Rust 返回拥有所有权的 `SQLError` 值。Go 构造 API 使用可变参数，Rust 的共享 errors API 使用参数切片/向量。

## 扩展指南

新增错误类别时，应选择未占用的稳定编号，同时更新类别常量与 `ERROR_CLASSES`；若还需 Go 兼容，必须同步 `terror.go` 的注册顺序、编号和描述，并在 `terror_test.rs::terror_registry_matches_go_classes` 增加断言。只调用 `RegisterErrorClass` 可用于运行期/测试动态类别，但不能替代稳定内置类别定义。

新增本地标准错误时，应在冻结前通过相应 `ErrClass::NewStd` 或 `NewStdErr` 建立模板，并确认 MySQL 消息表存在该 code；同步测试 `ToSQLError` 的 code、消息与 `EqualClass`。来自外部系统且不应污染本地登记表的错误应使用 `Synthesize`，并明确接受未预登记 code 转 SQL 时回退 `ErrUnknown` 的契约。

修改 RFC code 格式、类别描述或注册时序会影响 `EqualClass`、`GetErrClass`、SQL 协议码以及持久化/日志中的错误身份，属于高兼容风险。修改全局表或锁顺序时要避免锁中毒与嵌套锁死；当前 `initError` 先释放 code 表写锁，随后才写反向表，不同时持有两把锁。性能上，构造/转换均访问全局锁；若优化缓存，必须保持注册完成前后的可见性及未知 code 回退语义。

测试应继续放在独立文件 `pkg/parser/terror/terror_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。冻结或进程退出场景应继续使用子进程隔离，避免污染同一测试进程的全局状态。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/terror` 确认本目录六个 Rust/Go 源与测试文件；`node --file pkg/parser/terror/terror.rs --offset 1 --limit 500` 读取了完整 473 行目标源码，并报告 72 个符号及 35 个使用文件。
- 符号查询：`query ToSQLError`、`query GetErrClass`、`query RegisterFinish` 分别区分出 Go 与 Rust 定义；精确 Rust symbol ID 的 `callers/callees` 未返回可用边，因此调用关系又以目标源码内部调用和 `rg` 的直接引用交叉验证，没有采用同名 `Call` 等歧义结果。
- 已读生产与装配文件：`pkg/parser/terror/terror.rs`、`pkg/parser/terror/lib.rs`、`pkg/parser/terror/Cargo.toml`、`pkg/parser/terror/terror.go`；并检查根 `Cargo.toml` 及直接依赖者的 Cargo 声明。
- 已读测试：`pkg/parser/terror/terror_test.rs`、`pkg/parser/terror/migration_aster_unit_test.rs`、`pkg/parser/terror/terror_test.go`。关键断言覆盖 27 类映射、重复类别 panic、注册/合成差异、未知类别/code 回退、冻结后 panic、全局初始化、相等性、日志、栈和 `MustNil` 退出码。
- 直接调用证据：`pkg/parser/yy_parser.rs`、`pkg/parser/parser_semantic_support.rs`、`pkg/parser/charset/charset.rs`、`pkg/parser/charset/encoding_base.rs`、`pkg/server/internal/resultset/resultset.rs`，以及验证 SQL 转换和冻结时序的相邻独立测试。
- 本任务只新增说明文档，按计划不运行 Cargo；最终以固定 11 章节结构命令和人工事实复核验证。
