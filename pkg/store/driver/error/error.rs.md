# `pkg/store/driver/error/error.rs`

## 文件定位

本文件是 `astersql-store-driver-error` crate 的核心实现，位于存储驱动与 TiDB/AsterSQL 通用错误体系之间。crate 入口 [`lib.rs`](./lib.rs) 通过 `include!("error.rs")` 纳入本文件，并把其中的公开项重新导出；[`Cargo.toml`](./Cargo.toml) 将 crate 绑定到官方 `tikv-client` 的 `v0.4.2-aster.10` tag，同时依赖本仓库的 `dbterror`、`sqlkiller`、执行器错误和 parser 错误名模块。

它不是网络客户端或重试器，而是错误语义适配层：接收 TiKV/PD 客户端产生的 `SharedError`，识别错误链中的具体类型，再转换成 `kv`、`terror`、`exeerrors` 或 TiKV 错误类中的稳定错误。当前可见的生产调用链是 [`pkg/store/driver/backoff/backoff.rs`](../backoff/backoff.rs) 的 `Backoff::Backoff` 与 `Backoff::BackoffWithMaxSleep` 在返回前调用 `ToTiDBErr`；同文件的 `map_backoff_type_to_error` 也构造本文件的 `TiKvError` 变体。

## 核心职责

1. 用 `TiKvError` 和 `PdError` 表达 Go 客户端已有、但官方 Rust `tikv-client` 尚未完整暴露的类型化错误语义。
2. 用一组 `LazyLock<Box<terror::Error>>` 建立 TiKV/PD 错误码到 TiDB `dbterror::ClassTiKV` 错误类的稳定映射。
3. 由 `ToTiDBErr` 穿透 `Trace`、`WithStack`、`Wrap` 等错误包装，完成三类来源错误的归一化。
4. 保留错误参数，例如事务时间戳、大小限制、store ID、资源组名称和 GC 安全点，使上层获得与 Go 版本一致的错误码和诊断文本。
5. 对未知或尚未映射的错误不做有损简化，而是经 `errors::Trace` 返回原错误并补充堆栈。

## 主要符号

- `pub enum TiKvError`：client-go 风格的类型化错误集合。它覆盖 KV 基础错误、事务/条目/键过大、TiKV/TiFlash/PD 超时与繁忙、查询终止信号、GC 中止、锁等待、Region 不可用、store token 限流、结果不确定等情况；`Other(String)` 明确表示无专门映射的来源错误。
- `pub enum PdError`：PD 资源组错误的窄接口，包含资源组不存在、配置不可用、被限流以及 `Other(String)`。
- `ErrTokenLimit` 至 `ErrUnknown`：18 个惰性初始化的公开 TiDB 错误实例。每个实例由 `dbterror::ClassTiKV.NewStd(errno::...)` 创建，首次解引用时才初始化。
- `REGISTER_TIKV_RETURNED_ERRORS: Once` 与 `register_tikv_returned_errors()`：一次性注册 TiKV 可能直接返回的 `ErrDataOutOfRange`、`ErrTruncatedWrongValue`、`ErrDivisionByZero`。
- `clone_normalized(&terror::Error) -> SharedError`：把静态/惰性 TiDB 错误克隆为统一的共享错误对象。
- `find_typed<E>(&SharedError) -> Option<SharedError>`：借助 `errors::Find` 沿错误链查找具体错误类型，承担 Go `errors.As` 的对应职责。
- `convert_tikv_error`、`convert_official_tikv_error`、`convert_pd_error`：三个私有、无副作用的分类转换函数；返回 `None` 表示该来源错误应走保真回退路径。
- `pub fn ToTiDBErr(Option<SharedError>) -> Option<SharedError>`：唯一的公开转换入口；`Option` 用于保持 Go 中 `nil error` 的行为。

## 执行流程

`ToTiDBErr` 的流程如下：

1. 调用 `register_tikv_returned_errors()`；`Once::call_once` 保证三个数值类错误在进程内最多注册一次。
2. 对输入使用 `?`：`None` 立即返回 `None`，非空错误继续处理。
3. 先用 `find_typed::<TiKvError>` 穿透包装链。找到后交给 `convert_tikv_error`：
   - 简单哨兵映射到 `kv::*`、本文件的 `Err*` 或 `terror::ErrResultUndetermined`；
   - 带参数错误通过 `FastGenByArgs` 或 `GenWithStackByArgs` 注入时间戳、大小、store ID 等参数；
   - `QueryInterruptedWithSignal` 按 `sqlkiller` 的五个已知信号分流到查询中断、最大执行时间、查询/实例内存超限和 runaway 错误；两个内存错误因客户端不知道 connection ID 而填 `-1`；
   - `GcTooEarly` 复用 `ErrTxnAbortedByGC`，对旧错误类型缺失的数值字段填 `"<unknown>"`；
   - 未知信号和 `Other` 返回 `None`，不伪造错误类别。
4. 若未完成转换，再查找官方 `tikv_client::Error`。`UndeterminedError` 映射为 `ErrResultUndetermined`，`InvalidTransactionType` 和 `OperationAfterCommitError` 映射为 `kv::ErrInvalidTxn`，其余变体暂不转换。
5. 再查找 `PdError`。三个资源组变体分别转换为资源组不存在、配置不可用和限流错误；资源组名称被保留为参数。
6. 三类转换均未命中时，调用 `errors::Trace(Some(error))` 返回原错误。因此 `ToTiDBErr(Some(_))` 仍返回 `Some(_)`，同时保留原文并确保存在堆栈。

转换优先级是自定义 `TiKvError`、官方 `tikv_client::Error`、`PdError`。新增交叠类型时必须考虑该顺序，因为先命中的类型会提前返回。

## 数据与状态

文件中的业务数据主要存在于错误枚举字段：`start_ts`、`size`、`limit`、`key_size`、`signal`、事务与安全点时间/时间戳、`store_id`、资源组名称及自由文本。转换函数读取这些值并构造新错误，不修改输入。

全局状态只有两类：18 个 `LazyLock<Box<terror::Error>>` 和一个 `Once`。`LazyLock` 使错误模板按需创建；错误返回时通过 `clone_normalized` 克隆模板，避免把全局对象的所有权交给调用者。`REGISTER_TIKV_RETURNED_ERRORS` 记录注册是否完成，不保存请求数据，也不随请求重置。

`SharedError` 是错误链的统一所有权边界。`find_typed` 返回链中匹配错误的共享句柄，随后 `downcast_ref` 只借用具体类型；参数中的 `String` 在生成 TiDB 错误时按需克隆。文件没有缓存每次转换的结果。

## 依赖与调用关系

上游关系：

- [`pkg/store/driver/error/lib.rs`](./lib.rs) 组装依赖、内嵌 `pkg/kv/error.rs`，并公开导出本文件的全部公开 API。
- [`pkg/store/driver/backoff/backoff.rs`](../backoff/backoff.rs) 在 backoff 类型到错误的映射中构造 `TiKvError`，并在 `Backoff`、`BackoffWithMaxSleep` 返回错误时调用 `ToTiDBErr`。这是 Rust 源码搜索确认的直接生产调用边。
- [`pkg/ddl/split_region.rs`](../../../ddl/split_region.rs) 对 `astersql_store_driver_error::PdError` 做类型识别；它说明此 crate 的公开错误类型还承担跨 crate 的分类边界。
- 多个 crate 在各自 `Cargo.toml` 中依赖此 crate，包括 `store/copr`、`executor`、`session`、`ddl`、`server`、`ttlworker` 和导入模块；依赖声明只证明可见性，不能等同于这些 crate 都直接调用 `ToTiDBErr`。

下游关系：

- `errors::{Find, Trace, SharedError}` 提供错误链遍历、堆栈包装与统一容器。
- `dbterror::ClassTiKV` 与 `errno::*` 定义稳定错误类和错误码。
- `kv::*` 提供存储抽象层既有错误；`terror::ErrResultUndetermined` 表示提交结果不确定。
- `exeerrors::*` 和 `sqlkiller::*` 共同完成查询终止信号到执行器错误的转换。
- 官方 `tikv_client::Error` 只对三个明确语义的变体做转换，其余保持原样。

RustCodeGraph 对 `ToTiDBErr` 给出的直接下游调用边是 `register_tikv_returned_errors`；它还确认该注册函数由 `ToTiDBErr` 和迁移单元测试调用。对跨 crate 调用者，文件级图查询没有完整输出，因此以上生产调用点同时用 Rust 源码搜索核对。

## 错误处理与边界

- `None` 是合法输入，严格保持为空，不创建替代错误。
- 已识别错误必须保留错误类别和参数。`FastGenByArgs` 用于无需新堆栈的快速参数化错误，`GenWithStackByArgs` 用于需要堆栈的生成路径；不能随意互换，否则诊断表现可能偏离 Go 版本。
- 被包装的来源错误仍可识别；`find_typed` 是维持该不变量的关键。直接只对最外层 `downcast_ref` 会破坏 `Trace`/`WithStack`/`Wrap` 场景。
- 未知 `QueryInterruptedWithSignal`、`TiKvError::Other`、`PdError::Other` 和大多数官方客户端错误均回退到 `errors::Trace`。这是保真策略，不表示它们被归类为 `ErrUnknown`。
- `TiKvError::Unknown` 是客户端明确报告的“未知 TiKV 错误”，因此会映射到 `ErrUnknown`；它与无法识别的 Rust 错误是两个不同边界。
- `GcTooEarly` 缺少新错误类型拥有的数值时间戳，使用 `"<unknown>"` 是与 Go 实现一致的兼容占位，不应推导或伪造数值。
- 官方客户端适配当前只覆盖 `UndeterminedError`、`InvalidTransactionType`、`OperationAfterCommitError`。扩展映射前必须确认上游 tag 中变体的稳定语义，并为原错误文本/堆栈回退添加保护测试。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁等待流程、网络连接或事务；所有转换均在调用线程同步完成。并发安全依赖标准库的 `LazyLock` 和 `Once`：并发首次访问同一错误模板时只初始化一次，并发调用注册函数时也只执行一次注册闭包。

转换期间仅产生短生命周期的借用、必要的字符串克隆和一个结果 `SharedError`。`SharedError` 的后续生命周期由调用者管理。文件没有后台资源需要关闭，也没有取消协议；查询中断仅作为错误信号分类，不负责实际终止查询。

## 与 Go 版本的对应关系

直接对照文件是 [`error.go`](./error.go)，测试对照是 [`error_test.go`](./error_test.go)。两版共同点包括：空错误原样返回、按错误链匹配来源类型、完整的 client-go 分支、相同的 TiDB 错误类和参数、资源组错误转换，以及最终 `errors.Trace` 回退。

Rust 的主要结构差异如下：

- Go 直接匹配 `client-go/v2/error` 与 PD client 的错误类型；Rust 定义 `TiKvError`/`PdError` 保留这些分支，并额外接受官方 `tikv-client` 错误。
- Go 借助包初始化时的空标识符变量注册三个 TiKV 返回错误；Rust 使用 `Once`，由 `ToTiDBErr` 首次调用时触发。
- Go 的 `error`/`nil` 对应 Rust 的 `Option<SharedError>`；Go `errors.As`/`errors.Is` 对应 `errors::Find` 加 `downcast_ref`。
- Go 多个哨兵错误可直接返回全局对象；Rust 将 `terror::Error` 克隆进 `SharedError`，保持所有权安全。
- Rust 的 `convert_official_tikv_error` 是 Go 文件没有的兼容分支；它不能被视作 client-go 全量替代，因为当前只处理三个官方变体。

[`error_test.rs`](./error_test.rs) 对齐 Go 的 `TestConvertError` 和 `TestMemBufferOversizeError`；[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 进一步覆盖哨兵映射、字段参数、五种查询信号、GC、PD 资源组、一次注册以及官方客户端的映射/回退。Go `TestMain` 的公共环境和 goroutine 泄漏检查没有机械移植：本 crate 不启动 Go goroutine，Rust 测试由标准 harness 管理。

## 扩展指南

- 增加 client-go 风格错误时：先为 `TiKvError` 增加带足够字段的变体，再在 `convert_tikv_error` 增加精确映射；同步更新 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，并在 Go 有对应用例时同步 [`error_test.rs`](./error_test.rs)。不要把测试内嵌到生产文件。
- 增加官方 `tikv-client` 映射时：修改 `convert_official_tikv_error`，核对 [`Cargo.toml`](./Cargo.toml) 固定 tag 中的真实变体，并保留“未映射错误原文和堆栈”回归测试。若需要升级外部依赖，必须遵守上游独立移植、提交和发布 tag 的仓库规则。
- 增加 PD 分类时：扩展 `PdError` 与 `convert_pd_error`，确认错误码已存在于 `errno` 且 `ClassTiKV` 是正确类别；若 DDL 调用方按具体 `PdError` 分类，也要检查其匹配逻辑。
- 增加查询终止信号时：同时检查 `sqlkiller` 常量、执行器错误定义和 Go `ToTiDBErr`，明确未知 connection ID 等参数的占位策略。
- 改变匹配顺序或回退策略时：重点验证被包装错误、类型重叠、原错误文本和堆栈；这类改动有兼容风险，因为上层可能按 `terror::Error::Equal`、错误码或消息参数判断行为。
- 性能方面，转换仅在错误路径执行；仍应避免在热错误路径引入全链多次扫描或无必要的大字符串克隆。若扩展到更多来源类型，可评估保持当前优先级扫描是否足够。

## 验证依据

- 目标实现：[`error.rs`](./error.rs)，核对了两个公开枚举、18 个公开惰性错误、一次注册状态、五个私有辅助/转换函数和公开入口 `ToTiDBErr`，以及所有匹配与回退分支。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)，核对 crate 名、固定的 `tikv-client` Git tag、本地依赖、`include!` 和公开再导出关系。本目录不存在 `doc.go`，因此没有额外包契约文件可读。
- Go 对照：[`error.go`](./error.go) 与 [`error_test.go`](./error_test.go)，核对 client-go/PD 分支、注册方式、包装错误、结果不确定和超限消息语义。
- Rust 测试：[`error_test.rs`](./error_test.rs) 与 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，核对 `None`、重复注册、包装链、错误相等性、参数保留、全部五种已知查询信号、GC/资源组、官方客户端及保真回退。
- RustCodeGraph：`status` 显示索引包含本目录的 `error.rs`、Go 对照和两份 Rust 测试；`query ToTiDBErr` 定位到 `error.rs:322`；`node pkg/store/driver/error/error.rs::ToTiDBErr` 确认其调用 `register_tikv_returned_errors`；三个转换函数和注册函数的 `node` 查询确认了源码位置与内部调用边。文件级 `node` 与部分 callers 查询无输出/超时，因此跨 crate 调用者另以 `rg` 核对。
- 调用搜索：Rust 源码搜索确认 [`pkg/store/driver/backoff/backoff.rs`](../backoff/backoff.rs) 构造 `TiKvError`，并在 `Backoff`、`BackoffWithMaxSleep` 中调用 `ToTiDBErr`；[`pkg/ddl/split_region.rs`](../../../ddl/split_region.rs) 识别公开 `PdError`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验证要求本文恰好包含上述 11 个固定二级标题。
