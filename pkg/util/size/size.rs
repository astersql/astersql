// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 二进制容量单位与常见 Go 类型头部大小常量。
//
// 由 `pkg/util/size/size.go` 迁移。容量常量按 1024 进制递进；
// SizeOf* 对应 Go `unsafe.Sizeof` 语义，用于内存追踪估算，
// 对 slice/string/interface/map 等只计值头部，不计底层数据。

#![allow(non_upper_case_globals)]

// Keep the exported Go constant names so migrated callers can use the same API.

/// 千字节（KiB）：1024 字节，作为后续容量单位基准。
// KB is the kilobytes.
// KB 对应 Go 中的 uint64(1024)，作为后续容量单位常量的基准。
pub const KB: u64 = 1024;

/// 兆字节（MiB）：KB × 1024。
// MB is the megabytes.
// MB 保持 Go 的声明顺序，由 KB 逐级放大。
pub const MB: u64 = KB * 1024;

/// 吉字节（GiB）：MB × 1024。
// GB is the gigabytes.
// GB 保持 Go 中 MB * 1024 的换算关系。
pub const GB: u64 = MB * 1024;

/// 太字节（TiB）：GB × 1024。
// TB is the terabytes.
// TB 保持 Go 中 GB * 1024 的换算关系。
pub const TB: u64 = GB * 1024;

/// 拍字节（PiB）：TB × 1024。
// PB is the petabytes.
// PB 保持 Go 中 TB * 1024 的换算关系。
pub const PB: u64 = TB * 1024;

// below constants are the size of commonly used types, for memory trace
// 以下常量对应 Go 第二个 const 分组，用于估算常见 Go 类型“值本身”占用的内存。
// 注意这里迁移的是 Go 的 unsafe.Sizeof 语义：slice、string、interface 等记录的是 Go 头部大小，
// 不包含它们指向的底层元素、字符串字节或动态具体类型的额外内存。

/// Go slice 值头部大小（data/len/cap 三字），不含元素数组。
// SizeOfSlice is the memory itself used, excludes the elements' memory
// Go slice 值本身是 data/len/cap 三个机器字；这里按三个 usize 迁移头部大小，不计算元素数组。
pub const SizeOfSlice: i64 = std::mem::size_of::<[usize; 3]>() as i64;

/// 单字节（Go byte / uint8）大小。
// SizeOfByte is the memory each byte occupied
// Go byte 是 uint8 的别名，迁移为 Rust 的 u8 单字节大小。
pub const SizeOfByte: i64 = std::mem::size_of::<u8>() as i64;

/// Go string 值头部大小（data/len 两字），不含字符串内容。
// SizeOfString is the memory string itself occupied
// Go string 值本身是 data/len 两个机器字；这里保留头部大小，不包含实际字符串内容字节。
pub const SizeOfString: i64 = std::mem::size_of::<[usize; 2]>() as i64;

/// 单个 bool 占用大小。
// SizeOfBool is the memory each bool occupied
// Go bool 的 unsafe.Sizeof 结果按单个布尔值迁移；Rust bool 通常同样为 1 字节。
pub const SizeOfBool: i64 = std::mem::size_of::<bool>() as i64;

/// 指针值大小（一个机器字）。
// SizeOfPointer is the memory each pointer occupied
// Go new(int) 返回 *int，unsafe.Sizeof 计算的是指针值大小；用裸指针表示该机器字。
pub const SizeOfPointer: i64 = std::mem::size_of::<*const isize>() as i64;

/// Go interface/any 值头部大小（类型信息 + 数据指针），不含动态具体值。
// SizeOfInterface is the memory each interface occupied, exclude the real type memory usage
// Go any/interface{} 值本身包含类型信息和数据指针两个机器字，不包含实际动态值的内存。
pub const SizeOfInterface: i64 = std::mem::size_of::<[usize; 2]>() as i64;

/// float64 / f64 标量大小。
// SizeOfFloat64 is the memory each float64 occupied
// Go float64 机械对应 Rust f64 的标量大小。
pub const SizeOfFloat64: i64 = std::mem::size_of::<f64>() as i64;

/// uint64 / u64 标量大小。
// SizeOfUint64 is the memory each uint64 occupied
// Go uint64 机械对应 Rust u64 的标量大小。
pub const SizeOfUint64: i64 = std::mem::size_of::<u64>() as i64;

/// int32 / i32 标量大小。
// SizeOfInt32 is the memory each int32 occupied
// Go int32 机械对应 Rust i32 的标量大小。
pub const SizeOfInt32: i64 = std::mem::size_of::<i32>() as i64;

/// 平台字宽有符号整数（Go int / isize）大小。
// SizeOfInt is the memory each int occupied
// Go int 是平台字宽整数；用 isize 表达同样随目标平台变化的机器字语义。
pub const SizeOfInt: i64 = std::mem::size_of::<isize>() as i64;

/// uint8 / u8 标量大小。
// SizeOfUint8 is the memory each uint8 occupied
// Go uint8 机械对应 Rust u8 的标量大小。
pub const SizeOfUint8: i64 = std::mem::size_of::<u8>() as i64;

/// 平台字宽无符号整数（Go uint / usize）大小。
// SizeOfUint is the memory each uint occupied
// Go uint 是平台字宽无符号整数；用 usize 表达同样随目标平台变化的机器字语义。
pub const SizeOfUint: i64 = std::mem::size_of::<usize>() as i64;

/// Go func 值本身的机器字大小。
// SizeOfFunc is the memory each function occupied
// Go func 值的 unsafe.Sizeof 记录函数值本身的机器字大小；用 fn() 指针占位表达。
pub const SizeOfFunc: i64 = std::mem::size_of::<fn()>() as i64;

/// int64 / i64 标量大小。
// SizeOfInt64 is the memory each int64 occupied
// Go int64 机械对应 Rust i64 的标量大小。
pub const SizeOfInt64: i64 = std::mem::size_of::<i64>() as i64;

/// Go map 值本身（指向 hmap 的引用）大小，不含桶与键值数据。
// SizeOfMap is the memory each map itself occupied
// Go map 值本身是指向运行时 hmap 的引用，不包含桶和键值数据；用裸指针大小表达该语义。
pub const SizeOfMap: i64 = std::mem::size_of::<*const ()>() as i64;
