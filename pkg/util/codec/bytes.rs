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

// Bytes 的 memcomparable / compact 编解码。
//
// 对应 Go `pkg/util/codec/bytes.go`。Memcomparable 格式按 8 字节分组 + marker，
// 保证编码后字节可直接字典序比较；`EncodeBytesDesc` 通过对编码结果按位取反实现降序。
// Compact 格式用变长长度前缀，空间更省但不可比较。

/// 每组数据字节数（MyRocks memcomparable 固定为 8）。
pub const encGroupSize: usize = 8;
/// Marker 基准值：`0xFF - padCount`。
pub const encMarker: u8 = 0xFF;
/// 组内填充字节（升序编码用 0）。
pub const encPad: u8 = 0x0;

// pads 对应 Go 的全零 padding 切片。
/// 升序编码时用于补齐不足 8 字节的全零 padding。
pub static pads: [u8; encGroupSize] = [encPad; encGroupSize];

// EncodeBytes guarantees the encoded value is in ascending order for comparison,
// encoding with the following rule:
//     [group1][marker1]...[groupN][markerN]
//     group is 8 bytes slice which is padding with 0.
//     marker is `0xFF - padding 0 count`
// For example:
//     [] -> [0, 0, 0, 0, 0, 0, 0, 0, 247]
//     [1, 2, 3] -> [1, 2, 3, 0, 0, 0, 0, 0, 250]
//     [1, 2, 3, 0] -> [1, 2, 3, 0, 0, 0, 0, 0, 251]
//     [1, 2, 3, 4, 5, 6, 7, 8] -> [1, 2, 3, 4, 5, 6, 7, 8, 255, 0, 0, 0, 0, 0, 0, 0, 0, 247]
// Refer: https://github.com/facebook/mysql-5.6/wiki/MyRocks-record-format#memcomparable-format
// Go 版本按 8 字节一组追加 marker；idx <= dLen 会让整组输入额外追加一个终止组。
/// 将 `data` 编码为升序可比较的字节序列并追加到 `b`。
pub fn EncodeBytes(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
    // Allocate more space to avoid unnecessary slice growing.
    // Assume that the byte slice size is about `(len(data) / encGroupSize + 1) * (encGroupSize + 1)` bytes,
    // that is `(len(data) / 8 + 1) * 9` in our implement.
    let dLen = data.len();
    let reallocSize = (dLen / encGroupSize + 1) * (encGroupSize + 1);
    let mut result = reallocBytes(b, reallocSize);

    let mut idx = 0;
    while idx <= dLen {
        let remain = dLen - idx;
        let mut padCount = 0usize;

        if remain >= encGroupSize {
            result.extend_from_slice(&data[idx..idx + encGroupSize]);
        } else {
            // 不足一组时用零填充，padCount 写入 marker。
            padCount = encGroupSize - remain;
            result.extend_from_slice(&data[idx..]);
            result.extend_from_slice(&pads[..padCount]);
        }

        let marker = encMarker - padCount as u8;
        result.push(marker);
        idx += encGroupSize;
    }

    result
}

// EncodeBytesExt is an extension of `EncodeBytes`, which will not encode for `isRawKv = true` but just append `data` to `b`.
/// Raw KV 模式下直接追加原始字节，否则走 `EncodeBytes`。
pub fn EncodeBytesExt(mut b: Vec<u8>, data: &[u8], isRawKv: bool) -> Vec<u8> {
    if isRawKv {
        b.extend_from_slice(data);
        return b;
    }
    EncodeBytes(b, data)
}

// EncodedBytesLength returns the length of data after encoded
/// 计算 `EncodeBytes` 编码后的字节长度（含 padding 与 marker）。
pub fn EncodedBytesLength(dataLen: usize) -> usize {
    let modulo = dataLen % encGroupSize;
    let padCount = encGroupSize - modulo;
    dataLen + padCount + 1 + dataLen / encGroupSize
}

// decodeBytes 对应 Go 未导出函数：按 marker 校验 padding，并返回剩余输入和解码值。
// reverse=true 时处理降序编码，先按反向 marker 解析，最后对解码结果逐字节取反。
/// 按组解码 bytes；`reverse` 为 true 时按降序编码规则解析。
pub fn decodeBytes(
    mut b: &[u8],
    buf: Option<Vec<u8>>,
    reverse: bool,
) -> Result<(&[u8], Vec<u8>), errors::SharedError> {
    let mut buf = buf.unwrap_or_else(|| Vec::with_capacity(b.len()));
    buf.clear();

    loop {
        if b.len() < encGroupSize + 1 {
            return Err(errors::New("insufficient bytes to decode value"));
        }

        let groupBytes = &b[..encGroupSize + 1];
        let group = &groupBytes[..encGroupSize];
        let marker = groupBytes[encGroupSize];

        let padCount = if reverse { marker } else { encMarker - marker };
        if padCount as usize > encGroupSize {
            return Err(errors::Errorf(format!(
                "invalid marker byte, group bytes {:?}",
                groupBytes
            )));
        }

        let realGroupSize = encGroupSize - padCount as usize;
        buf.extend_from_slice(&group[..realGroupSize]);
        b = &b[encGroupSize + 1..];

        if padCount != 0 {
            let padByte = if reverse { encMarker } else { encPad };
            // Check validity of padding bytes.
            for v in &group[realGroupSize..] {
                if *v != padByte {
                    return Err(errors::Errorf(format!(
                        "invalid padding byte, group bytes {:?}",
                        groupBytes
                    )));
                }
            }
            break;
        }
    }

    if reverse {
        reverseBytes(&mut buf);
    }
    Ok((b, buf))
}

// DecodeBytes decodes bytes which is encoded by EncodeBytes before,
// returns the leftover bytes and decoded value if no error.
// `buf` is used to buffer data to avoid the cost of makeslice in decodeBytes when DecodeBytes is called by Decoder.DecodeOne.
/// 解码 `EncodeBytes` 的结果，返回剩余输入与解码值。
pub fn DecodeBytes(
    b: &[u8],
    buf: Option<Vec<u8>>,
) -> Result<(&[u8], Vec<u8>), errors::SharedError> {
    decodeBytes(b, buf, false)
}

// EncodeBytesDesc first encodes bytes using EncodeBytes, then bitwise reverses
// encoded value to guarantee the encoded value is in descending order for comparison.
/// 先升序编码再按位取反，得到降序可比较编码。
pub fn EncodeBytesDesc(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
    let n = b.len();
    b = EncodeBytes(b, data);
    reverseBytes(&mut b[n..]);
    b
}

// DecodeBytesDesc decodes bytes which is encoded by EncodeBytesDesc before,
// returns the leftover bytes and decoded value if no error.
/// 解码 `EncodeBytesDesc` 的结果。
pub fn DecodeBytesDesc(
    b: &[u8],
    buf: Option<Vec<u8>>,
) -> Result<(&[u8], Vec<u8>), errors::SharedError> {
    decodeBytes(b, buf, true)
}

// EncodeCompactBytes joins bytes with its length into a byte slice. It is more
// efficient in both space and time compare to EncodeBytes. Note that the encoded
// result is not memcomparable.
/// 用变长长度前缀拼接原始字节（不可比较，但更紧凑）。
pub fn EncodeCompactBytes(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
    b = reallocBytes(b, binary::MaxVarintLen64 + data.len());
    b = EncodeVarint(b, data.len() as i64);
    b.extend_from_slice(data);
    b
}

// DecodeCompactBytes decodes bytes which is encoded by EncodeCompactBytes before.
/// 解码 `EncodeCompactBytes` 的结果，返回 leftover 与 payload 切片。
pub fn DecodeCompactBytes(b: &[u8]) -> Result<(&[u8], &[u8]), errors::SharedError> {
    let (b, n) = DecodeVarint(b)?;
    if n < 0 || (b.len() as i64) < n {
        return Err(errors::Errorf(format!(
            "insufficient bytes to decode value, expected length: {}",
            n
        )));
    }
    Ok((&b[n as usize..], &b[..n as usize]))
}

// See https://golang.org/src/crypto/cipher/xor.go
/// 机器字大小，用于快速按字取反路径。
pub const wordSize: usize = std::mem::size_of::<usize>();

// supportsUnaligned 对应 Go 的 runtime.GOARCH 判断；用 cfg 表达 386/amd64 的意图。
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
/// 是否支持非对齐按字访问（x86/x86_64 为 true）。
pub const supportsUnaligned: bool = true;
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
/// 是否支持非对齐按字访问（非 x86 为 false）。
pub const supportsUnaligned: bool = false;

// fastReverseBytes 保留 Go 中 unsafe 按 machine word 取反的快路径。
// 这里用 chunks_exact_mut 模拟 word 批处理，避免在这里直接转裸 slice 头。
/// 按机器字批量按位取反（快路径）。
pub fn fastReverseBytes(b: &mut [u8]) {
    let mut chunks = b.chunks_exact_mut(wordSize);
    for word in &mut chunks {
        for v in word {
            *v = !*v;
        }
    }

    for v in chunks.into_remainder() {
        *v = !*v;
    }
}

/// 逐字节按位取反（安全路径）。
pub fn safeReverseBytes(b: &mut [u8]) {
    for v in b {
        *v = !*v;
    }
}

/// 按架构选择快路径或安全路径做按位取反。
pub fn reverseBytes(b: &mut [u8]) {
    if supportsUnaligned {
        fastReverseBytes(b);
        return;
    }

    safeReverseBytes(b);
}

// reallocBytes is like realloc.
/// 确保 `b` 还能再容纳 `n` 字节，不足则拷贝到更大容量的新 Vec。
pub fn reallocBytes(mut b: Vec<u8>, n: usize) -> Vec<u8> {
    let newSize = b.len() + n;
    if b.capacity() < newSize {
        let mut bs = Vec::with_capacity(newSize);
        bs.extend_from_slice(&b);
        return bs;
    }

    // slice b has capability to store n bytes
    b
}
