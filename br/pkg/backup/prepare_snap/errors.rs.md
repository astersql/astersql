# `br/pkg/backup/prepare_snap/errors.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-backup-prepare-snap`。该包以 `br/pkg/backup/prepare_snap/lib.rs` 为 crate 根，Cargo 元数据把它对应到 Go 包 `br/pkg/backup/prepare_snap`，且依赖表为空；因此这里的错误实现是 crate 内部自足的适配层，不依赖 PingCAP 的 Go 错误库或额外 Rust 错误库（依据：`br/pkg/backup/prepare_snap/Cargo.toml`、`br/pkg/backup/prepare_snap/lib.rs`）。

`lib.rs` 通过 `#[path = "errors.rs"] pub mod errors` 装入本模块，并向 crate 调用者重新导出 `Error`、`Result`、`convertErr`、`leaseExpired`、`retryLimitExceeded` 和 `unsupported`。`eof` 仍可通过 `crate::errors::eof` 使用，但没有从 crate 根重新导出。源码没有条件编译项；测试只是在 `lib.rs` 的 `cfg(test)` 下以独立文件挂载。

## 核心职责

本文件做三件事：

1. 用 `Error { message, cause, kind }` 表达当前迁移范围需要的文本、单链原因和错误类别，并实现标准库 `Display`/`std::error::Error` 接口。
2. 用 `convertErr` 把 PrepareSnapshot 协议的 `errorpb::Error` 转为本地错误，供 `stream.rs` 把 TiKV 响应送入 `Preparer` 事件链。
3. 为租约过期、不支持、重试耗尽和流结束提供稳定构造器，使 `stream.rs`、`prepare.rs` 与 Go 版本共享关键文案或控制信号。

它不是通用错误框架：只有 `Message` 与 `Eof` 两种内部类别，没有错误码、回溯、任意外部错误装箱或 Go `pingcap/errors` 的完整 Cause/Trace 语义（依据：`ErrorKind`、模块级注释）。

## 主要符号

- `pub struct Error`：持有私有 `message: String`、可选 `cause: Option<Box<Error>>` 和私有 `kind: ErrorKind`。`Clone` 会深拷贝整个单向错误链。
- `enum ErrorKind { Message, Eof }`：模块私有的身份标记。`PartialEq`/`Eq` 仅用于精确判别；调用者不能自行构造或匹配它。
- `Error::new(msg)`：创建无原因的普通消息错误，`kind` 固定为 `Message`。
- `Error::annotate(err, msg)`：将已有本地 `Error` 装入 `cause`，新建一个 `Message` 外层。它保留展示和 `source()` 链，但不会把内层 `Eof` 身份提升到外层。
- `Error::annotatef(err, msg: String)`：名称对齐 Go `errors.Annotatef` 的薄封装；格式化在调用点先完成，本函数只转调 `annotate`。
- `Error::message()`：只返回最外层消息，不拼接 cause。
- `Error::is_eof()`：只检查当前层的 `ErrorKind::Eof`。
- `impl Display for Error`：有 cause 时递归输出 `"外层: 内层"`，否则输出本层消息。
- `impl std::error::Error for Error`：`source()` 返回直接 cause，让标准错误链遍历继续向内。
- `impl From<String>` / `impl From<&str>`：都等价于 `Error::new`，不会推断 `"EOF"` 为 EOF 身份。
- `pub type Result<T>`：固定错误类型为本地 `Error` 的结果别名。
- `convertErr(Option<&errorpb::Error>) -> Option<Error>`：`None` 保持为 `None`；有值时只复制协议对象的 `Message` 字段。
- `leaseExpired()`、`unsupported()`、`retryLimitExceeded()`：分别创建文案为 `the lease has expired`、`unsupported operation`、`the limit of retrying exceeded` 的普通错误。
- `eof()`：创建消息为 `EOF` 且类别为 `Eof` 的专用错误。

本文件没有模块级可变状态、常量、trait 或其他 `impl`。

## 执行流程

协议响应进入错误链的主流程如下（依据：`stream.rs::convert_to_event`、`prepare.rs::onEvent`）：

1. `prepareStream` 收到 `PrepareSnapshotBackupResponse`。
2. `WaitApplyDone` 分支调用 `convertErr(resp.Error.as_ref())`；协议错误不存在时事件携带 `None`，存在时复制 `Message` 成普通 `Error`。
3. `UpdateLeaseResult` 且 `LastLeaseIsValid == false` 时调用 `leaseExpired()`，生成不可恢复的 misc 事件；有效租约响应不投递事件。
4. 未知响应类型用 `unsupported()` 建立根错误，再由 `Error::annotatef` 加上具体响应类型。
5. `Preparer::onEvent` 对 misc 错误再添加 store 上下文并向上返回；WaitApply 的 region 错误则驱动失败区间重试。
6. `Preparer::workOnPendingRanges` 在 `retryTime > RetryLimit` 时返回 `retryLimitExceeded()`；上层阶段函数继续用 `annotate`/`annotatef` 加入操作上下文。

EOF 走另一条控制路径：测试或 `PrepareClient::Recv` 返回 `eof()`，`AsyncStreamBy` 将它作为 `StreamResult.Err` 发送并停止生成线程；`prepareStream::stopClientLoop` 在错误尚未包装时以 `is_eof()`（并兼容最外层消息 `"EOF"`）识别正常流结束。

## 数据与状态

每个 `Error` 完全拥有自己的消息和可选 cause，不借用调用者数据；`convertErr` 同样克隆 `errorpb::Error.Message`。错误链是 `Box<Error>` 形成的单向树枝，而不是共享图，因此读取不需要锁，克隆成本与链深度及消息长度成正比。

重要不变量是：只有 `eof()` 能创建 `ErrorKind::Eof`；`new`、字符串转换、四个业务构造器中的前三个以及 `annotate` 的外层都属于 `Message`。因此消息文本与错误身份相互独立：`Error::new("EOF")` 的显示文本虽相同，但 `is_eof()` 为 false。

`message()` 只暴露最外层文本，而 `to_string()`/`Display` 展开整条链。调用者若需稳定分类，应使用 `is_eof()`；当前其余业务错误仍主要依赖构造位置和固定文案，没有独立 kind。

## 依赖与调用关系

直接下游依赖只有 `crate::env::errorpb` 与 Rust 标准库。`errorpb` 是 `env.rs` 暴露的本地协议边界；`convertErr` 只读取其中的 `Message`，不保留原协议对象或其他潜在字段。

直接上游包括：

- `br/pkg/backup/prepare_snap/stream.rs`：使用 `Error`/`Result` 统一 `PrepareClient`、后台收包和租约续期错误；在 `convert_to_event` 调用 `convertErr`、`leaseExpired`、`unsupported`。
- `br/pkg/backup/prepare_snap/prepare.rs`：在准备、推进、事件处理、建连、发送、Finalize 等边界调用 `Error::annotate`/`annotatef`；用 `unsupported` 拒绝未知事件，用 `retryLimitExceeded` 终止耗尽重试。
- `br/pkg/backup/prepare_snap/env.rs`：环境抽象及重试辅助返回同一个 `Result`，并用 `Error::new` 聚合或生成环境错误。
- `br/pkg/backup/prepare_snap/lib.rs`：向 crate 根重新导出主要错误 API。

RustCodeGraph 状态显示该索引包含目标文件的 21 个符号，文件节点被 41 个文件使用；精确 `callers` 对这些通用命名未解析出边，因此上述直接调用边由目标目录内的符号引用与对应源码交叉确认，未把索引的模糊全仓候选当作本模块调用关系。

## 错误处理与边界

- `convertErr(None)` 明确保留“无协议错误”；`Some` 即使消息为空也会成为 `Some(Error)`，不会按内容过滤。
- `convertErr` 会丢弃除 `Message` 外的协议结构信息；若 `errorpb::Error` 后续增加需要参与分支的字段，当前转换不足以保留它们。
- `annotate` 总是创建 `Message` 外层。因此 `Error::annotate(eof(), ...)` 的 `source()` 仍能追到 EOF，但外层 `is_eof()` 为 false；`errors_test.rs::eof_detection_matches_go_identity_check` 固定了这一行为。
- `Display` 使用冒号和空格拼接每一层，适合向上补上下文，但稳定机器判断不应依赖完整拼接字符串。
- `annotatef` 不接受格式参数；调用点须先 `format!`。这与 Go API 的调用形式不同，但最终文案语义对齐。
- `From<&str>` 会立即复制字符串，返回值不携带借用生命周期。
- 当前错误类型未实现按业务类别比较，租约过期、不支持和重试耗尽只能由创建位置或消息识别；扩展时应避免悄然改变这些被测试覆盖的固定文案。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务或网络资源；`Error` 由拥有所有权的 `String`/`Box` 组成，天然可以随上游值移动。现有字段也都是 `Send`/`Sync` 的标准拥有型数据，因此错误值可被 `stream.rs` 的后台线程、同步通道和 `prepare.rs` 的 Finalize 线程汇聚路径传递。

资源生命周期影响发生在调用方：`AsyncStreamBy` 把一次接收错误送入通道后终止；Finalize 排空流时把未包装的 EOF 当成正常结束；其他错误进入 misc 事件并可能结束准备流程。本模块自身没有清理动作，错误链在最后一个所有者释放时递归释放。

## 与 Go 版本的对应关系

`br/pkg/backup/prepare_snap/errors.go` 定义 `convertErr`、`leaseExpired`、`unsupported`、`retryLimitExceeded` 四个函数。Rust 版本逐一保留名称、nil/`None` 分支和三条固定业务文案：Go 的 `errors.New(err.Message)` 对应 Rust 的 `Error::new(err.Message.clone())`。

Rust 版本为承接 Go 包普遍使用的 `errors.Annotate`/`Annotatef`，额外实现了本地 `Error`、`Result` 和 cause 链；这些在 Go 文件中由 `github.com/pingcap/errors` 提供，而不是 `errors.go` 自己定义。Rust 的 `annotatef` 接收已格式化的 `String`，不是 Go 风格的格式串与变参。

`eof()` 也属于 Rust 迁移适配：Go mock/流直接返回标准 `io.EOF`，Go Finalize 用 `err == io.EOF` 判定结束；Rust 用私有 `ErrorKind::Eof` 和 `is_eof()` 提供对应身份。该身份只检查最外层，和 Go 侧经包装后需区分 cause 的完整错误生态并不等价，当前调用顺序通过在包装前识别 EOF 避开这一差异。

## 扩展指南

- 新增错误类别时，优先扩展 `ErrorKind` 和明确的查询方法，而不是让新控制流依赖 `to_string()`；同步覆盖“同文案不同身份”和包装后的身份规则。
- 若协议层需要保留错误码、region 信息或嵌套原因，应修改 `convertErr`，并同时检查 `stream.rs::convert_to_event` 与 `prepare.rs::onEvent` 的可恢复/不可恢复分支。
- 改动 `annotate`、`Display` 或 `source()` 时，要同步独立测试 `br/pkg/backup/prepare_snap/errors_test.rs`，并检查 `prepare_test.rs`、`parity_test.rs` 中对包装文案和重试/Finalize 行为的断言；不要把测试写回生产源文件。
- 改动三个固定业务构造器时，必须与 `errors.go` 同步，并验证 `stream.go`/`prepare.go` 对应调用点；文案兼容会影响现有子串断言和运维日志。
- 若引入外部错误依赖，需更新 `Cargo.toml` 并重新评估当前无依赖、可克隆、可跨线程传递的性质；本文件处于高频错误路径，但正常成功路径不会分配这些错误，性能风险主要来自深链格式化和克隆。

## 验证依据

- 目标源码：`br/pkg/backup/prepare_snap/errors.rs`，核对全部 137 行、`Error`/`ErrorKind`、2 个 trait 实现、2 个 `From` 实现、`Result` 和 5 个辅助函数；无条件编译项。
- crate 边界：`br/pkg/backup/prepare_snap/Cargo.toml`、`br/pkg/backup/prepare_snap/lib.rs`；确认 crate 名、Go 包映射、空依赖表、模块挂载和重新导出范围。
- 直接 Rust 调用：`br/pkg/backup/prepare_snap/stream.rs`、`prepare.rs`、`env.rs`；确认协议转换、lease/未知类型事件、重试耗尽、上下文包装和 EOF 消费位置。
- Go 对照：`br/pkg/backup/prepare_snap/errors.go`、`stream.go`、`prepare.go`；确认四个函数、固定文案及其生产调用点。
- 独立测试：`br/pkg/backup/prepare_snap/errors_test.rs` 验证 EOF 身份边界；`parity_test.rs::contract_error_retry_limit` 和 `contract_resource_finalize_clears_lease` 验证重试文案、三类业务文案与 `convertErr` 的空值/消息转换；`prepare_test.rs` 与 Go `prepare_test.go` 提供流结束、失败重试和租约场景证据。
- RustCodeGraph：`status` 显示 7,032 个 Rust 文件已索引；`files --filter br/pkg/backup/prepare_snap` 确认目标、Go 对照及独立测试都在索引范围；`node --file .../errors.rs` 返回完整源码及 41 个使用文件；`query` 定位 Rust/Go 同名符号；`callers/callees` 的通用名结果存在歧义，故调用边以精确目录搜索和源码复核为准。
- 本任务是纯文档分析，按任务计划不运行 Cargo；交付前以 `test -f` 和固定标题的 `rg -c` 组合命令验证文档存在且恰有 11 个固定二级章节（退出码 0），并人工检查未把本地适配层描述为完整 PingCAP 错误生态。
