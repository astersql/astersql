// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 非线程安全签名清单的 Aster 单元测试。
//
// 核对生成表完整顺序、`IsGeneratedThreadunsafeSignature` 分类结果，
// 以及签名数量与去重后长度一致（当前 88 个）。

use crate::builtin_threadunsafe_generated_kernel::*;

#[test]
/// 核对非线程安全签名表顺序、分类与去重计数。
fn every_generated_thread_unsafe_signature_is_classified_as_false() {
    macro_rules! check_signatures {
        ($($signature:ident),+ $(,)?) => {
            let expected = [$(stringify!($signature)),+];
            assert_eq!(GENERATED_THREADUNSAFE_SIGNATURES, expected);
            assert!(expected
                .iter()
                .all(|signature| IsGeneratedThreadunsafeSignature(signature)));
        };
    }

    check_signatures!(
        builtinArithmeticMultiplyRealSig,
        builtinGreatestCmpStringAsTimeSig,
        builtinGreatestTimeSig,
        builtinLeastCmpStringAsTimeSig,
        builtinLeastTimeSig,
        builtinIntervalIntSig,
        builtinIntervalRealSig,
        builtinInternalFromBinarySig,
        builtinAesDecryptSig,
        builtinAesDecryptIVSig,
        builtinAesEncryptSig,
        builtinAesEncryptIVSig,
        builtinValidatePasswordStrengthSig,
        builtinFtsMysqlMatchAgainstSig,
        builtinIlikeSig,
        builtinFoundRowsSig,
        builtinCurrentUserSig,
        builtinCurrentRoleSig,
        builtinCurrentResourceGroupSig,
        builtinUserSig,
        builtinConnectionIDSig,
        builtinLastInsertIDSig,
        builtinLastInsertIDWithIDSig,
        builtinTiDBIsDDLOwnerSig,
        builtinBenchmarkSig,
        builtinRowCountSig,
        builtinTiDBMVCCInfoSig,
        builtinTiDBEncodeRecordKeySig,
        builtinTiDBEncodeIndexKeySig,
        builtinTiDBDecodeKeySig,
        builtinTiDBDecodeSQLDigestsSig,
        builtinNextValSig,
        builtinLastValSig,
        builtinSetValSig,
        builtinJSONSchemaValidSig,
        builtinLikeSig,
        builtinRandSig,
        builtinSleepSig,
        builtinLockSig,
        builtinReleaseLockSig,
        builtinFreeLockSig,
        builtinUsedLockSig,
        builtinReleaseAllLocksSig,
        builtinVectorFloat32IsTrueSig,
        builtinVectorFloat32IsFalseSig,
        builtinUnaryMinusDecimalSig,
        builtinSetStringVarSig,
        builtinSetRealVarSig,
        builtinSetDecimalVarSig,
        builtinSetIntVarSig,
        builtinSetTimeVarSig,
        builtinValuesIntSig,
        builtinValuesRealSig,
        builtinValuesDecimalSig,
        builtinValuesStringSig,
        builtinValuesTimeSig,
        builtinValuesDurationSig,
        builtinValuesJSONSig,
        builtinValuesVectorFloat32Sig,
        builtinRegexpLikeFuncSig,
        builtinRegexpSubstrFuncSig,
        builtinRegexpInStrFuncSig,
        builtinRegexpReplaceFuncSig,
        builtinConcatSig,
        builtinConcatWSSig,
        builtinRepeatSig,
        builtinSpaceSig,
        builtinLpadSig,
        builtinLpadUTF8Sig,
        builtinRpadSig,
        builtinRpadUTF8Sig,
        builtinFindInSetSig,
        builtinFromBase64Sig,
        builtinToBase64Sig,
        builtinInsertSig,
        builtinInsertUTF8Sig,
        builtinWeightStringSig,
        builtinDateLiteralSig,
        builtinTimeLiteralSig,
        builtinAddSubDateAsStringSig,
        builtinAddSubDateDatetimeAnySig,
        builtinAddSubDateDurationAnySig,
        builtinTimestamp1ArgSig,
        builtinTimestamp2ArgsSig,
        builtinTimestampLiteralSig,
        builtinConvertTzSig,
        builtinTiDBBoundedStalenessSig,
        builtinTiDBCurrentTsoSig,
    );

    assert_eq!(GENERATED_SIGNATURE_COUNT, 88);
    let unique = GENERATED_THREADUNSAFE_SIGNATURES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), GENERATED_SIGNATURE_COUNT);
}
