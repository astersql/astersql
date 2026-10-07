# `pkg/lightning/common/errors.rs`

## 文件定位

本文件属于 `astersql-lightning-common` crate。crate 入口 `pkg/lightning/common/lib.rs` 以私有 `mod errors` 装载本模块，再通过 `pub use errors::*` 将这里的错误类型、构造器、归一化函数和错误模板暴露给 Lightning 及 Import Into 相关代码。`pkg/lightning/common/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `package.metadata.porting.go-package = "pkg/lightning/common"` 表明它是 Go 包 `pkg/lightning/common` 的 Rust 移植组成部分。

本文件不是错误发生点的全集，而是公共错误协议层：它给错误附加稳定 RFC ID、保留原因链和简化栈信息，并把 BR 错误身份转换成 Lightning 身份。RustCodeGraph 将该文件识别为 33 个符号、被 18 个文件使用；当前可见的直接生产调用之一是 `pkg/lightning/common/dupdetect.rs::DupDetector::Next`，在 `ReportErrOnDup` 分支调用 `ErrFoundDuplicateKeys`。

## 核心职责

1. `CommonError` 统一承载 `ID`、展示消息、错误种类、可选协议状态、嵌套原因和栈片段，并实现 `Display` 与 `std::error::Error`。
2. `NormalizeError` 将任意 `CommonError` 归一为 Lightning/Import RFC 错误：取消错误透传，已有 Lightning/Import ID 保留，已知 BR ID 映射，其他错误归入 `Lightning:Common:ErrUnknown`。
3. `NormalizeOrWrapErr` 在归一化只能得到 `ErrUnknown` 时，改用调用方指定的 RFC 模板包裹原错误，以保留业务上下文。
4. `define_error!` 声明 63 个懒加载错误模板，覆盖 Common、Config、Storage、Loader、PreCheck、Checkpoint、MetaMgr、DB、PD、KV、Restore 和 Import 分类；另有 4 个 `BR_Err*` 静态样例供 BR 映射与测试使用。
5. `REDACT_LOG_ENABLED` 与 `redact_arg` 实现 `ErrCastValue` 第三个参数的日志脱敏；`ErrFoundDuplicateKeys` 则将原始字节稳定格式化为小写十六进制。

## 主要符号

- `CommonError`：可克隆、可比较的错误树。`new(kind, message)` 创建无 ID 错误，`rfc(id, message)` 创建 `Kind == "rfc"` 的稳定错误，`wrap` 追加 cause，`annotate` 创建无 ID 的外层注解，`gen_with_stack`/`gen_with_stack_by_args` 生成具体消息并在缺栈时加入占位栈帧。
- `CommonError::gen_with_stack_by_args`：按消息中最靠前的 `%s`、`%d` 或 `%x` 逐个替换参数；只有模板 ID 为 `Import:ErrCastValue` 且参数序号为 2 时调用 `redact_arg`。参数多于占位符时剩余参数被忽略，参数不足时剩余占位符保留。
- `CommonError::Display`：无 ID 时只输出 `Message`，有 ID 时输出 `[ID]Message`。因此错误身份同时参与用户可见文本。
- `withStack`：对应 Go `errors.WithStack` 形态的显式包装，`Cause`/`Unwrap` 返回内部错误，`Format(true)` 在错误文本后追加栈行。当前 `NormalizeError` 直接把栈复制到 `CommonError`，本文件内没有构造 `withStack` 的路径。
- `Is(error, expect)`：只在 `expect.ID` 非空时按 ID 匹配，并递归搜索 `Causes`；它不是按消息或 Rust 类型比较。
- `find_rfc_error`：深度优先返回错误树中第一个非空 ID 节点，是归一化识别内层 RFC 错误的入口。
- `map_br_error_id`：识别 Storage 的 `BR:KV:*`/`BR:Storage:*` 两套前缀、PD 更新失败和版本不匹配；未列出的 BR ID 一律映射到 `ErrUnknown`。
- `NormalizeError`、`NormalizeOrWrapErr`：公共归一化入口。RustCodeGraph 的 callee 边分别指向 `wrap`/`find_rfc_error`/`map_br_error_id`，以及 `wrap`/`gen_with_stack_by_args`/`Is`/`NormalizeError`。
- `ErrFoundDuplicateKeys(key, value)`：专用构造函数，生成 `Lightning:Restore:ErrFoundDuplicateKey`，避免通用 `%x` 替换器丢失 Go `%x` 的字节编码语义。
- `define_error!` 与各 `Err*`：`LazyLock<CommonError>` 确保模板首次使用时初始化，调用方通常先 `clone()`，再生成消息或包裹 cause，避免修改共享模板。

## 执行流程

`NormalizeError(error)` 的流程如下：

1. `None` 立即返回 `None`；`crate::IsContextCanceledError` 识别到 `Kind` 为 `cancelled`/`context` 且消息包含 `canceled` 时原样返回。
2. 先保存外层错误的 `Stack`，随后用 `find_rfc_error` 搜索原因树中的第一个 RFC 节点。
3. 如果找到了 RFC 节点，复制该节点。若外层文本形如 `前缀: [ID]内层消息`，则执行 issue 32133 的兼容修正：把前缀改成 RFC 错误的消息，仅保留其第一个 cause，并在后续恢复原栈。
4. ID 已以 `Lightning:` 或 `Import:` 开头时保持该身份；否则通过 `map_br_error_id` 创建新的 Lightning RFC 错误，并至多转移原 RFC 节点的第一个 cause。
5. 未找到任何 RFC 节点时，返回 `ErrUnknown.clone().wrap(error)`。若新结果没有栈，则补回步骤 2 保存的外层栈。

`NormalizeOrWrapErr(rfc_error, error, args)` 同样先处理空值和取消。其后调用 `NormalizeError`；若 `Is(normalized, ErrUnknown)`，使用 `rfc_error.clone().wrap(original).gen_with_stack_by_args(args)` 生成业务错误，否则直接返回已归一化结果。

重复键路径独立于上述归一化：`DupDetector::Next` 发现相同业务键且 `ReportErrOnDup` 为真时，把当前键和值传给 `ErrFoundDuplicateKeys`；函数逐字节输出两位十六进制，形成稳定的 Restore RFC 错误。

## 数据与状态

`CommonError` 的 `ID` 是分类与 `Is` 判断的核心不变量；`Message` 是模板或已展开文本；`Kind` 区分普通、注解、RFC、取消等来源；`Causes` 是可分叉的错误树；`Stack` 只是字符串片段而非 Rust 回溯对象。`Code`、`StatusCode`、`RpcCode` 在构造器中均初始化为 `None`，本文件没有填充值的逻辑。

错误模板由 `LazyLock` 保存为进程级只读原型。生成具体错误时必须克隆模板；`wrap`、`annotate` 和 `gen_*` 都消费并返回值，不会原地改变静态模板。`NormalizeError` 在映射时只保留第一个直接 cause，因此 `Causes` 中额外分支不会跨该映射完整传递。

唯一可变全局状态是 `REDACT_LOG_ENABLED: AtomicU8`。值 0/1/2 分别对应关闭、替换成 `?`、用 `‹…›` 标记；未知值按关闭处理。读取使用 `SeqCst`，保证各线程观察到统一的全序，但该设置仍是进程全局而非请求局部。

## 依赖与调用关系

- 上游装配：`pkg/lightning/common/lib.rs` 装载并再导出本模块；`Cargo.toml` 没有为本文件引入第三方运行时依赖，主要使用标准库的 `fmt`、`LazyLock`、`AtomicU8`。
- 上游生产调用：`pkg/lightning/common/dupdetect.rs::DupDetector::Next` 调用 `ErrFoundDuplicateKeys`。RustCodeGraph 还报告本文件被 `pkg/dxf/framework/storage/converter.rs`、`pkg/dxf/importinto/scheduler.rs`、`pkg/dxf/importinto/task_executor.rs` 等文件使用，说明其公开错误模板跨 Lightning/Import Into 边界共享。
- 下游调用：`NormalizeError` 调用同文件 `find_rfc_error`、`map_br_error_id`、`CommonError::wrap`，并调用 `pkg/lightning/common/util.rs::IsContextCanceledError`；`NormalizeOrWrapErr` 再调用 `NormalizeError`、`Is`、`wrap` 与 `gen_with_stack_by_args`。
- 测试装配：`pkg/lightning/common/lib.rs` 通过 `#[cfg(test)] #[path = "errors_test.rs"]` 保持测试与生产源分文件；`Cargo.toml` 的 `serial_test` 仅用于保护修改全局脱敏状态的测试。

## 错误处理与边界

- 空错误不被制造成错误：两个归一化入口接收 `Option<CommonError>` 并对 `None` 返回 `None`。
- 取消错误必须透传，避免被改写为 Unknown 或业务 RFC 错误；准确判定依赖 `util.rs::IsContextCanceledError` 当前的 kind/message 约定。
- `Is` 只认非空 RFC ID；不能用无 ID 的普通错误作为期望值，也不会比较 `Kind`、`Message` 或 cause 的对象身份。
- 未知 RFC ID 与完全无 RFC ID 的路径最终都可归为 `ErrUnknown`，但前者会重建错误且最多保留一个 cause，后者把完整原错误作为单个 cause 包入 Unknown。
- 注解兼容分支依赖严格的字符串后缀 `": {n_err_msg}"`。若格式不同，错误仍会被分类，但不会提取注解前缀作为新消息。
- `gen_with_stack_by_args` 是轻量占位符替换，不支持 Go `fmt` 的完整格式语法；字节 `%x` 因此由 `ErrFoundDuplicateKeys` 专门处理。
- 源码中的 `ErrChecksumMismatch` ID 为 `Lighting:Restore:ErrChecksumMismatch`（少一个 `n`），与同路径 Go 文件一致；这是已有兼容身份，不能在扩展时顺手更正而不评估外部消费者。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、文件句柄或网络资源。`CommonError` 和原因树均拥有自己的 `String`/`Vec` 数据，克隆会复制这些内容，生命周期不借用调用方缓冲区。

`LazyLock` 负责错误模板的一次性、线程安全初始化；`AtomicU8` 负责跨线程读取脱敏模式。测试 `test_normalize_error` 与 `test_err_cast_value_redact` 使用 `#[serial]`，并通过本地 guard 的 `Drop` 恢复原模式，避免并行测试互相污染。生产调用若在请求处理中切换该全局值，会影响同时发生的所有 `ErrCastValue` 格式化，因此应把它当作进程配置，而不是临时开关。

## 与 Go 版本的对应关系

权威对照是 `pkg/lightning/common/errors.go` 与 `errors_test.go`。Rust 的 63 个 `define_error!` 模板复刻 Go `var` 块的 RFC ID 和消息；`NormalizeError` 保留 Lightning/Import ID、转换五类 BR 错误、保留栈及修复注解前缀的主流程与 Go 一致；`NormalizeOrWrapErr`、三种 CastValue 脱敏模式和重复键十六进制文本也保持相同意图。

Rust 不是 `pingcap/errors` 的完整实现：`CommonError` 用值类型和 `Vec<CommonError>` 模拟错误链，栈是字符串占位；Go 可通过接口搜索任意 `*errors.Error` 和 `StackTracer`，Rust 则由 `find_rfc_error` 与字段复制完成。Go 的 `withStack` 真正包装任意 `error` 和 `errors.StackTracer`，Rust 的 `withStack` 只包装 `CommonError`，且当前归一化路径未实例化它。

Rust 额外公开 4 个 `BR_Err*` 静态值来代替 Go 对 `br/pkg/errors` 模板的直接引用；它们主要为映射测试提供输入。Rust 测试还增加了 `ErrEncodeKV` 参数展开、`ErrFoundDuplicateKeys` 字节格式，以及全局脱敏模式恢复的明确断言。当前 Rust `redact_arg` 对 Marker 模式直接包裹 `‹…›`，与 Go 测试期望一致。

## 扩展指南

- 新增稳定错误：在对应业务分类附近添加 `define_error!`，确保 ID、消息和同路径 Go 定义一致；调用时克隆模板，再用 `gen_with_stack_by_args` 或 `wrap` 生成实例。同步扩展独立测试 `pkg/lightning/common/errors_test.rs`，不要把测试写入本文件。
- 新增 BR 映射：修改 `map_br_error_id`，同时添加“原 BR ID、注解消息、期望 Lightning ID、期望文本、栈保持”测试；如果 Go 版本也支持该映射，应同步 `errors.go`/`errors_test.go` 的语义。
- 新增敏感参数：当前脱敏逻辑硬编码为 `ErrCastValue` 的第 3 个参数。扩展前应设计可表达多个模板/参数索引的元数据，覆盖 Disable/Enable/Marker，并评估全局原子配置的并发影响。
- 修改归一化：必须保持取消透传、已有 Lightning/Import ID 稳定、Unknown 包装保留原错误、注解前缀兼容和栈传播。尤其要测试多层 `Causes`，因为当前映射只转移第一个 cause。
- 修改错误 ID 或展示格式属于兼容性变化：日志解析、重试分类和外部 API 可能依赖 `[ID]Message`。`ErrChecksumMismatch` 的历史拼写也应按兼容 ID 对待。
- 重复键逻辑若改变，应同时覆盖 `errors_test.rs` 的编码断言和 `pkg/lightning/common/dupdetect_test.rs` 的调用场景；跨 crate 使用还可参考 `pkg/dxf/importinto/task_executor_test.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标文件已索引；`files --filter pkg/lightning/common/errors.rs` 显示目标含 33 个符号；`node --file` 阅读了全部 649 行，并给出“被 18 个文件使用”的反向关系。
- RustCodeGraph 调用边：`callees NormalizeError` 得到 Rust 定义到 `wrap`、`find_rfc_error`、`map_br_error_id`；`callees NormalizeOrWrapErr` 得到到 `wrap`、`gen_with_stack_by_args`、`Is`、`NormalizeError`。`callers` 对这些重载名称没有返回可区分的直接调用边，因此用精确源码搜索补充调用证据。
- 生产与装配源码：`pkg/lightning/common/errors.rs`、`lib.rs`、`Cargo.toml`、`util.rs::IsContextCanceledError`、`dupdetect.rs::DupDetector::Next`。
- Go 对照：`pkg/lightning/common/errors.go`、`pkg/lightning/common/errors_test.go`。
- Rust 独立测试：`pkg/lightning/common/errors_test.rs` 覆盖空值、Unknown、BR 映射、Lightning 保留、栈保持、业务包装、重复键十六进制及三种脱敏模式；`pkg/lightning/common/lib.rs` 确认测试通过独立文件装配。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务文件规定的 11 节结构验证，并人工复核本文没有把简化的 Rust 错误模型描述成完整 `pingcap/errors` 实现。
