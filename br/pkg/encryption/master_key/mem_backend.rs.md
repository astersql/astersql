# `br/pkg/encryption/master_key/mem_backend.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-encryption-master-key`，crate 根是 [`lib.rs`](lib.rs)，并由其中的 `pub mod mem_backend` 与 `pub use mem_backend::*` 对外公开。它不是 [`master_key.rs`](master_key.rs) 中 `AnyBackend` 的独立后端变体，而是把已经取得的明文 32 字节主密钥封装成 AES-256-GCM 内容加解密器的底层组件。

生产路径有两类直接使用者。[`file_backend.rs`](file_backend.rs) 从本地十六进制密钥文件取得明文 key 后长期持有一个 `MemAesGcmBackend`，用它完成加密和解密；[`kms_backend.rs`](kms_backend.rs) 从云 KMS 解封数据密钥后构造并缓存一个 `MemAesGcmBackend`，用它解密备份内容。RustCodeGraph 对目标文件的文件级关系也列出了这两个使用者。

## 核心职责

该文件只承担三项聚焦职责：

1. 通过 `NewMemAesGcmBackend` 把调用方提供的字节复制进 `PlainKey`，并以 `CryptographyTypeAesGcm256` 强制校验密钥恰好为 32 字节。
2. 通过 `MemAesGcmBackend::EncryptContent` 执行 AES-256-GCM 加密，把 `aes-gcm` crate 返回的“密文 + 16 字节认证标签”拆开，分别放入 `EncryptedContent.Content` 和元数据 `aes_gcm_tag`；同时写入算法名和 IV。
3. 通过 `MemAesGcmBackend::DecryptContent` 校验算法元数据，恢复 IV 和认证标签，重新拼装 AEAD 输入并完成认证解密。只有认证成功才返回明文。

随机 IV 的生成、文件格式解析、KMS 网络调用、后端选择和 protobuf 线格式编解码都不在本文件内。特别是 `EncryptedContent` 是 [`pb.rs`](pb.rs) 中只包含 `Vec<u8>` 与 `HashMap` 的轻量 Rust 镜像，本 crate 自身不负责将它编码成真正的 protobuf 字节流。

## 主要符号

- `pub struct MemAesGcmBackend { key: PlainKey }`：唯一状态类型。字段私有，crate 外调用者不能直接替换 key；`PlainKey` 来自 `astersql-br-pkg-kms`，内部拥有 key 字节及算法标签。
- `pub fn NewMemAesGcmBackend(key: &[u8]) -> Result<MemAesGcmBackend, String>`：公开构造器。先复制输入，再调用 `NewPlainKey(..., CryptographyTypeAesGcm256)`；后者根据 [`br/pkg/kms/common.rs`](../../kms/common.rs) 的 `TargetKeySize` 规则要求精确 32 字节。失败消息增加 `failed to create new mem aes gcm backend` 上下文。
- `pub fn MemAesGcmBackend::EncryptContent(&self, plaintext: &[u8], iv: &IV) -> Result<EncryptedContent, String>`：公开加密原语。它不生成 IV，也不接受取消上下文；调用者必须提供适用于 GCM 的 IV。
- `pub fn MemAesGcmBackend::DecryptContent(&self, content: &EncryptedContent) -> Result<Vec<u8>, String>`：公开认证解密原语。它依次检查 `method`、`iv` 和 `aes_gcm_tag` 元数据，并把认证失败归入“wrong master key”错误语义。

本文件没有模块级常量、trait、条件编译项或内部辅助函数。元数据键、算法字符串和 IV 类型均来自 [`common.rs`](common.rs)。

## 执行流程

构造流程如下：调用者传入 key 切片；`NewMemAesGcmBackend` 用 `to_vec` 取得独立所有权；`NewPlainKey` 按 AES-GCM-256 标签检查长度；成功后将 `PlainKey` 存入后端。原始切片随后可被修改或释放，不会改变实例内的副本。

加密流程如下：

1. 创建空 `EncryptedContent`，在元数据中写入 `method = "aes256-gcm"`，并复制 `iv.AsSlice()` 到 `iv` 字段。
2. 用 `self.key.Key()` 构造 `Aes256Gcm`，再把 IV 字节作为 nonce。
3. `Aead::encrypt` 返回密文末尾附带认证标签的字节串；本实现按固定 `tag_len = 16` 分割。
4. 前半段写入 `Content`，最后 16 字节写入 `Metadata["aes_gcm_tag"]`。这种拆分是持久化格式的一部分，而非仅供内部使用。

解密流程如下：

1. 必须存在 `method`，且字节值精确等于 `aes256-gcm`；本函数不做算法协商。
2. 必须存在 `iv`，并交给 `NewIVFromSlice` 做 12/16 字节长度解析和复制。
3. 必须存在 `aes_gcm_tag`。函数克隆 `Content`，把 tag 追加到尾部，恢复 `aes-gcm` crate 所需的组合输入。
4. 以相同 key 和 nonce 调用 `Aead::decrypt`；GCM 会同时验证密文与 tag，验证成功才返回明文。

实际上游链路为 `FileBackend::Encrypt -> NewIVGcm -> EncryptContent`、`FileBackend::Decrypt -> DecryptContent`，以及 `KmsBackend::DecryptWithContext -> NewMemAesGcmBackend -> DecryptContent`。KMS 路径只使用本文件解密；文件路径同时使用加密和解密。

## 数据与状态

`MemAesGcmBackend` 的持久状态只有一个 `PlainKey`。`PlainKey` 拥有 key 的 `Vec<u8>`，所以构造时会有一次复制；每次加解密都通过只读借用访问同一 key，不轮换、不重新读取外部来源，也没有内部缓存计数或可变状态。

`EncryptedContent.Content` 只保存不含 tag 的密文。它的 `Metadata: HashMap<String, Vec<u8>>` 至少包含：`method = b"aes256-gcm"`、调用方提供的 IV 副本，以及 16 字节 `aes_gcm_tag`。解密会克隆整段 `Content` 再追加 tag，因此时间和临时内存都随密文大小线性增长；现有 Rust/Go 测试用 1,000,000 字节输入验证了大块数据往返，但没有流式处理能力。

本文件不记录 IV 是否曾使用，也不保证 nonce 唯一性。生产文件后端通过 `NewIVGcm` 使用操作系统随机源生成 12 字节 IV；直接调用 `EncryptContent` 的代码必须自行保证同一 key 下不重复使用 nonce。

## 依赖与调用关系

- crate 内上游：[`file_backend.rs`](file_backend.rs) 构造并持有该类型，委托 `EncryptContent`/`DecryptContent`；[`kms_backend.rs`](kms_backend.rs) 在 KMS 解封成功后构造该类型，并在 `Mutex<Option<CachedKeys>>` 中缓存以供解密。
- crate 内下游：[`common.rs`](common.rs) 提供 `IV`、元数据键、`aes256-gcm` 字符串和 `NewIVFromSlice`；[`pb.rs`](pb.rs) 提供 `EncryptedContent`。
- workspace 下游依赖：[`br/pkg/kms/common.rs`](../../kms/common.rs) 提供 `CryptographyType`、`PlainKey`、`NewPlainKey`，其中 AES-GCM-256 的目标 key 长度为 32。
- 外部依赖：`aes-gcm = "0.10"` 提供 `Aes256Gcm`、`Aead`、`KeyInit` 和 nonce 类型。上述依赖及本库的 `lib.rs` 入口由 [`Cargo.toml`](Cargo.toml) 明确声明。
- 装配边界：[`master_key.rs`](master_key.rs) 只把 `FileBackend` 和 `KmsBackend` 放入 `AnyBackend`；因此外部业务通常通过这两个包装层间接到达本文件，而不是把内存后端当作可选主密钥配置。

RustCodeGraph 的目标文件节点给出 `file_backend.rs` 与 `kms_backend.rs` 两个文件级使用者；对 `NewMemAesGcmBackend` 的精确 `callers` 查询在本地索引上超过 90 秒无输出后被中止，因此具体方法级边使用相邻索引节点与源码引用搜索交叉核对，不声称图查询给出了完整 caller 闭包。

## 错误处理与边界

公开函数统一返回 `Result<_, String>`。构造器拒绝任何非 32 字节 key。加密可能返回 cipher 初始化或 AEAD 加密错误；在构造器保证 key 长度、调用方提供合法 12 字节 GCM nonce 的正常路径上，初始化应成功。解密为缺失的 `method`、`iv`、tag 分别返回错误；算法不匹配时同时报告期望值和实际字节的有损 UTF-8 展示；认证失败统一包装为 `wrong master key :decrypt in GCM mode failed`，它也覆盖错误 key、被篡改的密文、被篡改/长度错误的 tag 等不可区分情形。

当前实现存在需要调用者遵守的非 `Result` 边界：`IV` 的字段是公开的，而 `NewIVFromSlice` 同时接受 12 字节 GCM IV 和 16 字节 CTR IV；但 `aes_gcm::Nonce` 固定要求 12 字节。`EncryptContent` 直接调用 `Nonce::from_slice(iv.AsSlice())`，`DecryptContent` 也会在解析出 16 字节 IV 后执行同一步骤，因此传入非 12 字节数据会触发依赖库的长度断言（panic），而不是返回错误。安全调用必须使用 `NewIVGcm` 或确认 `iv.Data.len() == 12`；若未来修复，应在 nonce 构造前显式校验 `IvTypeGcm` 和长度，并在独立测试文件中添加回归用例。

另一个格式边界是加密端假定 `Aead::encrypt` 的结果至少有 16 字节并执行 `sealed.len() - 16`；对当前 AES-GCM 实现即使空明文也会产生 16 字节 tag，因此正常算法配置下成立。解密端不预先要求 tag 恰好 16 字节，而让 AEAD 验证返回认证错误。空明文可由算法处理，但现有专门测试未单独覆盖。

## 并发与资源生命周期

该类型没有锁、任务、通道、文件句柄或网络资源，所有公开方法都只借用 `&self`。`PlainKey` 内部是只读 `Vec<u8>`，所以实例可被多个只读调用共享时不会在本文件内产生状态竞争；真正跨线程共享时仍由上层选择 `Arc` 等所有权容器。KMS 后端的缓存互斥属于 [`kms_backend.rs`](kms_backend.rs)，不属于本类型。

每次调用的 cipher、nonce、输出和临时拼接缓冲都在栈帧/局部所有权范围内，返回或报错后释放。后端销毁时由 Rust 自动释放 key 的 `Vec<u8>`，但 `PlainKey` 当前没有显式清零（zeroize）实现；因此不能声称 drop 后密钥材料已从内存擦除。`FileBackend::Close` 是空操作，`KmsBackend::Close` 只关闭 provider，也不会主动清除此 key。

## 与 Go 版本的对应关系

Rust 实现逐项对应 [`mem_backend.go`](mem_backend.go)：同样用 `kms.NewPlainKey`/`NewPlainKey` 强制 32 字节 key，同样写入 method、IV 和独立 tag 元数据，同样把 tag 重新附加后执行 GCM 认证解密，主要错误文本也保持一致。Go 的 `cipher.AEAD.Seal` 与 Rust `Aead::encrypt` 都产生“密文 + tag”，两端均将最后一个 GCM overhead（当前 16 字节）拆开保存。

实现形式存在几项差异：Go 构造器返回指针，Rust 按值返回并由所有权管理；Go 方法保留但忽略 `context.Context` 参数，Rust 方法完全省略 context，因此本地 CPU 加解密都不支持中途取消；Go 使用生成的 `encryptionpb.EncryptedContent`，Rust 使用 [`pb.rs`](pb.rs) 的轻量镜像；Go 从 `aesgcm.Overhead()` 动态取得 tag 长度，Rust 固定写死 16。对 AES-GCM 而言结果一致，但若更换 AEAD 实现，Rust 的固定值必须重新审查。

独立测试 [`mem_backend_test.rs`](mem_backend_test.rs) 与 Go 的 [`mem_backend_test.go`](mem_backend_test.go) 场景对齐：32/16 字节 key、正常往返、错误 key、密文篡改、缺 method 和 1 MB 数据。相邻 [`file_backend_test.rs`](file_backend_test.rs) / [`file_backend_test.go`](file_backend_test.go) 还固定了标准 AES-256-GCM 向量、tag 篡改错误包含 `wrong master key`、缺 tag 错误包含 `aes gcm tag not found`。[`parity_test.rs`](parity_test.rs) 再从公开 crate 接口验证 key 长度、method 元数据和往返契约。

## 扩展指南

- 新增算法时，不要只在这里替换 cipher。应同步扩展 `common.rs` 的 method/IV 协议、KMS `CryptographyType` 长度规则、持久化元数据兼容策略、Go 同路径实现，以及独立的 `mem_backend_test.rs`/Go 测试。旧密文的 method 必须仍能明确路由或得到可诊断错误。
- 修改元数据布局或 tag 表示时，应同步检查文件与 KMS 两条调用链，以及真正的 protobuf 读写边界；`method`、`iv`、`aes_gcm_tag` 是跨语言持久化契约，不能当作私有字段随意改名。
- 增强输入防御时，最合适的接入点是 `EncryptContent` 和 `DecryptContent` 在 `Nonce::from_slice` 之前。应把 16 字节 IV、手工构造的错误 `IV`、错误 tag 长度、空明文和缺失各项元数据放进独立 [`mem_backend_test.rs`](mem_backend_test.rs)，不要把测试嵌入生产源文件。
- 优化大对象时，注意当前 API 和 `EncryptedContent` 都以完整 `Vec<u8>` 表示内容，解密还额外克隆一次密文。要改成流式或原地模式会改变调用契约和认证失败时的数据可见性，需同时审查文件/KMS 后端与 Go 兼容性。
- 加强密钥生命周期时，可评估为 `PlainKey` 引入可靠的内存清零；这应在其所属 `astersql-br-pkg-kms` crate 中实现并配套测试，而不是仅在本文件复制另一套 key 类型。
- 修改 tag 长度处理时，优先从 AEAD 类型能力取得长度或将固定 16 明确绑定到 AES-GCM；必须保留标准向量与篡改检测测试，避免只验证“能往返”而破坏跨语言格式。

## 验证依据

- RustCodeGraph：运行 `status` 确认索引包含 7032 个 Rust 文件；运行 `files --filter br/pkg/encryption/master_key` 确认目标及相关文件已索引；用 `node --file ...` 完整读取目标 99 行，并读取 `file_backend.rs`、`kms_backend.rs`、`master_key.rs`、`lib.rs`、`common.rs`、`pb.rs` 和 `br/pkg/kms/common.rs` 的直接相关节点。目标节点明确报告由 `file_backend.rs`、`kms_backend.rs` 使用。
- 调用边补证：`explore 'br/pkg/encryption/master_key/mem_backend.rs MemBackend NewMemBackend Encrypt Decrypt Close'` 识别 `NewMemAesGcmBackend` 的文件/KMS 使用路径；精确 `callers NewMemAesGcmBackend` 因超过 90 秒无输出被中止，随后以两个上游文件的索引节点及 `rg` 符号引用核验具体调用点。
- crate 与源码：读取 [`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、目标 [`mem_backend.rs`](mem_backend.rs)、[`common.rs`](common.rs)、[`pb.rs`](pb.rs)、[`file_backend.rs`](file_backend.rs)、[`kms_backend.rs`](kms_backend.rs)、[`master_key.rs`](master_key.rs) 和 [`br/pkg/kms/common.rs`](../../kms/common.rs)。
- Go 对照与测试：读取 [`mem_backend.go`](mem_backend.go)、[`mem_backend_test.go`](mem_backend_test.go)、[`mem_backend_test.rs`](mem_backend_test.rs)、[`file_backend_test.go`](file_backend_test.go)、[`file_backend_test.rs`](file_backend_test.rs) 与 [`parity_test.rs`](parity_test.rs)。这些证据覆盖构造、标准向量、往返、认证失败、元数据缺失和大对象路径。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证本文存在且恰好包含 11 个固定二级章节，并人工复核本文未把内存后端误写成独立配置后端、未声称具备取消/密钥清零/流式处理等当前不存在的能力。
