# `br/pkg/task/encryption.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-task`（[`br/pkg/task/Cargo.toml`](Cargo.toml)），由 crate 根 [`lib.rs`](lib.rs) 通过 `pub mod encryption` 挂载，并通过 `pub use encryption::*` 平铺导出。它位于 BR 公共任务配置层：把一条 `--master-key` URL 解析成任务层使用的 `encryptionpb::MasterKey` 配置对象，但不读取本地密钥文件、不连接云 KMS，也不执行加解密。

生产调用入口是 [`Config::parseAndValidateMasterKeyInfo`](common.rs)：该函数先处理明文数据密钥互斥与加密算法校验，再按逗号拆分多个 master-key 字符串，逐条去除首尾空白并调用 `validateAndParseMasterKeyString`，最后把结果加入 `MasterKeyConfig.MasterKeys`。因此，本文件只负责“一条 URL 到一个后端配置”的语法与必填字段校验；多密钥聚合、加密算法选择和配置级互斥均由 `common.rs` 负责。

## 核心职责

1. `validateAndParseMasterKeyString` 使用 `url::Url` 解析输入，并按 scheme 分发到 `local`、`aws-kms`、`azure-kms` 或 `gcp-kms` 的专用解析器。
2. 四个专用解析器校验 URL 的 host、path 和 query 参数，并构造 `MasterKeyBackend::File` 或 `MasterKeyBackend::Kms`。
3. 三个 `LazyLock<Regex>` 固化云厂商 key path 的允许形状：AWS 只允许一个 path 段；Azure 接受任意非空 path 尾部；GCP 要求完整的 `projects/.../locations/.../keyRings/.../cryptoKeys/...` 四段资源名。
4. 在配置进入后续备份/恢复流程前拒绝未知 scheme、缺少必填参数或不完整的显式凭据，避免产生部分填充的 KMS 配置。

职责边界很窄：本文件使用的 `crate::stubs::encryptionpb` 是 protobuf 数据结构的本地最小镜像（见 [`stubs.rs`](stubs.rs)），不会验证密钥是否真实存在、凭据是否有效、endpoint 是否可达，也不会保护或清除已进入 `String` 的敏感值。

## 主要符号

- `SchemeLocal`、`SchemeAWS`、`SchemeAzure`、`SchemeGCP`：`validateAndParseMasterKeyString` 的四个分发标签，分别为 `local`、`aws-kms`、`azure-kms`、`gcp-kms`。
- `AWSVendor`、`AzureVendor`、`GCPVendor`：写入 `MasterKeyKms.Vendor` 的稳定厂商标识。
- `AWSRegion`、`AWSEndpoint`、`AWSAccessKeyId`、`AWSSecretKey`：AWS query 键。`REGION` 必填，`ENDPOINT` 可选，access key 与 secret key 必须同时给出或同时省略。
- `AzureTenantID`、`AzureClientID`、`AzureClientSecret`、`AzureVaultName`：Azure query 键，四项全部必填；`AZURE_VAULT_NAME` 的值写入 `AzureKms.KeyVaultUrl`。
- `GCPCredentials`：GCP 的 `CREDENTIALS` query 键，不能为空。
- `AWS_REGEX`：`^/([^/]+)$`，提取单段 AWS key ID，拒绝空 path 和额外 `/`。
- `AZURE_REGEX`：`^/(.+)$`，移除开头 `/` 后保留完整非空尾部，可包含 key version 等多个 path 段。
- `GCP_REGEX`：校验并提取 project、location、key ring、key name；允许末尾有一个 `/`。
- `validateAndParseMasterKeyString(&str) -> Result<MasterKey>`：公开总入口；URL 解析失败包装为 `Error::Trace`，未知 scheme 返回 `Error::Errorf`。
- `parseLocalDiskConfig(&Url) -> Result<MasterKey>`：拒绝非空 host，把 `u.path()` 原样放入 `MasterKeyFile.Path`。
- `parseAwsKmsConfig(&Url) -> Result<MasterKey>`：校验 key ID 与 region，处理可选 endpoint 和成对凭据，构造 AWS KMS 后端。
- `parseAzureKmsConfig(&Url) -> Result<MasterKey>`：校验 path 和四个认证参数，构造 Azure KMS 后端。
- `parseGcpKmsConfig(&Url) -> Result<MasterKey>`：解析标准资源 path，要求凭据，并将四个捕获段归一化为不带前导/尾随斜杠的完整 `KeyId`。

文件没有 trait、struct、impl 或条件编译项；全部函数均为同步纯配置转换函数。虽然后四个解析函数均为 `pub`，生产路径只直接调用总入口，直接调用专用解析器主要见独立测试和 parity 测试。

## 执行流程

生产主链如下：

1. `Config::parseAndValidateMasterKeyInfo` 从 flag 读取逗号分隔的 master-key 配置，完成配置级互斥与加密算法校验。
2. 对每个去除首尾空白的子串调用 `validateAndParseMasterKeyString`。
3. `Url::parse` 将字符串拆成 scheme、host、path 与 query；解析失败立即返回。
4. 总入口按 scheme 精确匹配并调用对应解析器；scheme 区分大小写，未匹配值直接报“不支持”。
5. 专用解析器先验证 path/host，再读取 query 参数：
   - `local`：只检查 host 为空，随后产生 `File` 后端。
   - `aws-kms`：path 必须是单段 key ID，`REGION` 必填；AK/SK 均存在时写入 `AwsKms`，均不存在时保留 `None` 以允许下游默认凭据链，只有一项时失败；可选 `ENDPOINT` 写入通用 KMS 字段。
   - `azure-kms`：path 尾部作为完整 key ID；tenant、client ID、client secret、vault URL 任一为空即失败。
   - `gcp-kms`：path 必须匹配完整资源层级；`CREDENTIALS` 不能为空；捕获段重新拼成标准资源名。
6. 成功结果由 `common.rs` 追加进 `MasterKeyConfig.MasterKeys`；任何一条失败都会中止该次配置解析，并由上游添加 `invalid master key configuration` 上下文。

本文件只完成内存中的构造，不触发网络或文件 I/O。后续如何消费这些配置不在本文件中实现；`stubs.rs` 也明确说明其 KMS 类型“不含真实 KMS 加解密逻辑”。

## 数据与状态

输入状态只有借用的 `&str` 或 `&Url`，输出是拥有自身字符串的 `MasterKey`。解析器把 query pair 收集为局部 `HashMap<String, String>`，再克隆所需值；函数返回后不保留对输入 URL 的借用。

输出后端具有以下不变量：

- `parseLocalDiskConfig` 成功时 `Backend` 必为 `Some(MasterKeyBackend::File(...))`。
- 三个云解析器成功时 `Backend` 必为 `Some(MasterKeyBackend::Kms(...))`，`Vendor` 与解析器固定对应。
- AWS 成功时 `Region` 非空且 `KeyId` 为单段；`AwsKms` 要么包含一对非空凭据，要么为 `None`。
- Azure 成功时 `AzureKms` 为 `Some`，其中四个字段均非空。
- GCP 成功时 `GcpKms` 为 `Some` 且 credential 非空，`KeyId` 是归一化的完整资源路径。

`AWS_REGEX`、`AZURE_REGEX`、`GCP_REGEX` 是进程级惰性静态值。首次访问时编译一次，之后只读复用。正则表达式是源码常量，当前 `unwrap()` 只会在开发者引入无效正则时触发 panic，而不会因用户输入触发。

敏感的 AWS secret、Azure client secret 和 GCP credential 会以普通 `String` 存在于 query map 与返回对象中；这里没有显式清零、密文存储或日志脱敏。日志脱敏属于 `common.rs` 的 flag 处理边界，调用者扩展诊断时不得打印原始 URL 或这些字段。

## 依赖与调用关系

上游调用边：

- RustCodeGraph 将 Rust `validateAndParseMasterKeyString` 的生产调用者识别为 [`Config::parseAndValidateMasterKeyInfo`](common.rs)，测试调用者为 [`parity_test.rs`](parity_test.rs) 的 `go_rust_public_contract_matches`。
- 四个 `parse*Config` 的内部调用者是 `validateAndParseMasterKeyString`；它们还被 [`encryption_test.rs`](encryption_test.rs) 和 `parity_test.rs` 直接调用以验证分支契约。
- crate 根 [`lib.rs`](lib.rs) 挂载 `encryption.rs`，并在 `cfg(test)` 下把 `encryption_test.rs` 注册为独立测试模块；测试逻辑未嵌入生产源文件。

下游依赖：

- 标准库 `HashMap` 保存已解码的 query，`LazyLock` 提供正则的线程安全惰性初始化。
- Cargo 直接依赖 `url = "2"` 与 `regex = "1"`，对应 URL 拆解和 path 校验。
- `crate::stubs::{Error, Result}` 提供任务层错误类型；`crate::stubs::encryptionpb` 提供 `MasterKey`、各厂商字段和 `MasterKeyBackend` 枚举。
- 当前 `Cargo.toml` 没有控制本文件行为的 feature，也没有真实云厂商 SDK 依赖，进一步说明这里是配置解析边界而不是 KMS 客户端。

RustCodeGraph 索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；其 `explore` 同时返回 Go 与 Rust 同名符号，因此本文对语言归属均以路径限定。

## 错误处理与边界

- 输入不是 `url::Url` 可接受的绝对 URL 时，`validateAndParseMasterKeyString` 把解析错误字符串放入 `Error::Trace(Error::new(...))`。
- 未知、空或大小写不同的 scheme 返回 `unsupported master key type: <scheme>`。
- `local` 只拒绝非空 host；它没有显式拒绝空 path、query 或 fragment，也不检查文件是否存在、是否可读。因此“绝对路径”在当前实现中实际由“无 host”近似表达，调用者不能把成功解析等同于可用文件。
- AWS 拒绝空 key ID、多段 path、缺 region，以及只提供 AK/SK 之一；允许省略整对显式凭据，并允许空 endpoint。
- Azure 拒绝空 path 或四个 query 值中任何空值，但不验证 vault URL 的格式、租户/客户端标识格式或 key path 的段数。
- GCP 拒绝非固定层级 path 或空 credential；各资源段只能是非空且不含 `/` 的字符串，但这里不进一步验证厂商命名规则。
- URL query 会经过 `url` crate 的解码。由于实现先收集为 `HashMap`，重复 query 键以最后出现的值为准；Go 对照使用 `url.Values.Get`，取同名键的第一个值。这是当前 Rust/Go 在重复键输入上的已确认差异，测试尚未覆盖，扩展兼容性时应先决定是否需要对齐。
- 所有校验均为失败即返回；没有部分成功结果。上游循环若后续 key 失败，`self.MasterKeyConfig` 已可能含有先前成功项，但整个方法返回 `Err`，调用者不应在错误后继续使用该配置对象。

## 并发与资源生命周期

函数没有 `async`、线程、锁、通道、事务或外部句柄；每次调用只分配 URL、局部 query map 与输出字符串，调用结束即释放临时状态。不同线程可并发调用全部公开函数，因为它们不修改共享业务状态。

唯一共享资源是三个 `LazyLock<Regex>`。标准库负责一次性初始化同步；初始化完成后正则只读共享。首次命中某个厂商解析器会承担一次正则编译成本，后续调用复用编译结果。正则均有首尾锚点，且模式简单，不包含易导致灾难性回溯的嵌套重复结构。

返回值拥有全部数据，不依赖 `Url` 的生命周期。凭据字符串的生命周期随局部 map 和返回的 `MasterKey` 分别结束；本文件没有零化机制，因此含凭据对象的复制、调试输出与持久化应由上层严格控制。

## 与 Go 版本的对应关系

直接对照文件是 [`encryption.go`](encryption.go)，测试对照是 [`encryption_test.go`](encryption_test.go)。Rust 保留了 Go 的四个 scheme、厂商与 query 常量、三条正则、分发顺序、成功字段形状和主要错误文案：

- Go `validateAndParseMasterKeyString` 对应 Rust 同名函数；`net/url.Parse`、switch 和 `errors.Trace/Errorf` 分别对应 `Url::parse`、`match` 和本地 `Error` 构造。
- Go 的 protobuf oneof 指针包装对应 Rust `Option<MasterKeyBackend>` 枚举；厂商专属消息指针对应 `Option<AwsKms/AzureKms/GcpKms>`。
- Go AWS 允许凭据全部省略以交给环境/默认链，Rust 用 `AwsKms: None` 保留该语义。
- Azure key ID 保留 `/` 后的完整多段 path；GCP 则从捕获段重建规范资源名，两种语言一致。
- Rust `encryption_test.rs` 逐项复刻 Go 的四组表驱动用例：local 成功/带 host 失败，AWS 完整凭据/缺 key ID/凭据不成对，Azure 四参数齐全/缺 secret，GCP 标准资源/错误 path/缺 credential。

已知差异与未覆盖点：Rust 函数及常量因 crate 导出策略为 `pub`，而 Go 小写函数仅包内可见；Rust URL 解析库与 Go `net/url` 对畸形 URL 的接受范围未被穷举验证；重复 query 键的首值/末值语义不同。独立 Rust 测试不直接覆盖总入口的 URL 解析和所有 scheme 分发，只有 `parity_test.rs` 覆盖 AWS 总入口及 unknown scheme；local/Azure/GCP 的总入口分发仍缺直接断言。

## 扩展指南

新增厂商时，应在本文件同时增加 scheme/vendor/query 常量、必要的惰性正则、专用解析函数，以及 `validateAndParseMasterKeyString` 的分发分支；还必须在 `stubs.rs` 的 `MasterKeyKms` 数据模型或真实 protobuf 对照中确认承载字段，而不是只让 URL 校验通过。同步修改独立的 [`encryption_test.rs`](encryption_test.rs)，并参照 Go 版本补齐 [`encryption.go`](encryption.go) 与 [`encryption_test.go`](encryption_test.go) 的对应语义；不要把测试写回生产源文件。

修改现有厂商参数时，应保持以下风险可见：

- 兼容性：scheme、query 键、path 正则和错误行为属于 CLI 配置契约；收紧规则可能拒绝已有配置，放宽规则可能把错误推迟到真实 KMS 调用。
- 安全性：新增凭据字段必须避免出现在错误和日志中，并检查 `common.rs` 的 flag 脱敏是否覆盖；不要把完整原始 URL放进诊断。
- 行为对齐：优先同步 Go 的校验顺序和默认凭据语义；若要修复重复 query 键等差异，应新增明确的 Go/Rust 边界用例。
- 性能：静态格式校验宜继续使用一次编译的 `LazyLock<Regex>`；不要在每次解析中重复编译正则，也不要在此层引入网络探测。

最可能的修改点是 `validateAndParseMasterKeyString`、对应 `parse*Config`、常量/正则定义及 `stubs.rs` 数据结构。若变更多 key 聚合、明文 key 互斥或包装算法，应改 `Config::parseAndValidateMasterKeyInfo` 及其独立测试，而不是把这些职责塞入本文件。

## 验证依据

本文依据以下直接证据编写：

- 生产实现：[`br/pkg/task/encryption.rs`](encryption.rs)，核对了全部常量、三个静态正则和五个公开函数。
- crate 边界：[`br/pkg/task/Cargo.toml`](Cargo.toml) 与 [`br/pkg/task/lib.rs`](lib.rs)，确认 crate 名、`regex`/`url` 依赖、模块挂载、平铺导出和独立测试模块。
- 生产上游：[`br/pkg/task/common.rs`](common.rs) 的 `Config::parseAndValidateMasterKeyInfo`，确认多 key 拆分、trim、错误包装与 `MasterKeyConfig` 聚合。
- 数据模型：[`br/pkg/task/stubs.rs`](stubs.rs) 的 `encryptionpb`，确认这些类型是本地最小 serde 镜像且不执行真实 KMS 操作。
- Go 对照：[`br/pkg/task/encryption.go`](encryption.go) 与 [`br/pkg/task/common.go`](common.go)，核对函数逐分支语义和生产调用位置。
- 测试：[`br/pkg/task/encryption_test.rs`](encryption_test.rs)、[`br/pkg/task/encryption_test.go`](encryption_test.go) 和 [`br/pkg/task/parity_test.rs`](parity_test.rs)，核对现有成功/失败样例与总入口覆盖范围。
- RustCodeGraph：运行 `status` 后用 `explore` 查询 `br/pkg/task/encryption.rs` 及五个入口，确认 Rust 生产调用边 `Config::parseAndValidateMasterKeyInfo -> validateAndParseMasterKeyString -> parse*Config`，并识别 parity 测试调用边。精确 `files --filter br/pkg/task/encryption` 未返回结果，但 `explore` 能定位并展示该 Rust 文件；调用关系另以源码 `rg` 交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令校验目标文件存在且恰有 11 个固定二级标题，并人工复核没有把配置解析描述成真实密钥访问。
