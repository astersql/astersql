# `pkg/parser/util/hash64.rs`

## 文件定位

`hash64.rs` 位于 `astersql-parser-util` crate 中，是解析器公共工具层的 64 位增量哈希协议定义。crate 根 `pkg/parser/util/lib.rs` 通过 `pub mod hash64` 声明模块，并以 `pub use hash64::*` 再导出其中的 `IHasher`；`pkg/parser/util/Cargo.toml` 将 `lib.rs` 指定为库入口，且没有为本模块声明 feature 或第三方依赖。

本文件不是哈希算法实现，也不创建哈希器。它把“按类型、按顺序向一个有状态哈希器写值，最后读取 64 位摘要”的能力抽象为 trait，供需要稳定结构哈希的上层对象使用。RustCodeGraph 的文件关系显示当前 Rust 生产代码中的直接使用文件为 `pkg/parser/types/field_type.rs`。

## 核心职责

- 定义公开 trait `IHasher`，统一布尔值、整数、浮点数、字符、字符串和原始字节的增量写入入口。
- 用 `&mut self` 明确所有 `Hash*` 方法和 `Reset` 都可能改变累积状态；用 `&self` 表明 `Sum64` 只观察当前摘要。
- 保留 Go `pkg/parser/util/hash64.go::IHasher` 的方法面和值类型映射，使迁移后的结构哈希调用可以维持原来的字段顺序和类型边界。
- 将编码、分隔、具体混合算法和初始种子留给实现者。本文件本身不承诺 FNV、xxHash 等任何具体算法，也不保证不同实现会产生相同摘要。

## 主要符号

文件中只有一个公开 trait，没有常量、结构体、枚举、自由函数、实现块或条件编译项。

- `pub trait IHasher`：可作为 `dyn IHasher` 使用的增量 64 位哈希接口。
- `HashBool(&mut self, bool)`：写入布尔值。
- `HashInt(&mut self, isize)`：承接 Go `int`；位宽随 Rust 目标平台指针宽度变化。
- `HashInt64(&mut self, i64)` 与 `HashUint64(&mut self, u64)`：分别写入固定宽度的有符号和无符号 64 位整数。
- `HashFloat64(&mut self, f64)`：写入双精度浮点值；NaN、正负零及字节序如何编码由实现决定。
- `HashRune(&mut self, i32)`：承接 Go `rune` 的 `int32` 值域；trait 本身没有限制传入值必须是有效 Unicode 标量值。
- `HashString(&mut self, &str)`：借用并写入有效 UTF-8 字符串。
- `HashByte(&mut self, u8)` 与 `HashBytes(&mut self, &[u8])`：写入单字节或任意字节切片，借用接口不要求复制。
- `Reset(&mut self)`：请求实现清空累积状态以便复用。
- `Sum64(&self) -> u64`：读取当前摘要，不取得所有权，也不通过类型签名改变状态。

方法名保留 Go 风格的大写形式；crate 根的 `#![allow(non_snake_case)]` 允许这一迁移接口保持原名。

## 执行流程

本文件没有独立可执行流程，实际流程由调用者和 trait 实现共同组成：

1. 上层取得一个具体哈希器的可变引用，必要时擦除为 `&mut dyn IHasher`。
2. 上层以能表达字段类型的方法逐项写入，并自行维护字段顺序、长度前缀或其他消歧信息。
3. 具体实现把每次写入编码并混入自己的内部状态；`IHasher` 不规定编码规则。
4. 调用者通过 `Sum64` 读取摘要；若复用实例，则调用 `Reset` 后开始下一轮写入。

当前直接生产示例是 `pkg/parser/types/field_type.rs::FieldType::Hash64`。它依次写入 `tp`、`flag`、`flen`、`decimal`、`charset`、`collate`，再为 `elems` 和 `elemsIsBinaryLit` 分别先写长度、后写元素，最后写 `array`。因此结构哈希的稳定性不仅依赖具体 `IHasher` 实现，也依赖调用者保持字段类型和写入次序不变。

## 数据与状态

`IHasher` 自身不拥有字段；状态完全存在于具体实现中。trait 的借用关系给出以下约束：

- 九个 `Hash*` 方法通过独占可变借用串行更新同一实例。
- `HashString` 和 `HashBytes` 只在调用期间借用输入，trait 签名不允许实现把短生命周期引用直接保存为自身借用状态。
- `Sum64` 使用共享借用，可以在不消费哈希器的情况下读取摘要；签名层面没有自动重置行为。
- `Reset` 允许复用实例，但“恢复到何种初始种子”属于实现契约，本文件没有给出默认状态或默认实现。

`HashInt(isize)` 是平台相关宽度，若摘要必须跨 32/64 位目标完全一致，应在实现和跨平台测试中显式验证，或由调用方选择固定宽度入口。字符串与字节序列之间、相邻字段之间是否加类型标记或长度分隔也不是 trait 的保证；调用者需要像 `FieldType::Hash64` 那样在可变长集合前显式写长度，具体实现则必须维持项目所需的编码兼容性。

## 依赖与调用关系

- 模块装配：`pkg/parser/util/lib.rs` 声明并再导出 `hash64`，使使用方可通过 parser util crate 取得 `IHasher`。
- crate 边界：`pkg/parser/util/Cargo.toml` 定义 `astersql-parser-util`，其 manifest 当前没有 `[dependencies]` 或 feature 配置；本文件只使用 Rust 原生标量、切片、字符串切片和 trait 机制。
- Rust 生产调用边：RustCodeGraph 报告 `pkg/parser/types/field_type.rs` 使用本文件；`FieldType::Hash64(&self, h: &mut dyn util::IHasher)` 调用 `HashByte`、`HashUint64`、`HashInt`、`HashString` 和 `HashBool`。
- Rust 测试调用边：`pkg/parser/util/escape_test.rs` 的 `StateHasher` 和 `pkg/parser/util/migration_aster_unit_test.rs` 的 `RecordingHasher` 实现该 trait，并分别直接调用全部写入方法、`Sum64` 和 `Reset`。
- Go 侧更广的相邻调用：`pkg/parser/types/field_type.go::FieldType.Hash64`、`pkg/parser/ast/dml.go::SelectLockInfo.Hash64` 和 `pkg/parser/ast/model.go::CIStr.Hash64` 接受 Go `util.IHasher`。其中后两个没有出现在当前 Rust `IHasher` 的生产调用边中，不能据此声称 Rust 侧已完成同等接线。

本 trait 不调用下游函数；所谓“被调用者”是各个方法的具体实现，必须由实现类型提供。

## 错误处理与边界

所有方法都不返回 `Result`，因此协议没有可传播的编码或写入错误。实现若内部可能失败，必须在选择实现策略时另行处理；不能通过本 trait 将失败返回给调用者。

需要特别维护的边界包括：`isize` 的平台宽度，`i64::MIN`/`u64::MAX`，浮点数的位级特殊值，`i32` 中并非有效 Unicode 标量的数值，空字符串、空字节切片、含零字节的数据以及字段拼接歧义。trait 只区分调用了哪个方法，不规定这些输入的字节表示，也不规定哈希碰撞后的等价判定策略。64 位摘要天然可能碰撞，上层若把它用于等价类或缓存键，应保留必要的结构相等复核，而不能把摘要相等视为绝对等价。

## 并发与资源生命周期

接口没有锁、原子变量、任务、通道、I/O 资源或事务。`&mut self` 保证单次安全 Rust 调用期间对同一实例具有独占访问权，但 `IHasher` 没有 `Send`/`Sync` 超 trait，因此不能据此推断实现可在线程间移动或共享。是否需要同步、堆分配或外部资源，完全取决于具体实现。

输入 `&str`/`&[u8]` 的借用仅覆盖方法调用；哈希器实例则由调用方拥有并决定创建、复用和销毁时机。`Sum64` 不消费实例，`Reset` 是显式生命周期分界。若多个逻辑对象复用同一实例，遗漏 `Reset` 会把前一对象状态带入后一对象摘要。

## 与 Go 版本的对应关系

Rust `pkg/parser/util/hash64.rs::IHasher` 逐项对应 Go `pkg/parser/util/hash64.go::IHasher`：`bool` 对应 `bool`，`isize` 对应 `int`，`i64`/`u64` 对应 `int64`/`uint64`，`f64` 对应 `float64`，`i32` 对应 `rune`，`&str` 对应 `string`，`u8` 对应 `byte`，`&[u8]` 对应 `[]byte`，并保留 `Reset` 与 `Sum64`。

主要语言差异是 Rust 显式表达可变性与借用：Go 接口方法没有在签名中标出接收者状态变化，而 Rust 的写入和重置方法要求 `&mut self`；Go 切片和字符串按值传递其描述符，Rust 则显式短借用；Go `int` 与 Rust `isize` 都随目标位宽变化。Rust 还可通过 `&mut dyn IHasher` 进行动态分派，`migration_aster_unit_test.rs` 对此有直接覆盖。

Go 文件同样只声明接口，不提供算法。Rust 当前可确认的生产接线只覆盖 `FieldType::Hash64`；Go 中接受 `util.IHasher` 的 AST 方法不能自动视为 Rust 已迁移功能。

## 扩展指南

- 新增哈希值类型时，应同时评估并同步 `pkg/parser/util/hash64.go::IHasher`、本 trait、所有 Rust 实现和调用方；优先扩展现有独立测试文件，不要把测试内嵌进 `hash64.rs`。
- 修改既有方法的类型、编码含义或调用顺序属于摘要兼容性变化。先盘点 `FieldType::Hash64` 等调用者以及真实算法实现，再决定是否需要版本化缓存键或清理持久化摘要。
- 新增具体实现时，应放在独立生产文件，并用独立测试覆盖所有类型的确定性编码、空值与极值、浮点位模式、字段边界消歧、`Sum64` 非破坏读取和 `Reset` 恢复初始状态。
- 若要求跨平台稳定，避免直接把原生 `isize` 内存表示作为协议；应明确整数宽度、字节序、字符串/切片长度编码及浮点数规范，并增加 32/64 位目标证据。
- 若用于并发路径，应由具体实现明确 `Send`/`Sync` 和同步策略；不要仅凭本 trait 假设线程安全。
- 与 Go 继续对齐时，先核对 Go 的真实调用者和具体 `cascades/base.Hasher` 行为。本文件注释表达的是接口用途，不构成 Rust 已有完整算法实现的证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/parser/util/hash64.rs`；`files --filter pkg/parser/util` 显示该文件及相邻 Go、模块入口和两个 Rust 测试文件；`node --file pkg/parser/util/hash64.rs` 列出完整 58 行并报告由 `pkg/parser/types/field_type.rs` 使用；`query IHasher --kind trait` 定位 trait、测试和 Go/Rust 调用符号；`node IHasher --file pkg/parser/util/hash64.rs` 核对全部方法签名。`callers`/`callees` 查询在本地 30 秒窗口内超时，调用关系因此另以文件级图结果和源码搜索交叉核验。
- 源码：`pkg/parser/util/hash64.rs`（trait 方法面、借用与返回类型）；`pkg/parser/util/lib.rs`（模块声明、再导出及命名规则）；`pkg/parser/types/field_type.rs::FieldType::Hash64`（当前 Rust 生产写入顺序）。
- crate：`pkg/parser/util/Cargo.toml`（crate 名、`lib.rs` 入口、Go 包迁移元数据以及无额外依赖/feature 的事实）。
- Go 对照：`pkg/parser/util/hash64.go`（同名接口）；`pkg/parser/types/field_type.go::FieldType.Hash64`（字段顺序对齐）；通过 `rg` 定位的 `pkg/parser/ast/dml.go::SelectLockInfo.Hash64` 与 `pkg/parser/ast/model.go::CIStr.Hash64`（Go 侧相邻使用范围）。
- 独立 Rust 测试：`pkg/parser/util/escape_test.rs::test_ihasher_state_updates_and_reset` 以 `StateHasher` 覆盖全部写入方法、极值、UTF-8、原始字节、摘要读取和重置；`pkg/parser/util/migration_aster_unit_test.rs::ihasher_preserves_go_method_surface_and_mutation_protocol` 以 `RecordingHasher` 覆盖 `dyn IHasher` 动态分派、调用计数及重置。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时使用任务指定的结构命令确认目标文档存在且恰有十一个固定二级标题，并人工复核只修改本文档（完成后按任务协议删除任务文件）。
