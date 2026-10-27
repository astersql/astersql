# `pkg/types/pg_array.rs`

## 定位与接口

`PgArray::new(element_oid, element_typmod, shape, lower_bounds, elements)` 是规范矩形数组的唯一构造入口。私有字段与只读切片保持构造不变量；shape/lower使用signed i32，逐元素值按最后一轴变化最快的行主序保存，不重排。维数属于值，不属于元素OID。内部容器支持OID16/`PgArrayElement::Bool(bool)`、OID20/`Int8(i64)`、OID23/`Int4(i32)`和OID25/`Text(String)`，构造检查OID与实际元素变体匹配并拒绝text中的NUL；元素不可为嵌套数组、JSON或目录字符串。这里的text载体仅为内部codec闭环，不开放SQL/wire族。未开放族包括空数组及全NULL数组均返回UnsupportedElement/0A000。

元素typmod原样保存，容器不执行元素精度或转换规则。元素NULL为`Option<PgArrayElement>`，数组SQL NULL为外层`Option<PgArray>`，NULL的类型身份由外层定型消费者保存。零元素规范化为零维、零下界，保留元素OID/typmod；它与SQL NULL及单个NULL元素不同。模块不实现SQL定型、typed NULL描述、参数/结果接线；后续批次必须实际接线，不能用此规范值测试冒称四方向通过。

## 构造检查与错误

最多6维，shape与lower长度必须一致。逐轴拒绝负维长，采用checked i32乘积，检查PostgreSQL `MaxArraySize = 0x3fffffff / sizeof(Datum)`（这里以目标usize宽度表示Datum），再检查Rust实际元素载体的Layout；不按恶意shape分配内存。下界检查exclusive endpoint `lower + length`，不能仅检查最后一个下标。全部轴校验后才把零元素规范化；元素数必须精确匹配，不能补NULL或截断。

维数、大小与下界超限返回54000，与本机PostgreSQL18.6参考构建及其arrayutils.c一致。shape/lower数量与元素数量不一致是内部构造错误，`sqlstate()`返回None；后续输入codec需按文本、二进制和语义阶段将其转为准确诊断，不在此猜成某个输入错误码。错误不会回显输入。失败会销毁传入值，没有部分成功结果；之后正常构造仍可用。

## 文本输入与元素委托

`PgArray::from_text_with(bytes, element_oid, element_typmod, delimiter, input)`处理UTF8载荷，固定array_nulls=on。支持可选`[upper]`/`[lower:upper]...=`、最多六维矩形花括号，保存行主序与非默认下界；空数组规范化复用构造入口。typdelim由调用者提供，接受不与花括号/引号/反斜线冲突的单字节ASCII标点，其他约定0A000。

解析器先完成词法、矩形、前缀、尺寸和下界检查，再调用`input(&str, typmod)`逐一转换非NULL元素。回调返回真实PgArrayElement；构造入口最终校验元素身份。不把数组词法解析与整数/其他标量解析混在一起，不从types反向依赖expression。int4使用方通过回调委托已有pg_cast_int4，独立expression测试实际验证此路径。text在UTF8/NUL已验证后直接保存Rust String，不增加另一套字符串编码。

未引用且未转义的NULL大小写不敏感；引用/转义NULL保留字样，空串、Unicode、引号/反斜线与分隔符保真。ASCII词法空白包括VT/FF，未引用元素去掉未转义的首尾空白；引用或转义的空白保留。只进行数组一层反斜线转义，不把Bind输入再当SQL字面量反转义。

`PgArrayTextError<E>`将词法/形状/前缀错误映射22P02，UTF8/原始NUL为22021，上界小于下界为2202E，维数/尺寸/下界超限沿54000。元素错误以Element(E)原值传回，容器sqlstate()对此返回None，由调用者取标量错误的SQLSTATE（例如int4非法22P02、超域22003）。失败不返回部分数组，也不调用后续元素输入。错误消息不包含输入原文。构造身份不符为内部错误，无SQLSTATE，不伪造为输入错误。边界分类已在本机PostgreSQL18.6/UTF8/array_nulls=on采集确认。

解析耗时O(输入字节数+元素输入)，临时存储O(输入字节数+元素数)，递归深度上限6。先存词法元素再委托，以确保形状错误优先；不按声明尺寸提前分配。调用方不得用此内部codec的验证代替后续SQL、参数、结果四方向差分。

## 文本输出

`PgArray::to_text(delimiter)`从规范值按行主序输出花括号，逐层使用相同元素分隔符。只要任一下界不是1，就输出全部维度的`[lower:upper]`前缀；上下界来自已验证的私有布局。空数组固定`{}`，元素NULL为未引用`NULL`，外层SQL NULL继续由消费者处理，不变为空数组。

int4使用i32十进制输出，text保存原始UTF8；两族均应用同一引用规则。空串、大小写不敏感的NULL字样、元素分隔符、花括号、双引号、反斜线以及六种ASCII空白需要双引号，内部引号和反斜线加一层反斜线；非ASCII空白与方括号本身不触发引用。delimiter资格与输入共用校验，非法约定即使空数组也返回UnsupportedElement/0A000，不产生部分结果，后续正常调用可恢复。后续扩充元素族必须补充真实标量输出，不能仅登记OID。

独立`pg05_array_text_output`测试同时断言固定输出和真实解析器往返，覆盖多维、非默认及i32极值下界、六维、元素身份/typmod、空/外层NULL/元素NULL、引用转义、Unicode/ASCII空白、替代分隔符和恢复。当前PostgreSQL18.6/180006/UTF8/array_nulls=on参考实例核对27组输出原始UTF8字节。输出耗时与输出字节数成正比，递归最多6层；内存为输出字符串加单个整数的短暂十进制文本。这里不开放SQL/wire族，也不代替后续四格式组合验收。

## 二进制输入

`PgArray::from_binary_with(bytes, element_oid, element_typmod, input)`读取完整Bind字段的网络序Int32 ndim、flags、UInt32元素OID、每轴length/lower，以及行主序的元素长度/载荷。元素NULL仅length=-1；flags仅允许0/1，但flags=0仍接受NULL。期望OID与typmod来自调用者，未知wire OID不能覆盖期望身份；只有双方OID小于PostgreSQL FirstGenbkiObjectId=10000的内建类型错配才返回42804。空数组规范化继续保存期望元素OID/typmod。

回调`input(&mut &[u8], typmod)`接收严格限界的非NULL元素载荷，并推进切片；零长元素仍调用标量接收器。剩余字节不为空返回UnconsumedElement/22P03，标量错误原样保留在Element(E)，由调用者提取标量SQLSTATE。NULL不调用回调。当前内部载体仅int4/text，其他合法族0A000；尚未接通SQL、typed NULL、通用参数或结果，也未开放新族。调用者必须提供真实的标量binary接收器，不得把此容器接口当作完整wire支持。

解析按参考阶段顺序检查：负ndim与非法flags为22P03，7维为54000；头、维度或元素长度Int32读取不足为08P01；元素长度小于-1或大于剩余数据为22P03。负维长、乘积/分配限制及exclusive下界终点溢出沿共享layout_count返回54000。全部维度先验证，再进入元素循环；不按声明元素数量reserve，不为短消息声明的巨大shape分配元素存储。每项长度/载荷检查后才委托并保存元素，遇到失败立即退出，不调用后续接收器。完整字段尾随字节返回22P03，包含零维数组后的垃圾。

当前PostgreSQL18.6/180006/UTF8实例通过libpq binary Bind核对39组原始输入、输出与SQLSTATE，包括截断头、元素内部短读08P01、元素未消费22P03、两类尾随垃圾22P03、flags=0含NULL、未知wire OID及多维非默认下界。4项独立`pg05_array_binary_input`测试覆盖这些容器行为，并增加六维、零维/零轴、typmod、合法空text/Unicode、未开放族门禁、标量错误保留、停止后续回调及恢复。失败不返回部分数组。时间O(输入字节数+元素接收)，空间O(已接收元素及载荷大小)，维度存储至多六轴；未做超大实际分配或性能基准。

## 二进制输出

`PgArray::to_binary()`返回完整数组字段载荷，不包含DataRow外层长度、varlena头或内存对齐填充。网络序头为Int32维数、Int32实际NULL标志、UInt32元素OID（不是数组OID）；随后保存每轴Int32维长和有符号下界，再按行主序写元素Int32长度及标量载荷。NULL元素仅写-1；非NULL空text写0；int4写长度4和i32网络序，text直接写UTF8字节及其字节长度，沿现有标量二进制规则。typmod不出现在数组wire头，仍保留在规范值中，不执行额外元素转换。

空int4数组固定12字节0/0/23；`{1,NULL}`为1/1/23、2/1、4/00000001、-1。输入flags=0但实际含NULL，输出改为1；输入flags=1但没有NULL，输出改为0。私有不可变字段和唯一构造入口保证非法shape、元素身份、text NUL以及未开放族无法进入输出；缺codec的族包括空值和全NULL仍由构造入口拒绝0A000。输出先检查逐元素长度和总载荷大小，超限返回SizeOverflow/54000，再分配并写入，不暴露部分输出；未扩充载体或SQL/wire白名单。

独立`pg05_array_binary_output`三项测试覆盖固定头及载荷、六维、下界极值、多维行主序、NULL flags规范化、空数组/SQL NULL/NULL元素、typmod保存、text空串/引用/转义/Unicode的字节长度与无对齐、构造门禁及错误后恢复，并使用真实文本/二进制解析器往返。当前PostgreSQL18.6/180006/aarch64/UTF8/array_nulls=on实例经libpq核对11组值×4种输入/输出格式共44组，断言参考结果OID1007/1009、格式、原始字节和成功状态；这只是参考端及内部容器codec证据，不代表本仓库SQL四方向已接通。新测试先以可编译占位入口取得3项行为失败，再以真实实现通过。时间O(元素数+输出字节数)，空间O(输出字节数)，先扫描长度再一次分配；超大真实分配、32位及性能基准未验证。

## 实现与验证边界

构造耗时O(维数+元素数+text字节数)，没有元素拷贝；`has_nulls`按需O(元素数)扫描。大数组的实际内存占用与分配失败、32位目标和性能基准未验证。未来增加其他族时必须同步构造身份匹配与独立测试，不能仅增加OID白名单。

独立`pg05_array_value`行为测试覆盖一维/二维行主序与signed lower、i32元素边界、typmod、零维/零轴/数组NULL/元素NULL、6/7维、负维长/乘积/分配上限/下界溢出、不一致形状、元素数及恢复。

从仓库根按编号任务的共享槽位规则动态原子领取槽位、设置绝对CARGO_TARGET_DIR后执行，默认features：

    cargo fmt --all
    cargo test -p astersql-types --lib pg05_array_binary_output -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg05_array_binary_input -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg05_array_text_output -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg05_array_text_input -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg05_array_ -- --nocapture --test-threads=1
    cargo test -p astersql-expression --lib pg05_array_text_input -- --nocapture --test-threads=1
    cargo fmt --all -- --check
    make lint
    git diff --check

Ready按根AGENTS与testing-flow选取。只修改Rust和说明不自动触发bazel_prepare；完整触发清单见根AGENTS。此模块的行为测试不覆盖后续数组SQL/四方向codec、其他标量族、MySQL桥接、持久化或RealTiKV。

## 2026-10-27 数组显式转换（任务23）

新增规范Int8(i64)/元素OID20，仅供明确数组转换目标与结果编码；严格元素身份、8字节网络序、shape/lower/NULL不变。未开放binary输入族20，协议Bind白名单也保持关闭。


## bool 数组逐族接入（任务26）

新增PgArrayElement::Bool(bool)与元素OID16的严格身份校验。文本输出t/f、binary输出单字节00/01；接收codec由from_text_with/from_binary_with回调委派，不在容器复制标量解析。shape/lower/typmod、两层NULL、空数组规范化沿原内核。

## 2026-10-27 int2数组局部接线

增加Int2(i16)规范元素；沿既有shape/lower/typmod/两层NULL容器校验，输出2字节网络序。

行为与验证见 `docs/postgresql-pg05-array-int2-evidence.md`；不开放完整跨族cast、存储或数组运算。


## 2026-10-27 int8数组输入注册

from_binary_with元素支持门禁纳入OID20；复用已有Int8(i64)规范载体、维数/下界/typmod/NULL校验和8字节网络序输出。生产入口由server逐元素委派既有标量codec；无新增数组容器或标量子系统。行为与验证见 `docs/postgresql-pg05-array-int8-evidence.md`。


## 任务29 bytea数组局部接线

增加Bytea(Vec<u8>)，OID17与任意原始字节保真；构造及text/binary输入门禁注册17，输出复用统一bytea hex与raw binary。bytea_text_input/bytea_text_output从server原有标量codec最小提取，错误分类和hex/escape规则保持，session和server共用入口，数组仅处理自己的转义层。独立pg_array_test.rs验证shape/lower/typmod、两层NULL、全字节及逐字节输出。


## numeric数组开放（任务30）

新增PgArrayElement::Numeric(PgNumeric)和1700容器注册。文本/二进制输出均委托规范标量codec，shape、下界、元素typmod与两层NULL沿既有容器不变量保留。

当前精确18.6差分与验证记录见 `docs/postgresql-pg05-array-numeric-evidence.md`；只承诺记录范围。


## float4数组与原始binary32位型（任务31）

PgArrayElement::Float4(u32)是原始IEEE位型，Eq不执行NaN比较；规范容器新增OID700。float4_binary_input只复制4字节并分类长度错误；float4_from_binary复用容器边界/NULL/shape检查和该标量codec。float_text从server原样提取供两端复用，float4_text仅在文本出口规范NaN/Infinity。

验证与兼容性范围见 `docs/postgresql-pg05-array-float4-evidence.md`；测试位于对应独立测试文件，生产文件不嵌入单元测试。


## float8数组与binary64位型（任务32）

PgArrayElement::Float8(u64)规范载体、共享float8_binary_input及float8_text，8字节网络位型不计算NaN；长度不足08P01、超长22P03。

行为证据与本机验证限制见 `docs/postgresql-pg05-array-float8-evidence.md`；测试保持在同目录独立测试文件。

## 2026-10-28 varchar数组局部接线

规范数组载体注册1043元素，复用Text(String)值和既有text/binary输出；身份/typmod在容器独立保存，不把varchar折为text OID。独立测试见同目录既有测试文件及server的pg05_array_varchar前缀；真实18.6证据见`docs/postgresql-pg05-array-varchar-evidence.md`。

## 2026-10-28 bpchar数组局部接线

本文件在既有数组消费者中注册Bpchar元素1042/数组1014身份，保留字符长度typmod、shape/lower及两层NULL。标量转换复用pg_text_cast，按字符截断并补U+0020，不复制标量实现。独立测试见对应测试文件的pg05_array_bpchar前缀；18.6差分及限制见`docs/postgresql-pg05-array-bpchar-evidence.md`。


## date数组逐族接入（任务35）

PgArrayElement::Date复用规范PgDate；容器验证1082元素，文本输出沿共享引用规则，binary逐元素输出四字节PG日序。shape/lower/typmod/两层NULL沿现有容器保持。独立测试使用pg05_array_date前缀，真实18.6差分见docs/postgresql-pg05-array-date-evidence.md。


## 任务36：time数组

PgArrayElement::Time直接持有已验证PgTime，文本显示及八字节网络序输出委派标量；shape/lower/两层NULL沿共享容器。 独立pg05_array_time测试与18.6差分见docs/postgresql-pg05-array-time-evidence.md。

## timestamp数组接线（任务37）

增加Timestamp(PgTimestamp)元素1114，沿已验证shape/lower/typmod与两层NULL，文本委派Display、二进制委派八字节网络序标量codec。 独立行为与18.6差分证据见 `docs/postgresql-pg05-array-timestamp-evidence.md`。
