# `pkg/util/collate/bin.rs`

## 文件定位

本文件是 `astersql-util-collate` crate 的二进制排序规则实现。crate 入口 `pkg/util/collate/lib.rs` 以 `pub mod bin` 装入本文件并公开重导出其中类型；`pkg/util/collate/Cargo.toml` 声明该 crate 默认启用 `full_collate`，但本文件自身没有 feature 或平台条件编译，因此基础 binary、PAD SPACE binary 和 derived binary 在两种 feature 配置下都存在。

它位于字符串比较、排序键生成与 SQL `LIKE` 匹配的公共底层。`pkg/util/collate/collate.rs` 的工厂将名称和开关状态映射到本文件的三种 `Collator`，表达式层 `pkg/expression/builtin_like.rs::builtinLikeSig::new` 又直接以 `binCollator` 作为默认 LIKE collator。这里不负责解析 collation 名称、维护全局开关或执行表达式，只实现选定 binary 规则后的具体行为。

## 核心职责

- `compare`/`compare_bytes` 对原始 UTF-8 字节做字典序比较，并把 Rust `Ordering` 规范化为 Go 接口约定的 `-1/0/1`。
- `binCollator` 实现严格 binary 语义：比较和所有 key 入口都保留尾部空格，pattern 以字节为一个匹配单位。
- `derivedBinCollator` 保持同样的字节比较与 key，却将 pattern 改为 Unicode 标量值（Go rune）匹配；它也是旧 collation 模式和 `GetBinaryCollator` 的统一回退实现。
- `binPaddingCollator` 为比较与普通 key 应用 PAD SPACE 语义，只裁剪尾部 ASCII 空格；显式的 `KeyWithoutTrimRightSpace` 仍保留原字节，pattern 也不裁尾空格。
- `binPattern` 与 `derivedBinPattern` 保存编译后的 LIKE pattern，并分别委托 `stringutil` 的字节版和 rune 版算法。

这些职责以 `Collator`/`WildcardPattern` trait 为边界；本文件不拥有注册表、错误对象或全局状态。

## 主要符号

- `fn compare(a: &str, b: &str) -> i32`：私有字符串适配器，将两端转为字节切片后调用 `compare_bytes`。
- `fn compare_bytes(a: &[u8], b: &[u8]) -> i32`：私有核心比较器，按 Rust 切片字典序比较，保证返回值只能是 `-1`、`0`、`1`。原始字节入口避免 trait 默认实现的有损 UTF-8 转换。
- `pub struct binCollator`：无字段、可 `Default` 构造。其 `Compare`/`CompareBytes`、`Key`/`KeyBytes`、`ImmutableKey` 和 `KeyWithoutTrimRightSpace` 都保留完整字节；`Pattern` 创建 `binPattern`。
- `pub struct derivedBinCollator`：无字段、可 `Default` 构造。比较和 key 与 `binCollator` 相同，唯一实质差异是 `Pattern` 创建 `derivedBinPattern`。
- `pub struct binPaddingCollator`：无字段、可 `Default` 构造。`Compare`、`CompareBytes`、`Key`、`KeyBytes` 和 `ImmutableKey` 经 `truncateTailingSpace`/`truncateTailingSpaceBytes` 裁尾；`KeyWithoutTrimRightSpace` 是保留尾空格的逃生口。
- `pub struct derivedBinPattern { patChars: Vec<char>, patTypes: Vec<u8> }`：公开类型、私有状态。`Compile` 通过 `stringutil::CompilePattern` 生成 rune 与 pattern 类型序列，`DoMatch` 调用 `stringutil::DoMatch`。
- `pub struct binPattern { patChars: Vec<u8>, patTypes: Vec<u8> }`：公开类型、私有状态。`Compile`/`DoMatch` 分别调用 `CompilePatternBinary`/`DoMatchBinary`，所以 `_` 消耗一个字节而非一个 Unicode 字符。

三个 collator 的 `Clone` 都返回新的零大小对象；`as_any` 支持 `pkg/util/collate/collate.rs::CanUseRawMemAsKey` 做真实类型判断；`MaxKeyLen` 以输入 UTF-8 字节长度作为上界。

## 执行流程

1. 上游通常经 `GetCollator`、`GetCollatorWithCollate` 或 `GetCollatorByID` 取得 `Box<dyn Collator>`。新 collation 开启时，`binary` 选择 `binCollator`，`ascii_bin`/`latin1_bin`/`utf8_bin`/`utf8mb4_bin` 选择 `binPaddingCollator`，`utf8mb4_0900_bin` 选择 `derivedBinCollator`；未知名称或 ID 回退 `binPaddingCollator`。新 collation 关闭时所有名称统一返回 `derivedBinCollator`。
2. 比较路径调用 `Compare` 或 `CompareBytes`。严格/derived 类型直接进入 `compare_bytes`；padding 类型先去除两端各自的尾部 ASCII 空格，再比较剩余字节。大小写、重音和 Unicode 规范化均不参与。
3. key 路径为输入分配并返回 `Vec<u8>`。严格/derived 类型复制完整字节，padding 类型先裁尾空格；`KeyWithoutTrimRightSpace` 对三者都复制完整字节。`CanUseRawMemAsKey` 只认可 `binCollator` 和 `derivedBinCollator`，不认可会裁剪的 padding 类型。
4. LIKE 路径先由 `Pattern` 创建新的可变 matcher，再以 `Compile(pattern, escape)` 填充 `patChars`/`patTypes`，最后可重复调用只读的 `DoMatch`。`binPattern` 按字节编译和匹配，`derivedBinPattern` 按 rune 编译和匹配；`pkg/expression/builtin_like.rs` 在表达式签名内另用互斥锁管理 matcher 缓存。
5. `gb18030BinPattern` 包装并转发到 `binPattern`；`gbkBinPattern` 包装并转发到 `derivedBinPattern`，说明这两个 pattern 也是编码专用 binary collator 的复用构件。

## 数据与状态

三个 collator 都是零大小、无内部可变状态的值；同一实例的比较和 key 结果只由输入决定。每次 key 调用都会构造拥有所有权的 `Vec<u8>`，因此 Rust 版所谓 `ImmutableKey` 表示接口语义而不是与输入共享只读内存。

两个 pattern 是有状态对象：`Compile` 会整体替换此前的 `patChars` 与 `patTypes`，未编译的默认对象持有两个空向量。`patTypes` 的元素含义来自 `pkg/util/stringutil/string_util.rs`：`PatMatch` 为字面匹配、`PatOne` 为 `_`、`PatAny` 为 `%`。编译后 `DoMatch` 只读取状态，因此同一已编译 pattern 可被连续匹配；若要改变 pattern，调用方必须持有可变引用重新编译。

尾空格规则只处理字节 `0x20`，不是所有 Unicode whitespace。`MaxKeyLen` 返回 `s.len() as i32`；对当前三种实现，实际 key 不会超过输入字节数，但极大于 `i32::MAX` 的理论输入会发生窄化，代码没有单独检查。

## 依赖与调用关系

上游与装配关系：

- `pkg/util/collate/lib.rs` 装入并公开重导出本模块，且把独立测试 `bin_1_aster_unit_test.rs` 接入 `#[cfg(test)]`。
- `pkg/util/collate/collate.rs::new_collator`、`GetCollatorWithCollate`、`GetBinaryCollator`、`GetCollatorByID` 构造三种 collator；`CanUseRawMemAsKey` 通过 `as_any` 识别严格/derived 类型。
- `pkg/expression/builtin_like.rs::builtinLikeSig::new` 直接构造 `binCollator`，将其作为 LIKE 的默认字节匹配规则。
- `pkg/util/collate/gbk_bin.rs::gbkBinPattern` 复用 `derivedBinPattern`；`pkg/util/collate/gb18030_bin.rs::gb18030BinPattern` 复用 `binPattern`。

下游依赖：

- `crate::collate::{Collator, WildcardPattern}` 定义动态分派契约；`truncateTailingSpace` 和 `truncateTailingSpaceBytes` 提供 PAD SPACE 裁剪。
- `crate::stringutil::{CompilePattern, DoMatch}` 实现 rune 级 LIKE，`CompilePatternBinary`/`DoMatchBinary` 实现字节级 LIKE。
- 标准库的切片 `cmp`、`Any`、`Box` 和 `Vec` 分别承担字典序、类型识别、trait object 所有权和 key/pattern 存储。

RustCodeGraph 的目标文件节点列出 42 个符号，并确认直接使用者包括 `collate.rs`、`bin_1_aster_unit_test.rs`、`collate_test.rs`、`collate_bench_test.rs`、`gb18030_bin.rs` 等；同名 Go/Rust 符号使全局 callers 查询存在歧义，因此上述精确边又由文件级引用核验。

## 错误处理与边界

本文件没有 `Result`、显式错误或 panic 分支；所有 trait 方法对合法 Rust `&str`/切片均给出确定结果。空字符串可直接比较、生成空 key 或参与 pattern 匹配。比较不解码字符，因此多字节 Unicode 按 UTF-8 字节顺序排序；`CompareBytes` 和 `KeyBytes` 可无损接受无效 UTF-8 字节，这是覆盖 Go `string` 字节语义的重要边界。

PAD SPACE 只影响比较和普通 key，不影响 `Pattern`：Go 源码 `binPaddingCollator.Pattern` 明确说明尾空格在 pattern 中有意义。`KeyWithoutTrimRightSpace` 也有意绕过裁剪。修改这些分支时必须维持“比较相等的值生成相同普通 key”以及“显式不裁剪 key 保留原输入”这两个不变量。

字节 pattern 与 rune pattern 的 `_` 语义不同：中文字符在 `binPattern` 中占多个匹配单位，在 `derivedBinPattern` 中占一个。转义字节与 `%`/`_` 的细节由 `stringutil` 实现，本文件仅保存其编译结果并转发。

## 并发与资源生命周期

`Collator` 和 `WildcardPattern` trait 都要求 `Send + Sync`。零状态 collator 可安全跨线程共享；其每次 `Pattern`/`Clone` 都分配独立对象，不共享可变状态。key 的 `Vec<u8>` 也由调用方独占，生命周期与输入解耦。

pattern 的编译需要 `&mut self`，匹配只需要 `&self`；因此 Rust 类型系统禁止无同步保护地边编译边匹配。文件内部没有锁、原子、线程、任务、通道、事务、I/O 或需显式释放的外部资源。真正的缓存同步位于调用方，例如 `builtinLikeSig::pattern_cache` 用 `Mutex<Option<Box<dyn WildcardPattern>>>` 管理表达式可能跨会话共享的 matcher。本文件不管理 `newCollationEnabled`；该原子开关归 `collate.rs` 所有。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/collate/bin.go`。类型及流程一一对应：Go `binCollator`、`derivedBinCollator`、`binPaddingCollator`、`derivedBinPattern`、`binPattern` 分别对应 Rust 同名类型，比较、裁尾、pattern 选择和克隆语义保持一致。Rust 没有用嵌入复用 `derivedBinCollator` 的基础方法，而是显式实现完整 `Collator` trait；这是语言结构差异，不是行为简化。

Rust 为 trait 补充了 Go 接口中没有的 `CompareBytes`、`KeyBytes` 和 `as_any`：前两者保留任意 Go string 字节而不经有损 UTF-8 转换，后者服务运行时类型判断。另一方面，Go `ImmutableKey` 用 `hack.Slice` 可零拷贝借用字符串内存，Rust 当前签名返回拥有所有权的 `Vec<u8>`，所以三个 Rust 实现都会复制；行为内容一致，但分配特征不同。

Go `strings.Compare` 与 Rust `compare_bytes` 都产生字节字典序的 `-1/0/1`。Go padding 版本以 `truncateTailingSpace` 裁剪；Rust 对 `&str` 和原始字节分别调用字符串版/字节版辅助函数。Go `[]rune` 对应 Rust `Vec<char>`，Go `[]byte` 对应 Rust `Vec<u8>`。

测试证据来自 Go `pkg/util/collate/collate_test.go` 和 Rust 独立测试 `pkg/util/collate/bin_1_aster_unit_test.rs`：两边都验证 binary 保留 `"a "`、`utf8mb4_bin` 去除尾空格、`utf8mb4_0900_bin` 返回 derived 类型，以及新 collation 关闭时统一回退 derived binary。Rust 独立测试额外明确验证 byte/rune pattern 对中文的差异。

## 扩展指南

- 增加 binary 变体时，先决定三个相互独立的维度：比较前是否 PAD SPACE、key 是否裁尾、LIKE 按字节还是 rune；不要仅复制一个现有类型后隐式继承错误语义。
- 新类型应实现完整 `Collator`，尤其同步实现 `CompareBytes`/`KeyBytes`，否则 trait 默认实现会把无效 UTF-8 替换为 U+FFFD，偏离 Go string 行为。若原始内存可直接作为 key，还应审查并按事实更新 `CanUseRawMemAsKey`。
- 若新增名称或 ID 映射，应在 `pkg/util/collate/collate.rs::new_collator` 及相关注册测试中接线；feature 归属也需与 `Cargo.toml` 和 `group2_collator` 的边界一致。
- 修改 pattern 必须同步检查 `pkg/util/stringutil/string_util.rs` 的编译/匹配算法，以及复用它们的 `gbkBinPattern`、`gb18030BinPattern`。byte/rune、转义、连续 `%`、空 pattern、多字节字符和尾空格都是必要回归边界。
- 测试逻辑应继续放在独立文件 `pkg/util/collate/bin_1_aster_unit_test.rs` 或现有 `collate_test.rs`，不要内嵌到生产 `bin.rs`。Go 对照行为变化时还需同步核验 `pkg/util/collate/bin.go` 与 `collate_test.go`。
- 性能变更应关注 key 分配和 rune pattern 的 `Vec<char>` 构造；语义优化不能破坏 Go 对齐。`collate_bench_test.rs`/`collate_bench_test.go` 已覆盖多长度的 compare、key 和 immutable-key 基准，可作为性能回归入口。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引可用（Rust/Go 均已索引）；`files --filter pkg/util/collate` 定位目标、Go 对照与独立测试；`node --file pkg/util/collate/bin.rs --offset 22 --limit 180` 核对完整实现及文件级使用者；对五个主要类型执行 `query`，确认 Rust/Go 定义位置。因同名跨语言符号令裸 `callers/callees` 查询歧义，调用边使用文件节点与精确引用交叉确认。
- 生产源码：`pkg/util/collate/bin.rs`（全部符号和分支）、`lib.rs`（模块装配与测试接线）、`collate.rs`（trait、工厂、回退和 raw-key 判断）、`../stringutil/string_util.rs`（pattern 类型与算法）、`gbk_bin.rs`/`gb18030_bin.rs`（pattern 复用）、`pkg/expression/builtin_like.rs`（应用层默认调用者）。
- crate 配置：`pkg/util/collate/Cargo.toml`，确认 crate 名、`lib.rs` 入口、默认 `full_collate` feature、依赖和 Go package 元数据。
- Go 对照：`pkg/util/collate/bin.go` 与 `collate_test.go`，核对五个类型、key/比较/pattern/clone 行为、尾空格样例和工厂类型映射。
- Rust 测试：`pkg/util/collate/bin_1_aster_unit_test.rs` 验证 strict/padding key 与比较、`KeyWithoutTrimRightSpace`、byte/rune pattern 语义和注册回退；`collate_test.rs` 验证名称/ID 到具体类型；`collate_bench_test.rs` 提供 compare/key 性能入口。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行任务规定的 11 章节结构检查，并人工检查每项关键结论可回溯到上述符号或文件。
