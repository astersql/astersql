# `pkg/errors/group.rs`

## 文件定位

`group.rs` 是 `astersql-errors` crate 的多原因错误抽象与遍历实现。该 crate 由 [`pkg/errors/Cargo.toml`](Cargo.toml) 定义，库入口是 [`pkg/errors/mod.rs`](mod.rs)；入口将本文件的 `ErrorGroup`、`Errors`、`WalkDeep` 全部公开再导出。它位于单因错误链工具 [`pkg/errors/wrap.rs`](wrap.rs) 与多错误构造器 [`pkg/errors/join.rs`](join.rs) 之间：`wrap.rs::Unwrap` 提供纵向 cause 边，`join.rs::JoinError` 实现本文件的横向组边。

本文件不负责创建具体错误文本、捕获栈或启动并发任务。它解决的是错误已经形成后，如何统一表示“一项操作包含多个独立失败”，以及如何以确定顺序遍历混合的 cause 链和错误组。固定 Go 来源是 `github.com/pingcap/errors` 提交 `306e305bcf41` 的 `group.go`；仓库当前没有同路径 Go 文件，Rust 对等性清单在 [`pkg/errors/tests/api_parity_test.rs`](tests/api_parity_test.rs) 中固定了该来源提交。

## 核心职责

1. `ErrorGroup` 定义多原因错误的最小协议：具体错误仍是标准错误，同时可返回全部直接子错误。
2. `Errors` 将“可能是组的错误”归一成 `Vec<SharedError>`：组返回直接子项，普通错误返回包含自身的单元素列表。
3. `WalkDeep` 深度优先遍历混合错误图：先访问当前节点，再沿单一 `Unwrap` cause 递归，最后按 `ErrorGroup::Errors` 的顺序递归各子项；访问器返回 `true` 时立即短路。

这三个职责刻意不做扁平化、去重或错误分类。调用方可以用 `Errors` 只展开一层，也可以用 `WalkDeep` 穿过任意层的 cause 与 group 组合；具体例子分别见 `pkg/errctx/context.rs::Context::HandleError` 和 `pkg/errors/wrap.rs::Find`。

## 主要符号

- `pub trait ErrorGroup: StdError + Send + Sync + 'static`：公开 trait。实现者必须同时满足标准错误展示/来源协议、可跨线程共享的 `Send + Sync` 约束和 `'static` 生命周期。唯一方法 `fn Errors(&self) -> Vec<SharedError>` 返回直接子错误的拥有型列表。当前 crate 内的真实实现是 `pkg/errors/join.rs::JoinError`；测试实现包括 `tests/group_join_test.rs::TestGroup` 和 `tests/api_parity_test.rs::ApiGroup`。
- `pub fn Errors(error: &SharedError) -> Vec<SharedError>`：公开的一层展开函数。它通过 `SharedError::error_group()` 取得构造时缓存的 group 视图，存在时调用 trait 方法，否则克隆 `SharedError` 自身形成单元素向量。
- `pub fn WalkDeep<F>(error: Option<&SharedError>, visitor: F) -> bool where F: FnMut(&SharedError) -> bool`：公开的深度遍历入口。`None` 返回 `false` 且不调用访问器；`true` 表示访问器曾要求提前结束，`false` 表示完整走完或输入为空。
- `walk`：`WalkDeep` 内部泛型递归函数，不对 crate 外公开。它实现“当前节点 → 单因 cause → 组子项”的固定优先级，并把短路信号逐层向上传播。

本文件没有模块级常量、结构体、枚举、宏或条件编译项。

## 执行流程

`Errors` 的执行只有一个分支：先调用 `SharedError::error_group`。若 `SharedError` 是经 `SharedError::new_group` 构造，缓存的 trait 对象会提供直接子项；否则返回 `vec![error.clone()]`。因此调用方始终得到非空结果，但组实现自身仍可合法返回空向量，文件中没有额外校验。

`WalkDeep` 的流程如下：

1. 外层用 `Option::is_some_and` 检查入口；`None` 直接得到 `false`。
2. `walk` 先把当前错误借给 `visitor`。若回调返回 `true`，立即返回，不读取 cause 或组子项。
3. 调用 `wrap.rs::Unwrap(Some(error))` 获取至多一个直接 cause；存在时递归处理完整 cause 子树。cause 子树短路会直接结束整个遍历。
4. cause 子树未短路后，调用 `SharedError::error_group`。若当前节点也是组，则按 `group.Errors()` 返回的向量顺序逐个递归。
5. 所有后代都未要求停止时返回 `false`。

`tests/group_join_test.rs::walk_deep_and_join_match_go` 给出了可复核顺序：包装后的嵌套 `Join` 会先访问外层包装和其 cause 组，再进入第一个子错误的包装链，之后访问嵌套组及其子项；命中 `b1` 返回 `true` 后不会再访问兄弟 `b2`。

## 数据与状态

本文件自身没有全局状态或可变字段。核心数据是 `SharedError`：`pkg/errors/core.rs` 显示它用 `Arc<DynError>` 持有底层标准错误，并用 `Option<Arc<dyn ErrorGroup>>` 缓存同一对象的组视图。`SharedError::new_group` 同时填充两份 `Arc` 视图，`error_group` 只借用缓存，不做运行时 downcast。

`ErrorGroup::Errors` 和顶层 `Errors` 都返回拥有型 `Vec<SharedError>`；当前 `JoinError::Errors` 克隆内部向量，而 `SharedError::clone` 只增加 `Arc` 引用计数。因此遍历期间不会借用组内部容器，但每次展开会分配向量并克隆子项句柄。子项顺序完全由实现者返回的向量决定，`WalkDeep` 保留该顺序。

访问器是 `FnMut`，可以在遍历过程中累积顺序、计数或命中结果；状态归调用方所有，`WalkDeep` 只串行地可变借用它。

## 依赖与调用关系

直接标准库依赖只有 `std::error::Error`。crate 内直接依赖是 `SharedError` 与 `Unwrap`：前者承载错误所有权和 group 视图，后者提供单因链下一跳。`Cargo.toml` 没有为本文件引入专属第三方依赖或 feature；整个 crate 的运行时依赖只有 `backtrace` 与带 `derive` 的 `serde`，本文件均未直接使用。

下游构造关系为 `join.rs::Join` → `SharedError::new_group(JoinError)` → `JoinError: ErrorGroup`。直接消费关系包括：

- `wrap.rs::Find` 调用 `WalkDeep`，在第一个满足谓词的节点克隆结果并返回 `true` 短路；`tests/wrap_test.rs::find_searches_error_group_children` 验证它会进入组子项。
- `pkg/errctx/context.rs::Context::HandleError` 调用 `Errors`，逐项处理组错误并在首个必须返回的错误处停止。
- `br/pkg/conn/conn.rs::status_code` 用 `Errors` 展开最多两层组，优先提取子错误中的 gRPC 状态码。
- `br/pkg/utils/backoff.rs::BackoffStrategyImpl::NextBackoff` 用 `Errors` 取最后一个直接子错误作为退避判定依据。

RustCodeGraph 的目标文件节点记录了 `tests/api_parity_test.rs`、`tests/group_join_test.rs`、`wrap.rs` 等使用者；精确 `WalkDeep` 节点的调用轨迹包含 `errors_api_parity_test` 与 `walk_deep_and_join_match_go`。

## 错误处理与边界

- 空入口只由 `WalkDeep` 表达：`None` 返回 `false`。`Errors` 接受 `&SharedError`，类型层面排除了 Go 的 `nil error` 输入。
- 普通错误不会被误报为空组；`Errors` 返回自身的廉价克隆，并由 `ptr_eq` 测试确认仍指向同一底层对象。
- `WalkDeep` 不捕获访问器 panic，也不把访问失败转换为新的错误；回调控制流只有布尔短路。
- 本实现没有环检测或已访问集合。如果自定义组把自身或祖先重新作为子项，或 cause/group 组合形成环，递归不会终止，并可能导致栈溢出。实现 `ErrorGroup` 时必须保证可遍历图无环。
- 同一 `SharedError` 出现在多个子项或同时可经 cause 与 group 到达时会被重复访问；这是按边遍历而非按对象去重。
- 深度与错误链/组嵌套层数成正比，极深的外部错误图存在递归栈风险。当前实现也不限制组宽度。
- `ErrorGroup::Errors` 可以返回空向量；顶层 `Errors` 会原样返回空结果，`WalkDeep` 仍已访问组节点本身。调用方不能假设所有组至少有一个子项。

## 并发与资源生命周期

`ErrorGroup` 的 `Send + Sync + 'static` 上界与 `SharedError` 的 `Arc` 表示允许错误及其组视图跨线程传递和共享，但本文件不创建线程、锁、任务、通道或异步 future，也不协调产生这些错误的工作。并发任务如何收集错误属于具体组实现或上游执行器职责。

遍历是调用线程上的同步串行过程。`Errors` 返回拥有型克隆后，向量在调用结束时正常释放，`SharedError` 的底层对象在最后一个 `Arc` 句柄释放时销毁。`WalkDeep` 每处理完一个临时 cause 或 group 子项便按作用域释放该句柄；访问器只获得本次调用期间有效的借用，如需越过回调保存错误必须自行克隆。

虽然 trait 要求实现类型线程安全，`Errors(&self)` 仍可能由自定义实现读取内部同步状态；本文件不加锁，也不保证并发修改下的跨次调用快照一致性。扩展实现应自行定义并同步其子项快照语义。

## 与 Go 版本的对应关系

固定来源 `pingcap/errors@306e305bcf41/group.go` 同样定义 `ErrorGroup.Errors() []error`、一层展开函数 `Errors` 和深度遍历 `WalkDeep`。两版的关键语义一致：普通错误被包装成单元素集合；遍历先访问当前节点，再递归 `Unwrap` 的单因链，最后按序递归组子项；访问器返回 `true` 会结束全局遍历。

Rust 的类型映射有四点需要注意：

1. Go 用运行时接口断言识别任意 `ErrorGroup`；Rust 必须通过 `SharedError::new_group` 显式缓存 group trait 对象。仅用 `SharedError::new` 包装一个实现了 `ErrorGroup` 的类型不会被本文件识别为组。
2. Go 的 `error`/`nil` 映射为 Rust 的 `SharedError`/`Option<SharedError>`；所以 `WalkDeep(None, ...)` 对应 Go 的 nil 分支，而 `Errors` 没有空输入形态。
3. Go 返回 `[]error`，Rust 返回 `Vec<SharedError>` 并克隆共享句柄；这保留了拥有权安全和输入顺序。
4. Rust trait 额外要求 `StdError + Send + Sync + 'static`，把标准错误兼容性和跨线程安全变成编译期约束。

`tests/api_parity_test.rs` 的公开 API 清单明确记录来源提交并编译期验证 `ErrorGroup` 可实现；`tests/group_join_test.rs` 将 Go 的组、Join 和遍历意图合并为独立 Rust 回归测试。

## 扩展指南

新增具体错误组时，应让类型实现 `Display`、`Debug`、`StdError` 和 `ErrorGroup`，并必须用 `SharedError::new_group` 构造公开值；否则 `Errors`/`WalkDeep` 看不到组视图。`Errors` 应返回稳定、有业务意义的直接子项顺序，并避免把自身或祖先放回结果。若内部可变，先在实现内部取得一致快照，再返回拥有型向量，不要把锁或借用泄漏到遍历层。

修改遍历策略时，最可能改动 `WalkDeep::walk`，必须保持并明确评估三项兼容合同：当前节点是否先于后代、cause 是否先于 group、短路是否跨递归层传播。同步扩展独立测试 [`pkg/errors/tests/group_join_test.rs`](tests/group_join_test.rs)，不要把测试嵌入生产文件；若影响 `Find`，还要同步 [`pkg/errors/tests/wrap_test.rs`](tests/wrap_test.rs)。公开面变化需更新 [`pkg/errors/tests/api_parity_test.rs`](tests/api_parity_test.rs)。

性能上，应警惕每层 `Errors()` 的向量分配和句柄克隆，以及递归深度。若未来需要无分配迭代或环检测，这会改变 trait 返回类型或遍历复杂度，不能作为局部优化偷偷引入；应同时核对 Go 兼容需求、所有实现者和调用方。错误顺序影响 `HandleError` 的首个返回项、gRPC 状态选择和退避判定，不能随意排序或去重。

## 验证依据

- RustCodeGraph `status`：项目索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/errors/group.rs`：读取目标文件完整 62 行并确认 6 个符号；文件使用者包含 `tests/api_parity_test.rs`、`tests/group_join_test.rs`、`wrap.rs` 等。
- RustCodeGraph `query ErrorGroup`、`query error_group` 与 `node WalkDeep`：确认 trait、`SharedError::error_group` 和 `WalkDeep` 的精确位置；`WalkDeep` 调用轨迹包含两个独立测试入口。
- RustCodeGraph 文件节点：核对 `pkg/errors/core.rs::SharedError::{new_group,error_group}`、`pkg/errors/wrap.rs::{Unwrap,Find}`、`pkg/errors/join.rs::{JoinError,Join}`、`pkg/errctx/context.rs::Context::HandleError`、`br/pkg/conn/conn.rs::status_code`、`br/pkg/utils/backoff.rs::BackoffStrategyImpl::NextBackoff`。
- crate 边界：读取 `pkg/errors/Cargo.toml` 和 `pkg/errors/mod.rs`，确认库入口、再导出和依赖；该目录没有 `doc.go`。
- Rust 测试：读取 `pkg/errors/tests/group_join_test.rs`、`pkg/errors/tests/api_parity_test.rs` 与 `pkg/errors/tests/wrap_test.rs` 的相关用例，覆盖 trait 可实现性、普通/组错误展开、空输入、嵌套遍历顺序、短路和 `Find` 的组搜索。
- Go 对照：仓库搜索未发现 `pkg/errors/group.go`；核对官方 `pingcap/errors` 固定提交 `306e305bcf41` 的 [`group.go`](https://github.com/pingcap/errors/blob/306e305bcf41/group.go)，并由本仓库 `api_parity_test.rs` 的固定来源声明交叉确认。
- 结构验证按任务要求执行：目标文档存在，且固定二级标题恰好 11 个。纯文档任务未运行 Cargo。
