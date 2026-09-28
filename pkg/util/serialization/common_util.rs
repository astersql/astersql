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

// 序列化/反序列化公共常量：`interface{}` 类型码与各原生类型字节长度。
//
// 聚合算子（agg）溢出落盘（spill，内存不足时把中间结果写到磁盘）时，
// 用类型码标识载荷种类，并按本机布局（native endian）计算定长字段宽度，
// 保证与 Go 侧二进制格式一致。

use crate::types;
use std::mem::size_of;

// These types are used for serializing or deserializing interface type.
/// `interface{}` 类型码：布尔。
pub const BoolType: i64 = 0;
/// `interface{}` 类型码：有符号 64 位整数。
pub const Int64Type: i64 = 1;
/// `interface{}` 类型码：无符号 64 位整数。
pub const Uint64Type: i64 = 2;
/// `interface{}` 类型码：浮点（对应 Go float64）。
pub const FloatType: i64 = 3;
/// `interface{}` 类型码：字符串。
pub const StringType: i64 = 4;
/// `interface{}` 类型码：二进制 JSON（BinaryJSON）。
pub const BinaryJSONType: i64 = 5;
/// `interface{}` 类型码：不透明字节载荷（Opaque）。
pub const OpaqueType: i64 = 6;
/// `interface{}` 类型码：时间（types.Time）。
pub const TimeType: i64 = 7;
/// `interface{}` 类型码：时长（types.Duration）。
pub const DurationType: i64 = 8;

/// 接口类型码本身占用的字节数。
pub const InterfaceTypeCodeLen: i64 = 1;
/// JSON 类型码占用的字节数。
pub const JSONTypeCodeLen: i64 = 1;
/// bool 定长宽度。
pub const BoolLen: i64 = size_of::<bool>() as i64;
/// 单字节（u8）宽度。
pub const ByteLen: i64 = size_of::<u8>() as i64;
/// i8 定长宽度。
pub const Int8Len: i64 = size_of::<i8>() as i64;
/// u8 定长宽度。
pub const Uint8Len: i64 = size_of::<u8>() as i64;
/// Go `int` / Rust `isize` 本机宽度。
pub const IntLen: i64 = size_of::<isize>() as i64;
/// i32 定长宽度。
pub const Int32Len: i64 = size_of::<i32>() as i64;
/// u32 定长宽度。
pub const Uint32Len: i64 = size_of::<u32>() as i64;
/// i64 定长宽度。
pub const Int64Len: i64 = size_of::<i64>() as i64;
/// u64 定长宽度。
pub const Uint64Len: i64 = size_of::<u64>() as i64;
/// f32 定长宽度。
pub const Float32Len: i64 = size_of::<f32>() as i64;
/// f64 定长宽度。
pub const Float64Len: i64 = size_of::<f64>() as i64;
/// `types::Time` 定长宽度。
pub const TimeLen: i64 = size_of::<types::Time>() as i64;
/// Go `time.Duration`（纳秒 i64）定长宽度。
pub const TimeDurationLen: i64 = size_of::<i64>() as i64;
/// 不安全指针本机宽度（对齐 Go unsafe.Pointer）。
pub const UnsafePointerLen: i64 = size_of::<*const ()>() as i64;
