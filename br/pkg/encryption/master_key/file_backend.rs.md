# `br/pkg/encryption/master_key/file_backend.rs`

## 文件定位

该文件属于 Cargo crate `astersql-br-pkg-encryption-master-key`，crate 入口是同目录的 `lib.rs`，其中以 `pub mod file_backend` 装配本模块并通过 `pub use file_backend::*` 重导出其公开符号。`Cargo.toml` 将该 crate 标为 library，并声明 `aes-gcm`、`rand` 与 `astersql-br-pkg-kms` 依赖；本文件本身不直接调用加密库，而是把本地文件中的主密钥交给 `mem_backend.rs` 的内存 AES-GCM 后端。

在主密钥装配链中，`master_key.rs::CreateBackend` 遇到 `MasterKeyBackend::File` 时读取 `MasterKeyFile.Path`，调用 `createFileBackend`，再包装成 `AnyBackend::File`。因此本文件是“文件配置”到“可执行 AES-256-GCM 后端”的适配层，不负责解析命令行配置，也不负责持久化业务密文。

## 核心职责

- `createFileBackend` 严格读取并校验磁盘主密钥文件：文件必须恰好是 64 个十六进制字符加一个换行符，共 65 字节。
- `decode_hex` 把去掉末尾换行后的 UTF-8 十六进制文本转换为 32 字节密钥；非法 UTF-8、奇数长度或非法十六进制字符均返回错误。
- `FileBackend::Encrypt` 为每次加密生成新的 12 字节 GCM IV，再委托 `MemAesGcmBackend::EncryptContent` 产生密文和元数据。
- `FileBackend::Decrypt` 将密文及元数据原样交给内存后端完成方法、IV、认证标签校验和解密。
- `FileBackend::Close` 保持与统一后端生命周期接口相容，但当前没有资源回收动作。

该层不缓存文件路径、不重新读取或监控文件，也不直接保留文件句柄；创建成功后，后续密码学操作只依赖 `memCache` 中已经构造的密钥状态。

## 主要符号

- `AesGcmKeyLen: usize = 32`：AES-256 原始密钥的字节数。构造函数据此计算文本文件长度 `32 * 2 + 1 = 65`。
- `FileBackend { pub(crate) memCache: MemAesGcmBackend }`：唯一生产类型。字段对当前 crate 可见，便于同 crate 测试用固定 IV 验证标准向量；对 crate 外部并非公开字段。
- `createFileBackend(keyPath: &str) -> Result<FileBackend, String>`：模块级构造入口。它先完整读取文件，再检查长度、末尾换行、hex 解码和内存后端构造，任一步失败都不会返回部分初始化对象。
- `FileBackend::Encrypt(&self, plaintext: &[u8]) -> Result<EncryptedContent, String>`：公开加密入口。通过 `common.rs::NewIVGcm` 使用操作系统随机源生成 GCM IV，然后调用内存后端。
- `FileBackend::Decrypt(&self, content: &EncryptedContent) -> Result<Vec<u8>, String>`：公开解密入口，认证与解密细节由 `MemAesGcmBackend::DecryptContent` 实现。
- `FileBackend::Close(&self)`：空操作。注意它是固有方法，签名使用 `&self`；`master_key.rs` 中 `Backend for AnyBackend` 通过枚举分派调用它。
- `decode_hex(&[u8]) -> Result<Vec<u8>, String>`：私有辅助函数，按两个字符一组调用 `u8::from_str_radix(..., 16)`。

本文件没有 trait、枚举、宏或条件编译项。

## 执行流程

构造流程如下：

1. `master_key.rs::CreateBackend` 从 `MasterKeyBackend::File` 取得路径并调用 `createFileBackend`。
2. `std::fs::read` 一次性读取整个文件；读取失败时立即返回带路径读取语境的字符串错误。
3. 构造函数要求文件长度为 65，并要求最后一个字节是 `\n`。这意味着 CRLF、无换行、额外空白、BOM 或附加内容都不会被容忍。
4. 去掉最后一个换行字节后，`decode_hex` 校验 UTF-8 并逐字节对解码。64 个十六进制字符得到 32 字节密钥。
5. `NewMemAesGcmBackend` 用 `CryptographyTypeAesGcm256` 包装密钥；成功后将其存入 `FileBackend.memCache`。

加密流程是 `Encrypt` → `NewIVGcm` → `MemAesGcmBackend::EncryptContent`。下游将 `method = "aes256-gcm"`、12 字节 `iv` 和 16 字节 `aes_gcm_tag` 写入 `EncryptedContent.Metadata`，而 `Content` 只保存不含 tag 的密文字节。

解密流程是 `Decrypt` → `MemAesGcmBackend::DecryptContent`。下游依次核对方法、IV 和 tag，把 tag 重新拼到密文尾部后执行 AES-256-GCM 认证解密；任一元数据缺失、方法不符、IV 非法或认证失败都会返回错误，不会产出未认证的明文。

## 数据与状态

`FileBackend` 的持久状态只有 `memCache`。文件内容在构造期间作为局部 `Vec<u8>` 存在，hex 解码后又产生密钥 `Vec<u8>`，随后由 `NewMemAesGcmBackend` 转成 `astersql_br_pkg_kms::PlainKey`。构造完成后不保留路径和文件内容，因此磁盘文件被修改或删除不会改变现有实例；要应用轮换后的文件密钥，调用方必须重新创建后端。

输入明文和 `EncryptedContent` 均以借用传入。加密返回新建的 `EncryptedContent`；解密在下游克隆密文字节并附加 tag，不修改调用者传入的对象。`Encrypt`/`Decrypt` 都只借用 `&self`，本文件没有可变计数器、缓存刷新、锁或全局状态。

密钥文件格式是不带前缀的十六进制文本：接受大小写十六进制数字，但不接受 `0x` 前缀、空格或多行内容。长度检查先于解码，因此能进入 `decode_hex` 的生产输入固定为偶数长度；`decode_hex` 仍保留奇数长度防御，避免其内部假设失效。

## 依赖与调用关系

上游生产接线由源码确认：

- `master_key.rs::CreateBackend` 在文件型配置分支调用 `createFileBackend`，并形成 `AnyBackend::File`。
- `multi_master_key_backend.rs::NewMultiMasterKeyBackend` 对每个 `MasterKey` 调用 `CreateBackend`；其 `Decrypt` 依次尝试各后端，所以文件后端可以参与主密钥轮换或回退解密。
- `lib.rs` 公开模块并重导出本文件符号。

直接下游依赖是：

- `common.rs::NewIVGcm`：通过 `rand::rngs::OsRng` 生成 12 字节随机 IV。
- `mem_backend.rs::NewMemAesGcmBackend`：校验并持有 AES-GCM-256 密钥。
- `MemAesGcmBackend::{EncryptContent, DecryptContent}`：调用 `aes-gcm` crate，定义密文元数据和认证失败语义。
- `pb.rs::EncryptedContent`：Rust 侧使用的密文载体，包含 `Content` 与字符串到字节数组的 `Metadata` 映射。

RustCodeGraph 的文件关系报告将 `file_backend_test.rs` 和 `multi_master_key_backend_test.rs` 标为本文件的直接使用者；精确源码接线还显示 `master_key.rs` 直接导入 `FileBackend` 和 `createFileBackend`。因此图的文件级 “used by” 结果不能替代对模块装配源码的核对。

## 错误处理与边界

本模块统一返回 `Result<_, String>`，并在边界处补充语境：文件读取、hex 解码和内存后端创建分别带有 `failed to read...`、`failed to decode...`、`failed to create...` 前缀。长度不符会同时报告期望值与实际值，末尾不是 LF 则返回固定错误 `master key file should end with newline`。

需要保持的边界包括：

- 空文件、63/64/66 字节文件、CRLF 文件或含额外尾随内容的文件在长度或换行检查阶段失败。
- 64 字节主体中出现非 UTF-8 或非十六进制字符时在 `decode_hex` 失败。
- 文件通过格式校验但密钥构造失败时，错误由 `NewMemAesGcmBackend` 包装后上抛。
- 加密时随机源失败会由 `NewIVGcm` 直接传播。
- 解密缺少 `method`、`iv` 或 `aes_gcm_tag` 元数据时失败；篡改 tag、密文或使用错误主密钥时，AES-GCM 认证失败，错误包含 `wrong master key`。

当前 Rust 单元测试覆盖标准 AES-256-GCM 向量、往返加解密、tag 篡改与 tag 缺失；它没有直接覆盖构造阶段的文件长度、换行、非法 hex、文件不存在或随机源失败分支。扩展构造逻辑时应在独立的 `file_backend_test.rs` 增加这些回归用例，而不是把测试写入生产文件。

## 并发与资源生命周期

构造函数同步读取文件并在返回前关闭由 `std::fs::read` 内部管理的文件资源；本类型不启动任务、不持有通道、不创建锁，也没有异步操作。每次 `Encrypt` 都独立生成 IV，每次 `Decrypt` 都只读取后端密钥和输入内容。

`Close` 为空操作，与 Go 版本一致地表示没有需要显式关闭的句柄。不过，这也意味着本文件不主动擦除 `memCache` 中的密钥；密钥的最终释放行为取决于 `PlainKey` 的实现和 `FileBackend` 被丢弃的时机。源码没有提供热重载、密钥轮换监听或内存清零保证，文档和调用方不能假定这些能力已经存在。

本文件的方法没有内部可变状态，但是否跨线程共享仍应由 Rust 类型系统及外围所有权结构决定；当前代码没有自行包装 `Arc` 或声明额外的同步协议。多主密钥后端按顺序同步尝试各后端，不会并行调用本类型。

## 与 Go 版本的对应关系

Rust 的 `AesGcmKeyLen`、`FileBackend.memCache`、`createFileBackend`、`Encrypt`、`Decrypt` 和 `Close` 逐项对应 `file_backend.go`。文件格式、检查顺序、AES-256-GCM 内存后端构造以及错误语境均保持同一意图；Rust 测试也复用了 Go 测试中的密钥、明文、密文和 IV 向量。

主要实现差异如下：

- Go 返回 `*FileBackend` 并在字段中持有 `*MemAesGcmBackend`；Rust 按值返回并按值持有内存后端，由所有权管理生命周期。
- Go 的 `Encrypt`/`Decrypt` 接受 `context.Context` 并把它传给内存后端；Rust 方法没有 context 参数。当前内存 AES-GCM 路径是同步计算，没有取消检查，因此 Rust 不提供 Go 签名层面的取消传播能力。
- Go 使用 `encoding/hex.DecodeString`；Rust 私有 `decode_hex` 先要求 UTF-8，再用 `from_str_radix` 按字节对解析。对规定的 64 字符 ASCII hex 文件，两者行为等价。
- Go 的 `Close` 和 Rust 的 `Close` 都不执行动作；Rust 的统一 `Backend` trait 只暴露解密与关闭，`FileBackend::Encrypt` 是额外的固有方法。
- Go 使用生成的 `encryptionpb.EncryptedContent`；Rust 使用 `pb.rs` 的轻量镜像，该镜像不负责 protobuf 线格式编解码。

## 扩展指南

若要改变密钥文件格式，应优先修改 `createFileBackend`，并同步评估长度检查、换行策略、`decode_hex` 与 Go `file_backend.go` 的兼容性。宽松接受空白或 CRLF 会改变既有错误边界，不应作为无意的“容错优化”。对应回归测试应放在 `file_backend_test.rs`，Go 语义也应由 `file_backend_test.go` 对照确认。

若要新增加密算法或元数据，职责通常应落在 `mem_backend.rs` 和 `common.rs`，而本文件只选择/构造相应内存后端；同时更新 `EncryptedContent` 元数据兼容策略及标准向量测试。不能只让新版本成功解密自己的输出，还要验证旧备份、错误密钥、缺失字段和篡改内容。

若要支持密钥热轮换，需要显式设计文件变更检测、原子后端替换、并发读写和旧密钥回退，而不是在现有 `Encrypt`/`Decrypt` 中临时重复读文件。现有对象不保存路径，`MultiMasterKeyBackend` 已提供“多个既定后端顺序尝试”的静态回退机制，可作为兼容性参照。

安全扩展时还应关注：密钥文件权限与符号链接策略、密钥字节的内存清零、错误文本是否泄露敏感内容、随机 IV 的唯一性，以及 `Close` 是否需要演化为真正的清理动作。若改变统一生命周期接口，需同步 `master_key.rs::Backend`、`AnyBackend` 分派和多后端关闭逻辑。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter br/pkg/encryption/master_key` 确认目标及相邻文件入图；`query FileBackend --kind struct`、`query createFileBackend --kind function`、`query decode_hex --kind function` 定位真实符号；`node --file` 读取目标、模块入口、下游实现及测试。`explore` 识别出 `createFileBackend`、`Encrypt`、`Decrypt` 的测试调用关系，并将目标文件关联到 `file_backend_test.rs` 与 `multi_master_key_backend_test.rs`。
- 生产源码：`file_backend.rs`、`lib.rs`、`master_key.rs`、`multi_master_key_backend.rs`、`mem_backend.rs`、`common.rs`、`pb.rs`。
- crate/构建边界：`br/pkg/encryption/master_key/Cargo.toml` 与 `BUILD.bazel`。
- Go 对照：`file_backend.go`，并以 `file_backend_test.go` 核对测试向量、认证错误和清理方式。
- Rust 独立测试：`file_backend_test.rs` 验证标准向量、往返、tag 篡改和缺失；`multi_master_key_backend_test.rs` 验证两个真实文件密钥的顺序回退解密。
- 本任务是纯文档分析，未运行 Cargo；交付检查仅执行任务指定的 11 章节结构验证，并人工复核本文件没有宣称热重载、上下文取消、内存清零等源码未实现能力。
