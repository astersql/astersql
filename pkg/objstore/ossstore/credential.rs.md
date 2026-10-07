# `pkg/objstore/ossstore/credential.rs`

## 文件定位

本文件属于 `astersql-objstore-ossstore` crate 的凭证层。crate 入口 `pkg/objstore/ossstore/lib.rs` 通过 `mod credential` 声明模块并用 `pub use credential::*` 重导出其公开符号；`pkg/objstore/ossstore/Cargo.toml` 则表明该 crate 直接依赖 `anyhow`、`arc-swap`、`tokio` 以及 reqsign 的 Aliyun OSS、文件读取和 HTTP 发送组件。

它位于 OSS 配置与实际请求之间：`pkg/objstore/ossstore/store.rs::NewOSSStorage` 根据后端配置创建 `StaticCredentialsProvider` 或 `ReqsignCredentialsProvider`，动态凭证再包进 `CredentialRefresher`；`pkg/objstore/ossstore/interface.rs::AliyunOssApi::client` 在构造每次请求使用的 `ali_oss_rs::blocking::Client` 时读取当前凭证快照。文件不执行 OSS 对象操作，也不决定 bucket、region 或 endpoint。

## 核心职责

1. `ProviderCredentials` 定义 OSS 签名所需的一份完整快照，避免 AK、SK 和 STS token 分字段更新时出现代际混用。
2. `CredentialsProvider` 把静态密钥、reqsign 默认链和刷新缓存统一为同步的 `get_credentials` 接口。
3. `StaticCredentialsProvider` 保存并复制返回显式配置的固定凭证。
4. `ReqsignCredentialsProvider` 配置 reqsign Aliyun 默认凭证链，并把其异步查询桥接成 crate 内部的同步接口；可选配置 AssumeRole 的 `role_arn` 与 `external_id`。
5. `CredentialRefresher` 先同步取得可用凭证，再由唯一后台线程周期刷新，以原子整快照发布给并发读者，并在关闭或析构时停止线程。

本文件的关键可用性约束是“先成功初始化，后向读者发布或启动后台线程”：`refresh_once` 失败不会覆盖旧快照，`start_refresh_with_interval` 的首次刷新失败也不会创建 worker。

## 主要符号

- `ProviderCredentials { access_key_id, access_key_secret, security_token, provider_name }`：可克隆的完整凭证值。`provider_name` 不参与签名；`store.rs::metadata_region` 用它识别 ECS RAM Role 来源并决定是否查询实例 region。
- `CredentialsProvider: Send + Sync`：对象安全的提供者 trait，唯一方法为 `fn get_credentials(&self) -> anyhow::Result<ProviderCredentials>`。`Send + Sync` 允许其通过 `Arc<dyn CredentialsProvider>` 在存储对象与刷新线程间共享。
- `StaticCredentialsProvider::new(String, String, String) -> Self`：把显式 AK/SK/token 固化为 `provider_name = "static"`；其 trait 实现每次返回快照克隆，不做 I/O。
- `ReqsignCredentialsProvider::new(&str, &str) -> Result<Self>`：创建多线程 Tokio runtime、reqsign `Context` 和 `DefaultCredentialProvider`。仅当 `role_arn` 非空时附加 `AssumeRoleCredentialProvider`；`external_id` 也仅在该分支且非空时生效，角色会话名固定为 `tidb-ossstore`。
- `ReqsignCredentialsProvider::get_credentials`：在自有 runtime 上 `block_on` reqsign 查询；把 reqsign 的 `None` 转为明确错误，把缺失的 security token 转为空字符串，并标记 `provider_name = "reqsign_default"`。
- `CredentialRefresher::new(Arc<dyn CredentialsProvider>) -> Self`：创建未初始化的刷新器；快照为空、停止标志为 `false`、worker 为空。
- `CredentialRefresher::refresh_once(&self) -> Result<()>`：先调用底层 provider，成功后把新值包装为 `Arc` 并通过 `ArcSwapOption::store` 一次性替换整份快照。
- `CredentialRefresher::start_refresh(&Arc<Self>) -> Result<()>`：使用五秒默认间隔委托给 `start_refresh_with_interval`。
- `CredentialRefresher::start_refresh_with_interval(&Arc<Self>, Duration) -> Result<()>`：同步刷新一次，然后在 worker 锁保护下幂等地创建至多一个后台线程。
- `CredentialRefresher::close(&self)`：设置停止标志、唤醒条件变量、取走并 join worker；无 worker 时可安全重复调用。
- `impl CredentialsProvider for CredentialRefresher`：用 `load_full` 取得当前不可变快照并复制返回；初始化前返回 `credentials not initialized`。
- `impl Drop for CredentialRefresher`：析构时调用 `close`，确保仍存在的 worker 不会脱离对象生命周期。

## 执行流程

动态凭证在应用中的主流程如下：

1. `store.rs::NewOSSStorage` 在未同时提供 `AccessKey` 和 `SecretAccessKey` 时调用 `ReqsignCredentialsProvider::new(RoleArn, ExternalId)`。
2. 它以该 provider 创建 `CredentialRefresher`，并先调用 `refresh_once`；失败会被加上 `failed to get initial OSS credentials` 上下文并终止存储构造。
3. 刷新器本身作为 `Arc<dyn CredentialsProvider>` 传给探测 region 和正式/预签名两个 `AliyunOssApi`。`set_backend_credentials` 在需要向 TiKV 转发凭证时也读取同一快照。
4. `AliyunOssApi::client` 在每次创建 SDK client 时调用 `get_credentials`，因此后续请求能够观察到最新发布的整份 AK/SK/token。
5. bucket region 探测和权限检查成功后，`NewOSSStorage` 调用 `start_refresh`。该方法会再同步刷新一次，然后创建后台线程；线程用条件变量等待五秒或关闭通知，超时后调用 `refresh_once`。
6. 周期刷新失败只记录 `failed to refresh OSS credentials` 警告，线程继续运行，读者仍取得上一次成功快照。
7. `OSSStore::Close` 调用 `CredentialRefresher::close`；若调用者遗漏显式关闭，最后一个刷新器所有者析构时仍由 `Drop` 执行相同清理。

静态凭证路径不创建刷新器：`NewOSSStorage` 直接构造 `StaticCredentialsProvider`，所有 API 调用读取同一个克隆值，`OSSStore::credential_refresher` 为 `None`。

## 数据与状态

`ProviderCredentials` 是最小发布单元。`security_token` 用空字符串表达“无 STS token”；`AliyunOssApi::client` 仅在其非空时调用 SDK 的 `sts_token`。字符串未在本层做非空或格式校验，静态路径是否成立由 `NewOSSStorage` 的“AK 与 SK 同时非空”分支决定，reqsign 路径则依赖上游 provider 的返回。

`CredentialRefresher` 有三组状态：底层 `provider` 在整个生命周期内不变；`credentials: ArcSwapOption<ProviderCredentials>` 从 `None` 变为最近一次成功值；`stop` 与 `worker` 管理后台线程。快照由 `ArcSwapOption` 原子替换，读者持有的旧 `Arc` 可继续有效，且不会看到部分字段来自不同刷新轮次。

`worker: Mutex<Option<JoinHandle<()>>>` 保证已安装 worker 后再次启动是幂等的。需要注意，`start_refresh_with_interval` 在获取 worker 锁和检查 `is_some` 之前先调用 `refresh_once`，所以重复或并发调用不会创建多个线程，但仍可能额外拉取凭证。`close` 取走 handle 后 join；后台退出后当前实现不会自行清空 handle，只有 `close` 完成该状态转换。

## 依赖与调用关系

上游直接关系：

- `pkg/objstore/ossstore/store.rs::NewOSSStorage` 构造三个 provider 类型，执行初次刷新与 `start_refresh`，并把动态刷新器保存在 `OSSStore` 中。
- `store.rs::set_backend_credentials` 通过 trait 读取快照，选择写入或清空传给 TiKV 的 AK/SK/token。
- `store.rs::build_api` 把 `Arc<dyn CredentialsProvider>` 传入 `AliyunOssApi::new`；RustCodeGraph 显示它由 `NewOSSStorage` 调用。
- `pkg/objstore/ossstore/interface.rs::AliyunOssApi::client` 在构造 ali-oss-rs client 时读取 AK/SK/token。
- `store.rs::OSSStore::Close` 调用刷新器 `close`，将存储生命周期与 worker 生命周期绑定。

下游依赖：

- `reqsign_aliyun_oss` 提供默认链和 AssumeRole provider；`reqsign_core::ProvideCredential` 执行凭证查询。
- `reqsign_core::Context` 注入 `OsEnv`、Tokio 文件读取器和 reqwest HTTP 发送器，覆盖默认链所需的环境、文件与网络能力。
- 自建 `tokio::runtime::Runtime` 把异步 reqsign API 转换成同步 trait；Cargo 为 Tokio 启用 `rt-multi-thread`。
- `arc_swap::ArcSwapOption` 提供无 `Mutex` 的读取和原子快照替换；标准库 `Mutex`、`Condvar`、`JoinHandle` 只负责 worker 控制。
- `anyhow` 统一构造、传播及补充错误上下文，`log` 记录后台刷新与 join 异常。

## 错误处理与边界

- `ReqsignCredentialsProvider::new` 用 `?` 传播 Tokio runtime 创建失败；builder 本身在本文件中不返回可处理错误。
- reqsign 查询的执行错误原样经 `anyhow::Result` 传播；默认链返回 `None` 时转换为 `no credentials found in the Aliyun default credential chain`，避免把“没有凭证”误作空凭证成功发布。
- `refresh_once` 先完整获取新值、后 store，因此 provider 失败时旧快照保持不变。后台线程吞下该轮错误并告警；初始同步刷新则向调用者返回错误。
- 未初始化的刷新器调用 `get_credentials` 返回错误而非空值或 panic。独立测试 `credential_test.rs::test_credential_refresher` 明确覆盖这一边界。
- `start_refresh_with_interval` 接受任意 `Duration`；本层不拒绝零或极短间隔，调用方必须避免造成紧密刷新循环。生产入口只使用五秒常量。
- 对停止标志、worker 或条件变量的锁若 poisoned，代码使用 `expect` 并 panic；这是当前实现的故障边界，不会转换成 `Result`。
- worker 内的 provider panic 会导致线程退出；`close` 在 join 发现 panic 时记录警告。它不把该异常返回给关闭调用者。
- `close` 没有返回值，不能报告清理失败；其保证是发出停止通知并同步等待现有 worker 结束。

## 并发与资源生命周期

读路径不取得 worker 控制锁：`ArcSwapOption::load_full` 原子加载一个 `Arc<ProviderCredentials>`，随后复制其值。写路径只有 `refresh_once` 的整快照 store；生产运行时由初始化线程或唯一 worker 调用。`CredentialsProvider: Send + Sync` 是跨线程共享的类型约束，但 trait 本身并不替任意自定义 provider 串行化并发调用；当前生产启动流程在 worker 建立前完成同步调用。

worker 持有 `Arc<CredentialRefresher>`，所以仅丢弃外层 `OSSStore` 并不会在 worker 自持引用尚存时自然触发刷新器 `Drop`；正常生命周期必须通过 `OSSStore::Close` 发出停止信号，使线程退出并释放其 `Arc`。显式 `close` 后，worker handle 被取走并 join，随后最后一个外部引用析构时的 `Drop::close` 成为空操作。

条件变量让关闭不必等待完整刷新间隔：`close` 将布尔值设为 `true` 并 `notify_all`，`wait_timeout_while` 被唤醒后立即退出。若 provider 的一次 `get_credentials` 已经在执行，当前实现没有向 reqsign 调用传递取消信号；`close` 会在 join 中等待该调用返回。`ReqsignCredentialsProvider` 自有 Tokio runtime，其析构随 provider 生命周期发生。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/objstore/ossstore/credential.go`，独立测试分别是 `credential_test.go::TestCredentialRefresher` 与 `credential_test.rs::test_credential_refresher`。

- Go `credentialRefresher` 与 Rust `CredentialRefresher` 都在启动时同步刷新、默认每五秒刷新、刷新失败仅告警、读取未初始化状态时报错，并在关闭时等待后台执行单元退出。
- Go 用 `atomic.Pointer[credentials.Credentials]` 发布整快照，Rust 用 `ArcSwapOption<ProviderCredentials>`；两者都避免热读路径因 provider 内部串行获取而阻塞，也都保证字段成组更新。
- Go 用 `context.WithCancel`、`WaitGroupWrapper` 和 goroutine 管理退出；Rust 用停止布尔值、`Condvar` 和 `JoinHandle`。Rust 额外实现 `Drop`，但生产上仍依赖 `OSSStore::Close` 打破 worker 持有的 `Arc` 生命周期环。
- Go 文件只包装 Alibaba SDK 的 `providers.CredentialsProvider`；Rust 文件还定义统一 trait、静态 provider 与 reqsign 默认链适配器，这是 Rust OSS 接线所需的额外职责。
- Go 测试借助 `testing/synctest` 推进虚拟时间并验证较长运行后的刷新；Rust 测试注入原子计数 provider 与五毫秒测试间隔，验证未初始化错误、整快照字段一致、至少多次刷新以及 `close` 后计数冻结。
- Rust 测试目前没有直接覆盖 `StaticCredentialsProvider`、reqsign/AssumeRole 配置、重复启动、provider 返回错误、锁 poison 或 worker panic；这些不能从现有测试推断为已验证行为。

## 扩展指南

- 新增凭证来源时，优先实现 `CredentialsProvider` 并在 `store.rs::NewOSSStorage` 的选择分支接线；动态或会过期来源应继续通过 `CredentialRefresher` 发布整快照。同步更新独立的 `pkg/objstore/ossstore/credential_test.rs`，不要把测试内嵌回生产源文件。
- 扩展 `ProviderCredentials` 字段时，必须同时检查 `StaticCredentialsProvider::new`、`ReqsignCredentialsProvider::get_credentials`、`CredentialRefresher` 的快照复制、`store.rs::set_backend_credentials` 和 `interface.rs::AliyunOssApi::client`，防止签名字段漏传或跨代组合。
- 调整刷新策略时，应保留“首次成功后再对外使用”“失败保留旧值”“单 worker”“关闭可及时唤醒并 join”四个不变量，并增加重复 `start_refresh_with_interval`、失败恢复和 close/refresh 交错测试。过短间隔会放大网络、STS 配额与日志压力。
- 若要让关闭可取消正在进行的 reqsign 网络请求，需要把取消语义贯穿 provider trait、reqsign context 和 worker，而不只是缩短条件变量等待；这会改变公开 trait，应评估所有实现及 mock。
- 若调整 AssumeRole 参数，应在 `ReqsignCredentialsProvider::new` 修改 builder 接线，并验证空/非空 `role_arn`、`external_id` 组合以及会话名兼容性。涉及真实云端行为的部分当前本地单元测试没有覆盖。
- 任何安全相关日志都不应输出 AK、SK 或 token；当前日志只包含错误文本。新增错误上下文时需继续避免泄露凭证内容。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 `pkg/objstore/ossstore/credential.rs`（231 行、18 个符号）；`node --file` 核对了本文件全貌，`query` 确认 `CredentialRefresher`、`StaticCredentialsProvider`、`ReqsignCredentialsProvider`、`CredentialsProvider` 及其在 `store.rs`、测试和 Go 对照中的候选。
- RustCodeGraph `node build_api` 与 `node set_backend_credentials`：确认二者均位于 `store.rs`、由 `NewOSSStorage` 调用；前者向 `AliyunOssApi::new` 传 provider，后者通过 `get_credentials` 转发当前快照。
- 生产源码：`pkg/objstore/ossstore/credential.rs`、`store.rs::NewOSSStorage`/`OSSStore::Close`/`build_api`/`set_backend_credentials`、`interface.rs::AliyunOssApi::client`、模块入口 `lib.rs`。
- crate 配置：`pkg/objstore/ossstore/Cargo.toml`，核对 crate 名、`lib.rs` 入口、`autotests = false`、Tokio runtime feature 和 reqsign/arc-swap/anyhow/log 依赖；测试由 `lib.rs` 的 `#[cfg(test)] include!` 显式接入。
- Go 语义对照：`pkg/objstore/ossstore/credential.go`；Go 回归测试：`credential_test.go::TestCredentialRefresher`。
- Rust 回归测试：`pkg/objstore/ossstore/credential_test.rs::test_credential_refresher`，覆盖未初始化、手动刷新、周期整快照更新与关闭后停止。依据任务约束，本次是纯文档分析，未运行 Cargo 或单元测试。
