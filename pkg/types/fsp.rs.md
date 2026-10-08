# `pkg/types/fsp.rs`

## 文件定位

本文件实现 MySQL 时间值的 FSP（Fractional Seconds Precision，小数秒精度）基础规则：校验精度范围、把十进制小数秒文本转换为六位微秒，以及为小数字符串右侧补零。源码本身不声明 Rust 模块；生产接线位于 `pkg/types/internal/field/lib.rs` 的 `fsp` 模块中，该模块以 `include!("../../fsp.rs")` 纳入本文件并用 `pub use fsp::*` 再导出。根 `astersql-types` crate 又在 `pkg/types/lib.rs` 中以 `pub use types_field_group as field` 暴露这个 field 子 crate。因此这里属于 `astersql-types-field` 的生产实现，而不是根 crate 中名为 `fsp` 的直接模块。

`pkg/types/Cargo.toml` 将根包命名为 `astersql-types`，并以路径依赖 `types-field-group = { package = "astersql-types-field", path = "internal/field" }` 连接上述实现。根 `pkg/types/lib.rs` 中的 `mod fsp_test` 仅在 `#[cfg(test)]` 下挂载独立测试，不是生产接线。

## 核心职责

- 用 `UnspecifiedFsp`、`MinFsp`、`MaxFsp` 和 `DefaultFsp` 固化与 Go `pkg/types/fsp.go` 相同的 FSP 取值协议：未指定为 `-1`，合法范围为 `0..=6`，默认值为 `0`。
- `CheckFsp` 把调用方给出的整数精度规范化为可用的 `i32`，同时保持 Go 的非对称越界策略：小于最小值时报错，大于最大值则静默截到 `6`。
- `ParseFrac` 将只含十进制数字的小数部分解释为微秒，按请求精度进行四舍五入，并用独立的 `overflow` 标志报告舍入是否进位到下一整秒。
- `alignFrac` 只负责文本补零，不解析数值；`AlignFracForTest` 是访问该包内函数的公开测试出口。

本文件不负责完整日期/时间的语法解析、时区处理或把 `overflow` 加到秒字段上；这些应由更上层时间解析逻辑处理。仓库中 `pkg/types/time.rs` 还有另一组同名 FSP 常量及一个返回 `Result<i32, TimeError>` 的 `CheckFsp`，不能把两套实现或错误类型混为一谈。

## 主要符号

- `pub const UnspecifiedFsp: i32 = -1`：表示调用方没有显式指定精度；`CheckFsp` 将其转换为 `DefaultFsp`。
- `pub const MaxFsp: i32 = 6`：MySQL 支持的最大六位小数秒，同时是 `ParseFrac` 输出微秒时的固定宽度。
- `pub const MinFsp: i32 = 0`：最小合法精度。
- `pub const DefaultFsp: i32 = 0`：未指定 FSP 时采用的默认精度。
- `pub fn CheckFsp<T: Into<i64>>(fsp: T) -> (i32, Option<errors::SharedError>)`：泛型入口先提升为 `i64`，使测试和调用方可以检查远超 `i32` 的边界；成功时错误为 `None`，非法负值返回默认值和共享错误。
- `pub fn ParseFrac(s: &str, fsp: i32) -> (i32, bool, Option<errors::SharedError>)`：返回 `(微秒, 是否进位到整秒, 错误)`。它只接受可由十进制整数解析器处理的文本，不自行过滤符号、空白或非数字字符。
- `pub(crate) fn alignFrac(s: &str, fsp: i32) -> String`：若有效数字长度不足 `fsp`，在右侧补零；开头的 ASCII `-` 不计入有效长度，其余字符仍按 UTF-8 字节长度计数。
- `pub fn AlignFracForTest(s: &str, fsp: i32) -> String`：直接转调 `alignFrac`，没有额外行为。它在当前源码中的用途是让独立迁移测试覆盖包内实现。

文件没有自定义类型、trait、`impl`、宏或条件编译项；错误类型来自包含它的 field crate 作用域中的 `errors` 模块。

## 执行流程

`CheckFsp` 的流程如下：

1. 通过 `Into<i64>` 将输入统一扩大为 `i64`。
2. 输入等于 `UnspecifiedFsp` 时返回 `(DefaultFsp, None)`。
3. 输入小于 `MinFsp` 时返回默认精度，并用 `errors::Errorf("Invalid fsp %d", ...)` 创建错误。
4. 输入大于 `MaxFsp` 时返回最大精度且不报错。
5. 其余输入位于 `0..=6`，安全转换为 `i32` 后原样返回。

`ParseFrac` 的流程如下：

1. 空串立即返回 `(0, false, None)`，因此空串不会触发 FSP 校验。
2. 调用 `CheckFsp`；若得到错误，则原样返回 `(0, false, error)`。
3. 当规范化后的 `fsp >= s.len()` 时，解析完整字符串，再乘以 `10^(6 - s.len())` 补齐为六位微秒。例如 `"1235"` 得到 `123500`。这条分支不舍入。
4. 当 `fsp < s.len()` 时，按字节取前 `fsp + 1` 位：前 `fsp` 位是保留位，额外一位是舍入依据。切片先经 `from_utf8` 校验，避免前缀截断多字节字符时 panic。
5. 将前缀解析为 `i64`，执行 `(tmp + 5) / 10`，即与 Go 相同的十进制半入舍入。
6. 若舍入结果达到 `10^fsp`，返回 `(0, true, None)`，由调用方负责把进位反映到整秒。
7. 否则乘以 `10^(6 - fsp)`，返回固定微秒尺度。例如 `"1236"` 在 `fsp=3` 时先得到 `124`，最终返回 `124000`。

`alignFrac` 先用字节长度计算当前宽度，若首字节为 `-` 则减一；宽度不足目标 `fsp` 时追加所需数量的 `0`，否则返回内容相同的新 `String`。`AlignFracForTest` 只调用该函数。

## 数据与状态

所有状态均为函数局部值；模块没有全局可变状态。四个常量是编译期 `i32` 值，`CheckFsp` 的临时值为 `i64`，`ParseFrac` 的解析中间值也是 `i64`，最终微秒值收窄为 `i32`。在既定 FSP 约束下，正常数字小数秒的结果范围为 `0..=999999`；达到下一秒时不用 `1000000` 表示，而以 `(0, true, None)` 表示。

`ParseFrac` 的长度和前缀切割均以 UTF-8 字节为单位，这与 Go 字符串按字节切片的语义相符。输入所有权不转移：`ParseFrac` 和 `alignFrac` 借用 `&str`；只有 `alignFrac`/`AlignFracForTest` 分配并返回新的 `String`。错误通过 `Option<errors::SharedError>` 传递，没有异常或隐式告警状态。

## 依赖与调用关系

生产装配链为 `pkg/types/lib.rs` 的 `field` 再导出 → `pkg/types/internal/field/lib.rs` 的 `fsp` 模块 → `include!("../../fsp.rs")`。因此消费者通常通过 field 子 crate或根 crate 的 `field` 命名空间取得这些符号，而不是直接编译本文件。

RustCodeGraph 对 `pkg/types/fsp.rs` 的节点读取显示该文件被多个文件关联使用；精确调用边确认：

- `ParseFrac` 调用本文件的 `CheckFsp`，并引用 `MaxFsp`。
- `CheckFsp` 引用四个 FSP 常量，并调用 field 错误层提供的 `errors::Errorf`。
- `AlignFracForTest` 调用 `alignFrac`；`alignFrac` 没有仓库内函数型下游依赖。

同名的 Go/Rust 符号使 RustCodeGraph 的 `callers` 消歧结果为空，故调用者范围以源码接线补证：`pkg/types/fsp_test.rs` 通过 `include!("fsp.rs")` 直接测试本实现；`pkg/types/field_type_5_aster_unit_test.rs` 通过 `crate::field::*` 调用 `CheckFsp`、`ParseFrac` 和 `AlignFracForTest`；`pkg/types/internal/field/migration_aster_unit_test.rs` 也覆盖这些公开符号。对非测试 Rust 源执行精确调用搜索，没有找到 `ParseFrac` 或本文件 `CheckFsp` 的明确生产调用；当前可确认的生产价值是对外提供 field FSP API，不能据此宣称它已经接入完整时间解析主链。

## 错误处理与边界

- `CheckFsp(-1)` 是合法的“未指定”哨兵；其他小于 `0` 的值返回 `DefaultFsp` 和文本形如 `Invalid fsp <值>` 的错误。大于 `6` 的值被截断为 `6`，不报错。
- `ParseFrac("")` 总是成功返回零，即使传入的 FSP 本身非法；这是早返回顺序决定的现有行为。
- 非数字、空白或其他无法解析为 `i64` 的输入返回 `(0, false, Some(...))`，错误文本带 `strconv.ParseInt:` 前缀以保持 Go 风格。
- 舍入分支的前缀若截断 UTF-8 码点，`from_utf8` 会将其转换为相同前缀风格的错误而不是 panic；`pkg/types/fsp_test.rs` 以 `ParseFrac("é9", 0)` 固化了该 Rust 安全边界。
- `overflow=true` 不是错误。它表示小数部分舍入到了下一秒，微秒返回 `0`；忽略该标志会造成一秒的数值偏差。
- 算法假定调用方提供“小数部分”而非任意超长整数文本。完整解析分支先把文本解析为 `i64`，超出范围会报错；浮点 `powi`/乘法后再转整数是对 Go `float64 * math.Pow10` 的直接语义移植，而非任意精度十进制实现。
- `alignFrac` 只特别处理首字节 `-`，不校验其余内容，也不会截断超过目标宽度的字符串。负的 `fsp` 不会补零，而是原样返回。

## 并发与资源生命周期

这些函数是纯计算式同步函数：不读取或写入共享可变状态，不持锁，不创建线程、异步任务、通道、文件、网络连接或事务。只要输入相同，输出即相同，因此可以并发调用。

资源生命周期仅涉及局部借用和短期分配：`ParseFrac` 的 `&str` 前缀借用不会越过函数返回；解析错误被包装为拥有所有权的共享错误；`alignFrac` 无论是否补零都会返回拥有所有权的 `String`，补零分支还会为重复的零和拼接结果分配内存。没有需要调用方显式释放或清理的资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/fsp.go`，测试对照是 `pkg/types/fsp_test.go`：

- 四个常量的名称和数值一致。
- Go `CheckFsp(int) (int, error)` 对应 Rust 的泛型整数入口与 `(i32, Option<SharedError>)`；两者对 `-1`、其他负值、`>6` 和合法范围的行为一致。Rust 泛型提升到 `i64`，用于保留 Go 测试中大幅越界输入的覆盖能力。
- Go `ParseFrac(string, int) (int, bool, error)` 对应 Rust 三元组。两者都在 `fsp >= len(s)` 时补齐到六位，在需要降精度时只解析 `fsp+1` 位并执行 `(tmp+5)/10`，也都以独立布尔值表示整秒进位。
- Rust 将 Go 的 `errors.Trace`/`strconv.ParseInt` 错误改为 `SharedError`，并人工保留 `strconv.ParseInt:` 文本前缀。Rust 额外显式检查截断后的 UTF-8 前缀，避免非法字节边界造成 panic；该行为仍与 Go 对非数字输入返回解析错误的结果类别一致。
- Go 私有 `alignFrac` 对应 Rust 的 `pub(crate) alignFrac`；Rust 另增 `AlignFracForTest` 测试出口，Go 文件没有这个公开函数。

`pkg/types/fsp_test.rs` 基本逐项移植 Go 测试：覆盖极端上下界、空串、非数字、补齐、舍入和 `999` 进位，并额外覆盖 UTF-8 截断。`pkg/types/internal/field/migration_aster_unit_test.rs` 和 `pkg/types/field_type_5_aster_unit_test.rs` 从实际再导出路径补充验证。

## 扩展指南

- 修改合法精度范围或未指定语义时，应同步审查四个常量、`CheckFsp`、`ParseFrac` 的指数计算，以及 `pkg/types/time.rs` 中独立存在的同名规则；两套 API 的签名不同，不能只改其中一处后假定全仓库一致。
- 修改舍入规则时，主要接入点是 `ParseFrac` 的前缀长度、`(tmp + 5) / 10` 和 overflow 判定。必须同步独立测试 `pkg/types/fsp_test.rs`，并与 `pkg/types/fsp_test.go` 保持测试意图一致；还应更新 field 路径测试 `pkg/types/internal/field/migration_aster_unit_test.rs`。
- 若要支持新的输入格式（例如符号、Unicode 数字或超过 `i64` 的文本），应先明确与 Go 的兼容契约，再替换两个解析分支；不能仅放宽一条分支，否则同一输入会因 `fsp` 不同而出现不一致行为。
- 若生产调用需要直接使用 `alignFrac`，优先在 `astersql-types-field` 内部调用包内函数，不应依赖命名为 `ForTest` 的公开门面。若确认需要稳定的外部 API，应单独设计名称、可见性和输入校验，并增加独立测试文件，而不是把测试写进生产源码。
- 性能上，本文件处理的有效前缀最多与输入长度相关；避免在热路径中反复调用 `alignFrac` 产生字符串分配。正确性风险主要是舍入进位被忽略、字节长度与字符长度混淆，以及修改错误文本后破坏 Go 兼容断言。

## 验证依据

- 源码与接线：`pkg/types/fsp.rs`、`pkg/types/internal/field/lib.rs`、`pkg/types/lib.rs`。
- crate 边界：`pkg/types/Cargo.toml`；确认根 crate 通过 `types-field-group` 路径依赖连接 `pkg/types/internal/field`，且没有为本文件声明独立 feature。
- Go 对照：`pkg/types/fsp.go`、`pkg/types/fsp_test.go`。
- Rust 独立测试：`pkg/types/fsp_test.rs`、`pkg/types/internal/field/migration_aster_unit_test.rs`、`pkg/types/field_type_5_aster_unit_test.rs`。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`node --file pkg/types/fsp.rs --offset 1 --limit 260` 读取了完整 126 行并报告关联使用文件；`query CheckFsp/ParseFrac/alignFrac --json` 定位到本文件符号；`callees fsp.rs::CheckFsp` 与 `callees fsp.rs::ParseFrac` 验证 `ParseFrac → CheckFsp`、常量引用和错误构造；`callees fsp.rs::AlignFracForTest` 验证测试出口到 `alignFrac`。由于同名符号消歧限制，调用者由上述模块入口和 `rg` 精确搜索补证。
- 边界证据：Rust 测试覆盖非法负 FSP、大幅正负越界、非数字、空串、UTF-8 截断、补齐、四舍五入和整秒进位；Go 测试提供原始兼容意图。
- 本任务是纯文档分析，依照任务约束未运行 Cargo。交付前使用任务指定命令验证文档存在且恰含 11 个固定二级章节，并人工复核本文没有把未找到的生产调用描述为已接线行为。
