# `pkg/lightning/common/retry.rs`

## 文件定位

该文件属于 `astersql-lightning-common` crate，负责把 Lightning 导入链路中形态各异的 `CommonError` 归一为“可重试”或“不可重试”的布尔决策。模块在 [`pkg/lightning/common/lib.rs`](lib.rs) 中以私有 `mod retry` 装配，再通过 `pub use retry::*` 导出公开项；crate 边界及 Go 包映射由 [`pkg/lightning/common/Cargo.toml`](Cargo.toml) 声明。

当前可确认的生产消费点是 [`pkg/lightning/common/util.rs`](util.rs) 的 `Retry`：动作失败且不是 not-found 时，它调用 `IsRetryableError(Some(&error))` 决定是否进入下一次尝试。因此本文件只负责错误分类，不负责等待、尝试次数、日志或动作执行。RustCodeGraph 将本文件标记为被 `util.rs` 和独立测试 `retry_test.rs` 使用；其函数级外部 caller 查询未返回额外生产调用边。

## 核心职责

- `IsRetryableError` 提供总入口，处理空错误、组合错误和普通错误。组合错误必须非空且每个子错误都可重试，整体才可重试。
- `isSingleRetryableError` 按稳定的优先级识别 URL、包装根因、明确错误种类、网络 syscall、MySQL/TiDB 编号、TiKV/PD 错误 ID、HTTP 状态、gRPC 状态，最后才使用错误消息子串兜底。
- `ErrWriteTooSlow` 构造一个专用 `CommonError`，把“gRPC 写入长期阻塞”转换为显式可重试种类。
- 本文件不执行重试，也不改变错误；它读取 `CommonError` 的 `Kind`、`Message`、`Code`、`StatusCode`、`RpcCode` 和 `Causes` 字段并返回判断结果。

## 主要符号

- `retryableErrorMsgList: &[&str]`：三个小写消息片段，覆盖 coprocessor deadline、rate limiter 等待失败和测试注入错误。`isRetryableFromErrorMessage` 会先将完整消息转为 ASCII 小写，再执行包含匹配。
- `retryableErrorIDs: &[&str]`：TiKV、PD、Lightning 和 driver 层已知瞬态错误 ID 白名单，包括 region epoch/leader 变化、server busy、超时、stale command 等。
- `ErrWriteTooSlow() -> CommonError`：返回 `Kind == "write-too-slow"` 的错误；该 kind 在单错误分类中直接判为可重试。
- `isRetryableFromErrorMessage(&CommonError) -> bool`：无法由结构化字段分类时的最终消息兜底。
- `isRetryableURLInnerError(Option<&CommonError>) -> bool`：URL 错误没有内因或内因为 `eof` 时返回 `true`；消息含 `net/http: request canceled` 时返回 `false`，其余 URL 内因返回 `true`。
- `has_retryable_syscall_cause(&CommonError) -> bool`：私有递归函数，搜索整棵 `Causes` 子树是否含 `connection-refused`、`connection-reset` 或 `broken-pipe`。
- `isSingleRetryableError(&CommonError) -> bool`：单错误分类器。除 URL 特例外，它沿每层第一个 cause 解包；遇到 `multi` 或 `url` 停止。
- `IsRetryableError(Option<&CommonError>) -> bool`：公开总入口。`None` 为不可重试；`multi` 使用非空检查和 `all(isSingleRetryableError)`；其他错误交给单错误分类器。

文件没有 trait、struct、impl、泛型状态容器或条件编译项。公开函数和静态表借助 crate 根的通配再导出可见；`has_retryable_syscall_cause` 是唯一私有函数。

## 执行流程

1. 调用方把错误引用交给 `IsRetryableError`。若参数为 `None`，立即返回 `false`。
2. 若顶层 `Kind` 为 `multi`，要求 `Causes` 非空，并对每个直接子错误调用 `isSingleRetryableError`；任何一个不可重试都会短路为 `false`。
3. 单错误分类首先处理顶层 `url`，避免先解包而丢失 URL 语义。URL 无内因和 EOF 内因视为网络传输瞬态问题；显式 HTTP request canceled 不重试。
4. 非 URL 错误沿每层第一个 cause 走到根因，随后按以下顺序分类：
   - `cancelled`、`eof`、`no-rows` 明确不可重试；连接、超时、temporary 和 write-too-slow 等 kind 明确可重试。
   - `dns`、`net`、`addr`、`op` 只有在 cause 树中存在三类可重试 syscall kind 时才可重试。
   - 若存在 MySQL/TiDB `Code`，只接受固定白名单；其他编号立即为不可重试，不再落入后续规则。
   - 错误 `ID` 命中 `retryableErrorIDs` 时可重试。
   - 若存在 HTTP `StatusCode`，400 与 404 不重试，其他状态重试。
   - 若存在 `RpcCode`，固定九类 gRPC code 可重试；`Unknown` 默认可重试，但磁盘空间不足和 SST key 非严格递增两类消息除外；其他 code 不重试。
   - 前述结构化分类均未命中时，才执行大小写不敏感的消息片段匹配。
5. 上层 `util.rs::Retry` 收到 `true` 后保存该错误并继续循环；它最多执行 `defaultMaxRetry`（当前为 3）次，并在后续尝试前休眠。分类器自身不休眠。

分类顺序是兼容性契约。例如一旦 `Code` 存在但不在白名单，函数直接返回 `false`，不会再让错误 ID、HTTP、gRPC 或消息兜底覆盖这个结果。

## 数据与状态

输入 `CommonError` 定义在 [`pkg/lightning/common/errors.rs`](errors.rs)，相关字段为：字符串 `ID`、`Message`、`Kind`，可选 `Code`、`StatusCode`、`RpcCode`，以及拥有所有权的 `Vec<CommonError>` 类型 `Causes`。本文件仅借用并读取这些字段，不修改输入，也不保存跨调用状态。

两个静态切片是进程生命周期内只读的分类表；没有动态注册表、缓存或全局可变状态。根因解包只沿 `Causes.first()` 前进，而 syscall 检查会递归遍历所有 cause 分支。`multi` 的语义只在公开入口处理；把 `multi` 直接交给单错误分类器不会执行“所有子错误”规则。

## 依赖与调用关系

- 上游模块装配：`lib.rs` 声明 `mod retry` 并再导出 `retry::*`；独立测试通过 crate 根导入 `ErrWriteTooSlow` 和 `IsRetryableError`。
- 生产上游：`util.rs::Retry` 在 action 返回错误、且 `is_not_found` 未命中后调用 `IsRetryableError`。not-found 的提前停止规则属于 `util.rs`，不是本文件的分类规则。
- 内部调用边：RustCodeGraph 确认 `IsRetryableError -> isSingleRetryableError`；后者调用 `isRetryableURLInnerError`、`has_retryable_syscall_cause`、`isRetryableFromErrorMessage`，并引用 `retryableErrorIDs`。
- 下游数据类型：唯一 Rust 导入是 crate 内的 `CommonError`；本文件不直接依赖 Cargo 中的外部 crate。`Cargo.toml` 表明整个 common crate 仅有 `astersql-lightning-log`、`libc` 生产依赖和 `serial_test` 开发依赖，但这些依赖均未被 `retry.rs` 直接使用。
- 图验证限制：RustCodeGraph 能解析内部调用边和文件级使用关系，但对本文件公开函数的 `callers` 查询为空；生产调用点因此额外由 `rg` 在 `pkg/lightning/**/*.rs` 中核验，当前只发现 `util.rs::Retry`。

## 错误处理与边界

- `None`、取消、裸 EOF、无行结果均不可重试；URL 包装中的 EOF 则特意可重试。这一差异依赖 URL 特例先于根因解包。
- 空 `multi` 返回 `false`，避免空集合的 `all` 结果被误当作可重试；非空组合必须所有直接子项都可重试。
- URL 取消判断使用区分大小写的消息包含匹配；字符串 `net/http: request canceled` 同时覆盖更长的 `...while waiting for connection` 形式。
- `dns`、`net`、`addr`、`op` 不因自身种类自动重试，必须能在 cause 树中找到指定 syscall kind。递归没有显式深度限制，并假定 `CommonError` 的值拥有关系不会形成引用环。
- HTTP 规则是排除式白名单：只有 400/404 不重试，其余状态都重试。gRPC `Unknown` 同样偏向重试，但为两个已知永久错误消息设置否决条件。
- 消息兜底可能受错误文案变化影响；新增结构化错误时应优先扩展 kind、code、ID 或 RPC 分支，而不是增加宽泛片段。
- 本分类器只返回布尔值，不记录拒绝原因。与 Go 版本不同，Rust 当前也不在判断为不可重试时写诊断日志。

## 并发与资源生命周期

所有函数都是同步、只读、无锁的纯分类过程；没有线程、异步任务、channel、事务、文件描述符或网络连接的创建与释放。静态切片只包含编译期字符串引用，可安全被多线程并发读取。

等待和重试生命周期由 `util.rs::Retry` 管理：闭包按顺序执行，失败后保留最后一个 `CommonError`，生产构建中后续尝试前等待 3 秒（测试配置为 10 毫秒），成功立即返回，重试耗尽或遇到永久错误则包装目的文本后返回最后错误。本文件不拥有该错误，也不控制休眠或取消。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/lightning/common/retry.go`](retry.go)，Rust 保留了 Go 的主要分类层次：消息片段列表、TiKV/PD 错误 ID、write-too-slow sentinel、URL 内因、网络/系统调用、MySQL/TiDB 编号、HTTP 状态、gRPC code、组合错误的全员可重试规则。Rust 独立测试 [`retry_test.rs`](retry_test.rs) 与 Go 测试 [`retry_test.go`](retry_test.go) 覆盖相同的主要正反例。

实现表示方式存在以下差异：

- Go 依靠具体错误类型、`errors.Cause`、`errors.As` 和 gRPC status 解码；Rust 将这些信息预先归一到 `CommonError` 的字符串 kind、编号、ID 和可选字段。因此 Rust 语义正确性还依赖上游错误转换是否填入一致字段。
- Go 的 `ErrWriteTooSlow` 是包级错误值，Rust 是每次构造 `CommonError` 的函数。
- Go 在遇到非取消的不可重试错误时记录类型和值；Rust 分类器没有对应日志副作用。
- Go 使用两个 URL 取消消息条目；Rust 用较短的共同前缀包含匹配，同时覆盖二者。
- Go 的网络类型匹配可通过标准库 unwrap 查找嵌套 syscall；Rust 的普通根因解包只沿第一个 cause，而网络 syscall 辅助函数遍历全部 `Causes`。

这些差异是当前代码事实，不表示可以任意简化 Go 逻辑。修改 Rust 规则时应逐项核对 Go 源码与两侧测试，尤其关注分类顺序和错误归一化字段。

## 扩展指南

- 新增结构化瞬态错误：优先在 `isSingleRetryableError` 对应层增加精确规则；TiKV/PD RFC 错误加入 `retryableErrorIDs`，MySQL/TiDB 错误同步编号白名单，gRPC 则更新 code 或 `Unknown` 否决条件。
- 新增消息兜底：仅当上游确实丢失结构化类型时修改 `retryableErrorMsgList`，使用尽可能稳定且窄的片段，确认 ASCII 小写转换后的匹配形式，并评估误判导致重复副作用的风险。
- 修改 URL 或 cause 处理：必须保持“顶层 URL 先处理”“组合错误全员通过”“带 Code 的错误不继续兜底”等顺序不变量；否则包装方式变化会改变结果。
- 每次规则变更都应同步独立的 `pkg/lightning/common/retry_test.rs`，不要把测试内嵌到生产源文件；若目标是继续保持 Go 对齐，还要核对并按实际 Go 行为更新 `retry_test.go` 的同类用例。
- 若要改变尝试次数、间隔、not-found 优先级或错误包装，应修改并测试 `util.rs::Retry`，而不是把执行策略塞入本分类文件。
- 兼容性风险主要是永久错误被误判后重复写入，以及瞬态错误被拒绝后提前终止导入；性能风险主要来自 cause 深度和消息扫描，但当前列表很小。新增递归结构时还应保持 `CommonError` 无环这一值语义前提。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件查询显示 `retry.rs` 共 159 行，并被 `retry_test.rs`、`util.rs` 使用。
- RustCodeGraph 源码与符号查询：`node --file pkg/lightning/common/retry.rs`；`query IsRetryableError`、`query isSingleRetryableError`、`query ErrWriteTooSlow`、`query isRetryableURLInnerError`。
- RustCodeGraph 调用查询：`callees IsRetryableError --file pkg/lightning/common/retry.rs` 确认进入 `isSingleRetryableError`；`callees isSingleRetryableError --file ...` 确认三个辅助函数和错误 ID 表依赖。对应 `callers` 查询为空，文件级使用和生产消费点由后续检索补证。
- 已读实现与边界文件：`pkg/lightning/common/retry.rs`、`errors.rs` 中的 `CommonError` 定义、`lib.rs` 模块装配、`util.rs::Retry`、`Cargo.toml`。
- 已读对照与测试：`pkg/lightning/common/retry.go`、`retry_test.rs`、`retry_test.go`。测试覆盖 URL 无内因/EOF/取消、timeout 与 syscall、HTTP 400/404/500、KV 与 driver ID、MySQL 编号、gRPC code 和 `Unknown` 特例、普通错误、组合错误及消息片段。
- 生产引用补查：`rg` 检索 `pkg/lightning/**/*.rs` 中的主要符号，除定义和独立测试外只发现 `util.rs` 导入 `IsRetryableError` 并在 `Retry` 中调用。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构以任务指定命令校验恰有十一个固定二级章节，并人工复核不存在测试内嵌建议、整段源码复制或超出直接证据的支持声明。
