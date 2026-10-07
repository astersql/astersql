# `pkg/objstore/ossstore/store.rs`

## 文件定位

本文件是 `astersql-objstore-ossstore` crate 的存储构造层：它把兼容 BR protobuf 的 `s3like::backuppb::S3` 配置、`storeapi::Options` 和 OSS 凭证组合成 `OSSStore`。真正的对象请求由同 crate 的 `Client` 和 `AliyunOssApi` 执行，本文件负责其之前的凭证选择、桶地域探测、endpoint 选择、权限预检，以及其之后的资源关闭。

`pkg/objstore/ossstore/lib.rs` 通过 `mod store; pub use store::*;` 导出这里的公开符号；crate 边界和依赖见 `pkg/objstore/ossstore/Cargo.toml`。根 `Cargo.toml` 以 `facade_objstore_ossstore` 登记该 crate，但当前 Rust 生产源码搜索不到 `NewOSSStorage` 的静态调用。统一入口 `pkg/objstore/storage.rs::New` 对网络后端使用 `Options::external_factory`，因此这里是可供集成工厂调用的完整 OSS 实现，不能据现有静态边声称它已由统一入口直接接线。

对应的 Go 实现是 `pkg/objstore/ossstore/store.go`。Rust 文件不是门面或桩：`NewOSSStorage` 包含真实联网探测和资源管理逻辑；只有 `new_oss_storage_for_test` / `new_oss_store_for_test` 是绕过网络的测试构造入口。

## 核心职责

- `NewOSSStorage` 复制调用方配置，选择静态凭证或 reqsign 默认凭证链，并按 `SendCredentials` 决定是否把当前凭证快照回填给调用方持有的 backend。
- 先以输入地域（为空时用 `DEFAULT_REGION`）访问 Bucket Location，再以服务返回的真实地域构造数据客户端；显式地域与真实地域不一致时拒绝创建。
- 仅当凭证提供者表明其来自 ECS RAM Role、元数据给出的 ECS 地域非空且与桶地域相同，才选择 OSS 内网 endpoint；自定义 `Endpoint` 始终优先。
- 数据 API 可以使用内网 endpoint，而预签名 API 默认保持公网 endpoint，因为预签名 URL 可能由 VPC 外部消费者使用。
- 在返回存储前调用 `s3like::CheckPermissions`，动态凭证则在初始化和权限检查成功后启动后台刷新。
- `OSSStore::URI` 保持 `oss://` 协议语义，`OSSStore::Close` 同时关闭底层存储和动态凭证刷新线程。

## 主要符号

- `DEFAULT_REGION: &str = "cn-hangzhou"`：未配置地域时，仅用于首次 Bucket Location 探测的默认地域。
- `ECS_RAM_ROLE_PROVIDER_NAME` 与 `REGION_ID_META_URL`：判定 ECS RAM Role 凭证及查询实例地域的常量。
- `OSSStore { pub Storage, credential_refresher }`：公开包装的 `s3like::Storage` 和私有的可选 `Arc<CredentialRefresher>`。静态 AK/SK 时后者为 `None`。
- `OSSStore::URI(&self) -> String`：从 `Storage.GetBucketPrefix()` 生成 `oss://<bucket>/<normalized-prefix>`。
- `OSSStore::Close(&self)`：调用 `Storage.Close()`，随后停止并 join 凭证刷新器。
- `prepare_backend(&mut S3, bool) -> Result<S3>`：当前只克隆 backend；保留参数形状以隔离随后对副本 `qs` 的规范化和对原 backend 的凭证回填。
- `set_backend_credentials(&mut S3, &dyn CredentialsProvider, bool)`：关闭转发时清空三个密钥字段且不访问 provider；开启时一次性取得完整快照后依次写入 AK、SK、SessionToken。
- `endpoint_for_region(region, internal)`：生成公网或 `-internal` 的 HTTPS endpoint。
- `trim_oss_region_id(region)`：只剥离一个开头的 `oss-`，使 Location API 返回值可与配置地域比较。
- `can_use_internal_endpoint(ecs_region_id, bucket_region_id)`：要求 ECS 地域非空且严格相等。
- `metadata_region(provider_name)`：只有 provider 名包含 `ecs_ram_role` 才用阻塞 reqwest 客户端访问 ECS 元数据，连接和总超时均为 30 秒；其他凭证来源返回空串。
- `build_api(credentials, qs, region, internal, access_rec)`：自定义 endpoint 优先，否则按地域生成 endpoint，再构造 `AliyunOssApi`。
- `NewOSSStorage(ctx, backend, opts) -> Result<OSSStore>`：本文件的生产入口。
- `new_oss_storage_for_test`：把注入的 `Arc<dyn API>` 直接封装进 `Client::from_dyn` 和 `s3like::NewStorage`。
- `new_oss_store_for_test`：仅在 `cfg(test)` 下再包装成 `OSSStore`，不带刷新器。
- `trimOSSRegionID`、`canUseInternalEndpoint`：保留 Go 命名风格的公开别名，逻辑委托给 snake_case 实现。

## 执行流程

`NewOSSStorage` 的顺序具有资源和安全含义：

1. `prepare_backend` 克隆 `backend` 为局部 `qs`。因此 prefix 规范化等构造期修改不会污染调用方；只有凭证转发函数有意修改原 backend。
2. `ForcePathStyle` 为真时记录“不支持”警告，但不立即失败，也不改变后续 virtual-hosted 请求路径。
3. 若 AK 与 SK 同时非空，创建 `StaticCredentialsProvider` 并且不创建刷新器；只提供其中一个字段不会进入静态分支，而会走默认链。
4. 默认链分支以 `RoleArn`、`ExternalId` 创建 `ReqsignCredentialsProvider`，包装成 `CredentialRefresher`，先 `refresh_once`，确保后续读取的是已初始化快照。
5. `set_backend_credentials` 按 `opts.SendCredentials` 回填当前快照或清空调用方 backend 的敏感字段。构造用的局部 `qs` 已在此之前克隆，仍保留本次客户端所需配置。
6. 再读取当前凭证快照，根据 `provider_name` 决定是否查询 ECS 元数据地域。非 ECS 来源不会发起元数据 HTTP 请求。
7. 使用输入 Region，或为空时使用杭州默认值，构造公网 `location_api`；调用 `bucket_location(ctx, Bucket)` 得到真实地域并去掉 `oss-` 前缀。
8. 调用方显式填写了 Region 且与探测值不等时返回错误；未填写时接受探测结果。
9. 计算是否可用内网 endpoint，规范化 `qs.Prefix`，生成 `BucketPrefix`。随后创建数据 API；另建一个 `internal=false` 的预签名 API。若配置了自定义 endpoint，两者都保留该 endpoint。
10. `Client::with_presign_api` 将数据与预签名请求面分开；`s3like::CheckPermissions` 在启动后台线程前完成权限验证。
11. 动态凭证场景启动周期刷新，最后由 `s3like::NewStorage` 构造通用对象存储并返回 `OSSStore`。

测试构造流程更短：`new_oss_storage_for_test` 不解析凭证、不探测地域、不检查权限、不启动刷新线程，只建立 bucket/prefix、注入 `API` 并交给 `s3like::NewStorage`。它适合验证对象操作适配，不是生产构造等价物。

## 数据与状态

`backend` 有两种视图。原始可变引用属于调用方，只由 `set_backend_credentials` 改写密钥字段；局部 `qs` 是构造快照，随后会规范化 Prefix，并被克隆进 `Client` 与 `s3like::Storage`。这保证发送给 TiKV 的凭证策略与当前进程使用的客户端配置可以分离。

凭证通过 `Arc<dyn CredentialsProvider>` 共享给 Location API、数据 API 和预签名 API。动态场景中的实际对象是 `CredentialRefresher`：据 `pkg/objstore/ossstore/credential.rs`，它用 `ArcSwapOption` 发布不可变整快照，用 `Mutex<Option<JoinHandle>>` 保管唯一工作线程，用 `Mutex<bool> + Condvar` 实现可唤醒停止。调用端不会观察到半更新的 AK/SK/Token 组合。

地域状态分为 `input_region`、`detected_region` 和可能为空的 `ecs_region_id`。真正的数据和预签名 API 都使用 `detected_region` 进行签名；是否内网仅影响默认 endpoint 的选择，不改变地域。`BucketPrefix` 由规范化后的 Prefix 创建，`OSSStore::URI` 因而稳定保留尾部 `/`；`store_test.rs::uri_preserves_oss_scheme_and_normalizes_prefix` 覆盖空前缀、缺少尾斜线和含 `%2E` 的前缀。

## 依赖与调用关系

上游方面，`lib.rs` 将本文件 API 再导出；根 workspace 为 crate 配置了 `facade_objstore_ossstore`。RustCodeGraph 对目标文件报告一个文件级“used by”关系到 `pkg/importsdk/file_scanner.rs`，但精确 `callers/callees` 查询未返回 `NewOSSStorage` 调用边，文本搜索也只在定义处找到该函数。`pkg/objstore/storage.rs::New` 的云后端走调用方提供的 `external_factory`，所以当前能确认的是可注入边界，而不是静态直连调用者。Go 主链则由 `pkg/objstore/storage.go` 明确调用 `ossstore.NewOSSStorage`。

下游关系可由 `NewOSSStorage` 源码直接核对：

- 凭证：`StaticCredentialsProvider`、`ReqsignCredentialsProvider`、`CredentialRefresher::{new,refresh_once,start_refresh,close}`。
- HTTP/OSS：`reqwest::blocking::Client` 读取元数据；`AliyunOssApi::new` 创建实际 API；`API::bucket_location` 探测桶地域。
- 适配：`Client::with_presign_api` 区分数据与预签名服务；`s3like::CheckPermissions` 执行能力探测；`s3like::NewStorage` 提供统一读写接口。
- 配置与观测：`storeapi::{NewPrefix,NewBucketPrefix}` 规范化对象键空间，`objectio::recording::AccessStats` 传给两个 API 和最终 Storage。

`Cargo.toml` 证明直接外部依赖包括 `ali-oss-rs`、reqsign 组件、阻塞 `reqwest`、`anyhow` 和 `log`；仓库内依赖是 `objectio`、`s3like`、`storeapi`。`tokio`、`arc-swap` 等也属于 crate 依赖，但本文件对后台刷新具体同步原语的使用是经 `credential.rs` 间接发生的。

## 错误处理与边界

所有生产构造错误通过 `anyhow::Result` 返回，并在关键跨层点增加上下文：默认凭证链配置、首次凭证拉取、发送给 TiKV 的凭证读取、ECS 元数据读取、Bucket Location 获取、权限检查和刷新线程启动。地域不匹配错误包含 bucket、输入地域和真实地域，便于定位错误配置。

重要边界如下：

- `ForcePathStyle` 仅警告，不代表支持；调用方不能依赖 path-style 行为。
- 自定义 `Endpoint` 覆盖公网/内网自动拼装，因此 `use_internal_endpoint=true` 不会重写用户 endpoint。
- provider 名不含 `ecs_ram_role` 时不访问 `100.100.100.200`；包含时元数据失败会使构造失败，而不是退回公网。
- Location 探测始终先走公网模式，但显式 endpoint 仍由 `build_api` 保留。
- `SendCredentials=false` 会主动清空原 backend 的三个敏感字段，并且不会调用 provider；`true` 时 provider 失败发生在赋值之前，因此已有字段保持不变。该不变量由 Rust `test_set_backend_credentials` 和 Go `TestSetBackendCredentials` 覆盖。
- 权限检查在返回存储前执行，失败统一包装为 `check permission failed due to ...`；Go 的真实 OSS 测试覆盖 bucket 不存在、List/Get/Put/Delete 权限成功与失败，Rust 注入测试不覆盖这些真实服务错误。
- `prepare_backend` 当前不可失败，但保留 `Result`；不要把这一现状解释为未来永远无校验。
- 本文件没有把 context 传给 ECS 元数据 reqwest 请求；取消语义主要由 `bucket_location` 与后续 API 的 `storeapi::Context` 承担。Go `TestSendCredentialsIsSupported` 验证已取消 context 下凭证仍先完成转发，再由后续调用返回取消错误；Rust 没有对应的真实构造测试证据。

## 并发与资源生命周期

静态凭证路径不创建线程，`OSSStore::Close` 只关闭底层 `s3like::Storage`。动态路径先同步取得一次凭证，权限检查通过后才调用 `start_refresh`，避免在构造中途较早启动后台资源。成功返回后，`OSSStore` 持有 `Arc<CredentialRefresher>`，API 也通过 trait object 共享同一刷新器快照。

`credential.rs` 显示默认刷新间隔为 5 秒，启动函数保证最多一个工作线程；线程等待 Condvar、周期调用 `refresh_once`，刷新失败只记录警告并保留上一份快照。`OSSStore::Close` 设置停止标志、唤醒并 join 线程；`CredentialRefresher::Drop` 也调用 `close`，提供遗漏显式关闭时的兜底。`close` 可重复调用，因为 worker handle 通过 `take()` 只 join 一次。

本文件的 `metadata_region` 和 `AliyunOssApi` 构造接口是阻塞式的；创建过程应视为可能执行网络 I/O 的同步操作。`AccessStats`、provider 和 API 都以 `Arc` 共享。测试辅助 `MemoryOssApi` 用 `Mutex` 保护对象表和 multipart 会话，并以 `AtomicUsize` 生成 upload id，证明经 `s3like::Storage` 的多分片/遍历行为可以在并发安全的 API 实现上运行；这不是对真实 SDK 并发上限的性能证明。

## 与 Go 版本的对应关系

Rust `OSSStore`、`URI`、`Close`、`NewOSSStorage`、`set_backend_credentials`、地域裁剪和内网判定分别对应 `store.go` 的同名或 snake_case 等价实现。两版共同保持以下语义：复制 backend 后构造、静态与默认凭证二选一、可选回填凭证、Bucket Location 探测、显式地域校验、同区 ECS 内网优化、Prefix 规范化、权限预检、动态凭证刷新，以及关闭刷新器。

实现依赖不同：Go 使用 Alibaba Cloud OSS SDK v2 与 `credentials-go`，Rust 使用 `ali-oss-rs` 和 reqsign 默认链。Rust 的 `build_api` 显式构造数据与预签名 API；Go 用 `newPresignClient` 复制配置、强制公网并关闭 SDK 日志以避免预签名 query 中凭证被原始日志暴露。Rust 代码能确认公网预签名 endpoint 的分离，但本文件没有与 Go `LogOff` 完全同形的日志开关，日志安全性需要继续在 `AliyunOssApi`/SDK 层核验，不能宣称完全等价。

另一个差异是 Go `NewOSSStorage` 直接从默认 provider 返回的 `ProviderName` 判断 ECS 来源；Rust `ReqsignCredentialsProvider::get_credentials` 在 `credential.rs` 当前将 `provider_name` 写为 `reqsign_default`。因此仅凭当前直接源码，动态 reqsign 路径是否能触发 `metadata_region` 的 ECS 分支存在迁移差异：条件函数本身已实现，但 provider 名传播未体现 Go 的串联 provider 名。扩展或修复时应先确认 reqsign 是否另有来源信息，不能假设自动内网选择已与 Go 完全对齐。

Go `TestStore` 和 `TestInternalEndpoint` 依赖真实 OSS 凭证且正常 CI 中跳过；Rust `store_test.rs` 用 `MemoryOssApi` 覆盖通用读写、范围读、批量删除、multipart、复制和分页遍历，但同样没有真实 OSS 构造测试。因此 Rust 的对象适配行为有确定性证据，真实凭证链、地域探测和 endpoint 联网兼容性仍主要由源码对照支持。

## 扩展指南

- 新增凭证来源或改变 RoleArn/ExternalId 行为，应修改 `credential.rs` 的 provider，并同步检查 `NewOSSStorage` 的静态/动态分支、首次刷新、`SendCredentials` 快照和 provider 名传播；测试放在独立的 `credential_test.rs` 或 `store_test.rs`，不要内嵌进生产文件。
- 改变 endpoint 或地域策略，应集中修改 `endpoint_for_region`、`metadata_region`、`can_use_internal_endpoint`、`build_api` 和 `NewOSSStorage` 的探测顺序，并增加自定义 endpoint、空地域、地域不匹配、ECS 同区/异区、预签名公网的独立测试。需特别评估流量费用、签名地域和 VPC 外消费兼容性。
- 增加新的构造校验时可落在 `prepare_backend`；必须保持“构造副本”与“按策略修改调用方 backend”边界，避免 Prefix 规范化等副作用泄漏。
- 调整凭证转发时维持整快照写入和失败前不改旧值，且明确临时凭证有效期由下游调用方负责。敏感字段不得写日志。
- 增加权限类型应先扩展 `storeapi::Permission` 和 `Client` 的检查实现，再让 `s3like::CheckPermissions` 调度；不要在本文件复制对象操作逻辑。
- 改变关闭或后台刷新时必须同步 `OSSStore::Close` 与 `CredentialRefresher::{start_refresh,close,Drop}`，验证重复关闭、启动失败、刷新失败保留旧快照和线程 join。
- 若要把该 crate 接入 Rust 统一对象存储入口，应在集成层提供 `pkg/objstore/storage.rs::Options::external_factory`，并新增覆盖 backend 分发的独立测试；不能只因 crate 已在 workspace 注册就认为接线完成。
- 性能风险主要来自构造期串行网络调用、阻塞元数据请求和权限预检；兼容风险集中在 endpoint 优先级、签名地域、Prefix/URI 格式及 Go/Rust 凭证链差异。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/objstore/ossstore` 确认目标源、Go 对照、模块入口和测试均已索引。
- RustCodeGraph `node --file pkg/objstore/ossstore/store.rs --offset 1 --limit 500`：读取目标文件全部 294 行并核对 23 个符号；文件级结果报告 `used by pkg/importsdk/file_scanner.rs`。
- RustCodeGraph `query NewOSSStorage --kind function`、`query build_api --kind function`、`query set_backend_credentials --kind function`：消歧到本文件的生产入口和两个关键辅助函数。对精确符号运行 `callers/callees` 未返回边，因此调用关系以源码、模块导出和文本引用补证，并在正文标明限制。
- 已读生产与配置路径：`pkg/objstore/ossstore/store.rs`、`pkg/objstore/ossstore/Cargo.toml`、`pkg/objstore/ossstore/lib.rs`、`pkg/objstore/ossstore/credential.rs`、`pkg/objstore/ossstore/client.rs`、`pkg/objstore/storage.rs`、根 `Cargo.toml`。
- 已读 Go 对照与测试：`pkg/objstore/ossstore/store.go`、`pkg/objstore/ossstore/store_test.go`。已读 Rust 独立测试：`pkg/objstore/ossstore/store_test.rs`、`pkg/objstore/ossstore/migration_aster_unit_test.rs` 的地域/endpoint 断言区段。
- `rg` 直接引用检查确认 Rust 生产源码没有 `NewOSSStorage` 调用点，相关公开辅助符号主要由本 crate 的独立测试覆盖；Go 的 `pkg/objstore/storage.go` 存在明确工厂调用。
- Rust 测试证据：`uri_preserves_oss_scheme_and_normalizes_prefix`；`test_store` 的读写、范围读、删除、multipart、复制和 WalkDir；`test_internal_endpoint`；`test_can_use_internal_endpoint`；`test_set_backend_credentials`。Go 测试补充真实服务的地域、权限、取消和 AccessRecording 语义，但真实凭证测试默认跳过。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认文件存在且恰有 11 个固定二级标题，并人工复核所有“已支持”表述均能追溯到上述符号或测试。
