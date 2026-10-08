# `pkg/util/tikvutil/tikvutil.rs` 逻辑说明

## 文件定位

`pkg/util/tikvutil/tikvutil.rs` 是 `astersql-util-tikvutil` crate 当前唯一的业务实现文件，负责保存 `tidb_committer_concurrency` 的进程级原子缓存。crate 的边界由 `pkg/util/tikvutil/Cargo.toml` 定义，`lib.rs` 通过 `pub mod tikvutil` 装载本文件并以 `pub use tikvutil::*` 再导出公开符号；工作区根 `Cargo.toml` 将它登记为 `facade_util_tikvutil`，随后 `pkg/lib.rs` 的 `pkg::util::tikvutil` 门面再次公开这些符号。

它不是 TiKV RPC 客户端、两阶段提交调度器或系统变量校验器。当前 Rust 文件只提供一个与 Go 原子整数用法对齐的共享值容器。全仓 Rust 文本引用检索显示，生产 Rust 尚未直接读取或写入这个缓存；除定义和门面再导出外，直接使用只出现在 `pkg/util/tikvutil/migration_aster_unit_test.rs`。因此，Rust 侧已经具备数据结构和公开路径，但尚不能据此断言完整的系统变量到 TiKV client 配置链已经迁移。

## 核心职责

本文件有两项紧密关联的职责：

1. `GoAtomicI32` 用标准库 `AtomicI32` 封装一个 `i32`，把公开操作限制为构造、顺序一致读取和顺序一致写入。
2. `CommitterConcurrency` 以 `128` 初始化一个进程级全局实例，表示系统变量 `tidb_committer_concurrency` 的当前缓存值。

该缓存面向“提交阶段并发请求数”这一配置语义；名称和默认值还可由 `pkg/sessionctx/vardef/tidb_vars.rs` 中的 `TiDBCommitterConcurrency` 与 `DefTiDBCommitterConcurrency` 交叉核对。范围校验并不属于本文件：Go 的系统变量定义将合法值限制为 `1..=10000`，Rust 的系统变量测试也验证相同截断规则，但 `GoAtomicI32::store` 本身接受任意 `i32`。

## 主要符号

- `pub struct GoAtomicI32(AtomicI32)`：公开类型、私有元组字段。调用方只能通过本文件给出的 API 访问内部原子值，不能绕开所选内存序。
- `GoAtomicI32::new(value: i32) -> Self`：`const fn` 构造器，使实例可以在静态初始化期创建；当前用于初始化 `CommitterConcurrency`。
- `GoAtomicI32::load(&self) -> i32`：以 `Ordering::SeqCst` 读取当前值。
- `GoAtomicI32::store(&self, value: i32)`：以 `Ordering::SeqCst` 替换当前值；不返回旧值，也不做范围检查。
- `pub static CommitterConcurrency: GoAtomicI32`：公开的进程级单例，初值为 `128`。名称刻意保留 Go 风格，因此符号处有 `#[allow(non_upper_case_globals)]`。

文件没有 trait、enum、宏、条件编译分支或错误类型。`lib.rs` 中只有测试模块受 `#[cfg(test)]` 控制，本实现文件本身在正常构建中始终参与编译。

## 执行流程

静态初始化时，`CommitterConcurrency` 调用 `GoAtomicI32::new(128)`，内部进一步调用 `AtomicI32::new(128)`；该过程不需要运行时惰性初始化或锁。

读取路径是调用方执行 `CommitterConcurrency.load()`，随后 `GoAtomicI32::load` 以 `SeqCst` 从内部 `AtomicI32` 取值并直接返回。写入路径是调用方执行 `CommitterConcurrency.store(value)`，随后 `GoAtomicI32::store` 以 `SeqCst` 覆盖内部值。文件内没有重试、比较交换、累加、回调或向 TiKV 配置主动推送的步骤。

现有独立 Rust 测试 `committer_concurrency_matches_go_atomic_load_store_semantics` 观察默认值 `128`，写入并读回 `256`，最后恢复为 `128`。另一个 Rust 测试 `pkg/sessionctx/variable/sysvar_test.rs::TestTiDBCommitterConcurrency` 验证系统变量层对 `1024`、`10001` 和 `0` 的规范化，但它没有调用本文件的静态量，不能作为生产接线证据。

## 数据与状态

唯一可变状态是 `CommitterConcurrency` 内部的一个有符号 32 位整数。它属于整个进程而非 session、事务或请求；所有持有该公开静态量路径的线程看到同一个原子对象。初值 `128` 与 `pkg/sessionctx/vardef/tidb_vars.rs::DefTiDBCommitterConcurrency` 以及 Go 对照文件一致。

本文件不保存值的来源、版本、更新时间或持久化状态。一次 `store` 只替换内存中的整数；进程重启后重新得到编译期初值。也不存在把多个相关配置作为一个快照共同更新的机制。由于类型仅提供 load/store，调用方不能通过该 API 原子地执行“读取后条件更新”或数值增减。

## 依赖与调用关系

下游依赖只有 Rust 标准库 `std::sync::atomic::{AtomicI32, Ordering}`；`pkg/util/tikvutil/Cargo.toml` 没有声明第三方 crate、feature 或构建脚本。内部调用边为 `GoAtomicI32::new -> AtomicI32::new`、`GoAtomicI32::load -> AtomicI32::load(SeqCst)`、`GoAtomicI32::store -> AtomicI32::store(SeqCst)`。

公开路径依次为：`tikvutil.rs` 的公开符号 -> `pkg/util/tikvutil/lib.rs` 的通配再导出 -> 工作区依赖别名 `facade_util_tikvutil` -> `pkg/lib.rs::util::tikvutil` 门面。RustCodeGraph 将目标文件识别为 48 行、4 个主要索引符号的实现文件；对通用方法名的全局 callers/callees 查询存在名称歧义，因此又以全仓 Rust 引用检索核验实际直接引用，结果只有 `migration_aster_unit_test.rs` 使用 `CommitterConcurrency`。

Go 主链提供了迁移语义参照：`pkg/sessionctx/variable/sysvar.go` 的全局系统变量 setter 写入 `tikvutil.CommitterConcurrency`，再调用 `tikvcfg.StoreGlobalConfig`；getter 读取同一值。`pkg/config/config.go::GetTiKVConfig` 读取该值并填入 client-go 的 `tikvcfg.Config::CommitterConcurrency`。这些是 Go 调用边，不应描述成当前 Rust 生产调用边。

## 错误处理与边界

本文件的三个方法都没有可失败返回值，也不会主动产生业务错误。标准原子 load/store 不涉及锁中毒。`store` 不验证 `tidb_committer_concurrency` 的业务范围，因此直接调用可以写入零、负数或大于 `10000` 的值；合法性必须由上层系统变量入口保证。

`i32` 是本文件的表示边界。来自更宽整数类型的值应在上层完成校验后再转换，不能依赖窄化转换替代范围检查。文件也不保证缓存与外部 TiKV client 全局配置同步：Go setter 在写缓存后另行刷新 client-go 配置，而当前 Rust 实现没有等价副作用。

## 并发与资源生命周期

`AtomicI32` 允许多个线程在不加互斥锁的情况下安全共享读写；两种运行时操作均使用最强的 `SeqCst` 顺序，为所有顺序一致原子操作提供单一全局次序。这与源文件声明的 Go `go.uber.org/atomic.Int32` 对齐目标一致，也避免调用者选择不一致的内存序。

全局实例从程序静态初始化开始存活至进程结束，没有 `Drop`、堆分配、文件描述符、网络连接、异步任务或后台线程需要释放。一次读写是单个原子操作，但“更新缓存后刷新其他配置”之类的多步骤流程并不由本文件原子化。

测试 `migration_aster_unit_test.rs` 会修改共享静态状态并在末尾恢复默认值。若未来新增会并行访问该静态量的测试，必须串行化相关测试或采用可隔离实例；仅靠末尾恢复不能防止测试执行期间的相互干扰，而且断言失败或 panic 会跳过手工恢复。

## 与 Go 版本的对应关系

Go 对照 `pkg/util/tikvutil/tikvutil.go` 只有一个生产符号：`var CommitterConcurrency = atomic.NewInt32(128)`。Rust 用 `GoAtomicI32` 加 `pub static CommitterConcurrency` 复刻了名称、`i32` 宽度、默认值以及 Load/Store 所需的并发安全语义；额外封装的原因是 Rust 标准库原子 API要求每次调用显式选择 `Ordering`，本文件统一固定为 `SeqCst`。

两端的差异主要在接线成熟度。Go 的静态量已由 `pkg/sessionctx/variable/sysvar.go` 读写，并由 `pkg/config/config.go::GetTiKVConfig` 消费；相关 Go 测试 `pkg/sessionctx/variable/sysvar_test.go::TestTiDBCommitterConcurrency` 验证系统变量边界。Rust 当前仅由 `pkg/util/tikvutil/migration_aster_unit_test.rs` 直接验证默认值和 load/store，系统变量范围测试位于 `pkg/sessionctx/variable/sysvar_test.rs`，但全仓 Rust 引用中尚无把两层连接起来的生产代码。因此移植状态应表述为“原子缓存与测试已存在，生产配置传播链未在 Rust 侧证实”。

## 扩展指南

若只是增加同类的进程级整数缓存，可复用 `GoAtomicI32`，但应先确认 Go 对照的整数宽度、默认值和原子操作集合；不要为了方便把字段公开，也不要擅自降低内存序。新增业务值还应在 `pkg/sessionctx/vardef` 定义唯一默认值，避免本文件与系统变量层的常量漂移。

若要完成 Rust 生产接线，最可能的修改点不是原子包装本身，而是 Rust 系统变量 setter/getter 与生成 TiKV client 配置的对应路径。实现时应复刻 Go 顺序：先经过系统变量类型和 `1..=10000` 范围规范化，再更新缓存，并明确处理下游全局配置刷新失败或不可用时的一致性策略。不能把无校验的 `store` 直接暴露为 SQL 配置入口。

测试应继续放在独立文件：原子包装与默认值覆盖 `pkg/util/tikvutil/migration_aster_unit_test.rs`；系统变量边界覆盖 `pkg/sessionctx/variable/sysvar_test.rs`；若增加实际配置传播，再在拥有该接线的 crate 的独立测试文件中验证缓存和 TiKV 配置同步。重点风险是全局测试状态污染、默认值多处复制、合法范围绕过，以及多步骤配置更新只完成一半。

## 验证依据

- 源码与模块边界：`pkg/util/tikvutil/tikvutil.rs`、`pkg/util/tikvutil/lib.rs`、`pkg/util/tikvutil/Cargo.toml`、根 `Cargo.toml` 的 `facade_util_tikvutil`、`pkg/lib.rs::util::tikvutil`。
- RustCodeGraph：执行了索引状态检查、`files --filter pkg/util/tikvutil`、目标文件 `node --file`、`GoAtomicI32` 的 `query/node` 及 callers/callees 探查；索引确认目标源码、模块入口、独立测试和 Go 对照文件均存在。通用 `load/store/new` 和静态量查询出现歧义，故未把噪声结果当成调用关系。
- 直接引用核验：全仓对 `CommitterConcurrency`、`GoAtomicI32`、`facade_util_tikvutil` 的 Rust `rg` 检索；除定义/再导出外，仅 `pkg/util/tikvutil/migration_aster_unit_test.rs` 直接读写静态量。
- Go 语义与在线调用边：`pkg/util/tikvutil/tikvutil.go`、`pkg/sessionctx/variable/sysvar.go`、`pkg/config/config.go`、`pkg/sessionctx/variable/sysvar_test.go`。
- Rust 相邻契约与测试：`pkg/sessionctx/vardef/tidb_vars.rs`、`pkg/sessionctx/variable/sysvar_test.rs`、`pkg/util/tikvutil/migration_aster_unit_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；完成判定使用任务规定的十一章节结构检查，并人工复核文档明确区分 Rust 已有实现、Go 生产链和尚未验证的 Rust 接线。
