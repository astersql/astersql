# [`pkg/kv/version.rs`](version.rs)

## 文件定位

`version.rs` 属于 `astersql-kv` crate，是 KV 层表示 MVCC 版本（通常是时间戳/TSO）的基础值类型文件。crate 根 `pkg/kv/lib.rs` 通过 `#[path = "version.rs"] mod version_impl;` 装入该文件，并用 `pub use version_impl::*;` 将其公开，因此上层通常以 `astersql_kv::Version`、`astersql_kv::NewVersion`、`astersql_kv::MaxVersion` 等路径使用这些符号。

该文件不负责分配时间戳、打开快照或执行事务；它只定义版本值、两个边界哨兵、构造与比较规则，以及一个尚未接入 Rust 生产实现的版本供给接口。真正消费版本的 KV 抽象位于 `pkg/kv/kv.rs` 的 `Storage::GetSnapshot` 和 `Storage::CurrentVersion`。

## 核心职责

- 用 `Version { Ver: u64 }` 为裸 `u64` 增加 KV/MVCC 语义，供快照读取和当前版本查询在模块间传递。
- 用 `NewVersion` 保留 Go 风格的显式构造入口，便于迁移代码保持调用结构一致。
- 用 `Version::Cmp` 提供与 Go 完全一致的三路比较结果：大于为 `1`，小于为 `-1`，相等为 `0`。
- 用 `MaxVersion`、`MinVersion` 表示非有效版本的上下界哨兵。尤其 `MaxVersion` 可被存储适配层解释为“最新已提交快照”；`pkg/store/driver/kv_adapter.rs::snapshot_timestamp` 会先向当前时间戳供给方解析它，而不会把 `u64::MAX` 直接发往 TiKV。
- 保留 Go `VersionProvider` 的接口形状。当前仓库搜索只找到该 trait 的定义，未找到 Rust 生产实现或调用者；实际生产存储路径使用参数不同的 `Storage::CurrentVersion(&self, txn_scope: &str)`。

## 主要符号

- `pub trait VersionProvider`：声明 `CurrentVersion(&self) -> Result<Version, Box<dyn std::error::Error>>`。它表达“提供单调递增当前版本”的最小能力，但错误类型被装箱，且没有事务作用域参数。不要将它与 `pkg/kv/kv.rs::Storage::CurrentVersion(&self, txn_scope)` 混为一谈。
- `pub struct Version`：只有公开字段 `Ver: u64`。派生 `Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`，所以复制和相等判断不需要分配或借用；未派生 `Ord`/`PartialOrd`，排序语义由显式的 `Cmp` 提供。
- `pub const MaxVersion`：`Version { Ver: u64::MAX }`。这是哨兵而非已经分配的合法版本。
- `pub const MinVersion`：`Version { Ver: 0 }`。同样是哨兵而非合法版本。
- `pub fn NewVersion(v: u64) -> Version`：不做校验、转换或分配，原样将 `v` 写入 `Ver`。
- `pub fn Version::Cmp(&self, another: Version) -> i32`：先比较大于，再比较小于，否则返回相等。参数按值传入是安全且廉价的，因为 `Version: Copy`。

## 执行流程

典型快照读取链如下：

1. 上层取得或持有一个原始时间戳。例如 `pkg/session/runtime/relational_scan.rs` 多处将 `read_ts` 传给 `kv::NewVersion`，`pkg/domain/domain.rs::KvSchemaSource::snapshot` 也将 `ts` 包装为版本。
2. `NewVersion` 仅构造 `Version { Ver: ts }`，不检查时间戳是否合法，也不访问外部状态。
3. 调用方把 `Version` 传给 `Storage::GetSnapshot`。例如 `pkg/domain/canonical_domain.rs::KvInfoSchemaLoader::load_at` 用指定版本读取 catalog 并构建 InfoSchema。
4. 具体存储实现读取 `Ver` 并建立快照。`pkg/store/driver/kv_adapter.rs::GetSnapshot` 将其交给 `ClientSnapshot::new`；mock/故障注入实现则转发或保存同一版本。

“读取最新快照”是一个特殊分支：Session 导入路径、Domain 等调用 `GetSnapshot(kv::MaxVersion)`；存储驱动的 `snapshot_timestamp` 识别 `MaxVersion.Ver`，调用当前时间戳回调，把哨兵解析成一次确定的真实时间戳，并检查它能否进入 client-rust 的有符号时间戳范围。

比较流程完全局部：`Cmp` 对两个 `Ver` 做无符号比较，不解析 TSO 的物理/逻辑部分，也不改变任一对象。`pkg/kv/version_test.rs::test_version` 覆盖小于、大于、相等以及 `MinVersion < MaxVersion`。

## 数据与状态

本文件的唯一实例状态是 `Version::Ver`。它没有隐藏字段、全局可变变量、缓存或引用计数。`MaxVersion` 和 `MinVersion` 是编译期常量，每次使用得到同一个值语义；它们不表示分配器状态。

重要不变量与限制：

- `NewVersion(v).Ver == v`，包括 `0` 和 `u64::MAX`；构造函数不会拒绝哨兵值。
- `Cmp` 只取决于两个 `u64`，结果严格属于 `{-1, 0, 1}`。
- `Eq`/`PartialEq` 与 `Cmp == 0` 都以 `Ver` 相等为依据。
- “版本单调递增”是 `VersionProvider` 对实现方的语义要求，不由 `Version` 自身强制。
- `MaxVersion` 的“最新快照”含义是下游存储契约；若新增存储实现，不能把 `u64::MAX` 无条件当成真实 TiKV 时间戳发送。

## 依赖与调用关系

文件自身只依赖 Rust 标准库：`u64`、派生 trait 和 `std::error::Error`；`NewVersion`、`Cmp` 均无下游函数调用。`pkg/kv/Cargo.toml` 确认它属于 `astersql-kv`，本文件没有直接使用该 manifest 中的外部 crate，也不受 `nextgen` feature 条件编译控制。

模块出口由 `pkg/kv/lib.rs` 提供。直接生产使用包括：

- `pkg/domain/domain.rs::KvSchemaSource::snapshot`：把 schema 时间戳包装后取快照。
- `pkg/domain/canonical_domain.rs::KvInfoSchemaLoader::{load_info_schema,load_snapshot_info_schema}`：分别使用存储当前版本或指定版本加载 InfoSchema。
- `pkg/session/runtime/relational_scan.rs` 与 `pkg/session/runtime/explain_read.rs`：把 SQL 读取时间戳包装为 KV 版本。
- `pkg/session/runtime/import_query.rs`、`import_sst.rs` 和 `modify_column_cloud_executor.rs`：以 `MaxVersion` 请求最新快照。
- `pkg/store/driver/kv_adapter.rs`：把后端当前时间戳转换为 `kv::Version`，并处理 `MaxVersion` 的特殊语义。
- `pkg/kv/fault_injection.rs::InjectedStore`：在包装存储时原样转发 `Version` 与当前版本结果。

RustCodeGraph 的文件节点报告 `pkg/kv/version.rs` 被 28 个文件使用，并将 `pkg/domain/domain.rs::snapshot`、`pkg/kv/lib.rs` 测试夹具的 `CurrentVersion` 等列为 `NewVersion` 的调用者。由于路径限定的 `callers` 查询未返回进一步结果，直接使用点又通过上述源码搜索核验。

## 错误处理与边界

`NewVersion` 和 `Cmp` 不返回错误，也不会 panic。任何版本合法性、后端范围和网络错误都由调用方或存储实现处理。例如 `pkg/store/driver/kv_adapter.rs::snapshot_timestamp` 在非哨兵版本大于 `i64::MAX` 时返回错误；这不是 `Version` 构造阶段的限制。

`VersionProvider::CurrentVersion` 可以返回装箱动态错误，但 trait 没有规定具体错误类别、重试策略或取消语义。相比之下，生产 `Storage::CurrentVersion` 使用 crate 的共享错误类型并要求 `txn_scope`；在接线或统一接口前，不能假设二者可直接替换。

边界值 `0` 和 `u64::MAX` 明确是无效版本哨兵。`Cmp` 仍按普通数值处理它们，因此 `MinVersion.Cmp(MaxVersion) == -1`；哨兵含义不会改变比较规则。

## 并发与资源生命周期

`Version` 是无所有权资源的 `Copy` 值，不持有锁、任务、通道、事务、快照或堆分配；构造、复制和比较均为常数时间。只要包含它的外层类型满足并发要求，它本身可以安全地在线程间按值传递。

`VersionProvider` 只借用 `&self`，但没有声明内部同步策略，也没有作为 trait object 的 `Send + Sync` 约束。实现者若共享分配器、PD 客户端或缓存，必须在实现侧保证版本单调性、并发安全和错误传播；这些生命周期不由本文件管理。快照资源的创建与释放属于 `Storage::GetSnapshot` 返回对象的实现，而不是 `Version`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/kv/version.go`，Rust 版本逐项保留其公开结构：

- Go `VersionProvider.CurrentVersion() (Version, error)` 对应 Rust `Result<Version, Box<dyn Error>>`。
- Go `Version{Ver uint64}` 对应 Rust `Version { pub Ver: u64 }`。
- Go 包变量 `MaxVersion`/`MinVersion` 对应 Rust `const`，数值分别是 `math.MaxUint64` 与 `0`。
- Go `NewVersion` 与 Rust `NewVersion` 都原样包装输入。
- Go `Cmp` 返回 `int`，Rust 返回 `i32`，但三路比较分支及 `-1/0/1` 结果一致。

`pkg/kv/version_test.go::TestVersion` 与 `pkg/kv/version_test.rs::test_version` 的四项断言一致。Rust 测试文件还承载同一 Go 测试文件中的 MPP 与 Exchange 压缩测试，但那些行为分别来自其他模块，不属于 `version.rs`。

迁移差异需要特别记录：Rust 生产存储接口已有带 `txn_scope` 的 `Storage::CurrentVersion`，而本文件的 `VersionProvider` 保留 Go 的无参数签名且当前未接线。若未来收敛两套接口，应先确认 Go 上游语义与 Rust keyspace/事务作用域要求，不能简单删除参数或静默固定为 `"global"`。

## 扩展指南

- 新增版本构造规则或合法性检查时，优先评估是否应新增显式构造函数，而不是改变 `NewVersion` 的无校验 Go 对齐语义；同步扩展独立测试 `pkg/kv/version_test.rs` 和 Go 对照测试意图。
- 新增排序能力时，可考虑实现 `Ord`/`PartialOrd`，但必须保证与 `Cmp`、`Eq` 完全一致，并增加边界与排序集合测试；不要移除现有 `Cmp`，它是迁移 API。
- 改动 `MaxVersion` 语义时，必须同步审查 `pkg/store/driver/kv_adapter.rs::snapshot_timestamp`、所有 `GetSnapshot(MaxVersion)` 调用点及其针对“最新快照”的测试，否则可能把超大时间戳发送给 TiKV。
- 实现或替换 `VersionProvider` 时，应明确错误类型、事务作用域、单调性、`Send + Sync` 和取消/重试契约，并检查它与 `Storage::CurrentVersion` 是否应桥接；当前代码没有可复制的生产实现。
- 为 `Version` 增加字段会影响公开结构体字面量、`Copy` 成本和 Go 对齐，应先搜索 `Version { Ver: ... }` 的调用点。Rust 测试逻辑应继续放在独立的 `version_test.rs`，不要内嵌到生产文件。
- 性能上现有路径是零分配、常数时间；扩展时避免在高频快照构造和比较路径引入字符串解析、锁或网络调用。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/kv/version.rs` 读取完整 63 行并报告 28 个使用文件；`query NewVersion`、`query VersionProvider` 定位 Rust/Go 对照定义；`node NewVersion` 给出 `pkg/domain/domain.rs::snapshot` 等 Rust 调用边。路径限定 `callers/callees` 没有返回结果，因此对调用面补用了源码搜索。
- Rust 源码：`pkg/kv/version.rs`；模块出口：`pkg/kv/lib.rs`；生产接口：`pkg/kv/kv.rs`。
- crate 边界与 feature：`pkg/kv/Cargo.toml`。
- Rust 独立测试：`pkg/kv/version_test.rs::test_version`；相关存储转发与消费证据：`pkg/kv/fault_injection.rs`、`pkg/store/driver/kv_adapter.rs`、`pkg/domain/domain.rs`、`pkg/domain/canonical_domain.rs`、`pkg/session/runtime/relational_scan.rs`。
- Go 对照：`pkg/kv/version.go`；Go 测试：`pkg/kv/version_test.go::TestVersion`。
- 本任务是纯文档分析，按计划不运行 Cargo。结构校验要求本文恰好含有指定的 11 个二级标题；交付前另行执行该命令并检查退出码。
