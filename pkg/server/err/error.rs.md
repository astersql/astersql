# `pkg/server/err/error.rs`

## 文件定位

本文件属于 `astersql-server-err` crate（见 `pkg/server/err/Cargo.toml`），是 Server 层标准错误到 `errno` 的集中绑定表。crate 根模块以 `#[path = "error.rs"] pub mod server_err` 暴露这些错误，并通过平台启动段中的 `SERVER_ERR_PACKAGE_INIT` 在程序或测试主体运行前调用 `initialize_server_errors`。它对应 Go 文件 `pkg/server/err/error.go`，不负责判断何时发生错误，而是为连接、认证、协议与服务器生命周期代码提供统一、可格式化的 `terror::Error` 模板。

## 核心职责

1. `server_error!` 宏以同名的公开静态项和 `errno` 常量为输入，延迟构造 `dbterror::ClassServer.NewStd(errno::<code>)`，从而把 Server 错误类、数字错误码、RFC code 和标准消息模板绑定在一起。
2. 文件声明 15 个 `LazyLock<dbterror::Error>`，覆盖类型/调用序列校验、认证与连接限制、安全传输、多语句策略、协议包大小、密码更新和服务器关闭等 Server 边界。
3. `initialize_server_errors` 按 Go 包变量的声明顺序强制初始化全部静态项，使注册型构造在 `terror::RegisterFinish` 冻结注册表之前完成。

该文件因此是“错误定义与注册”层，而不是错误恢复层：调用方仍负责选择错误、填充参数、附加栈或将其转换为协议响应。

## 主要符号

- `server_error!($name, $code)`：模块私有声明宏。每次展开生成一个 `pub static $name: LazyLock<dbterror::Error>`；初始化闭包调用 `dbterror::ClassServer.NewStd(errno::$code)`。
- `ErrInvalidType`、`ErrInvalidSequence`、`ErrNotAllowedCommand`：分别表示值类型不符合预期、协议/方法调用顺序非法，以及当前协议环境不允许某命令。
- `ErrAccessDenied`、`ErrAccessDeniedNoPassword`：两个独立的认证失败模板。二者不能合并；错误码、RFC code 和消息模板均不同。
- `ErrConCount`、`ErrTooManyUserConnections`：分别表示服务器总连接数上限和单用户连接数上限。
- `ErrSecureTransportRequired`、`ErrUserPrefixMismatch`、`ErrMultiStatementDisabled`：连接策略错误，分别对应必须使用安全传输、用户名与 keyspace 前缀不匹配及客户端多语句能力被禁用。
- `ErrNewAbortingConnection`、`ErrNotSupportedAuthMode`：连接建立阶段的中止与认证协议不兼容。
- `ErrNetPacketTooLarge`、`ErrMustChangePassword`、`ErrServerShutdown`：协议包越限、密码必须更新和服务器关闭状态。
- `initialize_server_errors()`：crate 内可见的预热函数，依次对上述 15 个 `LazyLock` 调用 `LazyLock::force`。

文件没有自定义结构体、枚举、trait、条件编译分支或可变模块状态；宏本身也不导出到 crate 外。

## 执行流程

1. 链接 `astersql-server-err` 时，`pkg/server/err/lib.rs` 将 `SERVER_ERR_PACKAGE_INIT` 放入 Unix、macOS 或 Windows 对应的初始化段。
2. 运行时进入该初始化函数，调用 `server_err::initialize_server_errors()`。
3. `initialize_server_errors` 按 `pkg/server/err/error.go` 的变量顺序逐个 `force` 静态项。每项只在首次 force 时执行宏生成的闭包。
4. 闭包把对应 `errno` 常量交给 `dbterror::ClassServer.NewStd`；后者从 `errno::MySQLErrName` 取标准消息，并经 `NewStdErr` 构造带 MySQL code、RFC code（形如 `server:<code>`）和脱敏参数位置的规范化错误，同时登记错误类与错误码关系。
5. 正常请求路径只读取已初始化的静态模板。例如 `pkg/server/conn.rs` 和 `pkg/server/conn_stmt.rs` 在 `ConnError::NetPacketTooLarge` 或预处理语句包过大时格式化 `ErrNetPacketTooLarge`；`pkg/server/internal/column/column.rs::DumpTextRow` 在文本行编码遇到不支持类型时由 `ErrInvalidType.GenWithStack` 生成具体错误。

## 数据与状态

每个公开静态项保存一个 `LazyLock<Box<terror::Error>>`（`dbterror::Error` 是 `Box<crate::terror::Error>` 的别名）。模板自身携带标准消息以及 MySQL/RFC 元数据；调用方使用 `FastGenByArgs`、`GenWithStackByArgs` 或 `GenWithStack` 生成带现场参数的错误，而不是修改模板。

本文件的唯一生命周期状态是 15 个 `LazyLock` 的一次性初始化状态。错误码和消息数据来自 `astersql-errno`，错误类别与注册表来自 `astersql-util-dbterror`/`astersql-parser-terror`；本文件不复制这些表，也没有请求级缓存、连接状态或配置状态。

## 依赖与调用关系

- crate 边界：`pkg/server/err/Cargo.toml` 将本 crate 命名为 `astersql-server-err`，直接依赖 `astersql-errno`、`astersql-errors`、`astersql-parser-mysql`、`astersql-parser-terror` 和 `astersql-util-dbterror`。本文件直接使用由 `lib.rs` 再导出的 `dbterror` 与 `errno`。
- 上游初始化：`pkg/server/err/lib.rs::SERVER_ERR_PACKAGE_INIT` 调用 `initialize_server_errors`。RustCodeGraph 能定位该函数，但精确 `callers` 未返回边，故此边以 `lib.rs` 的直接调用为依据。
- 下游构造：宏展开最终进入 `pkg/util/dbterror/terror.rs::ErrClass::NewStd`，再进入 `pkg/parser/terror/terror.rs::ErrClass::NewStdErr` 和注册逻辑。
- Rust 业务使用：当前直接可见的生产调用包括 `pkg/server/conn.rs`、`pkg/server/conn_stmt.rs` 对 `ErrNetPacketTooLarge` 的格式化，以及 `pkg/server/internal/column/column.rs` 对 `ErrInvalidType` 的具体化。并非所有 15 个模板都已在 Rust Server 主链中直接引用；未发现引用不能解释成已接线。
- Go 主链对照：`pkg/server/conn.go` 使用认证、连接策略、多语句、关闭和命令限制错误；`pkg/server/user_connections.go` 使用连接数错误；`pkg/server/driver_tidb.go` 使用包大小和改密错误；`pkg/server/internal/packetio.go` 使用序号与包大小错误。

## 错误处理与边界

`NewStd` 要求传入的错误码存在于标准消息表；缺失映射会在索引/查表处失败，而不是生成无消息的占位错误。构造过程会注册错误码，因此必须发生在全局注册阶段：`pkg/parser/terror/terror.rs::RegisterFinish` 之后再首次初始化此类静态项会 panic。启动段预热和 `migration_aster_unit_test.rs::server_errors_are_registered_before_registry_freeze` 专门保护这一不变量。

错误模板与错误实例须区分：本文件只提供模板，具体参数数量和语义由对应消息模板及调用点约束。尤其 `ErrAccessDenied` 与 `ErrAccessDeniedNoPassword` 虽属同一场景族，却是不同标准错误；迁移测试明确断言其 code、RFC code 和消息均不相同。

本文件不捕获 I/O、认证或协议错误，也不决定 MySQL wire response；这些属于上游业务代码和协议转换层。扩展时也不应在此加入恢复、副作用或日志逻辑。

## 并发与资源生命周期

`LazyLock` 保证每个模板至多构造一次，并允许初始化完成后由并发调用者共享不可变模板。真正需要遵守的是进程级阶段顺序：先初始化并注册全部标准错误，再冻结注册表，最后进入并发请求处理。`RegisterFinish` 使用 Release 写、注册表冻结检查使用 Acquire 读；启动段预热避免请求线程在冻结后竞争首次初始化。

文件不创建线程、任务、锁、通道、事务、网络连接或需显式释放的资源。模板存活期等同于进程；`Box<terror::Error>` 由静态 `LazyLock` 持有，不存在请求结束时的清理步骤。

## 与 Go 版本的对应关系

`pkg/server/err/error.go` 以包级 `var` 顺序立即执行 15 次 `dbterror.ClassServer.NewStd(errno.<name>)`；Rust 文件保留相同名称、顺序、错误类和 errno 映射。差异仅在初始化机制：Go 由语言保证包变量在导入时完成，Rust 使用 `LazyLock` 表达静态所有权，再由 `lib.rs` 的平台初始化段显式 force，以恢复 Go 的“冻结前已经注册”语义。

`pkg/server/err/migration_aster_unit_test.rs` 是最直接的 Rust/Go 对照测试：`server_errors_keep_go_class_code_and_standard_message_bindings` 逐项验证全部 15 个错误；`access_denied_variants_remain_distinct_go_errors` 验证访问拒绝变体；`server_errors_are_registered_before_registry_freeze` 以子进程隔离不可逆的注册表冻结并验证初始化时序。Go 侧实际调用点还表明这些模板覆盖连接建立、认证、命令分派、包读取、用户连接限制和服务器关闭等路径；Rust 当前直接接线只是其中一部分。

## 扩展指南

新增 Server 标准错误时，应同时完成以下局部改动：

1. 确认 `pkg/errno` 中已有正确的数字常量与标准消息；若是 Go 对齐任务，先核对 `pkg/server/err/error.go` 的增量和声明顺序。
2. 在本文件用 `server_error!` 新增同名静态项，并在 `initialize_server_errors` 的对应顺序位置增加 `LazyLock::force`。遗漏 force 会让冻结后的首次访问 panic。
3. 在独立测试文件 `pkg/server/err/migration_aster_unit_test.rs` 增加 `assert_standard` 断言；若存在相近变体，额外断言 code、RFC code 或模板的区分不变量。不要把测试内嵌进 `error.rs`。
4. 在实际 Server 调用点选择合适的生成方法并提供与消息模板匹配的参数；不要为复用方便把不同 errno 合并成一个模板。
5. 兼容风险主要是错误码、SQLSTATE/RFC code、消息文字及参数格式发生漂移；并发风险主要是未预热导致冻结后首次注册。正常读取静态模板的性能成本只有一次性初始化和共享解引用，不应在请求热路径重复构造标准模板。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter pkg/server/err` 确认 `error.rs`、`lib.rs` 和独立迁移测试均已索引；`node --file pkg/server/err/error.rs --offset 1 --limit 240` 读取了完整 86 行源码；`query initialize_server_errors --kind function` 唯一定位到本文件；`callees` 未发现可用边，精确 `callers` 也未返回边，因此调用关系以直接源码引用补证。
- 已读实现与配置：`pkg/server/err/error.rs`、`pkg/server/err/lib.rs`、`pkg/server/err/Cargo.toml`、`pkg/util/dbterror/terror.rs`、`pkg/parser/terror/terror.rs`。
- 已读 Go 对照与调用证据：`pkg/server/err/error.go`、`pkg/server/conn.go`、`pkg/server/user_connections.go`、`pkg/server/driver_tidb.go`、`pkg/server/internal/packetio.go`。
- 已读 Rust 调用与测试证据：`pkg/server/conn.rs`、`pkg/server/conn_stmt.rs`、`pkg/server/internal/column/column.rs`、`pkg/server/err/migration_aster_unit_test.rs`。同目录没有 `error_test.rs`；迁移测试是该 crate 的独立测试模块。
- 本任务只增加说明文档，未运行 Cargo。结构验证要求文档存在且恰有“文件定位”至“验证依据”11 个固定二级章节；交付前另行检查 Git 范围，确保未修改 Rust、Go、Cargo 或只读的 `plan.md`。
