# `pkg/types/pg_array.rs`

## 定位与接口

`PgArray::new(element_oid, element_typmod, shape, lower_bounds, elements)` 是规范矩形数组的唯一构造入口。私有字段与只读切片保持构造不变量；shape/lower使用signed i32，逐元素值按最后一轴变化最快的行主序保存，不重排。维数属于值，不属于元素OID。当前仅开放OID23和真实`PgArrayElement::Int4(i32)`；元素不可为嵌套数组、JSON或目录字符串。未开放族包括空数组及全NULL数组均返回UnsupportedElement/0A000。

元素typmod原样保存，容器不执行元素精度或转换规则。元素NULL为`Option<PgArrayElement>`，数组SQL NULL为外层`Option<PgArray>`，NULL的类型身份由外层定型消费者保存。零元素规范化为零维、零下界，保留元素OID/typmod；它与SQL NULL及单个NULL元素不同。模块不实现SQL定型、typed NULL描述、参数、结果或codec；后续批次必须实际接线，不能用此规范值测试冒称四方向通过。

## 构造检查与错误

最多6维，shape与lower长度必须一致。逐轴拒绝负维长，采用checked i32乘积，检查PostgreSQL `MaxArraySize = 0x3fffffff / sizeof(Datum)`（这里以目标usize宽度表示Datum），再检查Rust实际元素载体的Layout；不按恶意shape分配内存。下界检查exclusive endpoint `lower + length`，不能仅检查最后一个下标。全部轴校验后才把零元素规范化；元素数必须精确匹配，不能补NULL或截断。

维数、大小与下界超限返回54000，与本机PostgreSQL18.6参考构建及其arrayutils.c一致。shape/lower数量与元素数量不一致是内部构造错误，`sqlstate()`返回None；后续输入codec需按文本、二进制和语义阶段将其转为准确诊断，不在此猜成某个输入错误码。错误不会回显输入。失败会销毁传入值，没有部分成功结果；之后正常构造仍可用。

## 实现与验证边界

构造耗时O(维数)，没有元素拷贝；`has_nulls`按需O(元素数)扫描。大数组的实际内存占用与分配失败、32位目标和性能基准未验证。当前有类型枚举仅含int4，未来增加其他族时必须同步构造身份匹配与独立测试，不能仅增加OID白名单。

独立`pg05_array_value`行为测试覆盖一维/二维行主序与signed lower、i32元素边界、typmod、零维/零轴/数组NULL/元素NULL、6/7维、负维长/乘积/分配上限/下界溢出、不一致形状、元素数及恢复。

从仓库根按编号任务的共享槽位规则动态原子领取槽位、设置绝对CARGO_TARGET_DIR后执行，默认features：

    cargo fmt --all
    cargo test -p astersql-types --lib pg05_array_value -- --nocapture --test-threads=1
    cargo fmt --all -- --check
    make lint
    git diff --check

Ready按根AGENTS与testing-flow选取。只修改Rust和说明不自动触发bazel_prepare；完整触发清单见根AGENTS。此模块的行为测试不覆盖后续数组SQL/四方向codec、其他标量族、MySQL桥接、持久化或RealTiKV。
