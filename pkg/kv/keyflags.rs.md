# `pkg/kv/keyflags.rs`

## 文件定位

本文件定义 `astersql-kv` crate 的“事务内单键元数据”位编码，以及把高层标志操作合并到该编码的纯函数。源码由 [`pkg/kv/lib.rs`](lib.rs) 的 `keyflags` 模块通过 `include!("keyflags.rs")` 纳入，并由 `pub use keyflags::*` 从 crate 根重新导出；因此 `KeyFlags`、`FlagsOp` 和 `ApplyFlagsOps` 是 `astersql_kv` 的公开 API，而六个具体 bit 常量保持 `pub(crate)`。

它位于 SQL 层与事务内存缓冲之间的 KV 抽象层：[`pkg/kv/kv.rs`](kv.rs) 的 `MemBuffer` 接口用 `KeyFlags`/`FlagsOp` 描述 `GetFlags`、`SetWithFlags`、`UpdateFlags`、`DeleteWithFlags` 和 `InspectStage`，具体缓冲实现负责保存每个 key 的位集。本文件本身不访问 TiKV、不发起 prewrite，也不管理事务；它只提供可被这些路径携带和判定的紧凑元数据。

[`pkg/kv/Cargo.toml`](Cargo.toml) 证明归属 crate 为 `astersql-kv`、库入口为 `lib.rs`。本文件只依赖 Rust 核心运算 trait，没有 feature 条件或外部 crate 依赖；`nextgen` feature 不改变这里的布局或分支。

## 核心职责

- 用 `KeyFlags(pub u8)` 固定六个已有语义位：延迟检查键不存在、需要加锁、两个存在性断言位、将约束检查推迟到 prewrite、以及“延迟不存在判断来自前序语句”。依据是 `flagPresumeKNE` 至 `flagPreviousPresumeKNE`。
- 用两个断言位编码四态：`00` 未设置、`01` 必须存在、`10` 必须不存在、`11` 无法断言。查询由 `HasAssertExists`、`HasAssertNotExists`、`HasAssertUnknown` 和 `HasAssertionFlags` 完成。
- 用 `FlagsOp(pub u16)` 保留与 Go `iota` 相同的操作编号，并由 `ApplyFlagsOps` 将已知操作对应的位 OR 入旧值。
- 保持追加式合并语义：普通 `FlagsOp` 只置位、不清位；断言位的互斥更新由相邻的 [`pkg/kv/assertion.rs`](assertion.rs) 中 `ApplyAssertionOp` 负责，而不是本文件负责。
- 对未知 `FlagsOp` 保持 no-op，使透明整数包装能够容忍当前实现不认识的操作值，行为与 Go 无 `default` 的 `switch` 一致。

## 主要符号

- `pub struct KeyFlags(pub u8)`：公开的透明语义包装（未声明 `repr(transparent)`），派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq`、`PartialEq`；默认值为零，即没有任何标志。公开元组字段允许边界适配器直接读写原始字节。
- `BitOrAssign`、`BitAndAssign`、`Not`：分别对底层 `u8` 执行 `|=`、`&=`、`!`。本文件的 `ApplyFlagsOps` 直接 OR 底层值；这些 trait 主要供断言更新等同 crate 逻辑组合和清除位使用。
- `flagPresumeKNE = 1 << 0`：键存在性采用延迟检查；`HasPresumeKeyNotExists` 查询该位。
- `flagNeedLocked = 1 << 1`：键需要获取锁；`HasNeedLocked` 查询该位。
- `flagAssertExists = 1 << 2` 与 `flagAssertNotExists = 1 << 3`：共同组成断言四态。单一查询方法要求目标位为 1 且相反位为 0，避免把 `11` 误报为“存在”或“不存在”。
- `flagNeedConstraintCheckInPrewrite = 1 << 4`：把冲突和约束检查推迟到下一次悲观锁或 prewrite；`HasNeedConstraintCheckInPrewrite` 查询该位。
- `flagPreviousPresumeKNE = 1 << 5`：记录 PNE 来自前序语句，以便语句重试或回滚路径不错误清除。当前文件提供置位操作，但没有公开查询方法；需要读取该语义的下游必须经同 crate 内部逻辑或专门适配层处理，不能把 `HasPresumeKeyNotExists` 当成此位的查询。
- `pub struct FlagsOp(pub u16)`：使用 `#[repr(transparent)]` 的公开操作码包装；常量值 `SetPresumeKeyNotExists`、`SetNeedLocked`、`SetNeedConstraintCheckInPrewrite`、`SetPreviousPresumeKeyNotExists` 依次为 0、1、2、3。
- `pub fn ApplyFlagsOps(origin: KeyFlags, ops: &[FlagsOp]) -> KeyFlags`：唯一变换入口，顺序扫描操作切片，将已知操作映射为位并合并，返回新值。

## 执行流程

典型写入链从 `MemBuffer::SetWithFlags` 或 `MemBuffer::DeleteWithFlags` 接收 key、值与 `&[FlagsOp]` 开始。具体实现先完成 set/delete，再调用自己的 `UpdateFlags`。例如 [`pkg/store/driver/kv_adapter.rs`](../store/driver/kv_adapter.rs) 的 `ClientMemBuffer::UpdateFlags` 和 [`pkg/store/mockstore/mockstorage/canonical_storage.rs`](../store/mockstore/mockstorage/canonical_storage.rs) 的 `KVTxn::UpdateFlags` 都按如下流程运行：

1. 从按 key 保存的 flags map 中读取旧字节；不存在时使用 `KeyFlags::default()`，即 0。
2. 把旧值和操作切片传给 `ApplyFlagsOps`。
3. `ApplyFlagsOps` 逐项匹配四个已知 `FlagsOp`；匹配成功时选择对应 bit，未知值产生 `None`。
4. 对已知 bit 执行 OR，因此重复操作幂等，先后顺序不会改变最终位并集；空切片和全未知切片原样返回 `origin`。
5. 实现把结果重新写回该 key 的 flags map。之后 `GetFlags`、`InspectStage` 或提交适配逻辑可以携带/读取这些语义。

断言更新走另一条相邻链：`MemBuffer::UpdateAssertionFlags` 调用 `ApplyAssertionOp`。后者利用本文件的按位运算 trait 和两个 crate-private 断言常量，在保留 PNE、锁与约束检查位的同时切换断言状态。

## 数据与状态

`KeyFlags` 是值类型，不持有 key，也不自行保存到全局状态。位布局为：bit 0 `PresumeKNE`、bit 1 `NeedLocked`、bit 2 `AssertExists`、bit 3 `AssertNotExists`、bit 4 `NeedConstraintCheckInPrewrite`、bit 5 `PreviousPresumeKNE`；bit 6、7 当前未定义。由于 `ApplyFlagsOps` 从 `origin` 开始且只 OR 指定位，既有的未知高位会被保留。

断言位有一个重要不变量：查询语义以 bit 2/3 的组合为准，而非任选一个位。`HasAssertionFlags` 对 `01`、`10`、`11` 都返回 true；`HasAssertUnknown` 只对 `11` 返回 true；`HasAssertExists` 和 `HasAssertNotExists` 在 `11` 时都返回 false。源码注释约定断言一旦设置，在当前事务内预期不可变；本类型并不在运行时强制这一事务级约束，真正的断言转换集中在 `ApplyAssertionOp`。

`FlagsOp` 使用 `u16` 而 `KeyFlags` 使用 `u8`：前者是操作协议空间，后者是实际存储位集，两者不能按数值直接互换。`FlagsOp::default()` 的值是 0，因此等同 `SetPresumeKeyNotExists`；这与 Go 的零值/iota 语义一致，但调用者若想表达“不做操作”应传空切片，而不是传默认操作。

## 依赖与调用关系

上游契约来自 [`pkg/kv/kv.rs`](kv.rs) 的 `MemBuffer`。其中 flags 既可在写/删时附带，也可单独更新，并会随 `InspectStage` 的回调返回。`FindKeysInStage` 进一步允许调用者按 `(Key, KeyFlags, value)` 谓词筛选 staged key。

已确认的直接 Rust 调用边包括：

- `ClientMemBuffer::UpdateFlags` → `kv::ApplyFlagsOps`（[`pkg/store/driver/kv_adapter.rs`](../store/driver/kv_adapter.rs)），用于 client-rust 事务适配器的本地 flags 镜像。
- `KVTxn::UpdateFlags` → `kv::ApplyFlagsOps`（[`pkg/store/mockstore/mockstorage/canonical_storage.rs`](../store/mockstore/mockstorage/canonical_storage.rs)），用于 canonical mock storage 的事务内 flags map。
- `ApplyAssertionOp` 使用 `KeyFlags`、`flagAssertExists`、`flagAssertNotExists` 及其位运算实现（[`pkg/kv/assertion.rs`](assertion.rs)）。
- `MemBuffer` 的 `GetFlags`/`SetWithFlags`/`UpdateFlags`/`DeleteWithFlags`/`InspectStage` 在接口层传播这些类型（[`pkg/kv/kv.rs`](kv.rs)）。

本文件没有函数调用下游业务服务；`ApplyFlagsOps` 的下游仅是本地匹配和整数位运算。RustCodeGraph 将目标文件标为被 `pkg/kv/assertion.rs`、`pkg/kv/kv.rs`、独立测试等 7 个文件使用；由于索引的精确 `callers` 命令在本地持续超时，以上跨目录调用边又以限定范围源码检索核实。

## 错误处理与边界

全部 API 都是无失败返回的纯值运算，没有 `Result`、panic 分支、I/O 或分配。未知 `FlagsOp(u16)` 明确为 no-op，不会报错，也不会清除旧 flags；[`pkg/kv/keyflags_test.rs`](keyflags_test.rs) 用 `u16::MAX` 验证这一点。空 `ops` 同样保持原值。

边界行为如下：

- 重复设置同一操作是幂等的，因为合并采用 OR。
- `ApplyFlagsOps` 不提供清位操作；需要清理或互斥切换的断言必须走 `ApplyAssertionOp`，不能通过追加 `FlagsOp` 达成。
- `KeyFlags` 的公开字段允许构造任意 `u8`，所以方法必须正确处理未知高位和任意断言位组合；独立测试穷举 `0..=u8::MAX` 验证四个断言查询。
- `Not` 会翻转全部 8 位，而不仅是当前已定义的 6 位；它应与掩码通过 `&=` 配合用于清除特定位。单独对一个完整 flags 值取反会同时生成未知高位。
- `SetPreviousPresumeKeyNotExists` 与 `SetPresumeKeyNotExists` 是两个独立 bit；设置前者不会隐式设置后者。
- 本文件只记录“需要在 prewrite 检查”的意图，不执行检查，也不保证某个具体存储后端一定消费该位；后端接线正确性属于相应适配器的职责。

## 并发与资源生命周期

`KeyFlags` 和 `FlagsOp` 都是 `Copy` 值，`ApplyFlagsOps` 只修改函数栈上的副本，因此本文件没有锁、原子变量、任务、通道、引用生命周期或资源释放逻辑。函数可在并发线程中独立调用，不共享可变状态。

并发安全和事务生命周期由持有 flags 的实现决定。例如 `ClientMemBuffer` 在 [`pkg/store/driver/kv_adapter.rs`](../store/driver/kv_adapter.rs) 中用 `RwLock<BufferState>` 保护 flags map；canonical mock 的 `KVTxn` 则由可变借用串行更新。`MemBuffer` 的 staging、`Release`/`Cleanup` 负责语句级写集生命周期：canonical mock 的 staging 快照同时包含 flags，回滚时恢复它们。`flagPreviousPresumeKNE` 的语义与跨语句保留有关，但本文件仅编码该状态，不执行重试或回滚。

## 与 Go 版本的对应关系

直接对照文件为 [`pkg/kv/keyflags.go`](keyflags.go)。Rust 保持了以下 Go 语义：

- `KeyFlags` 对应 Go `uint8`，六个位按相同顺序分配；两个断言位的四态解释完全相同。
- 六个查询方法逐一对应 Go 同名方法，布尔表达式一致。
- `FlagsOp` 对应 Go `uint16`，四个常量保持 `iota` 的 0 至 3 编号。
- `ApplyFlagsOps` 与 Go `switch` 一样只置位；Go 没有 `default`，Rust 用 `_ => None` 保持未知操作 no-op。
- Go 可变参数 `ops ...FlagsOp` 在 Rust 中表现为借用切片 `ops: &[FlagsOp]`；调用方式不同，但顺序与合并结果一致。

Rust 特有部分是为 `KeyFlags` 实现 `BitOrAssign`、`BitAndAssign`、`Not`，并通过 newtype 和派生 trait 获得类型区分及值语义。`FlagsOp` 显式使用 `#[repr(transparent)]`；`KeyFlags` 没有该属性，因此不应据此宣称其具备稳定的 FFI ABI。Go 同路径未发现专门的 `keyflags_test.go`；当前直接回归证据来自 Rust 独立测试 [`pkg/kv/keyflags_test.rs`](keyflags_test.rs)，以及覆盖断言转换和非断言位保留的 [`pkg/kv/assertion_1_aster_unit_test.rs`](assertion_1_aster_unit_test.rs)。

## 扩展指南

新增普通元数据位时，应先确认 `u8` 剩余容量与 Go 的位布局，再同步修改 `pkg/kv/keyflags.go` 和本文件：增加 crate-private bit、公开查询方法、`FlagsOp` 常量及 `ApplyFlagsOps` 映射。操作码必须追加并保持已有 0 至 3 的数值，避免破坏持久化、跨 crate 或 TiKV 适配约定；如果超过 bit 7，则需要评估 `KeyFlags` 的底层宽度、所有 flags map 及协议转换，而不能只改此文件。

新增断言状态或清位操作时，应优先修改 [`pkg/kv/assertion.rs`](assertion.rs) 的 `AssertionOp`/`ApplyAssertionOp`，因为 `ApplyFlagsOps` 的契约是追加式置位。若确实新增清位类 `FlagsOp`，必须重新审视“操作顺序不影响结果”和幂等性，不应继续把它描述为简单位并集。

测试必须保持独立文件，不内嵌进 `keyflags.rs`。至少同步扩展 [`pkg/kv/keyflags_test.rs`](keyflags_test.rs)：验证精确 bit 值、所有查询组合、空/重复/未知操作、保留既有与未知高位；涉及断言时同步 [`pkg/kv/assertion_1_aster_unit_test.rs`](assertion_1_aster_unit_test.rs)。若操作需要传入存储后端，还要检查 `ClientMemBuffer::UpdateFlags`、canonical mock 的 `KVTxn::UpdateFlags`，以及独立的 unionstore/TiKV 操作映射，防止 KV 抽象已支持但后端丢失语义。

兼容性风险主要是改变位或操作码编号、把未知操作从 no-op 改成错误、改变零值含义；正确性风险主要是混淆 `PreviousPresumeKNE` 与当前 PNE、或破坏断言 `11` 的 unknown 语义；性能风险很低，但扩大表示或引入分配会影响每 key 的事务内存开销和热路径更新成本。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query KeyFlags` 定位到 `pkg/kv/keyflags.rs:27`，`query ApplyFlagsOps --kind function --json` 定位到 Rust `keyflags.rs::ApplyFlagsOps` 与 Go `pkg/kv/keyflags.go::ApplyFlagsOps`。
- RustCodeGraph `node --file pkg/kv/keyflags.rs --offset 1 --limit 260`：读取目标文件全部 129 行，并报告它被 7 个文件使用。精确 `callers keyflags.rs::ApplyFlagsOps` 在本地 SQLite 后端持续无输出并超时，已用下述限定范围直接调用点补证，未把不完整图结果推断为不存在调用者。
- 目标源码 [`pkg/kv/keyflags.rs`](keyflags.rs)：核对全部常量、类型、trait impl、查询方法与 `ApplyFlagsOps`；文件没有条件编译项。
- crate 与模块入口：[`pkg/kv/Cargo.toml`](Cargo.toml)、[`pkg/kv/lib.rs`](lib.rs)、[`pkg/kv/kv.rs`](kv.rs)。
- Go 对照：[`pkg/kv/keyflags.go`](keyflags.go)。
- 直接实现与调用点：[`pkg/store/driver/kv_adapter.rs`](../store/driver/kv_adapter.rs) 的 `ClientMemBuffer`，以及 [`pkg/store/mockstore/mockstorage/canonical_storage.rs`](../store/mockstore/mockstorage/canonical_storage.rs) 的 `KVTxn`。
- 相邻断言逻辑：[`pkg/kv/assertion.rs`](assertion.rs)。
- 独立测试：[`pkg/kv/keyflags_test.rs`](keyflags_test.rs) 的 `key_flags_match_go_bit_semantics`、`unknown_flags_op_is_a_no_op_like_go`；[`pkg/kv/assertion_1_aster_unit_test.rs`](assertion_1_aster_unit_test.rs) 的 `assertion_and_key_flag_transitions_match_go`。

本任务为纯文档分析，按计划不运行 Cargo。结构验证要求目标文件存在且恰有 11 个固定二级标题；交付前另行执行该命令并检查退出码。
