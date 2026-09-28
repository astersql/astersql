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

// kv::key 单元测试：键序、点区间、句柄、HandleMap 与内存感知映射。

use kv::Handle;
use kv_dependency as kv;
use std::any::Any;
use std::collections::HashMap;

/// 用 UTC 时区把 Datum 列表编码为键字节。
fn encode_key(datums: Vec<kv::types::Datum>) -> Vec<u8> {
    kv::codec::EncodeKey(chrono_tz::UTC, Vec::new(), datums).unwrap()
}

/// 构造含 int + string 两列的 CommonHandle。
fn common_handle(int_value: i64, string_value: &str) -> kv::CommonHandle {
    kv::NewCommonHandle(encode_key(vec![
        kv::types::NewIntDatum(int_value),
        kv::types::NewStringDatum(string_value.to_owned()),
    ]))
    .unwrap()
}

/// PrefixNext 应跳过同前缀的完整键，落在下一前缀之前。
#[test]
fn test_partial_next() {
    let key_a = kv::codec::EncodeValue(
        chrono_tz::UTC,
        Vec::new(),
        vec![
            kv::types::NewStringDatum("abc".to_owned()),
            kv::types::NewStringDatum("def".to_owned()),
        ],
    )
    .unwrap();
    let key_b = kv::codec::EncodeValue(
        chrono_tz::UTC,
        Vec::new(),
        vec![
            kv::types::NewStringDatum("abca".to_owned()),
            kv::types::NewStringDatum("def".to_owned()),
        ],
    )
    .unwrap();
    let seek_key = kv::codec::EncodeValue(
        chrono_tz::UTC,
        Vec::new(),
        vec![kv::types::NewStringDatum("abc".to_owned())],
    )
    .unwrap();

    // Next 只追加 0，仍小于完整两列键；PrefixNext 进位后越过 key_a。
    assert!(kv::Key(seek_key.clone()).Next().0 < key_a);
    let partial_next = kv::Key(seek_key).PrefixNext().0;
    assert!(partial_next > key_a);
    assert!(partial_next < key_b);
}

/// KeyRange::IsPoint 覆盖等长进位、追加 0、以及非法点区间等边界。
#[test]
fn test_is_point() {
    let cases = [
        (b"rowkey1".to_vec(), b"rowkey2".to_vec(), true),
        (b"rowkey1".to_vec(), b"rowkey3".to_vec(), false),
        (Vec::new(), vec![0], true),
        (vec![123, 123, 255, 255], vec![123, 124, 0, 0], true),
        (vec![123, 123, 255, 255], vec![123, 124, 0, 1], false),
        (vec![123, 123], vec![123, 123, 0], true),
        (vec![255], vec![0], false),
    ];
    for (start, end, expected) in cases {
        assert_eq!(
            expected,
            kv::KeyRange {
                StartKey: kv::Key(start),
                EndKey: kv::Key(end)
            }
            .IsPoint()
        );
    }
}

/// IsTxnRetryableError 对 None、可重试原型与普通错误的判定。
#[test]
fn test_basic_func() {
    assert!(!kv::IsTxnRetryableError(None));
    let retryable = kv::ErrTxnRetryable.FastGenByArgs(&[]);
    assert!(kv::IsTxnRetryableError(Some(&retryable)));
    let ordinary = kv::errors::New("test");
    assert!(!kv::IsTxnRetryableError(Some(&ordinary)));
}

/// IntHandle / CommonHandle / PartitionHandle 的编码、比较与相等语义。
#[test]
fn test_handle() {
    let integer = kv::IntHandle(100);
    assert!(integer.IsInt());
    let (_, decoded) = kv::codec::DecodeInt(&integer.Encoded()).unwrap();
    assert_eq!(integer.IntValue(), decoded);
    let next = integer.Next();
    assert_eq!(101, next.IntValue());
    assert!(!integer.Equal(next.as_ref()));
    assert_eq!(-1, integer.Compare(next.as_ref()));
    assert_eq!("100", integer.String());

    let common = common_handle(100, "abc");
    assert!(!common.IsInt());
    let common_next = common.Next();
    assert!(!common.Equal(common_next.as_ref()));
    assert_eq!(-1, common.Compare(common_next.as_ref()));
    assert_eq!(common.Encoded().len(), common_next.Encoded().len());
    assert_eq!(2, common.NumCols());
    let (_, first) = kv::codec::DecodeOne(&common.EncodedCol(0)).unwrap();
    assert_eq!(100, first.GetInt64());
    let (_, second) = kv::codec::DecodeOne(&common.EncodedCol(1)).unwrap();
    assert_eq!("abc", second.GetString());
    assert_eq!("{100, abc}", common.String());

    // 分区句柄与底层句柄在 Equal 上互相兼容。
    let partition_integer = kv::NewPartitionHandle(2, Box::new(integer));
    assert!(partition_integer.Equal(&integer));
    assert!(integer.Equal(&partition_integer));
    let partition_common = kv::NewPartitionHandle(1, common_next.Copy());
    assert!(partition_common.Equal(common_next.as_ref()));
    assert!(common_next.Equal(&partition_common));
}

/// 短编码 CommonHandle 补齐到 9 字节后，列切片仍基于原始编码内容。
#[test]
fn test_padding_handle() {
    let decimal = kv::types::NewDecFromInt(1);
    let encoded = encode_key(vec![kv::types::NewDecimalDatum(decimal)]);
    assert!(encoded.len() < 9);
    let handle = kv::NewCommonHandle(encoded.clone()).unwrap();
    assert_eq!(9, handle.Encoded().len());
    assert_eq!(encoded, handle.EncodedCol(0));
    let reconstructed = kv::NewCommonHandle(handle.Encoded()).unwrap();
    assert_eq!(handle.EncodedCol(0), reconstructed.EncodedCol(0));
}

/// 将 Option<&dyn Any> 断言并与期望值比较。
fn downcast_value<T: 'static + PartialEq + std::fmt::Debug>(value: Option<&dyn Any>, expected: T) {
    assert_eq!(
        Some(&expected),
        value.and_then(|value| value.downcast_ref::<T>())
    );
}

/// HandleMap 的 Set/Get/Delete、MemUsage、Len 与 Range 提前终止。
#[test]
fn test_handle_map() {
    let mut map = kv::NewHandleMap();
    let integer = kv::IntHandle(1);
    assert_eq!(kv::SizeofHandleMap, map.MemUsage());
    map.Set(&integer, Box::new(1_i32));
    downcast_value(map.Get(&integer), 1_i32);
    assert_eq!(
        kv::SizeofHandleMap + kv::size::SizeOfInt64 + kv::size::SizeOfInterface,
        map.MemUsage()
    );
    map.Delete(&integer);
    assert!(map.Get(&integer).is_none());
    assert_eq!(kv::SizeofHandleMap, map.MemUsage());

    let common = common_handle(100, "abc");
    map.Set(&common, Box::new("a".to_owned()));
    downcast_value(map.Get(&common), "a".to_owned());
    let expected = kv::SizeofHandleMap
        + kv::size::SizeOfString
        + common.Encoded().len() as i64
        + kv::SizeofStrHandleVal;
    assert_eq!(expected, map.MemUsage());
    map.Delete(&common);
    assert!(map.Get(&common).is_none());

    let common_2 = common_handle(101, "abc");
    let common_3 = common_handle(99, "def");
    map.Set(&common, Box::new("a".to_owned()));
    map.Set(&common_2, Box::new("b".to_owned()));
    map.Set(&common_3, Box::new("c".to_owned()));
    assert_eq!(3, map.Len());
    let mut count = 0;
    // 回调返回 false 时 Range 立即停止，只遍历前两项。
    map.Range(|handle, value| {
        count += 1;
        let actual = value.downcast_ref::<String>().unwrap();
        if handle.Equal(&common) {
            assert_eq!("a", actual);
        } else if handle.Equal(&common_2) {
            assert_eq!("b", actual);
        } else {
            assert_eq!("c", actual);
        }
        count != 2
    });
    assert_eq!(2, count);
}

/// 复合句柄编码字节序应严格落在 IntHandle 的 MIN/MAX 编码之间。
#[test]
fn test_common_handles_fit_int_handle_range() {
    let min = kv::IntHandle(i64::MIN).Encoded();
    let max = kv::IntHandle(i64::MAX).Encoded();
    let cases = vec![
        vec![
            kv::types::NewIntDatum(101),
            kv::types::NewStringDatum("abc".to_owned()),
        ],
        vec![
            kv::types::NewStringDatum("abc".to_owned()),
            kv::types::NewIntDatum(101),
        ],
        vec![
            kv::types::NewIntDatum(-101),
            kv::types::NewStringDatum("abc".to_owned()),
        ],
        vec![
            kv::types::NewIntDatum(i64::MIN),
            kv::types::NewIntDatum(i64::MAX),
        ],
        vec![kv::types::NewBytesDatum(vec![0xff, 0xff])],
        vec![kv::types::NewBytesDatum(vec![0x00, 0x00])],
        vec![kv::types::NewBinaryLiteralDatum(kv::types::BinaryLiteral(
            vec![0xff, 0xff],
        ))],
    ];
    for datums in cases {
        let common = kv::NewCommonHandle(encode_key(datums)).unwrap().Encoded();
        assert!(min < common);
        assert!(max > common);
    }
}

/// 构造若干分区/整数/复合句柄供后续映射测试复用。
fn partial_handles() -> (
    kv::PartitionHandle,
    kv::PartitionHandle,
    kv::PartitionHandle,
    kv::IntHandle,
    kv::CommonHandle,
) {
    let decimal = kv::types::NewDecFromInt(1);
    let common =
        kv::NewCommonHandle(encode_key(vec![kv::types::NewDecimalDatum(decimal)])).unwrap();
    (
        kv::NewPartitionHandle(1, Box::new(kv::IntHandle(1))),
        kv::NewPartitionHandle(2, Box::new(kv::IntHandle(1))),
        kv::NewPartitionHandle(1, Box::new(kv::IntHandle(3))),
        kv::IntHandle(1),
        common,
    )
}

/// 分区句柄与普通句柄在 HandleMap 中互不覆盖，删除未知分区无副作用。
#[test]
fn test_handle_map_with_partial_handle() {
    let (p1, p2, p3, integer, common) = partial_handles();
    let mut map = kv::NewHandleMap();
    for (handle, value) in [
        (&p1 as &dyn Handle, 1),
        (&p2, 2),
        (&p3, 5),
        (&integer, 3),
        (&common, 4),
    ] {
        map.Set(handle, Box::new(value));
    }
    for (handle, expected) in [
        (&p1 as &dyn Handle, 1),
        (&p2, 2),
        (&p3, 5),
        (&integer, 3),
        (&common, 4),
    ] {
        downcast_value(map.Get(handle), expected);
    }
    assert_eq!(5, map.Len());
    map.Delete(&p1);
    assert!(map.Get(&p1).is_none());
    assert_eq!(4, map.Len());
    map.Delete(&kv::NewPartitionHandle(3, Box::new(kv::IntHandle(1))));
    assert_eq!(4, map.Len());
}

/// MemAwareHandleMap 对分区与普通句柄的 Get/Set 与 HandleMap 规则一致。
#[test]
fn test_mem_aware_handle_map_with_partial_handle() {
    let (p1, p2, p3, integer, common) = partial_handles();
    let mut map = kv::NewMemAwareHandleMap::<i32>();
    for (handle, value) in [
        (&p1 as &dyn Handle, 1),
        (&p2, 2),
        (&p3, 5),
        (&integer, 3),
        (&common, 4),
    ] {
        map.Set(handle, value);
    }
    assert_eq!(Some(&1), map.Get(&p1));
    assert_eq!(Some(&2), map.Get(&p2));
    assert_eq!(Some(&5), map.Get(&p3));
    assert_eq!(Some(&3), map.Get(&integer));
    assert_eq!(Some(&4), map.Get(&common));
}

/// KeyRange 默认值、切片内存估算，以及两种句柄映射填装后的取值一致性。
#[test]
fn test_key_range_definition() {
    let empty = kv::KeyRange::default();
    assert!(empty.StartKey.0.is_empty() && empty.EndKey.0.is_empty());
    let ranges = vec![
        kv::KeyRange {
            StartKey: kv::Key(b"s1".to_vec()),
            EndKey: kv::Key(b"e1".to_vec()),
        },
        kv::KeyRange {
            StartKey: kv::Key(b"s2".to_vec()),
            EndKey: kv::Key(b"e2".to_vec()),
        },
    ];
    assert_eq!(104, kv::KeyRangeSliceMemUsage(&ranges));

    let handles: Vec<Box<dyn Handle>> = (0..100)
        .map(|i| {
            if i % 2 == 0 {
                Box::new(kv::IntHandle(i)) as Box<dyn Handle>
            } else {
                common_handle(i, "").Copy()
            }
        })
        .collect();
    assert_eq!(99, mem_aware_int_map(&handles));
    assert_eq!(99, native_int_map(&handles));
}

/// 用 MemAwareHandleMap 按句柄存 index，返回最后一个句柄对应值。
fn mem_aware_int_map(handles: &[Box<dyn Handle>]) -> i32 {
    let mut map = kv::NewMemAwareHandleMap::<i32>();
    for (index, handle) in handles.iter().enumerate() {
        map.Set(handle.as_ref(), index as i32);
    }
    handles
        .iter()
        .map(|handle| *map.Get(handle.as_ref()).unwrap())
        .last()
        .unwrap_or_default()
}

/// 用原生 HashMap 按 Encoded 存 index，作为对照实现。
fn native_int_map(handles: &[Box<dyn Handle>]) -> i32 {
    let mut map = HashMap::new();
    for (index, handle) in handles.iter().enumerate() {
        map.insert(handle.Encoded(), index as i32);
    }
    handles
        .iter()
        .map(|handle| map[&handle.Encoded()])
        .last()
        .unwrap_or_default()
}
