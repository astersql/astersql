# `pkg/util/hack/hack.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-util-hack`，包入口是 [`pkg/util/hack/lib.rs`](lib.rs)：入口以 `pub mod hack` 装入本模块，并以 `pub use hack::*` 把这里的公开类型、函数和常量提升到 crate 根。[`pkg/util/hack/Cargo.toml`](Cargo.toml) 指定 `lib.rs` 为库入口，没有普通依赖，只有测试初始化用的 `testsetup` 开发依赖；其 `package.metadata.porting.go-package` 指向 Go 包 `pkg/util/hack`。

它是低层兼容工具的一部分，负责两组能力：一是字节、字符串和裸指针之间的无拷贝只读视图；二是保留 Go Swiss map 默认桶大小的估算常量。它不实现 map 容器本身；Go 1.25/1.26 ABI 镜像和 `MemAwareMap` 分别位于 `map_abi.rs`、`map_abi_go126.rs`。`pkg/util/codec/lib.rs` 还通过 `pub use hack_crate::hack::*` 再导出本模块 API。

## 核心职责

1. `MutableString`、`String` 和 `Slice` 用 Rust 借用视图表达 Go `unsafe.String`/`unsafe.Slice` 的“不复制底层字节”意图。
2. `GetBytesFromPtr` 把调用方提供的裸地址和长度包装成借用切片，并单独兼容“空长度可配空指针”的 Go 行为。
3. `init` 保留 Go 包初始化时触发 map ABI 检查的源码结构，但在 Rust 中只是普通函数，不会因载入 crate 自动执行。
4. 六个 `DefBucketMemoryUsageFor*` 常量镜像 Go 的 Swiss map 默认桶内存估算值；当前全仓 Rust 精确检索未发现这些常量的消费者。

本文件没有编码转换、分配所有权转移或内存回收职责。`String` 要求输入已经是合法 UTF-8；`Slice` 只暴露 `str` 已有的 UTF-8 字节；`GetBytesFromPtr` 的有效性、长度和生命周期均由调用方保证。

## 主要符号

| 符号 | 可见性与签名 | 语义 |
| --- | --- | --- |
| `MutableString<'a>` | `pub type MutableString<'a> = &'a str` | Go `type MutableString string` 的 Rust 借用视图；生命周期绑定到底层字节。 |
| `String` | `pub unsafe fn String(b: &[u8]) -> MutableString<'_>` | 空输入返回静态空串；非空输入调用 `str::from_utf8_unchecked`，无分配、无校验。 |
| `Slice` | `pub fn Slice(s: &str) -> &[u8]` | 调用 `str::as_bytes` 返回共享底层存储的不可变字节切片。 |
| `init` | `pub fn init()` | 显式调用 `crate::map_abi_go126::checkMapABI()`；不是 Rust 生命周期钩子。 |
| `GetBytesFromPtr` | `pub unsafe fn GetBytesFromPtr<'a>(ptr: *const u8, length: usize) -> &'a [u8]` | 长度为零时返回 `&[]`；否则以 `slice::from_raw_parts` 构造借用切片。 |
| `DefBucketMemoryUsageForMapStringToAny` | `pub const ...: usize = 312` | `map[string]any` 默认桶估算。 |
| `DefBucketMemoryUsageForSetString` | `pub const ...: usize = 248` | `map[string]struct{}` 默认桶估算。 |
| `DefBucketMemoryUsageForSetFloat64` | `pub const ...: usize = 184` | `map[float64]struct{}` 默认桶估算。 |
| `DefBucketMemoryUsageForSetInt64` | `pub const ...: usize = 184` | `map[int64]struct{}` 默认桶估算。 |
| `DefBucketMemoryUsageForMapStringToDecimal` | `pub const ...: usize = 248` | `map[string]Decimal` 默认桶估算。 |
| `DefBucketMemoryUsageForMapStringToString` | `pub const ...: usize = 312` | `map[string]string` 默认桶估算。 |

文件内没有结构体、枚举、trait、`impl` 或条件编译项。命名沿用 Go 风格，因此 crate 根在 `lib.rs` 统一允许 `non_snake_case` 和 `non_upper_case_globals`。

## 执行流程

`String` 的主路径是：接收借用字节切片；若 `is_empty()` 则直接返回 `""`，与 Go 的空输入特例保持一致；否则由调用方承担 UTF-8 与别名不变量，再用 `str::from_utf8_unchecked` 创建指向同一地址的 `&str`。`migration_aster_unit_test.rs::byte_string_views_match_go_without_copying` 通过比较指针检查该路径不复制。

`Slice` 直接调用 `s.as_bytes()`，返回值的生命周期由输入 `&str` 推导；没有分支、分配或错误路径。对应测试同时检查内容和指针相同。

`GetBytesFromPtr` 先判断 `length == 0`。该分支无条件返回规范空切片，即使 `ptr` 是空指针也不会调用 `from_raw_parts`；`hack_test.rs::get_bytes_from_null_ptr_with_zero_length_is_empty` 专门锁定此行为。非零分支把 `(ptr, length)` 交给 `slice::from_raw_parts`，函数本身不检查地址是否为空、对齐、可读或覆盖有效 allocation。

`init` 若被显式调用，仅向下调用 `map_abi_go126::checkMapABI`。当前该函数为空体，所以 Rust 路径既不会自动启动检查，也不会产生运行时版本错误；这是现状而不是“已完成 Go ABI 防护”的证据。

六个常量只在编译期提供数值，没有初始化流程。RustCodeGraph 对 `String`、`Slice`、`init`、`GetBytesFromPtr` 的 callers/callees 查询均返回空数组；因此调用证据由精确源码检索补充，不能把图中空边解释为所有符号都未使用。

## 数据与状态

本文件不拥有可变全局状态，也不缓存数据。所有转换结果都是借用：`String` 的输出生命周期受输入 `&[u8]` 约束，`Slice` 的输出生命周期受输入 `&str` 约束；两者不增加引用计数，也不接管底层 allocation。`GetBytesFromPtr` 的返回生命周期 `'a` 不从参数推导，完全由调用点选择，因此安全性依赖调用方确保切片不超过 allocation 和其内容的真实存活期。

六个 `usize` 常量是静态估算值，不会随 map 容量、平台或运行时状态变化。它们来自 Go 同文件中的同名值；本文件没有把这些常量接入 `map_abi_go126::MemAwareMap` 的动态 `Bytes`/`RealBytes` 计算。

## 依赖与调用关系

下游依赖只有标准库 `std::str::from_utf8_unchecked` 与 `std::slice::from_raw_parts`，以及 `init` 指向的 `crate::map_abi_go126::checkMapABI`。crate 内部装配链为 `lib.rs -> mod hack -> pub use hack::*`；另一条再导出链为 `pkg/util/codec/lib.rs -> hack_crate::hack::*`。仓库根 `pkg/lib.rs` 还会通过 `facade_util_hack::*` 暴露整个 crate 的根级 API。

可确认的直接调用者位于独立 Rust 测试：

- `pkg/util/hack/hack_test.rs` 调用 `String`、`Slice` 和 `GetBytesFromPtr`，覆盖内容、别名可见性及空指针/零长度特例。
- `pkg/util/hack/migration_aster_unit_test.rs::byte_string_views_match_go_without_copying` 调用三个转换函数并验证内容及指针同一性。

全仓精确检索没有发现生产 Rust 代码直接调用本 crate 的 `String`、`Slice`、`GetBytesFromPtr` 或六个桶常量。搜索到的 `pkg/types/datum.rs` 和 `pkg/dxf/framework/storage/task_table.rs` 中 `hack::String`/`hack::Slice` 分别解析到它们自己 `lib.rs` 内定义的兼容模块，不是本文件；`pkg/executor/join/join_table_meta.rs` 中的 `GetBytesFromPtr` 仅存在于注释。生产代码当前对 `astersql-util-hack` 的明确使用主要是 `map_abi::MemAwareMap`，属于相邻模块而非本文件。

## 错误处理与边界

这些 API 均不返回 `Result`，所以失败由类型前提或 unsafe 契约表达。`String` 不校验 UTF-8；传入无效 UTF-8 会破坏 `str` 必须合法 UTF-8 的语言不变量，后续行为不受保证。即使初始字节合法，只要 `&str` 存活，调用方也必须避免通过裸指针或其他别名修改底层字节；特别是不能改出非法 UTF-8。`hack_test.rs` 为对齐 Go 而通过裸指针展示修改可见性，这验证迁移意图，但不应作为安全生产调用范式。

`GetBytesFromPtr` 的非零路径要求指针非空、正确对齐、指向至少 `length` 个已初始化且可读的连续 `u8`、总范围不超过单个 allocation，并在返回切片的整个 `'a` 生命周期内保持有效；同时必须遵守 Rust 的别名规则。零长度是唯一由函数内部兜底的特例。

`Slice` 是安全函数，因为 `&str` 已保证 UTF-8 和有效生命周期，且返回不可变 `&[u8]`。空字符串自然得到空切片。

`init` 当前不会报错或 panic，因为 Rust `checkMapABI` 是空实现。相反，Go `map_abi_go126.go::checkMapABI` 在 `runtime.Version()` 不含 `go1.26` 时 panic；不能依赖 Rust 函数获得同等版本保护。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或文件/网络资源。不可变 `&str`、`&[u8]` 视图本身可按 Rust 的 `Sync` 规则跨线程共享，但底层存储必须在所有借用结束前保持有效，并且不能在共享借用期间被并发写入。通过裸指针绕过这一规则会造成数据竞争或未定义行为，而不是 Go 风格“可变字符串”的受支持并发语义。

资源释放仍由原所有者负责：转换函数不分配也不释放。`Vec<u8>` 重分配、字符串所有者析构或裸指针对应 allocation 释放后，任何尚存视图都不得继续使用。正常的 `String(&bytes)` 与 `Slice(&text)` 借用由编译器限制生命周期；`GetBytesFromPtr<'a>` 无法从签名推导真实所有者，所以调用者需要人工维持这一不变量。

## 与 Go 版本的对应关系

[`pkg/util/hack/hack.go`](hack.go) 是直接对照源：符号名称与六个常量值逐项一致。Go `String` 用 `unsafe.String(unsafe.SliceData(b), len(b))` 生成 `MutableString`，Rust 用 `from_utf8_unchecked` 生成 `&str`；Rust 因 `str` 不变量额外要求合法 UTF-8，而 Go `string` 可以容纳任意字节。Go `MutableString` 是具名字符串类型，Rust `MutableString` 只是 `&str` 类型别名，不形成新的类型边界。

Go `Slice` 返回可写的 `[]byte` 并与字符串共享存储，Rust `Slice` 返回不可变 `&[u8]`，保留零拷贝但主动收窄了可变能力。Go `GetBytesFromPtr(unsafe.Pointer, int)` 与 Rust 裸指针版本都不复制；Rust 额外为零长度/空指针分支返回 `&[]`，因为 `slice::from_raw_parts` 即使长度为零也要求非空指针。

Go 的包级 `init()` 在导入包时自动执行，并由 Go 1.26 版本的 `checkMapABI` 检查 runtime 版本、不匹配即 panic。Rust 同名函数没有特殊初始化语义，而且 Rust `map_abi_go126.rs::checkMapABI` 当前为空；因此这里只保留了结构上的调用关系，没有等价的自动 ABI 防线。

`hack_test.go` 的 `TestString`、`TestByte`、`TestMutable` 在 `hack_test.rs` 中有对应测试。Rust 测试为复现 Go 的可变别名效果使用裸指针；迁移补充测试则用指针相等验证零拷贝，并额外覆盖 `GetBytesFromPtr`。Go 测试没有零长度空指针用例，该边界由 Rust 独立测试补充。

## 扩展指南

新增转换能力时应优先保持借用关系可由类型系统表达：安全转换放在 `Slice` 一类安全 API 中；任何依赖原始地址、未检查编码或人为生命周期的入口必须标为 `unsafe`，并在函数文档完整列出 Safety 前提。不要为了复刻 Go 的可写 `[]byte` 而从共享 `&str` 构造 `&mut [u8]`。

修改 `String`、`Slice` 或 `GetBytesFromPtr` 时，应同步更新独立的 `pkg/util/hack/hack_test.rs` 和 `pkg/util/hack/migration_aster_unit_test.rs`，至少检查空输入、无效 UTF-8 的契约边界、指针相等、生命周期约束以及零长度空指针。测试逻辑应继续与 `hack_test.go` 的原意对齐，不应嵌入生产文件。

调整桶估算常量时，需要先用对应 Go runtime/`pkg/util/hack/hack.go` 验证数值，并检查 `map_abi.rs`、`map_abi_go126.rs` 的布局公式；常量是兼容/内存记账契约，错误值可能导致内存估算偏差。若要恢复 Go 的启动 ABI 检查，接入点是 `init` 与 `map_abi_go126::checkMapABI`，同时必须提供真实、可自动触发的 Rust 初始化机制和独立测试，不能仅依赖名为 `init` 的普通函数。

性能方面应维护“无分配、常数时间”的性质；兼容方面要注意 `Slice` 的不可变返回与 Go 可变切片并不完全等价；正确性方面最重要的风险是 `String` 的 UTF-8/别名约束和 `GetBytesFromPtr` 的人为生命周期。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点，目标目录可见 `hack.rs`、Go 对照与独立测试；`files --filter pkg/util/hack` 显示目标文件有 14 个符号。
- RustCodeGraph `node --file pkg/util/hack/hack.rs --offset 1 --limit 260`：核对完整 84 行源码、公开 API、常量、标准库调用和 `init -> checkMapABI` 源码边。
- RustCodeGraph `query 'pkg/util/hack/hack.rs' --json`：核对 `MutableString`、四个函数和六个常量节点；对四个函数执行 `callers`/`callees`，图查询均返回空数组，因此又以精确 `rg` 搜索补查引用。
- RustCodeGraph 读取 `pkg/util/hack/lib.rs`、`hack_test.rs`、`hack_test.go`、`hack.go`、`map_abi_go126.rs`、`migration_aster_unit_test.rs`：核对 crate 装配、Go/Rust 对照、测试边界与 ABI 检查现状。
- 直接读取 `pkg/util/hack/Cargo.toml`：核对包名、库入口、开发依赖和 Go 包迁移元数据；目标目录不存在 `doc.go`。
- 精确 `rg` 检索 crate 引用、四个函数与六个常量：确认测试调用、`pkg/util/codec` 再导出、常量当前无消费者，并排除其他 crate 内同名 `hack` 兼容模块造成的假调用边。
- 本任务是纯文档分析，依计划不运行 Cargo。交付前使用任务指定命令验证本文件恰有十一个固定二级章节，并人工复查所有当前行为与迁移差异都有上述源码或搜索证据。
