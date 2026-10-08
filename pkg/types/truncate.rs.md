# `pkg/types/truncate.rs` 逻辑说明

## 文件定位

`pkg/types/truncate.rs` 实现 MySQL 语义下的“截断类错误”分类和处置策略。它的物理路径位于 `pkg/types` 顶层，但当前不是由 `pkg/types/lib.rs` 直接声明的模块；真实编译边界是 `astersql-types-vector` crate，其 `pkg/types/internal/vector/lib.rs` 通过 `#[path = "../../truncate.rs"] mod truncate;` 将本文件挂载为私有子模块。

`pkg/types/internal/vector/Cargo.toml` 定义该 crate，并使用 `tidb-util-context` 提供的错误抽象、`tidb-errno` 提供的 MySQL errno。根 `pkg/types/Cargo.toml` 又以 `types-vector = { package = "astersql-types-vector", path = "internal/vector" }` 依赖并在 `pkg/types/lib.rs` 中再导出该子 crate。因此，本文件的实现属于 vector 子系统的转换上下文，不应与 `pkg/types/context.rs::Context::HandleTruncate<T>` 的带值泛型接口混淆。

## 核心职责

本文件只负责两层决策：

1. `is_truncate_error` 检查根因是否为 `errors::Error`，且其 MySQL 错误码是否属于固定的 10 个截断/越界错误码。
2. `Context::HandleTruncate` 先剖离包装错误至根因，再依据 `Context` 的标志将截断错误忽略、记为 warning，或作为硬错误返回。

它不负责产生截断错误，也不负责格式化 SQL 值；调用者必须传入已构造的 `errors::SharedError`。当前全仓 Rust 引用显示，这一具体 API 的直接调用仅见于 `pkg/types/truncate_12_aster_unit_test.rs`；生产模块已被编译接线，但未在 vector 生产转换流中找到直接调用边。

## 主要符号

- `is_truncate_error(error: &errors::SharedError) -> bool`（`pkg/types/truncate.rs:24`）是模块私有分类器。它对 `SharedError` 执行 `downcast_ref::<errors::Error>()`；非规范化 SQL 错误直接返回 `false`。成功下转后，将 `Error::Code()` 与本地 `TRUNCATE_CODES` 常量数组比较。
- `TRUNCATE_CODES: [i32; 10]`（`pkg/types/truncate.rs:29`）是函数内常量，包含 `ErrTruncatedWrongValue`、`ErrDataTooLong`、`ErrTruncatedWrongValueForField`、`ErrWarnDataOutOfRange`、`ErrDataOutOfRange`、`ErrBadNumber`、`ErrWrongValueForType`、`ErrDatetimeFunctionOverflow`、`WarnDataTruncated` 和 `ErrIncorrectDatetimeValue`。这一列表是当前的分类闭集。
- `Context::HandleTruncate(&mut self, error: Option<errors::SharedError>) -> Result<(), errors::SharedError>`（`pkg/types/truncate.rs:46`）是本文件唯一的公开 API。方法依附在 `pkg/types/internal/vector/lib.rs::Context` 上；需要 `&mut self` 是因为 warning 分支会追加到上下文的 `Vec<errors::SharedError>`。

本文件没有自定义 struct、enum、trait、模块级常量或条件编译项。

## 执行流程

`Context::HandleTruncate` 的分支顺序本身就是兼容性契约：

1. 输入为 `None` 时立即返回 `Ok(())`，不读标志，也不改变 warning 列表。
2. 对 `Some(error)` 调用 `errors::Cause(Some(&error))`，取单因链最内层的 `SharedError`。理论上 `Some` 输入会得到 `Some`；`unwrap_or(error)` 仍提供原值回退。
3. 将根因传给 `is_truncate_error`。如果它不是 `errors::Error` 或 errno 不在 10 项列表中，立即 `Err(cause)`；忽略/警告标志不得吞掉这类错误。
4. 对已识别的截断错误，先检查 `Flags::IgnoreTruncateErr()`。为真时返回 `Ok(())`，不写 warning。
5. 仅在未忽略时检查 `Flags::TruncateAsWarning()`。为真时通过 `Context::AppendWarning(cause)` 保存根因，然后返回 `Ok(())`。
6. 两个标志都未生效时返回 `Err(cause)`。

由于 ignore 分支位于 warning 分支之前，两标志同时开启时必然以忽略为准，不会留下 warning。

## 数据与状态

方法的输入是可选的、拥有所有权的 `errors::SharedError`。`SharedError` 的实现在 `pkg/errors/core.rs::SharedError` 中，内部以 `Arc<dyn Error + Send + Sync>` 共享错误，因此根因遍历和 warning 保存不需要复制具体错误对象。

`Context` 的相关状态定义在 `pkg/types/internal/vector/lib.rs`：

- `flags: Flags` 是只读决策输入；本方法不改写它。
- `warnings: Vec<errors::SharedError>` 是唯一可能被改写的状态；只有“已识别为截断错误且未忽略且设置了 warning 标志”时才追加一项。
- 返回值不携带裁剪后的业务值，只表示错误是否已被政策消化。这与 `pkg/types/context.rs::HandleTruncate<T>` 返回 `ValueResult<T>` 的设计不同。

## 依赖与调用关系

直接依赖均由 `use crate::{Context, errno, errors};` 引入：

- `Context`、`Flags` 与 `AppendWarning` 来自 `pkg/types/internal/vector/lib.rs`。
- `errno` 是 vector crate 对 `tidb_errno::errcode::*` 的再导出；依赖声明见 `pkg/types/internal/vector/Cargo.toml` 的 `tidb-errno`。
- `errors` 是 vector crate 对 `tidb_util_context::errors` 的再导出，最终对应 `astersql-errors`；`errors::Cause` 在 `pkg/errors/wrap.rs:270` 沿单因链走到最内层，`errors::Error` 与 `Error::Code` 在 `pkg/errors/normalize.rs`。

已验证的调用边为：`Context::HandleTruncate -> errors::Cause`、`Context::HandleTruncate -> is_truncate_error`、`Context::HandleTruncate -> Flags::{IgnoreTruncateErr, TruncateAsWarning}`、`Context::HandleTruncate -> Context::AppendWarning`，以及 `is_truncate_error -> SharedError::downcast_ref -> Error::Code`。RustCodeGraph 的 `node` 轨迹明确识别出 `HandleTruncate -> is_truncate_error` 和反向的 called-by 边。

上游直接调用边当前仅在 `pkg/types/truncate_12_aster_unit_test.rs`中。该测试由 `pkg/types/internal/vector/Cargo.toml` 以显式 `[[test]]` 目标挂载；测试通过公开再导出的 `astersql_types_vector::Context` 调用本方法。

## 错误处理与边界

- `None` 是无错误快速路径。
- 错误分类在根因上进行，所以 `errors::WithMessage` 等包装层不会遮蔽内层 SQL errno；warning 和返回值也是剥离后的根因，不是外层上下文。
- 非 `errors::Error` 的普通 `SharedError` 始终作为错误返回；即使开启 ignore 也不例外。
- 是 `errors::Error` 但 errno 不在 10 项闭集中时同样作为错误返回。新的 MySQL 截断错误码不会自动被纳入，必须显式更新列表和测试。
- ignore 优先于 warning；改变分支顺序会改变两标志并存时的可观测状态。
- 方法没有 panic 路径。`Cause(Some(...))` 的回退分支保留了原错误；下转失败用 `false` 表示，而非强制解包。

`pkg/types/truncate_12_aster_unit_test.rs::handle_truncate_matches_go_error_classification_and_flag_priority` 覆盖上述 10 个 errno、三种标志组合、包装错误、普通错误和其他 SQL 错误。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、文件或网络资源。`Context::HandleTruncate` 要求 `&mut self`，在 Rust 类型层面防止同一上下文的 warning 向量被无同步并发修改；`Context` 本身也没有内部锁。如需跨线程共享这个可变上下文，同步与所有权管理属于上层调用者的责任。

错误对象内部由 `Arc` 管理生命周期；进入 warning 列表后，其生命周期与所属 `Context` 一致。返回 `Err(cause)` 则将该 `SharedError` 的所有权交给调用者。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/truncate.go`。Rust `Context::HandleTruncate` 保留了 Go 实现的核心顺序：空错误成功、`errors.Cause` 取根因、限定同样的 10 个 errno、ignore 先于 warning，最后在严格模式返回根因。`pkg/types/context.go` 也明确声明 `FlagIgnoreTruncateErr` 开启时忽略 `FlagTruncateAsWarning`。

语言层差异为：

- Go 使用 `error`/`nil`，Rust 使用 `Option<SharedError>` 表达可选错误，使用 `Result<(), SharedError>` 表达处置结果。
- Go 通过 `*errors.Error` 类型断言取错误码，Rust 通过 `SharedError::downcast_ref::<errors::Error>()`。
- Go `Context` 的 warning 处理通过抽象 handler 转发；本文件所属的 vector `Context` 直接在 `Vec<SharedError>` 中累积 warning。
- Go 注释中保留了未来将 warning 统一转成 `WarnDataTruncated` 的 TODO；当前 Go 与 Rust 都记录实际根因，Rust 文件中没有实现该 TODO。

Go 目录下未找到直接调用 `HandleTruncate` 的专用 Go 单元测试；语义边界的直接回归证据是 Rust 独立测试 `pkg/types/truncate_12_aster_unit_test.rs`。

## 扩展指南

- 增加或删除截断类 errno 时，修改 `is_truncate_error` 内的 `TRUNCATE_CODES`，同步检查 `pkg/types/truncate.go::HandleTruncate` 的列表，并扩展 `handle_truncate_matches_go_error_classification_and_flag_priority` 的 `truncate_codes` 表。风险是把原本必须失败的错误静默忽略或降级为 warning。
- 修改标志策略时，首先保留“非截断错误不受截断标志影响”和“ignore 优先于 warning”两个不变量，除非 Go 契约也同步改变。
- 若需在 vector 生产转换流中使用该 API，接入点应位于产生 `SharedError` 的转换边界，而不是在底层错误构造器中预先吞错。新增生产调用时需增加对应的独立 `*_test.rs` 回归，不应把测试内嵌到本生产文件。
- 若要与根 `astersql-types` 的 `pkg/types/context.rs::HandleTruncate<T>` 统一，必须先解决 API 差异：本方法先分类 errno 且返回 `Result<()>`，后者信任调用者传入截断错误并返回带值的 `ValueResult<T>`。不能仅因同名就机械替换。
- 性能上，当前固定 10 项线性查找的上界很小；除非 errno 集合显著扩大且性能证据表明这里是热点，否则不应引入额外分配或全局可变缓存。

## 验证依据

本说明基于以下直接证据：

- 生产源文件：`pkg/types/truncate.rs`、`pkg/types/internal/vector/lib.rs`、`pkg/types/internal/vector/Cargo.toml`、`pkg/types/Cargo.toml`、`pkg/types/lib.rs`。
- 错误实现：`pkg/errors/core.rs::SharedError`、`pkg/errors/wrap.rs::Cause`、`pkg/errors/normalize.rs::{Error, Error::Code}`。
- Go 对照：`pkg/types/truncate.go::Context.HandleTruncate` 和 `pkg/types/context.go::{Flags, IgnoreTruncateErr, TruncateAsWarning}`。
- Rust 直接测试：`pkg/types/truncate_12_aster_unit_test.rs::handle_truncate_matches_go_error_classification_and_flag_priority`；其测试目标声明于 `pkg/types/internal/vector/Cargo.toml`。
- RustCodeGraph：`status` 显示本仓索引包含 `pkg/types/truncate.rs` 及 3 个符号；`query` 定位 `is_truncate_error` 与本文件的 `HandleTruncate`；`node --file pkg/types/truncate.rs` 核对了两个符号的实现，并给出 `HandleTruncate -> is_truncate_error` 的图边。`callers/callees` 命令在当前索引上 30 秒内未返回，因此上游调用者和其余下游边由全仓 `rg` 引用搜索与模块/Cargo 挂载关系交叉核验。

本任务是纯文档分析，未运行 Cargo 或代码测试。结构验证应确认本文档存在且恰好包含任务规定的 11 个二级章节。
