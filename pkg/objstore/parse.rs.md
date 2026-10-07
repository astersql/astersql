# `pkg/objstore/parse.rs`

## 文件定位

`parse.rs` 是 `astersql-objstore` crate 的存储地址解析边界，由 `pkg/objstore/lib.rs` 以 `pub mod parse` 对外暴露。它把用户或上层模块提供的字符串 URI、已解析 URL 以及额外后端选项，转换为本 crate 使用的统一 `StorageBackend`；反向还可把后端格式化成不含认证 query 的规范 URI。

该文件不创建网络客户端，也不执行对象读写。真正的实例化在 `pkg/objstore/storage.rs` 的 `New`/`NewFromURL` 中完成；本文件负责在进入该层以前确定后端种类、bucket、prefix、endpoint、认证信息等配置。生产调用证据包括 `pkg/objstore/helper.rs::ValidateCloudStorageURI`、`pkg/executor/importer/precheck.rs::validate_global_sort_uri`、`pkg/planner/extstore/extstore.rs::NewExtStorage`、`pkg/session/runtime/load_data.rs` 和 `pkg/importsdk/file_scanner.rs`。

`pkg/objstore/Cargo.toml` 声明 crate 名为 `astersql-objstore`，入口是同目录 `lib.rs`。本文件直接使用其中声明的 `anyhow`、`base64`、`sha2`、`url` 和本地 `parser-ast` 依赖；标准库承担路径处理、环境变量与凭证文件读取。

## 核心职责

1. `ParseRawURL` 将原始输入拆成 `ParsedURL { scheme, host, path, query, original }`，并在解析前保护 query 值中的 `+`，避免 form decoding 把凭证中的加号变为空格。
2. `ParseBackend`/`ParseBackendFromURL` 经内部 `parseBackend` 按 scheme 分派，生成 `Local`、`Hdfs`、`Noop`、`MemStore`、`S3`、`Gcs` 或 `AzureBlobStorage`。
3. `ExtractQueryParameters` 把 query 规范化后叠加到克隆的后端 options 上，再清空 `ParsedURL.query`；因此 query 优先于调用者传入的同名选项，同时不会回写调用者的 `BackendOptions`。
4. 各 options 的应用逻辑校验或派生配置：S3 校验 endpoint 与 AK/SK，GCS 可读取凭证文件，Azure 可从字段或 `AZURE_ENCRYPTION_KEY` 读取客户密钥并计算摘要。
5. `FormatBackendURL` 生成不携带 endpoint、访问密钥、token 等选项的后端地址；`IsLocalPath`、`IsLocal`、`IsS3Like` 提供轻量分类。

## 主要符号

- 常量 `OSSProvider`、`KS3SDKProvider`：分别是 `oss-sdk`、`ks3-sdk`，用于在统一 S3 结构中保留兼容后端身份，并影响 `FormatBackendURL` 的输出 scheme。
- `BackendOptions`：聚合 `S3BackendOptions`、`GCSBackendOptions` 和 `AzblobBackendOptions`。解析时只克隆当前后端对应成员。
- 数据模型 `Local`、`Hdfs`、`S3`、`Gcs`、`AzureCustomerKey`、`AzureBlobStorage`：保存解析结果。`StorageBackend` 是这些结果的统一枚举；`StorageBackend::kind` 返回供日志或分支使用的稳定短名。
- `ParsedURL`：保存解析后的 scheme、含端口的 host、已解码 path、私有 query 和原串。`ParsedURL::String` 在 `ParseBackendFromURL` 未收到 raw 字符串时重建地址，并保留显式空 authority，确保 `s3:///path` 的错误上下文不丢失。
- `ParseRawURL(raw_url) -> Result<ParsedURL>`：无冒号输入直接视为本地路径；其他输入由 `url::Url` 解析。辅助函数 `decode_url_path`、`hex_value` 令 path 视图靠近 Go `net/url.URL.Path`。
- `ParseBackend` 与 `ParseBackendFromURL`：公开入口。前者拒绝空串并自行解析；后者允许上游先修改 `ParsedURL.path`，例如 `NewExtStorage` 追加 namespace 后再构造后端。
- `parseBackend`：内部 scheme 分派核心；`absolute_clean_path` 与 `require_bucket` 分别处理无 scheme 本地路径和云 bucket 前置条件。
- `QueryParameterOptions`：用显式 trait 取代 Go 版本的反射赋值；三个 options 类型各自实现允许的 query key。未知 key 被忽略，非法布尔值也保持原值。
- `S3BackendOptions::Apply`/`SetForcePathStyle`：前者校验并复制配置，后者根据显式 query、provider、AWS endpoint、role ARN 或 accelerate 选项调整 path-style。
- `GCSBackendOptions::apply`、`AzblobBackendOptions::apply`：分别处理凭证文件，以及 Azure 客户密钥的环境变量回退、Base64 和 SHA-256 派生。
- `FormatBackendURL`/`format_backend_url`：按后端反向构造 URL，并通过 `url::Url::set_path` 编码空格等 path 字符。

## 执行流程

典型 `ParseBackend(raw, options)` 流程如下：

1. 空字符串立即返回 `empty store is not allowed`。
2. `ParseRawURL` 先把全部 `+` 替换为 `%2B`。若输入不含冒号，则保留原串为本地 path；否则解析 scheme、authority（含显式端口）、解码后的 path 和 query pairs。
3. `parseBackend` 选择有效 raw 字符串。`ParseBackendFromURL` 传入空 raw 时调用 `ParsedURL::String` 重建它，主要用于 HDFS remote 和错误消息。
4. 按 scheme 分支：
   - 空 scheme：将相对路径拼到当前工作目录，再词法清理 `.`/`..`，不解析符号链接，也不要求路径存在。
   - `local`/`file`：直接采用 URL path；`hdfs` 保留完整 remote；`noop`、`memstore` 生成无配置枚举分支。
   - `s3`/`ks3`/`oss`：要求 host 非空，以 host 为 bucket、去掉首尾斜杠后的 path 为 prefix。默认无 options 时先令 `force_path_style=true`，再叠加 query、调用 `SetForcePathStyle`、校验并应用；最后为 KS3/OSS 强制设置 provider 常量。
   - `gs`/`gcs`：要求 bucket，叠加 query；若配置了 `credentials-file`，同步读取整个文本作为 `credentials_blob`。
   - `azure`/`azblob`：要求 bucket，叠加 query；应用账号、shared key、SAS、access tier、加密 scope 与客户密钥。
   - 其他 scheme：返回 `storage <scheme> not support yet`。
5. query 通过 `ExtractQueryParameters` 后从局部 `ParsedURL` 清除；返回值携带已合并的配置，原始 `BackendOptions` 因先被克隆而保持不变。

反向流程 `FormatBackendURL` 只选取后端身份、bucket 和 prefix：S3 根据 provider 恢复成 `s3`/`oss`/`ks3`，GCS 与 Azure 使用规范 scheme，本地、noop、memstore、HDFS 使用各自约定。认证参数不会进入返回字符串。

## 数据与状态

本文件没有全局可变状态。解析的主要状态是函数栈上的 owned `String`、`Vec<(String, String)>` 和后端枚举；调用者给出的 `&BackendOptions` 只读，当前后端 options 会被克隆后再让 URL query 覆盖。`ExtractQueryParameters` 唯一显式修改调用者对象的入口是其 `&mut ParsedURL` 和 `&mut T` 参数：它填充局部 options 并清空 URL query。

敏感配置可存在于 `S3.access_key`、`S3.secret_access_key`、`S3.session_token`、Azure shared key/SAS/customer key 以及 GCS `credentials_blob`。`FormatBackendURL` 刻意不序列化这些字段；bucket 缺失错误通过 `parser_ast::misc::redact_url` 脱敏。`ParsedURL.original` 用于区分显式空 authority 与无 authority 的形式，但自身并不公开。

Azure 客户密钥的优先级是 `AzblobBackendOptions.encryption_key` 高于环境变量 `AZURE_ENCRYPTION_KEY`。非空密钥会被 Base64 编码，同时对原始密钥字节计算 SHA-256 后再 Base64 编码；两者一起存入 `AzureCustomerKey`。GCS 凭证文件在解析阶段被完整读入内存。

## 依赖与调用关系

上游调用关系（由 RustCodeGraph 的文件使用者信息与 `rg` 精确补证）：

- `pkg/objstore/storage.rs::NewFromURL`：`ParseBackend` 后将枚举交给 `New` 创建存储；`memstore://` 另有提前返回捷径。
- `pkg/objstore/helper.rs::ValidateCloudStorageURI`：解析后以权限检查选项打开云存储。
- `pkg/executor/importer/precheck.rs::validate_global_sort_uri`：先 `ParseRawURL`，再 `ParseBackendFromURL`，并限定结果必须是 S3/GCS/Azure。
- `pkg/planner/extstore/extstore.rs::NewExtStorage`：解析 URL、修改 path 追加 namespace，再从修改后的 URL 构造后端。
- `pkg/executor/importer/import.rs`、`pkg/util/sem/v2/sql_rule.rs`：组合 `ParseRawURL` 与 `IsLocal` 判断是否允许或需要分布式处理。
- `pkg/session/runtime/load_data.rs`、`pkg/importsdk/file_scanner.rs`、`pkg/dumpformat/testutils/parquet_writer.rs`：直接解析来源路径供后续存储打开或扫描。

主要下游依赖：

- `url::Url` 负责非本地路径的 URI 语法、query 解码和格式化编码。
- `parser_ast::misc::redact_url` 负责错误消息中的云认证参数脱敏。
- `std::env::current_dir` 与 `std::path` 实现本地绝对路径和词法清理；`std::fs::read_to_string` 读取 GCS 凭证。
- `sha2::Sha256` 与 `base64` 派生 Azure customer-provided key 所需字段。
- `anyhow::Result` 统一传播 URL、当前目录、文件读取和配置校验错误。

RustCodeGraph 对 `pkg/objstore/parse.rs` 报告 53 个符号，并指出 6 个直接使用文件；精确 `callers/callees --file` 查询没有返回边，因此上述跨文件调用使用源码搜索补齐，未将模糊的同名 `ParseBackend` 结果当作证据。

## 错误处理与边界

- `ParseBackend` 明确拒绝空输入；URL 语法错误由 `url::Url::parse` 透传。无冒号字符串例外地作为本地路径处理。
- S3/KS3/OSS、GCS 和 Azure/AzBlob 必须有非空 host。错误消息包含经脱敏的有效原 URL；`pkg/objstore/parse_test.rs::missing_bucket_errors_redact_cloud_credentials` 同时验证直接入口与 `ParseBackendFromURL` 不泄露三类云密钥。
- S3 endpoint 必须含冒号、`://` 和 host；末尾 `/` 被移除。没有 profile 时，AK 与 SK 必须同时为空或同时非空；有 profile 时允许部分显式凭证覆盖。
- `parse_go_bool` 仅接受 Go `strconv.ParseBool` 支持的大小写集合；非法值不报错，而是忽略该 query 并保留 options 当前值。未知 query key 同样静默忽略。
- GCS 凭证文件不可读或不是 UTF-8 时解析失败。Azure 环境变量缺失或不可读被视为空值，不报错。
- `decode_url_path` 对合法 `%XX` 解码；非法或不完整转义按原字节保留，非 UTF-8 结果使用替换字符。这是当前实现边界，扩展时不能假定 path 始终可无损往返任意字节。
- `format_backend_url` 对非空 path 使用 `expect`，其不变量是内部固定 scheme 与解析所得/配置所得 host 能形成 URL；若未来允许不可构成 URL 的 host，此处会 panic。
- `IsS3Like` 当前只认可 `s3` 和 `oss`，不包含虽走同一解析分支的 `ks3`；调用方若需要“所有 S3 兼容 scheme”，必须先确认是否应改变该兼容契约。

## 并发与资源生命周期

解析本身是同步、无锁、无异步任务的纯局部流程，可由多个线程并发调用；没有缓存、channel、事务或后台任务。`BackendOptions` 通过共享引用传入且被克隆，因此并发调用不会互相覆盖配置。

资源生命周期仅有两个同步边界：GCS `credentials-file` 在 `apply` 调用期间打开、读完并关闭，内容随后由返回的 `Gcs.credentials_blob` 持有；Azure 环境变量在每次 `apply` 时读取一次。二者都发生在返回 `StorageBackend` 之前，失败不会返回部分后端。实际网络连接和存储句柄生命周期属于 `pkg/objstore/storage.rs` 及具体 provider，不由本文件管理。

## 与 Go 版本的对应关系

主要语义以同路径 `pkg/objstore/parse.go` 及 `pkg/objstore/parse_test.go` 为基准：`+` 保护、scheme 分派、bucket/prefix 提取、query 优先级、query 清除、S3 provider、路径判断和脱敏格式化均保持相同意图。Rust 独立测试 `pkg/objstore/parse_test.rs` 对应覆盖 Go 的创建、格式化、原始 URL、本地判断、profile/凭证和默认 path-style 用例。

实现形态存在以下明确差异：

- Go 返回 protobuf `backuppb.StorageBackend`；Rust 在本 crate 内定义等价数据结构与 `StorageBackend` 枚举，后续 provider 层消费该枚举。
- Go `ExtractQueryParameters` 通过 JSON tag 和反射支持 string/bool，并对新增未支持字段类型 panic；Rust 使用 `QueryParameterOptions` 的显式 match，新增字段必须主动加入对应实现，未知字段不会 panic。
- Go 的 GCS/Azure options 与 `apply` 位于 `gcs.go`、`azblob.go`；Rust 为形成独立 crate API，将它们和 S3 options 放在本文件。
- Rust `parseBackend` 明确支持 `memstore`，`FormatBackendURL` 也处理 `MemStore` 与 `Hdfs`；当前 Go 同路径 switch 没有这些对应格式化分支。此差异是现状，不应据此推断 Go 调用方拥有同样入口。
- Rust 为贴近 Go 额外实现了含端口 authority 保留、`filepath.Abs` 风格的词法清理、显式空 authority 重建以及 endpoint 必须含 `://` 的检查；对应边界由 Rust 测试直接固定。
- 错误类型不同：Go 主要以 `ErrStorageInvalidConfig` 注解错误，Rust 使用 `anyhow` 文本错误并在上层按需转换。因此调用方不应依赖 Rust 侧的结构化错误身份。

## 扩展指南

- 新增 scheme：在 `StorageBackend` 增加数据分支（如需要），同步更新 `kind`、`parseBackend`、`FormatBackendURL`，并检查 `pkg/objstore/storage.rs::New` 的实例化分派。还要审查云 URI 白名单，例如 `pkg/executor/importer/precheck.rs::validate_global_sort_uri`。
- 新增 query 选项：先在对应 options 与结果结构加入字段，再在该类型的 `QueryParameterOptions::set_query_parameter` 和 `apply`/`Apply` 中接线；布尔项应继续使用 `parse_go_bool`。必须验证 URL query 覆盖克隆值但不修改原 `BackendOptions`。
- 改动 URI 往返：同时检查 `ParsedURL::String`、`decode_url_path` 与 `format_backend_url`，保留空 authority、端口、空格编码、prefix 首尾斜杠和不暴露认证参数等契约。
- 改动敏感字段：更新 `parser_ast::misc::redact_url` 的识别范围，并扩充 `missing_bucket_errors_redact_cloud_credentials`；绝不能让 `FormatBackendURL` 或错误文本携带明文。
- 改动 S3 寻址规则：集中修改 `S3BackendOptions::SetForcePathStyle`，覆盖显式 true/false、AWS endpoint、role ARN、accelerate 与 provider 分支。
- 测试必须继续放在独立的 `pkg/objstore/parse_test.rs`，不要内嵌进生产源文件；涉及 Go 对齐时同步对照 `pkg/objstore/parse_test.go`。若变更影响真正的打开行为，再补 `storage_test.rs` 或具体 provider 的独立测试。
- 兼容风险主要是既有 URI 解析结果、错误脱敏和默认 path-style；性能风险主要来自解析阶段同步读取大 GCS 凭证文件及反复克隆 options，但当前配置规模很小，不能在无基准证据时引入全局缓存或共享状态。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/objstore` 确认目标及 Go/Rust 测试均已索引；`node --file pkg/objstore/parse.rs --offset 1 --limit 500` 与后续 offset 499 查询覆盖源码 1–642 行和 53 个符号；`query` 确认 `ParseBackend`、`ParseRawURL`、`FormatBackendURL`、`ExtractQueryParameters` 的 Rust/Go 定义。精确 `callers/callees --file pkg/objstore/parse.rs` 无输出，因此调用边由 `rg` 补证。
- 已阅读生产与边界文件：`pkg/objstore/parse.rs`、`pkg/objstore/Cargo.toml`、`pkg/objstore/lib.rs`、`pkg/objstore/storage.rs`、`pkg/objstore/helper.rs`、`pkg/executor/importer/precheck.rs`、`pkg/planner/extstore/extstore.rs`。
- 已阅读对照与测试：`pkg/objstore/parse.go`、`pkg/objstore/parse_test.rs`、`pkg/objstore/parse_test.go`；并通过源码搜索确认其他直接调用位置。
- 测试证据覆盖：各 scheme 与不支持 scheme、缺 bucket 脱敏、query 覆盖且不回写 options、GCS 凭证文件、S3 profile 与 AK/SK 配对、force-path-style、含 `+` 密钥、含端口 host、本地相对路径清理、格式化不携带秘密。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核文档只描述由上述符号、调用点和测试支持的当前事实。
