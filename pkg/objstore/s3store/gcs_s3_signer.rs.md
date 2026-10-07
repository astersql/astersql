# `pkg/objstore/s3store/gcs_s3_signer.rs` 逻辑说明

## 文件定位

本文件属于 `astersql-objstore-s3store` crate（见 `pkg/objstore/s3store/Cargo.toml`），由私有模块 `gcs_s3_signer` 装配进 crate（`pkg/objstore/s3store/lib.rs`）。它位于 S3 客户端配置构造阶段与 AWS SDK 请求签名阶段之间，专门修正 Google Cloud Storage（GCS）S3/XML API 对 AWS SigV4 规范实现差异的兼容问题。

生产调用链是 `NewS3Storage` → `store.rs::build_api` → `store.rs::is_gcs_s3_compatible` → `configure_gcs_signer`。只有 provider 被识别为 GCS，或 endpoint 的主机名匹配 `storage.googleapis.com` 及其子域时，`build_api` 才把本文件的 interceptor 注入 `aws_sdk_s3::config::Builder`。因此它不是所有 S3 请求的全局签名器，也不负责 GCS 检测、凭证加载、区域探测或实际 SigV4 算法。

## 核心职责

核心约束来自 GCS 的 S3 兼容接口：HTTP 请求仍应携带 `accept-encoding`，但该头不能出现在 SigV4 canonical request 的 `SignedHeaders` 中。`GcsS3CompatibleSigner` 在签名前暂时移除该头，并在网络发送前恢复其全部原始值。

Rust AWS SDK 与 Go SDK 的 hook 时序不同。Rust SDK 原生的 `InvocationIdInterceptor` 和 `RequestInfoInterceptor` 通常在签名后生成 `amz-sdk-invocation-id` 与 `amz-sdk-request`；为了与 Go 版实际签名集合一致，本文件在 `modify_before_signing` 中提前调用这两个原生 interceptor，并保存它们生成的精确值。SDK 后续正常运行 transmit hooks 后，本文件再把保存值恢复，保证“签名时的 SDK 元数据”与“最终发出的 SDK 元数据”相同。

文件不计算签名，不修改 payload、凭证、endpoint 或重试策略；真正的 SigV4 签名仍由 AWS SDK 原生 signer 完成。

## 主要符号

- `SavedHeaders(Vec<(String, Vec<String>)>)`：私有、按请求尝试保存的头快照。外层 `Vec` 只记录实际存在的目标头，内层 `Vec<String>` 保留同名头的全部值与顺序。其 `Storable` 实现选择 `StoreReplace<Self>`，使新快照替换该 interceptor state 中的旧快照。
- `GcsS3CompatibleSigner`：无字段、私有的 `Intercept` 实现；`name()` 固定返回 `"GcsS3CompatibleSigner"`，供 SDK 识别和诊断。
- `restore_headers(request, cfg)`：私有恢复函数。从 `ConfigBag` 的 interceptor state 取出 `SavedHeaders`，先删除请求中的同名头，再按保存顺序逐值 `append`。`std::mem::take` 清空快照，使重复执行恢复路径不会再次写入旧值。
- `GcsS3CompatibleSigner::modify_before_signing`：签名前入口。提前执行两个 SDK 元数据 interceptor，快照 `accept-encoding`、`amz-sdk-invocation-id`、`amz-sdk-request`，只删除 `accept-encoding`，再把快照写入当前配置状态。
- `GcsS3CompatibleSigner::modify_before_transmit`：正常发送路径的恢复入口，在 HTTP transport 看见请求前调用 `restore_headers`。
- `GcsS3CompatibleSigner::modify_before_attempt_completion`：尝试结束路径的兜底恢复入口；仅当 finalizer context 仍含请求时恢复，因此也覆盖签名阶段失败、尚未进入 transport 的情况。
- `configure_gcs_signer(builder)`：本文件唯一 crate 内可见入口，通过 `SharedInterceptor::new` 向 S3 config builder 注册无状态 interceptor。文件没有公开 API、模块级常量、trait 定义或条件编译项。

## 执行流程

1. `store.rs::build_api` 创建 AWS S3 `Builder`，配置 endpoint、path-style、HTTP client 与重试策略。
2. `store.rs::is_gcs_s3_compatible(options)` 返回 true 时，`build_api` 调用 `configure_gcs_signer(&mut builder)`；随后 builder 构建 client，后续每次请求均进入该 interceptor。
3. 每次请求尝试到达 `modify_before_signing` 后，代码遍历 `RuntimeComponents::interceptors()`，按 `name()` 精确筛选 `InvocationIdInterceptor` 与 `RequestInfoInterceptor`，调用它们的 `modify_before_transmit`，提前生成本次要签名的 SDK 元数据头。任一调用失败即通过 `?` 返回错误。
4. 代码从当前 `HttpRequest` 快照三个目标头的所有值；不存在的头不进入快照。随后仅移除 `accept-encoding`，所以原生 signer 会签入两个 SDK 元数据头，却不会签入 `accept-encoding`。
5. 正常情况下，AWS SDK 完成签名及其原生 transmit hooks，本 interceptor 的 `modify_before_transmit` 用快照覆盖三个头的当前值。最终 transport 收到原有全部 `accept-encoding` 值，以及与签名完全一致的 invocation/request 元数据。
6. 如果尝试在签名或发送准备期间失败，`modify_before_attempt_completion` 在仍可访问请求时执行同一恢复逻辑。若随后重试，下一次尝试重新生成并保存自己的 `amz-sdk-request`，因此 `attempt=1`、`attempt=2` 能分别正确签名和发送。

## 数据与状态

`GcsS3CompatibleSigner` 本身是零大小、无可变字段的共享对象。可变数据完全位于 SDK 传入的 `HttpRequest` 与 `ConfigBag` interceptor state 中；`SavedHeaders` 是一次请求尝试的临时快照，而不是进程级或 client 级缓存。

快照采用拥有所有权的 `String`，不会借用请求头。恢复前先 `remove` 再逐值 `append`，避免 SDK 在签名后再次运行原生 interceptor 所产生的值与旧值叠加，也保留 `accept-encoding` 的多值语义。不存在的 `accept-encoding` 不会被凭空添加：没有值就不记录，恢复阶段也没有对应项。

`StoreReplace` 明确了同类型状态的覆盖语义；`std::mem::take(&mut saved.0)` 则把已恢复快照变为空值。正常发送恢复后，即使 attempt completion hook 再到达，也不会重复恢复或复制头。

## 依赖与调用关系

上游生产调用者由 RustCodeGraph 确认为 `pkg/objstore/s3store/store.rs::build_api`；直接测试调用者是 `gcs_s3_test.rs::gcs_signer_excludes_accept_encoding_but_sends_all_values` 与测试辅助函数 `signer_client`。`build_api` 的上游是 `NewS3Storage`，所以权限检查的 `HEAD`/`GET` 与之后通过该 client 发出的对象存储请求都会应用兼容逻辑。

直接依赖如下：

- `aws-sdk-s3` 提供 `Builder`、`Intercept`、生命周期 context、`ConfigBag` 与 `SharedInterceptor`；在 `Cargo.toml` 中是直接依赖。
- `aws-smithy-types` 提供 `Storable` 与 `StoreReplace`，用于把私有快照放入配置包；也是直接依赖。
- `storeapi` 重新导出 Smithy runtime API，本文件经它取得 `BoxError`、`HttpRequest` 与 `RuntimeComponents`；`storeapi` 是同仓 workspace 路径依赖。
- 下游的真实签名、重试调度和 HTTP 发送均由 AWS SDK/Smithy runtime 执行；RustCodeGraph 对 SDK 外部实现没有本仓调用边，本文件只实现 SDK 规定的 hook。

模块是 `lib.rs` 中的私有 `mod gcs_s3_signer`，入口也仅为 `pub(crate)`，因此兼容策略被限制在本 crate 的构建流程内。

## 错误处理与边界

`modify_before_signing` 唯一主动传播的错误来自提前调用原生 `InvocationIdInterceptor` 或 `RequestInfoInterceptor`；错误保持为 `BoxError` 返回，不包装也不吞掉。保存快照和头操作本身没有 `Result` 分支。

本 interceptor 之后发生的签名错误由 SDK 保留。`modify_before_attempt_completion` 只负责恢复头并返回 `Ok(())`，不会替换原始错误。`gcs_signer_restores_headers_when_signing_fails_and_keeps_error` 注入 `injected signing failure`，验证 transport 未收到请求、观察者仍能看到恢复后的两个 `accept-encoding` 值，且最终错误仍包含注入原因。

关键边界包括：多值 `accept-encoding` 必须完整恢复；缺失该头时不得新增；重试之间 invocation id 保持一致，而 `amz-sdk-request` 的 attempt 编号随尝试变化；`FinalizerInterceptorContextMut::request_mut()` 为 `None` 时无法也无需恢复。对目标三种头，恢复采取“快照覆盖”而非合并；这是保持签名值与发送值一致所必需的约束，新增同类头时也应维持这一行为。

## 并发与资源生命周期

实现没有锁、线程、异步任务、通道或外部资源。`SharedInterceptor` 可由 client 跨请求共享，因为 `GcsS3CompatibleSigner` 无内部状态；请求相关状态由 SDK 的 `ConfigBag` 和 request context 隔离。

单次尝试的生命周期是“签名前生成并保存 → 正常发送前恢复”或“签名前生成并保存 → 失败完成前恢复”。重试会为每次 attempt 重新进入保存流程，测试 `gcs_signer_restores_multivalue_headers_on_retry_and_preserves_sdk_metadata` 通过一个先返回 500、后返回 200 的 transport 验证两个请求均恢复多值头，并分别携带正确的 attempt 元数据。

代码不持有 HTTP body、连接或 runtime，也不延长请求引用的生命周期。资源释放由 AWS SDK 与 Rust 所有权规则负责；`SavedHeaders` 在配置状态被替换或 attempt 结束后随状态销毁。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/objstore/s3store/gcs_s3_signer.go`。Go 的 `gcsS3CompatibleSigner.SignHTTP` 包装 `v4.Signer`：复制并删除 `Accept-Encoding`，调用真实 signer，无论 signer 成功与否都恢复保存值，再返回原错误。Rust 版保留同一用户可见契约：发送该头但不把它列入签名，并在签名失败时恢复请求。

实现形态不同是 SDK 扩展点造成的。Go 可通过 `s3.Options.HTTPSignerV4` 替换 signer；Rust 通过 `Intercept` 包围 SDK 原生 signer。Rust 版还必须提前运行并快照 `InvocationIdInterceptor`、`RequestInfoInterceptor`，因为 Rust SDK 默认在签名后才添加这两个头，而 Go 请求会将其签入。Go signer 只保存 `Accept-Encoding`；Rust 版保存三种头，并显式支持全部重复值、重试 attempt 变化及 finalizer 失败路径。

Go 集成测试 `pkg/objstore/s3store/gcs_s3_test.go::TestGCSS3CompatibleSignerSkipsAcceptEncoding` 验证 GCS provider/endpoint 下权限请求有签名且排除 `accept-encoding`。Rust 对应文件 `pkg/objstore/s3store/gcs_s3_test.rs` 除覆盖相同主流程外，还覆盖多值头、缺失头、一次重试及签名失败恢复；这些是 Rust SDK 生命周期差异所需的额外回归证据。

## 扩展指南

若 GCS 又要求某个请求头“发送但不签名”，最可能修改 `modify_before_signing` 的快照列表与移除逻辑，并让 `restore_headers` 继续统一恢复；不要只删除头而漏掉正常发送和失败 finalizer 两条恢复路径。新增目标头必须明确是覆盖还是合并语义，并在 `gcs_s3_test.rs` 添加多值、缺失、重试和失败场景。

若要改变启用范围，应修改并测试 `store.rs::is_gcs_s3_compatible` 或 `build_api`，而不是让 interceptor 自行解析 endpoint。若 AWS SDK 改名或改变 `InvocationIdInterceptor` / `RequestInfoInterceptor` 时序，必须同步检查 `modify_before_signing` 的按名称筛选；字符串名称是升级 SDK 时最脆弱的兼容点。若 SDK 提供稳定的“签名前生成元数据”接口，应优先迁移到该接口，同时保持现有签名头集合测试。

兼容风险主要是 signed headers 与实际发送 headers 不一致会导致 GCS 拒绝签名；正确性风险是失败路径未恢复、重试复用旧 attempt 值或重复追加头；性能成本是每个 attempt 遍历 interceptor 列表并复制至多三种头，当前数据量很小。同步测试应放在独立的 `pkg/objstore/s3store/gcs_s3_test.rs`，不要嵌入生产文件；Go 语义发生变化时还应对照 `gcs_s3_signer.go` 与 `gcs_s3_test.go`。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`explore` 读取了 `gcs_s3_signer.rs` 全文。
- 符号查询：`query`/`node` 确认 `SavedHeaders`、`GcsS3CompatibleSigner`、`restore_headers`、`configure_gcs_signer` 及三个 `Intercept` hook 的位置和可见性。
- 调用边：`callers configure_gcs_signer` 返回 `store.rs::build_api`、`gcs_s3_test.rs::gcs_signer_excludes_accept_encoding_but_sends_all_values`、`gcs_s3_test.rs::signer_client`；`callers restore_headers` 返回正常发送与 attempt completion 两条恢复路径。SDK 外部函数的精细 callee 边未由本仓索引展开，相关结论以 trait hook 源码和测试为准。
- crate 与装配：读取 `pkg/objstore/s3store/Cargo.toml` 和 `pkg/objstore/s3store/lib.rs`，确认直接依赖、路径依赖、私有模块及独立测试模块。
- 生产接线：读取 `pkg/objstore/s3store/store.rs` 的 `NewS3Storage`、`build_api` 与 `is_gcs_s3_compatible` 附近代码，确认条件注入和主链位置。
- 对照与测试：完整读取 `pkg/objstore/s3store/gcs_s3_signer.go`、`pkg/objstore/s3store/gcs_s3_test.rs` 与 `pkg/objstore/s3store/gcs_s3_test.go`。Rust 测试实际断言签名集合、多值恢复、缺失头、重试元数据及失败时错误保留；本任务按要求不运行 Cargo。
