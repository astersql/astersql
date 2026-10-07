# [`pkg/objstore/s3like/metrics.rs`](metrics.rs)

## 文件定位

本文件属于 Cargo crate `astersql-objstore-s3like`；crate 根 `pkg/objstore/s3like/lib.rs` 通过 `mod metrics` 装入本模块，并用 `pub use metrics::*` 把公开常量、`S3_API_CALL_COUNTER`/`S3APICallCounter`、`init` 和 `RecordAPICall` 提升为 crate 级 API。`Cargo.toml` 表明指标实现直接依赖 `prometheus`，并通过本地 crate `astersql-metrics-common`（代码中的 `metricscommon`）复用全局常量标签规则。

它位于 S3-like 对象存储的观测边界：S3 与 OSS 客户端在真正调用后端 API 前记录 API 尝试次数，重试判定与读取恢复路径则在确认将要重试时记录错误文本。文件不执行对象存储 I/O，也不决定一次调用是否成功或是否应重试。

## 核心职责

- 定义稳定的后端标签值 `s3`、`oss`、`ks3`，以及 API 标签值 `ListObjects`、`HeadObjects`、`PutObject`。
- 构造并注册 Prometheus `CounterVec` `tidb_br_s3_api_call_total`，其可变标签固定为 `backend` 与 `api`；Help 文案说明它统计 BR external storage 发起的 S3-compatible API 调用。
- 用 `RecordAPICall` 将某个 `(backend, api)` 序列递增一次。调用发生在底层请求之前，因此统计的是调用尝试而非仅成功响应；Go 的 `pkg/objstore/s3store/client_test.go` 也验证失败调用仍计数。
- 用 crate 内部函数 `RecordRetryableError` 按完整错误字符串递增 `s3like_retryable_error_total{error=...}`，为读取恢复与 retryer 的可重试分支保留内部计数。
- 同时暴露 Rust 风格全大写常量和 Go 迁移兼容别名（如 `BACKEND_S3` 与 `BackendS3`），减少既有 Go 命名调用面的迁移成本。

## 主要符号

- `BACKEND_S3`、`BACKEND_OSS`、`BACKEND_KS3: &str`：`backend` 标签的三个预定义值；对应的 `BackendS3`、`BackendOSS`、`BackendKS3` 是同值兼容别名。
- `API_CALL_LIST_OBJECTS`、`API_CALL_HEAD_OBJECTS`、`API_CALL_PUT_OBJECT: &str`：`api` 标签的三个预定义值；对应 `APICallListObjects`、`APICallHeadObjects`、`APICallPutObject` 是 Go 风格别名。`HeadObjects` 同时覆盖显式 Head 与基于 Head 的存在性判断。
- `S3_API_CALL_COUNTER: LazyLock<prometheus::CounterVec>`：公开、进程级懒初始化计数器。初始化时调用 `metricscommon::NewCounterVec`，由后者注入 `pkg/metrics/common/wrapper.rs` 管理的常量标签；随后把 clone 注册到默认 Prometheus registry。
- `S3APICallCounter`：对 `S3_API_CALL_COUNTER` 的公开再导出，不是第二份计数器。`go_metric_public_names_remain_available` 用指针相等验证二者共享同一实例。
- `RETRYABLE_ERROR_COUNTER: LazyLock<prometheus::CounterVec>`：模块私有的进程级计数器，只有 `error` 标签。当前代码只构造并递增它，没有注册到默认 registry，也没有公开 gather 入口。
- `init()`：显式 force API 计数器，触发构造与注册；`LazyLock` 已保证同一进程内只初始化一次，因此重复调用本函数不会重复注册。
- `RecordAPICall(backend: &str, api: &str)`：选择二元标签序列并 `inc()`；公开给 S3/OSS 等具体实现使用。
- `RecordRetryableError(error: &str)`：选择单一错误标签序列并 `inc()`；可见性为 `pub(crate)`，仅供该 crate 的 retry/read 路径使用。

## 执行流程

API 调用指标的流程如下：

1. S3 或 OSS 客户端准备执行 Put、Head/存在性判断或 List 操作。
2. 客户端先调用 `RecordAPICall`，传入后端常量与 API 常量；例如 `pkg/objstore/s3store/client.rs::ListObjects` 使用 `(BACKEND_S3, API_CALL_LIST_OBJECTS)`，`pkg/objstore/ossstore/client.rs::PutObject` 使用 `(BACKEND_OSS, API_CALL_PUT_OBJECT)`。
3. 首次解引用 `S3_API_CALL_COUNTER` 时，`LazyLock` 调用 `metricscommon::NewCounterVec` 创建指标，注册其 clone 到默认 registry，再返回原句柄。
4. `with_label_values` 获得对应时间序列并递增；随后调用者继续执行远端 API。后端调用即使失败，本次尝试也已经计入。
5. Prometheus 从默认 registry gather 时看到名称 `tidb_br_s3_api_call_total`。若程序尚未调用 `init()`，也尚未首次访问/记录该计数器，则懒初始化尚未发生，registry 中还没有该 collector；仓库搜索未发现本模块 `init()` 的显式生产调用者，当前主要由首次指标访问完成注册。

可重试错误的流程独立于上述 API 指标：`Retryer::IsErrorRetryable` 仅在最终判定为可重试时记录有效错误；`S3ObjectReader::read` 在读失败、上下文未取消且仍有重试额度时记录；`Storage::doReadFile` 在排除 deadline/cancel 后、进入下一次读取尝试前记录。每次调用都以完整 `error.to_string()` 作为标签值，首次调用只会构造内部 CounterVec，不会把它注册到默认 registry。

## 数据与状态

两个 CounterVec 都由 `std::sync::LazyLock` 持有，生命周期覆盖整个进程，计数单调递增；文件中没有重置、删除标签序列或持久化逻辑。API 指标的状态维度是 `(backend, api)`，预定义组合最多为三种后端乘三种 API，但函数签名接受任意字符串，调用者仍可创建额外时间序列。`metricscommon::NewCounterVec` 会在创建时读取公共常量标签并覆盖传入 `Opts` 的 `const_labels`。

重试指标的状态维度是完整错误文本。它也接受任意字符串，动态内容（地址、对象名、请求 ID 等）可能造成高基数；扩展调用点时必须先评估标签归一化。当前 `RETRYABLE_ERROR_COUNTER` 既非 `pub`，也未注册，因此其值只保存在进程内 collector 中，不能从默认 registry 抓取。

Go 兼容别名均引用同一静态字符串或同一静态计数器，不复制状态。源码上的 `#![allow(non_snake_case, non_upper_case_globals)]` 专门容纳这些 Go 风格公开名称。

## 依赖与调用关系

上游调用关系经 RustCodeGraph 与源码交叉核对：

- `pkg/objstore/s3store/client.rs` 的 `PutObject`、`IsObjectExists`、`HeadObject`、`ListObjects` 调用 `RecordAPICall`，后端标签为 `s3`。
- `pkg/objstore/ossstore/client.rs` 的同类四个方法调用 `RecordAPICall`，后端标签为 `oss`。
- `pkg/objstore/s3like/retry.rs::Retryer::IsErrorRetryable`、`pkg/objstore/s3like/io.rs::S3ObjectReader::read` 与 `pkg/objstore/s3like/store.rs::doReadFile` 调用 `RecordRetryableError`。
- `BACKEND_KS3` 保留 Go/KS3 的标签契约；当前 Rust 调用点搜索没有发现以它记录 API 的生产路径，而 Go 的 `pkg/objstore/s3store/ks3.go` 存在 Put、Head、List 调用点。

下游依赖为 `std::sync::LazyLock`、`prometheus::{Opts, CounterVec, default_registry}` 和 `metricscommon::NewCounterVec`。后者只负责创建 CounterVec 并合入包级常量标签，实际默认 registry 注册由本文件完成。crate 根的 glob 再导出使具体 S3/OSS crate 能以 `s3like::RecordAPICall` 等路径使用这些符号。

## 错误处理与边界

本文件的两个记录函数都不返回 `Result`，不会把观测失败传播到对象存储业务调用。相反，指标定义或注册错误被视为编程/启动配置错误：`NewCounterVec`/`CounterVec::new` 使用 `expect` 处理非法描述符或标签，默认 registry 注册也使用 `expect("register S3 API call counter")`；若发生同名冲突，首次初始化会 panic。

`LazyLock` 将上述失败推迟到 `init()` 或第一次指标访问。正常情况下初始化仅执行一次；但同一默认 registry 若已被其他 collector 占用相同完整名称，仍会在首次 force 时失败。`with_label_values` 要求标签数量与定义一致，本文件固定分别传两个值和一个值；新增封装时不可改变这一不变量。

API 计数在请求之前递增，因此不能据此推导成功率，也不会区分结果或重试次数。重试计数只覆盖显式调用 `RecordRetryableError` 的分支：deadline/cancel 等直接返回分支不会由 `doReadFile` 记录，`Retryer` 也只记录最终判定为 retryable 的错误。当前重试 collector 未注册是可观测性边界，而不是“已有 Prometheus 指标可抓取”的证据。

## 并发与资源生命周期

`LazyLock` 为并发首次访问提供一次性初始化；注册完成后所有调用者共享同一 CounterVec。Prometheus CounterVec/Counter 支持并发递增，因此 S3、OSS 客户端以及并行读重试无需在本文件额外加锁。注册到默认 registry 的是 `S3_API_CALL_COUNTER` 的 clone，它与公开句柄共享底层指标状态，collector 的 registry 生命周期与进程一致。

本文件不创建线程、异步任务、通道、锁守卫、网络连接或需要关闭的资源。错误字符串在 `with_label_values` 调用期间借用，时间序列内部由 Prometheus collector 管理；调用方无需保存该字符串。随着新标签组合出现，CounterVec 会持有更多序列直至进程结束，尤其应控制 `error` 标签的基数。

## 与 Go 版本的对应关系

`pkg/objstore/s3like/metrics.go` 是 API 调用计数的直接对照：三种 backend、三种 API 值、指标 Namespace `tidb`、Subsystem `br_s3`、Name `api_call_total`、Help 文案以及标签顺序 `backend, api` 均一致。Go 的 `RecordAPICall` 和 Rust 的同名函数都在目标标签序列上加一；Rust 额外提供全大写惯用名，并保留 Go 名作为兼容别名。

初始化机制存在实现差异：Go 包的 `init()` 在包加载时调用 `prometheus.MustRegister`；Rust 用 `LazyLock`，`init()` 只负责 force，而首次 `RecordAPICall` 或直接访问静态量也会触发注册。因仓库内未找到显式生产 `init()` 调用，Rust 指标可能直到第一笔相关调用才出现在默认 registry。`pkg/objstore/s3like/metrics_test.rs` 正是通过先调用 `RecordAPICall` 再 gather 来验证这一行为。

`RETRYABLE_ERROR_COUNTER` 与 `RecordRetryableError` 没有出现在同路径 Go 文件中，是 Rust 读取/重试路径的附加实现；且当前未注册，不能把它描述为 Go 已有指标的完整移植。Go 的 S3 client 测试验证 Head/List/Put 在成功和错误返回时都按尝试次数递增，支持 Rust 将计数放在远端调用之前的语义。

## 扩展指南

- 增加新的 S3-like API 指标时，先新增稳定、低基数的 API 常量，再在具体客户端发起远端请求之前调用 `RecordAPICall`；S3 与 OSS 对等方法应同步接线，若 Rust KS3 实现落地也应使用既有 `BACKEND_KS3`。
- 增加后端时，需要新增 backend 常量及 Go 兼容策略，并同步检查所有具体客户端、`metrics.go` 和独立测试。不要把桶名、对象键、endpoint 等高基数字段作为标签。
- 若要让重试错误可抓取，必须明确决定注册位置、稳定的指标命名/namespace、重复注册行为和错误分类方案；直接注册当前完整错误字符串标签可能引入严重基数与内存风险。
- 若调整懒初始化，应保持“同一 collector 只注册一次”和 Go 包加载后可见性的兼容目标，并增加“未发生 API 调用时 gather 是否可见”的回归测试。
- 测试逻辑应继续放在独立文件：基本注册行为放在 `pkg/objstore/s3like/metrics_test.rs`，标签值、Go 兼容别名及增量行为放在 `pkg/objstore/s3like/migration_aster_unit_test.rs`；具体 S3/OSS 调用覆盖则在各自 client 测试中验证。不要把测试内嵌进 `metrics.rs`。
- 修改指标名、Help、标签顺序或公开别名属于监控兼容性变更，会影响 dashboard、告警与查询；修改计数时点则会改变“尝试次数”的业务语义，均需与 Go 对照和下游查询一起评估。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 `pkg/objstore/s3like/metrics.rs`；`node --file` 核对了该文件全部 87 行及 19 个符号；针对 `RecordAPICall`、`RecordRetryableError`、`init` 的 callers/callees/explore 查询确认了计数器引用及 S3、OSS、retry、io、store 调用边。
- 源码与装配：`pkg/objstore/s3like/metrics.rs`；`pkg/objstore/s3like/lib.rs` 的模块声明、公开再导出和独立测试装配；`pkg/metrics/common/wrapper.rs::NewCounterVec` 的常量标签合并逻辑。
- crate 边界：`pkg/objstore/s3like/Cargo.toml` 的 `astersql-objstore-s3like` 包声明、`lib.rs` 入口，以及 `metricscommon`、`prometheus` 依赖。
- 生产调用点：`pkg/objstore/s3store/client.rs`、`pkg/objstore/ossstore/client.rs`、`pkg/objstore/s3like/retry.rs`、`pkg/objstore/s3like/io.rs`、`pkg/objstore/s3like/store.rs`。
- Go 对照：`pkg/objstore/s3like/metrics.go`；`pkg/objstore/s3store/client.go`、`pkg/objstore/ossstore/client.go`、`pkg/objstore/s3store/ks3.go` 的计数调用；`pkg/objstore/s3store/client_test.go` 对成功/失败调用仍累计尝试次数的断言。
- Rust 独立测试：`pkg/objstore/s3like/metrics_test.rs::record_api_call_registers_metric_with_default_registry` 验证完整指标名已注册；`pkg/objstore/s3like/migration_aster_unit_test.rs::go_metric_public_names_remain_available` 验证兼容别名；`api_call_metric_uses_backend_and_api_labels` 验证指定标签序列恰好加一。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以固定十一个二级标题的结构命令校验，并人工复核文档未把未注册的 retry collector 或尚未接线的 Rust KS3 路径写成已支持事实。
