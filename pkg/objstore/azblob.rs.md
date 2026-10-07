# `pkg/objstore/azblob.rs` 逻辑说明

## 文件定位

`pkg/objstore/azblob.rs` 是 `astersql-objstore` crate 的 Azure Blob 后端实现，模块由 `pkg/objstore/lib.rs` 以 `pub mod azblob` 暴露。它把异步的 `object_store::ObjectStore` 包装成 `objectio`/`storeapi` 所定义的同步、可取消接口，并在同一文件中提供认证选择、Azure 配置、范围读取、分块上传以及一个测试用内存后端。

crate 边界见 `pkg/objstore/Cargo.toml`：本文件直接依赖 `object_store` 的 `azure` feature、`tokio` 多线程运行时、`futures`、`bytes`、`url`、`serde`、`sha2`、`base64` 和 `anyhow`，接口来自工作区 crate `astersql-objstore-objectio` 与 `astersql-objstore-storeapi`。目录中没有 `doc.go`；最近的模块契约入口是 `pkg/objstore/lib.rs`。

需要特别区分“模块已公开”和“统一工厂已接线”：`pkg/objstore/parse.rs::ParseBackend` 能把 `azure://`/`azblob://` 解析为配置，但 `pkg/objstore/storage.rs::New` 对云后端仍委托调用方提供的 `external_factory`。仓库搜索只发现 `new_azure_blob_storage` 被本文件测试调用，未发现它已由统一 Rust 工厂直接调用。因此，本文件提供真实 Azure 构造能力，但不能据此推断所有 Rust `StorageBackend::AzureBlobStorage` 都会自动落到这里。

## 核心职责

1. `AzblobBackendOptions::apply` 将命令行形状的配置写入 `AzureBlobStorageConfig`，并把客户提供的明文密钥转换为 Azure 所需的 Base64 密钥及 Base64 SHA-256 摘要。
2. `select_azure_client` 按确定的优先级选择 SAS、显式 Shared Key、环境 Client Secret、环境 Shared Key、默认凭据或匿名模式；`new_azure_blob_storage` 再据此配置 `MicrosoftAzureBuilder`。
3. `AzureBlobStorage` 实现 `storeapi::Storage` 与 `storeapi::StrongConsistency`，提供整对象读写、存在性、删除、范围打开、遍历、URI、分块创建、重命名和不支持预签名 URL 的明确错误。
4. `ObjectStorageCore` 用专用 Tokio 运行时桥接同步 API 和异步 `object_store`，也被同目录的 GCS、本地等实现复用。
5. `ObjectStoreReader` 实现惰性范围 GET、`Read`、`Seek`、关闭和文件大小查询；`AzureBlockUploader` 实现有界并发的 multipart stage/commit 生命周期。
6. `MemoryStorage` 和 `ObjectStoreWriter` 提供进程内对象存储与缓冲写入，主要服务独立测试和轻量演示，不应与仓库其他 crate 中同名的 `MemoryStorage` 混淆。

## 主要符号

- 常量：`AZBLOB_RETRY_TIMES` 为 5，`AZBLOB_CHUNK_SIZE` 为 64 MiB；七个 `AZBLOB_*_OPTION` 常量定义配置键。当前 `AZBLOB_RETRY_TIMES` 在生产函数中没有被读取，只被测试用作重试次数下界。
- 配置类型：`AzureCustomerKey` 保存编码后的密钥和摘要；`AzureBlobStorageConfig` 保存 endpoint、container（字段名为 `bucket`）、prefix、storage class、账户、Shared Key、SAS、加密作用域与客户密钥；`AzblobBackendOptions` 是可应用到该配置的输入形状。
- 认证类型：`StorageOptions` 控制环境凭据是否回写及是否匿名；`AzureAuth` 表示五种认证选择；`AzureClientSelection` 返回最终认证类别、账户名和服务 URL。
- 公共辅助入口：`url_of_object_by_endpoint` 规范化 endpoint/container/object URL；`progress` 解析 `finished/total`；`select_azure_client` 选择认证；`new_azure_blob_storage` 创建真实 Azure 后端。
- `ObjectStorageCore`（`pub(crate)`）：持有 `Arc<dyn ObjectStore>` 与 `Arc<Runtime>`，提供 `put`、可取消 `put_with_context`、`get`、`head`、`delete`、`list`、`rename`、`copy`。
- `AzureBlobStorage`（公开）：`with_store` 支持注入任意 `ObjectStore`；`options`、`resolved_account_name`、`resolved_service_endpoint` 暴露只读信息；`copy_from` 在相同底层 store 时原生复制，否则整对象读回再写。
- `ObjectStoreReader`（`pub(crate)`）：维护 `path`、当前位置 `pos`、范围尾 `end`、总长 `total`、越界策略和可丢弃的本地 `Cursor`。
- `AzureBlockUploader`（私有）：持有 multipart handle、创建时父 context、派生 group context、并发上限、`JoinSet` 和首个/最近 stage 错误。
- `ObjectStoreWriter`（`pub(crate)`）：在内存累积数据，首次成功 `close` 时一次性 put，重复关闭幂等。
- `MemoryStorage`（公开）：基于 `object_store::memory::InMemory` 实现完整 `storeapi::Storage`。
- 私有/包内辅助：`default_service_url`、`validate_encryption_options`、`join_object_path`、`trim_storage_prefix`、`storage_class_attributes`、`is_not_found`、`lock_unpoisoned`。

## 执行流程

配置与构造主链如下：

1. 调用方先形成 `AzureBlobStorageConfig`；若使用 `AzblobBackendOptions::apply`，空的 `encryption_key` 会回退到 `AZURE_ENCRYPTION_KEY`，非空密钥被编码并计算摘要。
2. `new_azure_blob_storage` 调用 `select_azure_client`。它先拒绝空 container，然后依次检查：显式账户名+SAS、显式账户名+Shared Key、环境账户名、完整的 Client Secret 三元组、环境 Shared Key，最后选择匿名或默认凭据。只有环境 Client Secret/Shared Key 分支会在 `send_credentials` 为真时回写配置。
3. `validate_encryption_options` 拒绝“access tier 与 encryption scope/customer key 同时存在”，也拒绝 scope 与 customer key 同时存在。
4. `MicrosoftAzureBuilder::from_env` 设置账户与 container；显式 endpoint 会设置 endpoint，并仅对 `http://` 开启明文 HTTP。认证分支分别设置 SAS 参数、access key、Client Secret、跳过签名或保留 builder 默认行为；客户密钥通过 `with_encryption_key` 下传。
5. builder 成功后产生共享 `ObjectStore`，`AzureBlobStorage::with_store` 再创建双工作线程 Tokio runtime，并保存解析后的账户名和服务 endpoint。

对象操作统一先通过 `object_name`/`object_path` 拼入配置 prefix。`WriteFile` 在调用前检查 context，然后用 `tokio::select!` 在 PUT 与取消信号之间竞争；`ReadFile`、`FileExists`、`DeleteFile`、`Rename` 等在入口检查 context 后调用核心操作。`WalkDir` 组合 prefix、`SubDir`、`ObjPrefix`，列举并按 location 排序，去掉存储 prefix 后回调；它有意不应用 `WalkOption::StartAfter`，与 Go Azure 实现保持一致。

`Open` 先 `head` 获得总长度，再按 `ReaderOption` 构建半开区间 `[start,end)`。`ObjectStoreReader` 首次读取或 seek 改变位置后才调用 `get_range`；读取量受配置的 `end` 限制。`SeekFrom::End` 以完整对象总长为基准，而不是范围尾；若 seek 到范围尾之外、但仍在对象内，后续读取仍受原范围尾限制。

`Create` 根据 `WriterOption::PartSize` 选择块大小（非正数回退 64 MiB），把 `Concurrency` 钳制为至少 1，再由 `objectio::new_uploader_writer` 缓冲分块。`AzureBlockUploader::write` 惰性初始化 multipart upload；同步模式直接等待 `put_part`，并发模式先在任务数达到上限时 join 一个，再把拥有独立字节副本的 part future 放入 `JoinSet`。`close` 等待所有 stage，若任一失败则不 commit；否则检查关闭 context 并调用 `complete`。

## 数据与状态

`AzureBlobStorage` 是可克隆的轻量句柄：配置字符串和解析结果按值保存，`ObjectStorageCore` 内部的 store/runtime 通过 `Arc` 共享。对象名不保留 `.`、空段或已被 `..` 抵消的段，因为 `join_object_path` 会做词法规范化；它不是文件系统解析，不访问磁盘，也不会检查 `..` 是否越过逻辑根目录。

`ObjectStoreReader` 的关键不变量是 `0 <= pos` 且构造时 `end >= pos`；实际 `end` 被截断到 `total`。缓存 `reader` 只对应当前 `pos..end` 下载结果，任何有效位置变化都会丢弃缓存。`close` 只清缓存，不销毁共享 store/runtime。

`AzureBlockUploader` 的 `upload` 在首次写或空文件关闭时初始化。`tasks.len()` 受 `concurrency` 限制，part 注册顺序由 `MultipartUpload::put_part` 的调用顺序确定；请求可以乱序完成，但 complete 使用该注册顺序。`error` 用 `Arc<Mutex<Option<Arc<io::Error>>>>` 跨任务保存：并发模式保留首个失败，同步模式用最近一次失败覆盖，后续 `close` 返回所存错误且不提交部分对象。

`MemoryStorage` 的删除使用 `ignore_missing=true`，而 Azure 删除缺失对象会传播错误；其 `WalkDir` 会应用 `StartAfter`，Azure 实现不会。这些是有意的后端语义差异。

## 依赖与调用关系

上游接口边来自 `storeapi::Storage`、`storeapi::StrongConsistency`、`objectio::Reader` 和 `objectio::Writer`。`pkg/objstore/lib.rs` 公开本模块并在 `#[cfg(test)]` 下挂载 `azblob_test.rs` 与 `azblob_1_aster_unit_test.rs`。`pkg/objstore/parse.rs` 能产生 Azure 配置形状，但它定义的是解析层类型；真正创建本文件 `AzureBlobStorage` 仍需直接调用 `new_azure_blob_storage`，或由集成层 `external_factory` 显式桥接。

下游主要是 `object_store::azure::MicrosoftAzureBuilder`、`ObjectStore` 的 put/get/head/delete/list/copy/rename/get_range/multipart API，以及 Tokio runtime、`JoinSet` 和 `select!`。`bytes::Bytes` 保证提交给异步 part 的数据拥有独立生命周期；`futures::TryStreamExt` 将 list stream 收集成元数据数组。

RustCodeGraph 的文件级索引显示 `ObjectStorageCore`/`ObjectStoreReader` 等共享符号还被 `pkg/objstore/gcs.rs`、`pkg/objstore/local.rs` 等文件使用。因此修改这些包内类型不是 Azure 局部改动，必须检查其他对象存储后端。精确查询确认 `new_azure_blob_storage`、`select_azure_client`、`AzureBlockUploader` 和 `ObjectStoreReader` 均定义于本文件；命令行 callers/callees 对这些 Rust trait/impl 边没有给出可用结果，相关调用边以文件源码、模块使用和独立测试交叉核实。

## 错误处理与边界

- 配置阶段明确拒绝空 container、缺失账户名、冲突的加密/tier 组合、同时设置 encryption scope 与 customer key，以及非法 endpoint URL。
- `select_azure_client` 只在 Client Secret 三个环境变量全部非空时选该模式；若仅有一部分，会继续回退。显式 `shared_key` 没有显式 `account_name` 时不会单独触发 Shared Key 分支。
- `new_azure_blob_storage` 用上下文补充 builder 构造错误；`WriteFile`/`ReadFile` 为对象名添加操作上下文。`FileExists` 只把错误链中精确的 `object_store::Error::NotFound` 转成 `false`，其他错误保留。
- `ObjectStoreReader::new` 拒绝负 start 和 `end < start`；seek 拒绝负位置、整数溢出、正的 `SeekFrom::End` 偏移，以及默认策略下越过总文件长度。读到范围尾返回 `Ok(0)`。
- `DeleteFiles` 和遍历回调均为顺序、遇错即停；没有回滚已经完成的操作。
- `PresignFile` 明确返回不支持；`Close` 与 `MarkStrongConsistency` 当前为空操作。
- multipart stage 失败时不调用 `complete`，依赖 Azure 清理未提交块。空 writer 的 `close` 会初始化并提交一个 multipart upload；是否形成零字节对象由底层 `object_store` 实现决定。
- 当前 Rust 实现仅将 customer encryption key 传入 builder；`encryption_key_sha256` 未被读取，`encryption_scope` 除互斥校验外也未下传。扩展前不能把“配置字段存在”等同于“请求已携带对应 Azure 加密头”。
- `AZBLOB_RETRY_TIMES` 没有配置到 `MicrosoftAzureBuilder`；真实重试行为由 `object_store` builder/客户端默认配置决定。`progress` 当前也不在复制链上。

## 并发与资源生命周期

每个 `ObjectStorageCore::new` 创建一个两工作线程 Tokio runtime，并由克隆出的存储、reader、writer 共享；`AzureBlobStorage::Close` 不主动关闭它，最后一个 `Arc` 释放时 runtime 才随 core 析构。同步方法通过 `Runtime::block_on` 进入异步客户端，因此调用方线程会阻塞到操作完成或所实现的取消分支胜出。

整对象 PUT 与 multipart stage/complete 使用 `objectio::Context` 取消。并发 uploader 捕获 `Create` 时的父 context，用内部 group context 在任一 stage 失败后取消同组任务；同步 uploader 则使用每次 `write` 传入的 context。这一差异与 Go 的 `errgroup.WithContext` 路径一致，并由 `azure_upload_uses_creation_context_only_for_concurrent_stages` 验证。

并发上传在每次新增任务前按 `concurrency` 做背压，关闭时等待全部任务，确保 commit 不早于任何 stage 完成。成功等待后会取消派生 group context，以匹配 Go `errgroup.Wait` 的生命周期；因此已关闭 writer 后再次形成完整块会观察到中断。父 context 在 stage 进行中被取消时，任务记录错误、取消 group，`close` 返回错误且不 commit。

`ObjectStoreReader` 每次 reopen 把请求范围完整下载为本地 `Vec<u8>`，不是持续持有远端响应流；这简化了 seek 和关闭，但范围较大时会占用等量内存。跨不同底层 store 的 `copy_from` 同样把整个源对象读入内存，存在大对象峰值内存风险。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/objstore/azblob.go`，行为测试为 `pkg/objstore/azblob_test.go`。Rust 保留了 Go 的配置键、64 MiB 默认分块、认证优先级大框架、prefix 拼接、Azure `WalkDir` 不使用 `StartAfter`、半开读取范围、seek 到文件尾成功、access tier 与客户加密互斥、同步/并发上传分支以及 stage 失败不 commit 的契约。

主要实现差异如下：

- Go 使用 Azure SDK 的 container/block blob client；Rust 使用通用 `object_store` Azure 后端，并用专用 Tokio runtime 转成同步接口。
- Go 构造时会请求 container properties 验证可访问性；Rust builder 构造成功即返回，不做等价探测。
- Go 的 `ReadFile`/reader 显式创建 `RetryReader(MaxRetries=5)`；Rust 常量虽保留，但没有显式写入 builder，重试由 `object_store` 默认策略决定。
- Go `CopyFrom` 只接受 Azure 源，通过 Azure 异步 server-side copy、轮询 copy ID/status 并解析 `progress`；Rust 同一 `ObjectStore` 指针时调用通用 `copy`，不同指针时整对象下载再上传，不轮询 Azure copy 状态。因此 Rust `progress` 目前不是生产复制路径的一部分。
- Go `Rename` 明确执行 ReadFile→WriteFile→DeleteFile；Rust调用 `ObjectStore::rename`，具体实现通常是 copy+delete，但错误原子性和内存行为由后端决定。
- Go 下传 encryption scope、客户密钥及摘要；Rust 当前只向 builder 下传 customer key 字段，scope 和摘要没有对应接线。
- Go 支持调用方自定义 HTTP transport，并在 Client Secret 构造失败时告警后回退 Shared Key；本文件 `StorageOptions` 不含 HTTP client，且 builder 的 Client Secret 配置错误会在最终 build 时直接返回。
- Go 为每个块生成 UUID/Base64 block ID；Rust依赖 `object_store::MultipartUpload` 生成和按调用顺序管理 block ID。

这些差异是当前源码事实，不应把 Go 的完整能力描述成 Rust 已支持能力。

## 扩展指南

- 新增认证方式或调整优先级：修改 `AzureAuth`、`select_azure_client` 和 `new_azure_blob_storage` 的 builder 分支；同步扩展 `pkg/objstore/azblob_test.rs::test_new_azblob_storage`，并用串行测试隔离环境变量。
- 完整支持 encryption scope/customer key：首先确认 `object_store` 版本提供的 Azure builder/请求属性能力，再修改构造或每次请求属性；同时覆盖互斥校验、请求头和读取加密对象。不能只增加配置字段。
- 调整上传并发或错误策略：集中修改 `AzureBlockUploader::{write,close,join_one,remember_error}`，必须保持 part 顺序、并发上限、首错/末错语义、失败不 commit 和取消传播；对应测试是 `azure_writer_options_stage_bounded_concurrent_blocks`、`azure_stage_failure_never_commits_blocks`、`azure_synchronous_write_failure_is_retained_by_close`、`azure_commit_retains_tier_and_wait_cancels_derived_context`、`azure_parent_cancellation_interrupts_inflight_staging_without_commit`。
- 调整范围读取/seek：修改 `ObjectStoreReader::{new,reopen,read,seek}`，同步验证 `test_azblob`、`test_azblob_seek_to_end_should_not_error` 和 `azure_and_gcs_object_operations_ranges_and_walk_match_go`；尤其保持半开 end 和以完整文件为基准的 `SeekFrom::End`。
- 将 Azure 接入统一 Rust 工厂：在负责 `external_factory` 的集成层把 `parse.rs` 的 Azure 配置转换为本文件 `AzureBlobStorageConfig`，并明确映射 `send_credentials`/匿名选项；不要在两个同名配置类型之间依赖隐式等价。
- 修改 `ObjectStorageCore`、`ObjectStoreReader`、路径或属性辅助函数前，检查 `pkg/objstore/gcs.rs`、`pkg/objstore/local.rs` 及其测试，因为这些是跨后端共享实现。
- 性能风险集中在每个 core 独立 runtime、大范围读取整段缓冲、跨 store copy 整对象缓冲、`WalkDir` 全量收集后排序，以及批量删除串行执行。优化时需保留确定顺序和错误/取消契约。

## 验证依据

- RustCodeGraph：运行 `rustcodegraph status`，索引覆盖 11,467 个文件，其中 `pkg/objstore/azblob.rs` 有 1,118 行、116 个符号；运行 `files --filter pkg/objstore`；用 `node --file pkg/objstore/azblob.rs --offset 1 --limit 500` 与 `--offset 500 --limit 700` 阅读全文件；用 `query` 精确定位 `new_azure_blob_storage`、`select_azure_client`、`AzureBlobStorage`、`AzureBlockUploader`、`ObjectStoreReader`，并执行相应 `callers`/`callees` 查询。图命令未返回这些 Rust trait/impl 的有效边，因此又以模块源码和测试交叉核对。
- crate/模块：读取 `pkg/objstore/Cargo.toml`、`pkg/objstore/lib.rs`、`pkg/objstore/storage.rs` 和 `pkg/objstore/parse.rs`；确认依赖、模块导出、云工厂边界和 Azure URI 解析路径。目标目录不存在 `doc.go`。
- Go 对照：通过 RustCodeGraph 阅读 `pkg/objstore/azblob.go` 的选项/认证、构造、复制、CRUD、reader、uploader 全部相关段落；符号清单和行为与 `pkg/objstore/azblob_test.go` 对照。
- Rust 独立测试：通过 RustCodeGraph 阅读 `pkg/objstore/azblob_test.rs` 全文件，以及 `pkg/objstore/azblob_1_aster_unit_test.rs` 中 Azure 配置、URL、认证、范围读取与遍历测试。证据覆盖基本 CRUD、确定遍历、认证优先级、截断响应重试、seek 到尾、大对象复制、有界并发、数据所有权、stage 失败、取消和 access tier commit 头。
- 仓库搜索：确认 `new_azure_blob_storage` 除目标测试外没有 Rust 生产调用；确认共享核心符号被相邻后端复用。该结论只覆盖当前检出的仓库源码，不代表外部 crate 使用情况。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务规定的 11 章节结构命令，并人工复核本文没有把未接线或 Go 独有能力写成 Rust 现状。
