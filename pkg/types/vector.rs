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

// VECTOR(FLOAT32) 类型：小端线格式、解析、序列化与维度校验。
//
// 线布局与 Go 一致：首个 u32 存维度，其后为同序的 f32 分量；仅支持小端平台。
// 用于向量检索列存储与 SQL 文本 `[1.1,2.2]` 互转。

use crate::errors;
use goish::strconv;

#[cfg(not(target_endian = "little"))]
compile_error!("VectorFloat32 only supports little endian");

/// 启动时断言本机字节序与小端线格式假设一致。
pub fn init() {
    assert_eq!(u32::from_ne_bytes([0x02, 0x00, 0x00, 0x00]), 2);
}

/// 构造共享错误包装。
fn new_error(message: impl Into<String>) -> errors::SharedError {
    errors::New(message)
}

/// 按 Go strconv.FormatFloat 语义格式化浮点。
fn format_float(value: f64, format: u8, precision: i64, bit_size: i64) -> String {
    strconv::FormatFloat(value, format, precision, bit_size)
        .as_str()
        .to_owned()
}

/// Creates a vector and rejects the same non-finite values as the Go implementation.
/// 创建向量，并拒绝与 Go 相同的非有限值（NaN / Inf）。
pub fn CreateVectorFloat32(vector: &[f32]) -> Result<VectorFloat32, errors::SharedError> {
    for value in vector {
        if value.is_nan() {
            return Err(new_error("NaN not allowed in vector"));
        }
        if value.is_infinite() {
            return Err(new_error("infinite value not allowed in vector"));
        }
    }

    let mut result = InitVectorFloat32(vector.len() as i32);
    result.ElementsMut().copy_from_slice(vector);
    Ok(result)
}

/// 创建向量；失败则 panic（对应 Go Must* 辅助函数）。
pub fn MustCreateVectorFloat32(vector: &[f32]) -> VectorFloat32 {
    CreateVectorFloat32(vector).unwrap_or_else(|error| panic!("{error}"))
}

/// VectorFloat32 keeps the Go wire layout: one little-endian dimension word followed by f32 words.
/// Using aligned words makes the typed element views valid Rust while retaining zero-copy serialization.
/// 持有 Go 线格式：一个小端维度字 + 若干 f32 字；对齐存储以便零拷贝视图。
#[derive(Debug)]
pub struct VectorFloat32 {
    data: Vec<u32>,
}

/// 返回零维空向量。
pub fn ZeroVectorFloat32() -> VectorFloat32 {
    InitVectorFloat32(0)
}

/// 按维度分配向量，首字写入 dims，分量初始化为 0。
pub fn InitVectorFloat32(dims: i32) -> VectorFloat32 {
    assert!(dims >= 0, "vector dimensions must not be negative");
    let words = (dims as usize)
        .checked_add(1)
        .expect("vector allocation size overflow");
    let mut data = vec![0_u32; words];
    data[0] = dims as u32;
    VectorFloat32 { data }
}

/// 校验维度落在 `[0, 16383]`（VECTOR 类型上限）。
pub fn CheckVectorDimValid(dim: i32) -> Result<(), errors::SharedError> {
    const MAX_VECTOR_DIMENSION: i32 = 16_383;
    if dim < 0 {
        return Err(new_error("dimensions for type vector must be at least 0"));
    }
    if dim > MAX_VECTOR_DIMENSION {
        return Err(new_error(format!(
            "vector cannot have more than {MAX_VECTOR_DIMENSION} dimensions"
        )));
    }
    Ok(())
}

impl VectorFloat32 {
    /// 检查当前维度是否匹配列定义的 flen；`UnspecifiedLength` 表示不限制。
    pub fn CheckDimsFitColumn(&self, expected_flen: i32) -> Result<(), errors::SharedError> {
        if expected_flen != crate::UnspecifiedLength && self.Len() != expected_flen {
            return Err(new_error(format!(
                "vector has {} dimensions, does not fit VECTOR({expected_flen})",
                self.Len()
            )));
        }
        Ok(())
    }

    /// 返回维度（线格式首字）。
    pub fn Len(&self) -> i32 {
        self.data[0] as i32
    }

    /// 以 f32 切片视图读取分量（与 u32 存储零拷贝 reinterpret）。
    pub fn Elements(&self) -> &[f32] {
        let words = &self.data[1..];
        // u32 and f32 have identical size/alignment, and every bit pattern is a valid f32.
        unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<f32>(), words.len()) }
    }

    /// 可变分量视图，供初始化与就地修改。
    pub fn ElementsMut(&mut self) -> &mut [f32] {
        let words = &mut self.data[1..];
        // See Elements: the allocation is u32-aligned, so this mutable view is aligned as well.
        unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<f32>(), words.len()) }
    }

    /// 截断展示：最多 5 个分量，多余以 `(N more)...` 标明。
    pub fn TruncatedString(&self) -> String {
        const MAX_DISPLAY_ELEMENTS: usize = 5;
        let elements = self.Elements();
        let displayed = elements.len().min(MAX_DISPLAY_ELEMENTS);
        let truncated = elements.len() - displayed;

        let mut output = String::with_capacity(2 + self.Len() as usize * 2);
        output.push('[');
        for (index, value) in elements[..displayed].iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(&format_float(*value as f64, b'g', 2, 32));
        }
        if truncated > 0 {
            output.push_str(&format!(",({truncated} more)..."));
        }
        output.push(']');
        output
    }

    /// 完整字符串表示，形如 `[1.1,2.2]`。
    pub fn String(&self) -> String {
        let mut output = String::with_capacity(2 + self.Len() as usize * 2);
        output.push('[');
        for (index, value) in self.Elements().iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(&format_float(*value as f64, b'f', -1, 32));
        }
        output.push(']');
        output
    }

    /// 零拷贝序列化为小端字节视图。
    pub fn ZeroCopySerialize(&self) -> &[u8] {
        // A u8 view has weaker alignment and every byte value is valid.
        unsafe {
            std::slice::from_raw_parts(
                self.data.as_ptr().cast::<u8>(),
                self.data.len() * std::mem::size_of::<u32>(),
            )
        }
    }

    /// 将序列化字节追加到 `output` 并返回。
    pub fn SerializeTo(&self, mut output: Vec<u8>) -> Vec<u8> {
        output.extend_from_slice(self.ZeroCopySerialize());
        output
    }

    /// 线格式总字节数。
    pub fn SerializedSize(&self) -> usize {
        self.data.len() * std::mem::size_of::<u32>()
    }

    /// 估算内存占用（结构体 + 底层缓冲）。
    pub fn EstimatedMemUsage(&self) -> usize {
        std::mem::size_of::<VectorFloat32>() + self.SerializedSize()
    }

    /// 深拷贝一份独立向量。
    pub fn Clone(&self) -> VectorFloat32 {
        VectorFloat32 {
            data: self.data.clone(),
        }
    }

    /// 零维向量视为“零值”。
    pub fn IsZeroValue(&self) -> bool {
        self.Len() == 0
    }
}

/// 窥探前缀是否构成合法 VectorFloat32，返回应消费的字节数。
pub fn PeekBytesAsVectorFloat32(bytes: &[u8]) -> Result<usize, errors::SharedError> {
    if bytes.len() < 4 {
        return Err(new_error(format!(
            "bad VectorFloat32 value header (len={})",
            bytes.len()
        )));
    }

    // 首 4 字节为维度；总长 = 4 + dims*4
    let elements = u32::from_le_bytes(bytes[..4].try_into().expect("four-byte header"));
    let total_data_size = u64::from(elements) * 4 + 4;
    if (bytes.len() as u64) < total_data_size {
        return Err(new_error(format!(
            "bad VectorFloat32 value (len={}, expected={total_data_size})",
            bytes.len()
        )));
    }
    Ok(total_data_size as usize)
}

/// Deserializes the Go wire format into aligned owned storage and returns the unconsumed suffix.
/// 反序列化 Go 线格式到对齐自有存储，并返回未消费后缀。
pub fn ZeroCopyDeserializeVectorFloat32(
    bytes: &[u8],
) -> Result<(VectorFloat32, &[u8]), errors::SharedError> {
    let length = PeekBytesAsVectorFloat32(bytes)?;
    let mut data = Vec::with_capacity(length / 4);
    for word in bytes[..length].chunks_exact(4) {
        data.push(u32::from_le_bytes(word.try_into().expect("four-byte word")));
    }
    Ok((VectorFloat32 { data }, &bytes[length..]))
}

/// 从 JSON 数组文本解析向量；拒绝 null、NaN、Inf 与超出 f32 范围的值。
pub fn ParseVectorFloat32(text: &str) -> Result<VectorFloat32, errors::SharedError> {
    if text.trim() == "null" {
        return Err(new_error(format!("Invalid vector text: {text}")));
    }

    let parsed: Vec<f64> = serde_json::from_str(text)
        .map_err(|_| new_error(format!("Invalid vector text: {text}")))?;
    let mut values = Vec::with_capacity(parsed.len());
    for value in parsed {
        if value.is_nan() {
            return Err(new_error("NaN not allowed in vector"));
        }
        if value.is_infinite() {
            return Err(new_error("infinite value not allowed in vector"));
        }
        if value < -(f32::MAX as f64) || value > f32::MAX as f64 {
            return Err(new_error(format!(
                "value {} out of range for float32",
                format_float(value, b'g', -1, 64)
            )));
        }
        values.push(value as f32);
    }

    let dim = i32::try_from(values.len()).map_err(|_| {
        new_error(format!(
            "vector cannot have more than {} dimensions",
            16_383
        ))
    })?;
    CheckVectorDimValid(dim)?;

    let mut result = InitVectorFloat32(dim);
    result.ElementsMut().copy_from_slice(&values);
    Ok(result)
}
