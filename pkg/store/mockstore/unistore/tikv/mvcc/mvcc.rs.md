# `pkg/store/mockstore/unistore/tikv/mvcc/mvcc.rs`

## 文件定位

本文件是 `astersql-store-mockstore-unistore-tikv-mvcc` crate 中的基础 MVCC 数据格式层，源码由同目录 `lib.rs` 以 `pub mod mvcc` 挂载并再次 `pub use mvcc::*` 导出。它不执行事务调度、持久化或锁等待，而是定义 mock TiKV/UniStore 在这些路径间交换的三类二进制契约：内存锁 `Lock`、版本条目的 `DBUserMeta`，以及 Rollback/`Op_Lock` 使用的额外事务状态键。

crate 边界由 `pkg/store/mockstore/unistore/tikv/mvcc/Cargo.toml` 确定；本文件直接使用本 crate 的 `codec` 适配层、带 `protobuf-codec` feature 的 `kvproto`、`protobuf` 枚举转换和 `hex`。工作区根 `Cargo.toml` 将该 crate 注册为 `facade_store_mockstore_unistore_tikv_mvcc`，`pkg/lib.rs` 再把它放入总 facade。直接依赖还见于 `tikv/dbreader`、`tikv/kverrors`、`tikv/server`、父级 `tikv` 和可选的 `cophandler` Cargo 声明。

## 核心职责

1. `LockHdr`、`Lock`、`DecodeLock` 与 `Lock::MarshalBinary` 固定锁存储格式：40 字节小端锁头，随后依次为 primary、带 `u16` 小端长度的 async-commit secondaries、value。
2. `Lock::ToLockInfo` 把内部锁映射为 RPC 层 `kvproto::kvrpcpb::LockInfo`；`Lock::String` 提供与 Go 实现对齐的诊断文本。
3. `DBUserMeta` 以 16 字节小端数据携带 `startTS` 和 `commitTS`，供版本读取、冲突检查与 MVCC 信息查询解释存储引擎的 user meta。
4. `DecodeKeyTS`、`EncodeExtraTxnStatusKey`、`DecodeExtraTxnStatusKey` 维护降序时间戳键和额外事务状态命名空间；`LockUserMetaNone`/`LockUserMetaDelete` 则定义锁存储 user meta 的状态字节。

该文件的职责是“格式与转换”，不判断锁可见性、提交合法性、回滚规则或 GC 条件。那些策略由调用方实现；例如 Rust 的 `dbreader/db_reader.rs` 使用 `DBUserMeta::{StartTS, CommitTS}` 构造 `MvccWrite` 并执行 RC CheckTS 冲突判断，相邻 `tikv.rs::EncodeLockCFValue` 消费 `Lock` 生成 TiKV Lock CF 格式。

## 主要符号

- `defaultEndian: &str`：文档性公开常量，值为 `binary.LittleEndian`；真正的 Rust 编解码通过 `to_le_bytes`/`from_le_bytes` 完成，而不是通过该字符串分派。
- `DBUserMeta(pub Vec<u8>)`：公开元组结构，预期载荷恰为 16 字节。`NewDBUserMeta(startTS, commitTS)` 写入两个连续的小端 `u64`；`StartTS()` 读取 `[0..8]`，`CommitTS()` 读取 `[8..16]`。
- `LockHdr`：公开固定头字段集合。时间戳为 `StartTS`、`ForUpdateTS`、`MinCommitTS`，另含 `TTL`、RPC 操作码 `Op`、旧版本标记、primary 长度、async-commit 标记和 secondary 数量。
- `mvccLockHdrSize: usize = 40`：线格式常量。`#[repr(C)]` 表达与 Go 结构布局的对应关系，但实际持久化由 `from_go_bytes`/`write_go_bytes` 明确逐字段编解码，不依赖 Rust 原生内存布局。
- `LockHdr::from_go_bytes`、`LockHdr::write_go_bytes`：私有线格式边界。字节偏移依次为三个 `u64`（0、8、16）、`TTL`（24）、`Op`（28）、`HasOldVer`（29）、`PrimaryLen`（30）、`UseAsyncCommit`（32）、3 字节填充（33..36）、`SecondaryNum`（36）。
- `Lock`：公开拥有型锁对象，包含 `LockHdr`、`Primary: Vec<u8>`、`Value: Vec<u8>` 和 `Secondaries: Vec<Vec<u8>>`。
- `DecodeLock(data) -> Lock`：解析固定头，再按头中的长度字段复制所有可变数据。
- `Lock::MarshalBinary() -> Vec<u8>`：执行逆向编码并返回拥有型缓冲。
- `Lock::ToLockInfo(key) -> LockInfo`：复制 key、primary 和 secondaries，映射锁版本、TTL、操作类型、for-update/min-commit 时间戳与 async-commit 标记。
- `Lock::String() -> String`：输出操作类型、开始/for-update 时间戳、十六进制 primary 和 async-commit 标记。
- `LockUserMetaNoneByte`/`LockUserMetaDeleteByte` 及对应切片：状态值分别为 0 和 2。
- `DecodeKeyTS(buf) -> u64`：从键尾 8 字节调用 `codec::DecodeUintDesc` 还原降序编码时间戳。
- `EncodeExtraTxnStatusKey(key, startTS) -> Vec<u8>`：复制 key、追加降序时间戳，再把首字节加一以进入额外状态命名空间。
- `DecodeExtraTxnStatusKey(extraKey) -> Option<Vec<u8>>`：长度大于 9 时去掉末尾时间戳并把首字节减一；否则返回 `None`。

## 执行流程

锁写入流程从一个已构造的 `Lock` 开始。`MarshalBinary` 先按 primary/value 和每个 secondary 的“2 字节长度 + 内容”计算缓冲区，调用 `LockHdr::write_go_bytes` 写 40 字节头，再顺序写 primary、secondaries 和 value。`migration_aster_unit_test.rs::lock_binary_round_trip_matches_go_layout_and_copies_variable_fields` 逐偏移断言该布局，包括 33..36 的三个零填充字节。

锁读取时，`DecodeLock` 先调用 `LockHdr::from_go_bytes`，以 `PrimaryLen` 切出 primary；当 `SecondaryNum > 0` 时循环读取 `u16` 小端长度与 secondary 内容；余下字节全部作为 value。返回值拥有每段数据，因此后续修改不会影响输入缓冲。读取后的锁可以经 `ToLockInfo` 进入 RPC 错误/锁扫描响应，也可由相邻 `tikv.rs::EncodeLockCFValue` 转成真实 TiKV 兼容的 Lock CF 值；二者是不同的线格式，不应混用。

版本元信息流程由 `NewDBUserMeta` 写入两个时间戳，持久化层把这 16 字节作为 user meta 保存，读取路径再包装成 `DBUserMeta`。`dbreader/db_reader.rs` 的 `GetMvccInfoByKey`、`GetKeyByStartTs` 与 `CheckWriteItemForRcCheckTSRead` 分别用它枚举版本、按 start TS 查键、比较 commit TS 并构造写冲突。

额外状态键流程先由 `EncodeExtraTxnStatusKey` 对普通用户键复制并追加降序 `startTS`，再调整首字节以隔离键空间。范围扫描可通过对边界首字节做同样偏移进入这一空间；解码函数仅还原用户键，时间戳继续由 `DecodeKeyTS` 从原额外键读取。

## 数据与状态

`Lock`、`DBUserMeta` 和所有编码结果均拥有其 `Vec` 数据，没有借用输入的长期生命周期。锁头存在必须由调用者维持的交叉字段不变量：`PrimaryLen` 应等于 `Primary.len()`，`SecondaryNum` 应等于 `Secondaries.len()`；当前序列化器不会主动修正或验证它们。若 `SecondaryNum == 0`，即使 `Secondaries` 非空也不会编码；若数量非零，编码器遍历实际 vector，而解码器严格按头中数量读取。

锁格式的关键稳定状态为 40 字节头和固定偏移。布尔值编码为 0/1，解码时任何非零值都解释为 `true`；3 字节对齐区在写入时清零、读取时忽略。secondary 长度在线格式中只有 `u16`，但 Rust 端由 `usize` 强制转换为 `u16`，因此调用者应保证每个 secondary 不超过 65535 字节。

`DBUserMeta` 的公开内部 vector 允许构造任意长度值，但访问器要求至少 16 字节。额外状态键以首字节 wrapping 加减和末尾 8 字节降序时间戳为约定；空 key 无法安全编码，因为编码后首字节访问仍依赖存在用户键前缀。

## 依赖与调用关系

RustCodeGraph 对目标文件识别出 16 个符号，并确认内部边 `DecodeLock -> LockHdr::from_go_bytes` 与 `Lock::MarshalBinary -> LockHdr::write_go_bytes`。精确节点未给出生产上游函数调用边，文本检索进一步确认当前 Rust 迁移状态：

- `mvcc/lib.rs` 公开重导出本文件；`kverrors/lib.rs` 也重导出其类型。
- `dbreader/db_reader.rs` 直接构造 `DBUserMeta`，并调用 `StartTS`/`CommitTS` 解释版本和生成 `ErrConflict`；`dbreader/migration_aster_unit_test.rs` 使用 `NewDBUserMeta` 验证该读链。
- 同 crate 的 `tikv.rs::EncodeLockCFValue` 读取 `LockHdr`、`Primary`、`Value`，将操作码映射为 TiKV 的 P/D/L/S 标记并处理短值、`ForUpdateTS`、`MinCommitTS`。
- `tikv/write.rs` 的内存后端按本文件的 40 字节头、primary/value 和 secondary 长度前缀计算锁条目大小，但使用的是该模块自己的 `write::Lock` 表示，属于格式契约的间接依赖。

Go 对照显示完整应用主链的预期位置：`tikv/write.go` 调用 `MarshalBinary`、`NewDBUserMeta` 和 `EncodeExtraTxnStatusKey`；`tikv/mvcc.go` 在锁读取、事务状态、扫描和 GC 路径调用 `DecodeLock`、`ToLockInfo`、`DBUserMeta`、`DecodeKeyTS`；`tikv/server.go` 将锁转成 RPC `LockInfo`。这些 Go 调用是迁移语义依据，不应误写为已经存在的 Rust 直接调用边。

## 错误处理与边界

该 API 大部分保留 Go 的“输入已由存储格式保证有效”前置条件，并未返回结构化解析错误：

- `LockHdr::from_go_bytes` 对短于 40 字节的输入显式 `assert!`；后续 primary/secondary 切片越界或缺少长度前缀也会 panic。
- `DecodeLock` 不验证 `PrimaryLen`、`SecondaryNum` 与实际缓冲一致，也不要求 `UseAsyncCommit` 与 `SecondaryNum` 相互匹配。
- `MarshalBinary` 不验证 header 数量字段与 vectors 一致；secondary 长度超过 `u16::MAX` 会截断写入的长度字段，造成不可往返的数据。
- `ToLockInfo` 对未知 `Op` 使用 `unwrap_or_default()`，会退化为 protobuf 枚举默认值；`String` 则保留未知数值文本，两者处理未知操作码的表现不同。
- `DecodeKeyTS` 要求输入至少 8 字节；短输入切片会 panic，codec 错误也被转成 panic。
- `DBUserMeta::{StartTS, CommitTS}` 要求载荷至少 16 字节，否则切片或 `copy_from_slice` panic。
- `DecodeExtraTxnStatusKey` 把长度 `<= 9` 判为无效并返回 `None`，但不验证时间戳编码或命名空间首字节；编码函数对首字节使用 wrapping 加法，解码使用 wrapping 减法。

因此这些函数适合可信的内部存储数据。若要用于不可信输入，应在本层新增显式校验与 `Result` API，同时保留现有 Go 对齐入口的兼容行为。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、文件句柄或网络资源。所有操作都是同步的纯内存转换；并发安全取决于调用者如何共享 `Lock`/`DBUserMeta`，本文件自身不保存全局可变状态。

`DecodeLock`、`ToLockInfo`、`NewDBUserMeta` 和两个额外键函数通过复制建立独立所有权，避免输入缓冲或调用方 key 的生命周期泄漏到返回值。`MarshalBinary` 每次分配一个精确计算大小的新 vector。主要资源风险不是泄漏，而是攻击性长度字段导致大容量预分配或越界 panic；`Secondaries::with_capacity(header.SecondaryNum as usize)` 也意味着不可信头可触发过量分配。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/store/mockstore/unistore/tikv/mvcc/mvcc.go`。Rust 保留了 Go 的公开命名、字段和整体字节布局：锁头为 40 字节小端格式，primary/secondaries/value 顺序一致，user meta 是两个小端 `u64`，额外状态键通过首字节偏移隔离，RPC 字段映射和字符串模板也相同。

两版实现存在有意的语言级差异。Go 用 `unsafe.Pointer` 把头直接映射到内存，Rust 用显式偏移读取/写入并固定清零 padding，从而不依赖宿主 ABI；Go 的 `Lock` 匿名嵌入 `LockHdr`，Rust 使用具名 `LockHdr` 字段；Go 解码先整体 clone 锁体后让各切片共享该 backing array，Rust 分别分配 primary、每个 secondary 和 value；Go `ToLockInfo` 返回指针，Rust 返回拥有型值并复制字段；Go 的无效额外键返回 nil，Rust 表示为 `None`。

当前 Rust 迁移并非完整主链等价接线。独立 crate 和格式测试已经存在，`DBUserMeta` 已被 Rust `dbreader` 消费，`Lock` 已被同 crate TiKV 编码器消费；但仓库文本检索没有发现 Rust 生产代码直接调用本文件的 `DecodeLock`、`MarshalBinary`、`ToLockInfo` 或额外状态键函数。这些入口当前主要由 `mvcc/migration_aster_unit_test.rs` 验证，Go 文件仍提供完整事务主链的行为参照。

## 扩展指南

修改线格式时应首先区分“内存字段扩展”和“持久化格式扩展”。新增锁头字段需要同时更新 `LockHdr`、`mvccLockHdrSize`、`from_go_bytes`、`write_go_bytes`、`DecodeLock`/`MarshalBinary` 的偏移约定以及 Go `mvcc.go`；还应同步 `migration_aster_unit_test.rs::lock_binary_round_trip_matches_go_layout_and_copies_variable_fields`，以字节级断言兼容性。不得仅依赖 `#[repr(C)]` 推导新布局。

新增 RPC 可见字段时，应修改 `ToLockInfo` 并扩展 `lock_info_and_string_keep_go_field_mapping`；如果字段影响 TiKV Lock CF，还需同步相邻 `tikv.rs::EncodeLockCFValue` 及其独立测试。新增 user meta 字段会改变固定 16 字节契约，应审计 `dbreader/db_reader.rs` 的所有直接构造点、冲突判断及 Go 对照，优先设计带版本的兼容格式。

强化错误处理时，建议新增返回 `Result` 的严格解析入口，再让现有 Go 兼容函数决定是否继续 panic，避免静默改变调用方契约。必须覆盖短锁头、截断 primary、截断 secondary 长度/内容、数量不一致、超长 secondary、短 `DBUserMeta`、短时间戳键与未知 `Op`。Rust 单元测试应继续放在同目录独立测试文件 `migration_aster_unit_test.rs`（或新的独立 `*_test.rs`），不要内嵌到本生产文件。

性能方面，增加字段或校验时需留意 `DecodeLock` 的多次分配、`ToLockInfo` 的深复制以及由不可信 `SecondaryNum` 驱动的容量分配。兼容性方面，任何 40 字节头、padding、大小端、字段顺序或额外键首字节规则的变化都会影响已有数据和 Go/Rust 互操作。

## 验证依据

- 目标源码：`pkg/store/mockstore/unistore/tikv/mvcc/mvcc.rs`，完整读取 263 行并枚举其常量、结构、私有头编解码方法、公开函数与 impl；该文件没有条件编译项。
- RustCodeGraph：`status` 显示索引含目标文件；`files --filter pkg/store/mockstore/unistore/tikv/mvcc` 列出本 crate；`node --file .../mvcc.rs --offset 1 --limit 500` 返回完整源码；对 `DecodeLock`、`MarshalBinary`、`ToLockInfo`、`DecodeKeyTS`、`NewDBUserMeta`、`EncodeExtraTxnStatusKey`、`DecodeExtraTxnStatusKey` 的 `query` 同时定位 Rust 与 Go 定义。图中确认两条内部调用边，精确 callers 查询没有返回 Rust 生产上游，随后由 `rg` 补查未覆盖引用。
- crate 与装配：`pkg/store/mockstore/unistore/tikv/mvcc/Cargo.toml`、同目录 `lib.rs`、父级 `tikv/Cargo.toml`/`lib.rs`、`tikv/dbreader/Cargo.toml`/`lib.rs`、根 `Cargo.toml` 与 `pkg/lib.rs`。
- Rust 直接证据：`pkg/store/mockstore/unistore/tikv/dbreader/db_reader.rs`、`pkg/store/mockstore/unistore/tikv/mvcc/tikv.rs`、`pkg/store/mockstore/unistore/tikv/write.rs`。
- Go 对照与调用证据：`pkg/store/mockstore/unistore/tikv/mvcc/mvcc.go`、`pkg/store/mockstore/unistore/tikv/write.go`、`pkg/store/mockstore/unistore/tikv/mvcc.go`、`pkg/store/mockstore/unistore/tikv/server.go`、`pkg/store/mockstore/unistore/tikv/dbreader/db_reader.go`。
- 独立测试：`pkg/store/mockstore/unistore/tikv/mvcc/migration_aster_unit_test.rs` 覆盖锁头固定布局与往返复制、`LockInfo`/字符串映射、user meta、降序时间戳与额外状态键；`pkg/store/mockstore/unistore/tikv/dbreader/migration_aster_unit_test.rs` 覆盖 `NewDBUserMeta` 在读取路径中的使用。任务为纯文档分析，按计划未运行 Cargo。
