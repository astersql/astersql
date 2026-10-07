# `pkg/planner/cascades/base/hash_equaler.rs`

## 文件定位

本文件实现 Cascades 优化器共享的基础哈希器。它属于 `astersql-planner-cascades-base` crate；`pkg/planner/cascades/base/lib.rs` 在 `base` 模块内通过 `include!("hash_equaler.rs")` 嵌入实现，再以 `pub use base::*` 对外导出。crate 的边界由同目录 `Cargo.toml` 定义，当前没有单独声明第三方依赖或 feature。

该哈希器服务于 `base.rs` 定义的 `Hash64`/`Equals`/`HashEquals` 协议：对象把语义字段依次写入 `Hasher`，得到适合快速筛选的 64 位摘要；摘要相同不代表对象相等，调用方仍须用 `Equals` 完成类型及字段的二次确认。实际规划链上的直接证据是 `pkg/planner/cascades/memo/group_expr.rs` 中 `GroupExpression::Init` 创建哈希器、调用自身 `Hash64`，再用 `Sum64` 缓存摘要。

## 核心职责

- 提供 `Hasher` trait，统一布尔值、整数、浮点数、Unicode 标量、字符串和字节切片的增量写入接口，以及摘要、重置和临时缓存管理接口。
- 用 `hasher` 保存 FNV-1a 64 位累计状态；每写入一个基础值，都执行“与值异或，再乘 `prime64`”的混合步骤。
- 用长度前缀保护字符串与字节切片的组合边界，避免 `("abc", "def")` 与 `("abcdef", "")` 被解释成同一输入序列。
- 提供 `NilFlag`/`NotNilFlag`，要求复合类型实现者在指针或接口字段内容之前显式编码空/非空状态。
- 提供可复用的 `Vec<u8>` 临时缓存；`Reset` 清空逻辑长度但保留容量，减少 datum 等临时编码路径的重复分配。

本文件只计算有损摘要，不定义对象相等性、哈希表、Memo 去重策略或加密哈希。`pkg/planner/cascades/base/hash_equaler_test.rs::test_struct_type` 明确证明：不同 Rust 类型若写入相同字段序列可以产生相同摘要，类型差异必须由相等性逻辑识别。

## 主要符号

- `offset64: u64`：FNV-1a 64 位初始偏移量 `14695981039346656037`。
- `prime64: u64`：FNV-1a 64 位质数 `1099511628211`。
- `Hasher`：公开 trait。`HashBool`、`HashInt`、`HashInt64`、`HashUint64`、`HashFloat64`、`HashRune`、`HashString`、`HashByte`、`HashBytes` 追加输入；`Reset`、`SetCache`、`Cache` 管理状态；`Sum64` 只读取当前摘要。
- `NilFlag` / `NotNilFlag`：公开的 `0`/`1` 标记。非空标记不可省略，否则内容为单字节零时会与空值编码冲突。
- `Hash64a`：`u64` 类型别名，保留 Go 版本命名。
- `hasher`：私有实现类型，字段 `hash64a` 是累计摘要，`cache` 是复用缓冲区。
- `NewHashEqualer() -> Box<dyn Hasher>`：公开构造入口，以 `offset64` 和空缓存初始化 trait 对象。
- `hasher::mix`：私有公共混合原语，通过 `wrapping_mul` 明确实现模 `2^64` 回绕，保持 Go 无符号整数溢出语义。

文件没有条件编译项；测试条件编译位于 `lib.rs`，将独立的 `hash_equaler_test.rs` 作为子模块装配。

## 执行流程

典型调用流程如下：

1. 调用方以 `NewHashEqualer` 创建处于 FNV offset 状态的 `Box<dyn Hasher>`。
2. 对象的 `Hash64` 实现按稳定字段顺序调用各 `Hash*` 方法。复合值应递归写入字段；可空字段应先写 `NilFlag` 或 `NotNilFlag`。
3. 标量方法把值转换为 `u64` 后交给 `mix`。有符号整数和 rune 使用 Rust 的 `as u64` 转换以保留补码位模式；浮点数使用 `f64::to_bits`，因此哈希原始 IEEE-754 位模式。
4. `HashString` 先用 `HashInt` 写入 UTF-8 字节长度 `val.len()`，再按 `chars()` 顺序用 `HashRune` 写入 Unicode 标量；长度不是字符数量。`HashBytes` 同样先写字节数，再逐字节经 `HashByte`/`HashRune` 写入。
5. 调用方用 `Sum64` 读取累计摘要；它不会重置状态。`GroupExpression::Init` 随后把零摘要规范为 `1`，这是调用方不变量，并非本文件自动执行的行为。
6. 若同一实例用于下一次计算，调用 `Reset` 恢复 offset 并清空缓存长度；如需临时编码，调用方可用 `SetCache` 转移一个 `Vec<u8>`，再通过 `Cache` 借用其可变切片。

## 数据与状态

`hasher` 只有两个可变状态：`hash64a: u64` 和 `cache: Vec<u8>`。摘要取决于所有写入值及严格顺序；交换字段、漏写长度或漏写空值标记都会改变编码协议或制造歧义。`Sum64` 是观察操作，连续调用结果不变，直至再次写入或 `Reset`。

整数混合不是逐字节的标准库 `hash/fnv.Write` 模式，而是直接把一个基础值作为一次 `u64` 单元异或并乘质数；扩展实现必须沿用现有 `Hash*` 序列，不能用任意字节序列替换而期待摘要兼容。`HashFloat64` 区分不同位模式，例如 `-0.0` 与 `0.0`；NaN 也按载荷位哈希，而不是按数值相等关系归一化。

缓存与摘要彼此独立：`SetCache` 不影响 `hash64a`，`Cache` 只暴露当前长度范围的切片，`Reset` 同时恢复摘要并执行 `Vec::clear`。`migration_cache_reset_reuses_capacity_and_resets_digest` 还验证 Reset 前后分配地址保持不变。

## 依赖与调用关系

下游依赖仅为 Rust 标准语言/库能力：`Box<dyn Hasher>`、`Vec<u8>`、字符串的 `len`/`chars`、`f64::to_bits` 和整数 `wrapping_mul`；同目录 `Cargo.toml` 没有外部依赖。

内部调用边为：所有直接标量入口最终调用 `hasher::mix`；`HashString → HashInt + HashRune*`；`HashByte → HashRune`；`HashBytes → HashInt + HashByte*`。RustCodeGraph 查询也给出 `mix` 的六个直接调用者为 `HashBool`、`HashInt`、`HashInt64`、`HashUint64`、`HashFloat64` 和 `HashRune`。

上游调用者分布在多个 Rust 移植模块。最直接的 Cascades 主链是 `GroupExpression::Init → NewHashEqualer → GroupExpression::Hash64 → Sum64`；其中逻辑算子语义摘要和各子 `GroupID` 依次写入。RustCodeGraph 还识别出表达式聚合、codec、collation、planner core 测试等使用点，说明该 trait 是跨规划/表达式模块的基础协议，而不是只供本目录测试使用。

哈希与相等性的职责分离由 `base.rs` 固化：`Hash64` 接收 `&mut dyn Hasher`，`Equals` 接收 `&dyn Any` 做运行时类型判断，`HashEquals` 组合二者。哈希碰撞或同字段不同类型不能单凭 `Sum64` 判等。

## 错误处理与边界

本 API 不返回 `Result`，没有 I/O 或可恢复错误分支。算术溢出是算法的一部分，`mix` 使用 `wrapping_mul`，因此 debug/release 构建行为一致且不 panic。

调用契约的主要边界如下：

- 空字符串和空字节切片仍写入长度零，不能简单跳过；多个字段的边界由长度前缀区分。
- `HashString` 的长度必须是 UTF-8 字节数，而循环写入的是 Unicode 标量；`migration_string_length_and_struct_type_follow_go_behavior` 对中文字符覆盖了这一差异。
- 有符号负值转 `u64` 依赖补码模转换；改变为绝对值、文本表示或失败转换会破坏 Go 兼容性。
- `Cache` 返回的切片不能增长容量；需要替换/扩充缓存时必须构造 `Vec<u8>` 后调用 `SetCache`。借用受 Rust 生命周期约束，不能在再次可变借用哈希器时继续持有。
- `NilFlag`/`NotNilFlag` 只是常量，本文件不会自动为引用或 trait 对象写标记；责任在每个复合类型的 `Hash64` 实现。
- FNV-1a 非加密且允许碰撞，不适用于鉴权、完整性保护或把摘要当作相等性的唯一证据。

## 并发与资源生命周期

`hasher` 通过 `&mut self` 串行更新，不包含锁、原子、线程、异步任务、通道或外部资源。`Box<dyn Hasher>` 的所有权归构造调用者；当前 trait 未声明 `Send`/`Sync`，因此接口不承诺跨线程共享。若要并发计算，应为每条并发路径创建独立实例，或由上层提供经过验证的同步策略。

缓存的分配随 `hasher` 生命周期存在：`SetCache` 把 `Vec<u8>` 所有权移入实例，`Reset` 复用其 allocation，实例 drop 时统一释放。`Cache(&mut self) -> &mut [u8]` 的借用期由 Rust 编译器约束，防止缓存切片与对同一哈希器的其他可变访问并存。该设计没有清零容量内旧字节的安全保证，不应把缓存当作敏感数据擦除设施。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/base/hash_equaler.go`。Rust 保留了常量值、`Hasher` 方法集合与顺序、nil 标记、`Hash64a`/`hasher` 命名、构造初值、Reset 缓存复用，以及每种基础类型的哈希顺序。

关键语言映射为：Go `Hasher` 接口对应 Rust `dyn Hasher`；Go `*hasher` 返回值对应 `Box<dyn Hasher>`；Go `int` 对应当前目标平台宽度的 `isize`；Go `rune` 对应 `i32`；Go `math.Float64bits` 对应 `f64::to_bits`；Go 无符号乘法自然回绕对应 Rust `wrapping_mul`；Go `[]byte` 对应拥有所有权的 `Vec<u8>` 加借用视图 `&mut [u8]`。

`hash_equaler_test.rs` 逐项复刻 Go `hash_equaler_test.go` 的三类意图：组合字符串边界、不同结构类型的哈希相同但 Equal 为假、基础类型和 Reset 路径。`migration_aster_unit_test.rs` 进一步补充 Go 金值 `8465102021931103247`、负整数、`-0.0`、UTF-8 长度与 Reset 后缓存地址复用。当前可见差异主要是 Rust 所有权 API：`SetCache` 消费 `Vec<u8>`，`Cache` 返回固定长度的可变切片，而 Go 切片可由调用者 append 并再设置回来。

## 扩展指南

新增复合对象哈希时，应在对象自己的生产文件实现 `Hash64`，按稳定且与 `Equals` 一致的字段顺序调用本 trait；不要把对象专属逻辑塞进本文件。可空字段先写 `NilFlag`/`NotNilFlag`，变长字段必须保留边界信息，类型本身若不写入摘要则必须保证碰撞后二次 `Equals` 做类型检查。

若新增基础类型方法，需要同步修改 `Hasher` trait、`impl Hasher for hasher`、Go 对照接口/实现（若要求继续双端兼容），并在独立的 `hash_equaler_test.rs` 与迁移测试中加入固定向量和边界测试；Rust 单元测试不能内嵌回本生产文件。应重点评估摘要兼容性，因为改变现有方法的混合顺序、整数宽度、字符串长度单位或浮点规范化会改变所有上游对象的摘要。

若更换算法或让 `NewHashEqualer` 返回其他实现，必须复核 `GroupExpression::Init` 等调用方对非零摘要、Reset 复用和 trait object 的假设，同时保留碰撞后二次比较。性能评估应关注热路径每字段开销、动态分派和缓存分配；正确性评估应包含 Go 固定向量、空/非空、负数、Unicode、浮点特殊值、字段分段及不同类型同字段的碰撞案例。

## 验证依据

- 源实现：`pkg/planner/cascades/base/hash_equaler.rs`，核对全部 163 行、公开 API、私有状态和混合流程。
- crate 装配：`pkg/planner/cascades/base/Cargo.toml` 与 `pkg/planner/cascades/base/lib.rs`，核对 crate 名、`include!`、公开再导出和独立测试模块。
- 协议定义：`pkg/planner/cascades/base/base.rs`，核对 `Hash64`、`Equals`、`HashEquals` 的职责边界。
- 主链调用：`pkg/planner/cascades/memo/group_expr.rs::GroupExpression::{Hash64, Init}`，核对构造、字段写入、`Sum64` 和零值规范化；`pkg/planner/cascades/memo/group.rs::Group::Hash64` 核对子 GroupID 写入语义。
- Go 对照：`pkg/planner/cascades/base/hash_equaler.go` 与 `hash_equaler_test.go`，核对常量、接口、算法、长度边界和碰撞后二次相等判断。
- Rust 测试：`pkg/planner/cascades/base/hash_equaler_test.rs` 与 `migration_aster_unit_test.rs`，核对基础方法、Reset、固定 FNV 向量、UTF-8、负数、浮点和缓存复用。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file` 核对目标文件和主链源码；`explore/query` 识别 `NewHashEqualer` 的 Rust 定义、十个已索引调用者，以及 `mix`、`HashString`、`HashBytes` 的内部调用边。
- 本任务为纯文档分析，按计划不运行 Cargo；结构验收使用任务指定的 11 章节检查命令。
