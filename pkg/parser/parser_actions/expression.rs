// Copyright 2026 AsterSQL.

// Copyright 2015 PingCAP, Inc.
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

use super::super::*;
use super::{Context, Rhs};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ExpressionRule {
    ColumnNameAlt01,
    ColumnNameAlt02,
    ColumnNameAlt03,
    ColumnNameListAlt01,
    ColumnNameListAlt02,
    DefaultValueExprAlt06,
    BuiltinFunctionAlt01,
    BuiltinFunctionAlt02,
    BuiltinFunctionAlt03,
    BuiltinFunctionAlt04,
    BuiltinFunctionAlt05,
    NowSymOptionFractionParenthesesAlt01,
    NowSymOptionFractionAlt01,
    NowSymOptionFractionAlt02,
    NowSymOptionFractionAlt03,
    NowSymOptionFractionAlt04,
    NowSymOptionFractionAlt05,
    NextValueForSequenceParenthesesAlt01,
    NextValueForSequenceAlt01,
    NextValueForSequenceAlt02,
    SignedLiteralAlt01,
    SignedLiteralAlt02,
    SignedLiteralAlt03,
    LengthNumAlt01,
    Int64NumAlt01,
    ExpressionAlt01,
    ExpressionAlt02,
    ExpressionAlt03,
    ExpressionAlt04,
    ExpressionAlt05,
    ExpressionAlt06,
    ExpressionAlt07,
    ExpressionAlt08,
    ExpressionAlt09,
    DefaultOrExpressionAlt01,
    MaxValueOrExpressionAlt01,
    FulltextSearchModifierOptAlt01,
    FulltextSearchModifierOptAlt02,
    FulltextSearchModifierOptAlt03,
    FulltextSearchModifierOptAlt04,
    FulltextSearchModifierOptAlt05,
    ExpressionListAlt01,
    ExpressionListAlt02,
    MaxValueOrExpressionListAlt01,
    MaxValueOrExpressionListAlt02,
    DefaultOrExpressionListAlt01,
    DefaultOrExpressionListAlt02,
    ExpressionListOptAlt01,
    FuncDatetimePrecListOptAlt01,
    FuncDatetimePrecListAlt01,
    BoolPriAlt01,
    BoolPriAlt02,
    BoolPriAlt03,
    BoolPriAlt04,
    CompareOpAlt01,
    CompareOpAlt02,
    CompareOpAlt03,
    CompareOpAlt04,
    CompareOpAlt05,
    CompareOpAlt06,
    CompareOpAlt07,
    CompareOpAlt08,
    BetweenOrNotOpAlt01,
    BetweenOrNotOpAlt02,
    IsOrNotOpAlt01,
    IsOrNotOpAlt02,
    InOrNotOpAlt01,
    InOrNotOpAlt02,
    LikeOrNotOpAlt01,
    LikeOrNotOpAlt02,
    IlikeOrNotOpAlt01,
    IlikeOrNotOpAlt02,
    RegexpOrNotOpAlt01,
    RegexpOrNotOpAlt02,
    AnyOrAllAlt01,
    AnyOrAllAlt02,
    AnyOrAllAlt03,
    PredicateExprAlt01,
    PredicateExprAlt02,
    PredicateExprAlt03,
    PredicateExprAlt04,
    PredicateExprAlt05,
    PredicateExprAlt06,
    PredicateExprAlt07,
    LikeOrIlikeEscapeOptAlt01,
    LikeOrIlikeEscapeOptAlt02,
    LiteralAlt01,
    LiteralAlt02,
    LiteralAlt03,
    LiteralAlt04,
    LiteralAlt05,
    LiteralAlt06,
    LiteralAlt08,
    LiteralAlt09,
    LiteralAlt10,
    LiteralAlt11,
    LiteralAlt12,
    StringLiteralAlt01,
    StringLiteralAlt02,
    BitExprAlt01,
    BitExprAlt02,
    BitExprAlt03,
    BitExprAlt04,
    BitExprAlt05,
    BitExprAlt06,
    BitExprAlt07,
    BitExprAlt08,
    BitExprAlt09,
    BitExprAlt10,
    BitExprAlt11,
    BitExprAlt12,
    BitExprAlt13,
    BitExprAlt14,
    BitExprAlt15,
    SimpleIdentAlt01,
    SimpleIdentAlt02,
    SimpleIdentAlt03,
    SimpleExprAlt05,
    SimpleExprAlt08,
    SimpleExprAlt11,
    SimpleExprAlt12,
    SimpleExprAlt13,
    SimpleExprAlt14,
    SimpleExprAlt15,
    SimpleExprAlt16,
    SimpleExprAlt18,
    SimpleExprAlt19,
    SimpleExprAlt20,
    SimpleExprAlt21,
    SimpleExprAlt22,
    SimpleExprAlt23,
    SimpleExprAlt24,
    SimpleExprAlt25,
    SimpleExprAlt26,
    SimpleExprAlt27,
    SimpleExprAlt28,
    SimpleExprAlt29,
    SimpleExprAlt30,
    SimpleExprAlt31,
    SimpleExprAlt32,
    ArrayKwdOptAlt01,
    ArrayKwdOptAlt02,
    DistinctOptAlt01,
    DistinctOptAlt02,
    DefaultFalseDistinctOptAlt01,
    DefaultTrueDistinctOptAlt01,
    BuggyDefaultFalseDistinctOptAlt02,
    FunctionCallKeywordAlt01,
    FunctionCallKeywordAlt02,
    FunctionCallKeywordAlt03,
    FunctionCallKeywordAlt04,
    FunctionCallKeywordAlt05,
    FunctionCallKeywordAlt06,
    FunctionCallKeywordAlt07,
    FunctionCallKeywordAlt08,
    FunctionCallKeywordAlt09,
    FunctionCallKeywordAlt10,
    FunctionCallKeywordAlt11,
    FunctionCallKeywordAlt12,
    FunctionCallKeywordAlt13,
    FunctionCallNonKeywordAlt01,
    FunctionCallNonKeywordAlt02,
    FunctionCallNonKeywordAlt03,
    FunctionCallNonKeywordAlt04,
    FunctionCallNonKeywordAlt05,
    FunctionCallNonKeywordAlt06,
    FunctionCallNonKeywordAlt07,
    FunctionCallNonKeywordAlt08,
    FunctionCallNonKeywordAlt09,
    FunctionCallNonKeywordAlt10,
    FunctionCallNonKeywordAlt11,
    FunctionCallNonKeywordAlt12,
    FunctionCallNonKeywordAlt13,
    FunctionCallNonKeywordAlt14,
    FunctionCallNonKeywordAlt15,
    FunctionCallNonKeywordAlt16,
    FunctionCallNonKeywordAlt17,
    FunctionCallNonKeywordAlt18,
    FunctionCallNonKeywordAlt19,
    FunctionCallNonKeywordAlt20,
    FunctionCallNonKeywordAlt21,
    FunctionCallNonKeywordAlt22,
    FunctionCallNonKeywordAlt23,
    FunctionCallNonKeywordAlt24,
    GetFormatSelectorAlt01,
    GetFormatSelectorAlt02,
    GetFormatSelectorAlt03,
    GetFormatSelectorAlt04,
    TrimDirectionAlt01,
    TrimDirectionAlt02,
    TrimDirectionAlt03,
    FunctionNameSequenceAlt01,
    FunctionNameSequenceAlt02,
    SumExprAlt01,
    SumExprAlt02,
    SumExprAlt03,
    SumExprAlt04,
    SumExprAlt05,
    SumExprAlt06,
    SumExprAlt07,
    SumExprAlt08,
    SumExprAlt09,
    SumExprAlt10,
    SumExprAlt11,
    SumExprAlt12,
    SumExprAlt13,
    SumExprAlt14,
    SumExprAlt15,
    SumExprAlt16,
    SumExprAlt17,
    SumExprAlt18,
    SumExprAlt19,
    SumExprAlt20,
    SumExprAlt21,
    SumExprAlt22,
    SumExprAlt23,
    SumExprAlt24,
    SumExprAlt25,
    SumExprAlt26,
    SumExprAlt27,
    SumExprAlt28,
    OptGConcatSeparatorAlt01,
    OptGConcatSeparatorAlt02,
    FunctionCallGenericAlt01,
    FunctionCallGenericAlt02,
    FuncDatetimePrecAlt01,
    FuncDatetimePrecAlt02,
    FuncDatetimePrecAlt03,
    TimeUnitAlt02,
    TimeUnitAlt03,
    TimeUnitAlt04,
    TimeUnitAlt05,
    TimeUnitAlt06,
    TimeUnitAlt07,
    TimeUnitAlt08,
    TimeUnitAlt09,
    TimeUnitAlt10,
    TimeUnitAlt11,
    TimeUnitAlt12,
    TimestampUnitAlt01,
    TimestampUnitAlt02,
    TimestampUnitAlt03,
    TimestampUnitAlt04,
    TimestampUnitAlt05,
    TimestampUnitAlt06,
    TimestampUnitAlt07,
    TimestampUnitAlt08,
    TimestampUnitAlt09,
    TimestampUnitAlt10,
    TimestampUnitAlt11,
    TimestampUnitAlt12,
    TimestampUnitAlt13,
    TimestampUnitAlt14,
    TimestampUnitAlt15,
    TimestampUnitAlt16,
    TimestampUnitAlt17,
    ExpressionOptAlt01,
    WhenClauseListAlt01,
    WhenClauseListAlt02,
    WhenClauseAlt01,
    ElseOptAlt01,
    ElseOptAlt02,
    CastTypeAlt01,
    CastTypeAlt02,
    CastTypeAlt03,
    CastTypeAlt04,
    CastTypeAlt05,
    CastTypeAlt06,
    CastTypeAlt07,
    CastTypeAlt08,
    CastTypeAlt09,
    CastTypeAlt10,
    CastTypeAlt11,
    CastTypeAlt12,
    CastTypeAlt13,
    CastTypeAlt14,
    CharsetNameAlt01,
    CharsetNameAlt02,
    CollationNameAlt01,
    CollationNameAlt02,
    SystemVariableAlt01,
    UserVariableAlt01,
    StringTypeAlt17,
    SignedNumAlt02,
    SignedNumAlt03,
}

fn identify(rule_id: RuleId) -> Option<ExpressionRule> {
    match rule_id.as_str() {
        "columnname_identifier--01ade32d68461e4e" => Some(ExpressionRule::ColumnNameAlt01),
        "columnname_identifier_identifier--ab6513b845c58833" => {
            Some(ExpressionRule::ColumnNameAlt02)
        }
        "columnname_identifier_identifier_identifier--aae04cfb91e5578c" => {
            Some(ExpressionRule::ColumnNameAlt03)
        }
        "columnnamelist_columnname--dfe3c849fbeecc1c" => Some(ExpressionRule::ColumnNameListAlt01),
        "columnnamelist_columnnamelist_columnname--149b2b543c44eec1" => {
            Some(ExpressionRule::ColumnNameListAlt02)
        }
        "builtinfunction_builtinfunction--ee9ed51f5429e8b9" => {
            Some(ExpressionRule::BuiltinFunctionAlt01)
        }
        "builtinfunction_identifier--76b2e30f4629f7ff" => {
            Some(ExpressionRule::BuiltinFunctionAlt02)
        }
        "builtinfunction_identifier_expressionlist--7a9321e701b39121" => {
            Some(ExpressionRule::BuiltinFunctionAlt03)
        }
        "builtinfunction_uuid--640f9e263a4fc6e1" => Some(ExpressionRule::BuiltinFunctionAlt04),
        "builtinfunction_replace_expressionlist--ee2bf6f1deacba46" => {
            Some(ExpressionRule::BuiltinFunctionAlt05)
        }
        "nowsymoptionfractionparentheses_nowsymoptionfrac--09fd6c788ca5baaf" => {
            Some(ExpressionRule::NowSymOptionFractionParenthesesAlt01)
        }
        "nowsymoptionfraction_nowsym--8cda2545b8401e77" => {
            Some(ExpressionRule::NowSymOptionFractionAlt01)
        }
        "nowsymoptionfraction_nowsymfunc--edced262e6453950" => {
            Some(ExpressionRule::NowSymOptionFractionAlt02)
        }
        "nowsymoptionfraction_nowsymfunc_num--30ed4ecb2c885178" => {
            Some(ExpressionRule::NowSymOptionFractionAlt03)
        }
        "nowsymoptionfraction_curdatesym--c1434c9e33bba390" => {
            Some(ExpressionRule::NowSymOptionFractionAlt04)
        }
        "nowsymoptionfraction_current_date--fe8799e016abf68a" => {
            Some(ExpressionRule::NowSymOptionFractionAlt05)
        }
        "nextvalueforsequenceparentheses_nextvalueforsequ--26d2d26ee7bd8233" => {
            Some(ExpressionRule::NextValueForSequenceParenthesesAlt01)
        }
        "nextvalueforsequence_next_value_forkwd_tablename--139d576e5bd1d82c" => {
            Some(ExpressionRule::NextValueForSequenceAlt01)
        }
        "nextvalueforsequence_nextval_tablename--ae0882f78d977cae" => {
            Some(ExpressionRule::NextValueForSequenceAlt02)
        }
        "signedliteral_literal--2c963c070bf0d464" => Some(ExpressionRule::SignedLiteralAlt01),
        "signedliteral_numliteral--2526c818c28ce897" => Some(ExpressionRule::SignedLiteralAlt02),
        "signedliteral_numliteral--cfa1f9aed7ed82e1" => Some(ExpressionRule::SignedLiteralAlt03),
        "lengthnum_num--31b55f6dd352af9a" => Some(ExpressionRule::LengthNumAlt01),
        "int64num_num--a86badba1b2d3913" => Some(ExpressionRule::Int64NumAlt01),
        "expression_singleatidentifier_assignmenteq_expre--8f36cca5c5eab6f5" => {
            Some(ExpressionRule::ExpressionAlt01)
        }
        "expression_expression_logor_expression_prec_pipe--3421661b1689a079" => {
            Some(ExpressionRule::ExpressionAlt02)
        }
        "expression_expression_xor_expression_prec_xor--a0970bca92b9077d" => {
            Some(ExpressionRule::ExpressionAlt03)
        }
        "expression_expression_logand_expression_prec_and--ae986e6b53bd617a" => {
            Some(ExpressionRule::ExpressionAlt04)
        }
        "expression_not_expression_prec_not--435ee87cf3d7497b" => {
            Some(ExpressionRule::ExpressionAlt05)
        }
        "expression_match_columnnamelist_against_bitexpr--80fdc4275d2b3d97" => {
            Some(ExpressionRule::ExpressionAlt06)
        }
        "expression_boolpri_isornotop_truekwd_prec_is--a42d7f7da9d48465" => {
            Some(ExpressionRule::ExpressionAlt07)
        }
        "expression_boolpri_isornotop_falsekwd_prec_is--e1daa77e543d00ca" => {
            Some(ExpressionRule::ExpressionAlt08)
        }
        "expression_boolpri_isornotop_unknown_prec_is--49835f27474b8d51" => {
            Some(ExpressionRule::ExpressionAlt09)
        }
        "defaultorexpression_default--0cf117916adb08c3" => {
            Some(ExpressionRule::DefaultOrExpressionAlt01)
        }
        "maxvalueorexpression_maxvalue--71e6b6f4ab050681" => {
            Some(ExpressionRule::MaxValueOrExpressionAlt01)
        }
        "fulltextsearchmodifieropt--e27db1ee7c669893" => {
            Some(ExpressionRule::FulltextSearchModifierOptAlt01)
        }
        "fulltextsearchmodifieropt_in_natural_language_mo--d5e99e7d002b6c93" => {
            Some(ExpressionRule::FulltextSearchModifierOptAlt02)
        }
        "fulltextsearchmodifieropt_in_natural_language_mo--ee6da5127adbc94e" => {
            Some(ExpressionRule::FulltextSearchModifierOptAlt03)
        }
        "fulltextsearchmodifieropt_in_boolean_mode--3ddcdf6c70c10020" => {
            Some(ExpressionRule::FulltextSearchModifierOptAlt04)
        }
        "fulltextsearchmodifieropt_with_query_expansion--72aebd8e05d4d6e9" => {
            Some(ExpressionRule::FulltextSearchModifierOptAlt05)
        }
        "expressionlist_expression--2c4aeef656c132f0" => Some(ExpressionRule::ExpressionListAlt01),
        "expressionlist_expressionlist_expression--d4ab1f23eb0b9346" => {
            Some(ExpressionRule::ExpressionListAlt02)
        }
        "maxvalueorexpressionlist_maxvalueorexpression--97196c16c2218ec0" => {
            Some(ExpressionRule::MaxValueOrExpressionListAlt01)
        }
        "maxvalueorexpressionlist_maxvalueorexpressionlis--fbfaa536a1d17a38" => {
            Some(ExpressionRule::MaxValueOrExpressionListAlt02)
        }
        "defaultorexpressionlist_defaultorexpression--2a69d284bd6785c2" => {
            Some(ExpressionRule::DefaultOrExpressionListAlt01)
        }
        "defaultorexpressionlist_defaultorexpressionlist--793c9d21ed103af0" => {
            Some(ExpressionRule::DefaultOrExpressionListAlt02)
        }
        "expressionlistopt--0ed35bcbdb75f080" => Some(ExpressionRule::ExpressionListOptAlt01),
        "funcdatetimepreclistopt--e1d44f842af1fec1" => {
            Some(ExpressionRule::FuncDatetimePrecListOptAlt01)
        }
        "funcdatetimepreclist_intlit--3c9f69b3e340c2df" => {
            Some(ExpressionRule::FuncDatetimePrecListAlt01)
        }
        "boolpri_boolpri_isornotop_null_prec_is--9c4f31d00200f6f1" => {
            Some(ExpressionRule::BoolPriAlt01)
        }
        "boolpri_boolpri_compareop_predicateexpr_prec_eq--2847685f5add53a9" => {
            Some(ExpressionRule::BoolPriAlt02)
        }
        "boolpri_boolpri_compareop_anyorall_subselect_pre--8a9b6e9ff7391b9f" => {
            Some(ExpressionRule::BoolPriAlt03)
        }
        "boolpri_boolpri_compareop_singleatidentifier_ass--b7ecec2d97e8ab75" => {
            Some(ExpressionRule::BoolPriAlt04)
        }
        "compareop--44a802ae026aae75" => Some(ExpressionRule::CompareOpAlt01),
        "compareop--d4799b8be039d198" => Some(ExpressionRule::CompareOpAlt02),
        "compareop--3449b8adf96a8ddb" => Some(ExpressionRule::CompareOpAlt03),
        "compareop--d473138be0347ed2" => Some(ExpressionRule::CompareOpAlt04),
        "compareop--5fad83ae1203da28" => Some(ExpressionRule::CompareOpAlt05),
        "compareop--344d1eadf96d7104" => Some(ExpressionRule::CompareOpAlt06),
        "compareop--d476358be036ee6f" => Some(ExpressionRule::CompareOpAlt07),
        "compareop--438ed09ecfd57ee7" => Some(ExpressionRule::CompareOpAlt08),
        "betweenornotop_between--d87508d88e57715f" => Some(ExpressionRule::BetweenOrNotOpAlt01),
        "betweenornotop_notsym_between--0bf17efec9b4b595" => {
            Some(ExpressionRule::BetweenOrNotOpAlt02)
        }
        "isornotop_is--ff5aec675f8b76eb" => Some(ExpressionRule::IsOrNotOpAlt01),
        "isornotop_is_notsym--94ecb674b047f00d" => Some(ExpressionRule::IsOrNotOpAlt02),
        "inornotop_in--f7a03683c445d017" => Some(ExpressionRule::InOrNotOpAlt01),
        "inornotop_notsym_in--089912548922132f" => Some(ExpressionRule::InOrNotOpAlt02),
        "likeornotop_like--af9b1f1f5d78cbbb" => Some(ExpressionRule::LikeOrNotOpAlt01),
        "likeornotop_notsym_like--1c0c88729fb2d7b3" => Some(ExpressionRule::LikeOrNotOpAlt02),
        "ilikeornotop_ilike--c58e7fa15677238f" => Some(ExpressionRule::IlikeOrNotOpAlt01),
        "ilikeornotop_notsym_ilike--34044f29a800d0b5" => Some(ExpressionRule::IlikeOrNotOpAlt02),
        "regexpornotop_regexpsym--0442984f14cc090c" => Some(ExpressionRule::RegexpOrNotOpAlt01),
        "regexpornotop_notsym_regexpsym--ad1d960ef7cfc82c" => {
            Some(ExpressionRule::RegexpOrNotOpAlt02)
        }
        "anyorall_any--bba9f439b9e16bbc" => Some(ExpressionRule::AnyOrAllAlt01),
        "anyorall_some--3956372eef6c278a" => Some(ExpressionRule::AnyOrAllAlt02),
        "anyorall_all--cd0e2839c3c0459f" => Some(ExpressionRule::AnyOrAllAlt03),
        "predicateexpr_bitexpr_inornotop_expressionlist--2e9a4187aa2a12db" => {
            Some(ExpressionRule::PredicateExprAlt01)
        }
        "predicateexpr_bitexpr_inornotop_subselect--1154cc23b3900c9a" => {
            Some(ExpressionRule::PredicateExprAlt02)
        }
        "predicateexpr_bitexpr_betweenornotop_bitexpr_and--753a2d5eb83543e0" => {
            Some(ExpressionRule::PredicateExprAlt03)
        }
        "predicateexpr_bitexpr_likeornotop_simpleexpr_lik--2d6550c71bdf0079" => {
            Some(ExpressionRule::PredicateExprAlt04)
        }
        "predicateexpr_bitexpr_ilikeornotop_simpleexpr_li--0d41737d20c7debc" => {
            Some(ExpressionRule::PredicateExprAlt05)
        }
        "predicateexpr_bitexpr_regexpornotop_simpleexpr--c627facccb2b0337" => {
            Some(ExpressionRule::PredicateExprAlt06)
        }
        "predicateexpr_bitexpr_memberof_simpleexpr--8cac56ae568a2409" => {
            Some(ExpressionRule::PredicateExprAlt07)
        }
        "likeorilikeescapeopt_prec_empty--776b5d4ea372dc09" => {
            Some(ExpressionRule::LikeOrIlikeEscapeOptAlt01)
        }
        "likeorilikeescapeopt_escape_stringlit--841e024d9c6ed5a1" => {
            Some(ExpressionRule::LikeOrIlikeEscapeOptAlt02)
        }
        "literal_false--372444cc06ae64de" => Some(ExpressionRule::LiteralAlt01),
        "literal_null--fd4ce6dd718a07e2" => Some(ExpressionRule::LiteralAlt02),
        "literal_true--88e3b68357970c95" => Some(ExpressionRule::LiteralAlt03),
        "literal_floatlit--8b3c6c54f178ab26" => Some(ExpressionRule::LiteralAlt04),
        "literal_declit--9376589dc165f038" => Some(ExpressionRule::LiteralAlt05),
        "literal_intlit--07b34e81cb4dcf0b" => Some(ExpressionRule::LiteralAlt06),
        "literal_underscore_charset_stringlit--d794de068b3c180a" => {
            Some(ExpressionRule::LiteralAlt08)
        }
        "literal_hexlit--6fefcb046b74c13f" => Some(ExpressionRule::LiteralAlt09),
        "literal_bitlit--3e3b05b669eb4f39" => Some(ExpressionRule::LiteralAlt10),
        "literal_underscore_charset_hexlit--54a1ff89a337a866" => Some(ExpressionRule::LiteralAlt11),
        "literal_underscore_charset_bitlit--72734fd81e9e9358" => Some(ExpressionRule::LiteralAlt12),
        "stringliteral_stringlit--80e19945a955e144" => Some(ExpressionRule::StringLiteralAlt01),
        "stringliteral_stringliteral_stringlit--07025414de305818" => {
            Some(ExpressionRule::StringLiteralAlt02)
        }
        "bitexpr_bitexpr_bitexpr_prec--be5807ef82812983" => Some(ExpressionRule::BitExprAlt01),
        "bitexpr_bitexpr_bitexpr_prec--8793c554a774dd13" => Some(ExpressionRule::BitExprAlt02),
        "bitexpr_bitexpr_bitexpr_prec_lsh--469f03109b448e4a" => Some(ExpressionRule::BitExprAlt03),
        "bitexpr_bitexpr_bitexpr_prec_rsh--486b474181de0268" => Some(ExpressionRule::BitExprAlt04),
        "bitexpr_bitexpr_bitexpr_prec--444bb472769c1543" => Some(ExpressionRule::BitExprAlt05),
        "bitexpr_bitexpr_bitexpr_prec--02024b9e1fb83e3b" => Some(ExpressionRule::BitExprAlt06),
        "bitexpr_bitexpr_interval_expression_timeunit_pre--c488a6c3b2601e1f" => {
            Some(ExpressionRule::BitExprAlt07)
        }
        "bitexpr_bitexpr_interval_expression_timeunit_pre--a8927c42e52bb471" => {
            Some(ExpressionRule::BitExprAlt08)
        }
        "bitexpr_interval_expression_timeunit_bitexpr_pre--9554ea06eb76d15d" => {
            Some(ExpressionRule::BitExprAlt09)
        }
        "bitexpr_bitexpr_bitexpr_prec--1013803bbfaeab3b" => Some(ExpressionRule::BitExprAlt10),
        "bitexpr_bitexpr_bitexpr_prec--d4bd52a4cfccb8eb" => Some(ExpressionRule::BitExprAlt11),
        "bitexpr_bitexpr_bitexpr_prec--5afa6fdb71acc87b" => Some(ExpressionRule::BitExprAlt12),
        "bitexpr_bitexpr_div_bitexpr_prec_div--b690c011682742cf" => {
            Some(ExpressionRule::BitExprAlt13)
        }
        "bitexpr_bitexpr_mod_bitexpr_prec_mod--fa42d2064b54b9df" => {
            Some(ExpressionRule::BitExprAlt14)
        }
        "bitexpr_bitexpr_bitexpr--d3423a450caa143c" => Some(ExpressionRule::BitExprAlt15),
        "simpleident_identifier--fcbf0a744f1cb4cd" => Some(ExpressionRule::SimpleIdentAlt01),
        "simpleident_identifier_identifier--ddd47197f97cbe62" => {
            Some(ExpressionRule::SimpleIdentAlt02)
        }
        "simpleident_identifier_identifier_identifier--88018e5f8f674397" => {
            Some(ExpressionRule::SimpleIdentAlt03)
        }
        "simpleexpr_simpleexpr_collate_collationname--d96d79e384d51e6e" => {
            Some(ExpressionRule::SimpleExprAlt05)
        }
        "simpleexpr_parammarker--d50f020827d2cf00" => Some(ExpressionRule::SimpleExprAlt08),
        "simpleexpr_simpleexpr_prec_neg--8fc936c44434db6c" => Some(ExpressionRule::SimpleExprAlt11),
        "simpleexpr_simpleexpr_prec_neg--e16cb10bf6b27901" => Some(ExpressionRule::SimpleExprAlt12),
        "simpleexpr_simpleexpr_prec_neg--a155cac0013ab0c0" => Some(ExpressionRule::SimpleExprAlt13),
        "simpleexpr_simpleexpr_prec_neg--2e9cc0b5e643d5ee" => Some(ExpressionRule::SimpleExprAlt14),
        "simpleexpr_simpleexpr_pipes_simpleexpr--ea2591eaaadc982a" => {
            Some(ExpressionRule::SimpleExprAlt15)
        }
        "simpleexpr_not2_simpleexpr_prec_neg--c25f5ab41343aade" => {
            Some(ExpressionRule::SimpleExprAlt16)
        }
        "simpleexpr_expression--d001961949d4ea46" => Some(ExpressionRule::SimpleExprAlt18),
        "simpleexpr_expressionlist_expression--1c7edbbb05e7e9fe" => {
            Some(ExpressionRule::SimpleExprAlt19)
        }
        "simpleexpr_row_expressionlist_expression--e697c9441eb9ff5a" => {
            Some(ExpressionRule::SimpleExprAlt20)
        }
        "simpleexpr_exists_subselect--808b15c71735adc3" => Some(ExpressionRule::SimpleExprAlt21),
        "simpleexpr_identifier_expression--c81b0744f634bfa6" => {
            Some(ExpressionRule::SimpleExprAlt22)
        }
        "simpleexpr_binary_simpleexpr_prec_neg--39fae340588915fc" => {
            Some(ExpressionRule::SimpleExprAlt23)
        }
        "simpleexpr_builtincast_expression_as_casttype_ar--1383a1852e247457" => {
            Some(ExpressionRule::SimpleExprAlt24)
        }
        "simpleexpr_jsonsumcrc32_expression_as_casttype_a--abe39d228e87e820" => {
            Some(ExpressionRule::SimpleExprAlt25)
        }
        "simpleexpr_case_expressionopt_whenclauselist_els--7caa60219c63f082" => {
            Some(ExpressionRule::SimpleExprAlt26)
        }
        "simpleexpr_convert_expression_casttype--a47e524843e9b29e" => {
            Some(ExpressionRule::SimpleExprAlt27)
        }
        "simpleexpr_convert_expression_using_charsetname--2e20990c448e58c2" => {
            Some(ExpressionRule::SimpleExprAlt28)
        }
        "simpleexpr_default_simpleident--c710d9975ed248ed" => Some(ExpressionRule::SimpleExprAlt29),
        "simpleexpr_values_simpleident_prec_lowerthaninse--834f6b29318940b0" => {
            Some(ExpressionRule::SimpleExprAlt30)
        }
        "simpleexpr_simpleident_jss_stringlit--a761b18e38c2795d" => {
            Some(ExpressionRule::SimpleExprAlt31)
        }
        "simpleexpr_simpleident_juss_stringlit--2a2ce200e3655e10" => {
            Some(ExpressionRule::SimpleExprAlt32)
        }
        "arraykwdopt--43705341e824537d" => Some(ExpressionRule::ArrayKwdOptAlt01),
        "arraykwdopt_array--b1fccd866265cfa7" => Some(ExpressionRule::ArrayKwdOptAlt02),
        "distinctopt_all--7d08b3e0a72d6458" => Some(ExpressionRule::DistinctOptAlt01),
        "distinctopt_distinctkwd--4fd41aa4ae7c8f93" => Some(ExpressionRule::DistinctOptAlt02),
        "defaultfalsedistinctopt--c2c2f012a33fe6fa" => {
            Some(ExpressionRule::DefaultFalseDistinctOptAlt01)
        }
        "defaulttruedistinctopt--272c09acc835b1d7" => {
            Some(ExpressionRule::DefaultTrueDistinctOptAlt01)
        }
        "buggydefaultfalsedistinctopt_distinctkwd_all--1c4550c19ff7d256" => {
            Some(ExpressionRule::BuggyDefaultFalseDistinctOptAlt02)
        }
        "functioncallkeyword_functionnameconflict_express--6cb0c3f8dbeb4c22" => {
            Some(ExpressionRule::FunctionCallKeywordAlt01)
        }
        "functioncallkeyword_builtinuser_expressionlistop--8ce45b33d3309a11" => {
            Some(ExpressionRule::FunctionCallKeywordAlt02)
        }
        "functioncallkeyword_functionnameoptionalbraces_o--611e5e21ff73f67e" => {
            Some(ExpressionRule::FunctionCallKeywordAlt03)
        }
        "functioncallkeyword_builtincurdate--67dcedffa1099a1b" => {
            Some(ExpressionRule::FunctionCallKeywordAlt04)
        }
        "functioncallkeyword_functionnamedatetimeprecisio--4b443e75bd7dc0f2" => {
            Some(ExpressionRule::FunctionCallKeywordAlt05)
        }
        "functioncallkeyword_char_expressionlist--b5861cd2afd58fc8" => {
            Some(ExpressionRule::FunctionCallKeywordAlt06)
        }
        "functioncallkeyword_char_expressionlist_using_ch--722b7e867fe55df7" => {
            Some(ExpressionRule::FunctionCallKeywordAlt07)
        }
        "functioncallkeyword_date_stringlit--83af507b8b95e67b" => {
            Some(ExpressionRule::FunctionCallKeywordAlt08)
        }
        "functioncallkeyword_time_stringlit--6b069eef5bb1f96c" => {
            Some(ExpressionRule::FunctionCallKeywordAlt09)
        }
        "functioncallkeyword_timestamp_stringlit--6b2290c619bdc84d" => {
            Some(ExpressionRule::FunctionCallKeywordAlt10)
        }
        "functioncallkeyword_insert_expressionlistopt--81b8b014c050381c" => {
            Some(ExpressionRule::FunctionCallKeywordAlt11)
        }
        "functioncallkeyword_mod_expression_expression--4b0fecdde4c9b77a" => {
            Some(ExpressionRule::FunctionCallKeywordAlt12)
        }
        "functioncallkeyword_password_expressionlistopt--07a61d7533c81dbc" => {
            Some(ExpressionRule::FunctionCallKeywordAlt13)
        }
        "functioncallnonkeyword_builtincurtime_funcdateti--bccf2a5b9f73efd3" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt01)
        }
        "functioncallnonkeyword_builtinsysdate_funcdateti--079acf794b5333bd" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt02)
        }
        "functioncallnonkeyword_functionnamedatearithmult--9acff16d628ad2ac" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt03)
        }
        "functioncallnonkeyword_functionnamedatearithmult--08f5a8ccd4bc4254" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt04)
        }
        "functioncallnonkeyword_functionnamedatearith_exp--dabaf68894bed054" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt05)
        }
        "functioncallnonkeyword_builtinextract_timeunit_f--e47699e1eb009572" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt06)
        }
        "functioncallnonkeyword_get_format_getformatselec--6b1b2c9cad780a71" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt07)
        }
        "functioncallnonkeyword_builtinposition_bitexpr_i--1cd020aca7317846" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt08)
        }
        "functioncallnonkeyword_builtinsubstring_expressi--edb642b9ac69f761" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt09)
        }
        "functioncallnonkeyword_builtinsubstring_expressi--de8b1613efd447ed" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt10)
        }
        "functioncallnonkeyword_builtinsubstring_expressi--fcac4b63a9ee97d7" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt11)
        }
        "functioncallnonkeyword_builtinsubstring_expressi--d2b52043b01615bc" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt12)
        }
        "functioncallnonkeyword_timestampadd_timestampuni--2deeeb3d223bef94" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt13)
        }
        "functioncallnonkeyword_timestampdiff_timestampun--57eda886b2c3c030" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt14)
        }
        "functioncallnonkeyword_builtintrim_expression--e2b7ccb5f4105cb4" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt15)
        }
        "functioncallnonkeyword_builtintrim_expression_fr--3805a1d34f3e68e6" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt16)
        }
        "functioncallnonkeyword_builtintrim_trimdirection--0e8e88640ee04537" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt17)
        }
        "functioncallnonkeyword_builtintrim_trimdirection--d21b71eb918bf4f1" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt18)
        }
        "functioncallnonkeyword_weightstring_expression--bcbf0d291b0997ce" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt19)
        }
        "functioncallnonkeyword_weightstring_expression_a--53f2ae3f73c4ac2f" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt20)
        }
        "functioncallnonkeyword_weightstring_expression_a--9057389c077b3ea2" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt21)
        }
        "functioncallnonkeyword_builtintranslate_expressi--6f12815c328a7cd8" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt23)
        }
        "functioncallnonkeyword_compress_expressionlistop--5ac197885e091884" => {
            Some(ExpressionRule::FunctionCallNonKeywordAlt24)
        }
        "getformatselector_date--5af7b0675b208ee8" => Some(ExpressionRule::GetFormatSelectorAlt01),
        "getformatselector_datetime--6a60f58f62c9fd5b" => {
            Some(ExpressionRule::GetFormatSelectorAlt02)
        }
        "getformatselector_time--dc7ef7a138da6edf" => Some(ExpressionRule::GetFormatSelectorAlt03),
        "getformatselector_timestamp--1b4ddbcac73f38fc" => {
            Some(ExpressionRule::GetFormatSelectorAlt04)
        }
        "trimdirection_both--512cb4791b041b8e" => Some(ExpressionRule::TrimDirectionAlt01),
        "trimdirection_leading--ab7ef65f8f7ec5f3" => Some(ExpressionRule::TrimDirectionAlt02),
        "trimdirection_trailing--896c39155b91c82f" => Some(ExpressionRule::TrimDirectionAlt03),
        "functionnamesequence_lastval_tablename--0f6454e82ebccf69" => {
            Some(ExpressionRule::FunctionNameSequenceAlt01)
        }
        "functionnamesequence_setval_tablename_signednum--c18d3628167e0e1d" => {
            Some(ExpressionRule::FunctionNameSequenceAlt02)
        }
        "sumexpr_avg_buggydefaultfalsedistinctopt_express--2c0776a63c108f4a" => {
            Some(ExpressionRule::SumExprAlt01)
        }
        "sumexpr_builtinapproxcountdistinct_expressionlis--4169eb5e551ae363" => {
            Some(ExpressionRule::SumExprAlt02)
        }
        "sumexpr_builtinapproxpercentile_expressionlist--98a4ea3026a45179" => {
            Some(ExpressionRule::SumExprAlt03)
        }
        "sumexpr_builtinbitand_expression_optwindowingcla--075d5b60d8f70bd6" => {
            Some(ExpressionRule::SumExprAlt04)
        }
        "sumexpr_builtinbitand_all_expression_optwindowin--4f4edaad7dbd496f" => {
            Some(ExpressionRule::SumExprAlt05)
        }
        "sumexpr_builtinbitor_expression_optwindowingclau--62f77fe770c5f46a" => {
            Some(ExpressionRule::SumExprAlt06)
        }
        "sumexpr_builtinbitor_all_expression_optwindowing--c915618112135643" => {
            Some(ExpressionRule::SumExprAlt07)
        }
        "sumexpr_builtinbitxor_expression_optwindowingcla--524f9e1ca44bc21a" => {
            Some(ExpressionRule::SumExprAlt08)
        }
        "sumexpr_builtinbitxor_all_expression_optwindowin--214492d6d1833bb3" => {
            Some(ExpressionRule::SumExprAlt09)
        }
        "sumexpr_builtincount_distinctkwd_expressionlist--f4a0524351691591" => {
            Some(ExpressionRule::SumExprAlt10)
        }
        "sumexpr_builtincount_all_expression_optwindowing--0d4f9e9620793c46" => {
            Some(ExpressionRule::SumExprAlt11)
        }
        "sumexpr_builtincount_expression_optwindowingclau--76cc81c39dd2e55f" => {
            Some(ExpressionRule::SumExprAlt12)
        }
        "sumexpr_builtincount_optwindowingclause--6ab8953b7f77b14f" => {
            Some(ExpressionRule::SumExprAlt13)
        }
        "sumexpr_builtingroupconcat_buggydefaultfalsedist--45e47a8e74e9accc" => {
            Some(ExpressionRule::SumExprAlt14)
        }
        "sumexpr_builtinmax_buggydefaultfalsedistinctopt--4a07f24d170b2143" => {
            Some(ExpressionRule::SumExprAlt15)
        }
        "sumexpr_builtinmin_buggydefaultfalsedistinctopt--ae382207893b77cd" => {
            Some(ExpressionRule::SumExprAlt16)
        }
        "sumexpr_builtinsum_buggydefaultfalsedistinctopt--cd52dba6b858ef90" => {
            Some(ExpressionRule::SumExprAlt17)
        }
        "sumexpr_builtinsumint_buggydefaultfalsedistincto--857e558e9b620a83" => {
            Some(ExpressionRule::SumExprAlt18)
        }
        "sumexpr_builtinstddevpop_buggydefaultfalsedistin--695c61ded74ed5b4" => {
            Some(ExpressionRule::SumExprAlt19)
        }
        "sumexpr_builtinstddevsamp_buggydefaultfalsedisti--5416f2f9517bae50" => {
            Some(ExpressionRule::SumExprAlt20)
        }
        "sumexpr_builtinvarpop_buggydefaultfalsedistincto--1e6f7e95992a8fbf" => {
            Some(ExpressionRule::SumExprAlt21)
        }
        "sumexpr_builtinvarsamp_buggydefaultfalsedistinct--e7f017d69a8b851d" => {
            Some(ExpressionRule::SumExprAlt22)
        }
        "sumexpr_json_arrayagg_expression_optwindowingcla--ea81f1025b3b951a" => {
            Some(ExpressionRule::SumExprAlt23)
        }
        "sumexpr_json_arrayagg_all_expression_optwindowin--b0772796ad1b36b3" => {
            Some(ExpressionRule::SumExprAlt24)
        }
        "sumexpr_json_objectagg_expression_expression_opt--d30050e5f433b242" => {
            Some(ExpressionRule::SumExprAlt25)
        }
        "sumexpr_json_objectagg_all_expression_expression--887b235411b47a29" => {
            Some(ExpressionRule::SumExprAlt26)
        }
        "sumexpr_json_objectagg_expression_all_expression--ab0d197caac6793b" => {
            Some(ExpressionRule::SumExprAlt27)
        }
        "sumexpr_json_objectagg_all_expression_all_expres--ec4c750ea8bbc3b8" => {
            Some(ExpressionRule::SumExprAlt28)
        }
        "optgconcatseparator--5a2539bec2ba21ca" => Some(ExpressionRule::OptGConcatSeparatorAlt01),
        "optgconcatseparator_separator_stringlit--4591b4a6d27a813a" => {
            Some(ExpressionRule::OptGConcatSeparatorAlt02)
        }
        "functioncallgeneric_identifier_expressionlistopt--ea6382bdf3fe9436" => {
            Some(ExpressionRule::FunctionCallGenericAlt01)
        }
        "functioncallgeneric_identifier_identifier_expres--f90fe735ea6b6cef" => {
            Some(ExpressionRule::FunctionCallGenericAlt02)
        }
        "funcdatetimeprec--bd224e3efa734358" => Some(ExpressionRule::FuncDatetimePrecAlt01),
        "funcdatetimeprec--7cf3058a8e5bf27e" => Some(ExpressionRule::FuncDatetimePrecAlt02),
        "funcdatetimeprec_intlit--37228fca7f39323a" => Some(ExpressionRule::FuncDatetimePrecAlt03),
        "timeunit_second_microsecond--e9896801bcae636e" => Some(ExpressionRule::TimeUnitAlt02),
        "timeunit_minute_microsecond--23b5a035d1796ff6" => Some(ExpressionRule::TimeUnitAlt03),
        "timeunit_minute_second--abb5668c16a16a02" => Some(ExpressionRule::TimeUnitAlt04),
        "timeunit_hour_microsecond--5ae243600cab30b8" => Some(ExpressionRule::TimeUnitAlt05),
        "timeunit_hour_second--fd7dbc97e8c59f0c" => Some(ExpressionRule::TimeUnitAlt06),
        "timeunit_hour_minute--201c205051ca11d4" => Some(ExpressionRule::TimeUnitAlt07),
        "timeunit_day_microsecond--a79387a3ea1a4792" => Some(ExpressionRule::TimeUnitAlt08),
        "timeunit_day_second--4c49dbb6c5073ef6" => Some(ExpressionRule::TimeUnitAlt09),
        "timeunit_day_minute--a1be3b3b2395d6c2" => Some(ExpressionRule::TimeUnitAlt10),
        "timeunit_day_hour--54a31b13a3772868" => Some(ExpressionRule::TimeUnitAlt11),
        "timeunit_year_month--288c5684419e9283" => Some(ExpressionRule::TimeUnitAlt12),
        "timestampunit_microsecond--873fd520f6cf1c38" => Some(ExpressionRule::TimestampUnitAlt01),
        "timestampunit_second--ed5ac6db100c1d8c" => Some(ExpressionRule::TimestampUnitAlt02),
        "timestampunit_minute--0ff92a9379109054" => Some(ExpressionRule::TimestampUnitAlt03),
        "timestampunit_hour--b1e2408cbef5739e" => Some(ExpressionRule::TimestampUnitAlt04),
        "timestampunit_day--501769c0b92a6ffe" => Some(ExpressionRule::TimestampUnitAlt05),
        "timestampunit_week--caacdc75ccff592c" => Some(ExpressionRule::TimestampUnitAlt06),
        "timestampunit_month--3121289efc51d81a" => Some(ExpressionRule::TimestampUnitAlt07),
        "timestampunit_quarter--ba474c23410fbed8" => Some(ExpressionRule::TimestampUnitAlt08),
        "timestampunit_year--6e9aa70fad6fb2df" => Some(ExpressionRule::TimestampUnitAlt09),
        "timestampunit_sql_tsi_second--f1ab1dd9c0f80d94" => {
            Some(ExpressionRule::TimestampUnitAlt10)
        }
        "timestampunit_sql_tsi_minute--e82a5810bd07f18c" => {
            Some(ExpressionRule::TimestampUnitAlt11)
        }
        "timestampunit_sql_tsi_hour--11e8661b73a50566" => Some(ExpressionRule::TimestampUnitAlt12),
        "timestampunit_sql_tsi_day--81822449fe2874a6" => Some(ExpressionRule::TimestampUnitAlt13),
        "timestampunit_sql_tsi_week--b4146bb966dcf644" => Some(ExpressionRule::TimestampUnitAlt14),
        "timestampunit_sql_tsi_month--b721e29ef2e9ff52" => Some(ExpressionRule::TimestampUnitAlt15),
        "timestampunit_sql_tsi_quarter--3d62c4c08abc89b0" => {
            Some(ExpressionRule::TimestampUnitAlt16)
        }
        "timestampunit_sql_tsi_year--21c2cd26386533d7" => Some(ExpressionRule::TimestampUnitAlt17),
        "expressionopt--4dff2b0ddef06e08" => Some(ExpressionRule::ExpressionOptAlt01),
        "whenclauselist_whenclause--918d9832dbcd7fcc" => Some(ExpressionRule::WhenClauseListAlt01),
        "whenclauselist_whenclauselist_whenclause--1415f19abaea0a21" => {
            Some(ExpressionRule::WhenClauseListAlt02)
        }
        "whenclause_when_expression_then_expression--0b6c1cf6b8eb6574" => {
            Some(ExpressionRule::WhenClauseAlt01)
        }
        "elseopt--7597478121cdc329" => Some(ExpressionRule::ElseOptAlt01),
        "elseopt_else_expression--514d51f9c6ba42fb" => Some(ExpressionRule::ElseOptAlt02),
        "casttype_binary_optfieldlen--67ee6ce559693b8e" => Some(ExpressionRule::CastTypeAlt01),
        "casttype_char_optfieldlen_optbinary--e53ea511cb69d16d" => {
            Some(ExpressionRule::CastTypeAlt02)
        }
        "casttype_date--5a326cc6643a40af" => Some(ExpressionRule::CastTypeAlt03),
        "casttype_year--f010dbb41c64c76c" => Some(ExpressionRule::CastTypeAlt04),
        "casttype_datetime_optfieldlen--d3a238a390d7eab6" => Some(ExpressionRule::CastTypeAlt05),
        "casttype_decimal_floatopt--3faf7a6bdfb2de03" => Some(ExpressionRule::CastTypeAlt06),
        "casttype_time_optfieldlen--2150208d51e722ba" => Some(ExpressionRule::CastTypeAlt07),
        "casttype_signed_optinteger--e3908147841dbef4" => Some(ExpressionRule::CastTypeAlt08),
        "casttype_unsigned_optinteger--2b0b732acf947d27" => Some(ExpressionRule::CastTypeAlt09),
        "casttype_json--176b86924a0422e9" => Some(ExpressionRule::CastTypeAlt10),
        "casttype_double--456ec722f529a324" => Some(ExpressionRule::CastTypeAlt11),
        "casttype_float_floatopt--541630a30fbda7b2" => Some(ExpressionRule::CastTypeAlt12),
        "casttype_real--f7d81d906aa7d887" => Some(ExpressionRule::CastTypeAlt13),
        "casttype_vector_optvectorelementtype_optfieldlen--3ceb32baf1400432" => {
            Some(ExpressionRule::CastTypeAlt14)
        }
        "charsetname_stringname--359a6d640da74b17" => Some(ExpressionRule::CharsetNameAlt01),
        "charsetname_binarytype--3af215ef4e60a3d8" => Some(ExpressionRule::CharsetNameAlt02),
        "collationname_stringname--7aec0a29ac7dc864" => Some(ExpressionRule::CollationNameAlt01),
        "collationname_binarytype--dfb7368e0fc63537" => Some(ExpressionRule::CollationNameAlt02),
        "systemvariable_doubleatidentifier--08dcc6ea402dbfe6" => {
            Some(ExpressionRule::SystemVariableAlt01)
        }
        "uservariable_singleatidentifier--00bfa7385acef505" => {
            Some(ExpressionRule::UserVariableAlt01)
        }
        "signednum_int64num--8e73a54651af07de" => Some(ExpressionRule::SignedNumAlt02),
        "signednum_num--cede1e14efce1aeb" => Some(ExpressionRule::SignedNumAlt03),
        _ => None,
    }
}

pub(super) fn owns(rule_id: RuleId) -> bool {
    identify(rule_id).is_some()
}

pub(super) fn apply(
    rule_id: RuleId,
    rhs: Rhs<'_>,
    context: Context<'_>,
) -> Option<Result<bool, isize>> {
    let rule = identify(rule_id)?;
    Some(apply_rule(rule, rhs, context))
}

fn apply_rule(rule: ExpressionRule, mut rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    let rhs_len = rhs.len();
    let Context {
        output: out,
        parser_state,
        lexer: yylex,
    } = context;
    match rule {
        ExpressionRule::ColumnNameAlt01 => {
            out.item = Some(Box::new(parser_ast::ColumnName {
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }));
        }
        ExpressionRule::ColumnNameAlt02 => {
            out.item = Some(Box::new(parser_ast::ColumnName {
                Table: parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }));
        }
        ExpressionRule::ColumnNameAlt03 => {
            out.item = Some(Box::new(parser_ast::ColumnName {
                Schema: parser_ast::NewCIStr(&rhs[rhs_len - (4)].ident),
                Table: parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
            }));
        }
        ExpressionRule::ColumnNameListAlt01 => {
            let column = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
                .cloned()
                .unwrap_or_default();
            out.item = Some(Box::new(vec![column]));
        }
        ExpressionRule::ColumnNameListAlt02 => {
            let mut columns = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                .cloned()
                .unwrap_or_default();
            if let Some(column) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ColumnName>())
            {
                columns.push(column.clone());
            }
            out.item = Some(Box::new(columns));
        }
        ExpressionRule::DefaultValueExprAlt06 | ExpressionRule::SignedLiteralAlt01 => {
            out.expr = rhs[rhs_len - (0)].expr.clone()
        }
        ExpressionRule::BuiltinFunctionAlt02 | ExpressionRule::BuiltinFunctionAlt04 => {
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                Vec::new(),
            ));
        }
        ExpressionRule::BuiltinFunctionAlt01
        | ExpressionRule::NowSymOptionFractionParenthesesAlt01
        | ExpressionRule::NextValueForSequenceParenthesesAlt01 => {
            out.expr = rhs[rhs_len - (1)].expr.clone()
        }
        ExpressionRule::BuiltinFunctionAlt03 | ExpressionRule::BuiltinFunctionAlt05 => {
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&rhs[rhs_len - (3)].ident),
                rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                    .cloned()
                    .unwrap_or_default(),
            ));
        }
        ExpressionRule::NowSymOptionFractionAlt01 | ExpressionRule::NowSymOptionFractionAlt02 => {
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("CURRENT_TIMESTAMP"),
                Vec::new(),
            ))
        }
        ExpressionRule::NowSymOptionFractionAlt03 => {
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("CURRENT_TIMESTAMP"),
                vec![parser_ast::ExprNode::Value(
                    rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .map(semantic_value_text)
                        .unwrap_or_default(),
                )],
            ))
        }
        ExpressionRule::NowSymOptionFractionAlt04 | ExpressionRule::NowSymOptionFractionAlt05 => {
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("CURRENT_DATE"),
                Vec::new(),
            ))
        }
        ExpressionRule::NextValueForSequenceAlt01 | ExpressionRule::NextValueForSequenceAlt02 => {
            let table_back = if rule == ExpressionRule::NextValueForSequenceAlt01 {
                0
            } else {
                1
            };
            let Some(table) = rhs[rhs_len - (table_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("NEXTVAL"),
                vec![parser_ast::ExprNode {
                    node_text: Default::default(),
                    Kind: parser_ast::ExprKind::TableName(table),
                    OriginTextPosition: 0,
                    Flag: Default::default(),
                }],
            ));
        }
        ExpressionRule::SignedLiteralAlt02 | ExpressionRule::SignedLiteralAlt03 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(semantic_value_text)
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::Unary(
                if rule == ExpressionRule::SignedLiteralAlt02 {
                    "+"
                } else {
                    "-"
                }
                .to_owned(),
                Box::new(parser_ast::ExprNode::Value(value)),
            ));
        }
        ExpressionRule::LengthNumAlt01 => {
            let value = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            out.item = Some(Box::new(value));
        }
        ExpressionRule::CastTypeAlt08 => {
            let mut field_type =
                parser_types::types::NewFieldType(parser_mysql::r#type::TypeLonglong);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::SystemVariableAlt01 | ExpressionRule::UserVariableAlt01 => {
            let original = rhs[rhs_len - (0)].ident.clone();
            let (name, global, instance, system, explicit) =
                if rule == ExpressionRule::UserVariableAlt01 {
                    (
                        original.trim_start_matches('@').to_owned(),
                        false,
                        false,
                        false,
                        false,
                    )
                } else {
                    let value = original.to_ascii_lowercase();
                    if let Some(name) = value.strip_prefix("@@global.") {
                        (name.to_owned(), true, false, true, true)
                    } else if let Some(name) = value.strip_prefix("@@instance.") {
                        (name.to_owned(), false, true, true, true)
                    } else if let Some(name) = value
                        .strip_prefix("@@session.")
                        .or_else(|| value.strip_prefix("@@local."))
                    {
                        (name.to_owned(), false, false, true, true)
                    } else {
                        (
                            value.trim_start_matches("@@").to_owned(),
                            false,
                            false,
                            true,
                            false,
                        )
                    }
                };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Variable {
                    Name: name,
                    IsGlobal: global,
                    IsInstance: instance,
                    IsSystem: system,
                    ExplicitScope: explicit,
                    Value: None,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::CharsetNameAlt01 => {
            let name = &rhs[rhs_len - (0)].ident;
            match charset::charset::GetCharsetInfo(name) {
                Ok(info) => out.ident = info.Name,
                Err(_) => {
                    yylex.AppendError(
                        ErrUnknownCharacterSet.GenWithStackByArgs(&[name.clone().into()]),
                    );
                    return Err(1);
                }
            }
        }
        ExpressionRule::CharsetNameAlt02 => out.ident = "binary".to_owned(),
        ExpressionRule::CollationNameAlt01 => {
            let name = &rhs[rhs_len - (0)].ident;
            match charset::charset::GetCollationByName(name) {
                Ok(info) => out.ident = info.Name,
                Err(_) => {
                    yylex.AppendError(
                        ErrUnknownCollation.GenWithStackByArgs(&[name.clone().into()]),
                    );
                    return Err(1);
                }
            }
        }
        ExpressionRule::CollationNameAlt02 => out.ident = "binary".to_owned(),
        ExpressionRule::Int64NumAlt01 => {
            let Some(item) = rhs[rhs_len - (0)].item.as_deref() else {
                yylex.AppendError(yylex.Errorf("missing numeric semantic value", &[]));
                return Err(1);
            };
            let (value, range_error) = getInt64FromNUM(item);
            if !range_error.is_empty() {
                yylex.AppendError(yylex.Errorf(&range_error, &[]));
                return Err(1);
            }
            out.item = Some(Box::new(value));
        }
        ExpressionRule::LiteralAlt01 => out.expr = Some(parser_ast::ExprNode::BoolValue(false)),
        ExpressionRule::LiteralAlt02 => out.expr = Some(parser_ast::ExprNode::NullValue()),
        ExpressionRule::LiteralAlt03 => out.expr = Some(parser_ast::ExprNode::BoolValue(true)),
        ExpressionRule::LiteralAlt04
        | ExpressionRule::LiteralAlt05
        | ExpressionRule::LiteralAlt06
        | ExpressionRule::LiteralAlt09
        | ExpressionRule::LiteralAlt10 => {
            out.expr = rhs[rhs_len - (0)].item.as_deref().map(|value| {
                semantic_value_expr(value, &parser_state.charset, &parser_state.collation)
            });
        }
        ExpressionRule::StringLiteralAlt01 => {
            out.expr = Some(parser_ast::ExprNode::Value(
                rhs[rhs_len - (0)].ident.clone(),
            ))
        }
        ExpressionRule::StringLiteralAlt02 => {
            let prefix = rhs[rhs_len - (1)]
                .expr
                .as_ref()
                .and_then(|expr| match &expr.Kind {
                    parser_ast::ExprKind::Value(value) => Some(value.text()),
                    _ => None,
                })
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::StringValue(
                format!("{}{}", prefix, rhs[rhs_len - (0)].ident),
                &parser_state.charset,
                &parser_state.collation,
            ));
        }
        ExpressionRule::LiteralAlt08
        | ExpressionRule::LiteralAlt11
        | ExpressionRule::LiteralAlt12 => {
            let charset_name = rhs[rhs_len - (1)].ident.clone();
            let Ok(collation) = charset::charset::GetDefaultCollationLegacy(&charset_name) else {
                yylex.AppendError(AST_ERR_UNKNOWN_CHARACTER_SET.GenWithStack(
                    "Unsupported character introducer: '%-.64s'",
                    &[charset_name.into()],
                ));
                return Err(1);
            };
            let value = if rule == ExpressionRule::LiteralAlt08 {
                rhs[rhs_len - (0)].ident.clone()
            } else {
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .map(semantic_value_text)
                    .unwrap_or_default()
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::IntroducedValue {
                    Value: value,
                    Charset: charset_name,
                    Binary: collation == "binary",
                    Collation: collation,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::BitExprAlt01
        | ExpressionRule::BitExprAlt02
        | ExpressionRule::BitExprAlt03
        | ExpressionRule::BitExprAlt04
        | ExpressionRule::BitExprAlt05
        | ExpressionRule::BitExprAlt06
        | ExpressionRule::BitExprAlt10
        | ExpressionRule::BitExprAlt11
        | ExpressionRule::BitExprAlt12
        | ExpressionRule::BitExprAlt13
        | ExpressionRule::BitExprAlt14
        | ExpressionRule::BitExprAlt15 => {
            let Some(left) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let Some(right) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let operator = match rule {
                ExpressionRule::BitExprAlt01 => "|",
                ExpressionRule::BitExprAlt02 => "&",
                ExpressionRule::BitExprAlt03 => "<<",
                ExpressionRule::BitExprAlt04 => ">>",
                ExpressionRule::BitExprAlt05 => "+",
                ExpressionRule::BitExprAlt06 => "-",
                ExpressionRule::BitExprAlt10 => "*",
                ExpressionRule::BitExprAlt11 => "/",
                ExpressionRule::BitExprAlt12 | ExpressionRule::BitExprAlt14 => "%",
                ExpressionRule::BitExprAlt13 => "DIV",
                _ => "^",
            };
            out.expr = Some(parser_ast::ExprNode::Binary(
                operator.to_owned(),
                Box::new(left),
                Box::new(right),
            ));
        }
        ExpressionRule::SimpleIdentAlt01 => {
            out.expr = Some(parser_ast::ExprNode::Column(parser_ast::ColumnName {
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }));
        }
        ExpressionRule::SimpleIdentAlt02 => {
            out.expr = Some(parser_ast::ExprNode::Column(parser_ast::ColumnName {
                Table: parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
                ..Default::default()
            }));
        }
        ExpressionRule::SimpleIdentAlt03 => {
            out.expr = Some(parser_ast::ExprNode::Column(parser_ast::ColumnName {
                Schema: parser_ast::NewCIStr(&rhs[rhs_len - (4)].ident),
                Table: parser_ast::NewCIStr(&rhs[rhs_len - (2)].ident),
                Name: parser_ast::NewCIStr(&rhs[rhs_len - (0)].ident),
            }));
        }
        ExpressionRule::SimpleExprAlt27 => {
            let Some(expr) = rhs[rhs_len - (3)].expr.clone() else {
                return Ok(false);
            };
            let Some(mut field_type) = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_types::types::FieldType>())
                .cloned()
            else {
                return Ok(false);
            };
            let (flen, decimal) =
                parser_mysql::util::GetDefaultFieldLengthAndDecimalForCast(field_type.GetType());
            if field_type.GetFlen() == parser_types::types::UnspecifiedLength {
                field_type.SetFlen(flen);
            }
            if field_type.GetDecimal() == parser_types::types::UnspecifiedLength {
                field_type.SetDecimal(decimal);
            }
            let explicit_charset = parser_state.explicitCharset;
            parser_state.explicitCharset = false;
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Cast {
                    Expr: Box::new(expr),
                    Tp: field_type,
                    FunctionType: parser_ast::CastFunctionType::Convert,
                    ExplicitCharSet: explicit_charset,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::SimpleExprAlt26 => {
            let clauses = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::WhenClause>>())
                .cloned()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Case {
                    Value: rhs[rhs_len - (3)].expr.clone().map(Box::new),
                    WhenClauses: clauses,
                    ElseClause: rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                        .cloned()
                        .map(Box::new),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::BitExprAlt07
        | ExpressionRule::BitExprAlt08
        | ExpressionRule::BitExprAlt09 => {
            let (name, value_back, interval_back, unit_back) = match rule {
                ExpressionRule::BitExprAlt07 => ("DATE_ADD", 4, 1, 0),
                ExpressionRule::BitExprAlt08 => ("DATE_SUB", 4, 1, 0),
                _ => ("DATE_ADD", 0, 3, 2),
            };
            let (Some(value), Some(interval)) = (
                rhs[rhs_len - (value_back)].expr.clone(),
                rhs[rhs_len - (interval_back)].expr.clone(),
            ) else {
                return Ok(false);
            };
            let unit = rhs[rhs_len - (unit_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                .copied()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(name),
                vec![
                    value,
                    interval,
                    parser_ast::ExprNode {
                        node_text: Default::default(),
                        Kind: parser_ast::ExprKind::TimeUnit(unit),
                        OriginTextPosition: 0,
                        Flag: Default::default(),
                    },
                ],
            ));
        }
        ExpressionRule::SimpleExprAlt05 => {
            let Some(expr) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Collate {
                    Expr: Box::new(expr),
                    Collation: rhs[rhs_len - (0)].ident.clone(),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::SimpleExprAlt19 | ExpressionRule::SimpleExprAlt20 => {
            let mut values = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (1)].expr.clone() {
                values.push(value);
            }
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Row(values),
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::SimpleExprAlt22 => {
            let Some(expr) = rhs[rhs_len - (1)].expr.clone() else {
                return Ok(false);
            };
            let name = match rhs[rhs_len - (2)].ident.as_str() {
                "d" => Some("DATE"),
                "t" => Some("TIME"),
                "ts" => Some("TIMESTAMP"),
                _ => None,
            };
            out.expr = Some(match name {
                Some(name) => parser_ast::ExprNode::Function(
                    parser_ast::CIStr::default(),
                    parser_ast::NewCIStr(name),
                    vec![expr],
                ),
                None => expr,
            });
        }
        ExpressionRule::SimpleExprAlt23 => {
            let Some(expr) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let mut field_type =
                parser_types::types::NewFieldType(parser_mysql::r#type::TypeString);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Cast {
                    Expr: Box::new(expr),
                    Tp: field_type,
                    FunctionType: parser_ast::CastFunctionType::Binary,
                    ExplicitCharSet: false,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::SimpleExprAlt24 | ExpressionRule::SimpleExprAlt25 => {
            let Some(expr) = rhs[rhs_len - (4)].expr.clone() else {
                return Ok(false);
            };
            let Some(mut field_type) = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_types::types::FieldType>())
                .cloned()
            else {
                return Ok(false);
            };
            let (flen, decimal) =
                parser_mysql::util::GetDefaultFieldLengthAndDecimalForCast(field_type.GetType());
            if field_type.GetFlen() == parser_types::types::UnspecifiedLength {
                field_type.SetFlen(flen);
            }
            if field_type.GetDecimal() == parser_types::types::UnspecifiedLength {
                field_type.SetDecimal(decimal);
            }
            let is_array = rule == ExpressionRule::SimpleExprAlt25
                || rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false);
            field_type.SetArray(is_array);
            let explicit_charset = parser_state.explicitCharset;
            if is_array && !explicit_charset && field_type.GetCharset() != "binary" {
                field_type.SetCharset("utf8mb4".to_owned());
                field_type.SetCollate("utf8mb4_bin".to_owned());
            }
            parser_state.explicitCharset = false;
            let kind = if rule == ExpressionRule::SimpleExprAlt25 {
                parser_ast::ExprKind::JSONSumCrc32 {
                    Expr: Box::new(expr),
                    Tp: field_type,
                    ExplicitCharSet: explicit_charset,
                }
            } else {
                parser_ast::ExprKind::Cast {
                    Expr: Box::new(expr),
                    Tp: field_type,
                    FunctionType: parser_ast::CastFunctionType::Cast,
                    ExplicitCharSet: explicit_charset,
                }
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: kind,
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::SimpleExprAlt28 => {
            let Some(expr) = rhs[rhs_len - (3)].expr.clone() else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&rhs[rhs_len - (5)].ident),
                vec![
                    expr,
                    parser_ast::ExprNode::Value(rhs[rhs_len - (1)].ident.clone()),
                ],
            ));
        }
        ExpressionRule::SimpleExprAlt29 => {
            let Some(expr) = rhs[rhs_len - (1)].expr.as_ref() else {
                return Ok(false);
            };
            let parser_ast::ExprKind::Column(column) = &expr.Kind else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::NamedDefault(column.clone()),
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::SimpleExprAlt31 | ExpressionRule::SimpleExprAlt32 => {
            let Some(expr) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let extract = parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("JSON_EXTRACT"),
                vec![
                    expr,
                    parser_ast::ExprNode::Value(rhs[rhs_len - (0)].ident.clone()),
                ],
            );
            out.expr = Some(if rule == ExpressionRule::SimpleExprAlt32 {
                parser_ast::ExprNode::Function(
                    parser_ast::CIStr::default(),
                    parser_ast::NewCIStr("JSON_UNQUOTE"),
                    vec![extract],
                )
            } else {
                extract
            });
        }
        ExpressionRule::ArrayKwdOptAlt01 | ExpressionRule::ArrayKwdOptAlt02 => {
            out.item = Some(Box::new(rule == ExpressionRule::ArrayKwdOptAlt02))
        }
        ExpressionRule::CastTypeAlt14 | ExpressionRule::StringTypeAlt17 => {
            let element_back = 1;
            let element = rhs[rhs_len - (element_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<VectorElementTypeSemantic>())
                .copied()
                .unwrap_or_default();
            if element.tp != parser_mysql::r#type::TypeFloat {
                yylex.AppendError(yylex.Errorf("Only VECTOR is supported for now", &[]));
            }
            let mut field_type =
                parser_types::types::NewFieldType(parser_mysql::r#type::TypeTiDBVectorFloat32);
            field_type.SetFlen(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(semantic_numeric_isize)
                    .unwrap_or(-1),
            );
            field_type.SetDecimal(0);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::FunctionCallKeywordAlt01
        | ExpressionRule::FunctionCallKeywordAlt02
        | ExpressionRule::FunctionCallKeywordAlt11
        | ExpressionRule::FunctionCallKeywordAlt13
        | ExpressionRule::FunctionCallNonKeywordAlt01
        | ExpressionRule::FunctionCallNonKeywordAlt02
        | ExpressionRule::FunctionCallNonKeywordAlt24 => {
            let (name, args_back) = match rule {
                ExpressionRule::FunctionCallKeywordAlt11 => ("INSERT".to_owned(), 1),
                ExpressionRule::FunctionCallKeywordAlt13 => ("PASSWORD".to_owned(), 1),
                _ => (rhs[rhs_len - (3)].ident.clone(), 1),
            };
            let args = rhs[rhs_len - (args_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&name),
                args,
            ));
        }
        ExpressionRule::FunctionCallKeywordAlt03 | ExpressionRule::FunctionCallKeywordAlt04 => {
            let name_back = if rule == ExpressionRule::FunctionCallKeywordAlt03 {
                1
            } else {
                2
            };
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&rhs[rhs_len - (name_back)].ident),
                Vec::new(),
            ));
        }
        ExpressionRule::FunctionCallKeywordAlt05 => {
            let args = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                .cloned()
                .into_iter()
                .collect();
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&rhs[rhs_len - (1)].ident),
                args,
            ));
        }
        ExpressionRule::FunctionCallKeywordAlt06 | ExpressionRule::FunctionCallKeywordAlt07 => {
            let args_back = if rule == ExpressionRule::FunctionCallKeywordAlt06 {
                1
            } else {
                3
            };
            let mut args = rhs[rhs_len - (args_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            args.push(parser_ast::ExprNode::Value(
                if rule == ExpressionRule::FunctionCallKeywordAlt06 {
                    "NULL".to_owned()
                } else {
                    rhs[rhs_len - (1)].ident.clone()
                },
            ));
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("CHAR"),
                args,
            ));
        }
        ExpressionRule::FunctionCallKeywordAlt08
        | ExpressionRule::FunctionCallKeywordAlt09
        | ExpressionRule::FunctionCallKeywordAlt10 => {
            let name = match rule {
                ExpressionRule::FunctionCallKeywordAlt08 => "DATE",
                ExpressionRule::FunctionCallKeywordAlt09 => "TIME",
                _ => "TIMESTAMP",
            };
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(name),
                vec![parser_ast::ExprNode::Value(
                    rhs[rhs_len - (0)].ident.clone(),
                )],
            ));
        }
        ExpressionRule::FunctionCallKeywordAlt12 => {
            let (Some(left), Some(right)) = (
                rhs[rhs_len - (3)].expr.clone(),
                rhs[rhs_len - (1)].expr.clone(),
            ) else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode::Binary(
                "%".to_owned(),
                Box::new(left),
                Box::new(right),
            ));
        }
        ExpressionRule::FunctionCallNonKeywordAlt03
        | ExpressionRule::FunctionCallNonKeywordAlt04
        | ExpressionRule::FunctionCallNonKeywordAlt05
        | ExpressionRule::FunctionCallNonKeywordAlt06
        | ExpressionRule::FunctionCallNonKeywordAlt07
        | ExpressionRule::FunctionCallNonKeywordAlt08
        | ExpressionRule::FunctionCallNonKeywordAlt09
        | ExpressionRule::FunctionCallNonKeywordAlt10
        | ExpressionRule::FunctionCallNonKeywordAlt11
        | ExpressionRule::FunctionCallNonKeywordAlt12
        | ExpressionRule::FunctionCallNonKeywordAlt13
        | ExpressionRule::FunctionCallNonKeywordAlt14
        | ExpressionRule::FunctionCallNonKeywordAlt15
        | ExpressionRule::FunctionCallNonKeywordAlt16
        | ExpressionRule::FunctionCallNonKeywordAlt17
        | ExpressionRule::FunctionCallNonKeywordAlt18
        | ExpressionRule::FunctionCallNonKeywordAlt19
        | ExpressionRule::FunctionCallNonKeywordAlt20
        | ExpressionRule::FunctionCallNonKeywordAlt21
        | ExpressionRule::FunctionCallNonKeywordAlt22
        | ExpressionRule::FunctionCallNonKeywordAlt23 => {
            let name_back = match rule {
                ExpressionRule::FunctionCallNonKeywordAlt03
                | ExpressionRule::FunctionCallNonKeywordAlt06
                | ExpressionRule::FunctionCallNonKeywordAlt07
                | ExpressionRule::FunctionCallNonKeywordAlt08
                | ExpressionRule::FunctionCallNonKeywordAlt09
                | ExpressionRule::FunctionCallNonKeywordAlt10
                | ExpressionRule::FunctionCallNonKeywordAlt16
                | ExpressionRule::FunctionCallNonKeywordAlt17 => 5,
                ExpressionRule::FunctionCallNonKeywordAlt04
                | ExpressionRule::FunctionCallNonKeywordAlt05
                | ExpressionRule::FunctionCallNonKeywordAlt11
                | ExpressionRule::FunctionCallNonKeywordAlt12
                | ExpressionRule::FunctionCallNonKeywordAlt13
                | ExpressionRule::FunctionCallNonKeywordAlt14
                | ExpressionRule::FunctionCallNonKeywordAlt23 => 7,
                ExpressionRule::FunctionCallNonKeywordAlt18
                | ExpressionRule::FunctionCallNonKeywordAlt20
                | ExpressionRule::FunctionCallNonKeywordAlt21 => 6,
                ExpressionRule::FunctionCallNonKeywordAlt15
                | ExpressionRule::FunctionCallNonKeywordAlt19 => 3,
                _ => return Ok(false),
            };
            let special = |kind| parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: kind,
                OriginTextPosition: 0,
                Flag: Default::default(),
            };
            let expr = |back: usize| rhs[rhs_len - (back)].expr.clone();
            let unit = |back: usize| {
                rhs[rhs_len - (back)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<parser_ast::TimeUnitType>())
                    .copied()
                    .unwrap_or_default()
            };
            let args = match rule {
                ExpressionRule::FunctionCallNonKeywordAlt03 => vec![
                    expr(3),
                    expr(1),
                    Some(special(parser_ast::ExprKind::TimeUnit(
                        parser_ast::TimeUnitType::Day,
                    ))),
                ],
                ExpressionRule::FunctionCallNonKeywordAlt04
                | ExpressionRule::FunctionCallNonKeywordAlt05 => vec![
                    expr(5),
                    expr(2),
                    Some(special(parser_ast::ExprKind::TimeUnit(unit(1)))),
                ],
                ExpressionRule::FunctionCallNonKeywordAlt06 => vec![
                    Some(special(parser_ast::ExprKind::TimeUnit(unit(3)))),
                    expr(1),
                ],
                ExpressionRule::FunctionCallNonKeywordAlt07 => vec![
                    Some(special(parser_ast::ExprKind::GetFormatSelector(
                        rhs[rhs_len - (3)]
                            .item
                            .as_deref()
                            .and_then(|item| {
                                item.downcast_ref::<parser_ast::GetFormatSelectorType>()
                            })
                            .copied()
                            .unwrap_or_default(),
                    ))),
                    expr(1),
                ],
                ExpressionRule::FunctionCallNonKeywordAlt08
                | ExpressionRule::FunctionCallNonKeywordAlt09
                | ExpressionRule::FunctionCallNonKeywordAlt10 => vec![expr(3), expr(1)],
                ExpressionRule::FunctionCallNonKeywordAlt11
                | ExpressionRule::FunctionCallNonKeywordAlt12
                | ExpressionRule::FunctionCallNonKeywordAlt23 => vec![expr(5), expr(3), expr(1)],
                ExpressionRule::FunctionCallNonKeywordAlt13
                | ExpressionRule::FunctionCallNonKeywordAlt14 => vec![
                    Some(special(parser_ast::ExprKind::TimeUnit(unit(5)))),
                    expr(3),
                    expr(1),
                ],
                ExpressionRule::FunctionCallNonKeywordAlt15
                | ExpressionRule::FunctionCallNonKeywordAlt19 => vec![expr(1)],
                ExpressionRule::FunctionCallNonKeywordAlt16 => vec![expr(1), expr(3)],
                ExpressionRule::FunctionCallNonKeywordAlt17 => vec![
                    expr(1),
                    Some(parser_ast::ExprNode::Value(" ".to_owned())),
                    Some(special(parser_ast::ExprKind::TrimDirection(
                        rhs[rhs_len - (3)]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<parser_ast::TrimDirectionType>())
                            .copied()
                            .unwrap_or_default(),
                    ))),
                ],
                ExpressionRule::FunctionCallNonKeywordAlt18 => vec![
                    expr(1),
                    expr(3),
                    Some(special(parser_ast::ExprKind::TrimDirection(
                        rhs[rhs_len - (4)]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<parser_ast::TrimDirectionType>())
                            .copied()
                            .unwrap_or_default(),
                    ))),
                ],
                ExpressionRule::FunctionCallNonKeywordAlt20
                | ExpressionRule::FunctionCallNonKeywordAlt21 => vec![
                    expr(4),
                    Some(parser_ast::ExprNode::Value(
                        if rule == ExpressionRule::FunctionCallNonKeywordAlt20 {
                            "CHAR"
                        } else {
                            "BINARY"
                        }
                        .to_owned(),
                    )),
                    Some(parser_ast::ExprNode::Value(
                        rhs[rhs_len - (1)]
                            .item
                            .as_deref()
                            .map(semantic_value_text)
                            .unwrap_or_default(),
                    )),
                ],
                _ => Vec::new(),
            }
            .into_iter()
            .flatten()
            .collect();
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&rhs[rhs_len - (name_back)].ident),
                args,
            ));
        }
        ExpressionRule::GetFormatSelectorAlt01
        | ExpressionRule::GetFormatSelectorAlt02
        | ExpressionRule::GetFormatSelectorAlt03
        | ExpressionRule::GetFormatSelectorAlt04 => {
            out.item = Some(Box::new(match rule {
                ExpressionRule::GetFormatSelectorAlt01 => parser_ast::GetFormatSelectorType::Date,
                ExpressionRule::GetFormatSelectorAlt03 => parser_ast::GetFormatSelectorType::Time,
                _ => parser_ast::GetFormatSelectorType::Datetime,
            }))
        }
        ExpressionRule::TrimDirectionAlt01
        | ExpressionRule::TrimDirectionAlt02
        | ExpressionRule::TrimDirectionAlt03 => {
            out.item = Some(Box::new(match rule {
                ExpressionRule::TrimDirectionAlt01 => parser_ast::TrimDirectionType::Both,
                ExpressionRule::TrimDirectionAlt02 => parser_ast::TrimDirectionType::Leading,
                _ => parser_ast::TrimDirectionType::Trailing,
            }))
        }
        ExpressionRule::FunctionNameSequenceAlt01 | ExpressionRule::FunctionNameSequenceAlt02 => {
            let table_back = if rule == ExpressionRule::FunctionNameSequenceAlt01 {
                1
            } else {
                3
            };
            let Some(table) = rhs[rhs_len - (table_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::TableName>())
                .cloned()
            else {
                return Ok(false);
            };
            let mut args = vec![parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::TableName(table),
                OriginTextPosition: 0,
                Flag: Default::default(),
            }];
            if rule == ExpressionRule::FunctionNameSequenceAlt02 {
                args.push(parser_ast::ExprNode::Value(
                    rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .map(semantic_value_text)
                        .unwrap_or_default(),
                ));
            }
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(if rule == ExpressionRule::FunctionNameSequenceAlt01 {
                    "LASTVAL"
                } else {
                    "SETVAL"
                }),
                args,
            ));
        }
        ExpressionRule::SumExprAlt01
        | ExpressionRule::SumExprAlt02
        | ExpressionRule::SumExprAlt03
        | ExpressionRule::SumExprAlt04
        | ExpressionRule::SumExprAlt05
        | ExpressionRule::SumExprAlt06
        | ExpressionRule::SumExprAlt07
        | ExpressionRule::SumExprAlt08
        | ExpressionRule::SumExprAlt09
        | ExpressionRule::SumExprAlt10
        | ExpressionRule::SumExprAlt11
        | ExpressionRule::SumExprAlt12
        | ExpressionRule::SumExprAlt13
        | ExpressionRule::SumExprAlt14
        | ExpressionRule::SumExprAlt15
        | ExpressionRule::SumExprAlt16
        | ExpressionRule::SumExprAlt17
        | ExpressionRule::SumExprAlt18
        | ExpressionRule::SumExprAlt19
        | ExpressionRule::SumExprAlt20
        | ExpressionRule::SumExprAlt21
        | ExpressionRule::SumExprAlt22 => {
            let (name_back, args, distinct) = match rule {
                ExpressionRule::SumExprAlt02 | ExpressionRule::SumExprAlt03 => (
                    3,
                    rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                        .cloned()
                        .unwrap_or_default(),
                    false,
                ),
                ExpressionRule::SumExprAlt10 => (
                    4,
                    rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                        .cloned()
                        .unwrap_or_default(),
                    true,
                ),
                ExpressionRule::SumExprAlt13 => {
                    (4, vec![parser_ast::ExprNode::Value("1".to_owned())], false)
                }
                ExpressionRule::SumExprAlt14 => {
                    let mut args = rhs[rhs_len - (4)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                        .cloned()
                        .unwrap_or_default();
                    if let Some(arg) = rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<parser_ast::ExprNode>())
                    {
                        args.push(arg.clone());
                    }
                    (
                        7,
                        args,
                        rhs[rhs_len - (5)]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<bool>())
                            .copied()
                            .unwrap_or(false),
                    )
                }
                ExpressionRule::SumExprAlt19 | ExpressionRule::SumExprAlt21 => (
                    5,
                    rhs[rhs_len - (2)].expr.clone().into_iter().collect(),
                    rhs[rhs_len - (3)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                ),
                _ => {
                    let name_back = if matches!(
                        rule,
                        ExpressionRule::SumExprAlt04
                            | ExpressionRule::SumExprAlt06
                            | ExpressionRule::SumExprAlt08
                            | ExpressionRule::SumExprAlt12
                    ) {
                        4
                    } else {
                        5
                    };
                    (
                        name_back,
                        rhs[rhs_len - (2)].expr.clone().into_iter().collect(),
                        rhs[rhs_len - (3)]
                            .item
                            .as_deref()
                            .and_then(|item| item.downcast_ref::<bool>())
                            .copied()
                            .unwrap_or(false),
                    )
                }
            };
            let name = match rule {
                ExpressionRule::SumExprAlt19 => "STDDEV_POP".to_owned(),
                ExpressionRule::SumExprAlt21 => "VAR_POP".to_owned(),
                _ => rhs[rhs_len - (name_back)].ident.clone(),
            };
            if let Some(spec) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WindowSpec>())
                .cloned()
            {
                out.expr = Some(parser_ast::ExprNode {
                    node_text: Default::default(),
                    Kind: parser_ast::ExprKind::WindowFunction {
                        Name: name,
                        Args: args,
                        Distinct: distinct,
                        IgnoreNull: false,
                        FromLast: false,
                        Spec: Box::new(spec),
                    },
                    OriginTextPosition: 0,
                    Flag: Default::default(),
                });
            } else {
                let order = if rule == ExpressionRule::SumExprAlt14 {
                    rhs[rhs_len - (3)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<Vec<parser_ast::ByItem>>())
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                out.expr = Some(parser_ast::ExprNode {
                    node_text: Default::default(),
                    Kind: parser_ast::ExprKind::AggregateFunction {
                        Name: name,
                        Args: args,
                        Distinct: distinct,
                        Order: order,
                    },
                    OriginTextPosition: 0,
                    Flag: Default::default(),
                });
            }
        }
        ExpressionRule::FunctionCallGenericAlt01 => {
            let args = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr(&rhs[rhs_len - (3)].ident),
                args,
            ));
        }
        ExpressionRule::OptGConcatSeparatorAlt01 | ExpressionRule::OptGConcatSeparatorAlt02 => {
            let value = if rule == ExpressionRule::OptGConcatSeparatorAlt01 {
                ",".to_owned()
            } else {
                rhs[rhs_len - (0)].ident.clone()
            };
            out.item = Some(Box::new(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::IntroducedValue {
                    Value: value,
                    Charset: parser_state.charset.clone(),
                    Collation: parser_state.collation.clone(),
                    Binary: parser_state.collation == "binary",
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            }));
        }
        ExpressionRule::FunctionCallGenericAlt02 => {
            let args = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::NewCIStr(&rhs[rhs_len - (5)].ident),
                parser_ast::NewCIStr(&rhs[rhs_len - (3)].ident),
                args,
            ));
        }
        ExpressionRule::FuncDatetimePrecAlt01 | ExpressionRule::FuncDatetimePrecAlt02 => {
            out.item = None
        }
        ExpressionRule::FuncDatetimePrecAlt03 => {
            out.item = Some(Box::new(parser_ast::ExprNode::Value(
                rhs[rhs_len - (1)]
                    .item
                    .as_deref()
                    .map(semantic_value_text)
                    .unwrap_or_default(),
            )))
        }
        ExpressionRule::SumExprAlt23
        | ExpressionRule::SumExprAlt24
        | ExpressionRule::SumExprAlt25
        | ExpressionRule::SumExprAlt26
        | ExpressionRule::SumExprAlt27
        | ExpressionRule::SumExprAlt28 => {
            let (name_back, arg_backs): (usize, &[usize]) = match rule {
                ExpressionRule::SumExprAlt23 => (4, &[2]),
                ExpressionRule::SumExprAlt24 => (5, &[2]),
                ExpressionRule::SumExprAlt25 => (6, &[4, 2]),
                ExpressionRule::SumExprAlt26 => (7, &[4, 2]),
                ExpressionRule::SumExprAlt27 => (7, &[5, 2]),
                _ => (8, &[5, 2]),
            };
            let args = arg_backs
                .iter()
                .filter_map(|back| rhs[rhs_len - (*back)].expr.clone())
                .collect::<Vec<_>>();
            let name = rhs[rhs_len - (name_back)].ident.clone();
            if let Some(spec) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WindowSpec>())
                .cloned()
            {
                out.expr = Some(parser_ast::ExprNode {
                    node_text: Default::default(),
                    Kind: parser_ast::ExprKind::WindowFunction {
                        Name: name,
                        Args: args,
                        Distinct: false,
                        IgnoreNull: false,
                        FromLast: false,
                        Spec: Box::new(spec),
                    },
                    OriginTextPosition: 0,
                    Flag: Default::default(),
                });
            } else {
                out.expr = Some(parser_ast::ExprNode::Function(
                    parser_ast::CIStr::default(),
                    parser_ast::NewCIStr(&name),
                    args,
                ));
            }
        }
        ExpressionRule::TimeUnitAlt02
        | ExpressionRule::TimeUnitAlt03
        | ExpressionRule::TimeUnitAlt04
        | ExpressionRule::TimeUnitAlt05
        | ExpressionRule::TimeUnitAlt06
        | ExpressionRule::TimeUnitAlt07
        | ExpressionRule::TimeUnitAlt08
        | ExpressionRule::TimeUnitAlt09
        | ExpressionRule::TimeUnitAlt10
        | ExpressionRule::TimeUnitAlt11
        | ExpressionRule::TimeUnitAlt12
        | ExpressionRule::TimestampUnitAlt01
        | ExpressionRule::TimestampUnitAlt02
        | ExpressionRule::TimestampUnitAlt03
        | ExpressionRule::TimestampUnitAlt04
        | ExpressionRule::TimestampUnitAlt05
        | ExpressionRule::TimestampUnitAlt06
        | ExpressionRule::TimestampUnitAlt07
        | ExpressionRule::TimestampUnitAlt08
        | ExpressionRule::TimestampUnitAlt09
        | ExpressionRule::TimestampUnitAlt10
        | ExpressionRule::TimestampUnitAlt11
        | ExpressionRule::TimestampUnitAlt12
        | ExpressionRule::TimestampUnitAlt13
        | ExpressionRule::TimestampUnitAlt14
        | ExpressionRule::TimestampUnitAlt15
        | ExpressionRule::TimestampUnitAlt16
        | ExpressionRule::TimestampUnitAlt17 => {
            let unit = match rule {
                ExpressionRule::TimeUnitAlt02 => parser_ast::TimeUnitType::SecondMicrosecond,
                ExpressionRule::TimeUnitAlt03 => parser_ast::TimeUnitType::MinuteMicrosecond,
                ExpressionRule::TimeUnitAlt04 => parser_ast::TimeUnitType::MinuteSecond,
                ExpressionRule::TimeUnitAlt05 => parser_ast::TimeUnitType::HourMicrosecond,
                ExpressionRule::TimeUnitAlt06 => parser_ast::TimeUnitType::HourSecond,
                ExpressionRule::TimeUnitAlt07 => parser_ast::TimeUnitType::HourMinute,
                ExpressionRule::TimeUnitAlt08 => parser_ast::TimeUnitType::DayMicrosecond,
                ExpressionRule::TimeUnitAlt09 => parser_ast::TimeUnitType::DaySecond,
                ExpressionRule::TimeUnitAlt10 => parser_ast::TimeUnitType::DayMinute,
                ExpressionRule::TimeUnitAlt11 => parser_ast::TimeUnitType::DayHour,
                ExpressionRule::TimeUnitAlt12 => parser_ast::TimeUnitType::YearMonth,
                ExpressionRule::TimestampUnitAlt01 => parser_ast::TimeUnitType::Microsecond,
                ExpressionRule::TimestampUnitAlt02 | ExpressionRule::TimestampUnitAlt10 => {
                    parser_ast::TimeUnitType::Second
                }
                ExpressionRule::TimestampUnitAlt03 | ExpressionRule::TimestampUnitAlt11 => {
                    parser_ast::TimeUnitType::Minute
                }
                ExpressionRule::TimestampUnitAlt04 | ExpressionRule::TimestampUnitAlt12 => {
                    parser_ast::TimeUnitType::Hour
                }
                ExpressionRule::TimestampUnitAlt05 | ExpressionRule::TimestampUnitAlt13 => {
                    parser_ast::TimeUnitType::Day
                }
                ExpressionRule::TimestampUnitAlt06 | ExpressionRule::TimestampUnitAlt14 => {
                    parser_ast::TimeUnitType::Week
                }
                ExpressionRule::TimestampUnitAlt07 | ExpressionRule::TimestampUnitAlt15 => {
                    parser_ast::TimeUnitType::Month
                }
                ExpressionRule::TimestampUnitAlt08 | ExpressionRule::TimestampUnitAlt16 => {
                    parser_ast::TimeUnitType::Quarter
                }
                _ => parser_ast::TimeUnitType::Year,
            };
            out.item = Some(Box::new(unit));
        }
        ExpressionRule::ExpressionOptAlt01 => out.expr = None,
        ExpressionRule::WhenClauseListAlt01 => {
            let values = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WhenClause>())
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            out.item = Some(Box::new(values));
        }
        ExpressionRule::WhenClauseListAlt02 => {
            let mut values = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::WhenClause>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<parser_ast::WhenClause>())
            {
                values.push(value.clone());
            }
            out.item = Some(Box::new(values));
        }
        ExpressionRule::WhenClauseAlt01 => {
            let (Some(expr), Some(result)) = (
                rhs[rhs_len - (2)].expr.clone(),
                rhs[rhs_len - (0)].expr.clone(),
            ) else {
                return Ok(false);
            };
            out.item = Some(Box::new(parser_ast::WhenClause {
                Expr: expr,
                Result: result,
            }));
        }
        ExpressionRule::ElseOptAlt01 => out.item = None,
        ExpressionRule::ElseOptAlt02 => {
            out.item = rhs[rhs_len - (0)]
                .expr
                .clone()
                .map(|expr| Box::new(expr) as Box<dyn Any>)
        }
        ExpressionRule::CastTypeAlt01 | ExpressionRule::CastTypeAlt02 => {
            let flen_back = if rule == ExpressionRule::CastTypeAlt01 {
                0
            } else {
                1
            };
            let flen = rhs[rhs_len - (flen_back)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<isize>())
                .copied()
                .unwrap_or(-1);
            let mut field_type =
                parser_types::types::NewFieldType(parser_mysql::r#type::TypeVarString);
            field_type.SetFlen(flen);
            if flen != -1 {
                field_type.SetType(parser_mysql::r#type::TypeString);
            }
            if rule == ExpressionRule::CastTypeAlt01 {
                field_type.SetCharset("binary".to_owned());
                field_type.SetCollate("binary".to_owned());
                field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            } else {
                let option = rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<OptBinarySemantic>())
                    .cloned()
                    .unwrap_or_default();
                if option.binary {
                    field_type.SetCharset("binary".to_owned());
                    field_type.SetCollate("binary".to_owned());
                    field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
                } else if !option.charset.is_empty() {
                    field_type.SetCharset(option.charset.clone());
                    if let Ok(collation) = charset::charset::GetDefaultCollation(&option.charset) {
                        field_type.SetCollate(collation);
                    }
                } else {
                    field_type.SetCharset(parser_state.charset.clone());
                    field_type.SetCollate(parser_state.collation.clone());
                }
            }
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::CastTypeAlt03 | ExpressionRule::CastTypeAlt04 => {
            let tp = if rule == ExpressionRule::CastTypeAlt03 {
                parser_mysql::r#type::TypeDate
            } else {
                parser_mysql::r#type::TypeYear
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::CastTypeAlt05 | ExpressionRule::CastTypeAlt07 => {
            let tp = if rule == ExpressionRule::CastTypeAlt05 {
                parser_mysql::r#type::TypeDatetime
            } else {
                parser_mysql::r#type::TypeDuration
            };
            let mut field_type = parser_types::types::NewFieldType(tp);
            let (flen, _) = parser_mysql::util::GetDefaultFieldLengthAndDecimalForCast(tp);
            let decimal = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<isize>())
                .copied()
                .unwrap_or_default();
            field_type.SetFlen(flen + if decimal > 0 { 1 + decimal } else { 0 });
            field_type.SetDecimal(decimal);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::CastTypeAlt06 => {
            let option = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<FloatOptSemantic>())
                .copied()
                .unwrap_or(FloatOptSemantic {
                    flen: -1,
                    decimal: -1,
                });
            let mut field_type =
                parser_types::types::NewFieldType(parser_mysql::r#type::TypeNewDecimal);
            field_type.SetFlen(option.flen);
            field_type.SetDecimal(option.decimal);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::CastTypeAlt09 => {
            let mut field_type =
                parser_types::types::NewFieldType(parser_mysql::r#type::TypeLonglong);
            field_type
                .AddFlag(parser_mysql::r#type::UnsignedFlag | parser_mysql::r#type::BinaryFlag);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::CastTypeAlt10 => {
            let mut field_type = parser_types::types::NewFieldType(parser_mysql::r#type::TypeJSON);
            field_type
                .AddFlag(parser_mysql::r#type::BinaryFlag | parser_mysql::r#type::ParseToJSONFlag);
            field_type.SetCharset(parser_mysql::charset::DefaultCharset.to_owned());
            field_type.SetCollate(parser_mysql::charset::DefaultCollationName.to_owned());
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::CastTypeAlt11
        | ExpressionRule::CastTypeAlt12
        | ExpressionRule::CastTypeAlt13 => {
            let mut tp = if rule == ExpressionRule::CastTypeAlt12
                || (rule == ExpressionRule::CastTypeAlt13 && yylex.sql_mode_bits() & 1 != 0)
            {
                parser_mysql::r#type::TypeFloat
            } else {
                parser_mysql::r#type::TypeDouble
            };
            if rule == ExpressionRule::CastTypeAlt12 {
                let precision = rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .and_then(|item| item.downcast_ref::<FloatOptSemantic>())
                    .map(|item| item.flen)
                    .unwrap_or(-1);
                if precision >= 54 {
                    yylex.AppendError(yylex.Errorf("Too-big precision for CAST", &[]));
                } else if precision >= 25 {
                    tp = parser_mysql::r#type::TypeDouble;
                }
            }
            let mut field_type = parser_types::types::NewFieldType(tp);
            let (flen, decimal) = parser_mysql::util::GetDefaultFieldLengthAndDecimalForCast(tp);
            field_type.SetFlen(flen);
            field_type.SetDecimal(decimal);
            field_type.AddFlag(parser_mysql::r#type::BinaryFlag);
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            out.item = Some(Box::new(field_type));
        }
        ExpressionRule::SimpleExprAlt08 => {
            out.expr = Some(parser_ast::ExprNode::ParamMarker(
                rhs[rhs_len - (0)].offset.max(0) as usize,
            ))
        }
        ExpressionRule::SimpleExprAlt11
        | ExpressionRule::SimpleExprAlt12
        | ExpressionRule::SimpleExprAlt13
        | ExpressionRule::SimpleExprAlt14
        | ExpressionRule::SimpleExprAlt16 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let operator = match rule {
                ExpressionRule::SimpleExprAlt12 => "~",
                ExpressionRule::SimpleExprAlt13 => "-",
                ExpressionRule::SimpleExprAlt14 => "+",
                _ => "!",
            };
            out.expr = Some(parser_ast::ExprNode::Unary(
                operator.to_owned(),
                Box::new(value),
            ));
        }
        ExpressionRule::SimpleExprAlt15 => {
            let Some(left) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let Some(right) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("concat"),
                vec![left, right],
            ));
        }
        ExpressionRule::SimpleExprAlt18 => {
            let Some(value) = rhs[rhs_len - (1)].expr.clone() else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode::Parentheses(Box::new(value)));
        }
        ExpressionRule::SimpleExprAlt21 => {
            let Some(subquery) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let sel = parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Subquery {
                    Query: subquery.query,
                    MultiRows: false,
                    Exists: true,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::ExistsSubquery {
                    Sel: Box::new(sel),
                    Not: false,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::SimpleExprAlt30 => {
            let Some(value) = rhs[rhs_len - (1)].expr.clone() else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("values"),
                vec![value],
            ));
        }
        ExpressionRule::DistinctOptAlt01 | ExpressionRule::DefaultFalseDistinctOptAlt01 => {
            out.item = Some(Box::new(false))
        }
        ExpressionRule::DistinctOptAlt02
        | ExpressionRule::DefaultTrueDistinctOptAlt01
        | ExpressionRule::BuggyDefaultFalseDistinctOptAlt02 => out.item = Some(Box::new(true)),
        ExpressionRule::ExpressionAlt01 => {
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let name = rhs[rhs_len - (2)].ident.trim_start_matches('@').to_owned();
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Variable {
                    Name: name,
                    IsGlobal: false,
                    IsInstance: false,
                    IsSystem: false,
                    ExplicitScope: false,
                    Value: Some(Box::new(value)),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::ExpressionAlt02
        | ExpressionRule::ExpressionAlt03
        | ExpressionRule::ExpressionAlt04 => {
            let Some(left) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let Some(right) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let operator = match rule {
                ExpressionRule::ExpressionAlt02 => "OR",
                ExpressionRule::ExpressionAlt03 => "XOR",
                _ => "AND",
            };
            out.expr = Some(parser_ast::ExprNode::Binary(
                operator.to_owned(),
                Box::new(left),
                Box::new(right),
            ));
        }
        ExpressionRule::ExpressionAlt05 => {
            let Some(mut value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            if let parser_ast::ExprKind::ExistsSubquery { Not, .. } = &mut value.Kind {
                *Not = !*Not;
                out.expr = Some(value);
            } else {
                out.expr = Some(parser_ast::ExprNode::Unary(
                    "NOT".to_owned(),
                    Box::new(value),
                ));
            }
        }
        ExpressionRule::ExpressionAlt06 => {
            let columns = rhs[rhs_len - (6)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ColumnName>>())
                .cloned()
                .unwrap_or_default();
            let Some(against) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let modifier = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<u8>())
                .copied()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::MatchAgainst {
                    ColumnNames: columns,
                    Against: Box::new(against),
                    Modifier: modifier,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::ExpressionAlt07 | ExpressionRule::ExpressionAlt08 => {
            let Some(expr) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let not = !rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::IsTruth {
                    Expr: Box::new(expr),
                    Not: not,
                    True: rule == ExpressionRule::ExpressionAlt07,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::ExpressionAlt09 | ExpressionRule::BoolPriAlt01 => {
            let Some(expr) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let not = !rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::IsNull {
                    Expr: Box::new(expr),
                    Not: not,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::ExpressionListAlt01
        | ExpressionRule::MaxValueOrExpressionListAlt01
        | ExpressionRule::DefaultOrExpressionListAlt01 => {
            out.item = Some(Box::new(
                rhs[rhs_len - (0)]
                    .expr
                    .clone()
                    .into_iter()
                    .collect::<Vec<_>>(),
            ));
        }
        ExpressionRule::ExpressionListAlt02
        | ExpressionRule::MaxValueOrExpressionListAlt02
        | ExpressionRule::DefaultOrExpressionListAlt02 => {
            let mut values = rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            if let Some(value) = rhs[rhs_len - (0)].expr.clone() {
                values.push(value);
            }
            out.item = Some(Box::new(values));
        }
        ExpressionRule::ExpressionListOptAlt01 | ExpressionRule::FuncDatetimePrecListOptAlt01 => {
            out.item = Some(Box::new(Vec::<parser_ast::ExprNode>::new()))
        }
        ExpressionRule::DefaultOrExpressionAlt01 => {
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::DefaultValue,
                OriginTextPosition: 0,
                Flag: Default::default(),
            })
        }
        ExpressionRule::MaxValueOrExpressionAlt01 => {
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::MaxValue,
                OriginTextPosition: 0,
                Flag: Default::default(),
            })
        }
        ExpressionRule::FulltextSearchModifierOptAlt01
        | ExpressionRule::FulltextSearchModifierOptAlt02
        | ExpressionRule::FulltextSearchModifierOptAlt03
        | ExpressionRule::FulltextSearchModifierOptAlt04
        | ExpressionRule::FulltextSearchModifierOptAlt05 => {
            out.item = Some(Box::new(match rule {
                ExpressionRule::FulltextSearchModifierOptAlt04 => 1u8,
                ExpressionRule::FulltextSearchModifierOptAlt03
                | ExpressionRule::FulltextSearchModifierOptAlt05 => 1u8 << 4,
                _ => 0u8,
            }))
        }
        ExpressionRule::FuncDatetimePrecListAlt01 => {
            out.item = Some(Box::new(vec![parser_ast::ExprNode::Value(
                rhs[rhs_len - (0)]
                    .item
                    .as_deref()
                    .map(semantic_value_text)
                    .unwrap_or_default(),
            )]))
        }
        ExpressionRule::BoolPriAlt02 => {
            let Some(left) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let Some(right) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let operator = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<String>())
                .cloned()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::Binary(
                operator,
                Box::new(left),
                Box::new(right),
            ));
        }
        ExpressionRule::BoolPriAlt03 => {
            let Some(subquery) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(left) = rhs[rhs_len - (3)].expr.clone() else {
                return Ok(false);
            };
            let sel = parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Subquery {
                    Query: subquery.query,
                    MultiRows: true,
                    Exists: false,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::CompareSubquery {
                    Op: rhs[rhs_len - (2)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<String>())
                        .cloned()
                        .unwrap_or_default(),
                    L: Box::new(left),
                    R: Box::new(sel),
                    All: rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::BoolPriAlt04 => {
            let Some(left) = rhs[rhs_len - (4)].expr.clone() else {
                return Ok(false);
            };
            let Some(value) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let variable = parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Variable {
                    Name: rhs[rhs_len - (2)].ident.trim_start_matches('@').to_owned(),
                    IsGlobal: false,
                    IsInstance: false,
                    IsSystem: false,
                    ExplicitScope: false,
                    Value: Some(Box::new(value)),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            };
            let operator = rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<String>())
                .cloned()
                .unwrap_or_default();
            out.expr = Some(parser_ast::ExprNode::Binary(
                operator,
                Box::new(left),
                Box::new(variable),
            ));
        }
        ExpressionRule::CompareOpAlt01
        | ExpressionRule::CompareOpAlt02
        | ExpressionRule::CompareOpAlt03
        | ExpressionRule::CompareOpAlt04
        | ExpressionRule::CompareOpAlt05
        | ExpressionRule::CompareOpAlt06
        | ExpressionRule::CompareOpAlt07
        | ExpressionRule::CompareOpAlt08 => {
            let operator = match rule {
                ExpressionRule::CompareOpAlt01 => ">=",
                ExpressionRule::CompareOpAlt02 => ">",
                ExpressionRule::CompareOpAlt03 => "<=",
                ExpressionRule::CompareOpAlt04 => "<",
                ExpressionRule::CompareOpAlt05 | ExpressionRule::CompareOpAlt06 => "!=",
                ExpressionRule::CompareOpAlt07 => "=",
                _ => "<=>",
            };
            out.item = Some(Box::new(operator.to_owned()));
        }
        ExpressionRule::BetweenOrNotOpAlt01
        | ExpressionRule::IsOrNotOpAlt01
        | ExpressionRule::InOrNotOpAlt01
        | ExpressionRule::LikeOrNotOpAlt01
        | ExpressionRule::IlikeOrNotOpAlt01
        | ExpressionRule::RegexpOrNotOpAlt01
        | ExpressionRule::AnyOrAllAlt03 => out.item = Some(Box::new(true)),
        ExpressionRule::BetweenOrNotOpAlt02
        | ExpressionRule::IsOrNotOpAlt02
        | ExpressionRule::InOrNotOpAlt02
        | ExpressionRule::LikeOrNotOpAlt02
        | ExpressionRule::IlikeOrNotOpAlt02
        | ExpressionRule::RegexpOrNotOpAlt02
        | ExpressionRule::AnyOrAllAlt01
        | ExpressionRule::AnyOrAllAlt02 => out.item = Some(Box::new(false)),
        ExpressionRule::PredicateExprAlt01 => {
            let Some(expr) = rhs[rhs_len - (4)].expr.clone() else {
                return Ok(false);
            };
            let list = rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<Vec<parser_ast::ExprNode>>())
                .cloned()
                .unwrap_or_default();
            let not = !rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::InList {
                    Expr: Box::new(expr),
                    List: list,
                    Not: not,
                    Type: parser_ast::ExprNode::PredicateType(),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::PredicateExprAlt02 => {
            let Some(subquery) = rhs[rhs_len - (0)]
                .item
                .take()
                .and_then(|item| item.downcast::<SubquerySemantic>().ok())
                .map(|item| *item)
            else {
                return Ok(false);
            };
            let Some(expr) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let sel = parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Subquery {
                    Query: subquery.query,
                    MultiRows: true,
                    Exists: false,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            };
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::InSubquery {
                    Expr: Box::new(expr),
                    Sel: Box::new(sel),
                    Not: !rhs[rhs_len - (1)]
                        .item
                        .as_deref()
                        .and_then(|item| item.downcast_ref::<bool>())
                        .copied()
                        .unwrap_or(false),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::PredicateExprAlt07 => {
            let (Some(left), Some(right)) = (
                rhs[rhs_len - (4)].expr.clone(),
                rhs[rhs_len - (1)].expr.clone(),
            ) else {
                return Ok(false);
            };
            out.expr = Some(parser_ast::ExprNode::Function(
                parser_ast::CIStr::default(),
                parser_ast::NewCIStr("MEMBER OF"),
                vec![left, right],
            ));
        }
        ExpressionRule::PredicateExprAlt03 => {
            let Some(expr) = rhs[rhs_len - (4)].expr.clone() else {
                return Ok(false);
            };
            let Some(left) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let Some(right) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let not = !rhs[rhs_len - (3)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Between {
                    Expr: Box::new(expr),
                    Left: Box::new(left),
                    Right: Box::new(right),
                    Not: not,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::PredicateExprAlt04 | ExpressionRule::PredicateExprAlt05 => {
            let Some(expr) = rhs[rhs_len - (3)].expr.clone() else {
                return Ok(false);
            };
            let Some(pattern) = rhs[rhs_len - (1)].expr.clone() else {
                return Ok(false);
            };
            let escape = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<likeEscapeSpec>());
            if escape.is_some_and(|value| value.escape.len() > 1) {
                yylex.AppendError(ErrWrongArguments.GenWithStackByArgs(&["ESCAPE".into()]));
                return Err(1);
            }
            let not = !rhs[rhs_len - (2)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Like {
                    Expr: Box::new(expr),
                    Pattern: Box::new(pattern),
                    Not: not,
                    Escape: escape
                        .map(|value| value.escape.clone())
                        .unwrap_or_else(|| "\\".to_owned()),
                    Explicit: escape.map(|value| value.explicit).unwrap_or(false),
                    IsLike: rule == ExpressionRule::PredicateExprAlt04,
                    Type: parser_ast::ExprNode::PredicateType(),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::PredicateExprAlt06 => {
            let Some(expr) = rhs[rhs_len - (2)].expr.clone() else {
                return Ok(false);
            };
            let Some(pattern) = rhs[rhs_len - (0)].expr.clone() else {
                return Ok(false);
            };
            let not = !rhs[rhs_len - (1)]
                .item
                .as_deref()
                .and_then(|item| item.downcast_ref::<bool>())
                .copied()
                .unwrap_or(false);
            out.expr = Some(parser_ast::ExprNode {
                node_text: Default::default(),
                Kind: parser_ast::ExprKind::Regexp {
                    Expr: Box::new(expr),
                    Pattern: Box::new(pattern),
                    Not: not,
                    Type: parser_ast::ExprNode::PredicateType(),
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            });
        }
        ExpressionRule::LikeOrIlikeEscapeOptAlt01 => {
            out.item = Some(Box::new(likeEscapeSpec {
                escape: "\\".to_owned(),
                explicit: false,
            }))
        }
        ExpressionRule::LikeOrIlikeEscapeOptAlt02 => {
            out.item = Some(Box::new(likeEscapeSpec {
                escape: rhs[rhs_len - (0)].ident.clone(),
                explicit: true,
            }))
        }
        ExpressionRule::SignedNumAlt02 => out.item = rhs[rhs_len - (0)].item.take(),
        ExpressionRule::SignedNumAlt03 => {
            let unsigned = rhs[rhs_len - (0)]
                .item
                .as_deref()
                .map(getUint64FromNUM)
                .unwrap_or_default();
            if unsigned > (i64::MAX as u64) + 1 {
                yylex.AppendError(
                    yylex.Errorf("the Signed Value should be at the range of int64", &[]),
                );
                return Err(1);
            }
            out.item = Some(Box::new(if unsigned == (i64::MAX as u64) + 1 {
                i64::MIN
            } else {
                -(unsigned as i64)
            }));
        }
    }
    Ok(true)
}
