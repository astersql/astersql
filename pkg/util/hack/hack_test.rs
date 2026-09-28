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

// `hack` 包零拷贝字符串/字节视图的单元测试。
//
// 对应 Go `pkg/util/hack/hack_test.go`。验证 `String`/`Slice`/`MutableString`
// 在不复制底层内存时的别名语义与修改可见性。

use super::hack::{GetBytesFromPtr, Slice, String as HackString};

// TestString 对应 Go 的 String([]byte) 零拷贝转换测试。
#[test]
/// 验证 `String([]byte)` 零拷贝：共享底层数组，原地修改后字符串可见变化。
fn TestString() {
    let mut b = Vec::from("hello world".as_bytes());
    let data = b.as_mut_ptr();
    let a = unsafe { HackString(std::slice::from_raw_parts(data, b.len())) };

    if a != "hello world" {
        panic!("{}", a);
    }

    // Go 的 String 返回与 []byte 共享底层数组的 mutable string；修改 b[0] 后 a 也应观察到变化。
    unsafe { *data = b'a' };

    if a != "aello world" {
        panic!("{}", a);
    }

    // Rust 不能在仍有引用时释放旧 allocation，因此用新 Vec 表达 Go append
    // 改变 slice header、但旧字符串仍引用旧 backing array 的语义。
    let mut appended = b.clone();
    appended.extend_from_slice(b"abc");
    if a != "aello world" {
        panic!("a:{}, b:{:?}", a, appended);
    }
}

// TestByte 对应 Go 的 Slice(string) 零拷贝字节视图测试。
#[test]
/// 验证 `Slice(string)` 零拷贝字节视图与字节级相等比较。
fn TestByte() {
    let a = "hello world";

    let b = Slice(a);

    // Go 使用 bytes.Equal；Rust 用切片相等保留同样的字节级比较。
    if b != b"hello world" {
        panic!("{}", String::from_utf8_lossy(b));
    }
}

// TestMutable 对应 Go 对 MutableString 别名风险的显式验证。
#[test]
/// 验证 MutableString 别名风险：底层字节被写入后，所有别名字符串同步变化。
fn TestMutable() {
    let mut a = vec![b'a', b'b', b'c'];
    let data = a.as_mut_ptr();
    let b = unsafe { HackString(std::slice::from_raw_parts(data, a.len())) }; // b is a mutable string.
    let c = b; // Warn, c is a mutable string
    if c != "abc" {
        panic!("assert fail");
    }

    // c changed after a is modified. 通过裸指针执行写入，显式保留 Go unsafe
    // 别名风险，同时避免把这种能力扩散到生产 API。
    unsafe { *data = b's' };
    if c != "sbc" {
        panic!("test mutable string fail");
    }
}

// Go unsafe.Slice accepts a nil pointer when the requested length is zero.
#[test]
fn get_bytes_from_null_ptr_with_zero_length_is_empty() {
    let bytes = unsafe { GetBytesFromPtr(std::ptr::null(), 0) };

    assert!(bytes.is_empty());
}
