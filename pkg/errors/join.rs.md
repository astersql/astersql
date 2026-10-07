# `pkg/errors/join.rs`

## 文件定位

本文件属于 `astersql-errors` crate。crate 根由 [`pkg/errors/Cargo.toml`](Cargo.toml) 的 `[lib] path = "mod.rs"` 指向 [`mod.rs`](mod.rs)，后者声明私有 `join` 模块并以 `pub use join::Join` 对外暴露唯一公开入口。它位于错误值基础设施的“多原因聚合”层：[`core.rs`](core.rs) 提供线程安全、可克隆的 `SharedError` 载体，[`group.rs`](group.rs) 定义 `ErrorGroup`、组成员提取和深度遍历，而本文件负责构造一个具体的组错误。

该能力不参与 SQL 解析或执行本身，而是供上层在一次操作产生多个错误时保存全部原因。例如 [`pkg/executor/adapter.rs`](../executor/adapter.rs) 的 `joinRecordSetErrors` 聚合记录集生命周期中的错误；[`br/pkg/conn/conn.rs`](../../br/pkg/conn/conn.rs)、[`br/pkg/conn/util/util.rs`](../../br/pkg/conn/util/util.rs) 和 [`br/pkg/utils/retry.rs`](../../br/pkg/utils/retry.rs) 聚合多次重试失败。它因此是跨子系统的错误组合原语，而不是重试策略或日志策略的实现者。

## 核心职责

- `Join` 接受 `&[Option<SharedError>]`，丢弃 `None`，保留其余错误的输入顺序；没有有效错误时返回 `None`。
- 非空结果始终包装成新的 `JoinError` 组，包括仅有一个有效错误的情况；实现不会将嵌套的 `JoinError` 自动扁平化。
- `JoinError::fmt` 按顺序展示各子错误，并只在相邻子错误之间写入换行符。
- `ErrorGroup::Errors` 暴露组成员，使 [`group.rs`](group.rs) 中的 `Errors`、`WalkDeep` 以及间接使用 `WalkDeep` 的查找逻辑能够进入每个独立原因。

本文件不捕获新堆栈、不增加上下文文本、不判定错误类别，也不执行日志记录；这些职责分别由同 crate 的 `stack.rs`、`wrap.rs`、`normalize.rs` 或调用方承担。

## 主要符号

- `struct JoinError { errors: Vec<SharedError> }`（私有）：保存过滤后的有序子错误。私有性阻止外部依赖具体表示，调用者只看到 `SharedError` 及 `ErrorGroup` 行为。
- `impl fmt::Display for JoinError::fmt`（私有实现）：逐项调用子错误的 `Display`；第一个元素前不写分隔符，后续元素前写一个 `\n`。子错误格式化失败时用 `?` 原样传播 `fmt::Error`。
- `impl StdError for JoinError`：把组合对象接入 Rust 标准错误 trait，但没有覆盖 `source()`；多原因关系不通过标准库的单一 `source` 链表达。
- `impl ErrorGroup for JoinError::Errors`：克隆内部 `Vec<SharedError>`。这里克隆的是 `SharedError` 的 `Arc` 句柄，不会复制底层错误对象。
- `pub fn Join(errors: &[Option<SharedError>]) -> Option<SharedError>`：唯一公开 API。它用 `iter().flatten().cloned().collect()` 完成过滤与所有权获取，空向量提前返回，否则调用 `SharedError::new_group` 同时保存标准错误视图和组视图。

文件没有模块级常量、枚举、宏、条件编译项或公开类型；公开面仅为 `Join`，并经 `pkg/errors/mod.rs` 再导出。

## 执行流程

1. 调用者把每个可能存在的错误表示为 `Option<SharedError>`，按希望保留的诊断顺序组成切片并调用 `Join`。
2. `Join` 遍历切片：`flatten` 去掉 `None`，`cloned` 增加每个 `SharedError` 内部 `Arc` 的引用计数，`collect` 形成拥有所有权的 `Vec`。
3. 若过滤结果为空，函数返回 `None`，不分配 `JoinError`。
4. 若至少保留一个错误，`SharedError::new_group(JoinError { errors })` 创建共享组错误；`new_group` 同时缓存底层标准错误对象和 `ErrorGroup` trait 对象。
5. 展示组合错误时，`JoinError::fmt` 顺序委托各子错误的 `Display`，输出 `err1\nerr2` 形式的文本。
6. 需要检查全部原因时，`group::Errors` 读取缓存的组视图并调用 `JoinError::Errors`；`group::WalkDeep` 则先访问当前节点和单原因链，再按该向量顺序递归访问组成员。嵌套组由遍历阶段递归展开，而不是在 `Join` 构造时扁平化。

## 数据与状态

`JoinError` 的全部持久状态只有 `Vec<SharedError>`。向量在构造后不再改变，因此成员顺序是核心不变量：过滤 `None` 不得重排剩余错误，展示和组遍历都依赖这一顺序。空向量不会进入 `JoinError`；因此任何实际构造出的组至少有一个成员。

`SharedError` 在 [`core.rs`](core.rs) 中以 `Arc<dyn Error + Send + Sync + 'static>` 保存底层错误，并为组错误额外缓存 `Arc<dyn ErrorGroup>`。所以输入错误和 `Errors()` 返回值共享相同底层对象；[`tests/group_join_test.rs`](tests/group_join_test.rs) 用 `ptr_eq` 验证 `err1`、`err2` 经聚合和取回后仍保持身份。每次 `Errors()` 都会新建一个向量并克隆句柄，其时间和额外空间均为 O(n)；展示同样为 O(n) 加所有子错误格式化成本。

## 依赖与调用关系

直接标准库依赖是 `std::error::Error` 与 `std::fmt`；crate 内直接依赖是 `super::SharedError` 和 `super::ErrorGroup`。`Join` 的关键下游调用是 `SharedError::new_group`，`fmt` 的关键下游调用是每个成员的 `Display`，`Errors` 的关键操作是克隆成员向量。本文件自身不直接使用 `pkg/errors/Cargo.toml` 中的 `backtrace` 或 `serde` 依赖，也没有 feature 条件。

RustCodeGraph 将 `pkg/errors/join.rs` 识别为含 5 个符号的文件，并列出直接相关测试 `pkg/errors/tests/group_join_test.rs`、`wrap_test.rs`、`api_parity_test.rs`。精确 callers/callees 查询未返回边，仓库 `rg` 补证的实际调用点包括：

- [`pkg/executor/adapter.rs`](../executor/adapter.rs) 的 `joinRecordSetErrors`：把已有错误转为 `Some` 后交给 `Join`，对应 Go `pkg/executor/adapter.go` 中嵌套的 `errors.Join`。
- [`br/pkg/conn/conn.rs`](../../br/pkg/conn/conn.rs) 与 [`br/pkg/conn/util/util.rs`](../../br/pkg/conn/util/util.rs)：收集连接/重试错误，非空时返回组合错误，异常空集合时使用领域占位错误兜底。
- [`br/pkg/utils/retry.rs`](../../br/pkg/utils/retry.rs)：取消或耗尽重试时返回此前按发生顺序收集的全部失败。
- [`br/pkg/utils/misc.rs`](../../br/pkg/utils/misc.rs)：清理失败在前、既有错误在后地组合错误，表明调用方把顺序用于表达诊断优先级。

## 错误处理与边界

- `&[]`、`[None]` 或全为 `None` 的切片返回 `None`；调用方必须显式处理“没有错误”的情况。
- 单个有效错误仍会生成组包装，不直接返回原 `SharedError`。其显示文本相同，但组身份与可遍历结构不同。
- 中间的 `None` 被静默过滤；它不会产生空行。有效错误的相对顺序保持不变。
- 子错误文本可自行包含换行；本实现不会转义或标记边界，因此组合文本适合展示，却不应被反向解析为可靠的成员列表。结构化检查应使用 `Errors`/`WalkDeep`。
- 嵌套 `Join` 不会在构造时扁平化。外层 `Errors` 返回直接成员；`WalkDeep` 才会递归进入内层组。
- `JoinError` 的标准 `source()` 为默认的 `None`，多原因只能通过本 crate 的 `ErrorGroup` 视图访问。若扩展标准错误互操作，需要保留这一单链与多原因模型的区别。
- 格式化过程中任一成员返回 `fmt::Error` 时立即终止并传播；已写入 formatter 的前缀不会回滚。

## 并发与资源生命周期

本文件没有锁、通道、异步任务、线程、事务、文件句柄或网络资源。生命周期从 `Join` 克隆输入 `SharedError` 句柄开始；组合对象及其成员由 `Arc` 引用计数共同管理，最后一个引用释放时自动销毁。

`ErrorGroup` 要求实现者满足 `Send + Sync + 'static`，`SharedError` 的底层动态错误也具有相同边界，因此非空 `Join` 结果可在线程之间安全共享。安全性来自构造后不可变的 `Vec` 与线程安全的成员类型；本文件不提供并发写入，也不规定多个调用者观察或记录错误的时序。

## 与 Go 版本的对应关系

仓库 `go.mod` 锁定 `github.com/pingcap/errors v0.11.5-0.20260508054701-306e305bcf41`；该模块缓存中的 `join.go` 是直接 Go 对照。两边都丢弃 nil/`None`、全空返回 nil/`None`、保持输入顺序、用单个换行连接文本，并在单个非空输入时仍创建组合对象。Go 的 `joinError.Unwrap() []error` 对应 Rust 的 `JoinError: ErrorGroup` 加 `group::Errors`/`WalkDeep`，而不是 Rust 标准库的 `Error::source()`。

表示层存在三点差异：Go API 是可变参数 `Join(errs ...error)`，Rust API 因所有权而接收 `&[Option<SharedError>]`；Go `Unwrap` 返回内部切片，Rust `Errors` 返回克隆的 `Vec<SharedError>`；Go 测试还检查返回切片 `len == cap`，Rust 的公开契约没有暴露内部向量容量，因此不应把该内存布局断言移植成行为要求。

本地 Rust 对照测试 [`tests/group_join_test.rs`](tests/group_join_test.rs)覆盖 Go `join_test.go` 的全空、过滤、顺序、展示与成员身份语义，并增加了本 crate 的嵌套组遍历和短路检查。[`tests/wrap_test.rs`](tests/wrap_test.rs) 验证包装后的 `Find` 能进入组成员；[`tests/api_parity_test.rs`](tests/api_parity_test.rs) 验证 `Join` 位于公开 API 面。

## 扩展指南

- 修改过滤、顺序、单元素或格式化行为时，首先对照当前 Go 依赖版本的 `join.go`，并同步更新独立测试 [`tests/group_join_test.rs`](tests/group_join_test.rs)；不要把测试内嵌回生产文件。
- 修改组的可发现性或嵌套语义时，需要联合检查 [`group.rs`](group.rs) 的 `Errors`/`WalkDeep`、[`core.rs`](core.rs) 的 `SharedError::new_group`/`error_group`，以及 [`tests/wrap_test.rs`](tests/wrap_test.rs) 中 `Find` 对组成员的行为。
- 新增公开 API 必须在 [`mod.rs`](mod.rs) 再导出，并在 [`tests/api_parity_test.rs`](tests/api_parity_test.rs) 增加编译期使用证据；若新增外部 crate 依赖，还需同步 `pkg/errors/Cargo.toml`。
- 不应为性能方便改变输入顺序或提前扁平化嵌套组，因为展示、重试诊断优先级和 `WalkDeep` 访问序列均可观察。大量成员场景要关注两次线性工作：构造时克隆/收集，调用 `Errors()` 时再次克隆句柄向量。
- 若要加强与 Rust 标准错误生态的互操作，应先明确多原因如何映射到只能返回单一引用的 `source()`；不能用第一个成员冒充完整原因集合而隐藏其余错误。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标区域已索引；`files --filter pkg/errors` 列出 `join.rs` 及独立测试；`node --file pkg/errors/join.rs --offset 1 --limit 240` 核对 48 行源码和 5 个符号；`query Join --kind function` 定位 `fmt`、`Errors`、`Join` 及 `group_join_test`。精确 `callers/callees` 对目标符号未返回结果，因此没有把空图结果当作“无调用者”。
- 生产源码：[`join.rs`](join.rs)、[`core.rs`](core.rs)、[`group.rs`](group.rs)、[`mod.rs`](mod.rs)、[`pkg/executor/adapter.rs`](../executor/adapter.rs)、[`br/pkg/conn/conn.rs`](../../br/pkg/conn/conn.rs)、[`br/pkg/conn/util/util.rs`](../../br/pkg/conn/util/util.rs)、[`br/pkg/utils/retry.rs`](../../br/pkg/utils/retry.rs)、[`br/pkg/utils/misc.rs`](../../br/pkg/utils/misc.rs)。
- crate/依赖：[`pkg/errors/Cargo.toml`](Cargo.toml) 与仓库根 `Cargo.toml`/`go.mod`；前者确认 crate 边界和直接依赖，后者确认工作区引用及 Go 对照版本。
- Go 对照：本地 Go module cache 中 `github.com/pingcap/errors@v0.11.5-0.20260508054701-306e305bcf41/join.go`、`join_test.go`，以及仓库 [`pkg/executor/adapter.go`](../executor/adapter.go) 的实际使用。
- 独立 Rust 测试：[`tests/group_join_test.rs`](tests/group_join_test.rs)、[`tests/wrap_test.rs`](tests/wrap_test.rs)、[`tests/api_parity_test.rs`](tests/api_parity_test.rs)。本任务是纯文档分析，按计划未运行 Cargo；验证限于静态源码、调用搜索、Go 对照和文档结构检查。
