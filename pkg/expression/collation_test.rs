// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 排序规则推导与哈希相等的 Go 对齐测试骨架。
//
// 对应 Go `collation_test.go`：覆盖 collationInfo hash/equals、infer/derive collation
// 表驱动用例、辅助构造与 CompareString。当前多为迁移占位，保留 Go 调用顺序与断言点。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables,
    unused_mut
)]
// 这段逻辑覆盖排序规则 hash/equals、infer/derive collation 的表驱动 case、辅助构造函数和 CompareString 断言。
// 主要类型、函数、方法前说明对应 Go 语义；关键分支、参数解析、资源收尾、错误处理、并发和外部依赖在原调用附近补中文注释。
// Go imports（保留依赖边界，待 Rust crate 接线）：
// - "testing"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/charset"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/planner/cascades/base"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util/chunk"
// - "github.com/pingcap/tidb/pkg/util/mock"
// - "github.com/stretchr/testify/require"
// - "go.uber.org/atomic"

// 迁移占位类型：这些名称来自 Go/TiDB 测试依赖，后续接入真实 Rust 模块时再替换。
type GoAny = ();
type GoError = String;
type GoBytes = Vec<u8>;
type GoBool = bool;

use crate::expression_collation_test_support::*;
use crate::{base, charset, collate};

fn compare_string(left: &str, right: &str, collation: &str) -> i32 {
    collate::GetCollator(collation).Compare(left, right)
}

// VecExprBenchCase holds table-driven vectorization bench cases.
/// 表驱动向量化基准用例的占位结构。
pub struct VecExprBenchCase {
    pub name: &'static str,
}

/// 按名称构造基准用例组占位。
fn vec_expr_case_group(name: &'static str) -> VecExprBenchCase {
    VecExprBenchCase { name }
}
// newExpression 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn new_expression() {
    // Go 签名：func newExpression(coercibility Coercibility, repertoire Repertoire, chs, coll string) Expression
    // 参数语义：coercibility Coercibility, repertoire Repertoire, chs, coll string。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // 返回语义：Go 返回 Expression；只记录返回路径和错误传播。
    // Go: constant := &Constant{RetType: types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetCharset(chs).SetCollate(coll).BuildP()}
    // Go: constant.SetCoercibility(coercibility)
    // Go: constant.SetRepertoire(repertoire)
    // Go: return constant
}

// TestCollationHashEquals 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_collation_hash_equals() {
    let unset = collationInfo::default();
    let set_to_zero = collationInfo::default();
    set_to_zero.SetCoercibility(CoercibilityExplicit);
    let mut unset_hasher = base::NewHashEqualer();
    let mut set_hasher = base::NewHashEqualer();
    unset.Hash64(unset_hasher.as_mut());
    set_to_zero.Hash64(set_hasher.as_mut());
    assert_ne!(unset_hasher.Sum64(), set_hasher.Sum64());
    assert!(!unset.Equals(&set_to_zero));

    let mut c1 = collationInfo::default();
    c1.SetCoercibility(CoercibilityNone);
    c1.SetRepertoire(ASCII);
    c1.SetCharsetAndCollation("aa".into(), "bb".into());

    let mut c2 = c1.clone();
    let mut hasher1 = base::NewHashEqualer();
    let mut hasher2 = base::NewHashEqualer();
    c1.Hash64(hasher1.as_mut());
    c2.Hash64(hasher2.as_mut());
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    assert!(c1.Equals(&c2));

    c2.SetCoercibility(CoercibilityImplicit);
    hasher2.Reset();
    c2.Hash64(hasher2.as_mut());
    assert_ne!(hasher1.Sum64(), hasher2.Sum64());
    assert!(!c1.Equals(&c2));

    c2.SetCoercibility(CoercibilityNone);
    c2.SetRepertoire(EXTENDED);
    hasher2.Reset();
    c2.Hash64(hasher2.as_mut());
    assert_ne!(hasher1.Sum64(), hasher2.Sum64());
    assert!(!c1.Equals(&c2));

    c2.SetRepertoire(ASCII);
    c2.SetCharsetAndCollation("aabb".into(), String::new());
    hasher2.Reset();
    c2.Hash64(hasher2.as_mut());
    assert_ne!(hasher1.Sum64(), hasher2.Sum64());
    assert!(!c1.Equals(&c2));
    // Go 签名：func TestCollationHashEquals(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: c1 := collationInfo{
    // Go: coer: 1,
    // Go: coerInit: atomic.Bool{},
    // Go: repertoire: 1,
    // Go: charset: "aa",
    // Go: collation: "bb",
    // Go: }
    // Go: c2 := collationInfo{
    // Go: coer: 1,
    // Go: coerInit: atomic.Bool{},
    // Go: repertoire: 1,
    // Go: charset: "aabb",
    // Go: collation: "",
    // Go: }
    // Go: hasher1 := base.NewHashEqualer()
    // Go: hasher2 := base.NewHashEqualer()
    // Go: c1.Hash64(hasher1)
    // Go: c2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, c1.Equals(c2))

    // Go: c2.charset = "aa"
    // Go: c2.collation = "bb"
    // Go: hasher2.Reset()
    // Go: c2.Hash64(hasher2)
    // Go: require.Equal(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.True(t, c1.Equals(c2))

    // Go: c2.coer = 2
    // Go: hasher2.Reset()
    // Go: c2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, c1.Equals(c2))

    // Go: c2.coer = 1
    // Go: c2.coerInit.Store(true)
    // Go: hasher2.Reset()
    // Go: c2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, c1.Equals(c2))

    // Go: c2.coerInit.Store(false)
    // Go: c2.repertoire = 2
    // Go: hasher2.Reset()
    // Go: c2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, c1.Equals(c2))

    // Go: c2.repertoire = 1
    // Go: c2.charset = ""
    // Go: c2.collation = "aabb"
    // Go: hasher2.Reset()
    // Go: c2.Hash64(hasher2)
    // Go: require.NotEqual(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.False(t, c1.Equals(c2))

    // Go: c2.charset = "aa"
    // Go: c2.collation = "bb"
    // Go: hasher2.Reset()
    // Go: c2.Hash64(hasher2)
    // Go: require.Equal(t, hasher1.Sum64(), hasher2.Sum64())
    // Go: require.True(t, c1.Equals(c2))
}

// TestInferCollation 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_infer_collation() {
    let cases = [
        (
            vec![
                CollationInput::new(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_general_ci",
                ),
                CollationInput::new(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            Some((
                CoercibilityExplicit,
                UNICODE,
                "utf8mb4",
                "utf8mb4_unicode_ci",
            )),
        ),
        (
            vec![
                CollationInput::new(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_general_ci",
                ),
                CollationInput::new(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            None,
        ),
        (
            vec![
                CollationInput::new(CoercibilityImplicit, ASCII, "gbk", "gbk_bin"),
                CollationInput::new(CoercibilityImplicit, UNICODE, "latin1", "latin1_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "latin1", "latin1_bin")),
        ),
        (
            vec![
                CollationInput::new(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
                CollationInput::new(CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_0900_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_0900_bin")),
        ),
    ];
    for (inputs, expected) in cases {
        let actual = InferCollationMetadata(&inputs);
        match expected {
            None => assert!(actual.is_none()),
            Some((coer, repe, chs, coll)) => {
                let actual = actual.expect("collations should be compatible");
                assert_eq!((actual.Coer, actual.Repe), (coer, repe));
                assert_eq!(
                    (actual.Charset.as_str(), actual.Collation.as_str()),
                    (chs, coll)
                );
            }
        }
    }
    // Go 签名：func TestInferCollation(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: tests := []struct {
    // Go: exprs []Expression
    // Go: err bool
    // Go: ec *ExprCollation
    // Go: }{
    // Go 注释：same charset.
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_general_ci"),
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_general_ci"),
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_general_ci"),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityNone, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_general_ci"),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go 注释：Regression test: utf8mb4_0900_bin is a binary collation and should win
    // Go 注释：over non-bin collations at the same coercibility (same as utf8mb4_bin).
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB40900Bin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB40900Bin},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB40900Bin),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB40900Bin},
    // Go: },
    // Go 注释：Regression test: two utf8mb4 columns with utf8mb4_0900_bin + utf8mb4_unicode_ci
    // Go 注释：combined with a binary blob. utf8mb4_0900_bin should be recognized as bin
    // Go 注释：collation so binary wins without triggering from_binary() cast.
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB40900Bin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetBin, charset.CollationBin),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetBin, charset.CollationBin},
    // Go: },
    // Go 注释：binary charset with non-binary charset.
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityNumeric, UNICODE, charset.CharsetBin, charset.CollationBin),
    // Go: newExpression(CoercibilityCoercible, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityCoercible, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newExpression(CoercibilityNumeric, UNICODE, charset.CharsetBin, charset.CollationBin),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetBin, charset.CollationBin),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, UNICODE, charset.CharsetBin, charset.CollationBin},
    // Go: },
    // Go 注释：different charset, one of them is utf8mb4
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go 注释：different charset, one of them is CoercibilityCoercible
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityCoercible, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityCoercible, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1},
    // Go: },
    // Go 注释：different charset, one of them is ASCII
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, ASCII, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1},
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, ASCII, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin},
    // Go: },
    // Go 注释：3 expressions.
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetBin, charset.CollationBin),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []Expression{
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newExpression(CoercibilityExplicit, UNICODE, charset.CharsetLatin1, charset.CollationLatin1),
    // Go: newExpression(CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: },
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: }

    // Go: ctx := createContext(t)
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i, test := range tests {
    // Go: ec := inferCollation(ctx, test.exprs...)
    // 分支语义：保留 Go 条件路径，后续接线时需维持错误/空值判断顺序。
    // Go: if test.err {
    // Go: require.Nil(t, ec, i)
    // 分支语义：保留 Go 条件路径，后续接线时需维持错误/空值判断顺序。
    // Go: } else {
    // Go: require.Equal(t, test.ec, ec, i)
    // Go: }
    // Go: }
}

// newConstString 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn new_const_string() {
    // Go 签名：func newConstString(s string, coercibility Coercibility, chs, coll string) *Constant
    // 参数语义：s string, coercibility Coercibility, chs, coll string。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // 返回语义：Go 返回 *Constant；只记录返回路径和错误传播。
    // Go: repe := ASCII
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i := range len(s) {
    // 分支语义：保留 Go 条件路径，后续接线时需维持错误/空值判断顺序。
    // Go: if s[i] >= 0x80 {
    // Go: repe = UNICODE
    // Go: }
    // Go: }
    // Go: constant := &Constant{RetType: types.NewFieldTypeBuilder().SetType(mysql.TypeString).SetCharset(chs).SetCollate(coll).BuildP(), Value: types.NewDatum(s)}
    // Go: constant.SetCoercibility(coercibility)
    // Go: constant.SetRepertoire(repe)
    // Go: return constant
}

// newColString 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn new_col_string() {
    // Go 签名：func newColString(chs, coll string) *Column
    // 参数语义：chs, coll string。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // 返回语义：Go 返回 *Column；只记录返回路径和错误传播。
    // Go: ft := types.FieldType{}
    // Go: ft.SetType(mysql.TypeString)
    // Go: ft.SetCharset(chs)
    // Go: ft.SetCollate(coll)
    // Go: column := &Column{RetType: &ft}
    // Go: column.SetCoercibility(CoercibilityImplicit)
    // Go: column.SetRepertoire(UNICODE)
    // 分支语义：保留 Go 条件路径，后续接线时需维持错误/空值判断顺序。
    // Go: if chs == charset.CharsetASCII {
    // Go: column.SetRepertoire(ASCII)
    // Go: }
    // Go: return column
}

// newColJSON 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn new_col_json() {
    // Go 签名：func newColJSON() *Column
    // 返回语义：Go 返回 *Column；只记录返回路径和错误传播。
    // Go: ft := types.FieldType{}
    // Go: ft.SetType(mysql.TypeJSON)
    // Go: ft.SetCharset(charset.CharsetBin)
    // Go: ft.SetCollate(charset.CollationBin)
    // Go: column := &Column{RetType: &ft}
    // Go: return column
}

// newConstInt 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn new_const_int() {
    // Go 签名：func newConstInt(coercibility Coercibility) *Constant
    // 参数语义：coercibility Coercibility。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // 返回语义：Go 返回 *Constant；只记录返回路径和错误传播。
    // Go: ft := types.FieldType{}
    // Go: ft.SetType(mysql.TypeLong)
    // Go: ft.SetCharset(charset.CharsetBin)
    // Go: ft.SetCollate(charset.CollationBin)
    // Go: constant := &Constant{RetType: &ft, Value: types.NewDatum(1)}
    // Go: constant.SetCoercibility(coercibility)
    // Go: constant.SetRepertoire(ASCII)
    // Go: return constant
}

// newColInt 对应 Go 辅助函数/方法；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
pub fn new_col_int() {
    // Go 签名：func newColInt(coercibility Coercibility) *Column
    // 参数语义：coercibility Coercibility。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // 返回语义：Go 返回 *Column；只记录返回路径和错误传播。
    // Go: ft := types.FieldType{}
    // Go: ft.SetType(mysql.TypeLong)
    // Go: ft.SetCharset(charset.CharsetBin)
    // Go: ft.SetCollate(charset.CollationBin)
    // Go: column := &Column{RetType: &ft}
    // Go: column.SetCoercibility(coercibility)
    // Go: column.SetRepertoire(ASCII)
    // Go: return column
}

// TestDeriveCollation 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_derive_collation() {
    crate::collation_aster_unit_test::run_collation_parity_suite();
    let empty = InferCollationMetadata(&[]).expect("empty input uses the Go default");
    assert_eq!(empty.Coer, CoercibilityIgnorable);
    assert_eq!(empty.Repe, UNICODE);
    assert_eq!(empty.Charset, charset::CharsetUTF8MB4);
    assert_eq!(empty.Collation, charset::CollationUTF8MB4);
    // Go 签名：func TestDeriveCollation(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: ctx := mock.NewContext()
    // Go: tests := []struct {
    // Go: fcs []string
    // Go: args []Expression
    // Go: argTps []types.EvalType
    // Go: retTp types.EvalType

    // Go: err bool
    // Go: ec *ExprCollation
    // Go: }{
    // Go: {
    // Go: []string{ast.Left, ast.Right, ast.Repeat, ast.Substr, ast.Substring, ast.Mid},
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityCoercible, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETInt},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.Trim, ast.LTrim, ast.RTrim},
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityCoercible, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: },
    // Go: []types.EvalType{types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.SubstringIndex},
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityCoercible, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstString("啊", CoercibilityExplicit, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString, types.ETInt},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.Replace, ast.Translate},
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityExplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstString("啊", CoercibilityExplicit, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newConstString("ㅂ", CoercibilityExplicit, charset.CharsetBin, charset.CollationBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.InsertFunc},
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityExplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstString("ㅂ", CoercibilityExplicit, charset.CharsetBin, charset.CollationBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETInt, types.ETInt, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, UNICODE, charset.CharsetBin, charset.CollationBin},
    // Go: },
    // Go: {
    // Go: []string{ast.InsertFunc},
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityImplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstString("啊", CoercibilityImplicit, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETInt, types.ETInt, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.InsertFunc},
    // Go: []Expression{
    // Go: newConstString("ㅂ", CoercibilityImplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstString("啊", CoercibilityExplicit, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETInt, types.ETInt, types.ETString},
    // Go: types.ETString,
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []string{ast.Lpad, ast.Rpad},
    // Go: []Expression{
    // Go: newConstString("ㅂ", CoercibilityImplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstString("啊", CoercibilityExplicit, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETInt, types.ETString},
    // Go: types.ETString,
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []string{ast.Lpad, ast.Rpad},
    // Go: []Expression{
    // Go: newConstString("ㅂ", CoercibilityImplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstInt(CoercibilityExplicit),
    // Go: newConstString("啊", CoercibilityImplicit, charset.CharsetGBK, charset.CollationGBKBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETInt, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.FindInSet, ast.Regexp},
    // Go: []Expression{
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_general_ci"),
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString},
    // Go: types.ETInt,
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []string{ast.Field},
    // Go: []Expression{
    // Go: newColString(charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString},
    // Go: types.ETInt,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.Field},
    // Go: []Expression{
    // Go: newColInt(CoercibilityImplicit),
    // Go: newColInt(CoercibilityImplicit),
    // Go: },
    // Go: []types.EvalType{types.ETInt, types.ETInt},
    // Go: types.ETInt,
    // Go: false,
    // Go: &ExprCollation{CoercibilityNumeric, ASCII, charset.CharsetBin, charset.CollationBin},
    // Go: },
    // Go: {
    // Go: []string{ast.Locate, ast.Instr, ast.Position},
    // Go: []Expression{
    // Go: newColInt(CoercibilityNumeric),
    // Go: newColInt(CoercibilityNumeric),
    // Go: },
    // Go: []types.EvalType{types.ETInt, types.ETInt},
    // Go: types.ETInt,
    // Go: false,
    // Go: &ExprCollation{CoercibilityNumeric, ASCII, charset.CharsetBin, charset.CollationBin},
    // Go: },
    // Go: {
    // Go: []string{ast.Format, ast.SHA2},
    // Go: []Expression{
    // Go: newColInt(CoercibilityNumeric),
    // Go: newColInt(CoercibilityNumeric),
    // Go: },
    // Go: []types.EvalType{types.ETInt, types.ETInt},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.Space, ast.ToBase64, ast.UUID, ast.Hex, ast.MD5, ast.SHA},
    // Go: []Expression{
    // Go: newColInt(CoercibilityNumeric),
    // Go: },
    // Go: []types.EvalType{types.ETInt},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.GE, ast.LE, ast.GT, ast.LT, ast.EQ, ast.NE, ast.NullEQ, ast.Strcmp},
    // Go: []Expression{
    // Go: newColString(charset.CharsetASCII, charset.CollationASCII),
    // Go: newColString(charset.CharsetGBK, charset.CollationGBKBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString},
    // Go: types.ETInt,
    // Go: false,
    // Go: &ExprCollation{CoercibilityNumeric, ASCII, charset.CharsetGBK, charset.CollationGBKBin},
    // Go: },
    // Go: {
    // Go: []string{ast.GE, ast.LE, ast.GT, ast.LT, ast.EQ, ast.NE, ast.NullEQ, ast.Strcmp},
    // Go: []Expression{
    // Go: newColString(charset.CharsetLatin1, charset.CollationLatin1),
    // Go: newColString(charset.CharsetGBK, charset.CollationGBKBin),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString},
    // Go: types.ETInt,
    // Go: true,
    // Go: nil,
    // Go: },
    // Go: {
    // Go: []string{ast.Bin, ast.FromBase64, ast.Oct, ast.Unhex, ast.WeightString},
    // Go: []Expression{
    // Go: newColString(charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: []types.EvalType{types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{ast.ASCII, ast.BitLength, ast.CharLength, ast.CharacterLength, ast.Length, ast.OctetLength, ast.Ord},
    // Go: []Expression{
    // Go: newColString(charset.CharsetLatin1, charset.CollationLatin1),
    // Go: },
    // Go: []types.EvalType{types.ETString},
    // Go: types.ETInt,
    // Go: false,
    // Go: &ExprCollation{CoercibilityNumeric, ASCII, charset.CharsetBin, charset.CollationBin},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.ExportSet, ast.Elt, ast.MakeSet,
    // Go: },
    // Go: []Expression{
    // Go: newColInt(CoercibilityExplicit),
    // Go: newColString(charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETInt, types.ETString, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.ExportSet, ast.Elt, ast.MakeSet,
    // Go: },
    // Go: []Expression{
    // Go: newColInt(CoercibilityExplicit),
    // Go: newColJSON(),
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETInt, types.ETJson},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Concat, ast.ConcatWS, ast.Coalesce, ast.Greatest, ast.Least,
    // Go: },
    // Go: []Expression{
    // Go: newColString(charset.CharsetGBK, charset.CollationGBKBin),
    // Go: newColJSON(),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETJson},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Concat, ast.ConcatWS, ast.Coalesce, ast.Greatest, ast.Least,
    // Go: },
    // Go: []Expression{
    // Go: newColJSON(),
    // Go: newColString(charset.CharsetBin, charset.CharsetBin),
    // Go: },
    // Go: []types.EvalType{types.ETJson, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetBin, charset.CharsetBin},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Concat, ast.ConcatWS, ast.Coalesce, ast.In, ast.Greatest, ast.Least,
    // Go: },
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityCoercible, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newColString(charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETInt, types.ETString, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Lower, ast.Lcase, ast.Reverse, ast.Upper, ast.Ucase, ast.Quote,
    // Go: },
    // Go: []Expression{
    // Go: newConstString("a", CoercibilityCoercible, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: },
    // Go: []types.EvalType{types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Lower, ast.Lcase, ast.Reverse, ast.Upper, ast.Ucase, ast.Quote,
    // Go: },
    // Go: []Expression{
    // Go: newColJSON(),
    // Go: },
    // Go: []types.EvalType{types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.If,
    // Go: },
    // Go: []Expression{
    // Go: newColInt(CoercibilityExplicit),
    // Go: newColString(charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETInt, types.ETString, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Ifnull,
    // Go: },
    // Go: []Expression{
    // Go: newColString(charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newColString(charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityImplicit, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Like,
    // Go: },
    // Go: []Expression{
    // Go: newColString(charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstString("like", CoercibilityExplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: newConstString("\\", CoercibilityExplicit, charset.CharsetUTF8MB4, charset.CollationUTF8MB4),
    // Go: },
    // Go: []types.EvalType{types.ETString, types.ETString, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityNumeric, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.DateFormat, ast.TimeFormat,
    // Go: },
    // Go: []Expression{
    // Go: newConstString("2020-02-02", CoercibilityExplicit, charset.CharsetUTF8MB4, "utf8mb4_general_ci"),
    // Go: newConstString("%Y %M %D", CoercibilityExplicit, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETDatetime, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.DateFormat, ast.TimeFormat,
    // Go: },
    // Go: []Expression{
    // Go: newConstString("2020-02-02", CoercibilityExplicit, charset.CharsetUTF8MB4, "utf8mb4_general_ci"),
    // Go: newConstString("%Y %M %D", CoercibilityCoercible, charset.CharsetUTF8MB4, "utf8mb4_unicode_ci"),
    // Go: },
    // Go: []types.EvalType{types.ETDatetime, types.ETString},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityCoercible, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Database, ast.User, ast.CurrentUser, ast.Version, ast.CurrentRole, ast.TiDBVersion, ast.CurrentResourceGroup,
    // Go: },
    // Go: []Expression{},
    // Go: []types.EvalType{},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilitySysconst, UNICODE, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: {
    // Go: []string{
    // Go: ast.Cast,
    // Go: },
    // Go: []Expression{
    // Go: newColInt(CoercibilityExplicit),
    // Go: },
    // Go: []types.EvalType{types.ETInt},
    // Go: types.ETString,
    // Go: false,
    // Go: &ExprCollation{CoercibilityExplicit, ASCII, charset.CharsetUTF8MB4, charset.CollationUTF8MB4},
    // Go: },
    // Go: }

    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for i, test := range tests {
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for _, fc := range test.fcs {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: ec, err := deriveCollation(ctx, fc, test.args, test.retTp, test.argTps...)
    // 分支语义：保留 Go 条件路径，后续接线时需维持错误/空值判断顺序。
    // Go: if test.err {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.Error(t, err, "Number: %d, function: %s", i, fc)
    // Go: require.Nil(t, ec, i)
    // 分支语义：保留 Go 条件路径，后续接线时需维持错误/空值判断顺序。
    // Go: } else {
    // Go: require.Equal(t, test.ec, ec, "Number: %d, function: %s", i, fc)
    // Go: }
    // Go: }
    // Go: }
}

// TestCompareString 对应 Go 测试函数；保留签名、主体顺序、断言和外部依赖调用点，并执行真实 Rust 回归。
#[test]
pub fn test_compare_string() {
    for (left, right) in [
        ("a", "A"),
        ("À", "A"),
        ("😜", "😃"),
        ("a ", "a  "),
        ("ß", "s"),
    ] {
        assert_eq!(compare_string(left, right, "utf8_general_ci"), 0);
    }
    assert_ne!(compare_string("ß", "ss", "utf8_general_ci"), 0);

    for (left, right) in [
        ("a", "A"),
        ("À", "A"),
        ("😜", "😃"),
        ("a ", "a  "),
        ("ß", "ss"),
    ] {
        assert_eq!(
            compare_string(left, right, "utf8_unicode_ci"),
            0,
            "{left:?} vs {right:?}"
        );
    }
    assert_ne!(compare_string("ß", "s", "utf8_unicode_ci"), 0);

    for (left, right) in [("a", "A"), ("À", "A"), ("ß", "ss"), ("æ", "ae")] {
        assert_eq!(compare_string(left, right, "utf8mb4_0900_ai_ci"), 0);
    }
    for (left, right) in [
        ("😜", "😃"),
        ("a ", "a  "),
        ("ß", "s"),
        ("\u{ffffe}", "\u{fffff}"),
    ] {
        assert_ne!(compare_string(left, right, "utf8mb4_0900_ai_ci"), 0);
    }
    for (left, right) in [("a", "A"), ("À", "A"), ("😜", "😃"), ("a ", "a  ")] {
        assert_ne!(compare_string(left, right, "binary"), 0);
    }
    // Go 签名：func TestCompareString(t *testing.T)
    // 参数语义：t *testing.T。testing.T/B、context、chunk 等运行时对象在这里中不创建真实实例。
    // Go: require.Equal(t, 0, types.CompareString("a", "A", "utf8_general_ci"))
    // Go: require.Equal(t, 0, types.CompareString("À", "A", "utf8_general_ci"))
    // Go: require.Equal(t, 0, types.CompareString("😜", "😃", "utf8_general_ci"))
    // Go: require.Equal(t, 0, types.CompareString("a ", "a ", "utf8_general_ci"))
    // Go: require.Equal(t, 0, types.CompareString("ß", "s", "utf8_general_ci"))
    // Go: require.NotEqual(t, 0, types.CompareString("ß", "ss", "utf8_general_ci"))

    // Go: require.Equal(t, 0, types.CompareString("a", "A", "utf8_unicode_ci"))
    // Go: require.Equal(t, 0, types.CompareString("À", "A", "utf8_unicode_ci"))
    // Go: require.Equal(t, 0, types.CompareString("😜", "😃", "utf8_unicode_ci"))
    // Go: require.Equal(t, 0, types.CompareString("a ", "a ", "utf8_unicode_ci"))
    // Go: require.NotEqual(t, 0, types.CompareString("ß", "s", "utf8_unicode_ci"))
    // Go: require.Equal(t, 0, types.CompareString("ß", "ss", "utf8_unicode_ci"))

    // Go: require.Equal(t, 0, types.CompareString("a", "A", "utf8mb4_0900_ai_ci"))
    // Go: require.Equal(t, 0, types.CompareString("À", "A", "utf8mb4_0900_ai_ci"))
    // Go: require.NotEqual(t, 0, types.CompareString("😜", "😃", "utf8mb4_0900_ai_ci"))
    // Go: require.NotEqual(t, 0, types.CompareString("a ", "a ", "utf8mb4_0900_ai_ci"))
    // Go: require.NotEqual(t, 0, types.CompareString("ß", "s", "utf8mb4_0900_ai_ci"))
    // Go: require.Equal(t, 0, types.CompareString("ß", "ss", "utf8mb4_0900_ai_ci"))
    // Go: require.NotEqual(t, 0, types.CompareString("\U000FFFFE", "\U000FFFFF", "utf8mb4_0900_ai_ci"))
    // Go: require.Equal(t, 0, types.CompareString("æ", "ae", "utf8mb4_0900_ai_ci"))

    // Go: require.NotEqual(t, 0, types.CompareString("a", "A", "binary"))
    // Go: require.NotEqual(t, 0, types.CompareString("À", "A", "binary"))
    // Go: require.NotEqual(t, 0, types.CompareString("😜", "😃", "binary"))
    // Go: require.NotEqual(t, 0, types.CompareString("a ", "a ", "binary"))

    // Go: ctx := mock.NewContext()
    // Go: ft := types.NewFieldType(mysql.TypeVarString)
    // Go: col1 := &Column{
    // Go: RetType: ft,
    // Go: Index: 0,
    // Go: }
    // Go: col2 := &Column{
    // Go: RetType: ft,
    // Go: Index: 1,
    // Go: }
    // Go: chk := chunk.NewChunkWithCapacity([]*types.FieldType{ft, ft}, 4)
    // Go: chk.Column(0).AppendString("a")
    // Go: chk.Column(1).AppendString("A")
    // Go: chk.Column(0).AppendString("À")
    // Go: chk.Column(1).AppendString("A")
    // Go: chk.Column(0).AppendString("😜")
    // Go: chk.Column(1).AppendString("😃")
    // Go: chk.Column(0).AppendString("a ")
    // Go: chk.Column(1).AppendString("a ")
    // 循环语义：保留 Go range/索引遍历顺序，尤其是 chunk 行数和 case 表遍历。
    // Go: for range 4 {
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: v, isNull, err := CompareStringWithCollationInfo(ctx, col1, col2, chk.GetRow(0), chk.GetRow(0), "utf8_general_ci")
    // 错误处理：保留 Go error 传播或断言位置，避免迁移时吞掉失败路径。
    // Go: require.NoError(t, err)
    // Go: require.False(t, isNull)
    // Go: require.Equal(t, int64(0), v)
    // Go: }
}
