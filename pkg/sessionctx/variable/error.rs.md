# `pkg/sessionctx/variable/error.rs`

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate 的系统变量错误定义层。crate 入口 [`lib.rs`](lib.rs) 以 `pub mod error` 暴露整个模块，并额外在 crate 根再导出 `ErrIncorrectScope` 与 `ErrUnknownSystemVar`；[`Cargo.toml`](Cargo.toml) 指定该 crate 的库入口为 `lib.rs`，且没有为本文件配置条件 feature。它不负责读取或修改系统变量，而是为变量校验、next-gen 限制、GC 快照检查和密码策略等调用点提供稳定的错误分类、MySQL/TiDB errno 与消息模板。

当前 Rust 实现是 Go [`error.go`](error.go) 中 `dbterror` 错误表的轻量移植。它保留协议可见的 class、code 和消息文本，但没有移植 Go `*terror.Error` 的完整错误链、堆栈捕获、SQL error 转换与 `Equal` 等行为；调用方需要自行把描述符格式化或封装成自身的错误类型。

## 核心职责

1. `ErrorClass` 区分变量错误与执行器错误，保持 Go 侧 `dbterror.ClassVariable` / `ClassExecutor` 的分类信息。
2. `ErrorDescriptor` 将 `class`、`u16` 错误码和静态消息模板绑定为可复制的只读描述符。
3. `ErrorDescriptor::format` 实现本表所需的有限 printf 风格替换，使 `'%-.64s'`、`%s`、`%d` 与 `%%` 能生成用户可见文本。
4. 24 个静态描述符构成变量子系统的 errno 表；`ALL_ERRORS` 为枚举和一致性测试提供统一入口。

本文件的职责止于“描述和格式化”。它不创建 Rust `std::error::Error`，不决定警告还是失败，不捕获调用栈，也不执行变量作用域或取值校验。

## 主要符号

- `pub enum ErrorClass { Variable, Executor }`：稳定分类枚举。24 个描述符中只有 `ErrNotValidPassword` 使用 `Executor`，其余条目使用 `Variable`。
- `pub struct ErrorDescriptor`：三个公开字段分别是 `class: ErrorClass`、`code: u16`、`message: &'static str`。派生的 `Clone + Copy + Eq` 表明它是无所有权资源的值对象。
- `ErrorDescriptor::new(...) -> Self`：`const fn`，允许全部错误描述符在静态初始化期构造。
- `ErrorDescriptor::format(&self, args: &[&str]) -> String`：唯一运行时逻辑。按模板从左到右消费参数并返回新字符串。
- `VARIABLE`、`EXECUTOR`：私有分类别名，仅用于缩短静态表初始化。
- 公开描述符：包括 `ErrSnapshotTooOld`、`ErrUnknownSystemVar`、`ErrIncorrectScope`、`ErrWrongValueForVar`、`ErrUnsupportedIsolationLevel`、`ErrNotSupportedInNextGen`、`ErrNotValidPassword` 等。小写条目（如 `errGlobalVariable`）只在 crate 内可见。
- `ALL_ERRORS: &[&ErrorDescriptor]`：按声明顺序保存全部 24 个描述符引用；它不是按 code 去重的映射，因此 code `1193`、`1235`、`1681` 可由多个语义条目共享。

## 执行流程

`ErrorDescriptor::format` 的流程如下：

1. 以模板长度加少量余量创建输出字符串，并用字节索引扫描下一个 `%`；普通片段原样追加。
2. 若遇到 `%%`，追加一个字面 `%`，且不消费参数。
3. 否则继续扫描到下一个 `s` 或 `d`，把这段视为占位符。存在对应参数时，`%s` 追加字符串，`%d` 也直接追加调用方传入的字符串。
4. 对字符串占位符，若 `%` 与 `s` 之间含可解析的 `.N`，使用 `value.chars().take(N)` 截断；因此 `'%-.64s'` 按 Unicode 字符数而不是 UTF-8 字节数截断。
5. 参数缺失时保留原占位符文本；无终止 `s`/`d` 的孤立 `%` 作为字面 `%` 输出。成功处理一个占位符后，即使参数缺失也会推进参数序号。
6. 模板扫描完毕即返回字符串；多余参数不会被读取。

典型生产链是：调用方选择静态描述符，调用 `format` 生成文本，再组装本模块之外的业务错误。例如 [`nextgen.rs`](nextgen.rs) 的 `NextGenError::unsupported` 与 `wrong_value` 分别保存描述符引用并格式化消息；[`variable.rs`](variable.rs) 使用 `errGlobalVariable.format` 拒绝错误作用域。

## 数据与状态

所有描述符及其消息模板均为进程期静态只读数据。`ErrorDescriptor` 只含一个枚举、一个 `u16` 和一个 `&'static str`，没有堆上可变状态；`ALL_ERRORS` 只保存静态引用。

每次 `format` 调用只分配并返回自己的 `String`，扫描位置、参数位置和输出缓冲都位于调用栈/本次分配中，不回写描述符。错误码是协议兼容数据而非本地序号：例如未知系统变量为 `1193`、错误变量值为 `1231`、密码策略错误为 `1819`。相同 errno 可对应多个更具体的消息模板，因此不能用 code 唯一反查描述符。

## 依赖与调用关系

本文件只使用 Rust 标准库的字符串、切片和派生 trait，不直接依赖 [`Cargo.toml`](Cargo.toml) 中列出的外部 crate。crate 边界由 [`lib.rs`](lib.rs) 建立：内部模块可访问全部 `pub` 与 crate 可见条目，外部 crate 可通过 `astersql_sessionctx_variable::error::*` 访问公开项。

已核实的 Rust 生产使用点包括：

- [`variable.rs`](variable.rs) 的作用域校验使用 `errGlobalVariable.format`。
- [`nextgen.rs`](nextgen.rs) 的 `NextGenError` 保存 `&'static ErrorDescriptor`，使用 `ErrNotSupportedInNextGen` 和 `ErrWrongValueForVar`。
- [`../../session/runtime/session.rs`](../../session/runtime/session.rs) 读取 `errGlobalVariable.code`，把协议错误码带入会话运行时错误。
- [`../../util/gcutil/gcutil.rs`](../../util/gcutil/gcutil.rs) 使用 `ErrSnapshotTooOld` 描述 GC safe point 之前的快照错误。
- [`../../util/password-validation/password_validation.rs`](../../util/password-validation/password_validation.rs) 使用 `ErrNotValidPassword` 描述密码策略失败。

RustCodeGraph 将本文件标为被 34 个文件使用；精确 `callers` 命令在本次验证窗口内未完成，因此上述边以索引文件概览和限定范围文本搜索交叉确认，不把未逐项确认的 34 个文件都宣称为运行时调用者。

## 错误处理与边界

`format` 是总函数：它不返回 `Result`，对参数不足、额外参数、未知格式片段或孤立 `%` 均采取保守文本输出，而不会 panic。已验证的明确边界包括：额外参数被忽略；缺少参数时保留相应占位符；`%%` 不消费参数；精度按 Unicode 标量值计数。

该格式化器不是通用 `printf` 实现。它只识别最终以 ASCII `s` 或 `d` 结尾的形式；`%d` 不解析或验证数字；精度只作用于 `%s`；扫描器也不会验证 flag/width 的合法性。新增模板若需要其他动词、动态宽度或 Go `fmt` 的完整行为，不能假设当前实现自动兼容。

描述符本身也不是可抛出的错误对象。Rust 侧当前没有在此实现 `Display`、`std::error::Error`、错误链或栈信息；若调用链需要这些能力，应在调用方错误类型中明确包装，不能仅返回一个 `ErrorDescriptor` 就宣称等价于 Go `dbterror`。

## 并发与资源生命周期

本文件没有锁、原子量、通道、异步任务、事务或外部资源。静态描述符在程序整个生命周期内有效，`&'static ErrorDescriptor` 可被多个线程只读共享；派生的 `Copy` 不会复制消息内容。

`format` 的唯一拥有资源是新建的 `String`，所有权随返回值交给调用方，局部扫描状态在返回时释放。由于没有共享可变状态，并发调用之间互不影响；并发安全来自不可变数据与每次调用独立分配，而不是显式同步。

## 与 Go 版本的对应关系

Go [`error.go`](error.go) 通过 `dbterror.ClassVariable.NewStd`、`NewStdErr` 和 `ClassExecutor.NewStd` 从 `pkg/errno` 及 MySQL 消息表构造错误。Rust 将这套间接注册展开为显式的 class、数值 code 和消息模板，24 个条目的名称与顺序和 Go 表对应。

重要差异如下：

- Go 错误对象支持 `GenWithStackByArgs`、`FastGenByArgs`、`Equal`、SQL error 转换及错误链；Rust `ErrorDescriptor` 只保存元数据并格式化字符串。
- Go 的标准消息由中央 errno/message 注册表提供；Rust 在本文件内复制了数值和文本。因此上游 Go 消息或 errno 变化时，Rust 不会自动同步。
- Go 注释说明 `ErrFunctionsNoopImpl` 从 expression 包复制以规避循环依赖；Rust 同样在变量 crate 内独立定义该条目。另一个 Rust 文件 [`../../expression/errors.rs`](../../expression/errors.rs) 也有自己的同名错误，扩展时需避免两处文本漂移。
- Go 测试 `TestError` 重点确认若干条目能转换成非 unknown MySQL code；Rust 的独立测试以 `ALL_ERRORS` 非空且 code 非零覆盖相近不变量，但没有证明完整 Go `terror.ToSQLError` 转换语义。

## 扩展指南

新增或调整错误时，应同步完成以下工作：

1. 先核对 Go [`error.go`](error.go)、对应 `pkg/errno` 常量及中央消息模板，确定 `ErrorClass`、code 和精确文本；不要自行分配 errno。
2. 在本文件新增或修改 `ErrorDescriptor`，并把新条目加入 `ALL_ERRORS`。若要从 crate 根直接访问，再评估是否需要修改 [`lib.rs`](lib.rs) 的显式再导出。
3. 若模板仍只用 `%s`、`%d`、`%%` 和 `.N` 字符精度，可复用 `format`；引入其他格式语义时，应先扩展格式化器并在独立测试文件中覆盖缺参、多参、非 ASCII、转义百分号和新格式边界。
4. 测试逻辑保持在独立文件：格式化器单测放在 [`error_test.rs`](error_test.rs)，描述符表/分类回归可扩展 [`error_1_aster_unit_test.rs`](error_1_aster_unit_test.rs)；不要把测试嵌入 `error.rs`。
5. 检查所有直接消费者是否依赖具体 code、class 或文本。尤其注意 `session/runtime/session.rs` 直接读取 code，而 `nextgen.rs` 同时保留描述符和格式化消息。

主要兼容风险是错误码或文本变化影响 MySQL 客户端、测试断言和告警匹配；正确性风险是新模板超出有限格式化器能力，或忘记加入 `ALL_ERRORS`；性能风险较低，但高频路径每次格式化都会分配一个新 `String`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/sessionctx/variable` 确认目标及相关测试已索引；`node --file pkg/sessionctx/variable/error.rs` 读取 273 行完整定义，并报告本文件被 34 个文件使用；`query ErrorDescriptor --kind struct` 将类型定位到本文件。精确 `callers` 查询在 30 秒执行窗口内未返回，未据此作未验证的逐调用者结论。
- 源与边界：[`error.rs`](error.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。目标目录没有 `doc.go`，因此包边界以 crate 入口 `lib.rs` 为准。
- Rust 直接证据：[`variable.rs`](variable.rs)、[`nextgen.rs`](nextgen.rs)、[`../../session/runtime/session.rs`](../../session/runtime/session.rs)、[`../../util/gcutil/gcutil.rs`](../../util/gcutil/gcutil.rs)、[`../../util/password-validation/password_validation.rs`](../../util/password-validation/password_validation.rs)。
- Rust 测试：[`error_test.rs`](error_test.rs) 覆盖 Unicode 字符精度和 `%%`；[`error_1_aster_unit_test.rs`](error_1_aster_unit_test.rs) 覆盖 code、class、格式化与 24 项表长度；[`tests/variable_test.rs`](tests/variable_test.rs) 检查 `ALL_ERRORS` 非空且 code 非零。
- Go 对照：[`error.go`](error.go)、[`session.go`](session.go)、[`varsutil.go`](varsutil.go)、[`sysvar.go`](sysvar.go)、[`tests/variable_test.go`](tests/variable_test.go) 与 [`sysvar_test.go`](sysvar_test.go)。这些文件证明 Go 侧构造/使用方式及错误码回归意图；本任务未运行 Go 或 Rust 测试，因为计划明确为纯文档分析且禁止运行 Cargo。
