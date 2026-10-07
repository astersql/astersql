# `pkg/kv/error.rs`

## 文件定位

`error.rs` 是 `astersql-kv` crate 的公共错误目录。`pkg/kv/lib.rs:507-512` 在 `error` 模块中通过 `include!("error.rs")` 纳入本文件，随后以 `pub use error::*` 将全部公开项提升到 crate 根，因此其他模块通常以 `kv::ErrNotExist`、`kv::IsTxnRetryableError` 等名字使用它们。

本文件不负责访问存储、执行事务或实现重试循环；它统一定义 KV/事务边界上的标准错误原型、错误分类函数和重复键错误生成器。真正消费这些语义的代码位于调用方，例如 `pkg/kv/txn.rs:154-203` 的内部事务重试循环，以及 `pkg/kv/utils.rs:23-53` 的缺失键处理。

crate 边界由 `pkg/kv/Cargo.toml` 确认：包名为 `astersql-kv`，库入口为 `lib.rs`；错误模板直接依赖同 crate 导入的 `dbterror_dependency`、`parser-mysql` 和 errno 定义。`nextgen` feature 不对本文件做条件编译，本文件行为在默认与 nextgen 配置下相同。

## 核心职责

1. 以 `errors::SharedError` 作为 KV 层共享错误类型别名 `Error`（`error.rs:24-25`），让调用方使用统一的可共享错误载体。
2. 用 `LazyLock<Box<errors::Error>>` 惰性建立 13 个标准错误原型（`error.rs:31-96`），使错误码、错误类别和协议消息来自统一的 errno/dbterror 表，而非各调用点自行拼接。
3. 为 `ErrTxnRetryable`、`ErrWriteConflict`、`ErrWriteConflictInTiDB` 的消息附加兼容标记 `TxnRetryableMark`，并由 `IsTxnRetryableError` 精确识别这三类错误（`error.rs:27-43,67-85,98-107`）。
4. 用 `IsErrNotFound` 将任意可转换为可选共享错误引用的输入与 `ErrNotExist` 原型比较（`error.rs:109-112`）。
5. 用 `GenKeyExistsErr` 按 MySQL/TiDB 格式连接冲突列值，再由 `ErrKeyExists` 生成包含索引名的具体错误（`error.rs:114-117`）。

## 主要符号

- `pub type Error = errors::SharedError`：本模块的共享错误别名；它不是新的错误类型，也不改变底层错误的 code/class/message。
- `pub const TxnRetryableMark: &str = "[try again later]"`：对外可见的可重试消息后缀。Go 对照文件明确警告修改该字符串会影响向后兼容性（`pkg/kv/error.go:25-27`）。
- KV 类原型：`ErrNotExist`、`ErrTxnRetryable`、`ErrCannotSetNilValue`、`ErrInvalidTxn`、`ErrTxnTooLarge`、`ErrEntryTooLarge`、`ErrKeyTooLarge`、`ErrKeyExists`、`ErrNotImplemented`、`ErrWriteConflict`、`ErrWriteConflictInTiDB`。它们通过 `dbterror::ClassKV.NewStd` 或 `NewStdErr` 建立（`error.rs:31-85`）。
- TiKV 类原型：`ErrSharedLockLost`、`ErrLockExpire`、`ErrAssertionFailed`。它们通过 `dbterror::ClassTiKV.NewStd` 建立，表示更贴近存储节点/锁语义的错误（`error.rs:87-96`）。
- `IsTxnRetryableError(Option<&errors::SharedError>) -> bool`：`None` 返回 `false`；其余输入依次用三个可重试原型的 `Equal` 做类别匹配（`error.rs:99-107`）。
- `IsErrNotFound<'a>(impl Into<Option<&'a errors::SharedError>>) -> bool`：接受 `&SharedError` 或 `Option<&SharedError>` 等可转换输入，仅判断是否等价于 `ErrNotExist`（`error.rs:110-112`）。
- `GenKeyExistsErr(&[String], &str) -> errors::SharedError`：以 `-` 连接列值，并把连接结果和索引名作为两个模板参数传给 `ErrKeyExists.FastGenByArgs`（`error.rs:115-117`）。

所有错误原型都是公开静态值；本文件没有私有函数、trait、struct、enum、宏或条件编译项。

## 执行流程

错误原型首次被解引用时，`LazyLock` 执行闭包：普通原型直接用对应 errno 调用 `NewStd`；三个自定义可重试原型先从 `errno::MySQLErrName` 取 `Raw` 模板及 `RedactArgPos`，构造 `parser_mysql::Message`，再以 `NewStdErr` 注册同一错误码和修改后的消息（`error.rs:31-96`）。之后访问复用已初始化的原型。

事务重试路径的代表性流程是：`pkg/kv/txn.rs:154-203` 创建事务并运行回调；回调或提交失败时，只有调用参数允许重试且 `IsTxnRetryableError` 返回 `true`，循环才记录错误并进入下一次尝试。提交错误路径还会执行 `BackOff`。因此本文件提供分类事实，但不决定重试次数、退避或回滚生命周期。

缺失键路径的代表性流程是：`pkg/kv/utils.rs:23-53` 调用 `Get`，以 `IsErrNotFound` 区分正常未命中与其他失败；`IncInt64` 在未命中时写入初值，`GetInt64` 在未命中时返回 0，非 NotFound 错误继续向上传播。

生成重复键错误时，`GenKeyExistsErr` 先对 `keyCols` 做 `join("-")`，再调用 `FastGenByArgs`。空切片和单个空字符串都产生空的第一个模板参数；包含空列值或列值自身含 `-` 时不转义，只按顺序连接（由 `pkg/kv/error_test.rs:47-59` 验证）。

## 数据与状态

本文件持有的全局状态只有不可变错误原型。每个原型由 `LazyLock` 管理“一次初始化、后续共享读取”，内部值装在 `Box<errors::Error>` 中；调用方通常用 `FastGenByArgs`/`GenWithStackByArgs` 生成独立的 `SharedError`，而不是修改原型。

错误身份由 dbterror 的 class、errno code 与消息模板共同表达。普通错误直接采用 errno 表中的标准定义；可重试错误保留原 errno code 和参数脱敏位置，只修改展示模板以增加 `TxnRetryableMark`（`error.rs:35-43,68-85`）。`ErrKeyExists` 特意复用 `errno::ErrDupEntry`（`error.rs:60-62`）。

该文件没有可变静态变量、缓存淘汰、事务句柄、锁所有权记录、通道或异步任务。事务 start TS、重试计数和最后一次错误等运行状态由 `pkg/kv/txn.rs` 等调用方维护。

## 依赖与调用关系

下游依赖如下：

- `std::sync::LazyLock`：线程安全的惰性一次初始化。
- `errors`：由 `pkg/kv/lib.rs` 再导出的 dbterror 错误 API，提供 `SharedError`、`Error`、`Equal`、`FastGenByArgs` 等能力（`lib.rs:85-86`）。
- `dbterror`：由 `pkg/kv/lib.rs` 再导出的错误类别，使用 `ClassKV` 和 `ClassTiKV` 建立标准原型（`lib.rs:107`）。
- `errno` 与 `parser_mysql`：提供错误码、MySQL 消息模板、脱敏参数位置和 `Message` 结构（`lib.rs:111`；`pkg/kv/Cargo.toml` 的 `parser-mysql` 依赖）。

上游调用覆盖 KV 到 SQL 主链：`pkg/store/driver/kv_adapter.rs` 将存储未命中/空值写入映射到 `ErrNotExist`、`ErrCannotSetNilValue`；`pkg/store/driver/error/error.rs:202-215` 将 client 错误映射为本模块原型；`pkg/kv/txn.rs:172,196` 用可重试分类驱动内部事务循环；`pkg/structure/{hash,string,list}.rs`、`pkg/meta/reader.rs` 和多个 session/runtime 文件把 NotFound 当作可恢复的“值不存在”分支。RustCodeGraph 的目标文件节点报告该文件被 33 个文件直接使用；定向引用搜索进一步确认上述符号级调用点。

`GenKeyExistsErr` 在本文件之外的 Rust 生产引用当前主要由更高层的 DDL 同名辅助函数承担；本函数自身的格式契约由独立测试直接覆盖。不要把 `pkg/ddl/util/util.rs::GenKeyExistsErr` 与本函数混为同一符号：前者负责 DDL/keyspace 上下文，后者只负责 KV 重复键消息参数。

## 错误处理与边界

- `IsTxnRetryableError(None)` 与 `IsErrNotFound(None)` 均为 `false`，不会把“无错误”误判成分类成员（`error.rs:99-112`；`error_test.rs:20-27,30-43`）。
- 可重试集合是封闭的三项枚举式判断：锁过期、断言失败、NotFound、InvalidTxn 等即使来自事务/存储路径，也不会被本函数判定为安全重试（`error_test.rs:33-43`）。新增可重试错误不会自动生效，必须显式加入判定并同步调用方预期。
- `Equal` 比较错误类别/原型语义，允许识别由原型生成的具体错误，而不是依赖字符串相等；测试通过 `FastGenByArgs` 生成实例后再分类（`error_test.rs:23-27,33-43`）。
- `GenKeyExistsErr` 不验证列数、索引名是否为空，也不对连接符转义；这些输入均按模板参数原样格式化。调用方需要保证列顺序和索引名正确。
- 三个自定义模板从 `MySQLErrName` 以索引访问对应 errno。如果 errno 名表缺少条目，惰性初始化会失败；当前 errno 表在 `pkg/errno/errname.rs` 定义了所有相关键，独立测试还验证协议错误码不是 Unknown。
- `ErrSharedLockLost` 是事务致命语义，不在可重试集合内；其 SQL 错误码 9015 由 `error_test.rs:86-91` 验证。

## 并发与资源生命周期

`LazyLock` 保证多个线程首次访问同一错误原型时只初始化一次，并安全发布初始化结果；初始化后只发生共享只读访问。每个静态原型的生命周期等同于进程，既不显式释放，也不持有网络、磁盘、事务或 runtime 资源。

本文件不创建锁守护、Tokio task 或通道。名称中的 `ErrSharedLockLost`、`ErrLockExpire` 描述业务错误，而非本文件自身持有同步锁。错误实例的共享/释放遵循 `errors::SharedError` 的所有权实现；重试期间保存 `last_error`、事务回滚和退避均由 `pkg/kv/txn.rs:150-207` 管理。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/kv/error.go`：常量名、13 个错误原型、三个公开函数及 errno/class 选择一致。Rust 用 `LazyLock<Box<errors::Error>>` 模拟 Go 包级变量的延迟且线程安全初始化；Go 变量在包初始化时直接构造。Rust 的 `Option<&SharedError>`/`Into<Option<_>>` 对应 Go `error` 可为 `nil` 的输入语义。

三个可重试消息的空格规则与 Go 保持一致：`ErrTxnRetryable` 将原模板与标记直接拼接，而两个写冲突模板在标记前插入一个空格（`error.rs:35-43,68-85`；`error.go:36-79`）。`GenKeyExistsErr` 的 Rust `slice.join("-")` 对应 Go `strings.Join(keyCols, "-")`，随后都以连接值和 `keyName` 填充 duplicate-entry 模板。

现有 Rust 测试比同路径 Go 测试覆盖更多迁移契约：`pkg/kv/error_test.go` 只验证一组原型能转换为非 Unknown 且 code 一致；`pkg/kv/error_test.rs` 还验证 nil/None、分类集合、连接边界和 `ErrSharedLockLost` 的 9015 错误码。两侧共同确认协议错误码一致性，不代表所有生产调用路径都由该单元测试覆盖。

## 扩展指南

新增标准 KV/TiKV 错误时，应先确认 errno code 和 `MySQLErrName` 模板已存在，再按语义选择 `ClassKV` 或 `ClassTiKV`，在本文件增加 `LazyLock` 原型。若需要自定义消息，必须保留原模板的 `RedactArgPos`，避免日志/协议层脱敏退化。

新增或调整可重试错误时，必须同时评估：`TxnRetryableMark` 的客户端兼容性、`IsTxnRetryableError` 的白名单、`pkg/kv/txn.rs` 的回调失败与提交失败路径、`pkg/util/dbutil/retry.rs` 的错误码集合，以及存储驱动的错误映射。不可仅给消息加标记却遗漏分类，或仅加入分类而未确认事务可安全重放。

调整重复键格式时，应修改 `GenKeyExistsErr` 并同步 `pkg/kv/error_test.rs::test_key_exists_join_matches_go`，同时核对 Go `pkg/kv/error.go::GenKeyExistsErr` 与上层 `pkg/ddl/util/util.rs::GenKeyExistsErr`。连接规则或模板参数顺序的改变会影响用户可见错误文本和兼容性。

测试必须继续放在独立的 `pkg/kv/error_test.rs`，不要内嵌进生产文件。至少覆盖：新原型到 SQL error code 的映射、正反分类、`None` 边界、消息模板参数及 Go 对照行为。性能风险主要来自在热路径额外格式化字符串；静态原型应继续惰性复用，不应在每次分类时重建模板。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件且目标仓库已索引；`node --file pkg/kv/error.rs --offset 1 --limit 400` 读取目标文件 117 行完整符号，并报告 33 个直接使用文件；`query` 分别定位 Rust/Go 的 `IsTxnRetryableError`、`IsErrNotFound`、`GenKeyExistsErr`。同名跨语言符号使精确 `callers/callees` 未返回可用边，因此按技能规则用定向引用搜索补足调用证据。
- 源码与模块：`pkg/kv/error.rs`、`pkg/kv/lib.rs:85-111,507-512`、`pkg/kv/Cargo.toml`。
- 直接调用证据：`pkg/kv/txn.rs:150-207`、`pkg/kv/utils.rs:18-53`、`pkg/store/driver/error/error.rs:202-215`、`pkg/store/driver/kv_adapter.rs:362,408,859,1167`。
- Go 对照：`pkg/kv/error.go`；错误码/模板定义交叉核对 `pkg/errno/errcode.rs` 与 `pkg/errno/errname.rs`。
- 独立测试：`pkg/kv/error_test.rs`、`pkg/kv/error_test.go`；相关调用行为另见 `pkg/kv/txn_test.rs`、`pkg/kv/fault_injection_test.rs` 和 `pkg/store/mockstore/mockstorage/canonical_storage_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证本文档恰有 11 个固定二级章节，并人工复核只新增本文档、未修改 Rust/Go/Cargo/总计划。
