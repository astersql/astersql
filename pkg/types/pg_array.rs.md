# `pkg/types/pg_array.rs`

## 定位与接口

`PgArray::new(element_oid, element_typmod, shape, lower_bounds, elements)` 是规范矩形数组的唯一构造入口。私有字段与只读切片保持构造不变量；shape/lower使用signed i32，逐元素值按最后一轴变化最快的行主序保存，不重排。维数属于值，不属于元素OID。内部容器支持OID23/`PgArrayElement::Int4(i32)`和OID25/`Text(String)`，构造检查OID与实际元素变体匹配并拒绝text中的NUL；元素不可为嵌套数组、JSON或目录字符串。这里的text载体仅为文本codec闭环，不开放SQL/wire族。未开放族包括空数组及全NULL数组均返回UnsupportedElement/0A000。

元素typmod原样保存，容器不执行元素精度或转换规则。元素NULL为`Option<PgArrayElement>`，数组SQL NULL为外层`Option<PgArray>`，NULL的类型身份由外层定型消费者保存。零元素规范化为零维、零下界，保留元素OID/typmod；它与SQL NULL及单个NULL元素不同。模块不实现SQL定型、typed NULL描述、参数/结果接线或二进制codec；后续批次必须实际接线，不能用此规范值测试冒称四方向通过。

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

## 实现与验证边界

构造耗时O(维数+元素数+text字节数)，没有元素拷贝；`has_nulls`按需O(元素数)扫描。大数组的实际内存占用与分配失败、32位目标和性能基准未验证。未来增加其他族时必须同步构造身份匹配与独立测试，不能仅增加OID白名单。

独立`pg05_array_value`行为测试覆盖一维/二维行主序与signed lower、i32元素边界、typmod、零维/零轴/数组NULL/元素NULL、6/7维、负维长/乘积/分配上限/下界溢出、不一致形状、元素数及恢复。

从仓库根按编号任务的共享槽位规则动态原子领取槽位、设置绝对CARGO_TARGET_DIR后执行，默认features：

    cargo fmt --all
    cargo test -p astersql-types --lib pg05_array_text_output -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg05_array_text_input -- --nocapture --test-threads=1
    cargo test -p astersql-types --lib pg05_array_ -- --nocapture --test-threads=1
    cargo test -p astersql-expression --lib pg05_array_text_input -- --nocapture --test-threads=1
    cargo fmt --all -- --check
    make lint
    git diff --check

Ready按根AGENTS与testing-flow选取。只修改Rust和说明不自动触发bazel_prepare；完整触发清单见根AGENTS。此模块的行为测试不覆盖后续数组SQL/四方向codec、其他标量族、MySQL桥接、持久化或RealTiKV。
