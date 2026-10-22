# `pkg/kv/assertion.rs`

## 文件定位

这里不实现“同一事务中断言一旦设置便不可改变”的策略。

## 核心职责

- `AssertionOp` 把四种公开断言操作表示为与 Go `uint8` 相同的数值域，并与普通的 `FlagsOp` 分型，减少把断言位当成一般标志更新的误用。
- `ApplyAssertionOp` 将操作映射到两个断言位，同时保留 `KeyFlags` 中所有非断言位。
- 已知编码保持 Go 顺序：`AssertExist=0`、`AssertNotExist=1`、`AssertUnknown=2`、`AssertNone=3`；未知 `u8` 值保持输入不变。

这里不实现“同一事务中断言一旦设置便不可改变”的策略。该策略由上层先检查 `KeyFlags::HasAssertionFlags` 再决定是否调用更新接口；本函数若被直接连续调用，会按操作覆盖互斥位或合并为 unknown。Go 的对应职责边界见 `pkg/kv/assertion.go` 与 `pkg/table/tables/assertion.go`。

## 主要符号

- `pub struct AssertionOp(pub u8)`：`#[repr(transparent)]` 的公开新类型。内部字段公开，调用方能够构造未知值；`Clone + Copy + Default + Eq + PartialEq` 使其可按值传递和比较。派生的 `Default` 是 `AssertionOp(0)`，即数值上等同 `AssertExist`，并非 `AssertNone`。
- `AssertionOp::AssertExist`：值 `0`；结果必须只有 exists 断言位为 1。
- `AssertionOp::AssertNotExist`：值 `1`；结果必须只有 not-exists 断言位为 1。
- `AssertionOp::AssertUnknown`：值 `2`；两个断言位均置 1，表示无法作存在性断言，而不是“没有设置断言”。
- `AssertionOp::AssertNone`：值 `3`；不修改输入。它不会清除已有断言位。
- `pub fn ApplyAssertionOp(origin: KeyFlags, op: AssertionOp) -> KeyFlags`：纯值转换入口；消费 `KeyFlags` 的副本并返回更新后的副本。RustCodeGraph 将 `pkg/kv/assertion_1_aster_unit_test.rs::assertion_and_key_flag_transitions_match_go` 识别为直接调用者；源码搜索还确认 `pkg/store/driver/kv_adapter.rs::UpdateAssertionFlags` 和 `pkg/store/mockstore/mockstorage/canonical_storage.rs::UpdateAssertionFlags` 调用它。

## 执行流程

1. 调用者从键的内存缓冲状态取得当前 `KeyFlags`；缺失键通常以 `KeyFlags::default()`/零值开始（见两个 Rust `UpdateAssertionFlags` 实现）。
2. `ApplyAssertionOp` 按 `AssertionOp` 分支：
   - `AssertExist` 先置 exists 位，再清 not-exists 位；
   - `AssertNotExist` 先置 not-exists 位，再清 exists 位；
   - `AssertUnknown` 同时置两个位；
   - `AssertNone` 或任何其他 `u8` 值不做修改。
3. 函数返回完整的 `KeyFlags`。由于只触碰位 2、3，PNE、加锁、延迟约束检查等其他位保持原值。
4. 具体 `MemBuffer` 实现再把返回值写回各自的 flags 映射；锁与存储更新发生在调用方，不在本函数内。

当输入断言位已经是 unknown（`11`）时，调用 `AssertExist`/`AssertNotExist` 会得到 `01`/`10`；因此不可变性必须在调用本函数前保证。`AssertNone` 对任意输入都保持原样，也不能用作清除操作。

## 数据与状态

断言状态占用 `KeyFlags` 的两位（`pkg/kv/keyflags.rs` 中位 2 和位 3）：`00` 为未设置，`01` 为必须存在，`10` 为必须不存在，`11` 为 unknown。`KeyFlags::HasAssertExists`、`HasAssertNotExists`、`HasAssertUnknown` 和 `HasAssertionFlags` 负责解释这四种组合。

`AssertionOp` 和 `KeyFlags` 都是小型 `Copy` 值；本文件没有全局可变状态、堆分配、缓存或生命周期绑定。操作只在传入副本上执行位或、位与和按位非。公开 tuple 字段意味着未知操作值是合法可表示状态；当前兼容约定是将它们视为 no-op。

## 依赖与调用关系

上游接口是 `pkg/kv/kv.rs::MemBuffer::UpdateAssertionFlags(&mut self, Key, AssertionOp)`。已核实的 Rust 落地调用包括：

- `pkg/store/driver/kv_adapter.rs::UpdateAssertionFlags`：在写锁保护的状态中读取旧 flags，调用 `kv::ApplyAssertionOp` 后写回。
- `pkg/store/mockstore/mockstorage/canonical_storage.rs::UpdateAssertionFlags`：从内部字节 flags 映射构造 `kv::KeyFlags`，应用操作后写回底层 `u8`。

下游仅依赖 crate 根作用域中的 `KeyFlags`、`flagAssertExists`、`flagAssertNotExists` 以及它们实现的位运算 trait；这些符号由 `pkg/kv/lib.rs` 的 `use crate::*` 和 `pkg/kv/keyflags.rs` 提供。本文件没有直接外部 crate 调用，因此 `pkg/kv/Cargo.toml` 未为它引入专属依赖。

RustCodeGraph 的静态调用图只识别到直接单元测试调用边，未完整恢复 trait 实现中的跨模块调用；上述两个生产调用点由仓库源码搜索和对应函数体核实。仓库另有 `pkg/store/driver/txn/unionstore_driver.rs` 的本地 `AssertionOp`/`apply_assertion`，它属于驱动自身类型，不是本文件符号，扩展时不可混为同一 API。

## 错误处理与边界

`ApplyAssertionOp` 无返回错误且不会 panic；所有 `u8` 操作值都有定义好的结果。`AssertNone` 和未知值进入通配分支并保持 `origin`，对齐 Go `switch` 对未匹配值的隐式 no-op。该宽容行为也是前向兼容边界，不能在不评估 Go 行为和调用方的情况下改为报错或 panic。

函数不校验调用顺序，也不阻止覆盖已有断言。若业务需要事务内不可变性，应在表层/缓冲更新层用 `HasAssertionFlags` 拦截，而不是误认为此转换函数会拒绝第二次更新。文件也不处理键、事务、网络或存储错误；这些错误属于调用方。

## 并发与资源生命周期

本文件是无副作用的同步值计算，没有锁、原子量、任务、通道、I/O 或资源清理。`AssertionOp`/`KeyFlags` 按值复制，函数本身可被并发调用。

共享 flags 映射的一致性由实现 `MemBuffer` 的调用方负责。例如 `pkg/store/driver/kv_adapter.rs` 在读改写期间持有 `state.write()` 写锁；`canonical_storage.rs` 则由其对象的可变借用约束更新。任何新增调用点都必须在自身存储层完成“读取旧值—转换—写回”的同步和生命周期管理。

## 与 Go 版本的对应关系

Rust 使用通配分支同时覆盖 `AssertNone` 和未知值；Go 仅显式列出 `AssertNone`，但没有 `default`，所以未知值同样保持输入。`pkg/kv/assertion_test.rs::assertion_operations_cover_go_uint8_domain` 枚举全部 256 个 origin 和全部 256 个 op，验证这一差异只在语法表达上而非行为上。

Go 的 `pkg/table/tables/assertion_test.go::TestSetAssertion` 还验证上层语义：首次设置后断言不可改变，`AssertNone` 初次使用仍允许之后设置真实断言，普通 flag 更新不能破坏断言。这些是链路级约束，不全由本文件单独保证；Rust 的局部测试只验证这里负责的位转换。

## 扩展指南

- 新增或重编号操作时，同时修改 `AssertionOp` 常量和 `ApplyAssertionOp`，并首先确认 `pkg/kv/assertion.go`、存储客户端 flags 编码及所有驱动转换的兼容性；数值是跨实现契约，不能只在 Rust 侧调整。
- 修改断言位布局时，应同步 `pkg/kv/keyflags.rs` 的常量与四个查询方法，并检查生产调用方的底层 `u8` 存储。保留其他位是不变量，应继续用带非断言位的用例验证。
- 若要加入“清除断言”，不能复用 `AssertNone`，因为当前 Go/Rust 契约明确为 no-op；需要新增显式操作并评估事务不可变性规则。
- 测试应继续放在独立文件：穷举转换规则更新 `pkg/kv/assertion_test.rs`，跨标志位状态迁移更新 `pkg/kv/assertion_1_aster_unit_test.rs`；若更改业务不可变策略，还应同步表层独立测试及 Go 对照测试意图。
- 注意 `Default` 当前产生 `AssertExist`。若希望默认表示 none，应采用显式构造或先完成跨语言兼容评估，不能只更改派生实现。
