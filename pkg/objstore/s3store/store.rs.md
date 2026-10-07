# `pkg/objstore/s3store/store.rs`

## 文件定位

本文件是 `astersql-objstore-s3store` crate 的 S3 存储工厂与凭证装配层。模块入口 `pkg/objstore/s3store/lib.rs` 以 `mod store` 引入并用 `pub use store::*` 导出这里的公开符号；crate 边界和 AWS SDK、HTTP、序列化、异步运行时等依赖由 `pkg/objstore/s3store/Cargo.toml` 定义。

它位于调用方配置与数据面实现之间：`NewS3Storage` 接收 `backuppb::S3` 和 `storeapi::Options`，组装 AWS SDK 客户端及 `S3Client`，再交给 `s3like::NewStorage` 形成统一对象存储门面。当前已确认的生产入口有 `pkg/session/runtime/modify_column_cloud_store.rs` 的云存储构造分支，以及 `pkg/objstore/s3store/ks3.rs::NewKS3Storage` 对通用 S3 工厂的复用。对象的 Get/Put/List 等数据面行为不在本文件，而在 `client.rs`、`interface.rs` 和 `s3like` crate 中。

## 核心职责

- `NewS3Storage` 完成上下文检查、区域默认值/探测、凭证链、AssumeRole、endpoint、path-style、HTTP client、重试策略、GCS 签名器、权限检查、对象锁探测和最终存储包装。
- `credential_source`、`autoNewCred` 选择显式静态密钥、阿里云 ECS RAM 回退、腾讯 CVM CAM 角色或 AWS 默认凭证链；Profile 由 AWS loader 直接处理，优先于 `autoNewCred`。
- `FallbackCredentialsProvider` 保证阿里云 endpoint 先尝试完整 AWS 凭证链，仅在失败后访问 RAM 元数据。
- `TencentCvmRoleCredential` 缓存腾讯角色名和临时凭证，在到期前五分钟尝试刷新；`TencentCvmRoleCredentialsProvider` 把它转换为 AWS SDK 凭证。
- `is_gcs_s3_compatible` 以 provider 或合法 endpoint 主机名识别 GCS XML API，并使构造流程跳过 AWS bucket-region 探测、安装专用签名器。
- `NewS3StorageForTest` 提供可注入 `S3API` 的测试构造路径，不执行真实凭证装载、区域探测或权限检查。

## 主要符号

- 常量：`DEFAULT_REGION` 是未配置区域时的 `us-east-1`；`DOMAIN_ALIYUN`、`DOMAIN_TENCENTCLOUD_LEGACY`、`DOMAIN_TENCENTCLOUD` 用于 endpoint 分类；三个私有元数据常量及 `TENCENT_REFRESH_WINDOW_SECS` 定义元数据位置和刷新窗口。
- `CredentialSource`：公开枚举，区分 `Static`、`AliyunMetadata`、`TencentCvmRole`、`DefaultChain`。它描述选择结果；阿里云分支在 `autoNewCred` 中返回 `None`，真正的 RAM provider 是在 `load_sdk_config` 中作为 AWS 链的 fallback 安装。
- `ClientPurpose`：私有枚举，区分普通存储客户端与区域探测客户端。`build_api` 据此让区域探测固定采用 `S3StandardRetryer`，避免把调用方的数据面重试器用于预检。
- `NewS3Storage(ctx, backend, options) -> Result<s3like::Storage>`：主工厂。`backend` 可被回写区域、凭证和 `ObjectLockEnabled`，最终 `Storage` 则保存规范化后的配置副本。
- `build_api` / `load_sdk_config`：前者把全局 SDK 配置变为 S3 service client，后者负责 region、profile/凭证、HTTP/retry 及 STS AssumeRole。
- `retry_config_for_options`、`retry_classifier_for_options`、`http_client_for_options`：把 `storeapi::Options` 中的可注入策略投影到 AWS SDK；未给重试器时回落到 `S3StandardRetryer`。
- `IsObjectLockEnabled`：通过 `S3API::get_object_lock_configuration` 查询桶设置，任何 API 错误均折叠为 `false`。
- `FallbackCredentialsProvider<P, F>` 与 `fallback_credentials_provider`：可测试的两级 provider；主链失败后才调用 fallback，两者都失败时合并错误上下文。
- `AliyunRamCredentialsProvider`、`load_aliyun_ram_credentials`、`parse_aliyun_ram_credential`、`createOssRAMCred`：阿里云 RAM 元数据的异步 provider、解析器和兼容用阻塞入口。解析保留临时凭证到期时间。
- `TencentCredential`、`TencentCvmRoleCredential`、`TencentCvmRoleCredentialsProvider`、`createTencentCOSCred`：腾讯 CVM 元数据加载、带锁缓存、AWS provider 适配和容错构造。
- `is_tencent_cos_endpoint`、`is_gcs_s3_compatible`、`valid_url_escapes`：provider/endpoint 分类与 Go URL 语义兼容辅助函数。

## 执行流程

1. `NewS3Storage` 先执行 `ctx.check()`，克隆 `backend` 为局部 `query`，为空区域选取 `DEFAULT_REGION`，并创建供同步 API 阻塞异步 SDK 使用的 Tokio `Runtime`。
2. `load_sdk_config` 设置 region、重试与共享 HTTP client。Profile 非空时交由 AWS 配置加载器；否则 `autoNewCred` 可注入静态密钥或腾讯 CVM provider，未命中则保留 AWS 默认链。阿里云 endpoint 会在默认链外再包一层 `FallbackCredentialsProvider`。`RoleArn` 非空时，最后用可选 `ExternalId` 的 STS `AssumeRoleProvider` 替换凭证 provider。
3. `build_api` 设置 `ForcePathStyle`、HTTP client 和重试分类器；endpoint 只设置在 S3 service client 上，避免污染 AssumeRole 使用的 STS endpoint；GCS 兼容配置还安装 `gcs_s3_signer`。
4. 按 `SendCredentials` 处理调用方持有的 `backend`：关闭时清空三项秘密；开启且原配置不完整时，尝试从 SDK provider 解析当前凭证并回填。provider 解析失败不会在这里终止构造。
5. 仅 provider 为空或 `aws` 且不是 GCS 时通过独立 region-probe client 调用 `bucket_region`。空探测结果归一为 `us-east-1`；若用户显式区域与真实区域不同则报错，若原区域为空则回写探测值，并在非默认区域时重建 SDK 配置和普通客户端。其他兼容服务直接信任配置区域。
6. 使用 `storeapi::NewPrefix` 规范化前缀，构造 `BucketPrefix` 和 `S3Client`。`S3Client` 的 `s3Compatible` 参数为 `!official_s3`。
7. `s3like::CheckPermissions` 按 `options.CheckPermissions` 顺序执行桶访问、列举、读取或写删探测；任一错误终止构造。
8. 开启 `CheckS3ObjectLockOptions` 时调用 `IsObjectLockEnabled` 回写 `backend.ObjectLockEnabled`；最后把 client、前缀、局部配置和访问统计交给 `s3like::NewStorage`。

## 数据与状态

`NewS3Storage` 刻意区分两份配置：`backend` 是调用方所有、允许回写的配置；`query` 是构造期快照，经过 region 和 prefix 规范化后存入返回的 `s3like::Storage`。因此秘密清除或凭证回填作用于 `backend`，而数据面客户端使用的 `query` 仍保留创建连接所需信息。区域探测同时更新两者；对象锁结果只写回 `backend`。

`aws_types::SdkConfig` 承载 region、HTTP client 和 credentials provider，`build_api` 再叠加仅属于 S3 的 endpoint、path-style、重试分类器和 GCS signer。`Arc<AwsS3Api>` 让权限检查、对象锁检查与最终 `S3Client` 共享 API 实例；`AccessRecording` 也被克隆进入 API 和最终 storage。

腾讯凭证状态由 `Mutex<TencentRoleState>` 保护，其中固定保存角色名并更新最近一次 `TencentRoleResponse`。刷新条件是字段不完整，或 `ExpiredTime - 300 秒 <= 当前 Unix 时间`。AWS 适配层把导出的凭证到期时间设为当前时刻，促使外层 AWS cache 再次调用本 provider，从而观察内部刷新结果。

## 依赖与调用关系

上游生产调用关系为：

- `pkg/session/runtime/modify_column_cloud_store.rs` → `astersql_objstore_s3store::NewS3Storage`，用于 modify-column 云端临时存储构造。
- `pkg/objstore/s3store/ks3.rs::NewKS3Storage` → `crate::NewS3Storage`，先把 provider 标成 KS3，再复用工厂并回写解析后的凭证、区域和对象锁状态。
- `pkg/objstore/s3store/lib.rs` → `store.rs`，负责 crate 内模块装配与公开再导出。

主下游调用关系为：`NewS3Storage` → `load_sdk_config` / `build_api` → AWS config、S3 client、STS provider；`NewS3Storage` → `S3Client::new` → `s3like::CheckPermissions` → `s3like::NewStorage`。区域探测通过 `AwsS3Api::bucket_region`，对象锁通过抽象 `S3API`，因此测试能以 mock 替代网络。

`Cargo.toml` 证实外部边界包括 `aws-config`、`aws-credential-types`、`aws-sdk-s3`、`aws-smithy-types`、`aws-types`、`reqwest`、`tokio`、`serde`、`serde_json` 和 `tracing`；仓库内边界包括 `objectio`、`s3like`、`storeapi`。该 crate 没有 feature 条件，`store.rs` 本身也没有条件编译项。

## 错误处理与边界

- 主工厂以 `anyhow::Result` 传播 runtime 创建、SDK 配置、region 探测和权限检查错误，并用 `Context` 添加阶段信息。显式区域不匹配是硬错误，不静默覆盖用户配置。
- `SendCredentials=true` 时解析当前 provider 失败会被忽略，构造可继续；这只意味着无法回填下游配置，不代表后续实际 S3 请求必然失败。`SendCredentials=false` 始终清空调用方的三项秘密。
- `IsObjectLockEnabled` 把 API 错误与“未启用”统一成 `false`，调用者无法从布尔值区分无锁和查询失败。
- 阿里云异步 loader 对非成功 HTTP、空角色名返回 `Ok(None)`；JSON、时间解析或网络构建错误会传播。`FallbackCredentialsProvider` 只在完整 AWS 主链失败后访问元数据，并在双重失败时保留两侧错误文本。
- `createTencentCOSCred` 把元数据初始化失败记录为 warning 并返回 `Ok(None)`，允许退回 AWS 默认链。刷新失败也只告警并继续返回缓存值；但空 access key、secret 或 token 会在 AWS provider 适配层成为错误。锁中毒是明确错误。
- GCS 判定刻意模拟 Go `url.Parse` / `Hostname` 的边界：拒绝非法 scheme、反斜线、控制字符、非法转义和 IDNA 造成的全角主机折叠；接受纯数字的大端口，并只匹配 `storage.googleapis.com` 本身或其点分子域。
- 元数据 URL 是固定 link-local HTTP 地址；调用方不能通过本文件配置它们。阿里云 client 有 2 秒总超时，腾讯阻塞 client 未在这里设置显式超时，这是扩展时需要评估的资源风险。

## 并发与资源生命周期

每次 `NewS3Storage` 创建一个多线程 Tokio `Runtime`，并以 `Arc` 交给 `AwsS3Api`；返回 storage 持有 API，因此 runtime 随客户端共享引用一起存活。构造阶段以 `block_on` 同步等待 SDK 配置、provider 和网络操作，不创建由本文件管理的独立后台任务。

AWS SDK 的 client、credentials provider、共享 HTTP client 和访问统计均通过 `Arc`/共享包装跨调用复用。AssumeRole provider 由 SDK 负责缓存；`s3_test.rs::test_s3_storage_caches_assumed_role_credentials` 验证两次 S3 操作只触发一次 STS assume-role。

腾讯角色状态使用标准库 `Mutex` 串行化读取与刷新。刷新网络请求发生在持锁期间，可避免多个线程同时刷新，但慢请求会阻塞其他凭证读取；刷新失败保留旧状态。阿里云 fallback provider 本身无可变状态，缓存与到期刷新交给 AWS SDK 的 credentials cache。`NewS3StorageForTest` 仅共享调用方提供的 `Arc<S3API>`，不引入 runtime 或元数据生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/s3store/store.go`。主流程在两版中一致：默认 region、profile 优先、静态/腾讯/默认凭证选择、阿里云 AWS-chain-first fallback、S3 局部 endpoint、可选 HTTP client、AssumeRole、`SendCredentials`、AWS 区域探测、前缀规范化、权限检查、对象锁回写和测试注入构造。

Rust 用 `AwsS3Api` 和自有 `S3Client` 适配 AWS SDK，用显式 Tokio runtime 把异步 SDK 封装进当前同步接口；Go 直接使用 SDK v2 client。Go 的区域探测调用 `manager.GetBucketRegion` 并指定专用 retryer，Rust 通过 `ClientPurpose::RegionProbe` 构建独立 API 达到相同隔离目的。Go 的访问记录通过 Smithy middleware 注入，Rust 由 `AwsS3Api::new` 接收 `AccessRecording`。

Rust 额外保留 `credential_source`、`parse_aliyun_ram_credential`、`createOssRAMCred` 等可直接测试辅助符号，并手工实现 GCS URL 边界以贴合 Go 解析行为。两版对象锁查询失败都降级为 false；两版腾讯初始化失败都警告并回退。迁移状态不是桩：真实 AWS client、凭证 provider、权限与 region 流程均已接线，并有独立 Rust 测试覆盖；但本次纯文档任务没有运行这些测试。

## 扩展指南

- 新增凭证来源时，优先修改 `CredentialSource`、`credential_source`、`autoNewCred` 或 `load_sdk_config` 中恰当的一层，并在独立测试文件中验证优先级、失败回退、到期时间和秘密回写；不要把单元测试内嵌进 `store.rs`。
- 新增 provider/endpoint 分类时，应同步审查 `official_s3`、区域探测、`S3Client::new` 的兼容标志、endpoint 是否只作用于 S3，以及 GCS/腾讯专用逻辑。URL 行为要与 Go 对照，尤其是 Unicode、相对 URL、端口和转义边界。
- 修改重试或 HTTP 注入时，要同时覆盖 SDK 配置加载器、普通 S3 client 与 region-probe client；区域探测必须保持独立于用户数据面 retryer。对应测试入口是 `client_1_aster_unit_test.rs` 和 `s3_test.rs::region_probe_uses_301_bucket_region_header`。
- 修改 region 或凭证回写时，要明确 `backend` 与 `query` 两份状态的可见性，复核 `SendCredentials=false` 不泄漏 secret，并同步 `test_send_creds`、`test_s3_storage_bucket_region`、AssumeRole 缓存测试及 KS3 回写行为。
- 修改阿里云/腾讯元数据时，要评估网络超时、锁持有时间、旧凭证是否仍有效和日志中是否泄漏秘密；同步 `s3_test.rs` 的 fallback/expiration 测试与 `tencent_cos_test.rs` 的刷新、不完整凭证测试。
- 修改构造顺序或权限检查时，应同步 `s3like/permission_test.rs` 和 S3/GCS 工厂测试，防止在权限探测前错误地进行 AWS region 请求，或把 STS endpoint 改写成 S3 endpoint。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标 `store.rs` 有 67 个符号；`files --filter pkg/objstore/s3store` 确认 crate 内源/测试集合；`node --file pkg/objstore/s3store/store.rs --offset 1 --limit 500` 与后续 `--offset 500 --limit 400` 覆盖全部 771 行。`query` 确认 Rust/Go 两版 `NewS3Storage`、`autoNewCred`、`IsObjectLockEnabled`、`createTencentCOSCred` 以及 Rust `is_gcs_s3_compatible`。精确 `callers`/`callees` 未输出函数边，因此调用者由下述直接引用搜索补证，不把缺失图边推断成“无调用者”。
- 源与边界：`pkg/objstore/s3store/store.rs`、`lib.rs`、`Cargo.toml`；直接下游 `client.rs`、`interface.rs`、`pkg/objstore/s3like/permission.rs`、`pkg/objstore/s3like/store.rs`、`pkg/objstore/storeapi/storage.rs`。
- 上游直接引用：`pkg/session/runtime/modify_column_cloud_store.rs`、`pkg/objstore/s3store/ks3.rs`；`rg` 还确认测试从 crate 公开门面调用工厂和辅助函数。
- Go 对照：`pkg/objstore/s3store/store.go`；腾讯 Go 辅助实现位于 `pkg/objstore/s3store/tencent_cos.go`。
- Rust 测试证据：`s3_test.rs` 覆盖静态/默认凭证、阿里云主链优先与 fallback、RAM expiration、SendCredentials、对象锁、region、region-probe retry 隔离、AssumeRole 缓存；`gcs_s3_test.rs` 覆盖跳过区域探测、签名请求和 endpoint 边界；`tencent_cos_test.rs` 覆盖 endpoint 选择、静态密钥优先、轮换和不完整凭证；`client_1_aster_unit_test.rs` 覆盖对象锁与 Options 中重试/HTTP client 的选择。
- 本任务只生成说明文档，按计划未运行 Cargo。交付前以任务指定命令检查目标存在且恰有 11 个固定二级章节，并人工复核无运行能力的无依据声明。
