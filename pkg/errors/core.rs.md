# `pkg/errors/core.rs`

## 文件定位

`core.rs` 是 `astersql-errors` crate 的基础值层：它定义所有公开错误 API 共用的拥有型错误句柄 `SharedError`、Go 可变参数的 Rust 表示 `ErrorArg`，以及创建根错误的 `New`/`Errorf`。crate 根文件 `pkg/errors/mod.rs` 将 `DynError`、`SharedError`、`ErrorArg`、`New` 和 `Errorf` 重新导出，因此调用方通常通过 `astersql_errors::*` 使用它们，而不直接访问私有 `core` 模块。

该文件不是完整的错误链实现。堆栈捕获由 `pkg/errors/stack.rs` 提供，单因链包装和遍历由 `pkg/errors/wrap.rs` 提供，多原因组由 `pkg/errors/group.rs`、`pkg/errors/join.rs` 提供，规范化错误由 `pkg/errors/normalize.rs` 提供，Juju 风格适配 API 位于 `pkg/errors/adaptor.rs`。`pkg/errors/Cargo.toml` 声明 crate 名为 `astersql-errors`、库入口为 `mod.rs`；直接依赖只有 `backtrace` 与带 `derive` 的 `serde`，其中本文件本身只使用标准库并经 `stack.rs` 间接依赖 `backtrace`。

## 核心职责

1. 用 `Arc<dyn std::error::Error + Send + Sync + 'static>` 将异构错误统一为可拥有、可跨线程传递且可廉价克隆的 `SharedError`。
2. 保存可选的 `ErrorGroup` 视图，使一个 `SharedError` 在保留普通 `StdError` 表面的同时可被组遍历逻辑识别。
3. 用拥有型枚举 `ErrorArg` 承接 Go `...interface{}` 格式参数，避免借用生命周期越过延迟格式化边界，并支持字符串冻结与脱敏标记。
4. 用内部 `Fundamental` 节点保存根消息和调用栈；`New` 与 `Errorf` 都在创建点捕获栈。
5. 实现当前 Rust 迁移所需的 Go 风格格式子集，并为类型不匹配、缺参和不支持的动词产生稳定输出。
6. 为 `wrap.rs`、`normalize.rs`、`adaptor.rs` 提供 crate 内辅助函数，以便读取根消息、取得根栈、清除根栈及构造无栈根错误。

## 主要符号

- `pub type DynError = dyn StdError + Send + Sync + 'static`：`SharedError` 内部 trait object 的统一边界。`Send + Sync` 是跨线程共享的静态类型约束，`'static` 使对象可以安全存入 `Arc`。
- `pub struct SharedError`：包含 `error: Arc<DynError>` 和可选的 `group: Option<Arc<dyn ErrorGroup>>`。派生 `Clone` 只增加引用计数，不复制底层错误。
  - `SharedError::new` 包装普通标准错误，`group` 固定为 `None`。
  - `SharedError::new_group` 要求具体类型实现 `ErrorGroup`，从同一个 `Arc<E>` 同时构造普通错误与组视图。
  - `downcast_ref::<E>` 将查询转发给底层 `dyn Error`，供 `wrap.rs` 等模块识别具体内部节点，也允许外部调用者恢复自定义错误类型。
  - `ptr_eq` 比较底层错误 `Arc` 的对象身份；同文本的新错误不相等，克隆值相等。
  - `error_group` 是 crate 内接口，仅供 `group.rs` 取得缓存的多原因视图。
  - `Display`/`Debug` 透明转发到底层错误；`StdError::source` 返回底层错误本身，使标准库调用者可看到被句柄包裹的节点。
- `pub enum ErrorArg`：拥有 `String`、`Bool`、`Signed(i128)`、`Unsigned(u128)`、`Float(f64)`、预先冻结的 `Debug(String)`，以及隐藏的 `Redacted(Box<ErrorArg>)`。
  - `debug` 立即执行 `Debug` 格式化并保存字符串。
  - `from_hacked` 调用 `normalize::HackedStr::FreezeStr`，冻结可能别名可变存储的字符串。
  - `display_value`、`go_type_name`、`format_verb_with_precision` 是内部格式化支撑；`Redacted` 递归格式化、把已有 `‹`/`›` 加倍转义，再以一对 `‹…›` 包裹。
  - `From` 实现覆盖 `&str`、`String`、`bool`、全部有符号/无符号整数以及 `f32`/`f64`；整数统一提升到 128 位，`f32` 提升到 `f64`。
- `struct Fundamental`：私有根错误节点，字段为 `message: String` 与 `stack: Stack`。`Display` 只写消息；普通 `Debug` 等同 `Display`，交替 `Debug`（`{:#?}`）在消息后追加扩展栈；它实现 `StdError` 和 `StackTracer`。
- `pub fn New(message: impl Into<String>) -> SharedError`：按字面消息创建 `Fundamental`，不解析消息中的 `%`；用 `NewStack(1)` 跳过构造函数自身。
- `pub fn Errorf(format: &str, args: &[ErrorArg]) -> SharedError`：先由 `format_message` 产生消息，再以与 `New` 相同的栈策略创建 `Fundamental`。
- `pub(crate) fn format_message`：单遍扫描格式串，支持 `%%`、`%s`、`%d`、`%v` 以及形如 `%.6s`、`%-.4s` 的精度提取。
- `fundamental_message`、`fundamental_stack_tracer`、`without_fundamental_stack`、`new_no_stack`：仅 crate 内可见的 `Fundamental` 桥接函数，避免其他模块直接依赖私有类型。

文件中没有条件编译项，也没有公开 trait 或模块级常量。两个宏 `impl_signed_error_arg!` 与 `impl_unsigned_error_arg!` 只用于生成数值 `From` 实现，不导出到 crate 外。

## 执行流程

`New` 的主路径是：调用者传入字面消息 → `Into<String>` 获得拥有型消息 → `NewStack(1)` 捕获并跳过内部帧 → 构造 `Fundamental` → `SharedError::new` 将其装入 `Arc<DynError>`。消息中的 `%v` 等字符保持原样；`pkg/errors/tests/core_test.rs::new_and_errorf_match_go_basics` 明确覆盖这一点。

`Errorf` 的主路径是：调用者先将参数转成 `ErrorArg` 切片 → `format_message` 逐字符扫描 → 普通字符直接复制，`%%` 写入单个 `%` → 对其他 `%` 收集字符直到首个 ASCII 字母作为动词 → 仅识别 `s`、`d`、`v` → 从动词前缀最后一个 `.` 后解析十进制精度 → 消费一个参数并调用 `format_verb_with_precision` → 完成消息后捕获栈并创建 `Fundamental`。

关键分支如下：

- `%s` 只接受 `String`；有精度时按 Unicode `char` 数截断，而不是按 UTF-8 字节截断。
- `%d` 接受有符号或无符号整数；`%v` 对所有当前变体使用默认字符串表示。
- 动词与参数类型不匹配时输出 `%!<verb>(<go-type>=<value>)`，例如字符串配 `%d` 得到 `%!d(string=not-a-number)`。
- 支持的动词缺少参数时输出 `%!<verb>(MISSING)`。
- 不支持或不完整的说明符原样写回 `%` 加已收集文本，并且不消费参数。
- 额外参数不会附加到结果。这一点不同于完整 `fmt.Sprintf` 的额外参数诊断，属于当前实现的明确边界。

内部消费者路径也很重要：`wrap.rs::Wrapf`、`adaptor.rs::Annotatef`/`NewNoStackErrorf` 与 `normalize.rs` 的消息生成复用 `format_message`；`wrap.rs` 通过 `fundamental_message` 拼接自有消息，通过 `fundamental_stack_tracer` 把根错误接入 `StackTraceCarrier`，通过 `without_fundamental_stack` 在 `SuspendStack` 流程中重建无栈副本；`adaptor.rs::NewNoStackError` 直接调用 `new_no_stack`。

## 数据与状态

`SharedError` 的身份是底层 `Arc<DynError>` 的分配身份，而不是消息值。克隆共享同一分配，`ptr_eq` 为真；重新用同样消息构造的错误拥有不同分配，`ptr_eq` 为假。`new_group` 的 `error` 和 `group` 视图来自同一个 `Arc<E>`，从而确保错误文本、类型和多原因列表属于同一对象。

`ErrorArg` 完全拥有其内容。字符串被复制或移动，数值被规范化到宽类型，`Debug` 值在构造参数时就被冻结，`from_hacked` 也立即冻结字符串；因此后续格式化不依赖原对象的生命周期或可变状态。`Redacted` 是结构状态而非全局开关，它只改变该参数渲染时的边界标记；是否将参数改写为 `Redacted` 由 `normalize.rs::RedactErrorArg` 决定。

`Fundamental` 的消息与 `Stack` 创建后不再修改。普通根错误持有真实捕获栈，无栈辅助构造器持有 `Stack::default()`。`without_fundamental_stack` 只在底层确为 `Fundamental` 时返回相同消息的新对象；它不会原地修改或保持 `Arc` 身份。

本文件没有全局可变状态、缓存、I/O 句柄、事务或异步任务。内存成本主要是一次底层错误分配、一个 `Arc`，以及 `New`/`Errorf` 的 backtrace 捕获；格式化成本与格式串和输出长度线性相关，字符串精度截断还会遍历 Unicode 字符。

## 依赖与调用关系

向下依赖：

- 标准库 `std::error::Error`、`std::fmt`、`std::sync::Arc` 提供错误对象、格式协议和共享所有权。
- `stack::{NewStack, Stack, StackTrace, StackTracer}` 提供捕获、保存和导出调用栈；`NewStack` 的实现位于 `pkg/errors/stack.rs`，使用 `backtrace` crate 解析真实帧。
- `group::ErrorGroup` 为 `new_group` 与 `error_group` 提供多原因接口。
- `normalize::HackedStr` 为 `ErrorArg::from_hacked` 提供冻结协议。

向上消费者：

- `pkg/errors/mod.rs` 再导出公开 API，是 crate 外调用入口。
- `pkg/errors/wrap.rs` 消费全部四个 `Fundamental` 辅助查询/重建函数以及 `format_message`，并为 `SharedError` 实现 `StackTraceCarrier`。
- `pkg/errors/adaptor.rs` 使用 `format_message`、`new_no_stack`，分类错误最终回到 `Errorf`。
- `pkg/errors/normalize.rs` 用 `format_message` 生成规范化错误消息，并广泛持有 `SharedError`/`ErrorArg`。
- `pkg/errors/group.rs` 读取 `SharedError::error_group`；`pkg/errors/join.rs` 用 `SharedError::new_group` 创建合并错误。

RustCodeGraph 状态显示索引含 11,467 个文件、307,296 个节点，`pkg/errors/core.rs` 被 387 个文件使用；精确 `Errorf`、`format_message` 节点的 `callers/callees` 查询未返回细粒度边，因此上面的 crate 内直接边由 `rg` 对真实引用补证。图查询能确认的代表性公开调用者包括 `pkg/parser/mysql/error.rs` 的格式参数路径以及多个 BR Rust 模块中的 `New`/`Errorf` 使用点；应用代码依赖的是 `mod.rs` 再导出的门面，而不是私有模块路径。

## 错误处理与边界

`SharedError::new` 与 `new_group` 的泛型约束在编译期排除非 `Send`、非 `Sync` 或非 `'static` 错误；没有运行时降级。`downcast_ref`、`error_group` 与所有 `Fundamental` 查询函数用 `Option` 表示类型不匹配，不会 panic。

`format_message` 不返回 `Result`。格式问题被编码进输出：缺参使用 `MISSING` 诊断，已支持动词的类型不匹配使用 Go 风格 `%!…` 诊断；精度数字解析失败时忽略精度。扫描到不支持动词或格式串在 `%` 后结束时，说明符原样保留。当前解析器不是完整 Go `fmt`：只支持 `s`/`d`/`v`，没有宽度、索引参数、动态精度和多数数值动词的完整语义；虽然它能从带标志的字符串说明符提取精度，但不会实现对齐或填充。扩展功能时不能把“对现有迁移调用点足够”误写成“完整兼容 `fmt.Sprintf`”。

`SharedError` 的 `StdError::source` 总是返回底层节点，这表示第一层标准库 `source` 是句柄所包装的对象；库自己的单因链遍历由 `wrap.rs` 对已知包装类型处理。`Fundamental` 自身没有 cause。多原因组不通过 `StdError::source` 表达全部原因，而通过缓存的 `ErrorGroup` 视图交给 `group.rs`。

交替 `Debug` 是此库映射 Go `%+v` 的约定；普通 `Display`/`Debug` 仅显示消息。栈符号解析失败、空栈等边界由 `stack.rs` 处理，本文件只是保存和转发 `StackTracer`。

## 并发与资源生命周期

`SharedError` 的底层对象受 `Send + Sync + 'static` 约束并由线程安全的 `Arc` 管理，所以句柄可以跨线程移动或共享，克隆与销毁只调整原子引用计数；最后一个引用释放时底层错误和可选组对象随之释放。`new_group` 中两个 trait-object 视图共同持有同一分配，直到两个 `Arc` 字段都释放。

创建错误是同步操作：`New`/`Errorf` 在当前线程立即捕获 backtrace，之后保存的是拥有型 `Stack`，没有后台解析任务、锁、通道或外部资源。格式化只读取不可变字段；`ErrorArg` 的冻结策略避免并发环境下继续引用调用者的短生命周期或可变字符串。并发安全并不意味着底层任意错误都可包装——泛型边界会拒绝不满足 `Send + Sync` 的类型。

无栈重建会产生新的 `SharedError` 和新的分配，原错误仍由其已有 `Arc` 所有者持有，不会被就地清空。调用者若需要保留对象身份，不应把 `without_fundamental_stack`/`SuspendStack` 当作可逆的原位状态切换。

## 与 Go 版本的对应关系

仓库 `go.mod` 锁定 `github.com/pingcap/errors v0.11.5-0.20260508054701-306e305bcf41`。对应模块缓存的 `errors.go` 中，Go `New`/`Errorf` 都构造私有 `fundamental{msg, stack: callers()}`；`fundamental.Error` 返回消息，`Format` 在 `%+v` 时追加栈。这与 Rust 的 `Fundamental`、`NewStack(1)`、`Display` 和交替 `Debug` 是直接映射。

所有权与动态类型的主要差异是：Go 直接返回接口值 `error`，Rust 返回 `SharedError`；Go 运行时接口断言映射为 Rust 的 trait object、`downcast_ref` 和显式 `StackTraceCarrier`；Go 可变参数 `...interface{}` 映射为 `&[ErrorArg]`。Go `fmt.Sprintf` 支持完整格式语言，Rust `format_message` 仅实现当前迁移需要的子集。Rust 额外提供 `Arc` 指针身份 `ptr_eq`、强制 `Send + Sync`、拥有型参数冻结，以及用 `Redacted` 表达脱敏边界。

Go `fundamental` 内嵌 `*stack`，Rust `Fundamental` 按值持有 `Stack`。Go 的 `New`/`Errorf` 返回对象没有 cause；Rust 同样如此，但外层 `SharedError::source` 暴露被句柄包裹的 `Fundamental`，这是与 Rust 标准错误生态互操作的适配层。Go 的 `%q` 由 `fmt.Formatter` 直接处理；Rust 测试通过对 `Display` 结果做 Rust `Debug` 引号化表达同一测试意图。

`pkg/errors/tests/core_test.rs` 对应 Go `errors_test.go::TestNew`/`TestErrorf`，覆盖字面消息、基础格式化、克隆身份、downcast、标准 `source`、精度和类型错误诊断。`pkg/errors/tests/format_test.rs` 对应 Go `format_test.go::TestFormatNew`/`TestFormatErrorf`，验证纯消息与扩展栈输出。`pkg/errors/tests/api_parity_test.rs` 记录源提交 `306e305bcf41` 的公开 API 盘点，并明确可变参数、nil、所有权与格式 trait 的映射方式。

## 扩展指南

扩展格式语言时，首要修改点是 `ErrorArg`、`ErrorArg::format_verb_with_precision` 与 `format_message`。新增参数类型应同时决定：拥有方式、Go 类型名、默认 `%v` 表示、允许的动词、精度/宽度语义、脱敏递归行为，并补充 `From` 实现。不要让 `ErrorArg` 借用临时值，否则会破坏当前延迟格式化的生命周期保证。

修改根错误结构或栈行为时，必须同步检查 `Fundamental` 的 `Display`/`Debug`/`StackTracer`、`New`/`Errorf` 的 `NewStack` 跳帧数，以及 `fundamental_message`、`fundamental_stack_tracer`、`without_fundamental_stack`、`new_no_stack` 四个内部桥接函数。`wrap.rs::fmt_extended`、`StackTraceCarrier for SharedError`、`clear_stack` 与 `GetErrStackMsg` 都依赖这些不变量；改变私有具体类型而不更新桥接函数会让栈发现或消息拼接静默失效。

修改 `SharedError` 所有权或组支持时，应同时核对 `group.rs::Errors`/`WalkDeep`、`join.rs::Join`、标准库 `source` 互操作以及 `ptr_eq` 的身份语义。不能为方便扩展而去掉 `Send + Sync + 'static`，因为这会改变 crate 的线程安全契约。

测试应继续放在独立文件中，不内嵌进 `core.rs`。直接回归位置是 `pkg/errors/tests/core_test.rs`；格式输出和栈帧数放在 `pkg/errors/tests/format_test.rs`；公开表面变化同步更新 `pkg/errors/tests/api_parity_test.rs`；若涉及脱敏/冻结参数，再更新 `pkg/errors/tests/normalize_generation_test.rs`。新增 Go 格式语义时，应以仓库锁定的 `github.com/pingcap/errors` 版本及其 `errors_test.go`/`format_test.go` 为基线，并明确记录 Rust 无法或无需复制的差异。性能风险主要来自扩大格式解析复杂度、增加临时字符串分配，或改变每次构造都会捕获 backtrace 的策略。

## 验证依据

- RustCodeGraph：`status` 确认索引可用；`files --filter pkg/errors` 确认 `core.rs` 与独立测试集合；`node --file pkg/errors/core.rs --offset 1 --limit 260` 和 `--offset 261 --limit 140` 阅读完整 358 行源码；对 `SharedError ErrorArg Fundamental New Errorf format_message` 的 `explore` 确认文件级使用范围和代表性调用者；精确节点 `callers/callees` 返回空结果，故未把缺失的细粒度图边当作无调用者结论。
- 直接 Rust 源：`pkg/errors/core.rs`；模块与 crate 边界：`pkg/errors/mod.rs`、`pkg/errors/Cargo.toml`；直接实现依赖及消费者：`pkg/errors/stack.rs`、`pkg/errors/wrap.rs`、`pkg/errors/adaptor.rs`、`pkg/errors/normalize.rs`、`pkg/errors/group.rs`、`pkg/errors/join.rs`。
- Rust 独立测试：`pkg/errors/tests/core_test.rs`、`pkg/errors/tests/format_test.rs`、`pkg/errors/tests/api_parity_test.rs`，并通过引用检索确认 `normalize_generation_test.rs`、`std_interop_test.rs`、`group_join_test.rs` 等覆盖相邻契约。
- Go 对照：仓库 `go.mod`/`go.sum` 锁定 `github.com/pingcap/errors` 提交 `306e305bcf41`；读取本机模块缓存该版本的 `errors.go`、`errors_test.go`、`format_test.go`。仓库 `pkg/errors` 下没有同路径 `.go` 文件，因此未虚构本地 Go 对照文件。
- 人工复核结论：本文件存在是为了给整个 Rust 错误子系统提供线程安全的统一错误值、根错误与受控的 Go 格式映射；运行时由构造、格式化、栈捕获和 `Arc` 共享组成；安全扩展必须同时维护格式参数、根栈桥接、包装消费者与独立测试的契约。
