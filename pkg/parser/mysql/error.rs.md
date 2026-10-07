# [`pkg/parser/mysql/error.rs`](./error.rs)

## 文件定位

`error.rs` 属于 `astersql-parser-mysql` crate，由 `pkg/parser/mysql/lib.rs` 以 `pub mod error` 对外暴露。它位于解析器与 MySQL 协议语义的错误边界：将数字错误码、SQLSTATE、消息模板和参数组装为可展示的 `SQLError`。它只构造内存对象，不解析 SQL，不编码 MySQL 网络包，也不访问存储。依据是 `pkg/parser/mysql/error.rs:13-21` 的模块说明与导入，以及 `pkg/parser/mysql/lib.rs` 的 crate 入口。

`pkg/parser/mysql/Cargo.toml` 定义该 crate，其直接错误基础设施依赖是路径 crate `astersql-errors` 。`lib.rs` 将它转发为 `crate::errors`，因此本文件的 `ErrorArg`、`Errorf` 和 `RedactErrorArg` 实际来自 `pkg/errors`。

## 核心职责

1. `NewErr` 用错误码查找默认 SQLSTATE 和 `MySQLErrName` 消息模板，并在格式化前处理模板声明的敏感参数位置（`error.rs:64-82`）。
2. `NewErrf` 保留相同的 SQLSTATE 选择规则，但消息模板与脱敏位置由调用方提供（`error.rs:86-105`）。
3. `SQLError::Error` 及 `Display` 将结果统一呈现为 `ERROR <code> (<state>): <message>`，`std::error::Error` 实现使它能进入 Rust 错误链（`error.rs:47-60`）。
4. `ErrBadConn` 与 `ErrMalformPacket` 保留 Go 包的两个通用错误文本；它们每次返回新的 `std::io::Error`，不负责连接状态判定或报文校验（`error.rs:26-34`）。

## 主要符号

- `pub type Arg = ErrorArg`：面向本 crate 的参数别名。`ErrorArg` 保留字符串、布尔、有/无符号整数、浮点和调试文本等类型，以支持 Go `any` 的格式化差异（`error.rs:23-24`，`pkg/errors/core.rs:92-176`）。
- `pub fn ErrBadConn() -> std::io::Error` 和 `pub fn ErrMalformPacket() -> std::io::Error`：构造 `ErrorKind::Other` 的固定文本 I/O 错误。
- `pub struct SQLError { pub Code: u16, pub Message: String, pub State: String }`：拥有完整展示数据的错误值；字段公开，下游可直接读取或修改 `Message`。
- `pub fn NewErr(err_code: u16, args: Vec<Arg>) -> SQLError`：使用默认消息元数据的构造入口。
- `pub fn NewErrf(err_code: u16, format_text: &str, redact_arg_pos: &[usize], args: Vec<Arg>) -> SQLError`：使用调用方模板的构造入口。
- `fn format_string_args(...) -> String`：私有共享路径，先原地脱敏已拥有的参数向量，再调用 `Errorf`（`error.rs:107-112`）。
- `fn sprint_args(args: &[Arg]) -> String`：未知错误码时的 `fmt.Sprint(args...)` 对照：逐项用 `%v` 展示并无分隔符连接（`error.rs:114-119`）。

## 执行流程

`NewErr` 的主流程是：

1. 调用 `state::MySQLState()` 生成错误码到静态字符串的映射；找到时拷贝对应值到拥有的 `String`，否则使用 `DefaultMySQLState`（`HY000`）。
2. 查询 `errname::MySQLErrName()`。已知错误码取得 `ErrMessage.Raw` 和零起点 `RedactArgPos`，进入 `format_string_args`；未知码不假设模板，直接进入 `sprint_args`。
3. `format_string_args` 先根据全局脱敏模式改写指定参数，再让 `Errorf` 解释 `%s`、`%d`、`%v`、`%%` 和字符串精度。这个顺序保证原始敏感值不会先被写入最终 `Message`。
4. 构造拥有 `Code`、`Message` 和 `State` 的 `SQLError` 值并返回。

`NewErrf` 执行同样的 SQLSTATE 查找，随后无条件用调用方提供的模板和脱敏下标进入 `format_string_args`。RustCodeGraph 已确认内部边 `NewErr -> format_string_args`、`NewErr -> sprint_args` 以及 `NewErrf -> format_string_args`。

## 数据与状态

`SQLError` 是拥有型快照：构造后不借用错误表或调用参数。`NewErr`/`NewErrf` 都将 SQLSTATE 复制到 `String`，消息也在返回前完成格式化。`NewErr` 接收参数所有权，使脱敏可以安全地原地替换元素。

`MySQLErrName()` 由 `pkg/parser/mysql/errname.rs` 的 `LazyLock<HashMap<u16, ErrMessage>>` 惰性构建并共享；`MySQLState()` 则每次返回新的 `HashMap`（`pkg/parser/mysql/state.rs`）。本文件自身没有静态可变数据，但 `RedactErrorArg` 会读取 `pkg/errors/normalize.rs` 中进程级 `RedactLogEnabled`，所以相同输入的 `Message` 会随 OFF、ON 或 MARKER 模式改变。

## 依赖与调用关系

直接下游依赖为：

- `super::state::{MySQLState, DefaultMySQLState}`：提供专用 SQLSTATE 与 `HY000` 回退。
- `super::errname::MySQLErrName`：提供默认英文模板和敏感参数下标。
- `crate::errors::{ErrorArg, RedactErrorArg, Errorf}`：保留参数类型，应用脱敏策略，并执行 Go 风格格式化。
- 标准库 `std::io::Error`、`std::fmt::Display` 和 `std::error::Error`：提供通用 Rust 错误互操作。

已验证的 Rust 上游包括 `pkg/parser/mysql/const.rs` 中的 `newInvalidModeErr`、`newVersionErr` 和 `PriorityEnum::Restore`，以及 `pkg/parser/terror/terror.rs:390-397` 的 `ToSQLError`。后者是更完整应用错误链的关键桥接：将已注册 terror 错误码和消息转换为 `SQLError`。`pkg/expression/util.rs:1924,1942` 使用 `ErrMalformPacket()` 的文本包装表达式协议解码错误。直接下游还可以读取/修改结果，例如 `pkg/util/errmsg/errmsg.rs` 对 `SQLError.Message` 附加说明。

## 错误处理与边界

- 错误码未出现在 `MySQLState()` 时，状态稳定回退为 `HY000`；未出现在 `MySQLErrName()` 时，`NewErr` 按 `%v` 连接所有参数，不生成占位模板。
- `RedactErrorArg` 通过 `get_mut` 处理下标，越界脱敏位置被安全忽略；脱敏关闭时参数保持原值。
- 格式化器只明确支持 `%s`、`%d`、`%v`、`%%` 和已实现的精度形式；类型与动词不匹配时产生 Go 风格 `%!d(type=value)` 诊断，而不是返回第二个错误。`error_test.rs` 以整数 `7` 和字符串 `"7"` 验证此边界。
- `ErrBadConn` 和 `ErrMalformPacket` 是函数而非可比较身份的全局单例；调用方不应依赖指针/对象身份。
- `SQLError::Error` 始终包含码、状态和消息；`Display` 直接委托它，不另行脱敏。因此敏感数据必须在构造阶段正确标注。

## 并发与资源生命周期

本文件不启动线程/任务，不使用锁、通道、事务或 I/O 句柄。`SQLError` 和两个 `std::io::Error` 都由调用方拥有，按 Rust 普通 RAII 在离开作用域时释放。`MySQLErrName` 的 `LazyLock` 跨线程共享只读映射。

唯一的并发可见状态来自 `RedactLogEnabled`。`RedactErrorArg` 在一次调用开始时读取当前模式，所以单次参数处理不会在各下标间切换模式；但修改模式会影响其他同期构造器。涉及模式切换的测试应串行化并恢复原值；`error_3_aster_unit_test.rs` 已保存并恢复模式，但它本身未为并行测试加锁，这是扩展此类用例时要注意的测试隔离边界。

## 与 Go 版本的对应关系

Go 权威对照是 `pkg/parser/mysql/error.go`。两个版本共享以下语义：两个固定连接/报文错误文本；`SQLError` 的三字段和展示形状；专用 SQLSTATE 查找及默认回退；`NewErr` 的默认模板/未知码 `Sprint` 分支；以及 `NewErrf` 的自定义模板和格式化前脱敏。

明确的语言适配差异为：

- Go 的 `ErrBadConn`/`ErrMalformPacket` 是包级错误值，Rust 是每次构造 `std::io::Error` 的函数。
- Go 构造器返回 `*SQLError` 并接收 `args ...any`；Rust 返回拥有的 `SQLError` 并接收 `Vec<ErrorArg>`，用显式枚举保留常用运行时类型。
- Go 使用 `fmt.Sprintf`/`fmt.Sprint`；Rust 委托 `astersql-errors::Errorf`，只能承诺该共享格式化器已实现的 Go 格式子集。
- Go `MySQLState` 是全局 map；当前 Rust `MySQLState()` 每次构建 `HashMap`。这是性能/实现差异，不改变本文件观察到的查找结果。

`pkg/parser/mysql/error_test.go` 与 `error_test.rs` 都覆盖已知/未知错误码和默认/自定义模板入口；Rust 独立测试另外锁定了 `%d` 的参数类型诊断，`error_3_aster_unit_test.rs` 进一步验证 SQLSTATE、固定文本、OFF/ON/MARKER 脱敏和 Unicode 精度。

## 扩展指南

- 新增标准 MySQL/TiDB 错误时，不应只修改本文件：错误码放在 `errcode.rs`，默认消息与脱敏下标放在 `errname.rs`，专用 SQLSTATE 放在 `state.rs`；同时与对应 Go 表格核对。
- 扩展格式动词或脱敏语义时，真实实现点是 `pkg/errors/core.rs::Errorf`/`format_message` 与 `pkg/errors/normalize.rs::RedactErrorArg`；本文件只负责正确的调用顺序。
- 若增加构造分支，应保持不变量：未知 SQLSTATE 回退 `HY000`，脱敏先于格式化，`Display` 与 `Error()` 完全一致，已知默认模板不走未知码拼接路径。
- 与本文件对应的 Rust 测试必须继续放在独立文件：首选扩展 `pkg/parser/mysql/error_test.rs`；跨模块脱敏/元数据回归可扩展 `pkg/parser/mysql/error_3_aster_unit_test.rs`，不得将 `#[cfg(test)]` 测试内嵌到 `error.rs`。
- 保持 Go 对齐时应同步检查 `pkg/parser/mysql/error.go` 和 `error_test.go`。格式化扩展还需加入类型错配、缺失/多余参数、精度、Unicode、脱敏下标越界及并发切换模式等边界用例。
- 如需优化热路径，可评估将 `MySQLState()` 改为共享惰性映射，但这属于 `state.rs` 的行为/性能变更，必须单独验证，不是本文档任务的实现内容。

## 验证依据

- 目标源文件：`pkg/parser/mysql/error.rs`，已核对全部别名、函数、结构体和 trait impl；文件无条件编译项。
- crate 边界：`pkg/parser/mysql/Cargo.toml` 与 `pkg/parser/mysql/lib.rs`，已确认 `astersql-parser-mysql` 及 `astersql-errors` 路径依赖/转发。
- 元数据与格式化依赖：`pkg/parser/mysql/state.rs`、`pkg/parser/mysql/errname.rs`、`pkg/errors/core.rs`、`pkg/errors/normalize.rs`。
- Rust 上游调用：`pkg/parser/mysql/const.rs`、`pkg/parser/terror/terror.rs`、`pkg/expression/util.rs`、`pkg/util/errmsg/errmsg.rs`；此外用 `rg` 核对了 `pkg` 下 `NewErr`/`NewErrf`/`ErrMalformPacket`/`SQLError` 的引用。
- Go 对照：`pkg/parser/mysql/error.go` 和 `pkg/parser/mysql/error_test.go`。
- Rust 独立测试：`pkg/parser/mysql/error_test.rs`、`pkg/parser/mysql/error_3_aster_unit_test.rs`、`pkg/parser/mysql/unit_test.rs`；间接转换验证参考 `pkg/types/errors_test.rs`。本任务按计划不运行 Cargo。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/mysql` 确认 Rust/Go 目标与测试在索引中；`explore` 确认 `NewErr -> format_string_args`、`NewErr -> sprint_args`、`NewErrf -> format_string_args` 三条内部调用边，并识别 `const.rs`、`terror.rs` 及测试调用者。精确 ID 的 `node/callers/callees` 命令将 `NewErr` ID 误解析到 `pkg/parser/ast/base.rs::functionExpression`，因此该部分索引结果不作为证据，改用精确 `rg` 与直接源码读取补齐。
- 文档结构最终使用任务指定命令校验，要求目标文件存在且恰好命中 11 个固定二级标题。
