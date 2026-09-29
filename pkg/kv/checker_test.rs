// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 请求类型白名单检查器的单元测试（对应 Go `checker_test.go`）。
//
// 验证 Select/DAG/Analyze 支持的子类型与聚合表达式，以及 Checksum 等不受支持的请求。

use kv_dependency as kv;

/// 覆盖 Go 白名单中的典型正例与负例。
#[test]
fn test_is_request_type_supported() {
    let checker = kv::RequestTypeSupportedChecker;
    // Select + GroupBy 子类型应支持。
    assert!(checker.IsRequestTypeSupported(kv::ReqTypeSelect, kv::ReqSubTypeGroupBy));
    // DAG + Signature / Desc 子类型应支持。
    assert!(checker.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeSignature));
    assert!(checker.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeDesc));
    assert!(checker.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeSignature));
    // Select + SumInt 聚合表达式应支持。
    assert!(checker.IsRequestTypeSupported(kv::ReqTypeSelect, kv::ExprType::SumInt as i64));
    // DAG + AnalyzeIdx 不在白名单。
    assert!(!checker.IsRequestTypeSupported(kv::ReqTypeDAG, kv::ReqSubTypeAnalyzeIdx));
    // Analyze 请求整体放行；Checksum 不支持。
    assert!(checker.IsRequestTypeSupported(kv::ReqTypeAnalyze, 0));
    assert!(!checker.IsRequestTypeSupported(kv::ReqTypeChecksum, 0));
}

#[test]
fn go_merge_4_max_min_count_are_supported() {
    let checker = kv::RequestTypeSupportedChecker;
    for req in [kv::ReqTypeSelect, kv::ReqTypeIndex, kv::ReqTypeDAG] {
        assert!(checker.IsRequestTypeSupported(req, 3022));
        assert!(checker.IsRequestTypeSupported(req, 3023));
    }
}

/// Go converts subType to tipb.ExprType (int32), after request-specific checks.
#[test]
fn test_expr_type_conversion_matches_go() {
    let checker = kv::RequestTypeSupportedChecker;
    let supported = [
        0, 1, 2, 3, 4, 5, 6, 101, 102, 103, 104, 107, 121, 201, 3001, 3002, 3003, 3004, 3005, 3006,
        3007, 3008, 3009, 3010, 3020, 3021, 3022, 3023, 4001, 4002, 4003, 4004, 4005, 4006, 4007,
        4008, 4009, 4010, 4011, 10000, 10003,
    ];
    for req in [kv::ReqTypeSelect, kv::ReqTypeIndex, kv::ReqTypeDAG] {
        for offset in [0, 1_i64 << 32, -(1_i64 << 32), i64::MIN] {
            for subtype in supported {
                assert!(
                    checker.IsRequestTypeSupported(req, offset + subtype),
                    "request={req}, subtype={}",
                    offset + subtype
                );
            }
            for subtype in [
                7, 100, 105, 106, 108, 120, 122, 202, 3000, 3011, 3019, 3024, 4000, 4012, 10001,
                10002, 10004, 10005,
            ] {
                let input = offset + subtype;
                let expected = req != kv::ReqTypeDAG
                    && offset == 0
                    && [kv::ReqSubTypeGroupBy, kv::ReqSubTypeTopN].contains(&subtype);
                assert_eq!(
                    checker.IsRequestTypeSupported(req, input),
                    expected,
                    "request={req}, subtype={input}"
                );
            }
        }
        for subtype in [i64::MAX, -1, i32::MIN as i64, i32::MAX as i64] {
            assert!(!checker.IsRequestTypeSupported(req, subtype));
        }
    }
    for subtype in [i64::MIN, -1, 0, 10001, i64::MAX] {
        assert!(checker.IsRequestTypeSupported(kv::ReqTypeAnalyze, subtype));
        for req in [
            i64::MIN,
            -1,
            0,
            kv::ReqTypeChecksum,
            i64::MAX,
            (1_i64 << 32) + kv::ReqTypeDAG,
        ] {
            assert!(!checker.IsRequestTypeSupported(req, subtype));
        }
    }
}
