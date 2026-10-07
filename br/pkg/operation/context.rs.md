# `br/pkg/operation/context.rs`

## 文件定位

[`context.rs`](context.rs) 是独立 library crate `astersql-br-pkg-operation` 的主体实现。根工作区 [`Cargo.toml`](../../../Cargo.toml) 将 `br/pkg/operation` 列为 member；本目录的 [`Cargo.toml`](Cargo.toml) 以 [`lib.rs`](lib.rs) 为 crate 根，`lib.rs` 通过 `#[path = "context.rs"] pub mod context` 挂载本文件并用 `pub use context::*` 再导出全部公开符号。Cargo metadata 将它标记为 Go 包 `br/pkg/operation` 的 Rust 移植，唯一外部依赖是启用 `v4` feature 的 `uuid`。

它定义一次 BR 命令的运行身份、启动时刻和运维 hint，并能把这些状态转换成外部存储锁所需的元数据。需要注意当前接线边界：仓库 Cargo 清单搜索没有发现其他 crate 对 `astersql-br-pkg-operation` 的依赖，Rust 生产目录也没有直接调用本 crate 的 `NewContext`；[`br/pkg/restore/log_client/client.rs`](../restore/log_client/client.rs) 使用的是该 crate 自己 [`stubs.rs`](../restore/log_client/stubs.rs) 内的 `operation` 模块，不是本文件。因此这里是可独立测试的真实移植实现，但现有 Rust BR 主链尚不能据此认定已经接入它。

## 核心职责

1. `NewContext` 为一次命令生成 UUID v4、记录当前 `SystemTime`，并输出包含操作 ID、启动时间、主机、进程号和命令名的启动日志。
2. `Context::SetHintField` 维护有插入顺序的 hint 键值列表，支持新增、同键覆盖和以空值删除，并记录 resolved/changed 日志。
3. `Context::HintFields` 返回快照，配合 `Context: Clone` 保证调用方和 worker 副本不会通过可变引用改写原上下文。
4. `Context::LockMeta` 校验上下文和锁资源类型，生成 `OwnerID`、`LockType` 与便于人工排障的 `Hint`。
5. `format_time_rfc3339`、`go_zero_time`、`quote_go_string` 和公历换算函数复现 Go `time.Time{}`、`time.RFC3339`、`strconv.Quote` 所需的跨语言格式语义。
6. 线程本地日志捕获 API 为独立 Rust 测试提供 zap observer 等价断言；未捕获时 `emit_log` 把稳定的结构化文本写到标准错误。

本文件不获取、续租或释放真实分布式锁，也不访问对象存储；`LockMeta` 只构造锁输入值。

## 主要符号

- `HintField { Key, Value }`：可克隆、可比较的公开 hint 键值。字段名沿用 Go 命名以保持移植 API 形状。
- `Context { OperationID, StartedAt, hintFields }`：操作上下文。前两项公开，`hintFields` 私有；`Default` 使用空 ID、Go 零时间和空 hint，而不是 Unix epoch。
- `LockResourceType(&'static str)` 与四个常量：分别表示 `log-truncate-exclusive`、`migration-read`、`migration-write`、`migration-append`。元组字段公开，调用方也能构造空值或自定义静态类型，最终由 `LockMeta` 拒绝空值。
- `LockMetaInput { OwnerID, LockType, Hint }`：本 crate 自定义的锁元数据输出结构；它不是 Go 侧 `pkg/objstore.LockMetaInput` 的 Rust 类型别名，也未在本文件内执行持久化。
- `NewContext(&str) -> Result<Context, String>`：生成 UUID 与当前时间并记录启动日志。当前 `uuid::Uuid::new_v4()` 是无错误返回，函数体没有 `Err` 分支，`Result` 主要保留 Go 构造函数的接口形状。
- `Context::HintFields`：克隆内部 `Vec<HintField>`；外部修改返回值不会回写上下文。
- `Context::SetHintField`：空 key 直接忽略；未初始化上下文拒绝非空 value；空 value 删除已有 key；非空 value 更新或追加。值发生变化时先记 warn，所有成功非空写入再记 info。
- `Context::LockMeta`：按 ID、启动时间、资源类型的顺序做前置校验，再调用私有 `lockHint`。
- `format_time_rfc3339` / `time_utc`：前者把 `SystemTime` 格式化为不含亚秒、固定 `Z` 的 UTC RFC3339；后者是公开的日历时间构造辅助，当前主要供测试夹具使用。
- `CapturedLog`、`LogCaptureGuard`、`begin_log_capture`、`captured_logs`、`filter_captured_message`：线程本地测试日志设施。这些符号没有 `#[cfg(test)]`，所以当前也是普通库的公开 API，虽然设计用途是测试。
- `emit_log`、`render_production_log`、`hostname`、`hostname_from_command`、`quote_go_string`、`civil_from_days`、`days_from_civil`：模块私有的日志、主机名、转义和时间转换实现。

本文件没有 trait、enum、泛型或条件编译项。

## 执行流程

创建与日志流程如下：

1. 命令边界调用 `NewContext(command)`；函数以 `Uuid::new_v4()` 生成 `OperationID`，以 `SystemTime::now()` 填写 `StartedAt`。
2. `hostname` 运行操作系统 `hostname` 命令；启动失败、退出状态非零或去除空白后为空都回退为 `"unknown"`。它刻意不读 `HOSTNAME`/`COMPUTERNAME` 环境变量。
3. `emit_log` 组装 `CapturedLog`。当前线程若已经调用 `begin_log_capture`，记录被压入线程本地 `Vec`；否则经 `render_production_log` 格式化后用 `eprintln!` 输出。
4. 启动日志固定为 `BR operation started`，字段包括 `operation_id`、RFC3339 启动时间、host、pid 和仅用于日志的 command；command 不存入 `Context`。

hint 与锁元数据流程如下：

1. `SetHintField` 先过滤空 key，并通过 `isInitialized` 要求非空 ID 且启动时间不等于 Go 零时间。删除请求使用空 value，可以在未初始化上下文上安全成为无操作。
2. `hintFieldIndex` 线性寻找同 key。删除、覆盖和追加前都会 clone 当前向量；覆盖为不同值时输出 `BR operation hint field changed`，随后写入新值；每次成功非空写入都输出 `BR operation hint field resolved`。
3. 需要加锁时，调用方把资源常量和可选 detail 交给 `LockMeta`。校验通过后，OwnerID 复制操作 ID，LockType 复制资源字符串。
4. `lockHint` 以启动时间开头，按插入顺序追加所有非空 hint，最后把非空 detail 以 Go `strconv.Quote` 风格转义并加上 `detail=`。各段用单个空格连接。

时间格式化先把 `SystemTime` 转为 Unix 秒；epoch 前含亚秒的值向前一整秒取整，以匹配 Go 无小数 RFC3339。之后用欧几里得除法拆日与日内秒，再由 Howard Hinnant 公历算法完成日序号和年月日转换。

## 数据与状态

`Context` 的身份状态由 `OperationID` 与 `StartedAt` 共同决定；只设置其中一个仍未初始化。默认启动时间是 `UNIX_EPOCH - 62_135_596_800s`，对应 Go `time.Time{}` 的公元 1 年零时。Unix epoch 和 epoch 前一秒都是有效的非零时间，独立测试对此有明确回归覆盖。

`hintFields` 是按首次插入顺序排列的 `Vec`，同 key 更新不改变位置，删除后重新添加会进入末尾。查找和删除是 O(n)，快照、写入和删除还会复制整个向量，因此适合少量运维字段，不适合高频、大规模键集合。Rust 的 `String`/`Vec` 本来已经具有深 clone 语义；实现仍在变更前显式 clone，以贴近 Go `slices.Clone` 后再修改切片的意图。

日志捕获状态是 `thread_local RefCell<Option<Vec<CapturedLog>>>`。`None` 表示走生产 stderr，`Some` 表示捕获。`begin_log_capture` 保存旧 sink 并安装空缓冲；`LogCaptureGuard::drop` 恢复旧值，所以同线程嵌套捕获可还原外层缓冲。`captured_logs` 返回整个缓冲克隆，不借出内部状态。

`LockMetaInput` 和 `CapturedLog` 都是拥有所有权的值，不持有 `Context` 引用。生成元数据后再修改上下文不会影响已经返回的锁输入。

## 依赖与调用关系

crate 内部上游是 [`lib.rs`](lib.rs)：它无条件挂载并再导出 `context`，测试构建时另外挂载 [`context_test.rs`](context_test.rs) 与 [`parity_test.rs`](parity_test.rs)。两份 Rust 测试直接调用 `NewContext`、`SetHintField`、`HintFields` 和 `LockMeta`；`context_test.rs` 还使用日志捕获、`time_utc` 和生产日志子进程探针。

仓库搜索到的 `br/pkg/restore/log_client/client.rs::SetOperationContext`、`setOperationContextRestoreID` 以及测试辅助 `NewOperationContext` 操作的是 `crate::stubs::operation::Context`。该 stub 与本文件具有近似名字但不同 API（例如测试辅助读取 `GetHintField`），且相应 Cargo manifest 没有依赖本 crate，故这些位置只能说明未来可能的集成语义，不能列为本文件的直接调用边。

下游仅包括标准库与 `uuid`：UUID 负责随机操作 ID；`SystemTime`/`UNIX_EPOCH` 提供时钟；`std::process::Command` 解析主机名；进程 ID来自 `std::process::id`；`RefCell` 和 `thread_local!` 管理测试日志缓冲。当前没有日志框架、对象存储或 objstore crate 依赖，真实锁持久化必须由上层另行完成。

RustCodeGraph 文件节点报告 `context.rs` 有 30 个符号并“被 15 个文件使用”，但其候选包含大量仓库其他 `Context` 同名符号；对精确 Rust `NewContext` 的 `callers`/`callees` 查询没有返回边。因此调用结论以 Cargo 反向依赖搜索和带限定路径的源码调用点为准，不把图的同名 blast radius 当成真实接线。

## 错误处理与边界

- `NewContext` 的签名允许错误，但当前 UUID、时钟、主机名路径都不会向调用方返回错误：主机名失败降级为 `unknown`，UUID 构造无错误。它与 Go `uuid.NewRandom` 失败时 annotate 并返回 error 的能力不完全等价。
- `SetHintField` 对空 key、未初始化上下文的非空 value、删除不存在的 key 均静默返回；这些不是错误。相同值的覆盖不记 changed warn，但仍记 resolved info。
- hint 的 key/value 直接拼入 `lockHint`，只有 detail 使用 `quote_go_string`。调用方若在 key/value 中放空格或 `=`，Hint 文本可能不再适合简单按空格/等号解析；当前契约把它定位为人工排障文本，而非可逆编码。
- `LockMeta` 返回普通 `String` 错误，依次为缺少 operation ID、缺少启动时间、缺少资源类型；没有结构化错误分类。它不验证 UUID 格式、自定义资源字符串、hint 长度或字符集。
- `format_time_rfc3339` 把秒数强制转换为 `i64`，并以四位年份格式输出；现实系统时间范围内安全，但没有为极端 `SystemTime` 溢出或 RFC3339 年份范围提供显式错误。
- `quote_go_string` 对有效 UTF-8 `&str` 实现 Go 风格控制字符和 Unicode 转义；Rust 输入无法表示 Go string 中任意非法 UTF-8 字节，所以不是所有 Go 字节串的完整等价实现。
- `emit_log` 使用 `RefCell`；同线程在持有该 cell 可变借用时重入会 panic。当前路径在释放借用后才输出，不存在内部重入。
- `eprintln!` 和 `hostname` 子进程不通过可注入接口；日志写失败没有 Result，主机名查询会产生一次同步进程开销。

## 并发与资源生命周期

本模块不启动后台任务、不持有真实锁，也没有网络或存储句柄。`Context` 没有内部同步原语；可变 hint 操作要求 `&mut self`，跨线程共享并修改时需由上层提供锁。`Context: Clone` 会深复制字符串和 hint 向量，适合在创建完成后把值快照分发给 worker；各副本之后互不影响。

日志捕获按线程隔离，所以并行测试不会争用一个全局缓冲。但它也意味着在一个线程安装 capture 后，其他线程产生的日志不会被捕获；若生产代码未来把操作转交工作线程，现有测试辅助不能自动跨线程追踪。`LogCaptureGuard` 依赖 RAII 恢复旧 sink，即使测试 panic 展开也会执行；若进程 abort 则没有清理机会。

主机名命令和 stderr 写入均同步发生在调用线程。`NewContext` 的操作 ID/时间先完成，再查询 hostname；主机名查询变慢会延迟构造返回，但不会留下需要回收的后台资源。锁元数据只是值构造，生命周期由调用方所有权决定。

## 与 Go 版本的对应关系

直接对照文件是 [`context.go`](context.go)，测试意图来自 [`context_test.go`](context_test.go)。公开业务形状基本一一对应：`Context`、`HintField`、四个锁资源常量、`NewContext`、`HintFields`、`SetHintField`、初始化判断、线性索引、`LockMeta` 和 hint 拼接都保留同样的分支顺序、日志消息与字段语义。Rust 独立测试把 Go 的主要用例移植到 [`context_test.rs`](context_test.rs)，[`parity_test.rs`](parity_test.rs) 另行检查覆盖、删除、空 key、LockMeta 和空资源契约。

已验证差异如下：

- Go 使用 `github.com/google/uuid.NewRandom()` 并可能返回错误；Rust `Uuid::new_v4()` 当前不可失败，但仍保留 `Result<_, String>`。
- Go 用 `os.Hostname` 与 PingCAP zap logger；Rust 执行 `hostname` 子进程，并在没有测试 capture 时直接向 stderr 输出自行渲染的文本。测试只证明消息和关键字段，不证明与 zap 的编码、时间字段类型或日志路由完全一致。
- Go `LockMeta` 返回真实 `objstore.LockMetaInput`；Rust返回本地同名语义结构，当前没有 objstore 依赖或锁文件写入接线。
- Go `time.Time` 保留纳秒和 location，启动日志的 `zap.Time` 可记录完整时间；Rust持有 `SystemTime`，但字符串输出与 lock hint 固定 UTC、无亚秒。锁 hint 与 Go `time.RFC3339` 的无亚秒目标一致。
- Go 通过 slice header 的值复制可能共享 backing array，因此变更前 `slices.Clone` 很重要；Rust `Clone` 已深复制 `Vec`，额外 clone 主要是对齐操作意图而非解决 Rust 别名。
- Rust增加了公开的日志捕获、RFC3339、UTC 时间夹具函数和本地公历/quote 实现；这些不是 Go 包的业务公开面。

## 扩展指南

- 新增上下文字段时，从 `Context`、`Default`、`NewContext` 和相关日志/锁 hint 的最窄位置接入，先判断它是仅观测字段还是持久锁协议字段。同步更新 [`context_test.rs`](context_test.rs) 和 Go 的 [`context_test.go`](context_test.go)，测试必须继续放在独立文件而非生产源内。
- 新增 hint 规则时必须保持空 key、未初始化写入、空值删除、插入顺序、同值覆盖日志和 clone 独立性。若 key/value 要进入机器解析协议，应先引入明确转义或结构化编码，不能只改变 `lockHint` 拼接而破坏既有锁文件可读性。
- 新增锁资源类型时在 `LockResourceType` 常量区增加与 Go 完全相同的字面量，并补 `LockMeta`/parity 测试；还需检查真正的对象存储锁消费者是否接受该类型。当前本 crate 未接入生产锁链，接线工作需要同时替换上层 stub 和 Cargo 依赖，不能仅修改本文件。
- 修改时间或 quote 算法时应增加 epoch、epoch 前含亚秒、Go 零时间、闰年、控制字符、引号、反斜杠和非 BMP Unicode 的独立回归，并用 Go fixture 比较精确字符串。
- 若把测试日志设施移到 `#[cfg(test)]`，需要同步调整 crate 的公开 API 和所有测试导入；若改用正式日志 facade，则要保留线程隔离、嵌套 guard 恢复、生产输出以及 Go 消息/字段名契约。
- 若希望 `NewContext` 的 `Result` 有实际意义，应明确 UUID/hostname/日志中哪些失败向上传播，并与 Go 错误语义对齐；不要只在 Rust 侧随意新增失败条件。
- 性能优化应以 hint 数量证据为基础。若改为 map，需要额外保存稳定顺序，并验证克隆快照与锁 hint 输出不漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/operation` 列出 Rust/Go 实现、独立测试和 crate 根；`node --file br/pkg/operation/context.rs` 分段读取全部 439 行并确认 30 个符号。`query NewContext` 同时定位 Rust/Go 对照；精确 `callers`/`callees` 未返回边，故未据此宣称生产接线。
- Rust 源与 crate 边界：[`context.rs`](context.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml) 以及根 [`Cargo.toml`](../../../Cargo.toml)。本目录没有 `doc.go`。
- 接线复核：仓库范围搜索 `astersql-br-pkg-operation`/`astersql_br_pkg_operation`、`NewContext`、`SetHintField` 和 `LockMeta`；并读取 [`br/pkg/restore/log_client/client.rs`](../restore/log_client/client.rs)、[`stubs.rs`](../restore/log_client/stubs.rs) 与其测试辅助，确认它们当前使用本地 stub 而非本 crate。
- Go 对照：[`context.go`](context.go) 及引入该行为的提交 `807326b066`，核对 UUID 错误、hostname、zap 字段、切片 clone、LockMeta 校验和 `strconv.Quote`。
- 测试证据：[`context_test.rs`](context_test.rs) 覆盖初始化、hint 全部分支、clone/快照独立性、锁元数据、Go 零时间与 epoch、quote、生产日志和 OS hostname；[`parity_test.rs`](parity_test.rs) 覆盖公开契约；[`context_test.go`](context_test.go) 是原始 Go 测试意图。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前执行任务指定的 11 标题结构检查，并人工复核链接、真实接线边界和 Go/Rust 差异。
