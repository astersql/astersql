// Copyright 2015 PingCAP, Inc.
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

// DECIMAL 编解码单元测试：覆盖 EncodeDecimal/DecodeDecimal 往返与 frac 保留。
//
// 对应 Go `pkg/util/codec/decimal_test.go`，用表驱动用例验证 float 构造的
// MyDecimal 经编码解码后 Compare 为 0，以及小数位字符串表示一致。

// 本文件由 pkg/util/codec/decimal_test.go 迁移而来，保留 Go 测试结构。
//

use super::*;

/// 表驱动夹具：单字段 Input 为待编码的 float64。
// decimalCodecCase 对应 Go 表驱动夹具，只有 Input 一个 float64 字段。
struct decimalCodecCase {
    Input: f64,
}

/// 从 float 构造 decimal，编码后再解码，校验精度/小数位与 Compare 结果。
// TestDecimalCodec 对应 Go 的 decimal 从 float 构造、EncodeDecimal、DecodeDecimal 和 Compare 往返测试。
#[test]
fn TestDecimalCodec() {
    let inputs = vec![
        decimalCodecCase { Input: 123400.0 },
        decimalCodecCase { Input: 1234.0 },
        decimalCodecCase { Input: 12.34 },
        decimalCodecCase { Input: 0.1234 },
        decimalCodecCase { Input: 0.01234 },
        decimalCodecCase { Input: -0.1234 },
        decimalCodecCase { Input: -0.01234 },
        decimalCodecCase { Input: 12.3400 },
        decimalCodecCase { Input: -12.34 },
        decimalCodecCase { Input: 0.00000 },
        decimalCodecCase { Input: 0.0 },
        decimalCodecCase { Input: -0.0 },
        decimalCodecCase { Input: -0.000 },
    ];

    for input in inputs {
        let v = types::NewDecFromFloatForTest(input.Input);
        let datum = types::NewDecimalDatum(v.clone());
        let b = EncodeDecimal(
            Vec::new(),
            &datum.GetMysqlDecimal(),
            datum.Length(),
            datum.Frac(),
        )
        .expect("decimal must encode");
        let (remain, d, prec, frac) = DecodeDecimal(&b).expect("decimal must decode");
        assert!(remain.is_empty());
        if datum.Length() != 0 {
            assert_eq!(datum.Length(), prec);
            assert_eq!(datum.Frac(), frac);
        } else {
            // Go 在未设置 Length 时改用 decimal 自身 PrecisionAndFrac，保持同一条兜底语义。
            let (prec1, frac1) = datum.GetMysqlDecimal().PrecisionAndFrac();
            assert_eq!(prec1 as i32, prec);
            assert_eq!(frac1 as i32, frac);
        }
        assert_eq!(0, v.Compare(&d));
    }
}

/// 校验 frac 在编码往返后仍能还原为相同字符串表示（含 0.03 等小数）。
// TestFrac 对应 Go 的 frac 保留测试，覆盖整数 decimal 和 0.03 这类需要小数位的值。
#[test]
fn TestFrac() {
    let inputs = vec![types::NewDecFromInt(3), types::NewDecFromFloatForTest(0.03)];
    for input in inputs {
        let mut datum = types::Datum::default();
        datum.SetMysqlDecimal(input.clone());
        let b = EncodeDecimal(
            Vec::new(),
            &datum.GetMysqlDecimal(),
            datum.Length(),
            datum.Frac(),
        )
        .expect("decimal must encode");
        let (remain, dec, _, _) = DecodeDecimal(&b).expect("decimal must decode");
        assert!(remain.is_empty());
        assert_eq!(input.String(), dec.String());
    }
}
