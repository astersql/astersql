# `pkg/objstore/s3store/ks3.rs`

## 文件定位

本文件是 `astersql-objstore-s3store` crate 中的金山云 KS3 适配层。模块入口 `pkg/objstore/s3store/lib.rs` 以 `#[path = "ks3.rs"] mod ks3` 挂载它，再通过 `pub use ks3::*` 暴露其 API；crate 边界和依赖由 `pkg/objstore/s3store/Cargo.toml` 定义。KS3 兼容 S3 协议，因此这里不重复实现完整对象 I/O，而以 `KS3Storage` 包装 `s3like::Storage`，只保留 KS3 特有的构造限制、权限探测、复制冲突处理、URI 和 Presign 能力差异。

上层以 `storeapi::Storage` 使用该类型，底层实际 I/O 进入通用 S3 实现。源码中的 `MAX_SKIP_OFFSET_BY_READ` 与 `MAX_ERROR_RETRIES` 是从 Go 文件保留的公开兼容常量，但本文件当前没有读取它们；Rust 的范围读取、重试、分片上传等行为来自 `inner`，不能据这两个常量推断本文件自行实现了这些流程。

## 核心职责

- `NewKS3Storage` 在进入通用 S3 构造链之前拒绝空区域和 `RoleArn`，将 provider 强制设为 `s3like::KS3SDKProvider`；构造成功后把通用链解析出的密钥、会话令牌、区域和 Object Lock 状态回写给调用方的 `backend`。
- `s3BucketExistenceCheckKS3`、`listObjectsCheckKS3`、`getObjectCheckKS3`、`putAndDeleteObjectCheckKS3` 保存 KS3 SDK 对应的权限探测语义，便于与 Go 迁移行为对照或独立调用；当前 `NewKS3Storage` 没有把它们接入生产构造链，生产构造使用的是 `NewS3Storage` 的通用 `s3like::CheckPermissions`。
- `impl storeapi::Storage for KS3Storage` 将读、写、存在性检查、删除、打开、批量删除、遍历、创建 writer、重命名与关闭委托给 `inner`，同时提供 `ks3://` URI，并明确拒绝 `PresignFile`。
- `CopyFrom` 修正 KS3 与 AWS S3 的覆盖语义差异：目标已存在时先删除目标，再重试服务端复制。
- `NewKS3StorageForTest` 允许注入实现 `S3API` 的 mock，使测试无需真实凭证和网络。

## 主要符号

- `pub struct KS3Storage { inner: s3like::Storage, options: backuppb::S3 }`：保存通用实现和构造后的配置快照。字段不公开，外部通过 trait 或 `GetOptions` 访问。
- `pub fn NewKS3Storage(ctx, backend, options) -> Result<KS3Storage>`：生产构造入口；`backend` 是可变参数，因为凭证等字段可能被通用构造链清除或回填。
- `pub fn NewKS3StorageForTest<T: S3API + 'static>(...) -> KS3Storage`：测试入口，使用 `Arc<T>` 注入 API，并可传入访问统计对象。
- 四个 `*CheckKS3` 函数：分别执行 HeadBucket、ListObjects v1、对固定不存在 key 的 GetObject，以及临时对象 Put/Delete 探测。
- `buildPutObjectInputKS3`：拼接 `Prefix + file`，并把非空的 ACL、SSE、KMS key、StorageClass 转为请求可选字段；`non_empty` 是其内部辅助函数。
- `int64p`、`boolP`：把值包装为 `Option`，用于保留 Go 指针辅助函数的接口语义；本文件当前未在生产流程中调用。
- `maybeObjectAlreadyExists`：同时识别历史错误码拼写 `ObjectAlreayExists` 与正确拼写 `ObjectAlreadyExists`。
- `KS3Storage::GetOptions`：返回内部配置的共享引用；`KS3Storage::CopyFrom`：执行同类型 KS3 存储之间的复制与冲突恢复。
- `impl storeapi::StrongConsistency`：空的 `MarkStrongConsistency` 是能力标记，不产生状态变化。
- `impl storeapi::Storage`：对象存储统一接口实现，其中只有 `URI` 和 `PresignFile` 是本层直接给出结果，其余方法主要委托 `inner`。

## 执行流程

生产构造按以下顺序运行：

1. `NewKS3Storage` 检查 `backend.Region` 非空，并拒绝非空 `RoleArn`；失败时尚未创建客户端。
2. 克隆 `backend` 为 `local`，将 `local.Provider` 固定为 `KS3SDKProvider`，再调用 `crate::NewS3Storage`。
3. `NewS3Storage`（`pkg/objstore/s3store/store.rs`）负责上下文校验、SDK/runtime 和 API 创建、凭证处理、prefix 规范化、通用权限检查、可选 Object Lock 探测，并返回 `s3like::Storage`。
4. 成功后，把 `local` 中的 AccessKey、SecretAccessKey、SessionToken、Region、ObjectLockEnabled 回写至原 `backend`；返回同时保存 `inner` 与 `local` 快照的 `KS3Storage`。

普通 I/O 调用从 `storeapi::Storage` 方法直接进入同名的 `inner` 方法。`Open`、`Create` 因底层接口取得拥有所有权的上下文而传入 `ctx.clone()`；`WalkDir` 用一层闭包转发可变回调；`URI` 从配置拼成 `ks3://<Bucket>/<Prefix>`；`PresignFile` 立即返回不支持错误。

`CopyFrom` 在循环中调用 `self.inner.CopyFrom(ctx, &source.inner, spec)`：成功即结束；遇到两种“目标已存在”错误码时，调用目标 `inner.DeleteFile(ctx, &spec.To)`，删除成功后进入下一轮重试；带有其他结构化服务错误码时按 Go 分支返回成功；没有服务错误码的错误则原样返回。删除失败会附加 `during deleting an exist object for making place for copy` 上下文。

权限探测各自只发最小请求：HeadBucket 检查桶；ListObjects v1 使用配置 prefix 且 `max_keys = 1`；GetObject 请求 key `not-exists`，`NoSuchKey` 证明读权限可达；Put/Delete 使用 `storeapi::GenPermCheckObjectKey()` 生成临时 key，并无论 Put 成败都尝试 Delete，最终优先返回 Put 错误，否则返回 Delete 结果。

## 数据与状态

`KS3Storage` 自身没有可变字段：`inner` 和 `options` 都在构造时确定。`options` 是本地配置副本，因此 `GetOptions` 不会借用调用方传入的 `backend`；调用方后续改动原配置不会改变该实例的 URI 或桶前缀。真正的客户端、访问计数、上传/读取状态和并发细节都封装在 `s3like::Storage` 及其下层对象中。

对象路径遵循“配置前缀 + 相对文件名”。`buildPutObjectInputKS3` 和权限清理直接做字符串拼接，依赖生产构造链已通过 `storeapi::NewPrefix` 规范化 prefix；测试构造不会自动规范化，测试或新调用者必须传入期望的尾斜杠。`URI` 也直接使用保存的 Bucket 与 Prefix，不做 URL 编码或二次规范化。

`CopyFrom` 的循环不维护显式重试计数。其关键不变量是：只有识别出“目标已存在”才删除目标并重试；因此覆盖过程不是原子的，删除成功而后续复制失败时目标可能暂时或最终缺失。`MAX_SKIP_OFFSET_BY_READ`、`MAX_ERROR_RETRIES` 当前只是兼容常量，不控制此循环。

## 依赖与调用关系

模块边界证据来自 `pkg/objstore/s3store/lib.rs` 和 `Cargo.toml`。直接 Rust 依赖包括：`anyhow` 提供统一错误和上下文；`storeapi` 提供 `Context`、`Options`、`Storage`、`StrongConsistency`、`CopySpec` 及 reader/writer 选项；`s3like` 提供通用 `Storage` 与 KS3 provider 常量；`objectio` 提供 reader、writer 和访问统计；`backuppb::S3` 由 crate 入口从 `s3like` 再导出；本 crate 的 `S3API` 和请求输入类型来自 `interface.rs`。

明确的下游边包括：`NewKS3Storage -> crate::NewS3Storage`，`NewKS3StorageForTest -> crate::NewS3StorageForTest`，四个权限函数到相应 `S3API` 方法，`CopyFrom -> s3like::Storage::CopyFrom/DeleteFile`，以及 `storeapi::Storage` 各方法到 `inner` 同名方法。RustCodeGraph 将 `pkg/objstore/s3store/s3_test.rs` 标为本文件使用者，并识别 `NewKS3StorageForTest` 被 `test_ks3_create_honors_part_size` 调用；`ks3_test.rs` 直接调用测试构造、`CopyFrom`、URI 和权限辅助函数。

RustCodeGraph 的精确 `callers/callees` 命令对本文件符号未输出静态边，因此本说明没有虚构更远的生产调用者；上游能可靠确认的是模块再导出、trait 动态分派和上述测试入口。运行时通过 `storeapi::Storage` trait 使用时属于动态边界。

## 错误处理与边界

- 构造边界：空 `Region` 返回 `ks3 region is empty`；任何 `RoleArn` 返回明确的不支持错误。只有 `crate::NewS3Storage` 成功后才执行本层回写。
- Get 权限探测：`NoSuchKey` 是成功信号；其他带服务错误码的错误失败；没有可提取错误码的错误按 Go SDK 类型断言失败分支视为成功。这一兼容行为可能掩盖普通传输错误，`ks3_test.rs` 明确锁定了当前语义。
- Put/Delete 探测：始终尝试清理；Put 与 Delete 都失败时返回 Put 错误，Put 成功而 Delete 失败时返回 Delete 错误。Go 版本对非 `NoSuchKey` 的清理失败额外记录警告，Rust 没有这项日志副作用。
- Copy：结构化但非“已存在”的服务错误被视为成功，这是对 Go 分支的刻意兼容；非结构化错误向上传播。已存在分支没有重试上限，若服务持续返回同一错误且删除持续成功，调用可能长期循环。
- `PresignFile` 无条件失败；调用者必须在选择 KS3 后端时处理该能力缺失。
- 本层委托方法不改写底层错误；具体 NotFound、范围读取、批量删除和 writer close 行为应以 `s3like::Storage` 及其独立测试为准。

## 并发与资源生命周期

`KS3Storage` 通过其字段类型和 `storeapi::Storage: Send + Sync` 面向并发共享；本文件没有 `Mutex`、通道、后台任务或显式 runtime。`Arc<T>` 仅出现在测试构造入口，用于把可共享的 `S3API` 交给通用测试构造器。

网络响应体、分片 writer、预取 reader、访问统计和 SDK runtime 的生命周期由 `inner` 管理。本层的 `Close` 只是委托 `inner.Close()`；`MarkStrongConsistency` 是空操作。权限探测创建的临时对象由同一次 `putAndDeleteObjectCheckKS3` 调用同步清理，但清理失败会作为结果返回，并可能留下对象。`CopyFrom` 的删除再复制序列没有锁或事务保护，既不原子，也不能阻止其他客户端在两步之间观察或写入目标。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/s3store/ks3.go`。Rust 保留了 `KS3Storage`、生产/测试构造、四类权限探测、Put 请求字段、两种对象已存在错误码、`ks3://` URI、Presign 禁用和覆盖复制策略。

实现结构并非逐行复刻。Go 的 `KS3Storage` 直接持有 KS3 SDK client，并在同一文件实现读写、范围 reader、seek/retry、遍历、分片上传和异步 writer；Rust 把这些通用 S3 能力收敛到 `s3like::Storage`，本文件主要负责厂商差异。Go 构造器直接配置 KS3 SDK、凭证和访问记录 handler，并通过 `permissionCheckFnKS3` 调用四个 KS3 探测函数；Rust 通过将 provider 设为 `KS3SDKProvider` 后调用 `NewS3Storage` 复用统一 SDK 构造与通用权限检查链，四个同名 Rust helper 当前没有从生产构造器接线。

已验证的细节差异包括：Go `CopyFrom` 接收任意 `storeapi.Storage` 并运行时检查是否为 KS3，Rust 方法签名直接要求 `&KS3Storage`；Go 的已存在分支递归重试，Rust 用循环；Go 对其他结构化 Copy 错误最终返回 `nil`，Rust同样返回成功；Go Put/Delete 权限探测会对部分清理错误告警后忽略，而 Rust 当前在 Put 成功时返回 Delete 错误。Go 的 `maxSkipOffsetByRead`、`maxErrorRetries` 驱动本文件内 reader，Rust 同名常量在这里未接线，真正 reader 行为由通用内层负责。

Rust 测试 `pkg/objstore/s3store/ks3_test.rs` 对应验证 URI、删除后重试、ListObjects v1 参数、Get 错误分类、Put/Delete 结果、请求字段和双拼写错误码；`pkg/objstore/s3store/s3_test.rs::test_ks3_create_honors_part_size` 验证经 `inner.Create` 的 KS3 分片仍尊重 `WriterOption.PartSize`。Go 的 `pkg/objstore/s3store/s3_test.go::TestKS3CreateHonorsPartSize` 是后者的语义对照。

## 扩展指南

新增 KS3 专属构造限制或配置回写时修改 `NewKS3Storage`，并同步检查 `store.rs::NewS3Storage` 已承担的通用逻辑，避免重复或绕过 `SendCredentials`、prefix 规范化和权限检查。新增权限探测应优先落在统一权限抽象能够表达的位置；若必须保留 KS3 SDK 特例，则为辅助函数增加 `ks3_test.rs` 用例，覆盖请求参数、服务错误码、普通错误和清理失败。

新增对象操作前先判断是否真是 KS3 差异：通用行为应进入 `s3like::Storage` 或 S3 client，而不是在此重新实现。若扩展 `CopyFrom`，必须显式决定重试上限、幂等性和删除后失败的恢复策略，并保持两种历史错误码兼容；若改变非“已存在”结构化错误的吞掉行为，应同时评估 Go 兼容性。若增加 Presign 支持，需要替换当前明确错误，并增加有效期、凭证泄漏和 endpoint/path-style 组合测试。

测试逻辑应继续放在独立文件：本层专属行为放 `pkg/objstore/s3store/ks3_test.rs`，经通用 S3 内层实现的综合行为放 `pkg/objstore/s3store/s3_test.rs`；不要把测试内嵌进 `ks3.rs`。修改 Go 对齐语义时同步核对 `ks3.go` 和相关 Go 测试，但不要为让测试通过而删减 Rust 逻辑。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `files --filter pkg/objstore/s3store`：确认目标、模块入口、独立 Rust 测试及 Go 对照文件均被索引。
- RustCodeGraph `node --file pkg/objstore/s3store/ks3.rs --offset 1 --limit 500`：读取目标文件全部 299 行；结果报告 35 个符号，并将 `s3_test.rs` 标为使用者。
- RustCodeGraph `query KS3Storage`、`query NewKS3Storage`、`query NewS3Storage`、`query TestKS3CreateHonorsPartSize`、`query test_ks3_create_honors_part_size`：核对主要类型、构造入口、通用构造下游及 Go/Rust 测试位置。精确 `callers/callees` 查询未返回输出，此限制已在调用关系章节披露。
- 已读取路径：`pkg/objstore/s3store/lib.rs`、`store.rs`、`ks3_test.rs`、`s3_test.rs`、`pkg/objstore/storeapi/storage.rs`、Go 对照 `pkg/objstore/s3store/ks3.go` 与 `s3_test.go`，以及未被代码图覆盖的 `pkg/objstore/s3store/Cargo.toml`。该目录不存在 `doc.go`。
- 本任务只新增说明文档，没有运行 Cargo；最终结构检查验证文档存在且恰好包含任务要求的 11 个固定二级标题。
