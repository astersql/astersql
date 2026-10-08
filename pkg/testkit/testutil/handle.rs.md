# `pkg/testkit/testutil/handle.rs`

## 文件定位

本文件属于 `astersql-testkit-testutil` 测试辅助 crate，而不是 SQL 请求运行时的生产主链。crate 根 `pkg/testkit/testutil/lib.rs` 在私有 `handle` 模块中通过 `include!("handle.rs")` 编译本文件，并以 `pub use handle::*` 将两个函数暴露为 crate 根 API。`pkg/testkit/testutil/Cargo.toml` 的 `[lib] path = "lib.rs"` 以及 `codec-dependency`、`kv-dependency`、`mysql-dependency` 三个路径依赖共同确定了这一边界。

文件只定义 `MustNewCommonHandle` 和 `MaskSortHandles` 两个公开、同步、无条件编译的函数；没有类型、trait、模块级可变状态或条件编译项。它存在的目的，是让 Rust 测试使用与 Go `pkg/testkit/testutil/handle.go` 相同的 common handle 构造和分片行号比较语义，而不用在各测试中重复编码细节。

## 核心职责

- `MustNewCommonHandle`：把一组 `&dyn Any` 测试值转换成 `Datum`，按可比较 key 格式编码，再构造成实现 `kv::Handle` 的 `CommonHandle`。成功结果被装箱为 `Box<dyn kv::Handle>`，方便调用者按统一 handle 接口检查列数、编码列和字符串表示（`handle.rs:24-28`；`migration_aster_unit_test.rs:33-49`）。
- `MaskSortHandles`：根据 MySQL 字段类型的默认物理位宽、符号位和 `shard_row_id_bits` 计算要丢弃的高位数，通过左右算术移位保留未分片的低位，随后按 `i64` 数值升序排列（`handle.rs:35-59`）。它用于测试 `auto_random` 生成的物理 handle 是否在移除随机分片位后仍连续、单调或符合 rebase 预期。

这两个函数均是测试断言前的数据准备工具：前者把人类可读的列值转换为真实 handle 表示，后者把带随机高位的整数 handle 归一化为可比较序列。

## 主要符号

### `pub fn MustNewCommonHandle(values: Vec<&dyn Any>) -> Box<dyn kv::Handle>`

输入是借用的动态值集合，函数不保存这些引用。`types::MakeDatums` 对每项调用 `NewDatum`；随后 `codec::EncodeKey(chrono_tz::UTC, Vec::new(), ...)` 从空缓冲区生成可比较编码；最后 `kv::NewCommonHandle` 扫描编码并记录每列累计结束位置。两个可失败步骤都使用 `expect`，因此 API 的“Must”语义是失败即 panic，而不是返回 `Result`（`handle.rs:24-28`）。

返回值擦除具体的 `kv::CommonHandle` 类型，只承诺 `kv::Handle` trait。现有回归测试证明 `i64` 与 `String` 两列会被完整保留，结果 `IsInt() == false`、`NumCols() == 2`，并可逐列解码回 `100` 和 `"abc"`（`pkg/testkit/testutil/migration_aster_unit_test.rs:33-49`）。

### `pub fn MaskSortHandles(handles: Vec<i64>, shardBitsCount: isize, fieldType: u8) -> Vec<i64>`

函数消费输入向量，并返回新排序向量。它在 `mysql::DefaultLengthOfMysqlTypes` 中按类型码查找字节长度并乘以 8 得到 `typeBitsLength`；计算式为 `64 - typeBitsLength + shardBitsCount + 1`，最后的 `1` 是符号位。每个 handle 经 `(handle << shift) >> shift` 处理；右移为 `i64` 算术右移，所以被保留低位中的符号会扩展，负数低位语义得以保留。排序使用 `sort_unstable`，仅保证数值顺序，不保证相等元素的原始次序（`handle.rs:37-59`）。

## 执行流程

`MustNewCommonHandle` 的主流程如下：

1. 调用者组装 `Vec<&dyn Any>`；例如测试传入对 `100_i64` 和 `String("abc")` 的引用。
2. `types::MakeDatums` 逐项调用 `types::NewDatum`，把动态值映射为内部 `Datum`（`pkg/types/datum.rs:2393-2399,2537-2541`）。
3. `codec::EncodeKey` 使用 UTC、空输出缓冲区和当前全局新 collation 开关编码所有 Datum；实际入口委托给 `Encoder::EncodeKey` 的 comparable 模式（`pkg/util/codec/codec.rs:147-156,406-414`）。
4. `kv::NewCommonHandle` 保存编码，短于 9 字节时为 handle 表示补零，并用 `codec::CutOne` 扫描原始编码以建立列边界（`pkg/kv/key.rs:227-255`）。
5. 将具体 handle 装箱并作为 trait object 返回。

`MaskSortHandles` 的主流程如下：

1. 从 `DefaultLengthOfMysqlTypes` 查找字段类型位宽。当前表包含 `TypeLong = 4` 字节、`TypeLonglong = 8` 字节等固定长度类型（`pkg/parser/mysql/const.rs:333-351`）。
2. 将 64 位容器超出字段宽度的高位、分片高位以及符号位合并为 `shiftBitsCount`。
3. 断言移位数非负；若移位数达到或超过 64，显式把该 handle 归一化为 `0`，避免 Rust 的越界移位。
4. 对其余值先左移清除高位，再算术右移恢复低位有符号值。
5. 对归一化结果执行不稳定升序排序并返回。

## 数据与状态

本文件没有全局可变状态。`MustNewCommonHandle` 的中间状态只有拥有所有权的 `Vec<Datum>`、编码 `Vec<u8>` 和最终 `CommonHandle`；输入引用只在调用期间读取。`CommonHandle` 内部保存编码字节和每列的累计结束偏移，偏移由 `NewCommonHandle` 构造（`pkg/kv/key.rs:227-255`）。

`MaskSortHandles` 的状态局限于局部标量 `typeBitsLength`、`shiftBitsCount` 和新建的 `ordered: Vec<i64>`。输入 `handles` 被消费，因此调用后不能复用原向量；该设计与 Go 版本“分配同长度新切片、不原地修改调用者切片”的可观察效果一致。空间复杂度为 O(n)，排序时间为 O(n log n)；逐项掩码为 O(n)。

掩码不变量是：对于已知固定宽度字段类型，结果只取最低 `typeBitsLength - 1 - shardBitsCount` 位，并将该范围的最高位视为符号位。`migration_aster_unit_test.rs:51-61` 分别以 64 位 `TypeLonglong` 和 32 位 `TypeLong` 验证了高位分片被去除且低位 `[3,1,2]` 排序为 `[1,2,3]`。

## 依赖与调用关系

上游装配边为 `pkg/testkit/testutil/lib.rs -> include!("handle.rs") -> pub use handle::*`。RustCodeGraph 将本文件识别为 3 个节点（文件加两个函数），并报告直接使用文件 `pkg/executor/test/autoidtest/autoid_test.rs`；源码检索还确认 crate 内单元测试 `pkg/testkit/testutil/migration_aster_unit_test.rs` 调用两个函数。

已确认的 Rust 上游调用场景：

- `migration_aster_unit_test.rs:33-61` 直接验证 common handle 编解码以及 32/64 位掩码排序。
- `pkg/executor/test/autoidtest/autoid_test.rs:609-639` 在真实 TestKit/session 的 allocator rename、auto-increment rebase、auto-random rebase 场景中调用 `MaskSortHandles`，检查移除 5 个分片位后的 row ID。

直接下游依赖边为：

- `MustNewCommonHandle -> types::MakeDatums -> types::NewDatum`；
- `MustNewCommonHandle -> codec::EncodeKey -> Encoder::EncodeKey`；
- `MustNewCommonHandle -> kv::NewCommonHandle -> codec::CutOne`；
- `MaskSortHandles -> mysql::DefaultLengthOfMysqlTypes`，以及标准库迭代、移位和 `Vec::sort_unstable`。

RustCodeGraph 的 `callers/callees` 裸名查询因 Go/Rust 同名符号而未生成边；以上边均由其 `node --file` 源码视图、crate 装配文件和精确源码引用交叉核验，没有据此推断未出现的调用者。

## 错误处理与边界

`MustNewCommonHandle` 不返回错误。`codec::EncodeKey` 遇到不支持的 Datum kind 等问题会返回错误，本函数以消息 `encode common handle values` panic；`kv::NewCommonHandle` 无法用 `CutOne` 切分编码时，以 `construct common handle` panic。因此它只适合测试路径，不适合需要恢复或向用户返回诊断信息的生产路径（`handle.rs:25-27`；`pkg/util/codec/codec.rs:133-144`；`pkg/kv/key.rs:244-255`）。此外，动态值是否受支持由 `types::SetValueWithDefaultCollation` 决定，本文件不做预检。

`MaskSortHandles` 的边界行为需要调用者显式知晓：

- 未在 `DefaultLengthOfMysqlTypes` 中找到的 `fieldType` 得到位宽 `0`；此时移位数至少为 65，函数会把所有元素映射为 `0`。这不是错误返回，也未证明是 Go 侧对未知类型的等价行为。
- `shiftBitsCount < 0` 会触发 `assert!(..., "negative shift count")`。这可由字段位宽/分片参数不合法造成。
- `shiftBitsCount >= 64` 时返回零值而不执行越界移位。这是 Rust 为定义安全边界而增加的分支；Go 源码直接按运行时移位规则计算。
- 空输入自然返回空向量；相同归一化值的相对次序没有契约，因为使用 `sort_unstable`。
- 函数不验证 `shardBitsCount` 是否符合数据库 DDL 对 `shard_row_id_bits` 的合法范围；调用者负责传入与字段类型一致的值。

## 并发与资源生命周期

两个函数均无锁、无通道、无异步任务、无 I/O，也不缓存跨调用状态；只要下游全局 collation 配置在测试期间保持稳定，它们本身可被并发调用。`MustNewCommonHandle` 会读取 `codec::EncodeKey` 使用的全局新 collation 开关，因此编码结果间接受该全局配置影响；本文件既不修改也不同步该状态（`pkg/util/codec/codec.rs:406-414`）。

资源均由 Rust 所有权自动回收：`MustNewCommonHandle` 消费临时 Datum/编码向量并把 handle 所有权交给返回的 `Box`；`MaskSortHandles` 消费输入向量、建立结果向量并转移给调用者。没有显式关闭、回滚或清理阶段。传给 `MustNewCommonHandle` 的借用值只需活到函数返回，不会被 handle 保留。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/testkit/testutil/handle.go`。函数结构保持一致：Go 和 Rust 都先 `MakeDatums`、再 `EncodeKey`、再 `NewCommonHandle`；掩码排序也都使用 `64 - typeBitsLength + shardBitsCount + 1` 的移位数，并对归一化结果升序排序。

已验证的差异如下：

- Go `MustNewCommonHandle(t, values ...any)` 接收 `*testing.T`，用 `require.NoError` 将失败归因到测试；Rust 不接收测试上下文，使用两次 `expect` 直接 panic，并以 `Vec<&dyn Any>` 替代 variadic 参数。
- Go 编码时区来自新建 `StatementContext`，Rust固定传入 `chrono_tz::UTC`。当前整数/字符串回归不受时区影响；时间类 Datum 的完全等价性未在本任务证据中验证。
- Go 返回具体 `kv.Handle` 接口值，Rust返回 `Box<dyn kv::Handle>`；两者都隐藏具体 `CommonHandle`。
- Go 的默认类型长度是按类型码索引的 map，Rust是 `(u8, usize)` 切片并线性查找。Rust对未知类型使用 `0`，并显式处理负移位和 `>= 64` 移位；这些保护分支并非逐字复刻 Go 实现。
- Go 使用 `slices.Sort`，Rust使用 `sort_unstable`；对于这里只比较 `i64` 值的结果序列，两者都产生升序值，但都不应被用来依赖相等元素的来源顺序。

Go 的实际使用面更广：`pkg/kv/key_test.go`、`pkg/ddl/primary_key_handle_test.go`、多个 executor 测试都调用这些辅助函数。Rust 目前确认的调用面集中在 testutil 自测和 `autoid_test.rs`，不能据 Go 调用清单宣称所有测试均已迁移。

## 扩展指南

扩展 common handle 支持时，优先修改或核对 `types::NewDatum`/`SetValueWithDefaultCollation`、`codec::EncodeKey` 和 `kv::NewCommonHandle` 的真实能力，不要在本辅助函数内另建不一致的编码。若需要可恢复错误，应新增返回 `Result` 的独立 API，并保留 `MustNewCommonHandle` 作为薄 panic 包装，以免改变现有测试语义。涉及时间值时应先决定 Rust 固定 UTC 是否仍符合 Go `StatementContext` 时区契约，并增加非 UTC 的独立测试。

扩展掩码排序时，应在 `MaskSortHandles` 附近保持位宽公式可追溯到 Go 对照，并为未知 `fieldType`、空输入、负值、极端 `shardBitsCount` 以及 32/64 位字段增加独立测试。测试逻辑应继续放在 `pkg/testkit/testutil/migration_aster_unit_test.rs` 或新的同目录 `*_test.rs` 文件中，不嵌入 `handle.rs`。若改变未知类型或越界移位策略，还需评估现有调用是否依赖“全零”结果或 panic。

性能方面，常规改动不应引入每元素查表或重复编码；字段位宽只需在循环前解析一次。兼容性方面，必须同步核对 Go `handle.go`、真实 executor auto-random 场景以及 `TypeLong`/`TypeLonglong` 两种宽度，避免只用合成输入证明公式。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/testkit/testutil` 列出目标 Rust/Go 文件、模块入口和相关测试。
- RustCodeGraph 源码/符号查询：`node --file pkg/testkit/testutil/handle.rs`、`query MustNewCommonHandle --kind function`、`query MaskSortHandles --kind function`、两个精确 `node` 查询；确认本文件两个公开函数及其完整实现。`callers/callees` 因跨语言同名消歧限制未返回可靠边，文档未将空结果当作“无调用者”。
- 下游实现查询：`node --file pkg/util/codec/codec.rs`、`pkg/types/datum.rs`、`pkg/kv/key.rs`、`pkg/parser/mysql/const.rs`；核对 Datum 转换、key 编码、common handle 列边界和 MySQL 类型长度。
- crate 与装配：`pkg/testkit/testutil/Cargo.toml`、`pkg/testkit/testutil/lib.rs`。
- Go 对照：`pkg/testkit/testutil/handle.go`；实际 Go 调用证据包括 `pkg/executor/test/autoidtest/autoid_test.go`、`pkg/executor/test/seqtest/seq_executor_test.go` 和 `pkg/executor/test/ddl/ddl_test.go`。
- Rust 测试：`pkg/testkit/testutil/migration_aster_unit_test.rs`、`pkg/executor/test/autoidtest/autoid_test.rs`。前者验证编码列和 32/64 位掩码，后者验证真实 allocator/rebase 流程。
- 本任务是纯文档分析，按计划未运行 Cargo；最终仅执行固定十一章节的结构验证，并人工检查文档回答了文件定位、运行流程和安全扩展入口。
