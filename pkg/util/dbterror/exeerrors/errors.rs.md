# `pkg/util/dbterror/exeerrors/errors.rs`

## 文件定位

本文件是 `astersql-util-dbterror-exeerrors` crate 的执行层错误原型表。crate 入口 `pkg/util/dbterror/exeerrors/lib.rs` 通过 `pub mod exeerrors { include!("errors.rs"); }` 将这里的公开静态项暴露为 `astersql_util_dbterror_exeerrors::exeerrors::*`；`pkg/util/dbterror/exeerrors/Cargo.toml` 又把该 crate 对应到 Go 包 `pkg/util/dbterror/exeerrors`。因此它位于业务代码与底层 `terror` 错误模型之间：调用方选取一个具名错误原型，再通过 `GenWithStackByArgs`、`FastGenByArgs`、`Equal` 或显示格式化生成、比较和传播具体错误。

它不是错误发生条件的判断器，也不负责执行器控制流。比如 `pkg/util/sqlkiller/sqlkiller.rs::getKillError` 判断 kill signal 后选择查询中断、超时、内存或 runaway 原型；`pkg/executor/importer/precheck.rs::CheckImportTableTTL` 和 `validate_global_sort_uri` 判断导入前置条件后分别使用 `ErrLoadDataPreCheckFailed`、`ErrLoadDataInvalidURI`。本文件只固定这些错误的 MySQL 数值码、RFC 错误类和消息模板。

## 核心职责

- 声明 82 个公开的 `LazyLock<Box<terror::Error>>` 错误原型，保留 Go `errors.go` 包级 `var` 的名称、顺序和映射语义。
- 把 errno 常量分配到正确的 `dbterror` 错误类：77 个属于 `ClassExecutor`，3 个属于 `ClassDDL`，`ErrRoleNotGranted` 属于 `ClassPrivilege`，`ErrTruncateWrongInsertValue` 属于 `ClassTable`。
- 让 78 个 `NewStd` 原型从 `errno::MySQLErrName` 继承标准消息，同时用 4 个 `NewStdErr` 原型保留 Go 代码中的定制模板。
- 保留“公开名称与底层 errno 名不同”的兼容映射，例如 `ErrSubqueryMoreThan1Row -> ErrSubqueryNo1Row`、`ErrColumnsNotMatched -> ErrColumnNotMatched`、`ErrDeadlock -> ErrLockDeadlock`、`ErrSavepointNotExists -> ErrSpDoesNotExist`。

## 主要符号

文件没有函数、trait、结构体或 impl；全部业务 API 都是公开静态错误原型。局部模块 `dbterror` 再导出 `ClassDDL`、`ClassExecutor`、`ClassPrivilege`、`ClassTable`，并以 `type Error = Box<crate::terror::Error>` 明确静态项持有的类型。

82 个静态项可按用途理解为几组，而不是 82 条相互独立的流程：

- 执行与预处理：`ErrGetStartTS`、`ErrUnknownPlan`、`ErrPrepareMulti`、`ErrPrepareDDL`、`ErrBuildExecutor`、`ErrSubqueryMoreThan1Row` 等。
- 权限、账号与密码：`ErrIllegalGrantForTable`、`ErrRoleNotGranted`、`ErrPasswordNoMatch`、`ErrMustChangePassword` 以及三项双密码错误；其中 `ErrRoleNotGranted` 的 RFC 类必须是 `privilege`。
- 查询终止与资源控制：`ErrQueryInterrupted`、`ErrMaxExecTimeExceeded`、`ErrMemoryExceedForQuery`、`ErrMemoryExceedForInstance`、`ErrResourceGroupQueryRunawayInterrupted`、`ErrQueryExecStopped`。这些原型被 `sqlkiller.rs::getKillError` 和 `pkg/store/driver/error/error.rs` 的 TiKV 错误转换分支实际使用。
- BR、递归 CTE、插件、savepoint 与外键：`ErrBRIEBackupFailed` 至 `ErrForeignKeyCascadeDepthExceeded` 等。
- DDL/Table 特例：`ErrWrongStringLength`、`ErrUnsupportedFlashbackTmpTable`、`ErrUserNameNeedPrefix` 属于 `ddl`；`ErrTruncateWrongInsertValue` 属于 `table`。
- LOAD DATA / IMPORT INTO：从 `ErrWarnTooFewRecords` 到 `ErrLoadDataPreCheckFailed` 的错误组，以及末尾的 `ErrMaxKeysReadExceeded`。其中多个原型已被 `pkg/executor/importer/precheck.rs`、`pkg/dxf/importinto/task_executor.rs` 和相应独立测试引用。

4 个 `NewStdErr` 项是需要逐字保持模板和类别的特殊 API：`ErrFuncNotEnabled`（executor / `ErrNotSupportedYet`）、`ErrUnsupportedFlashbackTmpTable`（ddl / `ErrUnsupportedDDLOperation`）、`ErrTruncateWrongInsertValue`（table / `ErrTruncatedWrongValue`）、`ErrUserNameNeedPrefix`（ddl / `ErrUsername`）。

## 执行流程

1. `lib.rs` 首次暴露或调用方首次解引用某个静态项时，标准库 `LazyLock` 执行该项的闭包；未使用的原型无需提前构造。
2. 对普通项，闭包调用相应错误类的 `NewStd(errno)`。`pkg/util/dbterror/terror.rs::ErrClass::NewStd` 将 errno 转为 `ErrCode`，从 `MySQLErrName` 取标准消息，再委托 `NewStdErr`。
3. 对 4 个自定义项，闭包直接构造 `parser_mysql::Message(template, &[])` 并调用错误类的 `NewStdErr`，从而绕过 errno 表中的标准文本，但仍保留指定数值码。
4. `NewStd`/`NewStdErr` 返回 boxed `terror::Error`，由 `LazyLock` 缓存为进程生命周期内的错误原型。
5. 上游业务代码通常不会修改原型，而是调用其生成方法。例如 `sqlkiller.rs::getKillError` 用连接 ID 或描述填入消息参数；`precheck.rs::validate_global_sort_uri` 用数据源种类和解析失败原因生成带栈错误；错误转换层也可根据 TiKV signal 选择相应原型。

文件内部没有循环、I/O 或业务分支；有意义的分支发生在“选择哪个静态原型”的上游，或“标准消息/定制消息”的声明方式上。

## 数据与状态

每个静态项保存一个不可变的 `Box<terror::Error>` 原型，核心状态由错误类、MySQL 数值码、RFC code 和未格式化消息模板组成。具体调用参数与栈信息由原型的方法生成到新的错误值中，不写回静态原型，所以本文件没有请求级、会话级或事务级可变状态。

名称不是数值码的来源：兼容别名必须显式映射到底层 errno。例如 `ErrDeadlock` 的 code 来自 `mysql::ErrLockDeadlock`，`ErrSavepointNotExists` 来自 `mysql::ErrSpDoesNotExist`。消息模板中的 `%s`、`%d`、长度修饰符等也是兼容数据；特别是 4 个自定义模板，修改标点或占位符都会改变用户可见错误契约。

## 依赖与调用关系

下游依赖关系为：`errors.rs` 使用 `std::sync::LazyLock` 管理一次性初始化；通过 crate 的 `errno` 模块读取 `astersql-errno` 的错误码和标准消息表；通过 `parser::mysql::errname::Message` 构造定制模板；通过 `astersql-util-dbterror` 再导出的四个错误类调用 `NewStd`/`NewStdErr`；最终对象类型来自 `astersql-parser-terror`。这些边界由 `Cargo.toml` 与 `lib.rs` 的 path 依赖和再导出共同建立。

已核实的 Rust 上游包括：

- `pkg/util/sqlkiller/sqlkiller.rs::getKillError`：把原子 kill signal 映射为查询中断、最大执行时间、查询/实例内存、runaway 与主动停止错误。
- `pkg/store/driver/error/error.rs`：把带 signal 的 TiKV 错误转换为同一组执行层错误，使存储返回路径保持 SQL 层错误码。
- `pkg/executor/importer/precheck.rs`：生成 IMPORT INTO 的 TTL 前置检查失败和云存储 URI 错误。
- `pkg/dxf/importinto/task_executor.rs`：使用 `ErrLoadDataDuplicateKeyConflict` 报告导入数据冲突。
- `pkg/session/runtime/scan_adapter_runtime.rs`：使用 `ErrLazyUniquenessCheckFailure`。
- `pkg/keyspace/username_policy.rs`：使用 `ErrUserNameNeedPrefix` 并填入用户名各组成部分。

RustCodeGraph 报告目标文件被 225 个文件使用，但这包括同路径 Go 文件的广泛引用；当前图索引未把 `pub static` 建成独立符号，因此具体 Rust 调用点以上述 `rg` 结果和逐文件源码为准，不能把“225”解释成 225 个 Rust 调用方。

## 错误处理与边界

本文件定义错误而不捕获错误。构造闭包没有返回 `Result`：若 errno 在 `MySQLErrName` 中缺失，`ErrClass::NewStd` 的索引访问会失败；这应在迁移测试和 errno 表同步中提前发现，而不应在运行时降级成任意文案。

安全边界主要有三类：一是错误类必须正确，否则 `RFCCode()` 的 `executor:`、`ddl:`、`privilege:`、`table:` 前缀会改变；二是兼容别名必须继续指向 Go 使用的 errno；三是参数化模板的占位符顺序必须与所有 `GenWithStackByArgs`/`FastGenByArgs` 调用一致。本文件不校验调用参数个数或业务条件，因此新增/修改模板时必须审查所有调用点。

不同模块可能存在同名但不同错误类的原型，例如存储驱动还定义了 TiKV 类的 `ErrQueryInterrupted`；调用者必须通过 crate/module 路径选择正确原型，不能仅凭名称互换。

## 并发与资源生命周期

`LazyLock` 保证每个错误原型在并发首次访问时只初始化一次，之后多个线程共享只读原型；其生命周期与进程/已加载 crate 一致，没有显式销毁、任务、通道、锁持有区间、文件句柄或网络资源。boxed 值只在一次性初始化时分配。

`pkg/util/dbterror/terror.rs::ErrClass::NewStd` 保留了 Go 版本“通常用于全局初始化、非 goroutine-safe”的注释；本文件通过 Rust 的 `LazyLock` 串行化每个原型的初始化，并且构造后只共享不可变引用。不过该保证仅覆盖这里的静态初始化，不应据此假设错误对象的其他可变扩展或未来构造器天然线程安全。

## 与 Go 版本的对应关系

Go 基准文件是 `pkg/util/dbterror/exeerrors/errors.go`。Rust 保留了全部 82 个变量的公开名称、声明顺序、错误类、errno 映射和 4 个自定义模板。Go 在包初始化时立即执行 `dbterror.Class*.NewStd/NewStdErr` 并得到 `*terror.Error`；Rust 用 `LazyLock<Box<terror::Error>>` 延迟到首次访问，`Box` 对应拥有的指针对象，外部通过解引用使用同一原型。

语义上需要特别关注：Go 的 `parser_mysql.Message(template, nil)` 在 Rust 中写成 `Message(template, &[])`；这是空参数集合的表示差异，不是提前格式化消息。`migration_aster_unit_test.rs` 通过 `Code()`、`RFCCode()`、`GetMsg()` 验证标准项的数值码/类别/标准文案，并逐项验证 4 个自定义模板；该测试是 Rust 移植语义最直接的独立回归依据。

当前 Rust 生产代码对这些原型的接线并不均匀：LOAD DATA、SQL killer、存储错误转换、用户名策略等已有真实消费者，而许多兼容项目前主要用于保持 API 与 Go 的错误目录对齐。不能仅凭声明存在就声称所有 Go 业务路径都已经 Rust 化。

## 扩展指南

新增或调整错误时，应先在 Go `errors.go` 确认对应增量，并同步检查 `pkg/errno/errcode.rs`、`pkg/errno/errname.rs`（以及其 Go 对照）是否已有数值码和标准消息。标准消息使用正确错误类的 `NewStd`；只有 Go 明确使用定制模板时才用 `NewStdErr`，并原样保留占位符、错误类和 errno。若公开 Rust 名称与 errno 名不同，应在文档或测试中显式锁定映射。

测试逻辑应继续放在独立的 `pkg/util/dbterror/exeerrors/migration_aster_unit_test.rs`，不要内嵌进本生产文件。至少增加 `Code()`、`RFCCode()`、`GetMsg()` 断言；参数化模板还应检查代表性调用方的参数顺序和生成结果。若新错误用于具体子系统，也应扩展该子系统已有的独立测试，例如导入前置检查对应 `pkg/executor/importer/precheck_test.rs`，用户名规则对应 `pkg/keyspace/migration_aster_unit_test.rs`/`keyspace_test.rs`，signal 映射对应 `pkg/store/driver/error/migration_aster_unit_test.rs`。

兼容性风险高于性能风险：错误码、RFC 类别和文本会被客户端、重试/分类逻辑及测试识别；错误类或模板变化可能破坏 `Equal`、下游诊断和 MySQL 兼容性。性能方面，每项仅一次惰性分配，除非新增构造器执行昂贵工作，否则影响很小。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/dbterror/exeerrors` 找到 `errors.rs`、`lib.rs`、`errors.go`、`migration_aster_unit_test.rs`；`node --file` 完整读取这四个文件及代表性调用点。对 `ErrGetStartTS`、`ErrFuncNotEnabled` 执行 `query/callers/callees` 时，图未建立静态项定义，因此改用 `rg` 补充调用证据。
- 源与边界：`pkg/util/dbterror/exeerrors/errors.rs`（82 个静态原型）、`lib.rs`（`include!` 和公开模块）、`Cargo.toml`（crate 依赖和 Go 包映射）、`pkg/util/dbterror/terror.rs::ErrClass::{NewStd, NewStdErr}`（标准消息查表与底层构造）。
- Go 对照与测试：`pkg/util/dbterror/exeerrors/errors.go`；`pkg/util/dbterror/exeerrors/migration_aster_unit_test.rs::{standard_errors_preserve_go_codes_and_classes, custom_errors_preserve_go_templates_and_classes, all_go_error_instances_initialize_with_their_standard_message}`。
- 生产调用与相关独立测试：`pkg/util/sqlkiller/sqlkiller.rs::getKillError`、`pkg/store/driver/error/error.rs`、`pkg/store/driver/error/migration_aster_unit_test.rs`、`pkg/executor/importer/precheck.rs`、`pkg/executor/importer/precheck_test.rs`、`pkg/keyspace/username_policy.rs`、`pkg/keyspace/migration_aster_unit_test.rs`、`pkg/dxf/importinto/task_executor.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付仅执行任务规定的 11 章节结构检查，并人工复核文档没有把声明存在误写成所有业务路径均已接线。
