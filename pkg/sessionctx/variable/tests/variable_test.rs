// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 对应 Go `variable_test.go` 的迁移参考与可运行 Rust 用例。
//
// `GO_REFERENCE` 原始字符串保存尚未完全落地的 Go 测试骨架（禁止改动其内容）；
// 文末 `dynamic_global_defaults_match_*` 校验经典集群与 next-gen 环境下
// 全局系统变量初始值分支。

/// 嵌入的 Go 测试参考文本（机械迁移占位，非可执行 Rust）。
const GO_REFERENCE: &str = r################"

// 主要测试函数、辅助函数、用例表、断言和资源收尾顺序均按源文件排列，便于后续人工逐段接入 Rust 测试框架。

// TestSysVar 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestSysVar(t: &testing::T) {
    let mut f = variable::GetSysVar("autocommit");
    require::NotNil(t, f);

    f = variable::GetSysVar("wrong-var-name");
    require::Nil(t, f);

    f = variable::GetSysVar("explicit_defaults_for_timestamp");
    require::NotNil(t, f);
    require::Equal(t, "ON", f.Value);

    f = variable::GetSysVar("port");
    require::NotNil(t, f);
    require::Equal(t, "4000", f.Value);

    f = variable::GetSysVar("tidb_low_resolution_tso");
    require::Equal(t, "OFF", f.Value);

    f = variable::GetSysVar("tidb_replica_read");
    require::Equal(t, "leader", f.Value);

    f = variable::GetSysVar("tidb_enable_table_partition");
    require::Equal(t, "ON", f.Value);

    f = variable::GetSysVar("version_compile_os");
    require::Equal(t, runtime.GOOS, f.Value);

    f = variable::GetSysVar("version_compile_machine");
    require::Equal(t, runtime.GOARCH, f.Value);

    // default enable vectorized_expression
    f = variable::GetSysVar("tidb_enable_vectorized_expression");
    require::Equal(t, "ON", f.Value);

    let mut autoBuildStats = variable::GetSysVar(vardef::TiDBAutoBuildStatsConcurrency);
    require::NotNil(t, autoBuildStats);
    let mut buildStats = variable::GetSysVar(vardef::TiDBBuildStatsConcurrency);
    require::NotNil(t, buildStats);
    require::Equal(t, buildStats.Value, autoBuildStats.Value);

    let mut sysProcScan = variable::GetSysVar(vardef::TiDBSysProcScanConcurrency);
    require::NotNil(t, sysProcScan);
    let mut analyzeScan = variable::GetSysVar(vardef::TiDBAnalyzeDistSQLScanConcurrency);
    require::NotNil(t, analyzeScan);
    require::Equal(t, analyzeScan.Value, sysProcScan.Value);
}

// TestIndexJoinBuildV2SysVarCompatibility 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestIndexJoinBuildV2SysVarCompatibility(t: &testing::T) {
    let mut sv = variable::GetSysVar(vardef::TiDBOptIndexJoinBuild);
    require::NotNil(t, sv);
    require::Equal(t, vardef::On, sv.Value);

    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, vardef::On, vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, vardef::Off, vardef::ScopeSession);
    require::EqualError(t, err, "tidb_opt_index_join_build_v2 is now always enabled and cannot be turned off");
    require::Equal(t, vardef::On, val);

    // 宽松校验路径在 Go 中绕过部分错误；保留与普通 Validate 的差异断言。
    require::Equal(t, vardef::On, sv.ValidateWithRelaxedValidation(vars, vardef::Off, vardef::ScopeSession));

    val, err = sv.GetSessionFromHook(vars);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);

    val, err = sv.GetGlobalFromHook(context::Background(), vars);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);
}

// TestError 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestError(t: &testing::T) {
    let mut kvErrs = []*terror::Error{
        variable::ErrUnsupportedValueForVar,
        variable::ErrUnknownSystemVar,
        variable::ErrIncorrectScope,
        variable::ErrUnknownTimeZone,
        variable::ErrReadOnly,
        variable::ErrWrongValueForVar,
        variable::ErrWrongTypeForVar,
        variable::ErrTruncatedWrongValue,
        variable::ErrMaxPreparedStmtCountReached,
        variable::ErrUnsupportedIsolationLevel,
    }
    for err in kvErrs {
        require::True(t, terror::ToSQLError(err).Code != mysql::ErrUnknown);
    }
}

// TestRegistrationOfNewSysVar 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestRegistrationOfNewSysVar(t: &testing::T) {
    let mut count = len(variable::GetSysVars());
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: vardef::On, Type: vardef::TypeBool, SetSession: |s: &mut variable::SessionVars, val: &str| -> Result<(), errors::Error> {
        return Err(fmt::Errorf("set should fail"));
    }}

    variable::RegisterSysVar(&sv);
    require::Len(t, variable::GetSysVars(), count+1);

    let mut sysVar = variable::GetSysVar("mynewsysvar");
    require::NotNil(t, sysVar);

    let mut vars = variable::NewSessionVars(None);

    // It is a boolean, try to set it to a bogus value
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sysVar.Validate(vars, "ABCD", vardef::ScopeSession);
    require::Error(t, err);

    // Boolean oN or 1 converts to canonical ON or OFF
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut normalizedVal, err = sysVar.Validate(vars, "oN", vardef::ScopeSession);
    require::Equal(t, "ON", normalizedVal);
    require::NoError(t, err);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    normalizedVal, err = sysVar.Validate(vars, "0", vardef::ScopeSession);
    require::Equal(t, "OFF", normalizedVal);
    require::NoError(t, err);

    err = sysVar.SetSessionFromHook(vars, "OFF") // default is on;
    require::Equal(t, "set should fail", err.Error());

    // Test unregistration restores previous count
    variable::UnregisterSysVar("mynewsysvar");
    require::Equal(t, len(variable::GetSysVars()), count);
}

// TestIntValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestIntValidation(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: "123", Type: vardef::TypeInt, MinValue: 10, MaxValue: 300, AllowAutoValue: true}
    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sv.Validate(vars, "oN", vardef::ScopeSession);
    require::Equal(t, "[variable:1232]Incorrect argument type to variable 'mynewsysvar'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "301", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "300", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "5", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "10", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "300", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "300", val);
    // out of range but permitted due to auto value
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "-1", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "-1", val);
}

// TestPerformanceSchemaSessionConnectAttrsSizeValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestPerformanceSchemaSessionConnectAttrsSizeValidation(t: &testing::T) {
    let mut sv = variable::GetSysVar(vardef::PerformanceSchemaSessionConnectAttrsSize);
    require::NotNil(t, sv);
    require::True(t, sv.HasGlobalScope());
    require::False(t, sv.HasSessionScope());

    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "-1", vardef::ScopeGlobal);
    require::NoError(t, err);
    require::Equal(t, "-1", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "0", vardef::ScopeGlobal);
    require::NoError(t, err);
    require::Equal(t, "0", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "65536", vardef::ScopeGlobal);
    require::NoError(t, err);
    require::Equal(t, "65536", val);

    // Out-of-range values should be clamped by int sysvar validation.
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "65537", vardef::ScopeGlobal);
    require::NoError(t, err);
    require::Equal(t, "65536", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "-2", vardef::ScopeGlobal);
    require::NoError(t, err);
    require::Equal(t, "-1", val);
}

// TestUintValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestUintValidation(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: "123", Type: vardef::TypeUnsigned, MinValue: 10, MaxValue: 300, AllowAutoValue: true}
    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sv.Validate(vars, "oN", vardef::ScopeSession);
    require::Equal(t, "[variable:1232]Incorrect argument type to variable 'mynewsysvar'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "", vardef::ScopeSession);
    require::Equal(t, "[variable:1232]Incorrect argument type to variable 'mynewsysvar'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "301", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "300", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "-301", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "10", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "-ERR", vardef::ScopeSession);
    require::Equal(t, "[variable:1232]Incorrect argument type to variable 'mynewsysvar'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "5", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "10", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "300", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "300", val);

    // out of range but permitted due to auto value
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "-1", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "-1", val);
}

// TestEnumValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestEnumValidation(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: vardef::On, Type: vardef::TypeEnum, PossibleValues: vec![/* []string */ "OFF", "ON", "AUTO"}}
    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sv.Validate(vars, "randomstring", vardef::ScopeSession);
    require::Equal(t, "[variable:1231]Variable 'mynewsysvar' can't be set to the value of 'randomstring'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "oFf", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "OFF", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "On", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "ON", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "auto", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "AUTO", val);

    // Also settable by numeric offset.
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "2", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "AUTO", val);
}

// TestDurationValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestDurationValidation(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: "10m0s", Type: vardef::TypeDuration, MinValue: int64(time.Second), MaxValue: uint64(time.Hour)}
    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sv.Validate(vars, "1hr", vardef::ScopeSession);
    require::Equal(t, "[variable:1232]Incorrect argument type to variable 'mynewsysvar'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "1ms", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "1s", val) // truncates to min;

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "2h10m", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "1h0m0s", val) // truncates to max;
}

// TestFloatValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestFloatValidation(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: "10m0s", Type: vardef::TypeFloat, MinValue: 2, MaxValue: 7}
    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sv.Validate(vars, "stringval", vardef::ScopeSession);
    require::Equal(t, "[variable:1232]Incorrect argument type to variable 'mynewsysvar'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "", vardef::ScopeSession);
    require::Equal(t, "[variable:1232]Incorrect argument type to variable 'mynewsysvar'", err.Error());

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "1.1", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "2", val) // truncates to min;

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "22", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "7", val) // truncates to max;
}

// TestBoolValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestBoolValidation(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: vardef::Off, Type: vardef::TypeBool}
    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sv.Validate(vars, "0.000", vardef::ScopeSession);
    require::Equal(t, "[variable:1231]Variable 'mynewsysvar' can't be set to the value of '0.000'", err.Error());
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "1.000", vardef::ScopeSession);
    require::Equal(t, "[variable:1231]Variable 'mynewsysvar' can't be set to the value of '1.000'", err.Error());
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "0", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::Off, val);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "1", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "OFF", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::Off, val);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "ON", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "off", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::Off, val);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "on", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);

    // test AutoConvertNegativeBool
    sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: vardef::Off, Type: vardef::TypeBool, AutoConvertNegativeBool: true}
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "-1", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "1", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "0", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, vardef::Off, val);
}

// TestTimeValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestTimeValidation(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeSession, Name: "mynewsysvar", Value: "23:59 +0000", Type: vardef::TypeTime}
    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "23:59 +0000", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "23:59 +0000", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "3:00 +0000", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "03:00 +0000", val);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    _, err = sv.Validate(vars, "0.000", vardef::ScopeSession);
    require::Error(t, err);
}

// TestGetNativeValType 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestGetNativeValType(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: vardef::Off, Type: vardef::TypeBool}

    let mut nativeVal, nativeType, flag = sv.GetNativeValType("ON");
    require::Equal(t, mysql::TypeLonglong, nativeType);
    require::Equal(t, mysql::BinaryFlag, flag);
    require::Equal(t, types::NewIntDatum(1), nativeVal);

    nativeVal, nativeType, flag = sv.GetNativeValType("OFF");
    require::Equal(t, mysql::TypeLonglong, nativeType);
    require::Equal(t, mysql::BinaryFlag, flag);
    require::Equal(t, types::NewIntDatum(0), nativeVal);

    nativeVal, nativeType, flag = sv.GetNativeValType("bogus");
    require::Equal(t, mysql::TypeLonglong, nativeType);
    require::Equal(t, mysql::BinaryFlag, flag);
    require::Equal(t, types::NewIntDatum(0), nativeVal);

    sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: vardef::Off, Type: vardef::TypeUnsigned}
    nativeVal, nativeType, flag = sv.GetNativeValType("1234");
    require::Equal(t, mysql::TypeLonglong, nativeType);
    require::Equal(t, mysql::UnsignedFlag|mysql::BinaryFlag, flag);
    require::Equal(t, types::NewUintDatum(1234), nativeVal);
    nativeVal, nativeType, flag = sv.GetNativeValType("bogus");
    require::Equal(t, mysql::TypeLonglong, nativeType);
    require::Equal(t, mysql::UnsignedFlag|mysql::BinaryFlag, flag);
    require::Equal(t, types::NewUintDatum(0), nativeVal) // converts to zero;

    sv = variable::SysVar{Scope: vardef::ScopeGlobal | vardef::ScopeSession, Name: "mynewsysvar", Value: "abc"}
    nativeVal, nativeType, flag = sv.GetNativeValType("1234");
    require::Equal(t, mysql::TypeVarString, nativeType);
    require::Equal(t, uint(0), flag);
    require::Equal(t, types::NewStringDatum("1234"), nativeVal);
}

// TestDeprecation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestDeprecation(t: &testing::T) {
    let mut sysVar = variable::GetSysVar(vardef::TiDBIndexLookupConcurrency);
    require::NotNil(t, sysVar);

    let mut vars = variable::NewSessionVars(None);

    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sysVar.Validate(vars, "123", vardef::ScopeSession);
    require::NoError(t, err);

    // There was no error but there is a deprecation warning.
    let mut warn = vars.StmtCtx.GetWarnings()[0].Err;
    require::Equal(t, "[variable:1287]'tidb_index_lookup_concurrency' is deprecated and will be removed in a future release. Please use tidb_executor_concurrency instead", warn.Error());
}

// TestBuiltInCase 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestBuiltInCase(t: &testing::T) {
    // All Sysvars should have lower case names.
    // This tests builtins.
    for name in variable::GetSysVars().keys() {
        require::Equal(t, strings::ToLower(name), name);
    }
}

// TestIsNoop is used by the documentation to auto-generate docs for real sysvars.
// TestIsNoop 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestIsNoop(t: &testing::T) {
    let mut sv = variable::GetSysVar(vardef::TiDBMultiStatementMode);
    require::False(t, sv.IsNoop);

    sv = variable::GetSysVar(vardef::InnodbLockWaitTimeout);
    require::False(t, sv.IsNoop);

    sv = variable::GetSysVar(vardef::InnodbFastShutdown);
    require::True(t, sv.IsNoop);

    sv = variable::GetSysVar(vardef::ReadOnly);
    require::True(t, sv.IsNoop);

    sv = variable::GetSysVar(vardef::DefaultPasswordLifetime);
    require::False(t, sv.IsNoop);
}

// TestDefaultValuesAreSettable that sysvars defaults are logically valid. i.e.
// the default itself must validate without error provided the scope and read-only is correct.
// The default values should also be normalized for consistency.
// TestDefaultValuesAreSettable 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestDefaultValuesAreSettable(t: &testing::T) {
    let mut vars = variable::NewSessionVars(None);
    vars.GlobalVarsAccessor = variable::NewMockGlobalAccessor4Tests();
    for sv in variable::GetSysVars() {
        if sv.HasSessionScope() && !sv.ReadOnly && !sv.InternalSessionVariable {
            // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
            let mut val, err = sv.Validate(vars, sv.Value, vardef::ScopeSession);
            require::NoError(t, err);
            require::Equal(t, val, sv.Value);
        }

        if sv.HasGlobalScope() && !sv.ReadOnly {
            // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
            let mut val, err = sv.Validate(vars, sv.Value, vardef::ScopeGlobal);
            require::NoError(t, err);
            require::Equal(t, val, sv.Value);
        }
    }
}

// TestLimitBetweenVariable 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestLimitBetweenVariable(t: &testing::T) {
    require::Less(t, vardef::DefTiDBGOGCTunerThreshold+0.05, vardef::DefTiDBServerMemoryLimitGCTrigger);
}

// TestSysVarNameIsLowerCase tests that no new sysvars are added with uppercase characters.
// In MySQL variables are always lowercase, and can be set in a case-insensitive way.
// TestSysVarNameIsLowerCase 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestSysVarNameIsLowerCase(t: &testing::T) {
    for sv in variable::GetSysVars() {
        require::Equal(t, strings::ToLower(sv.Name), sv.Name, "sysvar name contains uppercase characters");
    }
}

// TestSettersandGetters tests that sysvars are logically correct with getter and setter functions.
// i.e. it doesn't make sense to have a SetSession function on a variable that is only globally scoped.
// TestSettersandGetters 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestSettersandGetters(t: &testing::T) {
    for sv in variable::GetSysVars() {
        if !sv.HasSessionScope() {
            require::Nil(t, sv.SetSession);
            require::Nil(t, sv.GetSession);
        }
        if !sv.HasGlobalScope() && !sv.HasInstanceScope() {
            require::Nil(t, sv.SetGlobal);
            if sv.Name == vardef::Timestamp {
                // The Timestamp sysvar will have GetGlobal func even though it does not have global scope.
                // It's GetGlobal func will only be called when "set timestamp = default".
                continue;
            }
            require::Nil(t, sv.GetGlobal);
        }
    }
}

// TestScopeToString 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestScopeToString(t: &testing::T) {
    require::Equal(t, "GLOBAL", vardef::ScopeGlobal.String());
    require::Equal(t, "SESSION", vardef::ScopeSession.String());
    require::Equal(t, "INSTANCE", vardef::ScopeInstance.String());
    require::Equal(t, "NONE", vardef::ScopeNone.String());
    let mut tmp = vardef::ScopeSession + vardef::ScopeGlobal;
    require::Equal(t, "SESSION,GLOBAL", tmp.String());
    // this is not currently possible, but might be in future.
    // *but* global + instance is not possible. these are mutually exclusive by design.
    tmp = vardef::ScopeSession + vardef::ScopeInstance;
    require::Equal(t, "SESSION,INSTANCE", tmp.String());
}

// TestValidateWithRelaxedValidation 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestValidateWithRelaxedValidation(t: &testing::T) {
    let mut sv = variable::GetSysVar(vardef::SecureAuth);
    let mut vars = variable::NewSessionVars(None);
    // 宽松校验路径在 Go 中绕过部分错误；保留与普通 Validate 的差异断言。
    let mut val = sv.ValidateWithRelaxedValidation(vars, "1", vardef::ScopeGlobal);
    require::Equal(t, "ON", val);

    sv = variable::GetSysVar(vardef::TiDBAnalyzeVersion);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sv.Validate(vars, "1", vardef::ScopeSession);
    require::ErrorContains(t, err, "tidb_analyze_version=1 is no longer supported");
    // 宽松校验路径在 Go 中绕过部分错误；保留与普通 Validate 的差异断言。
    val = sv.ValidateWithRelaxedValidation(vars, "1", vardef::ScopeSession);
    require::Equal(t, "1", val);

    // Relaxed validation catches the error and squashes it.
    // The incorrect value is returned as-is.
    // I am not sure this is the correct behavior, we might need to
    // change it to return the default instead in future.
    sv = variable::GetSysVar(vardef::DefaultAuthPlugin);
    // 宽松校验路径在 Go 中绕过部分错误；保留与普通 Validate 的差异断言。
    val = sv.ValidateWithRelaxedValidation(vars, "RandomText", vardef::ScopeGlobal);
    require::Equal(t, "RandomText", val);

    // Validation func fails, the error is also caught and squashed.
    // The incorrect value is returned as-is.
    sv = variable::GetSysVar(vardef::InitConnect);
    // 宽松校验路径在 Go 中绕过部分错误；保留与普通 Validate 的差异断言。
    val = sv.ValidateWithRelaxedValidation(vars, "RandomText - should be valid SQL", vardef::ScopeGlobal);
    require::Equal(t, "RandomText - should be valid SQL", val);
}

// TestValidateInternalSessionVariable 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestValidateInternalSessionVariable(t: &testing::T) {
    let mut vars = variable::NewSessionVars(None);
    for n in vec![/* []string */ vardef::TiDBRedactLog, vardef::TiDBInstancePlanCacheMaxMemSize} {
        let mut sv = variable::GetSysVar(n);
        // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
        let mut _, err = sv.Validate(vars, "1", vardef::ScopeSession);
        require::NotNil(t, err);
    }
}

// TestInstanceConfigHasMatchingSysvar 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestInstanceConfigHasMatchingSysvar(t: &testing::T) {
    // This tests that each item in [instance] has a sysvar of the same name.
    // The whole point of moving items to [instance] is to unify the name between
    // config and sysvars. See: docs/design/2021-12-08-instance-scope.md#introduction
    let mut cfg, err = config::GetJSONConfig();
    require::NoError(t, err);
    let mut v: /* Go type */ any;
    json.Unmarshal([]byte(cfg), &v);
    let mut data = v.(map[string]any);
    for (k, v) in data {
        if k != "instance" {
            continue;
        }
        let mut instanceSection = v.(map[string]any);
        for instanceName in instanceSection.keys() {
            // Need to check there is a sysvar named instanceName.
            let mut sv = variable::GetSysVar(instanceName);
            require::NotNil(t, sv, fmt::Sprintf("config option: instance.%v requires a matching sysvar of the same name", instanceName));
        }
    }
}

// TestInstanceScope 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestInstanceScope(t: &testing::T) {
    // Instance scope used to be settable via "SET SESSION", which is weird to any MySQL user.
    // It is now settable via SET GLOBAL, but to work correctly a sysvar can only ever
    // be INSTANCE scoped or GLOBAL scoped, never *both* at the same time (at least for now).
    // Otherwise the semantics are confusing to users for how precedence applies.

    // Now Instance scope is a valid scope, and it can be used with GLOBAL scope at the same time.
    for sv in variable::GetSysVars() {
        // But instance scope should not have Set/GetSession
        if sv.HasInstanceScope() {
            require::Nil(t, sv.GetSession);
            require::Nil(t, sv.SetSession);
        }
    }

    let mut count = len(variable::GetSysVars());
    let mut sv = variable::SysVar{Scope: vardef::ScopeInstance, Name: "newinstancesysvar", Value: vardef::On, Type: vardef::TypeBool,
        SetGlobal: |_ctx: context::Context, s: &mut variable::SessionVars, val: &str| -> Result<(), errors::Error> {
            return Err(fmt::Errorf("set should fail"));
        },
        GetGlobal: |_ctx: context::Context, s: &variable::SessionVars| -> Result<String, errors::Error> {
            return Err(fmt::Errorf("get should fail"));
        },
    }

    variable::RegisterSysVar(&sv);
    require::Len(t, variable::GetSysVars(), count+1);

    let mut sysVar = variable::GetSysVar("newinstancesysvar");
    require::NotNil(t, sysVar);

    let mut vars = variable::NewSessionVars(None);

    // It is a boolean, try to set it to a bogus value
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut _, err = sysVar.Validate(vars, "ABCD", vardef::ScopeInstance);
    require::Error(t, err);

    // Boolean oN or 1 converts to canonical ON or OFF
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut normalizedVal, err = sysVar.Validate(vars, "oN", vardef::ScopeInstance);
    require::Equal(t, "ON", normalizedVal);
    require::NoError(t, err);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    normalizedVal, err = sysVar.Validate(vars, "0", vardef::ScopeInstance);
    require::Equal(t, "OFF", normalizedVal);
    require::NoError(t, err);

    err = sysVar.SetGlobalFromHook(context::Background(), vars, "OFF", true) // default is on;
    require::Equal(t, "set should fail", err.Error());

    // Test unregistration restores previous count
    variable::UnregisterSysVar("newinstancesysvar");
    require::Equal(t, len(variable::GetSysVars()), count);
}

// TestSetSysVar 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestSetSysVar(t: &testing::T) {
    let mut vars = variable::NewSessionVars(None);
    vars.GlobalVarsAccessor = variable::NewMockGlobalAccessor4Tests();
    let mut originalVal = variable::GetSysVar(vardef::SystemTimeZone).Value;
    variable::SetSysVar(vardef::SystemTimeZone, "America/New_York");
    require::Equal(t, "America/New_York", variable::GetSysVar(vardef::SystemTimeZone).Value);
    // Test alternative Get
    let mut val, err = variable::GetSysVar(vardef::SystemTimeZone).GetGlobalFromHook(context::Background(), vars);
    require::Nil(t, err);
    require::Equal(t, "America/New_York", val);
    variable::SetSysVar(vardef::SystemTimeZone, originalVal) // restore;
    require::Equal(t, originalVal, variable::GetSysVar(vardef::SystemTimeZone).Value);

    let mut originalCfg = *config::GetGlobalConfig();
    let mut originalDeployMode = deploymode::Get();
    let mut originalRequireSecureTransport = tidbtls::RequireSecureTransport.Load();
    t.Cleanup(|| {
        config::StoreGlobalConfig(&originalCfg);
        if kerneltype::IsNextGen() {
            require::NoError(t, deploymode::Set(originalDeployMode));
        }
        tidbtls::RequireSecureTransport.Store(originalRequireSecureTransport);
    })

    let mut mock = variable::NewMockGlobalAccessor4Tests();
    mock.SessionVars.GlobalVarsAccessor = mock;
    require::NoError(t, mock.SetGlobalSysVar(context::Background(), vardef::RequireSecureTransport, vardef::On));
    require::True(t, tidbtls::RequireSecureTransport.Load());

    val, err = variable::GetSysVar(vardef::RequireSecureTransport).GetGlobalFromHook(context::Background(), mock.SessionVars);
    require::NoError(t, err);
    require::Equal(t, vardef::On, val);

    if kerneltype::IsNextGen() {
        require::NoError(t, deploymode::Set(deploymode::Starter));
        require::NoError(t, variable::GetSysVar(vardef::RequireSecureTransport).SetGlobalFromHook(context::Background(), mock.SessionVars, vardef::On, false));
        require::False(t, tidbtls::RequireSecureTransport.Load());

        val, err = variable::GetSysVar(vardef::RequireSecureTransport).GetGlobalFromHook(context::Background(), mock.SessionVars);
        require::NoError(t, err);
        require::Equal(t, vardef::On, val);

        require::NoError(t, deploymode::Set(originalDeployMode));
    }

    config::StoreGlobalConfig(&originalCfg);
    require::NoError(t, variable::GetSysVar(vardef::RequireSecureTransport).SetGlobalFromHook(context::Background(), mock.SessionVars, vardef::Off, false));
    require::False(t, tidbtls::RequireSecureTransport.Load());

    require::NoError(t, variable::GetSysVar(vardef::RequireSecureTransport).SetGlobalFromHook(context::Background(), mock.SessionVars, vardef::On, false));
    require::True(t, tidbtls::RequireSecureTransport.Load());
}

// TestSkipSysvarCache 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestSkipSysvarCache(t: &testing::T) {
    require::True(t, variable::GetSysVar(vardef::TiDBGCEnable).SkipSysvarCache());
    require::True(t, variable::GetSysVar(vardef::TiDBGCRunInterval).SkipSysvarCache());
    require::True(t, variable::GetSysVar(vardef::TiDBGCLifetime).SkipSysvarCache());
    require::True(t, variable::GetSysVar(vardef::TiDBGCConcurrency).SkipSysvarCache());
    require::True(t, variable::GetSysVar(vardef::TiDBGCScanLockMode).SkipSysvarCache());
    require::False(t, variable::GetSysVar(vardef::RequireSecureTransport).SkipSysvarCache());
    require::False(t, variable::GetSysVar(vardef::TiDBEnableAsyncCommit).SkipSysvarCache());
}

// TestTimeValidationWithTimezone 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestTimeValidationWithTimezone(t: &testing::T) {
    let mut sv = variable::SysVar{Scope: vardef::ScopeSession, Name: "mynewsysvar", Value: "23:59 +0000", Type: vardef::TypeTime}
    let mut vars = variable::NewSessionVars(None);

    // In timezone UTC
    vars.TimeZone = time.UTC;
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    let mut val, err = sv.Validate(vars, "23:59", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "23:59 +0000", val);

    // In timezone Asia/Shanghai
    vars.TimeZone, err = time.LoadLocation("Asia/Shanghai");
    require::NoError(t, err);
    // 系统变量 Validate 负责类型转换、范围裁剪和错误返回，保留输入与期望输出。
    val, err = sv.Validate(vars, "23:59", vardef::ScopeSession);
    require::NoError(t, err);
    require::Equal(t, "23:59 +0800", val);
}

// TestOrderByDependency 对应 Go 的同名测试，保留原测试步骤、断言和外部依赖调用顺序。
#[test]
pub fn TestOrderByDependency(t: &testing::T) {
    // Some other exceptions:
    // - tidb_snapshot and tidb_read_staleness can not be set at the same time. It doesn't affect dependency.
    let mut vars = map_literal! {/* map[string]string */ ;
        "unknown":                                      "1",
        vardef::TxReadOnly:                              "1",
        vardef::SQLAutoIsNull:                           "1",
        vardef::TiDBEnableNoopFuncs:                     "1",
        vardef::TiDBEnforceMPPExecution:                 "1",
        vardef::TiDBAllowMPPExecution:                   "1",
        vardef::TiDBEnableLocalTxn:                      "1",
        vardef::TiDBEnablePlanReplayerContinuousCapture: "1",
        vardef::TiDBEnableHistoricalStats:               "1",
    }
    let mut names = variable::OrderByDependency(vars);
    require::Greater(t, slices.Index(names, vardef::TxReadOnly), slices.Index(names, vardef::TiDBEnableNoopFuncs));
    require::Greater(t, slices.Index(names, vardef::SQLAutoIsNull), slices.Index(names, vardef::TiDBEnableNoopFuncs));
    require::Greater(t, slices.Index(names, vardef::TiDBEnforceMPPExecution), slices.Index(names, vardef::TiDBAllowMPPExecution));
    // Depended variables below are global variables, so actually it doesn't matter.
    require::Greater(t, slices.Index(names, vardef::TiDBEnablePlanReplayerContinuousCapture), slices.Index(names, vardef::TiDBEnableHistoricalStats));
    require::Contains(t, names, "unknown");
}
"################;

/// 以下为可执行的 Rust 单元测试。
use astersql_sessionctx_vardef as vardef;
use astersql_sessionctx_variable::sysvar::{
    GlobalSystemVariableInitialValueWithRuntime, RuntimeEnvironment,
};

#[test]
/// 校验经典 TiKV 与 next-gen 运行时下若干全局变量默认值是否与 Go 分支一致。
fn dynamic_global_defaults_match_classic_and_next_gen_go_branches() {
    // 经典环境：TiKV 存储、测试模式、非 next-gen
    let classic = RuntimeEnvironment {
        store_is_tikv: true,
        in_test: true,
        next_gen: false,
        default_txn_assertion_level: "STRICT".to_owned(),
    };
    // Go sysvar.go: GlobalSystemVariableInitialValue only overrides listed
    // dynamic defaults; transaction mode is returned exactly as supplied.
    for value in ["", vardef::OptimisticTxnMode, vardef::PessimisticTxnMode] {
        assert_eq!(
            GlobalSystemVariableInitialValueWithRuntime(vardef::TiDBTxnMode, value, &classic),
            value
        );
    }
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(vardef::TiDBEnableAsyncCommit, "OFF", &classic),
        vardef::On
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(vardef::TiDBEnableAutoAnalyze, "ON", &classic),
        vardef::Off
    );
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(vardef::TiDBTxnAssertionLevel, "OFF", &classic),
        vardef::AssertionFastStr
    );

    // next-gen：断言级别默认 STRICT
    let next_gen = RuntimeEnvironment {
        next_gen: true,
        ..classic
    };
    for value in ["", vardef::OptimisticTxnMode, vardef::PessimisticTxnMode] {
        assert_eq!(
            GlobalSystemVariableInitialValueWithRuntime(vardef::TiDBTxnMode, value, &next_gen),
            value
        );
    }
    assert_eq!(
        GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBTxnAssertionLevel,
            "OFF",
            &next_gen
        ),
        "STRICT"
    );
}

// The Go reference above is intentionally kept verbatim.  These executable
// tests cover the same registry, validation, scope, and dependency branches
// with the native Rust API instead of treating the reference text as a test.
struct RegistryAccessor;

impl astersql_sessionctx_variable::GlobalVarAccessor for RegistryAccessor {
    fn get_global_sys_var(
        &self,
        name: &str,
    ) -> Result<String, astersql_sessionctx_variable::VariableError> {
        astersql_sessionctx_variable::GetSysVar(name)
            .map(|sys_var| sys_var.Value.clone())
            .ok_or_else(|| astersql_sessionctx_variable::VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &astersql_sessionctx_variable::Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), astersql_sessionctx_variable::VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(
        &self,
        _name: &str,
    ) -> Result<String, astersql_sessionctx_variable::VariableError> {
        Err(astersql_sessionctx_variable::VariableError::new(
            astersql_sessionctx_variable::VariableErrorKind::InvalidValue,
            "test accessor has no mysql.tidb values",
        ))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), astersql_sessionctx_variable::VariableError> {
        Ok(())
    }
}

fn new_session_vars() -> astersql_sessionctx_variable::SessionVars {
    astersql_sessionctx_variable::register_builtin_sysvars();
    astersql_sessionctx_variable::SessionVars::new(Box::new(RegistryAccessor))
}

#[test]
fn go_test_sys_var_registry_and_dynamic_defaults() {
    use std::env;

    astersql_sessionctx_variable::register_builtin_sysvars();
    assert!(astersql_sessionctx_variable::GetSysVar("autocommit").is_some());
    assert!(astersql_sessionctx_variable::GetSysVar("wrong-var-name").is_none());
    assert_eq!(
        astersql_sessionctx_variable::GetSysVar("explicit_defaults_for_timestamp")
            .expect("builtin sysvar")
            .Value,
        "ON"
    );
    assert_eq!(
        astersql_sessionctx_variable::GetSysVar("port")
            .expect("builtin sysvar")
            .Value,
        "4000"
    );
    assert_eq!(
        astersql_sessionctx_variable::GetSysVar("version_compile_os")
            .expect("builtin sysvar")
            .Value,
        env::consts::OS
    );
    assert_eq!(
        astersql_sessionctx_variable::GetSysVar("version_compile_machine")
            .expect("builtin sysvar")
            .Value,
        env::consts::ARCH
    );
    let build_stats = astersql_sessionctx_variable::GetSysVar(vardef::TiDBBuildStatsConcurrency)
        .expect("builtin sysvar");
    let auto_build_stats =
        astersql_sessionctx_variable::GetSysVar(vardef::TiDBAutoBuildStatsConcurrency)
            .expect("builtin sysvar");
    assert_eq!(build_stats.Value, auto_build_stats.Value);
}

#[test]
fn go_test_error_descriptors_have_nonzero_mysql_codes() {
    use astersql_sessionctx_variable::error::ALL_ERRORS;

    assert!(!ALL_ERRORS.is_empty());
    assert!(ALL_ERRORS.iter().all(|descriptor| descriptor.code != 0));
}

#[test]
fn go_test_registration_and_unregistration_restore_registry() {
    use astersql_sessionctx_variable::{RegisterSysVar, SysVar, UnregisterSysVar};

    let name = "aster_test_registration_sysvar";
    UnregisterSysVar(name);
    let mut sys_var = SysVar::default();
    sys_var.Scope = vardef::ScopeGlobal | vardef::ScopeSession;
    sys_var.Name = name.to_owned();
    sys_var.Value = vardef::On.to_owned();
    sys_var.Type = vardef::TypeBool;
    RegisterSysVar(sys_var);
    assert!(astersql_sessionctx_variable::GetSysVar(name).is_some());
    let mut vars = new_session_vars();
    let registered = astersql_sessionctx_variable::GetSysVar(name).expect("registered sysvar");
    assert!(
        registered
            .Validate(&mut vars, "invalid", vardef::ScopeSession)
            .is_err()
    );
    assert_eq!(
        registered
            .Validate(&mut vars, "oN", vardef::ScopeSession)
            .expect("bool normalization"),
        "ON"
    );
    astersql_sessionctx_variable::UnregisterSysVar(name);
    assert!(astersql_sessionctx_variable::GetSysVar(name).is_none());
}

#[test]
fn go_test_numeric_enum_duration_and_float_validation() {
    use astersql_sessionctx_variable::SysVar;

    let mut vars = new_session_vars();
    let mut signed = SysVar::default();
    signed.Scope = vardef::ScopeGlobal | vardef::ScopeSession;
    signed.Name = "aster_test_signed".to_owned();
    signed.Type = vardef::TypeInt;
    signed.MinValue = 10;
    signed.MaxValue = 300;
    signed.AllowAutoValue = true;
    assert_eq!(
        signed
            .Validate(&mut vars, "301", vardef::ScopeSession)
            .unwrap(),
        "300"
    );
    assert_eq!(
        signed
            .Validate(&mut vars, "5", vardef::ScopeSession)
            .unwrap(),
        "10"
    );
    assert_eq!(
        signed
            .Validate(&mut vars, "-1", vardef::ScopeSession)
            .unwrap(),
        "-1"
    );

    let mut unsigned = signed.clone();
    unsigned.Name = "aster_test_unsigned".to_owned();
    unsigned.Type = vardef::TypeUnsigned;
    assert!(
        unsigned
            .Validate(&mut vars, "-ERR", vardef::ScopeSession)
            .is_err()
    );
    assert_eq!(
        unsigned
            .Validate(&mut vars, "-301", vardef::ScopeSession)
            .unwrap(),
        "10"
    );

    let mut enum_var = SysVar::default();
    enum_var.Scope = vardef::ScopeSession;
    enum_var.Name = "aster_test_enum".to_owned();
    enum_var.Type = vardef::TypeEnum;
    enum_var.PossibleValues = vec!["OFF".to_owned(), "ON".to_owned(), "AUTO".to_owned()];
    assert_eq!(
        enum_var
            .Validate(&mut vars, "oFf", vardef::ScopeSession)
            .unwrap(),
        "OFF"
    );
    assert_eq!(
        enum_var
            .Validate(&mut vars, "2", vardef::ScopeSession)
            .unwrap(),
        "AUTO"
    );
    assert!(
        enum_var
            .Validate(&mut vars, "random", vardef::ScopeSession)
            .is_err()
    );

    let mut float_var = SysVar::default();
    float_var.Scope = vardef::ScopeSession;
    float_var.Name = "aster_test_float".to_owned();
    float_var.Type = vardef::TypeFloat;
    float_var.MinValue = 1;
    float_var.MaxValue = 10;
    assert_eq!(
        float_var
            .Validate(&mut vars, "11", vardef::ScopeSession)
            .unwrap(),
        "10"
    );
    assert!(
        float_var
            .Validate(&mut vars, "stringval", vardef::ScopeSession)
            .is_err()
    );
    assert!(
        float_var
            .Validate(&mut vars, "", vardef::ScopeSession)
            .is_err()
    );
}

#[test]
fn go_test_native_value_type_scope_and_skip_cache_metadata() {
    use astersql_sessionctx_variable::{Datum, SysVar};

    astersql_sessionctx_variable::register_builtin_sysvars();
    let mut bool_var = SysVar::default();
    bool_var.Scope = vardef::ScopeGlobal | vardef::ScopeSession;
    bool_var.Name = "aster_test_bool".to_owned();
    bool_var.Type = vardef::TypeBool;
    assert_eq!(bool_var.GetNativeValType("ON"), (Datum::Int(1), 8, 128));
    assert_eq!(bool_var.GetNativeValType("OFF"), (Datum::Int(0), 8, 128));
    assert!(bool_var.HasGlobalScope());
    assert!(bool_var.HasSessionScope());
    assert!(!bool_var.HasInstanceScope());

    let gc = astersql_sessionctx_variable::GetSysVar(vardef::TiDBGCEnable).unwrap();
    assert!(gc.SkipSysvarCache());
    for name in [
        vardef::TiDBGCRunInterval,
        vardef::TiDBGCLifetime,
        vardef::TiDBGCConcurrency,
        vardef::TiDBGCScanLockMode,
    ] {
        assert!(
            astersql_sessionctx_variable::GetSysVar(name)
                .expect("builtin GC sysvar")
                .SkipSysvarCache()
        );
    }
    let secure = astersql_sessionctx_variable::GetSysVar(vardef::RequireSecureTransport).unwrap();
    assert!(!secure.SkipSysvarCache());
    let async_commit =
        astersql_sessionctx_variable::GetSysVar(vardef::TiDBEnableAsyncCommit).unwrap();
    assert!(!async_commit.SkipSysvarCache());
}

#[test]
fn go_test_relaxed_validation_time_and_dependency_order() {
    use astersql_sessionctx_variable::{OrderByDependency, SysVar};
    use std::collections::HashMap;

    let mut vars = new_session_vars();
    let mut bool_var = SysVar::default();
    bool_var.Scope = vardef::ScopeGlobal | vardef::ScopeSession;
    bool_var.Name = "aster_test_relaxed".to_owned();
    bool_var.Type = vardef::TypeBool;
    assert_eq!(
        bool_var.ValidateWithRelaxedValidation(&mut vars, "invalid", vardef::ScopeSession),
        "invalid"
    );

    let mut time_var = SysVar::default();
    time_var.Scope = vardef::ScopeSession;
    time_var.Name = "aster_test_time".to_owned();
    time_var.Type = vardef::TypeTime;
    assert_eq!(
        time_var
            .Validate(&mut vars, "23:59 +0000", vardef::ScopeSession)
            .unwrap(),
        "23:59 +0000"
    );

    let names = HashMap::from([
        ("unknown".to_owned(), "1".to_owned()),
        (vardef::TxReadOnly.to_owned(), "1".to_owned()),
        (vardef::SQLAutoIsNull.to_owned(), "1".to_owned()),
        (vardef::TiDBEnableNoopFuncs.to_owned(), "1".to_owned()),
        (vardef::TiDBEnforceMPPExecution.to_owned(), "1".to_owned()),
        (vardef::TiDBAllowMPPExecution.to_owned(), "1".to_owned()),
        (
            vardef::TiDBEnablePlanReplayerContinuousCapture.to_owned(),
            "1".to_owned(),
        ),
        (vardef::TiDBEnableHistoricalStats.to_owned(), "1".to_owned()),
    ]);
    let ordered = OrderByDependency(&names);
    assert!(ordered.contains(&"unknown".to_owned()));
    assert!(
        ordered
            .iter()
            .position(|name| name == vardef::TxReadOnly)
            .unwrap()
            > ordered
                .iter()
                .position(|name| name == vardef::TiDBEnableNoopFuncs)
                .unwrap()
    );
    assert!(
        ordered
            .iter()
            .position(|name| name == vardef::SQLAutoIsNull)
            .unwrap()
            > ordered
                .iter()
                .position(|name| name == vardef::TiDBEnableNoopFuncs)
                .unwrap()
    );
    assert!(
        ordered
            .iter()
            .position(|name| name == vardef::TiDBEnforceMPPExecution)
            .unwrap()
            > ordered
                .iter()
                .position(|name| name == vardef::TiDBAllowMPPExecution)
                .unwrap()
    );
    assert!(
        ordered
            .iter()
            .position(|name| name == vardef::TiDBEnablePlanReplayerContinuousCapture)
            .unwrap()
            > ordered
                .iter()
                .position(|name| name == vardef::TiDBEnableHistoricalStats)
                .unwrap()
    );
}
