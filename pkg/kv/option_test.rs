// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 事务来源位图（TxnSource）相关选项的单元测试。
//
// 覆盖 TiCDC 写来源（CDC write source）与有损 DDL 重组来源（lossy DDL reorg source）
// 的设置、读取与越界报错边界，对应 Go `option_test.go`。

use kv_dependency as kv;

/// 验证 `SetCDCWriteSource` 合法值写入与越界错误文案。
#[test]
fn test_set_cdc_write_source() {
    // (source, expected_set, expected_source, expected_error)
    let cases = [
        (1, true, 1, false),
        (0, false, 0, false),
        (16, false, 0, true),
    ];
    for (source, expected_set, expected_source, expected_error) in cases {
        let mut option = 0;
        let result = kv::SetCDCWriteSource(&mut option, source);
        if expected_error {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("out of TiCDC write source range")
            );
        } else {
            result.unwrap();
            assert_eq!(expected_set, kv::IsCDCWriteSourceSet(option));
            assert_eq!(expected_source, kv::GetCDCWriteSource(option));
        }
    }
}

/// 验证 `SetLossyDDLReorgSource` 在已有位图上叠加、清零与越界行为。
#[test]
fn test_set_lossy_ddl_reorg_source() {
    // (current, source, expected_set, expected_source, expected_error)
    let cases = [
        (0, 1, true, 1, false),
        (12, 1, true, 1, false),
        (12, 0, false, 0, false),
        (12, 256, false, 0, true),
    ];
    for (mut current, source, expected_set, expected_source, expected_error) in cases {
        let result = kv::SetLossyDDLReorgSource(&mut current, source);
        if expected_error {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("out of lossy DDL reorg source range")
            );
        } else {
            result.unwrap();
            assert_eq!(expected_set, kv::IsLossyDDLReorgSourceSet(current));
            assert_eq!(expected_source, kv::GetLossyDDLReorgSource(current));
        }
    }
}
