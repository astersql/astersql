# `pkg/store/mockstore/unistore/tikv/kverrors/errors.rs` 逻辑说明

## 文件定位

本文件是 `astersql-store-mockstore-unistore-tikv-kverrors` crate 的业务实现，定义 UniStore mock TiKV 在 MVCC、事务和锁处理过程中使用的结构化错误。crate 入口 `pkg/store/mockstore/unistore/tikv/kverrors/lib.rs` 通过 `#[path = "errors.rs"] pub mod kverrors` 装载本文件，再用 `pub use kverrors::*` 将其公开；仓库根门面 `pkg/lib.rs` 也在 `store::mockstore::unistore::tikv::kverrors` 下再导出该 crate。

crate 边界由 `pkg/store/mockstore/unistore/tikv/kverrors/Cargo.toml` 确定：直接依赖 `hex` 做键编码，依赖带固定 tag 的 `kvproto` 取得 `deadlockpb`/`kvrpcpb` 类型，并通过相邻 `mvcc` crate 取得 `mvcc::Lock`。它不负责检测冲突、操作存储或组装 RPC 响应，而是保存错误载荷并提供稳定的人类可读文本。

当前 Rust 接线不是 Go 主链的完整复刻。可确认的生产调用是 `pkg/store/mockstore/unistore/tikv/dbreader/db_reader.rs::CheckWriteItemForRcCheckTSRead` 构造 `ErrConflict`；其他错误类型均已公开且有本 crate 的迁移单测，但仓库搜索未发现它们在 Rust MVCC/server 主路径中的有效生产构造。`pkg/store/mockstore/unistore/tikv/mvcc_test.rs` 内提到若干类型的内容目前是注释化迁移草稿，不能当作已接线测试。

## 核心职责

1. 用带字段的 Rust 类型表达锁冲突、重试提示、非法操作、已提交、键已存在、死锁、写冲突、提交时间戳过期、事务不存在、断言失败和 primary 不匹配。
2. 保留 Go `pkg/store/mockstore/unistore/tikv/kverrors/errors.go` 的错误字符串协议，例如键使用小写十六进制、固定错误使用固定短句、可重试错误统一加 `retryable: ` 前缀。
3. 通过 `impl_std_error!` 为每种错误统一实现 `fmt::Display` 与 `std::error::Error`，使这些 Go 风格 `Error()` 类型可进入标准 Rust 错误链。
4. 用 `BuildLockErr` 封装 `ErrLocked` 的构造，并用三个惰性全局值提供 Go 变量块对应的可重试错误。

本文件不包含错误到 `kvrpcpb::KeyError` 的映射、重试决策、锁清理或死锁检测算法；这些应由上游调用者完成。Go 对照中的映射入口是 `pkg/store/mockstore/unistore/tikv/server.go`，当前 Rust 文件本身没有对应转换逻辑。

## 主要符号

- `impl_std_error!`：内部宏。对传入类型生成 `Display::fmt`，其实现调用该类型的 `Error()`；同时实现无额外方法的 `std::error::Error`。因此新增错误类型若要遵循本文件惯例，必须先提供 `Error(&self) -> String`。
- `ErrLocked { Key, Lock }` 与 `BuildLockErr`：保存被锁键和完整 `mvcc::Lock`。`Error()` 输出 hex key 和 `Lock::String()`，构造器返回 `Box<ErrLocked>`，对应 Go 的指针返回值。
- `ErrRetryable(Cow<'static, str>)`：可借用静态字符串或拥有动态 `String`；分别由 `From<&'static str>` 和 `From<String>` 构造。`ErrLockNotFound`、`ErrAlreadyRollback`、`ErrReplaced` 是 `LazyLock<ErrRetryable>` 静态值。
- `ErrInvalidOp { Op }`：携带 `kvrpcpb::Op`，文本为 `invalid op: <枚举调试名>`；手写 `Default` 选择 `Op::Put`。
- `ErrAlreadyCommitted(u64)`：元组字段保留 commit timestamp，但错误文本固定为 `txn already committed`。
- `ErrKeyAlreadyExists { Key }`：保留冲突键，文本固定为 `key already exists`。
- `ErrDeadlock { LockKey, LockTS, DeadlockKeyHash, WaitChain }`：保存死锁相关键、持锁事务时间戳、闭环边 key hash 和 `Vec<deadlockpb::WaitForEntry>` 等待链；文本固定为 `deadlock`。
- `ErrConflict { StartTS, ConflictTS, ConflictCommitTS, Key, Reason }`：描述读写版本冲突；`Reason` 是 `kvrpcpb::WriteConflictReason`，默认值为 `Unknown`，文本固定为 `write conflict`。
- `ErrCommitExpire { StartTs, CommitTs, MinCommitTs, Key }`：表示请求提交时间戳低于锁的最小提交时间戳，文本固定为 `commit expired`。
- `ErrTxnNotFound { StartTS, PrimaryKey }`：表示存储中找不到指定事务信息，文本固定为 `txn not found`。
- `ErrAssertionFailed { StartTS, Key, Assertion, ExistingStartTS, ExistingCommitTS }`：保留失败断言和已存在版本的时间戳；`Error()` 输出全部字段，其中 key 为 hex，断言使用枚举调试名。
- `ErrPrimaryMismatch { Key, Lock }`：表示 `CheckTxnStatus` 命中了 secondary lock；文本包含 hex key 和完整锁的 `String()`。

所有上述错误类型和构造器均为公开 API；宏仅在本文件内部可见。文件没有条件编译项，测试模块的 `#[cfg(test)]` 位于 `lib.rs`。

## 执行流程

本文件没有统一调度入口；典型流程由生产者、错误值和消费者三段组成：

1. 上游 MVCC/读取逻辑发现条件不满足，并把判断现场编码到相应结构体字段。例如 `db_reader.rs::CheckWriteItemForRcCheckTSRead` 在 `RcCheckTS` 开启且条目 `CommitTS() > readTS` 时，构造 `ErrConflict`，分别写入读取时间戳、冲突版本 start/commit timestamp，并把 `Reason` 设为 `RcCheckTs`。
2. 错误通过调用者自己的结果类型传播。上述生产路径把它包装为 `DbReaderError::Conflict`；`DbReaderError::Display` 再委托给 `ErrConflict` 的 `Display`。
3. 当日志、标准错误包装或 RPC 转换需要文本时，宏生成的 `Display::fmt` 调用具体类型的 `Error()`。详细错误执行 hex/锁字符串格式化；固定错误只返回协议短句。

锁错误的局部流程是 `BuildLockErr(key, lock)` 获取 `Vec<u8>` 与 `Box<mvcc::Lock>` 的所有权，创建并装箱 `ErrLocked`；格式化时只借用载荷，不修改它。静态可重试错误则在首次解引用对应 `LazyLock` 时构造一次，之后共享不可变实例。

## 数据与状态

错误值均是普通拥有型数据：键、primary 和等待链使用 `Vec`，锁用 `Box<mvcc::Lock>` 独占，时间戳与 hash 使用 `u64`，protobuf 枚举和值直接嵌入结构体。类型大多派生 `Clone`、`Debug`、`Default` 和相等比较；包含 protobuf 等待链的 `ErrDeadlock` 未派生 `Eq`，`ErrAlreadyCommitted` 等无堆字段类型额外派生 `Copy`/`Hash`。

`ErrRetryable` 的 `Cow<'static, str>` 是唯一带借用语义的字段：静态消息不分配，运行时 `String` 则转为 owned。三个预定义错误使用 `std::sync::LazyLock`，其初始化状态由标准库同步管理。

`Default` 只用于提供可构造零值，不表示业务上有效：`ErrInvalidOp::default()` 把操作设为 `Put`，`ErrConflict::default()` 把原因设为 `Unknown`，`ErrAssertionFailed::default()` 把断言设为 `None`。调用者在真正报告错误时应填充现场字段，不应把这些默认值误作检测结果。

## 依赖与调用关系

下游依赖如下：

- `hex::encode`：被 `ErrLocked::Error`、`ErrAssertionFailed::Error`、`ErrPrimaryMismatch::Error` 使用，确保二进制键以小写 hex 显示而不是尝试 UTF-8 解码。
- `mvcc::Lock`：由 `ErrLocked` 和 `ErrPrimaryMismatch` 持有，二者格式化都调用 `Lock::String()`；该类型经 `kverrors/lib.rs` 从相邻 MVCC crate 再导出。
- `kvrpcpb::{Op, WriteConflictReason, Assertion}`：分别约束非法操作、写冲突原因和断言失败语义。
- `deadlockpb::WaitForEntry`：构成 `ErrDeadlock::WaitChain`。
- `std::{borrow::Cow, fmt, sync::LazyLock}`：分别实现低分配消息、标准格式化/错误接口和线程安全的惰性全局值。

RustCodeGraph `files` 将 `errors.rs`、`errors.go`、`lib.rs`、`migration_aster_unit_test.rs` 均识别为该目录的索引文件，`query` 精确定位了 `BuildLockErr` 及 11 个错误类型。图的宽泛 `explore` 也识别出 `Display::fmt -> Error()` 与 `From -> ErrRetryable` 的文件内边；由于 `Error`、`default`、`from` 名称高度重复，调用结果存在跨仓库同名噪声，不能用作生产接线结论。

上游方面，仓库限定搜索确认：

- `pkg/store/mockstore/unistore/tikv/dbreader/db_reader.rs` 的 `DbReaderError::Conflict` 保存 `ErrConflict`，`CheckWriteItemForRcCheckTSRead` 是当前明确的生产构造点。
- `pkg/store/mockstore/unistore/tikv/dbreader/lib.rs` 把依赖 crate 的 `kverrors::*` 放入本 crate 期望的模块路径；其 `Cargo.toml` 以 `kverrors-crate` 别名依赖本 crate。
- `pkg/store/mockstore/unistore/tikv/Cargo.toml` 在 Windows 目标依赖区声明本 crate；`pkg/lib.rs` 通过 facade 再导出。
- `migration_aster_unit_test.rs` 是覆盖全部错误类型与格式的直接 Rust 测试消费者。

## 错误处理与边界

这些类型本身就是错误边界，不会捕获或转换下游错误。`Error()` 全部是不可失败的字符串生成接口；分配失败之外没有返回错误分支。固定文本有利于协议对齐，但具体诊断字段必须从结构体载荷或后续 protobuf 映射取得，不能只解析文本。

重要边界条件如下：

- 二进制 key 可为空或包含任意字节；详细文本始终使用 hex。迁移测试用 `00abff`、`00ff`、`1234` 覆盖前导零和非 UTF-8 字节。
- `ErrLocked`/`ErrPrimaryMismatch` 要求存在一个 `Box<Lock>`，不像 Go 指针那样可为 `nil`；这消除了格式化时的空指针状态。
- `ErrDeadlock::WaitChain` 可为空，也可携带完整条目；错误文本不暴露链内容，消费者需要读取字段。
- `ErrAlreadyCommitted` 的 commit timestamp、`ErrKeyAlreadyExists::Key` 等载荷不会进入固定文本，不能根据 `Display` 恢复。
- 三个 `Default` 实现是 Rust 便利接口；`Put`、`Unknown`、`None` 并不是从业务检查推导出的错误原因。
- `impl_std_error!` 没有覆盖 `source()`，所以这些错误均是错误链叶节点；若未来包裹底层错误，需要显式实现而不能继续无条件套用现有宏。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁守卫、事务或 I/O 资源。每个错误值随 Rust 所有权正常移动/克隆并在离开作用域时释放；`Box<mvcc::Lock>` 与其中的向量随错误一起释放，`Cow::Owned` 的字符串亦然。

三个 `LazyLock<ErrRetryable>` 是进程生命周期全局值，初始化只发生一次且由 `std::sync::LazyLock` 保证并发安全；初始化闭包只构造静态借用字符串，不执行 I/O，也没有可见副作用。其他类型是否可跨线程由其字段的 `Send`/`Sync` 自动推导，本文件没有手写不安全实现或额外同步约束。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/store/mockstore/unistore/tikv/kverrors/errors.go` 的类型、字段与 `Error()` 文案，`pkg/store/mockstore/unistore/tikv/kverrors/Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向该 Go 包。主要差异是语言适配，而不是业务语义删减：

- Go 的 `*mvcc.Lock`/`*ErrLocked` 对应 Rust 的 `Box<mvcc::Lock>`/`Box<ErrLocked>`；Rust 因此没有 nil lock 状态。
- Go 的字符串别名 `ErrRetryable` 对应 `Cow<'static, str>`，并增加 `From` 实现；Go 包级变量对应 `LazyLock`。
- Go 的 `[]*deadlockpb.WaitForEntry` 对应 Rust 的 `Vec<deadlockpb::WaitForEntry>`，Rust 向量元素不是可空指针。
- Go protobuf 枚举 `WriteConflict_Reason` 对应 Rust 生成类型 `WriteConflictReason`；`Op.String()`/`Assertion.String()` 在 Rust 中以 `Debug` 格式输出。迁移单测验证当前枚举输出为 `Del`、`Exist`，与 Go 文本一致。
- Rust 为 `ErrInvalidOp`、`ErrConflict`、`ErrAssertionFailed` 明确选择 protobuf 默认枚举，并为类型派生 traits；这是 Rust 构造/测试便利，不改变错误触发规则。
- Go 生产链在 `mvcc.go`、`detector.go`、`server.go` 中构造并映射全部错误；当前 Rust 搜索仅确认 DBReader 的 `ErrConflict` 生产接线。故本文档只把其他类型描述为“已定义并经局部迁移单测验证”，不宣称 Rust 已覆盖 Go 全部触发链。

Go 行为测试 `pkg/store/mockstore/unistore/tikv/mvcc_test.go` 进一步证明字段用途：锁冲突和已存在键做类型断言，rollback 场景匹配 `ErrAlreadyRollback`，secondary lock 场景检查 `ErrPrimaryMismatch`，断言测试核对五个载荷字段，RC CheckTS 测试核对冲突时间戳。Rust 的直接迁移测试覆盖文本和载荷，但上述完整 MVCC 行为场景在 `mvcc_test.rs` 中仍是注释内容。

## 扩展指南

新增或修改错误时应按以下接入点同步处理：

1. 在本文件新增/修改拥有型载荷与 `Error()`；若它仍是错误链叶节点，可复用 `impl_std_error!`，否则应手写 `std::error::Error::source`。涉及 key 文本时继续使用 `hex::encode`，避免二进制键格式漂移。
2. 同步 Go 对照 `errors.go` 的字段和文本协议。protobuf 字段应使用 `kvproto` 当前生成类型，不要自建重复枚举。
3. 在 `migration_aster_unit_test.rs` 增加独立单测，至少覆盖载荷保存、精确文本和 `dyn std::error::Error` 的 `Display`。不要把测试嵌入生产 `errors.rs`。
4. 若新增真实触发场景，在拥有该判断的 crate 中构造错误，并在 RPC/上层错误转换处补映射；不能仅定义类型或让编译通过就声称 Go 行为已迁移。MVCC 行为测试应放在相邻独立测试文件，并覆盖成功边界与失败字段。
5. 若新增依赖，更新本 crate `Cargo.toml`；若变更公开类型，还需检查 `dbreader`、Windows 目标的 `tikv` crate 和根 facade 的兼容性。

兼容风险主要是错误字符串、protobuf 枚举命名、字段类型/所有权和公开结构体布局的变化；性能风险主要来自在热错误路径克隆较大的 key、lock 或 wait chain，以及每次 `Display` 分配新 `String`。保持 `Cow` 的静态借用和按需格式化可避免不必要分配。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/store/mockstore/unistore/tikv/kverrors` 列出本目录 4 个源码/测试文件；对 `BuildLockErr`、`ErrLocked`、`ErrRetryable`、`ErrInvalidOp`、`ErrAlreadyCommitted`、`ErrKeyAlreadyExists`、`ErrDeadlock`、`ErrConflict`、`ErrCommitExpire`、`ErrTxnNotFound`、`ErrAssertionFailed`、`ErrPrimaryMismatch` 的 `query --json` 均定位到本文件。精确 `callers` 在 `BuildLockErr` 上长时间无输出后被终止，因此生产引用以限定路径的 `rg` 结果核对。
- 源与 crate：`pkg/store/mockstore/unistore/tikv/kverrors/errors.rs`、`lib.rs`、`Cargo.toml`；另核对 `pkg/store/mockstore/unistore/tikv/Cargo.toml`、`pkg/store/mockstore/unistore/tikv/dbreader/Cargo.toml`、`dbreader/lib.rs` 与根 `pkg/lib.rs` 的依赖和再导出关系。
- 生产调用边：`pkg/store/mockstore/unistore/tikv/dbreader/db_reader.rs::CheckWriteItemForRcCheckTSRead -> kverrors::ErrConflict -> DbReaderError::Conflict -> Display`。
- Go 对照：`pkg/store/mockstore/unistore/tikv/kverrors/errors.go`；生产触发与转换引用由 `mvcc.go`、`detector.go`、`deadlock.go`、`server.go` 的限定搜索确认。
- 测试：`pkg/store/mockstore/unistore/tikv/kverrors/migration_aster_unit_test.rs` 直接覆盖全部类型的载荷/文本/标准错误接口；`pkg/store/mockstore/unistore/tikv/mvcc_test.go` 提供完整行为边界；`pkg/store/mockstore/unistore/tikv/mvcc_test.rs` 的相关内容为注释化迁移草稿，未作为有效 Rust 测试证据。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务文件指定的 11 章节检查命令；人工复核重点是区分定义、局部单测与真实生产接线，且本文未把 Go 侧完整行为臆测为 Rust 已支持。
