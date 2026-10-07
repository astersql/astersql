# `pkg/config/configtypes/types.rs`

## 文件定位

本文件是 `astersql-config-configtypes` crate 的配置标量实现，源码为 [`types.rs`](types.rs)。它把配置文件中常见的“容量”和“时长”表示为 Rust 可计算的整数，同时保留 Go TiDB 配置所使用的人类可读 JSON/TOML 文本格式。crate 入口 [`lib.rs`](lib.rs) 通过 `pub mod types` 声明模块并用 `pub use types::*` 重导出其公开项；仓库总门面 `pkg/lib.rs` 又在 `config::configtypes` 下重导出该 crate。

它位于配置解析边界，而不是配置加载器本身：本文件只实现值级的解析、格式化及 JSON 字符串包装，不读取文件、不保存全局配置，也不直接实现 Serde trait。上层在 `pkg/config/config.rs` 的 `starter_byte_size` 模块和 `pkg/store/pdtypes/api.rs` 的 `byte_size_json`、`duration_option_json` 模块中把这些自由函数接入派生的 Serde 数据结构。

## 核心职责

- 用公开别名 `ByteSize = u64` 表示字节数，并在二进制容量单位文本与整数之间转换。
- 用公开结构体 `Duration { Duration: i64 }` 表示与 Go `time.Duration` 一致的纳秒计数。
- 为两种类型分别提供 JSON 字符串和 TOML/Text 字节串的编解码入口，保留机械移植后的 Go 风格函数名。
- 在私有解析器中执行 UTF-8、语法、单位和数值范围检查，在私有格式化器中产生规范化文本。
- 对齐 Go 版本的接收者更新时机，尤其区分解析失败后“保留旧值”和“清零”两种行为。

本文件不负责通用 Go 数字解析；同 crate 的 `go_units.rs` 提供 `ParseGoFloat64`、`ParseGoSize`，二者由 `lib.rs` 单独重导出。

## 主要符号

| 符号 | 可见性与签名语义 | 作用 |
| --- | --- | --- |
| `ByteSize` | `pub type ByteSize = u64` | 字节数的公开别名；没有独立运行时封装或额外不变量。 |
| `format_byte_size` | 私有，`ByteSize -> String` | 以 1024 为进位选择 `B`/`KiB`…单位，按四位有效数字近似并裁掉小数末尾的零。 |
| `parse_byte_size` | 私有，`&[u8] -> Result<ByteSize>` | 解析容量文本，接受大小写不敏感的 `B`、`K/KB/KiB`、`M`、`G`、`T`、`P` 族后缀。 |
| `ByteSize_MarshalJSON` / `ByteSize_MarshalText` | 公开 | 复用 `format_byte_size`；前者经 `serde_json::to_vec` 加 JSON 引号，后者返回裸文本字节。 |
| `ByteSize_UnmarshalJSON` / `ByteSize_UnmarshalText` | 公开 | 前者先要求合法 JSON 字符串，二者再调用 `parse_byte_size`，仅成功后写回接收者。 |
| `Duration` | 公开、`Clone + Copy + Debug + Default + Eq + PartialEq` | 单字段纳秒计数包装；字段名 `Duration` 延续 Go 嵌入字段的外观。 |
| `parse_duration` | 私有，`&[u8] -> Result<i64>` | 解析 Go 风格复合时长，支持符号、小数和 `ns/us/µs/μs/ms/s/m/h`。 |
| `append_fraction` | 私有 | 为格式化结果补足定宽余数，再删除末尾零。 |
| `format_duration` | 私有，`i64 -> String` | 实现 Go `time.Duration.String()` 风格的单位选择、复合时分秒和负号输出。 |
| `Duration_MarshalJSON` / `Duration_MarshalText` | 公开 | 复用 `format_duration`，分别生成 JSON 字符串字节和裸文本字节。 |
| `Duration_UnmarshalJSON` / `Duration_UnmarshalText` | 公开 | 复用 `parse_duration`；两者在失败时的接收者更新策略不同。 |

文件没有 trait、模块级可变状态、条件编译项或异步入口。所有公开函数均返回 `anyhow::Result`，公开项由 `lib.rs` 重导出。

## 执行流程

容量编码从 `ByteSize_MarshalJSON` 或 `ByteSize_MarshalText` 进入 `format_byte_size`。格式化器反复除以 1024 选择单位；字节单位直接输出整数 `B`，其余单位按数值数量级选择 0 至 3 位小数以模拟四位有效数字，然后去除多余的零。JSON 路径最后让 `serde_json` 生成正确转义和引号，Text 路径直接返回 UTF-8 字节。

容量解码时，JSON 路径先用 `serde_json::from_slice::<String>` 解开且验证 JSON 字符串，Text 路径直接使用输入。`parse_byte_size` 验证 UTF-8，分离数字与后缀，解析有限且非负的 `f64`，按二进制单位乘以 `1024^n`，检查不超过 `i64::MAX`，最后截断小数并转成 `u64`。两个公开解码函数均先保存临时解析结果，成功后才写入调用者提供的值。

时长解码由 `parse_duration` 扫描一个或多个“整数/小数 + 单位”片段。它先处理正负号及无单位特例 `0`，再为每段识别 `ns` 到 `h` 的纳秒倍率；整数部分使用检查乘法，小数部分按 Go `leadingFraction` 的风格限制累加精度，所有片段用 `u128` 汇总并限制到 `i64::MAX + 1`。负数允许该额外一个绝对值以精确表示 `i64::MIN`，正数最终必须转换为 `i64`。

时长编码由 `format_duration` 按绝对值选择格式：小于 1 微秒输出 `ns`，小于 1 毫秒输出 `µs`，小于 1 秒输出 `ms`，其余拆分为小时、分钟、秒及最多九位纳秒小数。`0` 规范化为 `0s`；出现小时后，即使分钟为零也会输出该段，例如 `1h0m0s`。

## 数据与状态

`ByteSize` 的存储状态只是一个 `u64`。但解析器为了匹配 Go `docker/go-units` 的容量习惯，把 `KB` 与 `KiB` 都按 1024 计算；接受小数容量并在最终转换时向零截断。实现显式把上界限制为 `i64::MAX`，因此虽然公开类型是 `u64`，文本入口不能构造更大的值。

`Duration` 的唯一状态是有符号 64 位纳秒数，默认值为零。解析期间使用局部 `u128` 避免符号绝对值边界问题；`format_duration` 使用 `i64::unsigned_abs()`，所以 `i64::MIN` 也能安全格式化。所有格式化函数只读取传入值；解码函数仅在明确的写回点改变 `&mut` 接收者。

关键状态不变量来自 `types_test.rs` 和 `migration_aster_unit_test.rs`：`ByteSize` 的 JSON/Text 解码失败都保留旧值；`Duration_UnmarshalJSON` 失败保留旧纳秒数；`Duration_UnmarshalText` 为对齐 Go 的直接赋值语义，在解析失败时先把字段重置为零再返回错误。

## 依赖与调用关系

`Cargo.toml` 定义 crate 名 `astersql-config-configtypes`，库入口为 `lib.rs`，没有 feature。目标文件直接使用 `anyhow` 生成带上下文错误，并使用 `serde_json` 处理 JSON 字符串；`toml` 只在独立测试辅助代码中用于字段级往返。清单还声明了 `bytesize`、`humantime`，但当前 `types.rs` 的实际实现没有调用它们，而是本地实现 Go 兼容语法与输出。

RustCodeGraph 给出的文件关系显示 `types.rs` 被 `pkg/config/config.rs`、`pkg/store/pdtypes/api.rs`、本目录独立测试以及若干测试/模拟代码使用。关键生产边如下：

- `pkg/config/config.rs::starter_byte_size::{serialize, deserialize}` → `ByteSize_MarshalText` / `ByteSize_UnmarshalText`，用于 `StarterParams.max_import_data_size` 的配置字符串。
- `pkg/store/pdtypes/api.rs::byte_size_json::{serialize, deserialize}` → `ByteSize_MarshalJSON` / `ByteSize_UnmarshalJSON`，用于 PD 类型中的容量字段。
- `pkg/store/pdtypes/api.rs::duration_option_json::{serialize, deserialize}` → `Duration_MarshalJSON` / `Duration_UnmarshalJSON`，用于可空、装箱的时长字段。
- `pkg/store/pdtypes/lib.rs::configtypes` 和 `pkg/lib.rs::config::configtypes` 重导出 `ByteSize`、`Duration` 或整个 crate API，扩大了类型的可见边界。

RustCodeGraph 的 callee 查询确认八个公开入口只分别下沉到 `format_byte_size`、`parse_byte_size`、`format_duration` 或 `parse_duration`；图索引未为这些自由函数列出完整 caller，因此上述上游边又以索引源码与 `rg` 引用搜索交叉核对。

## 错误处理与边界

所有错误通过 `anyhow::Error` 返回。UTF-8 解码、JSON 解引号和数值解析使用 `Context`/`with_context` 增加阶段信息；语法、未知单位和溢出使用 `anyhow!` 生成包含原输入的错误。JSON 反序列化只接受 JSON 字符串，裸文本即使内容本身合法也会失败。

容量边界包括：空或无法切分的输入、负数、`NaN`/无穷、未知单位、非法后缀尾部以及超过 `i64::MAX`。后缀只实现 K/M/G/T/P，尽管格式化单位表还列有 EiB/ZiB/YiB；由于输入上界限制，反向解析这些更高单位本来也不在当前契约内。容量解析允许数字与后缀间单个空格；如 `1.5 kb` 的行为由 Rust 测试覆盖。

时长边界包括：除裸 `0` 外每段必须有单位；不接受 `day`、`second` 或片段间空格；单位区分大小写；任意段或总和溢出都会失败。它接受 `.5s` 与 `1.s`、ASCII `us` 以及两种 Unicode 微秒字符。小数计算使用 `f64` 后截断到纳秒，目的是仿照 Go 的解析规则，而不是提供任意精度十进制时长。

接收者副作用是扩展时必须保持的兼容边界：不得把 `Duration_UnmarshalText` 简单改成“成功后才赋值”，否则会改变 Go 对照语义；也不得让其他三个解码入口在失败时覆盖旧值。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、任务、通道、事务、文件句柄或网络连接。全部计算使用调用栈上的局部数值和临时 `String`/`Vec<u8>`，函数返回后即由 Rust 所有权规则释放。公开解码函数借用可变接收者且不保存引用，不存在跨调用生命周期。

这些类型本身可被上层跨线程使用：`ByteSize` 是 `u64`，`Duration` 仅含 `i64`，没有内部可变性；但本文件不提供同步或热更新机制。并发配置替换、共享所有权和原子可见性均由调用者负责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/config/configtypes/types.go`，独立回归是 `types_test.go`。Rust `ByteSize` 对应 Go 的具名 `uint64` 类型，但 Rust 当前使用类型别名，因此没有方法；原 Go 的 `MarshalJSON`、`MarshalText`、`UnmarshalJSON`、`UnmarshalText` 被保留为带类型名前缀的自由函数。Go 通过 `docker/go-units.BytesSize` 和 `RAMInBytes` 完成容量转换，Rust 用 `format_byte_size`、`parse_byte_size` 本地复现关键语法和精度。

Rust `Duration` 的 `i64` 纳秒字段对应 Go 结构体中嵌入的 `time.Duration`。Go 依赖 `time.ParseDuration` 与 `Duration.String()`；Rust 的 `parse_duration`、`format_duration` 显式复现复合单位、小数、负数、零值、微秒符号和 `i64::MIN` 边界。Rust 结构体派生了比较、复制和默认 trait，但没有直接实现 Serde；上层使用适配模块调用自由函数。

`types_test.go` 与 `types_test.rs` 都覆盖 JSON 的 `1MiB`、TOML 的 `512KiB`、JSON 的 `1h2m3s` 和 TOML 的 `2m3s` 往返。Rust 侧还补充了 Go 兼容细节：四位有效数字、`KB` 二进制语义、非法容量不改接收者、不接受日/完整英文单位和空格、复合小数时长，以及 Text 时长失败清零。`migration_aster_unit_test.rs` 进一步覆盖负数、`us` 输入规范化为 `µs`、裸 `0` 与整小时格式。

## 扩展指南

新增容量单位或语法时，应同时修改 `parse_byte_size` 与 `format_byte_size`，明确是否仍受 `i64::MAX` 限制，并在独立的 `types_test.rs` 中覆盖大小写、空格、小数、溢出、失败后接收者状态及 JSON/Text 两条路径；若目标是 Go 对齐，还应同步核对 `types.go`、`types_test.go` 或上游依赖库的真实行为。

新增时长单位或改变格式化规则时，接入点是 `parse_duration`、`format_duration` 和必要时的 `append_fraction`。必须同步验证正负边界、`i64::MIN`、多片段累加、小数截断、单位规范化和四个公开入口的写回策略。测试逻辑应继续放在同目录独立文件 `types_test.rs` 或 `migration_aster_unit_test.rs`，不要嵌入生产源文件。

若希望这些类型直接参与 Serde 派生，需要谨慎处理 Rust 孤儿规则和 `ByteSize` 只是 `u64` 别名的事实；现有安全扩展方式是在使用方建立 `#[serde(with = "...")]` 适配模块，如 `starter_byte_size`、`byte_size_json` 和 `duration_option_json`。改变公开函数签名或别名/结构体形态会影响这些适配器及 crate 根重导出，属于兼容性变更。

性能风险主要在配置解析时的临时分配和逐字符扫描，通常不在请求热路径；不要为了消除分配而牺牲 JSON 转义正确性或 Go 格式兼容。正确性风险集中在浮点有效数字、溢出、微秒 Unicode 和失败副作用。

## 验证依据

- RustCodeGraph 索引状态：项目包含 11,467 个文件、307,296 个节点；`files --filter pkg/config/configtypes` 确认目标模块已索引，`node --file pkg/config/configtypes/types.rs` 读取了全部 356 行并列出 17 个符号。
- RustCodeGraph 调用查询：对 `ByteSize_MarshalJSON`、`ByteSize_MarshalText`、`ByteSize_UnmarshalJSON`、`ByteSize_UnmarshalText`、`Duration_MarshalJSON`、`Duration_UnmarshalJSON`、`Duration_UnmarshalText`、`Duration_MarshalText` 分别执行 `callers`/`callees`；callee 结果分别指向四个私有解析/格式化函数。
- crate 与装配证据：`pkg/config/configtypes/Cargo.toml`、`pkg/config/configtypes/lib.rs`、`pkg/lib.rs`。
- 生产调用证据：`pkg/config/config.rs` 的 `starter_byte_size`，`pkg/store/pdtypes/api.rs` 的 `byte_size_json` 与 `duration_option_json`，`pkg/store/pdtypes/lib.rs` 的类型重导出。
- Go 对照：`pkg/config/configtypes/types.go`、`pkg/config/configtypes/types_test.go`。
- Rust 独立测试证据：`pkg/config/configtypes/types_test.rs`、`pkg/config/configtypes/migration_aster_unit_test.rs`；测试与生产逻辑保持分文件。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定的 11 章节检查命令；具体退出码记录在任务交付信息中。
