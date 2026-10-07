# `pkg/executor/join/tagged_ptr.rs` 逻辑说明

## 文件定位

`tagged_ptr.rs` 属于 Cargo crate `astersql-executor-join`；crate 根由 `pkg/executor/join/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/join/lib.rs` 通过 `pub mod tagged_ptr` 暴露本模块，并在 `#[cfg(test)]` 下挂接独立测试 `tagged_ptr_test.rs`。本文件不依赖 crate 的外部依赖，只使用整数位运算、`std::mem::size_of` 和 `std::ffi::c_void`。

它是 Hash Join v2 的底层 tagged pointer（带标签指针）工具：利用 64 位地址通常未占用的高位保存哈希值高位片段，使桶头或冲突链节点能够在访问行数据前先做一次廉价过滤。Rust 当前接线只覆盖一部分：`join_row_table.rs` 使用 `TaggedPtr`、`TagPtrHelper::get_tagged_value` 和 `get_tagged_bits_from_uintptr`；`TagPtrHelper::to_tagged_ptr`、`to_unsafe_pointer` 的非测试 Rust 调用尚未出现。完整生产用法可在 Go 对照 `tagged_ptr.go`、`hash_table_v2.go`、`hash_join_v2.go` 和各 probe 文件中看到，不能将该 Go 接线直接视为 Rust 已完成的能力。

## 核心职责

- 定义可嵌入 tag 的上限 `MAX_TAGGED_BITS = 24` 和对应的低 40 位掩码 `MAX_TAGGED_MASK`。
- 用机器字整数别名 `TaggedPtr = usize` 表示“地址位 + 高位 hash tag”，避免把含 tag 的值当作可解引用裸指针。
- 由 `TagPtrHelper` 保存当前实例采用的高位掩码，并提供掩码初始化、hash tag 提取、地址与 tag 合并、tag 清除四项操作。
- 由 `get_tagged_bits_from_uintptr` 根据给定地址的前导零估计可用高位数，并将其限制为 24 位；非 64 位目标直接禁用 tagged bits。

本文件只封装表示和位运算，不分配内存、不维护 Hash Join 桶、不遍历冲突链，也不解引用传入地址。地址有效性、tag 位确实为空、对象生命周期和并发发布均由调用方负责。

## 主要符号

- `MAX_TAGGED_BITS: i8`：最多使用 24 个最高位，限制 tag 宽度并保留至少 40 个低位给地址。
- `MAX_TAGGED_MASK: u64`：值为 `0xffffffffff`，表示保留地址低 40 位；当 `init(24)` 时，`tagged_mask == !MAX_TAGGED_MASK`，这一关系由 `tagged_ptr_test.rs::tag_helper_masks_only_available_high_bits` 验证。
- `TAGGED_POINTER_LEN: i64`：`TaggedPtr` 的机器字字节数，用于与 Go `unsafe.Sizeof(taggedPtr(0))` 对齐。Rust 的 `hash_table_v2.rs` 另有同名本地常量用于内存估算，当前不是从本模块引用。
- `TaggedPtr = usize`：保存 tagged 地址的整数形态。类型别名不提供额外类型隔离，普通 `usize` 仍可无检查地传入相关 API。
- `TagPtrHelper { pub tagged_mask: u64 }`：唯一状态是高位掩码。字段公开，调用方可以绕过 `init` 写入任意掩码，因此正确性依赖调用约定。
- `TagPtrHelper::init(tagged_bits)`：构造最高 `tagged_bits` 位为 1 的掩码；对 0 显式写入零。正常输入来自 `get_tagged_bits_from_uintptr`，范围为 `0..=24`。
- `TagPtrHelper::get_tagged_value(hash_value)`：返回 `hash_value & tagged_mask`，结果仍位于原始最高位位置，不会右移成紧凑整数。
- `TagPtrHelper::to_tagged_ptr(tagged_value, ptr)`：把裸指针地址转为整数后与 tag 做按位或。它不主动把 `tagged_value` 限制到 `tagged_mask`，也不检查地址高位是否为空。
- `TagPtrHelper::to_unsafe_pointer(t_ptr)`：用 `!tagged_mask` 清除高位 tag 后转回 `*mut c_void`；只做转换，不解引用。
- `get_tagged_bits_from_uintptr(ptr)`：64 位平台取 `leading_zeros(ptr)` 与 24 的较小值，其他字长返回 0。

文件没有 trait、enum、宏或条件编译项；以上常量、类型、结构体、方法和自由函数均为公开符号。

## 执行流程

典型流程由本文件符号和 Go 生产接线共同界定：

1. 调用方取得将被保存的地址范围，用代表该范围的地址整数计算可用高位数。Rust `RowTableSegment::init_tagged_bits` 当前以 `rows.as_ptr()` 调用 `get_tagged_bits_from_uintptr`；Go `rowTableSegment.initTaggedBits` 则把首尾行地址按位或后计算，以覆盖整段地址范围。
2. 全局或分区构建阶段选取所有 segment 都能支持的最小位数，再调用 `TagPtrHelper::init` 固化 `tagged_mask`。这一步在 Go `hashTableContext.mergeRowTablesToHashTable` 已接线；Rust 生产代码中尚未找到对应的 helper 初始化链。
3. 建表时，对完整 hash 调用 `get_tagged_value`，再以 `to_tagged_ptr` 将 tag 与行地址合并。若桶内已有节点，Go `subTable.updateHashValue` 还把前一桶头的 tag 位并入新值，并把旧桶头写成行内 next 指针。
4. probe 时先比较 tagged pointer 的高位和目标 hash 的高位；匹配才继续访问候选行。Rust `RowTableSegment::get_next_row_address` 已实现 next 值的 tag 比较，但 Rust probe 中可见的相应链式调用仍为注释代码。
5. 真正访问行前，通过 `to_unsafe_pointer` 清除 tag，恢复原地址。该方法当前只由 `tagged_ptr_test.rs::tagged_pointer_round_trip_preserves_real_allocation` 在 Rust 中执行验证。

## 数据与状态

`TagPtrHelper` 的状态只有 `tagged_mask`。掩码为零表示禁用 tag：`get_tagged_value` 总是返回 0，`to_tagged_ptr` 仅保留地址整数，`to_unsafe_pointer` 不清除任何位。掩码非零时，tag 区与地址区必须互不重叠；本文件不会维护或证明这一不变量。

`TaggedPtr` 采用 `usize`，而位运算先转换为 `u64`。`get_tagged_bits_from_uintptr` 明确在非 64 位目标返回 0，使公共计算路径退化为无 tag 模式；`TAGGED_POINTER_LEN` 则始终反映实际 `usize` 宽度。`TagPtrHelper::init` 本身没有平台检查，调用方若在非 64 位目标手工传入非零位数，依然会生成 64 位掩码，转换回 `usize` 时可能截断。

地址和 tag 都按值传递，本模块不保存裸指针，也不拥有其指向的数据。`join_row_table.rs::RowTableSegment` 以 `Vec<Option<TaggedPtr>>` 保存冲突链 next 值，并以 `tagged_bits` 保存 segment 估算结果，这是当前 Rust 侧与本模块直接关联的持久状态。

## 依赖与调用关系

上游 Rust 关系：

- `pkg/executor/join/lib.rs` 声明公开模块，并仅在测试配置下声明 `tagged_ptr_test`。
- `pkg/executor/join/join_row_table.rs` 导入 `TagPtrHelper`、`TaggedPtr`；`RowTableSegment::init_tagged_bits` 调用 `get_tagged_bits_from_uintptr`，`get_next_row_address` 调用 `get_tagged_value` 并读取 `tagged_mask`。
- `pkg/executor/join/row_table_builder.rs` 调用 `RowTableSegment::init_tagged_bits` 和 `set_next_row_address`，因此间接使用本模块的位数估算和 `TaggedPtr` 存储形态。
- `pkg/executor/join/tagged_ptr_test.rs` 是当前覆盖全部核心 API 的直接调用者。

RustCodeGraph 的文件节点把 `tagged_ptr.rs` 的文件级使用者只报告为 `tagged_ptr_test.rs`，且精确 method callers/callees 未返回边；源码精确符号搜索补充确认了上述 `join_row_table.rs` 生产引用。这说明图的文件/方法边在此处不完整，文档没有据此声称生产调用不存在。

下游依赖全部来自标准库：`usize`/`u64` 转换、`u64::leading_zeros`、`std::mem::size_of` 和 `std::ffi::c_void`。`Cargo.toml` 没有为 tagged pointer 单独声明 feature 或第三方依赖；crate 的大量 Windows 条件依赖与本文件的位运算无直接关系。

## 错误处理与边界

API 不返回 `Result`，也没有显式运行时错误类型；失败模式体现为调用约定被破坏、调试构建 panic 或地址损坏：

- `init` 的有效输入应是 `0..=24`。方法未自行限制参数；过大的 `tagged_bits` 可能触发移位溢出或 `64 - tagged_bits` 下溢。安全扩展时应保持输入来自 `get_tagged_bits_from_uintptr`，或在公共边界新增校验并同步测试。
- `to_tagged_ptr` 要求 `tagged_value` 只包含 `tagged_mask` 允许的位，且原地址在这些位上为零。否则按位或会混淆地址和 tag，随后清位无法恢复原地址。
- `to_unsafe_pointer` 可接受零并返回空指针，但任何解引用责任都在调用方；传入任意整数可能产生悬垂、未对齐或无效地址。
- 非 64 位平台由 `get_tagged_bits_from_uintptr` 返回 0，但直接调用 `init(nonzero)` 不受保护。
- 当前 Rust `RowTableSegment::get_next_row_address` 使用“pointer 的 tag 区完全等于 hash tag”判断；Go `getNextRowAddress` 使用 `uint64(ret) & hashTagValue == hashTagValue`。两者在 tag 含额外置位时可能不同，迁移后续接线前需要确认这是有意修正还是尚未对齐。
- Rust segment 只用 `rows.as_ptr()` 估算前导零；Go 使用首尾行地址的按位或。若一个分配跨越更高地址位，Rust 估算可能比 Go 乐观，生产化前应补齐范围验证。

## 并发与资源生命周期

本模块没有锁、原子操作、线程或异步任务。`init` 需要 `&mut self`，初始化完成后的读取/转换方法只需 `&self`；如果 helper 在初始化后不再变更，其值语义适合由调用方共享，但公开的 `tagged_mask` 仍允许外部可变访问，调用方必须自行建立发布和不可变约束。

`to_tagged_ptr` 和 `to_unsafe_pointer` 不拥有、借用追踪或释放指针目标。整数化会脱离 Rust 生命周期系统：原分配必须在 tagged 值存活和恢复期间保持地址稳定，不能在此期间释放、移动或重新分配。`tagged_ptr_test.rs` 的 round-trip 测试用局部 `Box<u64>` 保持分配存活，并只在恢复后解引用，这是正确生命周期的最小示例。

Go `hash_table_v2.go::atomicUpdateHashValue` 在并发建表时用原子 CAS 更新桶头；该并发协议属于调用方哈希表，而不是本文件。Rust 当前没有从 `TagPtrHelper::to_tagged_ptr` 接到同等原子建表路径的直接证据，不能仅凭这些纯函数推断 Hash Join v2 的并发链已完成。

## 与 Go 版本的对应关系

主体为逐符号移植：Rust `MAX_TAGGED_BITS`、`MAX_TAGGED_MASK`、`TAGGED_POINTER_LEN`、`TaggedPtr`、`TagPtrHelper` 及其四个函数/方法，分别对应 `tagged_ptr.go` 中的小写同名概念。Rust 以 `usize`/裸指针转换代替 Go 通过 `unsafe.Pointer` 写入 `uintptr` 的实现，但共同目标都是不把含 tag 的整数作为 GC/运行时可追踪指针。

测试意图也基本对齐：

- Rust `tagged_bits_follow_machine_pointer_leading_zeros` 对应 Go `TestTaggedBits`，并额外覆盖非 64 位返回 0 的分支。
- Rust `tag_helper_init_matches_all_go_masks` 对应 Go `TestTagHelperInit`，覆盖 24 到 0 的全部掩码。
- Rust `tagged_pointer_round_trip_preserves_real_allocation` 对应 Go `TestTagHelper` 的地址/tag 往返；Go 用 10 MiB 切片的首尾地址，Rust 用一个 `Box<u64>`，因此 Rust 测试未覆盖跨较大分配范围的首尾地址约束。

可见差异包括：Rust `init(0)` 有显式分支以避免 64 位移位；Rust 类型和字段是公开 API，而 Go 符号是包内私有；Rust 当前生产接线不完整；`join_row_table.rs` 的 tag 比较和地址范围估算与 Go 存在前述语义差异。另有一处文档漂移：`lib.rs` 注释称在“指针低位”嵌入信息，实际 `tagged_ptr.rs` 和 Go 均使用最高位。

## 扩展指南

若要把 Rust Hash Join v2 的 tagged pointer 链补全，最可能修改或调用的入口是 `TagPtrHelper::init`、`get_tagged_value`、`to_tagged_ptr`、`to_unsafe_pointer`，以及 `join_row_table.rs::RowTableSegment::{init_tagged_bits,get_next_row_address}`。应优先复用现有 API，不在 `hash_table_v2.rs` 另建不一致的编码规则。

扩展时需要维护以下不变量：所有被编码地址在统一 `tagged_mask` 位上均为零；helper 位数取所有相关分配可用位数的下界；传入 tag 已被 mask；桶头和 next 链使用同一 helper；恢复地址前清除 tag；分配在链使用期间不移动、不释放。并发桶更新还必须在哈希表层定义清楚原子顺序，不能由本文件的普通整数转换替代。

测试应继续放在独立文件 `pkg/executor/join/tagged_ptr_test.rs`，不要内嵌到生产源文件。至少同步覆盖：0/1/24 位掩码、非法大位数的预期策略、tag 外位输入、空指针、多个真实分配地址或首尾范围、round-trip、非 64 位退化，以及与 Go 链表 tag 判断的契约。若接入建表/probe，还应同步 `hash_table_v2_test.rs`、`join_row_table_test.rs` 和相应 probe 独立测试。兼容性风险集中在地址宽度与平台 ABI；正确性风险是错误清位产生无效地址；性能风险是位数过少降低预过滤效果，而额外边界检查可能进入热点路径。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含 11,467 个文件，`files --filter pkg/executor/join` 收录目标及 Go/Rust 对照；`node --file pkg/executor/join/tagged_ptr.rs` 核对了完整 90 行源码和符号；`query TagPtrHelper`、`query get_tagged_bits_from_uintptr` 定位 Rust/Go 定义；精确 callers/callees 未返回方法边，故以源码符号搜索补充。
- Rust 源与模块边界：`pkg/executor/join/tagged_ptr.rs`、`pkg/executor/join/lib.rs`、`pkg/executor/join/join_row_table.rs`、`pkg/executor/join/row_table_builder.rs`。
- Cargo 边界：`pkg/executor/join/Cargo.toml`；crate 名为 `astersql-executor-join`，根文件为 `lib.rs`，无 tagged-pointer 专属 feature 或外部依赖。
- Go 对照：`pkg/executor/join/tagged_ptr.go`、`join_row_table.go`、`hash_table_v2.go`、`hash_join_v2.go`，以及各 probe 文件的 `toUnsafePointer` 调用。
- 独立测试：`pkg/executor/join/tagged_ptr_test.rs` 和 `pkg/executor/join/tagged_ptr_test.go`；额外直接接线证据来自 `join_row_table_test.rs`、`hash_table_v2_test.rs`。
- 人工复核结论：本文件存在于 Hash Join v2 的地址压缩/预过滤边界；自身只负责位表示，安全依赖地址高位、统一掩码和外部生命周期；Rust 已有局部 row-table 接线但尚不能据此认定完整 Go 主链已移植。
