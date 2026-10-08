# `pkg/util/split.rs`

## 文件定位

`pkg/util/split.rs` 是 `astersql-util` crate 中的字节键均匀切分工具。模块由 `pkg/util/lib.rs` 的 `pub mod split` 公开，核心入口是 `GetValuesList`：它把两个边界键去掉最长公共前缀后，将剩余部分近似映射到大端 `u64` 区间，再生成 `num - 1` 个切分键。文件只依赖 Rust 标准库；`pkg/util/Cargo.toml` 没有为它引入专用依赖。

从当前 Rust 接线看，该 API 已公开但尚未接入 Rust 业务主链：仓库内的直接 Rust 使用仅见 `pkg/util/security_2_aster_unit_test.rs` 的一致性测试。对应 Go 实现 `pkg/util/split.go` 已被 Region/索引预切分流程调用，例如 `pkg/util/regionsplit/split_handle.go` 的 `GetSplitTableKeys`、`GetSplitIndexKeys`，以及 `pkg/ddl/index_presplit.go` 的 `getSplitIdxPhysicalKeysFromBound`。因此，这个 Rust 文件目前是对 Go 通用算法的移植和待复用能力，不能据此宣称 Rust 端已经完成完整业务接线。

## 核心职责

- `GetValuesList` 保留输入键的最长公共前缀，在后缀数值区间内等步长地产生切分点，并把结果追加到调用者传入的 `valuesList`。
- `longestCommonPrefixLen` 找出两个任意字节串共有的起始字节数，以免对稳定的键前缀参与数值计算。
- `getStepValue` 使用不同的补位字节把下界后缀和上界后缀转换成 `u64`，再计算无符号区间步长。
- `getUint64FromBytes` 实现“大端、补到 8 字节、超过 8 字节只取前 8 字节”的转换规则。

该算法的目标是近似均分键空间，而不是对任意输入建立完备的有序区间抽象。边界顺序、`num` 的有效性，以及生成键是否适合作为存储层切分键，均由上游负责。Go 调用点在进入本函数前会比较编码后的上下界并拒绝 `lower >= upper`，这项前置校验不在本文件内。

## 主要符号

- `pub fn GetValuesList(lower: Vec<u8>, upper: Vec<u8>, num: usize, valuesList: Vec<Vec<u8>>) -> Vec<Vec<u8>>`：唯一公开入口。它取得两个输入 `Vec` 的所有权，返回追加切分点后的列表；已有列表内容和顺序保持不变。名称刻意保持 Go 导出函数的非 snake case 形式，文件级 `#![allow(non_snake_case)]` 为此消除 lint。
- `pub(crate) fn longestCommonPrefixLen(s1: &[u8], s2: &[u8]) -> usize`：crate 内可见的纯读取 helper。最多比较较短切片的长度，并在首个不同字节处停止。
- `pub(crate) fn getStepValue(lower: &[u8], upper: &[u8], num: usize) -> u64`：crate 内可见的步长计算 helper。下界以 `0x00` 补位，上界以 `0xff` 补位，然后执行环绕减法和整数除法。
- `fn getUint64FromBytes(bs: &[u8], pad: u8) -> u64`：文件私有转换 helper。先复制输入，短输入补齐到 8 字节，最终用 `u64::from_be_bytes` 解释前 8 字节。

文件没有模块级常量、结构体、枚举、trait、`impl` 或条件编译项。`#![allow(dead_code)]` 反映当前移植代码可能尚无生产调用，并不改变可见性或行为。

## 执行流程

`GetValuesList` 的执行顺序如下：

1. 调用 `longestCommonPrefixLen(&lower, &upper)`，得到公共前缀长度 `commonPrefixIdx`。
2. 对两个键从该位置开始的后缀调用 `getStepValue`。后者分别经 `getUint64FromBytes(..., 0)` 和 `getUint64FromBytes(..., 0xff)` 得到下、上界数值，以 `upper.wrapping_sub(lower) / num` 算出步长。
3. 再以 `0x00` 补位转换下界后缀，得到游标 `startV`。
4. 循环 `num.saturating_sub(1)` 次。每轮先复制下界的公共前缀，再用 `wrapping_add(step)` 推进游标，将其编码成 8 字节大端值并追加到前缀后，最后压入 `valuesList`。
5. 返回原列表及新增的 `num - 1` 个键。若 `num == 1`，步长仍会计算，但循环为空，列表原样返回。

例如测试 `pkg/util/security_2_aster_unit_test.rs::split_matches_go_big_endian_algorithm` 使用 `lower = b"a"`、`upper = b"z"`、`num = 4`；两键没有公共前缀，结果为 3 个长度为 8 且递增的切分点。这个测试验证了公开入口的基本形状，但没有证明所有输入都严格有序或严格位于边界之间。

## 数据与状态

本文件没有全局或持久状态。所有状态都局限于一次同步调用：

- `lower`、`upper` 和 `valuesList` 以所有权传入；函数只读取两个边界键，并原地扩展结果列表。
- `commonPrefixIdx` 是字节索引，不做 UTF-8 或字符解释，因此二进制键可直接使用。
- `startV` 是当前 64 位数值游标；每轮按 `step` 环绕相加。
- `buf: [u8; 8]` 在循环中复用，但 `value.extend_from_slice(&buf)` 会把当轮字节复制进独立 `Vec<u8>`，不同结果键不共享这块缓冲。
- 每个新键由 `lower[..commonPrefixIdx]` 和恰好 8 个数值字节组成，长度固定为 `commonPrefixIdx + 8`；它不一定与 `lower` 或 `upper` 等长。

转换只观察公共前缀之后的前 8 个字节。短后缀通过补位扩大到 8 字节；长后缀的第 9 字节及以后不会影响步长和起始数值。这是与 Go `binary.BigEndian.Uint64` 相同的截断语义，也是扩展到更长有效数值空间时必须正视的精度边界。

## 依赖与调用关系

内部调用链为 `GetValuesList` → `longestCommonPrefixLen`、`getStepValue`、`getUint64FromBytes`，其中 `getStepValue` 又调用两次 `getUint64FromBytes`。大端编码和最小值计算分别使用 `u64::{from_be_bytes,to_be_bytes}` 与 `std::cmp::min`，没有 I/O、存储、网络或第三方依赖。

RustCodeGraph 能定位 `pkg/util/split.rs` 中的四个函数符号，但对这些符号的 `callers`/`callees` 查询返回空数组；因此调用关系同时由函数体和仓库文本引用复核。Rust 侧可确认的直接调用者是 `pkg/util/security_2_aster_unit_test.rs::split_matches_go_big_endian_algorithm`，而 `pkg/util/split_test.rs` 直接测试两个 crate 内 helper。`pkg/util/lib.rs` 在 `cfg(test)` 下用 `#[path = "split_test.rs"] mod split_test` 挂载该独立测试文件。

Go 侧的业务关系提供算法所处主链的直接对照：`pkg/util/regionsplit/split_handle.go` 在 common handle 表键和索引键已编码、且验证下界小于上界后调用 `cutil.GetValuesList`；`pkg/ddl/index_presplit.go` 在索引预切分中执行同样的边界验证后调用 `util.GetValuesList`。这些是 Go 实现的调用关系，不能自动视为 Rust 生产调用。

## 错误处理与边界

所有函数都返回普通值，没有 `Result` 或自定义错误。调用者必须理解以下边界：

- `num == 0` 会在 `getStepValue` 的整数除法处 panic；后面的 `saturating_sub(1)` 不能避免这一点，因为步长先于循环计算。有效调用必须保证 `num > 0`。
- `lower` 与 `upper` 的顺序未校验。若转换后的上界小于下界，`wrapping_sub` 会按模 `2^64` 环绕；生成值也通过 `wrapping_add` 环绕。这对齐 Go 的 `uint64` 算术，但可能产生不符合调用者预期的切分键。
- 当数值区间小于 `num` 时，整数除法得到 `step == 0`，从而生成重复键。函数本身不去重，也不保证严格递增。
- 两个键完全相同时，去除全部公共前缀后，空下界补 `0x00`、空上界补 `0xff`，算法仍会在该前缀后生成 8 字节后缀；它不会把相等边界判为错误。
- 任一后缀超过 8 字节时仅前 8 字节参与计算；如果前 8 字节相同而差异在后面，步长可能为 0 或无法反映真实字典序距离。
- `getUint64FromBytes` 对所有输入先执行 `to_vec`。因此即使输入已有 8 字节也会分配并复制；它随后安全地读取 `buf[..8]`，短输入已先补齐，不会因长度不足越界。

这些情况没有在公开 API 内转成可恢复错误。安全接入时应像现有 Go 调用点一样先验证边界顺序，并明确限制 `num` 和有效后缀宽度。

## 并发与资源生命周期

实现是同步、无锁、无异步任务且不访问共享可变状态；不同线程可以对独立输入并发调用。函数参数中的所有权也避免结果列表在调用过程中被其他代码同时修改。

资源生命周期局限于栈上的索引、`u64` 和 8 字节数组，以及堆上的输入/结果 `Vec`。每个切分点单独分配一个容量为 `commonPrefixIdx + 8` 的 `Vec<u8>`；`valuesList.push` 可能触发外层列表扩容。`getUint64FromBytes` 每次调用都复制后缀并可能补位，因此一次 `GetValuesList` 至少执行三次后缀临时分配。不存在需要显式关闭的句柄或后台任务，函数返回后临时缓冲按 Rust 所有权规则释放。

## 与 Go 版本的对应关系

Rust 文件逐函数对应 `pkg/util/split.go`：`GetValuesList`、`longestCommonPrefixLen`、`getStepValue`、`getUint64FromBytes` 的算法顺序和补位规则一致。关键语义映射包括：

- Go 的 `[]byte`/`[][]byte` 映射为 Rust 的 `Vec<u8>`/`Vec<Vec<u8>>`；Go 通过 `append` 返回可能扩容后的 slice，Rust 通过取得并返回 `Vec` 所有权表达同一数据流。
- Go 的 `binary.BigEndian.Uint64` 和 `PutUint64` 对应 Rust 的 `u64::from_be_bytes` 和 `to_be_bytes`。
- Go `uint64` 的减法和加法溢出语义由 Rust 的 `wrapping_sub`、`wrapping_add` 显式表达，避免 debug 构建因溢出 panic。
- Go 1.22 的 `for range num - 1` 对有效正数 `num` 循环 `num - 1` 次；Rust 使用 `0..num.saturating_sub(1)`。不过两版都会先用 `num` 做除数，因此 `num == 0` 仍是无效输入。
- Go helper 在输入至少 8 字节时可直接借用原 slice；Rust helper 总是复制到临时 `Vec`。输出数值一致，但 Rust 当前实现有额外分配成本。

`pkg/util/split_test.rs` 复刻了 `pkg/util/split_test.go` 对公共前缀和步长的表驱动用例，包括空切片、真前缀、首字节不同、短后缀补位和长后缀截断。两边现有同名测试都没有直接覆盖 `GetValuesList` 的 `num == 0`、`step == 0`、环绕或预置 `valuesList` 行为；Rust 仅在另一个独立测试中补充了一个公开入口的正常路径用例。

## 扩展指南

- 若要接入新的 Rust Region 或索引切分流程，应调用公开的 `split::GetValuesList`，并在上游完成 `lower < upper`、`num > 0`、后缀宽度与重复切分点策略的校验；不要把 Go 调用点已经具备的校验假定为本函数内部能力。
- 若要改变均分算法或支持超过 64 位的有效键空间，主要修改点是 `getUint64FromBytes`、`getStepValue` 和 `GetValuesList` 的编码循环。此类变化会影响与 Go 的字节级兼容性，应同步评估 `pkg/util/split.go`，而不能只扩大 Rust 数值类型。
- 若要消除临时分配，可把 `getUint64FromBytes` 改为直接填充固定数组；必须保持“短输入右侧补 `pad`、长输入只取前 8 字节”的结果不变，并用现有表驱动用例防止补位方向改变。
- 测试必须继续放在独立文件 `pkg/util/split_test.rs`，不要嵌入生产源文件。建议新增公开入口用例，覆盖保留已有列表、`num == 1`、零步长、公共前缀输出、长后缀截断和明确约定的无效输入行为；若改变 panic/校验契约，还应同步 Go 测试或记录有意差异。
- 兼容风险主要是生成键字节变化会改变 Region 边界；性能风险主要来自每个结果键的分配和每次数值转换的后缀复制。优化时应先以字节级等价测试固定行为，再评估大 `num` 下的分配量。

## 验证依据

- 源码与符号：`pkg/util/split.rs`；RustCodeGraph `node --file` 确认全文件 106 行，`query` 定位 `GetValuesList`、`longestCommonPrefixLen`、`getStepValue`、`getUint64FromBytes`。对精确符号 ID 执行 `callers`/`callees` 均返回空数组，因此没有把图中缺失的边写成已证实的生产调用。
- crate 边界：`pkg/util/lib.rs` 的 `pub mod split` 与独立测试模块声明；`pkg/util/Cargo.toml` 的 crate 名 `astersql-util`、`lib.rs` 入口和依赖列表。目标目录没有 `doc.go`，因此没有可补充读取的包级 Go 契约文件。
- Go 对照：`pkg/util/split.go` 的四个同名函数；`pkg/util/split_test.go` 的两个表驱动测试。
- Rust 测试：`pkg/util/split_test.rs` 的 `TestLongestCommonPrefixLen`、`TestGetStepValue`；`pkg/util/security_2_aster_unit_test.rs::split_matches_go_big_endian_algorithm` 的公开入口正常路径。后者由 `pkg/util/security_formal_aster_unit_test.rs` 通过 `include!` 引入，并由 `pkg/util/Cargo.toml` 的显式测试目标承载。
- 业务位置：`pkg/util/regionsplit/split_handle.go` 的 common handle/索引切分调用，以及 `pkg/ddl/index_presplit.go::getSplitIdxPhysicalKeysFromBound` 的索引预切分调用；这些证据仅说明对应 Go 算法的应用位置。
- 本任务是纯文档分析，按计划不运行 Cargo。交付验证使用任务指定的 11 个固定章节结构检查，并人工复核重要结论均可回溯到上述符号或路径。
