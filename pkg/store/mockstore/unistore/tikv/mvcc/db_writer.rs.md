# `pkg/store/mockstore/unistore/tikv/mvcc/db_writer.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-store-mockstore-unistore-tikv-mvcc`，由同目录 `lib.rs` 以 `pub mod db_writer` 挂载，并通过 `pub use db_writer::*` 向 crate 使用方再导出。它位于 mock TiKV 的 MVCC 边界：定义持久化写入、锁存、事务写批和读快照所需的抽象，而不包含具体存储引擎或写队列实现。

当前 Rust 接线必须与 Go 主链区分开来。Go 的 `tikv/mvcc.go` 将 `mvcc.DBWriter` 保存为 `MVCCStore.dbWriter`，事务请求通过该接口创建并提交写批；Rust 的 `tikv/write.rs` 已有具体 `DbWriter` 和 `WriteBatch`，但仓库搜索未发现它们对本文件 `DBWriter`、`WriteBatch` 或 `LatchHandle` trait 的 `impl`。因此，本文件目前实际执行的 Rust 行为只有泛型快照构造 `NewDBSnapshot`；三个写路径 trait 仍主要承担 Go API 的迁移契约和未来接线边界。

## 核心职责

- `DBWriter` 规定写后端生命周期、批提交、范围删除和按事务时间戳创建写批的能力。
- `LatchHandle` 把按哈希值获取/释放 latch 的并发控制交给调用方实现，使范围删除能够与事务写协调。
- `WriteBatch` 描述 2PC 预写、提交、回滚以及悲观锁写入/回滚五类变更，但不规定变更如何编码或落盘。
- `DBSnapshotSource` 把“创建只读引擎快照”从具体引擎类型中抽象出来；`DBBundle` 和 `DBSnapshot` 分别组合引擎与内存锁表、引擎快照与共享锁表。
- `NewDBSnapshot` 每次向后端请求一个新的只读视图，同时通过 `Arc::clone` 共享同一个 `MemStore`。依据是 `db_writer.rs` 的 trait/结构定义以及 `migration_aster_unit_test.rs::db_snapshot_creates_read_view_and_shares_lock_store`。

## 主要符号

- `pub trait DBWriter`：公开写后端接口。`Open`/`Close` 管理资源；`Write(Box<dyn WriteBatch>) -> anyhow::Result<()>` 提交批次；`DeleteRange(start, end, &dyn LatchHandle)` 删除半开区间；`NewWriteBatch(startTS, commitTS, Option<&kvrpcpb::Context>)` 建立事务批次。方法名保留 Go 命名，因此文件级允许 `non_snake_case`。
- `pub trait LatchHandle`：公开的 latch 接口，`AcquireLatches` 与 `ReleaseLatches` 接收同一组 `u64` 哈希值。trait 本身不提供 RAII guard，成对释放由调用方保证。
- `pub trait WriteBatch`：公开的事务操作集合。`Prewrite`、`Commit`、`PessimisticLock` 接收可变 `Lock`，`Rollback` 的 `deleleLock`（沿用上游拼写）决定是否删除锁，`PessimisticRollback` 只表达移除悲观锁。
- `pub trait DBSnapshotSource`：公开的后端适配点，关联类型 `Snapshot` 表示实际快照，`NewReadSnapshot(&self)` 创建一次只读视图。
- `pub struct DBBundle<D>`：泛型后端容器，公开字段为 `DB: D`、`LockStore: Arc<MemStore>`、`MemStoreMu: Mutex<()>` 和 `StateTS: u64`。本文件只保存这些状态，不操作互斥量或时间戳。
- `pub struct DBSnapshot<S>`：一次读取使用的 `Txn: S` 与共享 `LockStore`。
- `pub fn NewDBSnapshot<D>`：要求 `D: DBSnapshotSource`，返回 `DBSnapshot<D::Snapshot>`；这是本文件唯一带函数体的生产逻辑。

## 执行流程

写路径的接口意图可由 Go 调用链复核：`MVCCStore` 在创建时调用 `writer.Open()`；预写、悲观锁、提交和回滚等请求先调用 `NewWriteBatch(startTS, commitTS, ctx)`，逐键调用相应 `WriteBatch` 方法累积变更，最后调用 `DBWriter.Write`。Go 的 `write.go::dbWriter.Write` 明确先等待 DB 批完成，再提交锁表批，保证提交数据成功后才删除锁。Rust `tikv/write.rs` 实现了相同顺序，但它当前没有实现本文件的 trait，不能据此声称 Rust 请求主链通过本接口运行。

范围删除的 Go 流程是：扫描 `[start, end)` 的全部版本键，以 4096 个键为一批计算用户键哈希，获取 latch，提交删除批，等待结果，再释放 latch；空 `end` 会触发 `panic("invalid end key")`。本文件的 `DeleteRange` 只保留该能力边界，没有实现这些步骤。

快照流程已经在 Rust 中接线：调用 `NewDBSnapshot(&bundle)` 后，先执行 `db.DB.NewReadSnapshot()` 取得独立的后端读视图，再克隆 `db.LockStore` 的 `Arc` 放入返回值。连续调用会创建不同 `Txn`，但两个结果与 `DBBundle` 指向同一个锁存储。

## 数据与状态

事务标识由 `startTS` 和 `commitTS` 传入 `NewWriteBatch`。接口没有自行校验时间戳关系，也不读取 `kvrpcpb::Context`；其语义由具体实现承担。Go 具体实现以非零 `commitTS` 更新最新时间戳，否则使用 `startTS`，并忽略 `math.MaxUint64` 系统哨兵值；这是实现证据，不是本 trait 的强制默认行为。

`DBBundle<D>` 同时持有四类状态：泛型数据库后端 `DB`、引用计数共享的内存锁表、用于协调内存状态的互斥量和 `StateTS`。`DBSnapshot<S>` 只复制后端新建的快照值并增加锁表 `Arc` 的强引用，不复制锁表内容，也不携带 `MemStoreMu` 或 `StateTS`。因此锁表观察是共享的，而引擎读视图是否隔离及其一致性级别由 `D::Snapshot` 的实现决定。

`WriteBatch` 方法接收 `&mut self`，表达批次内累积变更；`DBWriter::Write` 获取装箱后的 trait object 所有权，表达提交后批次不应再被调用方复用。锁参数使用 `&mut Lock`，接口本身允许实现修改锁对象，但本文件不规定是否修改。

## 依赖与调用关系

直接 Rust 依赖为标准库 `Arc`/`Mutex`、本 crate 的 `lockstore::MemStore` 与 `mvcc::Lock`、`anyhow::Result`，以及 `kvproto::kvrpcpb::Context`。`Cargo.toml` 将它们分别落实为本地适配模块、`anyhow = "1"` 和带固定 tag `v0.0.2-aster.20260929`/`protobuf-codec` feature 的 `kvproto` Git 依赖；该 crate 没有额外 feature 条件，本文件也没有 `cfg` 分支。

模块入口 `mvcc/lib.rs` 公开本模块并再导出全部符号。RustCodeGraph 将目标文件识别为 21 个符号，并能定位本文件与同路径 Go 的同名类型；精确源码搜索显示 Rust 侧只有 `migration_aster_unit_test.rs` 调用 `NewDBSnapshot`，未找到三个写 trait 的实现或业务调用。`tikv/write.rs` 的同名具体类型是邻近的真实 Rust 写实现，但属于另一个 crate/模块 API，不能视作 trait 实现。

Go 侧的直接上游是 `tikv/mvcc.go::MVCCStore`，直接下游实现是 `tikv/write.go::dbWriter`/`writeBatch`；`mvcc_test.go::NewTestStore` 组装 Badger、`DBBundle`、`NewDBWriter` 和 `NewMVCCStore`，覆盖真实接口接线。Rust 快照测试的下游是测试适配器 `CountingDb::NewReadSnapshot`。

## 错误处理与边界

只有 `DBWriter::Write` 和 `DeleteRange` 在接口上返回 `anyhow::Result<()>`；错误来源和上下文完全由实现定义。其他写批方法、生命周期方法和 `NewReadSnapshot` 无返回错误，因此实现若存在可恢复失败，需要在内部延迟到 `Write`、改变接口，或采用不可恢复处理，不能在本文件现有签名中直接传播。

边界条件包括：范围语义为 `[start, end)`；Go 参考实现拒绝空 `end`，但 Rust trait 本身没有编码这一前置条件；`Rollback` 可选择保留锁；悲观锁提交在 Go 具体实现中不写业务版本，只删除锁；`Op::Lock` 仅在主键上写额外事务状态。后两项属于具体 `WriteBatch` 实现语义，扩展 trait 时需保持，但不能误认为由默认方法保证。

`Mutex` 或后端锁中毒、`Arc` 计数、快照创建失败均未在本文件处理。`NewDBSnapshot` 假定 `NewReadSnapshot` 是不可失败操作；若新后端需要返回错误，应先调整 trait 和所有调用者，而不是吞掉错误。

## 并发与资源生命周期

`DBBundle.LockStore` 与各 `DBSnapshot.LockStore` 通过 `Arc` 共享所有权；快照析构只减少引用计数，不会主动关闭数据库。`DBSnapshot` 未实现显式 `Close`/`Discard`，具体 `Snapshot` 的释放依赖其 `Drop` 或调用者管理。`MemStoreMu` 是 bundle 级互斥量，但本文件既不加锁也不把它传入快照。

`DBWriter::Open`/`Close` 建模写资源的显式生命周期。Go 实现中 `Open` 启动 DB 与锁表两个后台 worker，`Close` 关闭通道并等待 worker；`Write` 串联两个 worker 的完成信号。Rust 邻近实现改用 `AtomicBool` 与 `RwLock`，没有后台线程。这种差异再次说明 trait 只定义能力，不保证具体调度模型。

`LatchHandle` 要求调用方围绕临界区成对调用获取和释放。由于接口没有 guard 类型，错误返回、panic 或提前退出都可能造成遗漏释放；新增 Rust 实现时宜在实现内部引入作用域 guard，但若改变公开 trait，需要同步所有调用者并验证 Go 的批量加锁顺序和死锁规避规则。

## 与 Go 版本的对应关系

`db_writer.rs` 基本逐项对应同路径 `db_writer.go`：`DBWriter`、`LatchHandle`、`WriteBatch` 的方法集合一致；泛型 `DBBundle<D>` 对应固定 `*badger.DB`；`DBSnapshot<S>` 对应固定 `*badger.Txn`；`Arc<MemStore>` 对应 Go 指针共享；`Mutex<()>` 对应 `sync.Mutex`；`NewDBSnapshot` 对应 `db.DB.NewTransaction(false)` 加共享 `LockStore`。

Rust 为避免把 MVCC crate 绑定到尚未选定的 Badger 类型，增加了 Go 中不存在的 `DBSnapshotSource` 与关联类型。Rust 还把可空 Go `*kvrpcpb.Context` 表达为 `Option<&Context>`，把接口值写批表达为 `Box<dyn WriteBatch>`，把错误统一为 `anyhow::Result`。字段和方法保留大写 Go 命名以方便迁移对照。

迁移尚不完整：Go 的 `dbWriter` 明确实现本接口并由 `MVCCStore` 使用；Rust `tikv/write.rs::DbWriter`/`WriteBatch` 使用惯用小写方法，未实现本文件 trait，Rust 的 store 主链也未出现这些 trait 的调用。已验证对齐的是 `NewDBSnapshot`：`migration_aster_unit_test.rs` 证明每次创建新的读视图且锁表仍由同一 `Arc` 共享。

## 扩展指南

若接入新的存储引擎，最小入口是为后端实现 `DBSnapshotSource`，明确关联快照的隔离与释放语义；若接入完整写路径，还需实现 `DBWriter`、`WriteBatch` 和可用的 `LatchHandle`，并把 `tikv/write.rs` 或新的后端实际接到调用主链。不要仅新增同名具体方法而假定 trait 已实现。

修改写批语义时，应同步核对 Go `tikv/write.go` 中的先 DB 后锁顺序、悲观锁和 `Op::Lock` 分支、rollback 状态记录及最新时间戳规则。修改范围删除时，应保留半开区间、空结束键处理、4096 批量上限、按用户键加 latch、错误时释放 latch等约束。性能风险主要是过度克隆锁数据、长时间持有锁、范围删除一次性收集大量键和将批处理退化为逐键写入。

测试必须放在独立文件。快照变化应扩展 `mvcc/migration_aster_unit_test.rs`；具体内存写实现变化应扩展 `tikv/main_test.rs` 或相应同目录独立测试；接口接线与端到端事务行为应参考 Rust `tikv/mvcc_test.rs` 的现有测试面，并与 Go `tikv/mvcc_test.go`/`write_test.go` 的边界案例对照。若改变公开签名，还需检查 `mvcc/lib.rs` 的再导出和所有 Cargo 使用方。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标文件含 21 个符号；`node --file .../db_writer.rs` 读取了完整 115 行；对 `DBWriter`、`LatchHandle`、`WriteBatch`、`DBSnapshotSource`、`DBBundle`、`DBSnapshot`、`NewDBSnapshot` 的 `query --json` 定位了 Rust/Go 对应符号。`callers`/`callees` 查询两次在 30 秒内未返回，因此没有据此推断调用边。
- 生产源码：`pkg/store/mockstore/unistore/tikv/mvcc/db_writer.rs`、`mvcc/lib.rs`、`tikv/write.rs`、`tikv/mvcc.rs`；后两者用于确认邻近 Rust 实现及当前是否接线。
- 包边界：`pkg/store/mockstore/unistore/tikv/mvcc/Cargo.toml`，确认 crate 名、库入口、`anyhow`/`kvproto`/本地依赖和无 feature 条件。
- Go 对照：`mvcc/db_writer.go` 给出一一对应的接口与快照结构；`tikv/write.go` 给出写批、顺序提交、时间戳和范围删除的实现；`tikv/mvcc.go` 给出 `MVCCStore` 上游调用主链。
- 测试证据：Rust `mvcc/migration_aster_unit_test.rs::db_snapshot_creates_read_view_and_shares_lock_store`；Rust `tikv/main_test.rs::memory_db_writer_preserves_lock_then_commit_order`（邻近具体实现，不是本 trait 的直接测试）；Go `tikv/mvcc_test.go::NewTestStore` 的接口组装。仓库搜索未发现同名独立 `db_writer` Rust 测试。
- 人工复核结论：本文件存在是为了稳定 Go MVCC 写/快照边界并解除快照对具体引擎的绑定；其唯一已验证的本地执行体是 `NewDBSnapshot`，安全扩展必须同时处理 trait 接线、独立测试、锁释放和 DB-before-lock 顺序，不能把未实现的接口当作已运行能力。
