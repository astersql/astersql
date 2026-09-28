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

// DECIMAL 编解码：将 MySQL DECIMAL（定点数）写成可按字节序比较的二进制，并支持反向解码。
//
// 对应 Go `pkg/util/codec/decimal.go`。编码布局为：`precision`、`frac` 各一字节，
// 后接 `MyDecimal::WriteBin` 写出的定长二进制体。`precision` 为总有效位数，
// `frac` 为小数位数；二者为 0 时从 decimal 自身推导。

/// 将 DECIMAL 编码进字节切片，结果可按字典序比较（memcomparable）。
///
/// `precision`/`frac` 为 0 时使用 decimal 自身的 PrecisionAndFrac；
/// `frac` 超过 MySQL 最大小数位时截断到上限。
// EncodeDecimal encodes a decimal into a byte slice which can be sorted lexicographically later.
pub fn EncodeDecimal(
    mut b: Vec<u8>,
    dec: &types::MyDecimal,
    mut precision: i32,
    mut frac: i32,
) -> Result<Vec<u8>, errors::SharedError> {
    // precision 未指定时从 decimal 自身取总位数与小数位
    if precision == 0 {
        let pair = dec.PrecisionAndFrac();
        precision = pair.0 as i32;
        frac = pair.1 as i32;
    }
    // 小数位不得超过 MySQL MaxDecimalScale
    if frac > mysql::MaxDecimalScale as i32 {
        frac = mysql::MaxDecimalScale as i32;
    }
    // 先写精度与小数位头，再写二进制体
    b.push(precision as u8);
    b.push(frac as u8);
    let (next, status) = dec.WriteBin(precision as isize, frac as isize, b);
    status
        .map(|_| next)
        .map_err(|error| errors::New(error.to_string()))
}

/// 估算编码后占用字节数：二进制体长度 + 2 字节头（precision/frac）。
pub(crate) fn valueSizeOfDecimal(
    dec: &types::MyDecimal,
    mut precision: i32,
    mut frac: i32,
) -> Result<usize, errors::SharedError> {
    if precision == 0 {
        let pair = dec.PrecisionAndFrac();
        precision = pair.0 as i32;
        frac = pair.1 as i32;
    }
    let bin_size = types::DecimalBinSize(precision as isize, frac as isize)
        .map_err(|error| errors::New(error.to_string()))?;
    Ok(bin_size + 2)
}

/// 从字节切片解码 DECIMAL，返回剩余未消费字节、decimal 值及精度/小数位。
///
/// 支持 gofail 注入点 `errorInDecodeDecimal`，便于故障注入测试。
// DecodeDecimal decodes bytes to decimal.
pub fn DecodeDecimal(b: &[u8]) -> Result<(&[u8], types::MyDecimal, i32, i32), errors::SharedError> {
    fail::fail_point!("errorInDecodeDecimal", |_| Err(errors::New("gofail error")));
    // 至少需要 precision、frac 与 1 字节体
    if b.len() < 3 {
        return Err(errors::New("insufficient bytes to decode value"));
    }
    let precision = b[0] as i32;
    let frac = b[1] as i32;
    let body = &b[2..];
    let bin_size = types::DecimalBinSize(precision as isize, frac as isize)
        .map_err(|error| errors::New(error.to_string()))?;
    if body.len() < bin_size {
        return Err(errors::New("insufficient bytes to decode value"));
    }

    let mut dec = types::MyDecimal::default();
    let (consumed, status) = dec.FromBin(body, precision as isize, frac as isize);
    status.map_err(|error| errors::New(error.to_string()))?;
    Ok((&body[consumed..], dec, precision, frac))
}
