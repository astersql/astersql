// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// MyDecimal 完整表驱动测试，对齐 Go `mydecimal_test.go`。
//
// 覆盖整数/浮点转换、hash/bin 编码、舍入、比较、四则运算、移位、解析与 JSON 编解码；
// 错误码与 require/strconv 辅助模块保留 Go 测试语义。

// 这段逻辑覆盖 MyDecimal 的整数/浮点转换、hash/bin 编码、舍入、比较、四则运算、移位、解析和 JSON 编解码。
// MyDecimal、ErrOverflow、ErrTruncated、json、strconv、strings 和 require 均保留 Go 测试语义。

use crate::decimal::mydecimal::*;

/// Go 测试里 `error` 的草稿别名：Ok 表示无错。
type DraftError = Result<(), DecimalError>;
/// 无错误（对应 Go nil error）。
const nil: DraftError = Ok(());
/// 溢出期望。
const ErrOverflow: DraftError = Err(DecimalError::Overflow);
/// 截断期望。
const ErrTruncated: DraftError = Err(DecimalError::Truncated);
/// 非法数字期望。
const ErrBadNumber: DraftError = Err(DecimalError::BadNumber);
/// 除零期望。
const ErrDivByZero: DraftError = Err(DecimalError::DivByZero);

/// ToString 字节转 UTF-8 文本。
fn string(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

/// 轻量断言辅助，模拟 Go testify/require。
mod require {
    use std::fmt::Debug;

    /// 断言期望与实际相等。
    pub fn Equal<E, A>(expected: E, actual: A)
    where
        E: Debug + PartialEq<A>,
        A: Debug,
    {
        assert!(expected == actual, "expected {expected:?}, got {actual:?}");
    }

    /// 断言 Result 为 Ok。
    pub fn NoError<T, E: Debug>(result: Result<T, E>) {
        result.unwrap();
    }
}

/// 模拟 Go strconv.ParseFloat。
mod strconv {
    /// 解析浮点；失败时返回 (0.0, Err)。
    pub fn ParseFloat(value: &str, _bits: u32) -> (f64, Result<(), std::num::ParseFloatError>) {
        match value.parse::<f64>() {
            Ok(value) => (value, Ok(())),
            Err(error) => (0.0, Err(error)),
        }
    }
}

/// 从 int64 构造 MyDecimal。
// TestFromInt 对应 Go 的 int64 到 MyDecimal 构造测试，覆盖负数、普通正数和 int64 下界。
#[test]
fn TestFromInt() {
    let tests = vec![
        (-12345_i64, "-12345"),
        (-1, "-1"),
        (1, "1"),
        (-9223372036854775807, "-9223372036854775807"),
        (-9223372036854775808, "-9223372036854775808"),
    ];
    for (input, output) in tests {
        let dec = NewDecFromInt(input);
        let str = dec.ToString();
        require::Equal(output, string(str));
    }
}

/// 从 uint64 构造 MyDecimal。
// TestFromUint 对应 Go 的 uint64 到 MyDecimal 构造测试，包含 ULLONG_MAX。
#[test]
fn TestFromUint() {
    let tests = vec![
        (12345_u64, "12345"),
        (0, "0"),
        (18446744073709551615, "18446744073709551615"),
    ];
    for (input, output) in tests {
        let mut dec = MyDecimal::default();
        dec.FromUint(input);
        require::Equal(output, string(dec.ToString()));
    }
}

/// ToInt 截断与溢出。
// TestToInt 对应 Go 的 ToInt 测试，保留截断和溢出错误的期望。
#[test]
fn TestToInt() {
    let tests = vec![
        IntConvCase {
            input: "18446744073709551615",
            output: 9223372036854775807,
            err: ErrOverflow,
        },
        IntConvCase {
            input: "-1",
            output: -1,
            err: nil,
        },
        IntConvCase {
            input: "1",
            output: 1,
            err: nil,
        },
        IntConvCase {
            input: "-1.23",
            output: -1,
            err: ErrTruncated,
        },
        IntConvCase {
            input: "-9223372036854775807",
            output: -9223372036854775807,
            err: nil,
        },
        IntConvCase {
            input: "-9223372036854775808",
            output: -9223372036854775808,
            err: nil,
        },
        IntConvCase {
            input: "9223372036854775808",
            output: 9223372036854775807,
            err: ErrOverflow,
        },
        IntConvCase {
            input: "-9223372036854775809",
            output: -9223372036854775808,
            err: ErrOverflow,
        },
    ];
    for tt in tests {
        let mut dec = MyDecimal::default();
        require::NoError(dec.FromString(tt.input.as_bytes()));
        let (result, ec) = dec.ToInt();
        require::Equal(tt.err, ec);
        require::Equal(tt.output, result);
    }
}

/// ToUint 负数/截断/上界。
// TestToUint 对应 Go 的 ToUint 测试，覆盖负数、截断和 uint64 上界溢出。
#[test]
fn TestToUint() {
    let tests = vec![
        UintConvCase {
            input: "12345",
            output: 12345,
            err: nil,
        },
        UintConvCase {
            input: "0",
            output: 0,
            err: nil,
        },
        UintConvCase {
            input: "18446744073709551615",
            output: 18446744073709551615,
            err: nil,
        },
        UintConvCase {
            input: "18446744073709551616",
            output: 18446744073709551615,
            err: ErrOverflow,
        },
        UintConvCase {
            input: "-1",
            output: 0,
            err: ErrOverflow,
        },
        UintConvCase {
            input: "1.23",
            output: 1,
            err: ErrTruncated,
        },
        UintConvCase {
            input: "9999999999999999999999999.000",
            output: 18446744073709551615,
            err: ErrOverflow,
        },
    ];
    for tt in tests {
        let mut dec = MyDecimal::default();
        require::NoError(dec.FromString(tt.input.as_bytes()));
        let (result, ec) = dec.ToUint();
        require::Equal(tt.err, ec);
        require::Equal(tt.output, result);
    }
}

/// FromFloat64 / NewDecFromFloatForTest。
// TestFromFloat 对应 Go 的 FromFloat64 测试；NewDecFromFloatForTest 在失败时会 panic。
#[test]
fn TestFromFloat() {
    for (s, f) in vec![
        ("12345", 12345.0),
        ("123.45", 123.45),
        ("-123.45", -123.45),
        ("0.00012345000098765", 0.00012345000098765),
        ("1234500009876.5", 1234500009876.5),
    ] {
        let dec = NewDecFromFloatForTest(f);
        require::Equal(s, string(dec.ToString()));
    }
}

/// ToFloat64 与科学计数法样本。
// TestToFloat 对应 Go 的 ToFloat64 测试，混合普通十进制、科学计数法和 strconv atof 边界样本。
#[test]
fn TestToFloat() {
    let tests = vec![
        ("12345", "12345"),
        ("123.45", "123.45"),
        ("-123.45", "-123.45"),
        ("0.00012345000098765", "0.00012345000098765"),
        ("1234500009876.5", "1234500009876.5"),
        ("1e39", "1e39"),
        ("1e-39", "1e-39"),
        ("1e00", "1"),
        ("1e001", "10"),
        ("-9223372036854775807", "-9223372036854775807"),
        ("-9223372036854775808", "-9223372036854775808"),
        ("18446744073709551615", "18446744073709551615"),
        ("123456789.987654321", "123456789.987654321"),
        ("1", "1"),
        ("+1", "1"),
        ("1e23", "1e+23"),
        ("1E23", "1e+23"),
        ("100000000000000000000000", "1e+23"),
        ("123456700", "1.234567e+08"),
        ("99999999999999974834176", "9.999999999999997e+22"),
        ("100000000000000000000001", "1.0000000000000001e+23"),
        ("100000000000000008388608", "1.0000000000000001e+23"),
        ("100000000000000016777215", "1.0000000000000001e+23"),
        ("100000000000000016777216", "1.0000000000000003e+23"),
        ("-1", "-1"),
        ("-0.1", "-0.1"),
        ("-0", "-0"),
        ("1e-20", "1e-20"),
        ("625e-3", "0.625"),
        ("0", "0"),
        ("22.222222222222222", "22.22222222222222"),
        (
            "1.00000000000000011102230246251565404236316680908203125",
            "1",
        ),
        (
            "1.00000000000000011102230246251565404236316680908203124",
            "1",
        ),
        (
            "1.00000000000000011102230246251565404236316680908203126",
            "1.0000000000000002",
        ),
        (
            "1.00000000000000033306690738754696212708950042724609375",
            "1.0000000000000004",
        ),
        ("1090544144181609348671888949248", "1.0905441441816093e+30"),
        ("1090544144181609348835077142190", "1.0905441441816094e+30"),
    ];
    for (input, out) in tests {
        let mut dec = MyDecimal::default();
        require::NoError(dec.FromString(input.as_bytes()));
        let f = dec.ToFloat64().unwrap();
        let (std, err) = strconv::ParseFloat(out, 64);
        require::NoError(err);
        require::Equal(std, f);
    }
}

/// 不同写法同一数值应得到同一 hash key。
// TestToHashKey 对应 Go 的 hash/bin 等价测试：同一数值的不同写法应得到同一 key。
#[test]
fn TestToHashKey() {
    let tests = vec![
        vec![
            "1.1",
            "1.1000",
            "1.1000000",
            "1.10000000000",
            "01.1",
            "0001.1",
            "001.1000000",
        ],
        vec![
            "-1.1",
            "-1.1000",
            "-1.1000000",
            "-1.10000000000",
            "-01.1",
            "-0001.1",
            "-001.1000000",
        ],
        vec![
            ".1",
            "0.1",
            "0.10",
            "000000.1",
            ".10000",
            "0000.10000",
            "000000000000000000.1",
        ],
        vec![
            "0",
            "0000",
            ".0",
            ".00000",
            "00000.00000",
            "-0",
            "-0000",
            "-.0",
            "-.00000",
            "-00000.00000",
        ],
        vec![
            ".123456789123456789",
            ".1234567891234567890",
            ".12345678912345678900",
            ".123456789123456789000",
            ".1234567891234567890000",
            "0.123456789123456789",
            ".1234567891234567890000000000",
            "0000000.123456789123456789000",
        ],
        vec![
            "12345",
            "012345",
            "0012345",
            "0000012345",
            "0000000012345",
            "00000000000012345",
            "12345.",
            "12345.00",
            "12345.000000000",
            "000012345.0000",
        ],
        vec![
            "123E5",
            "12300000",
            "00123E5",
            "000000123E5",
            "12300000.00000000",
        ],
        vec![
            "123E-2",
            "1.23",
            "00000001.23",
            "1.2300000000000000",
            "000000001.23000000000000",
        ],
    ];
    for numbers in tests {
        let mut keys = Vec::with_capacity(numbers.len());
        for num in numbers {
            let mut dec = MyDecimal::default();
            require::NoError(dec.FromString(num.as_bytes()));
            let key = dec.ToHashKey().unwrap();
            keys.push(key);
        }
        for i in 1..keys.len() {
            require::Equal(keys[0].clone(), keys[i].clone());
        }
    }

    // Go 还会比较去掉 hash digit len 后的 hash key 与 ToBin 输出，确保二进制序列兼容。
    let binTests = vec![
        HashBinCase {
            hashNumbers: vec![
                "1.1",
                "1.1000",
                "1.1000000",
                "1.10000000000",
                "01.1",
                "0001.1",
                "001.1000000",
            ],
            binNumbers: vec!["1.1", "0001.1", "01.1"],
        },
        HashBinCase {
            hashNumbers: vec![
                "-1.1",
                "-1.1000",
                "-1.1000000",
                "-1.10000000000",
                "-01.1",
                "-0001.1",
                "-001.1000000",
            ],
            binNumbers: vec!["-1.1", "-0001.1", "-01.1"],
        },
        HashBinCase {
            hashNumbers: vec![
                ".1",
                "0.1",
                "000000.1",
                ".10000",
                "0000.10000",
                "000000000000000000.1",
            ],
            binNumbers: vec![".1", "0.1", "000000.1", "00.1"],
        },
        HashBinCase {
            hashNumbers: vec![
                "0",
                "0000",
                ".0",
                ".00000",
                "00000.00000",
                "-0",
                "-0000",
                "-.0",
                "-.00000",
                "-00000.00000",
            ],
            binNumbers: vec!["0", "0000", "00", "-0", "-00", "-000000"],
        },
        HashBinCase {
            hashNumbers: vec![
                ".123456789123456789",
                ".1234567891234567890",
                ".12345678912345678900",
                ".123456789123456789000",
                ".1234567891234567890000",
                "0.123456789123456789",
                ".1234567891234567890000000000",
                "0000000.123456789123456789000",
            ],
            binNumbers: vec![
                ".123456789123456789",
                "0.123456789123456789",
                "0000.123456789123456789",
                "0000000.123456789123456789",
            ],
        },
        HashBinCase {
            hashNumbers: vec![
                "12345",
                "012345",
                "0012345",
                "0000012345",
                "0000000012345",
                "00000000000012345",
                "12345.",
                "12345.00",
                "12345.000000000",
                "000012345.0000",
            ],
            binNumbers: vec!["12345", "012345", "000012345", "000000000000012345"],
        },
        HashBinCase {
            hashNumbers: vec![
                "123E5",
                "12300000",
                "00123E5",
                "000000123E5",
                "12300000.00000000",
            ],
            binNumbers: vec!["12300000", "123E5", "00123E5", "0000000000123E5"],
        },
        HashBinCase {
            hashNumbers: vec![
                "123E-2",
                "1.23",
                "00000001.23",
                "1.2300000000000000",
                "000000001.23000000000000",
            ],
            binNumbers: vec!["123E-2", "1.23", "000001.23", "0000000000001.23"],
        },
    ];
    for ca in binTests {
        let mut keys = Vec::with_capacity(ca.hashNumbers.len() + ca.binNumbers.len());
        for num in ca.hashNumbers {
            let mut dec = MyDecimal::default();
            require::NoError(dec.FromString(num.as_bytes()));
            let mut key = dec.ToHashKey().unwrap();
            // Go 去掉最后一个 digit len 字节后再和 ToBin 结果比较。
            key = key[..key.len() - 1].to_vec();
            keys.push(key);
        }
        for num in ca.binNumbers {
            let mut dec = MyDecimal::default();
            require::NoError(dec.FromString(num.as_bytes()));
            let (prec, frac) = dec.PrecisionAndFrac();
            let (key, err) = dec.ToBin(prec as isize, frac as isize);
            require::NoError(err);
            keys.push(key);
        }
        for i in 1..keys.len() {
            require::Equal(keys[0].clone(), keys[i].clone());
        }
    }
}

/// 去掉尾随零后的小数位期望。
// TestRemoveTrailingZeros 对应 Go 的 removeTrailingZeros 测试，期望值由 ToString 后的小数尾零扫描得到。
#[test]
fn TestRemoveTrailingZeros() {
    let tests = vec![
        "0",
        "0.0",
        ".0",
        ".00000000",
        "0.0000",
        "0000",
        "0000.0",
        "0000.000",
        "-0",
        "-0.0",
        "-.0",
        "-.00000000",
        "-0.0000",
        "-0000",
        "-0000.0",
        "-0000.000",
        "123123123",
        "213123.",
        "21312.000",
        "21321.123",
        "213.1230000",
        "213123.000123000",
        "-123123123",
        "-213123.",
        "-21312.000",
        "-21321.123",
        "-213.1230000",
        "-213123.000123000",
        "123E5",
        "12300E-5",
        "0.00100E1",
        "0.001230E-3",
        "123987654321.123456789000",
        "000000000123",
        "123456789.987654321",
        "999.999000",
    ];
    for ca in tests {
        let mut dec = MyDecimal::default();
        require::NoError(dec.FromString(ca.as_bytes()));
        let rendered = string(dec.ToString());
        let normalized = if let Some((integer, fraction)) = rendered.split_once('.') {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                integer.to_owned()
            } else {
                format!("{integer}.{fraction}")
            }
        } else {
            rendered.clone()
        };
        let mut normalized_dec = MyDecimal::default();
        require::NoError(normalized_dec.FromString(normalized.as_bytes()));
        require::Equal(
            dec.ToHashKey().unwrap(),
            normalized_dec.ToHashKey().unwrap(),
        );
    }
}

/// HalfUp 舍入（函数名保留 Go HalfEven）。
// TestRoundWithHalfEven 保留 Go 文件中的函数名；实际调用的模式是 ModeHalfUp。
#[test]
fn TestRoundWithHalfEven() {
    run_round_cases(
        ModeHalfUp,
        vec![
            RoundCase {
                input: "123456789.987654321",
                scale: 1,
                output: "123456790.0",
                err: nil,
            },
            RoundCase {
                input: "15.1",
                scale: 0,
                output: "15",
                err: nil,
            },
            RoundCase {
                input: "15.5",
                scale: 0,
                output: "16",
                err: nil,
            },
            RoundCase {
                input: "15.9",
                scale: 0,
                output: "16",
                err: nil,
            },
            RoundCase {
                input: "-15.1",
                scale: 0,
                output: "-15",
                err: nil,
            },
            RoundCase {
                input: "-15.5",
                scale: 0,
                output: "-16",
                err: nil,
            },
            RoundCase {
                input: "-15.9",
                scale: 0,
                output: "-16",
                err: nil,
            },
            RoundCase {
                input: "15.17",
                scale: 1,
                output: "15.2",
                err: nil,
            },
            RoundCase {
                input: "15.4",
                scale: -1,
                output: "20",
                err: nil,
            },
            RoundCase {
                input: "-15.4",
                scale: -1,
                output: "-20",
                err: nil,
            },
            RoundCase {
                input: ".999",
                scale: 0,
                output: "1",
                err: nil,
            },
            RoundCase {
                input: "999999999",
                scale: -9,
                output: "1000000000",
                err: nil,
            },
        ],
    );
}

/// Truncate 舍入。
// TestRoundWithTruncate 对应 Go 的 ModeTruncate 舍入路径。
#[test]
fn TestRoundWithTruncate() {
    run_round_cases(
        ModeTruncate,
        vec![
            RoundCase {
                input: "123456789.987654321",
                scale: 1,
                output: "123456789.9",
                err: nil,
            },
            RoundCase {
                input: "15.5",
                scale: 0,
                output: "15",
                err: nil,
            },
            RoundCase {
                input: "-15.9",
                scale: 0,
                output: "-15",
                err: nil,
            },
            RoundCase {
                input: "15.17",
                scale: 1,
                output: "15.1",
                err: nil,
            },
            RoundCase {
                input: "15.4",
                scale: -1,
                output: "10",
                err: nil,
            },
            RoundCase {
                input: "-15.4",
                scale: -1,
                output: "-10",
                err: nil,
            },
            RoundCase {
                input: ".999",
                scale: 0,
                output: "0",
                err: nil,
            },
            RoundCase {
                input: "999999999",
                scale: -9,
                output: "0",
                err: nil,
            },
        ],
    );
}

/// Ceiling 舍入。
// TestRoundWithCeil 对应 Go 的 ModeCeiling 路径，保留源码中的负数 TODO 用例语义。
#[test]
fn TestRoundWithCeil() {
    run_round_cases(
        ModeCeiling,
        vec![
            RoundCase {
                input: "123456789.987654321",
                scale: 1,
                output: "123456790.0",
                err: nil,
            },
            RoundCase {
                input: "15.1",
                scale: 0,
                output: "16",
                err: nil,
            },
            RoundCase {
                input: "-15.1",
                scale: 0,
                output: "-16",
                err: nil,
            },
            RoundCase {
                input: "15.17",
                scale: 1,
                output: "15.2",
                err: nil,
            },
            RoundCase {
                input: "15.4",
                scale: -1,
                output: "20",
                err: nil,
            },
            RoundCase {
                input: "-15.4",
                scale: -1,
                output: "-20",
                err: nil,
            },
            RoundCase {
                input: ".999",
                scale: 0,
                output: "1",
                err: nil,
            },
            RoundCase {
                input: "999999999",
                scale: -9,
                output: "1000000000",
                err: nil,
            },
        ],
    );
}

/// 字符串标准化。
// TestToString 对应 Go 的字符串标准化测试，保留前导零被移除、尾随小数零保留的差异。
#[test]
fn TestToString() {
    for (input, output) in vec![
        ("123.123", "123.123"),
        ("123.1230", "123.1230"),
        ("00123.123", "123.123"),
    ] {
        let mut dec = MyDecimal::default();
        require::NoError(dec.FromString(input.as_bytes()));
        require::Equal(output, string(dec.ToString()));
    }
}

/// 二进制往返与非法参数矩阵。
// TestToBinFromBin 对应 Go 的二进制往返测试，并保留非法 precision/frac 的错误矩阵。
#[test]
fn TestToBinFromBin() {
    let tests = vec![
        BinCase {
            input: "-10.55",
            precision: 4,
            frac: 2,
            output: "-10.55",
            err: nil,
        },
        BinCase {
            input: "0.0123456789012345678912345",
            precision: 30,
            frac: 25,
            output: "0.0123456789012345678912345",
            err: nil,
        },
        BinCase {
            input: "12345",
            precision: 5,
            frac: 0,
            output: "12345",
            err: nil,
        },
        BinCase {
            input: "12345",
            precision: 10,
            frac: 3,
            output: "12345.000",
            err: nil,
        },
        BinCase {
            input: ".00012345000098765",
            precision: 15,
            frac: 14,
            output: "0.00012345000098",
            err: ErrTruncated,
        },
        BinCase {
            input: "111111111.11",
            precision: 10,
            frac: 2,
            output: "11111111.11",
            err: ErrOverflow,
        },
        BinCase {
            input: "1000",
            precision: 3,
            frac: 0,
            output: "0",
            err: ErrOverflow,
        },
        BinCase {
            input: "0.1000000",
            precision: 1,
            frac: 1,
            output: "0.1",
            err: ErrTruncated,
        },
        BinCase {
            input: "00000000000000000000000000000.00000000000012300",
            precision: 15,
            frac: 15,
            output: "0.000000000000123",
            err: ErrTruncated,
        },
        BinCase {
            input: "0.0000000000001234",
            precision: 20,
            frac: 20,
            output: "0.00000000000012340000",
            err: nil,
        },
    ];
    for ca in tests {
        let mut dec = MyDecimal::default();
        require::NoError(dec.FromString(ca.input.as_bytes()));
        let (buf, err) = dec.ToBin(ca.precision, ca.frac);
        require::Equal(ca.err, err);
        let mut dec2 = MyDecimal::default();
        require::NoError(dec2.FromBin(&buf, ca.precision, ca.frac).1);
        require::Equal(ca.output, string(dec2.ToString()));
    }

    let mut dec = MyDecimal::default();
    dec.FromInt(1);
    for tt in vec![
        BinErrCase {
            prec: 82,
            frac: 1,
            toBinErr: ErrBadNumber,
            fromBinErr: ErrTruncated,
        },
        BinErrCase {
            prec: -1,
            frac: 1,
            toBinErr: ErrBadNumber,
            fromBinErr: ErrBadNumber,
        },
        BinErrCase {
            prec: 10,
            frac: 31,
            toBinErr: ErrBadNumber,
            fromBinErr: ErrBadNumber,
        },
        BinErrCase {
            prec: 10,
            frac: -1,
            toBinErr: ErrBadNumber,
            fromBinErr: ErrBadNumber,
        },
    ] {
        require::Equal(tt.toBinErr, dec.ToBin(tt.prec, tt.frac).1);
        require::NoError(dec.FromString(b"0"));
        let (buf, err) = dec.ToBin(1, 0);
        require::NoError(err);
        require::Equal(tt.fromBinErr, dec.FromBin(&buf, tt.prec, tt.frac).1);
    }
}

/// DecimalBinSize 参数校验。
// TestDecimalBinSize 对应 Go 的 DecimalBinSize 参数校验。
#[test]
fn TestDecimalBinSize() {
    for tt in vec![
        (3, 1, 2, nil),
        (-1, 0, 0, ErrBadNumber),
        (3, 5, 0, ErrBadNumber),
    ] {
        let result = DecimalBinSize(tt.0, tt.1);
        require::Equal(tt.3, result.as_ref().map(|_| ()).map_err(Clone::clone));
        require::Equal(tt.2, result.unwrap_or(0));
    }
}

/// Compare 符号与零值。
// TestCompareMyDecimal 对应 Go 的 Compare 测试，覆盖符号、零值和小数比较。
#[test]
fn TestCompareMyDecimal() {
    for (a, b, cmp) in vec![
        ("12", "13", -1),
        ("13", "12", 1),
        ("-10", "10", -1),
        ("10", "-10", 1),
        ("-12", "-13", 1),
        ("0", "12", -1),
        ("-10", "0", -1),
        ("4", "4", 0),
        ("-1.1", "-1.2", 1),
        ("1.2", "1.1", 1),
        ("1.1", "1.2", -1),
    ] {
        let mut da = MyDecimal::default();
        let mut db = MyDecimal::default();
        require::NoError(da.FromString(a.as_bytes()));
        require::NoError(db.FromString(b.as_bytes()));
        require::Equal(cmp, da.Compare(&db));
    }
}

/// maxDecimal 构造。
// TestMaxDecimal 对应 Go 的 maxDecimal 构造测试，覆盖整数位、小数位和高精度边界。
#[test]
fn TestMaxDecimal() {
    for (prec, frac, result) in vec![
        (1, 1, "0.9"),
        (1, 0, "9"),
        (2, 1, "9.9"),
        (4, 2, "99.99"),
        (6, 3, "999.999"),
        (8, 4, "9999.9999"),
        (10, 5, "99999.99999"),
        (12, 6, "999999.999999"),
        (14, 7, "9999999.9999999"),
        (16, 8, "99999999.99999999"),
        (18, 9, "999999999.999999999"),
        (20, 10, "9999999999.9999999999"),
        (20, 20, "0.99999999999999999999"),
        (20, 0, "99999999999999999999"),
        (40, 20, "99999999999999999999.99999999999999999999"),
    ] {
        let dec = NewMaxOrMinDec(false, prec, frac);
        require::Equal(result, string(dec.ToString()));
    }
}

/// DecimalNeg。
// TestNegMyDecimal 对应 Go 的 DecimalNeg 测试，包含极小负小数、超长负整数和零。
#[test]
fn TestNegMyDecimal() {
    for (a, result) in vec![
        (
            "-0.0000000000000000000000000000000000000000000000000017382578996420603",
            "0.0000000000000000000000000000000000000000000000000017382578996420603",
        ),
        (
            "-13890436710184412000000000000000000000000000000000000000000000000000000000000",
            "13890436710184412000000000000000000000000000000000000000000000000000000000000",
        ),
        ("0", "0"),
    ] {
        let negResult = DecimalNeg(&NewDecFromStringForTest(a));
        require::Equal(result, string(negResult.ToString()));
    }
}

/// DecimalAdd 表驱动。
// TestAddMyDecimal 对应 Go 的 DecimalAdd 表驱动测试，覆盖符号组合和超长字符串拼接样本。
#[test]
fn TestAddMyDecimal() {
    run_binary_decimal_cases(
        DecimalAdd,
        vec![
            DecOpCase {
                a: ".00012345000098765",
                b: "123.45",
                result: "123.45012345000098765",
                err: nil,
            },
            DecOpCase {
                a: ".1",
                b: ".45",
                result: "0.55",
                err: nil,
            },
            DecOpCase {
                a: "1234500009876.5",
                b: ".00012345000098765",
                result: "1234500009876.50012345000098765",
                err: nil,
            },
            DecOpCase {
                a: "9999909999999.5",
                b: ".555",
                result: "9999910000000.055",
                err: nil,
            },
            DecOpCase {
                a: "999999999",
                b: "1",
                result: "1000000000",
                err: nil,
            },
            DecOpCase {
                a: "-12345",
                b: "123.45",
                result: "-12221.55",
                err: nil,
            },
            DecOpCase {
                a: "12345",
                b: "-123.45",
                result: "12221.55",
                err: nil,
            },
            DecOpCase {
                a: "5",
                b: "-6.0",
                result: "-1.0",
                err: nil,
            },
            DecOpCase {
                a: "-1234.1234",
                b: "1234.1234",
                result: "0.0000",
                err: nil,
            },
        ],
    );
}

/// DecimalSub 表驱动。
// TestSubMyDecimal 对应 Go 的 DecimalSub 表驱动测试，保留正负混合和小数尾零语义。
#[test]
fn TestSubMyDecimal() {
    run_binary_decimal_cases(
        DecimalSub,
        vec![
            DecOpCase {
                a: ".00012345000098765",
                b: "123.45",
                result: "-123.44987654999901235",
                err: nil,
            },
            DecOpCase {
                a: "1234500009876.5",
                b: ".00012345000098765",
                result: "1234500009876.49987654999901235",
                err: nil,
            },
            DecOpCase {
                a: "9999900000000.5",
                b: ".555",
                result: "9999899999999.945",
                err: nil,
            },
            DecOpCase {
                a: "1111.5551",
                b: "1111.555",
                result: "0.0001",
                err: nil,
            },
            DecOpCase {
                a: ".555",
                b: ".555",
                result: "0.000",
                err: nil,
            },
            DecOpCase {
                a: "1000000000",
                b: ".1",
                result: "999999999.9",
                err: nil,
            },
            DecOpCase {
                a: "-12345",
                b: "123.45",
                result: "-12468.45",
                err: nil,
            },
            DecOpCase {
                a: "12.12",
                b: "12.12",
                result: "0.00",
                err: nil,
            },
        ],
    );
}

/// DecimalMul。
// TestMulMyDecimal 对应 Go 的 DecimalMul 测试，覆盖截断、溢出、零和高精度乘积。
#[test]
fn TestMulMyDecimal() {
    run_binary_decimal_cases(
        DecimalMul,
        vec![
            DecOpCase {
                a: "12",
                b: "10",
                result: "120",
                err: nil,
            },
            DecOpCase {
                a: "-123.456",
                b: "98765.4321",
                result: "-12193185.1853376",
                err: nil,
            },
            DecOpCase {
                a: "-123456000000",
                b: "98765432100000",
                result: "-12193185185337600000000000",
                err: nil,
            },
            DecOpCase {
                a: "123",
                b: "0.01",
                result: "1.23",
                err: nil,
            },
            DecOpCase {
                a: "-0.0000000000000000000000000000000000000000000000000017382578996420603",
                b: "-13890436710184412000000000000000000000000000000000000000000000000000000000000",
                result: "0.000000000000000000000000000000",
                err: ErrTruncated,
            },
            DecOpCase {
                a: "0.5999991229316",
                b: "0.918755041726043",
                result: "0.5512522192246113614062276588",
                err: nil,
            },
            DecOpCase {
                a: "0.000",
                b: "-1",
                result: "0.000",
                err: nil,
            },
        ],
    );
}

/// DecimalDiv / DecimalMod。
// TestDivModMyDecimal 对应 Go 的 DecimalDiv/DecimalMod 测试，包含除零、指定 scale 和结果小数位保留。
#[test]
fn TestDivModMyDecimal() {
    run_div_cases(
        5,
        vec![
            DecOpCase {
                a: "120",
                b: "10",
                result: "12.000000000",
                err: nil,
            },
            DecOpCase {
                a: "123",
                b: "0.01",
                result: "12300.000000000",
                err: nil,
            },
            DecOpCase {
                a: "123",
                b: "0",
                result: "",
                err: ErrDivByZero,
            },
            DecOpCase {
                a: "1.000000000000",
                b: "3",
                result: "0.333333333333333333",
                err: nil,
            },
            DecOpCase {
                a: "51",
                b: "0.003430",
                result: "14868.804664723032069970",
                err: nil,
            },
        ],
    );
    run_mod_cases(vec![
        DecOpCase {
            a: "234",
            b: "10",
            result: "4",
            err: nil,
        },
        DecOpCase {
            a: "234.567",
            b: "10.555",
            result: "2.357",
            err: nil,
        },
        DecOpCase {
            a: "99999999999999999999999999999999999999",
            b: "3",
            result: "0",
            err: nil,
        },
        DecOpCase {
            a: "0.000",
            b: "0.1",
            result: "0.000",
            err: nil,
        },
    ]);
    run_div_cases(
        4,
        vec![
            DecOpCase {
                a: "1",
                b: "1",
                result: "1.0000",
                err: nil,
            },
            DecOpCase {
                a: "1.00",
                b: "1",
                result: "1.000000",
                err: nil,
            },
            DecOpCase {
                a: "2",
                b: "3",
                result: "0.6667",
                err: nil,
            },
            DecOpCase {
                a: "0.000",
                b: "0.1",
                result: "0.0000000",
                err: nil,
            },
        ],
    );
    run_mod_cases(vec![
        DecOpCase {
            a: "1",
            b: "2.0",
            result: "1.0",
            err: nil,
        },
        DecOpCase {
            a: "1.0",
            b: "2",
            result: "1.0",
            err: nil,
        },
        DecOpCase {
            a: "2.23",
            b: "3",
            result: "2.23",
            err: nil,
        },
        DecOpCase {
            a: "51",
            b: "0.003430",
            result: "0.002760",
            err: nil,
        },
    ]);
}

/// NewMaxOrMinDec。
// TestMaxOrMinMyDecimal 对应 Go 的 NewMaxOrMinDec 测试。
#[test]
fn TestMaxOrMinMyDecimal() {
    for tt in vec![
        (true, 2, 1, "-9.9"),
        (false, 1, 1, "0.9"),
        (true, 1, 0, "-9"),
        (false, 0, 0, "0"),
        (false, 4, 2, "99.99"),
    ] {
        let dec = NewMaxOrMinDec(tt.0, tt.1, tt.2);
        require::Equal(tt.3, dec.String());
    }
}

/// 复用输出变量时第二次运算覆盖旧值。
// TestReset 对应 Go 的复用输出变量测试，确保第二次 DecimalAdd 会覆盖旧内容。
#[test]
fn TestReset() {
    let mut x1 = dec_from("38520.130741106671");
    let mut y1 = dec_from("9863.944799797851");
    let mut z1 = MyDecimal::default();
    require::NoError(DecimalAdd(&mut x1, &mut y1, &mut z1));

    let mut x2 = dec_from("121519.080207244");
    let mut y2 = dec_from("54982.444519146");
    let mut z2 = MyDecimal::default();
    require::NoError(DecimalAdd(&mut x2, &mut y2, &mut z2));

    require::NoError(DecimalAdd(&mut x2, &mut y2, &mut z1));
    require::Equal(z2, z1);
}

/// Shift；会改 wordBufLen，需串行。
// TestShiftMyDecimal 对应 Go 的 Shift 测试；它会修改全局 wordBufLen，因此必须串行运行。
#[test]
fn TestShiftMyDecimal() {
    run_shift_cases(vec![
        ShiftCase {
            input: "123.123",
            shift: 1,
            output: "1231.23",
            err: nil,
        },
        ShiftCase {
            input: "123457189.123123456789000",
            shift: 17,
            output: "12345718912312345678900000",
            err: nil,
        },
        ShiftCase {
            input: "000.000",
            shift: 1000,
            output: "0",
            err: nil,
        },
        ShiftCase {
            input: "1",
            shift: 1000,
            output: "1",
            err: ErrOverflow,
        },
        ShiftCase {
            input: "123987654321.123456789000",
            shift: -12,
            output: "0.123987654321123456789",
            err: nil,
        },
        ShiftCase {
            input: "00000087654321.123456789000",
            shift: -14,
            output: "0.00000087654321123456789",
            err: nil,
        },
    ]);

    // Rust uses the fixed production buffer size. Exercise the same truncation
    // and overflow branches at the real 81-digit boundary instead of mutating
    // Go's package-level test hook.
    let mut truncated = dec_from("123.123");
    require::Equal(ErrTruncated, truncated.Shift(-82));
    require::Equal(
        format!("0.{}12", "0".repeat(79)),
        string(truncated.ToString()),
    );
    let mut overflowing = dec_from(&"9".repeat(81));
    require::Equal(ErrOverflow, overflowing.Shift(1));
}

/// FromString；会改 wordBufLen，需串行。
// TestFromStringMyDecimal 对应 Go 的 FromString 解析测试；它也修改 wordBufLen，必须串行。
#[test]
fn TestFromStringMyDecimal() {
    run_from_string_cases(vec![
        ParseCase {
            input: "12345",
            output: "12345",
            err: nil,
        },
        ParseCase {
            input: "12345.",
            output: "12345",
            err: nil,
        },
        ParseCase {
            input: "123.45.",
            output: "123.45",
            err: ErrTruncated,
        },
        ParseCase {
            input: ".00012345000098765",
            output: "0.00012345000098765",
            err: nil,
        },
        ParseCase {
            input: "123E5",
            output: "12300000",
            err: nil,
        },
        ParseCase {
            input: "1e1073741823",
            output: "999999999999999999999999999999999999999999999999999999999999999999999999999999999",
            err: ErrOverflow,
        },
        ParseCase {
            input: "1e18446744073709551620",
            output: "0",
            err: ErrBadNumber,
        },
        ParseCase {
            input: "1eabc",
            output: "1",
            err: ErrTruncated,
        },
        ParseCase {
            input: "1e -1",
            output: "0.1",
            err: nil,
        },
        ParseCase {
            input: "1.1.1.1.1",
            output: "1.1",
            err: ErrTruncated,
        },
        ParseCase {
            input: "1  ",
            output: "1",
            err: nil,
        },
    ]);

    run_from_string_cases(vec![
        ParseCase {
            input: "1234567890123456789012345678901234567890123456789012345678901234567890123456789012",
            output: "234567890123456789012345678901234567890123456789012345678901234567890123456789012",
            err: ErrOverflow,
        },
        ParseCase {
            input: "123456789012345678901234567890123456789012345678901234567890123456789012345678.000098765",
            output: "123456789012345678901234567890123456789012345678901234567890123456789012345678",
            err: ErrTruncated,
        },
    ]);
}

/// JSON marshal/unmarshal 往返。
// TestMarshalMyDecimal 对应 Go 的 JSON marshal/unmarshal 往返比较。
#[test]
fn TestMarshalMyDecimal() {
    for tt in vec![
        "12345",
        "12345.",
        ".00012345000098765",
        ".12345000098765",
        "-.000000012345000098765",
        "123E-2",
    ] {
        let mut v1 = MyDecimal::default();
        let mut v2 = MyDecimal::default();
        require::NoError(v1.FromString(tt.as_bytes()));
        let j = v1.MarshalJSON().unwrap();
        require::NoError(v2.UnmarshalJSON(&j));
        require::Equal(0, v1.Compare(&v2));
    }
}

/// 按模式跑一组 Round 用例。
fn run_round_cases(mode: RoundMode, tests: Vec<RoundCase>) {
    for ca in tests {
        let mut dec = MyDecimal::default();
        require::NoError(dec.FromString(ca.input.as_bytes()));
        let mut rounded = MyDecimal::default();
        let err = dec.Round(&mut rounded, ca.scale, mode);
        require::Equal(ca.err, err);
        require::Equal(ca.output, string(rounded.ToString()));
    }
}

/// 跑二元运算（加/减/乘）用例表。
fn run_binary_decimal_cases(
    op: fn(&MyDecimal, &MyDecimal, &mut MyDecimal) -> DraftError,
    tests: Vec<DecOpCase>,
) {
    for tt in tests {
        let mut a = dec_from(tt.a);
        let mut b = dec_from(tt.b);
        let mut out = MyDecimal::default();
        let err = op(&mut a, &mut b, &mut out);
        require::Equal(tt.err, err);
        require::Equal(tt.result, out.String());
    }
}

/// 跑除法用例；scale=4 时用 String() 比较。
fn run_div_cases(scale: isize, tests: Vec<DecOpCase>) {
    for tt in tests {
        let mut a = dec_from(tt.a);
        let mut b = dec_from(tt.b);
        let mut to = MyDecimal::default();
        let err = DecimalDiv(&mut a, &mut b, &mut to, scale);
        require::Equal(tt.err, err);
        if tt.err != ErrDivByZero {
            if scale == 4 {
                require::Equal(tt.result, to.String());
            } else {
                require::Equal(tt.result, string(to.ToString()));
            }
        }
    }
}

/// 跑取模用例。
fn run_mod_cases(tests: Vec<DecOpCase>) {
    for tt in tests {
        let mut a = dec_from(tt.a);
        let mut b = dec_from(tt.b);
        let mut to = MyDecimal::default();
        let err = DecimalMod(&mut a, &mut b, &mut to);
        require::Equal(tt.err, err);
        if tt.err != ErrDivByZero {
            require::Equal(tt.result, to.String());
        }
    }
}

/// 跑移位用例。
fn run_shift_cases(tests: Vec<ShiftCase>) {
    for test in tests {
        let mut dec = dec_from(test.input);
        require::Equal(test.err, dec.Shift(test.shift));
        require::Equal(test.output, string(dec.ToString()));
    }
}

/// 跑字符串解析用例。
fn run_from_string_cases(tests: Vec<ParseCase>) {
    for test in tests {
        let mut dec = MyDecimal::default();
        require::Equal(test.err, dec.FromString(test.input.as_bytes()));
        require::Equal(test.output, string(dec.ToString()));
    }
}

/// 解析成功才返回的测试夹具。
fn dec_from(input: &str) -> MyDecimal {
    let mut dec = MyDecimal::default();
    require::NoError(dec.FromString(input.as_bytes()));
    dec
}

/// 去掉小数尾零后剩余小数位数。
// 从小数点后向左扫掉尾随 '0'
fn expected_frac_without_trailing_zeros(str: String) -> i32 {
    if let Some(point) = str.find('.') {
        let mut pos = str.len() - 1;
        while pos > point && str.as_bytes()[pos] == b'0' {
            pos -= 1;
        }
        return (pos - point) as i32;
    }
    0
}

/// ToInt 用例：输入文本、期望整数、期望错误。
struct IntConvCase {
    input: &'static str,
    output: i64,
    err: DraftError,
}
/// ToUint 用例。
struct UintConvCase {
    input: &'static str,
    output: u64,
    err: DraftError,
}
/// hash/bin 等价样本集合。
struct HashBinCase {
    hashNumbers: Vec<&'static str>,
    binNumbers: Vec<&'static str>,
}
/// 舍入用例。
struct RoundCase {
    input: &'static str,
    scale: isize,
    output: &'static str,
    err: DraftError,
}
/// 二进制往返用例。
struct BinCase {
    input: &'static str,
    precision: isize,
    frac: isize,
    output: &'static str,
    err: DraftError,
}
/// 非法 precision/frac 错误矩阵行。
struct BinErrCase {
    prec: isize,
    frac: isize,
    toBinErr: DraftError,
    fromBinErr: DraftError,
}
/// 二元运算用例。
struct DecOpCase {
    a: &'static str,
    b: &'static str,
    result: &'static str,
    err: DraftError,
}
/// 移位用例。
struct ShiftCase {
    input: &'static str,
    shift: isize,
    output: &'static str,
    err: DraftError,
}
/// 解析用例。
struct ParseCase {
    input: &'static str,
    output: &'static str,
    err: DraftError,
}

#[test]
fn audit_parse_bytes_and_exponent_errors() {
    for bytes in [b"\n1".as_slice(), "\u{a0}1".as_bytes()] {
        let mut d = MyDecimal::default();
        assert_eq!(d.FromString(bytes), Err(DecimalError::TruncatedWrongValue));
    }
    let mut d = MyDecimal::default();
    assert_eq!(d.FromString(b"12\xff"), ErrTruncated);
    assert_eq!(d.ToString(), b"12");
    for (input, expected) in [
        (format!("{}e0", "9".repeat(82)), ErrOverflow),
        (format!("{}.1e0", "9".repeat(81)), ErrTruncated),
    ] {
        assert_eq!(d.FromString(input.as_bytes()), expected, "{input}");
    }
}

#[test]
fn audit_shift_preserves_result_scale_and_zero_noop() {
    let mut zero = dec_from("0.000");
    let original = zero.clone();
    assert_eq!(zero.Shift(0), nil);
    assert_eq!(zero, original);
    let mut d = dec_from("1.23");
    assert_eq!(d.Shift(-2), nil);
    assert_eq!(d.resultFrac, 2);
    assert_eq!(d.String(), "0.01");
    let mut d = dec_from("1.000000000");
    assert_eq!(d.Shift(-80), nil);
    assert_eq!(d.digitsFrac, 80);
}

#[test]
fn audit_nonfinite_float_errors() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut d = dec_from("123");
        assert_eq!(d.FromFloat64(value), Err(DecimalError::TruncatedWrongValue));
        assert!(d.IsZero());
    }
}

#[test]
fn audit_division_error_resets_output() {
    let a = dec_from("1.23");
    let zero = MyDecimal::default();
    let mut out = dec_from("999");
    assert_eq!(DecimalDiv(&a, &zero, &mut out, 4), ErrDivByZero);
    assert!(out.IsZero());
    assert_eq!(out.resultFrac, 6);
    out = dec_from("999");
    assert_eq!(DecimalMod(&a, &zero, &mut out), ErrDivByZero);
    assert!(out.IsZero());
    assert_eq!(out.resultFrac, 2);
    assert_eq!(DecimalDiv(&zero, &a, &mut out, 4), nil);
    assert_eq!(out.ToString(), b"0.0000");
}

#[test]
fn audit_bin_preserves_sign_and_truncation_precedence() {
    let d = dec_from("-0.1");
    assert_eq!(d.ToBin(1, 0), (vec![0x7f], ErrTruncated));
    let d = dec_from("123.45");
    assert_eq!(d.ToBin(2, 1).1, ErrTruncated);
}

#[test]
fn audit_json_go_defaults_and_case_insensitive_fields() {
    for input in [b"null".as_slice(), b"{}", b"{\"unknown\":1}"] {
        let mut d = dec_from("123");
        assert_eq!(d.UnmarshalJSON(input), nil);
        assert_eq!(d, MyDecimal::default());
    }
    let mut d = MyDecimal::default();
    assert_eq!(d.UnmarshalJSON(br#"{"digitsint":1,"wordbuf":[7]}"#), nil);
    assert_eq!(d.ToString(), b"7");
    let original = d.clone();
    assert_eq!(
        d.UnmarshalJSON(br#"{"DigitsInt":128}"#),
        Err(DecimalError::InvalidJson)
    );
    assert_eq!(d, original);
}

#[test]
fn audit_round_go_word_boundaries() {
    let mut out = MyDecimal::default();
    assert_eq!(dec_from("3.0001").Round(&mut out, 1, ModeCeiling), nil);
    // Go's documented unsupported ceiling mode inspects only the next digit
    // within a word, and all discarded words at a word boundary.
    assert_eq!(out.ToString(), b"3.0");
    assert_eq!(dec_from("3.0001").Round(&mut out, 0, ModeCeiling), nil);
    assert_eq!(out.ToString(), b"4");
    assert_eq!(
        dec_from("0000000001").Round(&mut out, 81, ModeHalfUp),
        ErrTruncated
    );
    assert_eq!(out.digitsFrac, 63);
}

#[test]
fn audit_add_discards_operand_fraction_before_carry() {
    let a = dec_from(&format!("{}.9", "9".repeat(72)));
    let b = dec_from(&format!("{}.9", "0".repeat(72)));
    let mut out = MyDecimal::default();
    assert_eq!(DecimalAdd(&a, &b, &mut out), ErrTruncated);
    assert_eq!(out.ToString(), "9".repeat(72).as_bytes());
}

#[test]
fn audit_go_capacity_fixtures() {
    for (op, a, b, n, expected, error) in [
        ("shift", ".999999999", "", -81, "0", "Truncated"),
        ("shift", "999999999", "", -90, "0", "Truncated"),
        ("shift", "0.0000000001", "", -72, "0", "Truncated"),
        (
            "shift",
            "-62055823954874198005819.17980719654689072402822712508",
            "",
            -90,
            "-0.000000000000000000000000000000000000000000000000000000000000000000062055823954874",
            "Truncated",
        ),
        (
            "shift",
            "-62055823954874198005819.17980719654689072402822712508",
            "",
            -81,
            "-0.000000000000000000000000000000000000000000000000000000000062055823954874198005819",
            "Truncated",
        ),
        (
            "shift",
            "-62055823954874198005819.17980719654689072402822712508",
            "",
            -72,
            "-0.000000000000000000000000000000000000000000000000062055823954874198005819179807197",
            "Truncated",
        ),
        (
            "mul",
            "0",
            "4427.752047545920766961172303888714258339951077352397013704364408389189004069",
            4,
            "0.0000000000000000000000000000000",
            "",
        ),
        (
            "mul",
            "0",
            "-9087.534542423051662292042383562628530",
            4,
            "0.000000000000000000000000000000",
            "",
        ),
        (
            "mul",
            "0.000",
            "-499359544.17999605075177443505905823377330018967304247",
            4,
            "0.000000000000000000000000000000",
            "",
        ),
        (
            "mul",
            "7622042184188082572972425846597638292091662827645249381527588128.082",
            "4427.752047545920766961172303888714258339951077352397013704364408389189004069",
            4,
            "33748512887520164760486052389649785626388720457352228666439320777223.766716988",
            "Truncated",
        ),
        (
            "mul",
            "-62055823954874198005819.17980719654689072402822712508",
            "-8585.946360644",
            4,
            "532807975842116875252921429.1995322453768158641188481080689",
            "",
        ),
        (
            "mul",
            "-62055823954874198005819.17980719654689072402822712508",
            "-27631235384584953317022343271276868383349286050052.6573516267161",
            4,
            "1714679078681494519269971409544604469186430564911105983985751502036779404",
            "Truncated",
        ),
        (
            "div",
            "00000000000000001",
            "1.00000000000000000000000000000000000000000000000000000000000000000000001",
            4,
            "0.999999999999999999999999999999999999999999999999999999999999999999999990",
            "Truncated",
        ),
        (
            "div",
            "999999999999999999999999999999999999999999999999999999999999999999999999999999999",
            "0.0000000001",
            4,
            "9999999999999999999999999999999999999999999999999999999999999999999999999",
            "Overflow",
        ),
        (
            "div",
            "00000000000000001",
            "1.00000000000000000000000000000000000000000000000000000000000000000000001",
            4,
            "0.999999999999999999999999999999999999999999999999999999999999999999999990",
            "Truncated",
        ),
        (
            "div",
            "999999999999999999999999999999999999999999999999999999999999999999999999999999999",
            "0.0000000001",
            4,
            "9999999999999999999999999999999999999999999999999999999999999999999999999",
            "Overflow",
        ),
    ] {
        let a = dec_from(a);
        let b = if b.is_empty() {
            MyDecimal::default()
        } else {
            dec_from(b)
        };
        let mut out = MyDecimal::default();
        let status = match op {
            "shift" => {
                out = a.clone();
                out.Shift(n)
            }
            "mul" => DecimalMul(&a, &b, &mut out),
            "div" => DecimalDiv(&a, &b, &mut out, n),
            _ => unreachable!(),
        };
        assert_eq!(
            status.err().map(|e| format!("{e:?}")).unwrap_or_default(),
            error,
            "{op} {a:?}"
        );
        assert_eq!(string(out.ToString()), expected, "{op}");
    }
}

#[test]
fn audit_constructor_reuse_matches_go_fields() {
    let mut d = dec_from("-12.34");
    d.FromUint(7);
    assert_eq!(d.digitsInt, 9);
    assert_eq!(d.resultFrac, 2);
    assert_eq!(d.wordBuf[1], 340000000);
    assert!(d.IsNegative());
    assert_eq!(d.ToString(), b"-7");
    assert_eq!(d.FromString(b"8"), nil);
    assert_eq!(d.ToString(), b"-8");
}

#[test]
fn audit_decode_word_capacity_and_leading_zero_words() {
    let d = dec_from("1.23");
    let (buf, err) = d.ToBin(20, 2);
    assert_eq!(err, nil);
    let mut out = MyDecimal::default();
    assert_eq!(out.FromBin(&buf, 20, 2).1, nil);
    assert_eq!(out.digitsInt, 9);
    assert_eq!(out.ToString(), b"1.23");
    // 73 integer digits plus one fractional digit consume ten words,
    // despite their combined precision being less than 81.
    assert_eq!(out.FromBin(&[0x80], 74, 1).1, ErrTruncated);
    assert_eq!(out.digitsFrac, 0);
    assert_eq!(out.resultFrac, 1);
}

#[test]
fn audit_float_scientific_overflow() {
    let mut d = MyDecimal::default();
    assert_eq!(d.FromFloat64(1e100), ErrOverflow);
    assert_eq!(d.ToString(), "9".repeat(81).as_bytes());
}

#[test]
fn audit_parquet_consumes_disposable_buffer() {
    let mut bytes = [0xff, 0x85];
    let mut d = MyDecimal::default();
    assert_eq!(d.FromParquetArray(&mut bytes, 2), nil);
    assert_eq!(d.ToString(), b"-1.23");
    assert_eq!(bytes, [0, 0]);
}

#[test]
fn audit_binary_partial_word_contract() {
    assert_eq!(dec_from("999999999.99999999").ToBin(25, 9).1, ErrTruncated);
    let mut d = MyDecimal::default();
    assert_eq!(d.FromBin(&[0x45], 1, 0).1, nil);
    assert_eq!(d.wordBuf[0], 58);
    assert_eq!(d.ToString(), b"-8");
}

#[test]
fn audit_json_duplicate_fields_and_wire_order() {
    let d = dec_from("1.2");
    assert_eq!(
        string(d.MarshalJSON().unwrap()),
        r#"{"DigitsInt":1,"DigitsFrac":1,"ResultFrac":1,"Negative":false,"WordBuf":[1,200000000,0,0,0,0,0,0,0]}"#
    );
    let mut d = MyDecimal::default();
    assert_eq!(
        d.UnmarshalJSON(br#"{"DigitsInt":1,"DigitsInt":null,"WordBuf":[7,8],"WordBuf":[null]}"#),
        nil
    );
    assert_eq!(d.digitsInt, 1);
    assert_eq!(d.wordBuf, [7, 0, 0, 0, 0, 0, 0, 0, 0]);
}

// Full Go table coverage for arithmetic, rounding, binary conversion, parsing
// and shifting. Go's 1/2-word test cases are moved to the real 9-word boundary
// (72 integer digits or 63 shift positions), with expected values from Go.
#[test]
fn audit_full_go_tables_at_production_capacity() {
    for (op, input, rhs, n, mode, expected, error) in [
        ("round", "123456789.987654321", "", 1, 5, "123456790.0", ""),
        ("round", "15.1", "", 0, 5, "15", ""),
        ("round", "15.5", "", 0, 5, "16", ""),
        ("round", "15.9", "", 0, 5, "16", ""),
        ("round", "-15.1", "", 0, 5, "-15", ""),
        ("round", "-15.5", "", 0, 5, "-16", ""),
        ("round", "-15.9", "", 0, 5, "-16", ""),
        ("round", "15.1", "", 1, 5, "15.1", ""),
        ("round", "-15.1", "", 1, 5, "-15.1", ""),
        ("round", "15.17", "", 1, 5, "15.2", ""),
        ("round", "15.4", "", -1, 5, "20", ""),
        ("round", "-15.4", "", -1, 5, "-20", ""),
        ("round", "5.4", "", -1, 5, "10", ""),
        ("round", ".999", "", 0, 5, "1", ""),
        ("round", "999999999", "", -9, 5, "1000000000", ""),
        ("round", "123456789.987654321", "", 1, 10, "123456789.9", ""),
        ("round", "15.1", "", 0, 10, "15", ""),
        ("round", "15.5", "", 0, 10, "15", ""),
        ("round", "15.9", "", 0, 10, "15", ""),
        ("round", "-15.1", "", 0, 10, "-15", ""),
        ("round", "-15.5", "", 0, 10, "-15", ""),
        ("round", "-15.9", "", 0, 10, "-15", ""),
        ("round", "15.1", "", 1, 10, "15.1", ""),
        ("round", "-15.1", "", 1, 10, "-15.1", ""),
        ("round", "15.17", "", 1, 10, "15.1", ""),
        ("round", "15.4", "", -1, 10, "10", ""),
        ("round", "-15.4", "", -1, 10, "-10", ""),
        ("round", "5.4", "", -1, 10, "0", ""),
        ("round", ".999", "", 0, 10, "0", ""),
        ("round", "999999999", "", -9, 10, "0", ""),
        ("round", "123456789.987654321", "", 1, 0, "123456790.0", ""),
        ("round", "15.1", "", 0, 0, "16", ""),
        ("round", "15.5", "", 0, 0, "16", ""),
        ("round", "15.9", "", 0, 0, "16", ""),
        ("round", "-15.1", "", 0, 0, "-16", ""),
        ("round", "-15.5", "", 0, 0, "-16", ""),
        ("round", "-15.9", "", 0, 0, "-16", ""),
        ("round", "15.1", "", 1, 0, "15.1", ""),
        ("round", "-15.1", "", 1, 0, "-15.1", ""),
        ("round", "15.17", "", 1, 0, "15.2", ""),
        ("round", "15.4", "", -1, 0, "20", ""),
        ("round", "-15.4", "", -1, 0, "-20", ""),
        ("round", "5.4", "", -1, 0, "10", ""),
        ("round", ".999", "", 0, 0, "1", ""),
        ("round", "999999999", "", -9, 0, "1000000000", ""),
        ("bin", "-10.55", "", 4, 2, "-10.55", ""),
        (
            "bin",
            "0.0123456789012345678912345",
            "",
            30,
            25,
            "0.0123456789012345678912345",
            "",
        ),
        ("bin", "12345", "", 5, 0, "12345", ""),
        ("bin", "12345", "", 10, 3, "12345.000", ""),
        ("bin", "123.45", "", 10, 3, "123.450", ""),
        ("bin", "-123.45", "", 20, 10, "-123.4500000000", ""),
        (
            "bin",
            ".00012345000098765",
            "",
            15,
            14,
            "0.00012345000098",
            "Truncated",
        ),
        (
            "bin",
            ".00012345000098765",
            "",
            22,
            20,
            "0.00012345000098765000",
            "",
        ),
        (
            "bin",
            ".12345000098765",
            "",
            30,
            20,
            "0.12345000098765000000",
            "",
        ),
        (
            "bin",
            "-.000000012345000098765",
            "",
            30,
            20,
            "-0.00000001234500009876",
            "Truncated",
        ),
        (
            "bin",
            "1234500009876.5",
            "",
            30,
            5,
            "1234500009876.50000",
            "",
        ),
        ("bin", "111111111.11", "", 10, 2, "11111111.11", "Overflow"),
        ("bin", "000000000.01", "", 7, 3, "0.010", ""),
        ("bin", "123.4", "", 10, 2, "123.40", ""),
        ("bin", "1000", "", 3, 0, "0", "Overflow"),
        ("bin", "0.1", "", 1, 1, "0.1", ""),
        ("bin", "0.100", "", 1, 1, "0.1", "Truncated"),
        ("bin", "0.1000", "", 1, 1, "0.1", "Truncated"),
        ("bin", "0.10000", "", 1, 1, "0.1", "Truncated"),
        ("bin", "0.100000", "", 1, 1, "0.1", "Truncated"),
        ("bin", "0.1000000", "", 1, 1, "0.1", "Truncated"),
        ("bin", "0.10", "", 1, 1, "0.1", "Truncated"),
        (
            "bin",
            "0000000000000000000000000000000000000000000.000000000000123000000000000000",
            "",
            15,
            15,
            "0.000000000000123",
            "Truncated",
        ),
        (
            "bin",
            "00000000000000000000000000000.00000000000012300",
            "",
            15,
            15,
            "0.000000000000123",
            "Truncated",
        ),
        (
            "bin",
            "0000000000000000000000000000000000000000000.0000000000001234000000000000000",
            "",
            16,
            16,
            "0.0000000000001234",
            "Truncated",
        ),
        (
            "bin",
            "00000000000000000000000000000.000000000000123400",
            "",
            16,
            16,
            "0.0000000000001234",
            "Truncated",
        ),
        ("bin", "0.1", "", 2, 2, "0.10", ""),
        ("bin", "0.10", "", 3, 3, "0.100", ""),
        ("bin", "0.1", "", 3, 1, "0.1", ""),
        (
            "bin",
            "0.0000000000001234",
            "",
            32,
            17,
            "0.00000000000012340",
            "",
        ),
        (
            "bin",
            "0.0000000000001234",
            "",
            20,
            20,
            "0.00000000000012340000",
            "",
        ),
        (
            "add",
            ".00012345000098765",
            "123.45",
            0,
            0,
            "123.45012345000098765",
            "",
        ),
        ("add", ".1", ".45", 0, 0, "0.55", ""),
        (
            "add",
            "1234500009876.5",
            ".00012345000098765",
            0,
            0,
            "1234500009876.50012345000098765",
            "",
        ),
        (
            "add",
            "9999909999999.5",
            ".555",
            0,
            0,
            "9999910000000.055",
            "",
        ),
        ("add", "99999999", "1", 0, 0, "100000000", ""),
        ("add", "989999999", "1", 0, 0, "990000000", ""),
        ("add", "999999999", "1", 0, 0, "1000000000", ""),
        ("add", "12345", "123.45", 0, 0, "12468.45", ""),
        ("add", "-12345", "-123.45", 0, 0, "-12468.45", ""),
        ("add", "-12345", "123.45", 0, 0, "-12221.55", ""),
        ("add", "12345", "-123.45", 0, 0, "12221.55", ""),
        ("add", "123.45", "-12345", 0, 0, "-12221.55", ""),
        ("add", "-123.45", "12345", 0, 0, "12221.55", ""),
        ("add", "5", "-6.0", 0, 0, "-1.0", ""),
        (
            "add",
            "211111111111111111111111111111111111111111111111111111111111111111111111",
            "888888888888888888888888888888888888888888888888888888888888888888888888888888888",
            0,
            0,
            "888888889099999999999999999999999999999999999999999999999999999999999999999999999",
            "",
        ),
        ("add", "-1234.1234", "1234.1234", 0, 0, "0.0000", ""),
        (
            "sub",
            ".00012345000098765",
            "123.45",
            0,
            0,
            "-123.44987654999901235",
            "",
        ),
        (
            "sub",
            "1234500009876.5",
            ".00012345000098765",
            0,
            0,
            "1234500009876.49987654999901235",
            "",
        ),
        (
            "sub",
            "9999900000000.5",
            ".555",
            0,
            0,
            "9999899999999.945",
            "",
        ),
        ("sub", "1111.5551", "1111.555", 0, 0, "0.0001", ""),
        ("sub", ".555", ".555", 0, 0, "0.000", ""),
        ("sub", "10000000", "1", 0, 0, "9999999", ""),
        ("sub", "1000001000", ".1", 0, 0, "1000000999.9", ""),
        ("sub", "1000000000", ".1", 0, 0, "999999999.9", ""),
        ("sub", "12345", "123.45", 0, 0, "12221.55", ""),
        ("sub", "-12345", "-123.45", 0, 0, "-12221.55", ""),
        ("sub", "123.45", "12345", 0, 0, "-12221.55", ""),
        ("sub", "-123.45", "-12345", 0, 0, "12221.55", ""),
        ("sub", "-12345", "123.45", 0, 0, "-12468.45", ""),
        ("sub", "12345", "-123.45", 0, 0, "12468.45", ""),
        ("sub", "12.12", "12.12", 0, 0, "0.00", ""),
        ("mul", "12", "10", 0, 0, "120", ""),
        (
            "mul",
            "-123.456",
            "98765.4321",
            0,
            0,
            "-12193185.1853376",
            "",
        ),
        (
            "mul",
            "-123456000000",
            "98765432100000",
            0,
            0,
            "-12193185185337600000000000",
            "",
        ),
        ("mul", "123456", "987654321", 0, 0, "121931851853376", ""),
        ("mul", "123456", "9876543210", 0, 0, "1219318518533760", ""),
        ("mul", "123", "0.01", 0, 0, "1.23", ""),
        ("mul", "123", "0", 0, 0, "0", ""),
        (
            "mul",
            "-0.0000000000000000000000000000000000000000000000000017382578996420603",
            "-13890436710184412000000000000000000000000000000000000000000000000000000000000",
            0,
            0,
            "0.000000000000000000000000000000",
            "Truncated",
        ),
        (
            "mul",
            "1000000000000000000000000000000000000000000000000000000000000",
            "1000000000000000000000000000000000000000000000000000000000000",
            0,
            0,
            "0",
            "Overflow",
        ),
        (
            "mul",
            "0.5999991229316",
            "0.918755041726043",
            0,
            0,
            "0.5512522192246113614062276588",
            "",
        ),
        (
            "mul",
            "0.5999991229317",
            "0.918755041726042",
            0,
            0,
            "0.5512522192247026369112773314",
            "",
        ),
        ("mul", "0.000", "-1", 0, 0, "0.000", ""),
        ("shift", "123.123", "", 1, 0, "1231.23", ""),
        (
            "shift",
            "123457189.123123456789000",
            "",
            1,
            0,
            "1234571891.23123456789",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            8,
            0,
            "12345718912312345.6789",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            9,
            0,
            "123457189123123456.789",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            10,
            0,
            "1234571891231234567.89",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            17,
            0,
            "12345718912312345678900000",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            18,
            0,
            "123457189123123456789000000",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            19,
            0,
            "1234571891231234567890000000",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            26,
            0,
            "12345718912312345678900000000000000",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            27,
            0,
            "123457189123123456789000000000000000",
            "",
        ),
        (
            "shift",
            "123457189.123123456789000",
            "",
            28,
            0,
            "1234571891231234567890000000000000000",
            "",
        ),
        (
            "shift",
            "000000000000000000000000123457189.123123456789000",
            "",
            26,
            0,
            "12345718912312345678900000000000000",
            "",
        ),
        (
            "shift",
            "00000000123457189.123123456789000",
            "",
            27,
            0,
            "123457189123123456789000000000000000",
            "",
        ),
        (
            "shift",
            "00000000000000000123457189.123123456789000",
            "",
            28,
            0,
            "1234571891231234567890000000000000000",
            "",
        ),
        ("shift", "123", "", 1, 0, "1230", ""),
        ("shift", "123", "", 10, 0, "1230000000000", ""),
        ("shift", ".123", "", 1, 0, "1.23", ""),
        ("shift", ".123", "", 10, 0, "1230000000", ""),
        ("shift", ".123", "", 14, 0, "12300000000000", ""),
        ("shift", "000.000", "", 1000, 0, "0", ""),
        ("shift", "000.", "", 1000, 0, "0", ""),
        ("shift", ".000", "", 1000, 0, "0", ""),
        ("shift", "1", "", 1000, 0, "1", "Overflow"),
        ("shift", "123.123", "", -1, 0, "12.3123", ""),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -1,
            0,
            "12398765432.1123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -2,
            0,
            "1239876543.21123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -3,
            0,
            "123987654.321123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -8,
            0,
            "1239.87654321123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -9,
            0,
            "123.987654321123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -10,
            0,
            "12.3987654321123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -11,
            0,
            "1.23987654321123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -12,
            0,
            "0.123987654321123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -13,
            0,
            "0.0123987654321123456789",
            "",
        ),
        (
            "shift",
            "123987654321.123456789000",
            "",
            -14,
            0,
            "0.00123987654321123456789",
            "",
        ),
        (
            "shift",
            "00000087654321.123456789000",
            "",
            -14,
            0,
            "0.00000087654321123456789",
            "",
        ),
        (
            "shift",
            "123.123",
            "",
            -65,
            0,
            "0.00000000000000000000000000000000000000000000000000000000000000123123",
            "",
        ),
        (
            "shift",
            "123.123",
            "",
            -66,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000123123",
            "",
        ),
        (
            "shift",
            "123.123",
            "",
            -69,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000123123",
            "",
        ),
        (
            "shift",
            "123.123",
            "",
            -70,
            0,
            "0.0000000000000000000000000000000000000000000000000000000000000000000123123",
            "",
        ),
        (
            "shift",
            "123.123",
            "",
            -78,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000123123",
            "",
        ),
        (
            "shift",
            "123.123",
            "",
            -79,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000012312",
            "Truncated",
        ),
        (
            "shift",
            "123.123",
            "",
            -80,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000001231",
            "Truncated",
        ),
        (
            "shift",
            "123.123",
            "",
            -81,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000123",
            "Truncated",
        ),
        (
            "shift",
            "123.123",
            "",
            -82,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000012",
            "Truncated",
        ),
        (
            "shift",
            "123.123",
            "",
            -83,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000001",
            "Truncated",
        ),
        ("shift", "123.123", "", -84, 0, "0", "Truncated"),
        (
            "shift",
            ".000000000123",
            "",
            -64,
            0,
            "0.0000000000000000000000000000000000000000000000000000000000000000000000000123",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            -69,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000123",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            -70,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000012",
            "Truncated",
        ),
        (
            "shift",
            ".000000000123",
            "",
            -71,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000001",
            "Truncated",
        ),
        ("shift", ".000000000123", "", -72, 0, "0", "Truncated"),
        (
            "shift",
            ".000000000123",
            "",
            64,
            0,
            "1230000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            71,
            0,
            "12300000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            72,
            0,
            "123000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            73,
            0,
            "1230000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            80,
            0,
            "12300000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            81,
            0,
            "123000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            82,
            0,
            "1230000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            83,
            0,
            "12300000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            84,
            0,
            "123000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            85,
            0,
            "1230000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            86,
            0,
            "12300000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            87,
            0,
            "123000000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            88,
            0,
            "1230000000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            89,
            0,
            "12300000000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            90,
            0,
            "123000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            ".000000000123",
            "",
            91,
            0,
            "0.000000000123",
            "Overflow",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -64,
            0,
            "0.0000000000000000000000000000000000000000000000000000000123456789987654321",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -65,
            0,
            "0.00000000000000000000000000000000000000000000000000000000123456789987654321",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -71,
            0,
            "0.00000000000000000000000000000000000000000000000000000000000000123456789987654321",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -72,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000123456789987654321",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -73,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000012345678998765432",
            "Truncated",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -80,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000001234567900",
            "Truncated",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -81,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000123456790",
            "Truncated",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -82,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000012345679",
            "Truncated",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            -89,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000001",
            "Truncated",
        ),
        ("shift", "123456789.987654321", "", -90, 0, "0", "Truncated"),
        (
            "shift",
            "123456789.987654321",
            "",
            64,
            0,
            "1234567899876543210000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            65,
            0,
            "12345678998765432100000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            67,
            0,
            "1234567899876543210000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            71,
            0,
            "12345678998765432100000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            72,
            0,
            "123456789987654321000000000000000000000000000000000000000000000000000000000000000",
            "",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            73,
            0,
            "123456789.987654321",
            "Overflow",
        ),
        (
            "shift",
            "123456789.987654321",
            "",
            0,
            0,
            "123456789.987654321",
            "",
        ),
        ("parse", "12345", "", 0, 0, "12345", ""),
        ("parse", "12345.", "", 0, 0, "12345", ""),
        ("parse", "123.45.", "", 0, 0, "123.45", "Truncated"),
        ("parse", "-123.45.", "", 0, 0, "-123.45", "Truncated"),
        (
            "parse",
            ".00012345000098765",
            "",
            0,
            0,
            "0.00012345000098765",
            "",
        ),
        ("parse", ".12345000098765", "", 0, 0, "0.12345000098765", ""),
        (
            "parse",
            "-.000000012345000098765",
            "",
            0,
            0,
            "-0.000000012345000098765",
            "",
        ),
        ("parse", "1234500009876.5", "", 0, 0, "1234500009876.5", ""),
        ("parse", "123E5", "", 0, 0, "12300000", ""),
        ("parse", "123E-2", "", 0, 0, "1.23", ""),
        (
            "parse",
            "1e1073741823",
            "",
            0,
            0,
            "999999999999999999999999999999999999999999999999999999999999999999999999999999999",
            "Overflow",
        ),
        (
            "parse",
            "-1e1073741823",
            "",
            0,
            0,
            "-999999999999999999999999999999999999999999999999999999999999999999999999999999999",
            "Overflow",
        ),
        (
            "parse",
            "1e18446744073709551620",
            "",
            0,
            0,
            "0",
            "BadNumber",
        ),
        ("parse", "1e", "", 0, 0, "1", "Truncated"),
        ("parse", "1e001", "", 0, 0, "10", ""),
        ("parse", "1e00", "", 0, 0, "1", ""),
        ("parse", "1eabc", "", 0, 0, "1", "Truncated"),
        ("parse", "1e 1dddd ", "", 0, 0, "10", "Truncated"),
        ("parse", "1e - 1", "", 0, 0, "1", "Truncated"),
        ("parse", "1e -1", "", 0, 0, "0.1", ""),
        (
            "parse",
            "0.00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
            0,
            0,
            "0.000000000000000000000000000000000000000000000000000000000000000000000000",
            "Truncated",
        ),
        ("parse", "1asf", "", 0, 0, "1", "Truncated"),
        ("parse", "1.1.1.1.1", "", 0, 0, "1.1", "Truncated"),
        ("parse", "1  1", "", 0, 0, "1", "Truncated"),
        ("parse", "1  ", "", 0, 0, "1", ""),
        (
            "parse",
            "123450000098765000000000000000000000000000000000000000000000000000000000000000000000000",
            "",
            0,
            0,
            "98765000000000000000000000000000000000000000000000000000000000000000000000000",
            "Overflow",
        ),
        (
            "parse",
            "123450000000000000000000000000000000000000000000000000000000000000000000000000.000098765",
            "",
            0,
            0,
            "123450000000000000000000000000000000000000000000000000000000000000000000000000",
            "Truncated",
        ),
    ] {
        let mut a = MyDecimal::default();
        let mut b = MyDecimal::default();
        let mut out = MyDecimal::default();
        if op != "parse" {
            assert_eq!(a.FromString(input.as_bytes()), nil, "{input}");
        }
        if !rhs.is_empty() {
            assert_eq!(b.FromString(rhs.as_bytes()), nil);
        }
        let status = match op {
            "parse" => out.FromString(input.as_bytes()),
            "shift" => {
                out = a.clone();
                out.Shift(n)
            }
            "round" => a.Round(&mut out, n, mode as i32),
            "add" => DecimalAdd(&a, &b, &mut out),
            "sub" => DecimalSub(&a, &b, &mut out),
            "mul" => DecimalMul(&a, &b, &mut out),
            "bin" => {
                let (bytes, status) = a.ToBin(n, mode);
                assert_eq!(out.FromBin(&bytes, n, mode).1, nil);
                status
            }
            _ => unreachable!(),
        };
        assert_eq!(
            status.err().map(|e| format!("{e:?}")).unwrap_or_default(),
            error,
            "{op} {input} {rhs} {n}"
        );
        let text = if op == "mul" {
            out.String()
        } else {
            string(out.ToString())
        };
        assert_eq!(text, expected, "{op} {input} {rhs} {n}");
    }
}

#[test]
fn audit_product_guard_digits_survive_subsequent_operations() {
    let tiny = dec_from("0.0000000000000001");
    let mut product = MyDecimal::default();
    assert_eq!(DecimalMul(&tiny, &tiny, &mut product), nil);
    assert_eq!(product.digitsFrac, 31);
    assert_eq!(product.Compare(&MyDecimal::default()), 1);
    assert_eq!(product.ToInt(), (0, ErrTruncated));
    let rhs = dec_from("0.00000000000000000000000000000001");
    let mut sum = MyDecimal::default();
    assert_eq!(DecimalAdd(&product, &rhs, &mut sum), nil);
    assert_eq!(sum.ToString(), b"0.00000000000000000000000000000002");
}

#[test]
fn audit_maximum_constructor_respects_word_capacity() {
    let d = NewMaxOrMinDec(false, 81, 1);
    assert_eq!(d.ToString(), "9".repeat(80).as_bytes());
}

#[test]
fn audit_inactive_product_words_do_not_hide_division_by_zero() {
    let tiny = dec_from("0.00000000000000000001");
    let mut divisor = MyDecimal::default();
    assert_eq!(DecimalMul(&tiny, &tiny, &mut divisor), nil);
    let mut result = MyDecimal::default();
    assert_eq!(
        DecimalDiv(&NewDecFromInt(1), &divisor, &mut result, 4),
        ErrDivByZero
    );
    assert_eq!(
        DecimalMod(&NewDecFromInt(1), &divisor, &mut result),
        ErrDivByZero
    );
}

#[test]
fn audit_shift_keeps_go_guard_and_unused_words() {
    let a = dec_from(".999999999");
    let b = dec_from("1.000000000000000000000000000001");
    let mut d = MyDecimal::default();
    assert_eq!(DecimalMul(&a, &b, &mut d), nil);
    assert_eq!(d.Shift(1), nil);
    assert_eq!(d.wordBuf, [9, 999999990, 0, 0, 9999990, 999000000, 0, 0, 0]);
}

#[test]
fn audit_full_go_division_and_modulo_tables() {
    for (op, a, b, increment, display, expected, error) in [
        ("div", "120", "10", 5, false, "12.000000000", nil),
        ("div", "123", "0.01", 5, false, "12300.000000000", nil),
        (
            "div",
            "120",
            "100000000000.00000",
            5,
            false,
            "0.000000001200000000",
            nil,
        ),
        ("div", "123", "0", 5, false, "", ErrDivByZero),
        ("div", "0", "0", 5, false, "", ErrDivByZero),
        (
            "div",
            "-12193185.1853376",
            "98765.4321",
            5,
            false,
            "-123.456000000000000000",
            nil,
        ),
        (
            "div",
            "121931851853376",
            "987654321",
            5,
            false,
            "123456.000000000",
            nil,
        ),
        ("div", "0", "987", 5, false, "0.00000", nil),
        ("div", "1", "3", 5, false, "0.333333333", nil),
        (
            "div",
            "1.000000000000",
            "3",
            5,
            false,
            "0.333333333333333333",
            nil,
        ),
        ("div", "1", "1", 5, false, "1.000000000", nil),
        (
            "div",
            "0.0123456789012345678912345",
            "9999999999",
            5,
            false,
            "0.000000000001234567890246913578148141",
            nil,
        ),
        (
            "div",
            "10.333000000",
            "12.34500",
            5,
            false,
            "0.837019036046982584042122316",
            nil,
        ),
        (
            "div",
            "10.000000000060",
            "2",
            5,
            false,
            "5.000000000030000000",
            nil,
        ),
        (
            "div",
            "51",
            "0.003430",
            5,
            false,
            "14868.804664723032069970",
            nil,
        ),
        ("mod", "234", "10", 0, false, "4", nil),
        ("mod", "234.567", "10.555", 0, false, "2.357", nil),
        ("mod", "-234.567", "10.555", 0, false, "-2.357", nil),
        ("mod", "234.567", "-10.555", 0, false, "2.357", nil),
        (
            "mod",
            "99999999999999999999999999999999999999",
            "3",
            0,
            false,
            "0",
            nil,
        ),
        ("mod", "51", "0.003430", 0, false, "0.002760", nil),
        ("mod", "0.0000000001", "1.0", 0, false, "0.0000000001", nil),
        ("mod", "0.000", "0.1", 0, false, "0.000", nil),
        ("div", "1", "1", 4, true, "1.0000", nil),
        ("div", "1.00", "1", 4, true, "1.000000", nil),
        ("div", "1", "1.000", 4, true, "1.0000", nil),
        ("div", "2", "3", 4, true, "0.6667", nil),
        ("div", "51", "0.003430", 4, true, "14868.8047", nil),
        ("div", "0.000", "0.1", 4, true, "0.0000000", nil),
        ("mod", "1", "2.0", 0, true, "1.0", nil),
        ("mod", "1.0", "2", 0, true, "1.0", nil),
        ("mod", "2.23", "3", 0, true, "2.23", nil),
        ("mod", "51", "0.003430", 0, true, "0.002760", nil),
    ] {
        let a = dec_from(a);
        let b = dec_from(b);
        let mut out = MyDecimal::default();
        let status = if op == "div" {
            DecimalDiv(&a, &b, &mut out, increment)
        } else {
            DecimalMod(&a, &b, &mut out)
        };
        assert_eq!(status, error);
        if error != ErrDivByZero {
            let actual = if display {
                out.String()
            } else {
                string(out.ToString())
            };
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn audit_clone_hash_size_and_binary_prefix() {
    assert_eq!(std::mem::size_of::<MyDecimal>(), MyDecimalStructSize);
    let original = dec_from("001.2300");
    assert_eq!((original.GetDigitsInt(), original.GetDigitsFrac()), (3, 4));
    let mut cloned = original.Clone();
    cloned.wordBuf[0] = 9;
    assert_eq!(original.ToString(), b"1.2300");
    let key = original.ToHashKey().unwrap();
    assert_eq!(original.HashKeySize(), Ok(key.len()));
    let (encoded, status) = original.ToBin(5, 4);
    assert_eq!(status, nil);
    let (appended, status) = original.WriteBin(5, 4, vec![0xaa, 0xbb]);
    assert_eq!(status, nil);
    assert_eq!(&appended[..2], &[0xaa, 0xbb]);
    assert_eq!(&appended[2..], &encoded);
    let mut packet = vec![5, 4];
    packet.extend_from_slice(&encoded);
    assert_eq!(DecimalPeak(&packet), Ok(packet.len()));
    assert_eq!(DecimalPeak(&[5, 4]), Err(DecimalError::BadNumber));
}
