# `pkg/config/configtypes/go_units.rs`

## 文件定位

本文件位于 `astersql-config-configtypes` crate，提供两个在 crate 根公开重导出的 Go 兼容解析入口：`ParseGoFloat64(&str) -> Result<f64, String>` 与 `ParseGoSize(&str, bool) -> Result<i64, String>`。模块本身在 `pkg/config/configtypes/lib.rs` 中以私有 `mod go_units` 装配，但两个函数通过 `pub use go_units::{ParseGoFloat64, ParseGoSize}` 成为 crate 公共 API。

它不是 `ByteSize` 配置类型的主体实现；后者位于相邻的 `types.rs`。本文件承担更窄的兼容层职责：复现 Go `strconv.ParseFloat` 的完整 float64 文法和 `docker/go-units v0.5.0` 的容量字符串解析约定，供需要与既有 Go 行为逐项一致的上层路径直接调用。`pkg/config/configtypes/Cargo.toml` 将该目录声明为 `astersql-config-configtypes` 库，`go_units.rs` 自身只使用 Rust 标准库，不引入 Cargo 依赖或 feature 条件。

当前生产调用链有两条：

- `pkg/sessionctx/variable/sysvar_builtins.rs::ddl_write_speed` 调用 `ParseGoSize(value, true)`，解析 `tidb_ddl_reorg_max_write_speed` 一类值，采用 `RAMInBytes` 的 1024 进制倍率，并在调用后施加 `[0, 1 PiB]` 上限。
- `pkg/store/driver/region_split_config.rs::get_region_split_config` 调用 `ParseGoSize(size, false)`，解析 TiKV `/config` 返回的 `region-split-size`，采用 `FromHumanSize` 的 1000 进制倍率。

## 核心职责

1. `ParseGoFloat64` 在不接受前后空白的前提下识别 Go float64 文法，包括十进制、十六进制浮点数、合法的数字分隔下划线、指数、大小写不敏感的 `NaN`/`Inf`/`Infinity`，并生成接近 Go `strconv.ParseFloat` 的错误文本。
2. 十六进制浮点数不经由普通浮点累加，而由 `hexadecimal` 以整数尾数、二进制缩放和 round-to-nearest-even 规则直接构造 IEEE-754 位模式，避免双重舍入；这也是文件头模块注释指出的存在理由。
3. `ParseGoSize` 复现 `docker/go-units v0.5.0` 的“最后一个数字、点或空格为数值/后缀分界”规则，支持 `B`、`K/M/G/T/P` 及可选 `B`/`iB` 后缀，并通过 `binary` 参数选择 1000 或 1024 的倍率。
4. 容量结果保持 Go float64 转 int64 的宿主行为：有限且未达到 `2^63` 的值截断小数部分；`NaN`、无穷大和正向溢出映射为 `i64::MIN`，而不是采用 Rust 浮点转整数的饱和结果。

## 主要符号

- `number_error(text, range) -> String`：内部错误格式化器。`range == false` 产生 `strconv.ParseFloat: parsing ...: invalid syntax`，`true` 产生 `value out of range`。错误中保留原始输入的调试字符串表示。
- `digits(bytes, cursor, hex, prefix_underscore) -> Result<usize, ()>`：从 `cursor` 起扫描十进制或十六进制数字，返回数字个数并原地推进游标。下划线必须夹在两个合法数字之间；仅十六进制前缀之后允许首个下划线，因此 `0X_1.8p+1` 合法而 `1__2`、`1._2` 非法。
- `rounded_shift(value, shift, sticky) -> u128`：将整数尾数按位移缩放。右移时比较被丢弃部分与半值，并结合更低位存在性的 `sticky` 标志及保留结果奇偶性实现 ties-to-even；左移和超宽右移有独立边界处理。
- `hexadecimal(text, negative) -> Result<f64, ()>`：解析已经去除下划线、已经过文法验证且不含符号的十六进制主体。它累计最多 128 位尾数，记录超出部分的位数与非零粘滞位，计算正规数或次正规数的指数与有效数，再用 `f64::from_bits` 组装结果。公开入口负责将其错误改写为范围错误。
- `ParseGoFloat64(text) -> Result<f64, String>`：公共浮点入口。先处理特殊值和符号，再以游标验证完整输入，随后对十进制调用标准库 `parse::<f64>()`，对十六进制调用 `hexadecimal`。
- `ParseGoSize(text, binary) -> Result<i64, String>`：公共容量入口。分离数值与后缀，复用 `ParseGoFloat64`，拒绝严格小于零的容量，校验后缀并乘倍率，最后执行 Go 兼容的 int64 转换。

文件没有结构体、枚举、trait、模块级可变状态或条件编译项；仅两个解析函数是公开 API，其余四个函数均为模块私有实现细节。

## 执行流程

`ParseGoFloat64` 的主流程如下：

1. 对无符号、大小写不敏感的 `NaN` 直接返回固定 quiet-NaN 位模式；随后剥离可选的 `+`/`-`。因此 `NaN` 合法，但 `+NaN` 会在后续普通数值文法中失败。
2. 对剥离符号后的 `Inf` 或 `Infinity` 返回相应符号的无穷大；空输入、仅符号和非 ASCII 普通数值被判为语法错误。
3. 根据 `0x`/`0X` 前缀选择十六进制或十进制文法。依次扫描整数部分、可选小数点和小数部分；整数与小数合计必须至少含一个真实数字。
4. 十进制可选 `e/E` 指数，十六进制必须有 `p/P` 指数；指数允许正负号，但必须含数字。游标最终必须恰好到达输入末尾，所以前后空白或尾随字符均不被容忍。
5. 去掉已经验证合法的下划线。十进制交给 Rust f64 解析，并把无穷结果改为范围错误；十六进制交给 `hexadecimal` 进行一次舍入的位级转换。

`hexadecimal` 首先定位 `p/P`，将过大的指数解析失败按符号钳到 `i64::MIN/MAX`，避免指数文本本身造成内部溢出。扫描尾数时，小数点后的每个十六进制位都会令二进制缩放减少 4；超过 u128 容量的低位不再累计，只记录 `dropped_bits` 和 `sticky`。零尾数直接保留正负零。非零尾数先计算最高有效位指数：大于 1023 报范围错误；低于 -1022 按次正规数的固定指数位置舍入；否则舍入到 53 位有效数，必要时处理舍入进位导致的指数增加，然后组装符号、偏置指数和 52 位 fraction。

`ParseGoSize` 的主流程如下：

1. 用 `rfind` 寻找最后一个 ASCII 数字、`.` 或空格。找不到分界时报 `invalid size`。若分界字符为空格，仅去掉这一个分隔空格；其他内部或尾部空格仍会进入数值/后缀校验并失败。
2. 用 `ParseGoFloat64` 解析数值。严格负数返回 `invalid size`；`-0` 不小于零，所以被保留为合法零。
3. 后缀最长三字节并转小写。空后缀或单独 `b` 不乘倍率；首字母 `k/m/g/t/p` 决定指数 1 到 5，尾部只允许空、`b` 或 `ib`。
4. 按 `binary` 选择 `1024^exponent` 或 `1000^exponent`。若结果非有限或大于等于精确表达的 `2^63`，返回 `i64::MIN`；否则以 Rust `as i64` 截断小数，这里输入范围保证与目标 Go 转换约定一致。

## 数据与状态

全部状态都是调用栈上的局部值，没有缓存、全局变量或跨调用状态。

- 浮点词法状态由 `bytes`、`cursor`、数字计数与 `hex` 标志表示；`cursor == bytes.len()` 是完整消费输入的不变量。
- 十六进制数值状态由 u128 `mantissa`、小数十六进制位计数、已丢弃位数和 `sticky` 组成。`sticky` 只表示被容量限制丢弃的部分是否含非零位，供最终一次舍入判断使用。
- IEEE-754 结果通过 u64 位域构造：最高位是符号，11 位偏置指数，低 52 位为 fraction。测试使用 `to_bits()` 验证正负零、次正规数和半值舍入，避免只比较数值而遗漏位级差异。
- 容量倍率只支持到 P（指数 5），与本文件对照的 docker/go-units 映射一致；它不维护单位表状态，而是在 `match` 中直接编码。
- `binary` 仅改变倍率，不改变后缀拼写规则：例如 `MiB` 在 `false` 时仍合法但按 `1000^2` 计算，在 `true` 时按 `1024^2` 计算。这一兼容特性由 `go_units_test.rs` 明确验证。

## 依赖与调用关系

下游依赖均为标准库能力：ASCII 字符判断、字符串查找与替换、整数/浮点解析、饱和整数运算、幂运算以及 `f64::{from_bits,to_bits}`。虽然同 crate 的 `Cargo.toml` 声明了 `anyhow`、`bytesize`、`humantime`、`serde_json` 和 `toml`，本文件未引用它们；这些依赖服务于相邻的配置类型实现。

内部调用边为：

- `ParseGoFloat64 -> number_error`：统一语法/范围错误文本。
- `ParseGoFloat64 -> digits`：扫描整数、小数和指数数字段。
- `ParseGoFloat64 -> hexadecimal`：转换经验证的十六进制浮点文本。
- `hexadecimal -> rounded_shift`：对正规数和次正规数执行一次 ties-to-even 舍入。
- `ParseGoSize -> ParseGoFloat64`：容量数值部分复用完整 Go 浮点文法。

上游装配及调用边为：

- `pkg/config/configtypes/lib.rs` 重导出两个入口，并在 `cfg(test)` 下把 `go_units_test.rs` 作为独立测试模块接入。
- `pkg/sessionctx/variable/sysvar_builtins.rs::ddl_write_speed -> ParseGoSize(_, true)`，其结果再经过系统变量范围校验与错误类型适配。
- `pkg/store/driver/region_split_config.rs::get_region_split_config -> ParseGoSize(_, false)`，解析失败会被适配为存储驱动错误，并触发尝试下一 TiKV store 的既有流程。

RustCodeGraph 的文件关系报告将本文件标记为被 `go_units_test.rs`、`sysvar_builtins.rs`、`region_split_config.rs` 三个文件使用；对公开符号的 `callers/callees` 命令在当前索引未输出函数级边，因此上述具体边同时由这三个已索引文件中的调用点核实。

## 错误处理与边界

- 浮点语法错误和范围错误都使用 `String`，不保留结构化错误类型；调用者只能按失败或文本处理。`ParseGoSize` 可能透传 `ParseGoFloat64` 的 `strconv.ParseFloat` 文本，也可能返回自己的 `invalid size` 或 `invalid suffix` 文本。
- 普通数字必须是 ASCII；特殊值在 ASCII 检查前按大小写不敏感比较。符号只对 infinity 特殊值生效，`+NaN`/`-NaN` 不在特殊分支内，测试确认 `+NaN` 非法。
- 下划线只允许出现在数字之间，例外是 `0x`/`0X` 后紧接的一个下划线仍须后随十六进制数字。指数标记或指数符号之后不能直接出现下划线。
- 十六进制必须带二进制指数；`0x1` 被拒绝。过大正指数或舍入后超过最大有限值报范围错误，极小值可以舍入为零；负的零尾数保留符号位。
- `rounded_shift` 显式处理位移大于 128 和等于 128，避免 Rust 的超宽移位；指数缩放使用饱和运算，避免恶意超长指数导致算术溢出。
- 容量解析按字节长度限制后缀为三字节；当前允许的单位均为 ASCII，因此大小写转换和切片安全。前导空格、尾随空格、双分隔空格、拆开的单位及多余后缀都失败。
- `size < 0.0` 不拒绝 `-0.0` 和 `NaN`，这是与 Go 对照行为一致的刻意边界。Go 在受支持的 64 位宿主上把 `NaN`、无穷或超范围 float64 转 int64 得到最小 int64，本实现显式返回 `i64::MIN`，避免 Rust 饱和转换改变结果。

## 并发与资源生命周期

两个公开函数都是同步、纯计算式解析器：只借用输入字符串，在返回前完成所有工作，不保存引用，不分配后台任务，不使用锁、通道、事务、文件描述符或网络资源。除构造错误文本、移除下划线、后缀小写化等短生命周期字符串分配外，没有外部资源生命周期。

因此函数本身可被多个线程并发调用，不存在共享可变状态或初始化次序要求。上游 `get_region_split_config` 虽位于异步网络流程中，但本解析调用发生在已取得 JSON 字符串之后，既不跨 `await` 持有内部状态，也不参与取消管理；取消和重试由上游函数负责。`ddl_write_speed` 后续更新全局系统变量的同步语义也不属于本文件。

## 与 Go 版本的对应关系

浮点部分对照 Go 标准库 `strconv.ParseFloat(..., 64)` 的可观察语义，而不是仓库内某个同名 Go 文件：支持十进制和 `0x...p...` 十六进制、数字间下划线、特殊值、范围错误及 float64 舍入。`pkg/config/configtypes/go_units_test.rs` 重点锁定 Rust 标准解析器不能直接覆盖的十六进制位级结果，包括 half-even 舍入、sticky 位、次正规最小值、下溢、最大有限值和溢出。

容量部分直接对照仓库 `go.mod` 锁定的 `github.com/docker/go-units v0.5.0`：其 `size.go::FromHumanSize` 使用十进制映射，`RAMInBytes` 使用二进制映射，两者共用 `parseSize` 的最后分隔符、float 解析、负值拒绝、大小写不敏感后缀和 float64-to-int64 转换。本文件把两个 Go 入口合并为 `ParseGoSize(text, binary)`，以布尔参数选择映射。

仓库同目录 `types.go::ByteSize.UnmarshalJSON/UnmarshalText` 调用 `units.RAMInBytes`，说明配置字节数的既有 Go 语义是二进制倍率。Rust 的相邻 `types.rs` 当前有独立 `parse_byte_size` 实现，并未调用本文件；因此不能把 `go_units.rs` 描述为所有 Rust `ByteSize` 反序列化的统一入口。当前真实接线仅是上述系统变量和 region split 配置两条生产路径。

Go 测试 `pkg/config/configtypes/types_test.go` 验证 `1MiB`、`512KiB` 的配置反序列化和格式化；Rust 直接兼容证据来自独立的 `go_units_test.rs`，上层链路还分别由 `sysvar_builtins_test.rs::ddl_write_speed_uses_go_ram_units_and_global_hooks` 和 `region_split_config_test.rs::pd_split_config_uses_first_successful_tikv_and_decimal_go_units` 覆盖。

## 扩展指南

- 若扩展浮点文法，优先修改 `ParseGoFloat64` 的游标验证和 `digits`，同时在独立文件 `pkg/config/configtypes/go_units_test.rs` 增加合法、非法和错误类别用例；不要把测试嵌入生产源文件。
- 若调整十六进制精度或边界，修改 `hexadecimal`/`rounded_shift` 时必须以 `to_bits()` 添加回归样例，覆盖正负零、半值两侧、sticky 位、次正规边界、正规数进位和最大有限值。这里的性能风险主要来自超长文本的线性扫描，不能通过先累加到 f64 的简化换取双重舍入。
- 若新增容量单位或后缀，应先确认目标 docker/go-units 或 Go 上游实际支持，再同步 `ParseGoSize` 的首字母映射、三字节长度限制和测试。随意加入 `E/Z/Y` 会偏离 v0.5.0 的解析映射，即使相邻格式化代码能输出这些单位也不能据此推断解析支持。
- 若需要消除 `binary: bool` 的误用风险，可在保持公共兼容性的前提下新增语义化包装函数；所有调用点必须明确选择 `RAMInBytes` 或 `FromHumanSize` 语义，并分别更新 `sysvar_builtins_test.rs` 与 `region_split_config_test.rs`。
- 若统一 `types.rs::parse_byte_size` 与本文件，必须单独评估现有错误文本、非有限值和溢出差异；这超出当前文件文档任务，不能仅因逻辑相似就假定可直接替换。
- 错误文本可能被日志、测试或上层错误包装观察。改变 `number_error`、`invalid size`、`invalid suffix` 前应加入精确错误回归，并检查两个生产调用者的错误适配。

## 验证依据

- RustCodeGraph `status`：当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/config/configtypes` 确认目标源、模块入口和独立测试均已索引。
- RustCodeGraph `node --file pkg/config/configtypes/go_units.rs --offset 1 --limit 260`：读取 238 行完整源文件，确认 7 个符号、签名、实现分支，并报告三个使用文件。
- RustCodeGraph `query ParseGoFloat64 --kind function` 与 `query ParseGoSize --kind function`：确认公开符号分别位于第 131、199 行，签名分别为 `(&str) -> Result<f64, String>` 和 `(&str, bool) -> Result<i64, String>`。
- RustCodeGraph 对两个限定符号执行 `callers`/`callees` 未返回函数级结果；随后通过已索引文件节点和精确调用点核实 `ParseGoSize` 的两个生产调用者及内部 `ParseGoFloat64` 调用，不据空结果推断“无调用者”。
- 已读源码与配置：`pkg/config/configtypes/go_units.rs`、`lib.rs`、`Cargo.toml`、`types.rs`，以及 `pkg/sessionctx/variable/sysvar_builtins.rs`、`pkg/store/driver/region_split_config.rs` 的直接调用段。
- 已读对照实现：`pkg/config/configtypes/types.go`、`types_test.go`，仓库 `go.mod`/`go.sum` 锁定的 `github.com/docker/go-units v0.5.0`，以及本机模块缓存对应版本的 `size.go::{FromHumanSize,RAMInBytes,parseSize}`。
- 已读独立 Rust 测试：`pkg/config/configtypes/go_units_test.rs`、`pkg/sessionctx/variable/sysvar_builtins_test.rs` 的 DDL 写速率用例、`pkg/store/driver/region_split_config_test.rs` 的十进制单位用例。它们分别验证底层位级/文法边界、二进制上层接线和十进制上层接线。
- 本任务为纯文档分析，按计划不运行 Cargo；最终使用任务指定命令验证目标文件存在且恰含 11 个固定二级章节，并人工复核未修改 Rust、Go、Cargo 或只读的 `plan.md`。
