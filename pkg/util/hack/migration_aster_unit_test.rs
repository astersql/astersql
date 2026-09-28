// Copyright 2026 AsterSQL.

// AsterSQL 迁移补充：hack 零拷贝与 MemAwareMap 行为回归。
//
// 覆盖 String/Slice/GetBytesFromPtr、Go 1.25/1.26 MemAwareMap CRUD，
// 以及用合成 runtime 布局校验 Cap/Size 公式。

use super::hack;
use super::map_abi as go125;
use super::map_abi_go126::{
    NewMemAwareMap, groupCap, groupsReference, mapData, mapGroupSlots, mapTable,
};
use std::ffi::c_void;

#[test]
/// 校验字节/字符串视图零拷贝：指针相同且内容一致。
fn byte_string_views_match_go_without_copying() {
    let bytes = b"hello world".to_vec();
    let text = unsafe { hack::String(&bytes) };
    assert_eq!(text, "hello world");
    assert_eq!(text.as_ptr(), bytes.as_ptr());

    let slice = hack::Slice("aster");
    assert_eq!(slice, b"aster");
    assert_eq!(slice.as_ptr(), "aster".as_ptr());

    let from_ptr = unsafe { hack::GetBytesFromPtr(bytes.as_ptr(), bytes.len()) };
    assert_eq!(from_ptr, bytes);
    assert_eq!(from_ptr.as_ptr(), bytes.as_ptr());
}

#[test]
/// 校验 MemAwareMap（Go 1.26 路径）CRUD、SetExt 插入判定与内存增长。
fn mem_aware_map_matches_go_crud_and_insert_semantics() {
    let mut map = NewMemAwareMap::<i32, String>(0);
    assert!(map.Empty());
    assert!(map.Bytes > 0);

    let (_, inserted) = map.SetExt(7, "first".to_owned());
    assert!(inserted);
    let (_, inserted) = map.SetExt(7, "updated".to_owned());
    assert!(!inserted);
    assert_eq!(map.Count(), 1);
    assert_eq!(map.Len(), 1);
    assert!(map.Exist(&7));
    let (value, ok) = map.Get(&7);
    assert!(ok);
    assert_eq!(value.map(String::as_str), Some("updated"));

    let initial = map.Bytes;
    let mut positive_delta = 0;
    for key in 8..80 {
        positive_delta += map.Set(key, key.to_string());
    }
    assert!(positive_delta > 0);
    assert!(map.Bytes > initial);
    assert_eq!(map.Bytes as i64 - initial as i64, positive_delta);
    assert!(map.RealBytes() > 0);
}

#[test]
/// 校验 Go 1.25 变体 MemAwareMap 的同样安全 map 行为。
fn go125_variant_has_the_same_safe_map_behavior() {
    let mut map = go125::NewMemAwareMap::<i32, i32>(0);
    assert!(map.Empty());
    let (_, inserted) = map.SetExt(1, 10);
    assert!(inserted);
    let (_, inserted) = map.SetExt(1, 20);
    assert!(!inserted);
    assert_eq!(map.Get(&1), (Some(&20), true));
    assert!(map.Bytes > 0);
    assert!(map.RealBytes() > 0);
}

#[test]
/// 用合成 mapTable/mapData 校验 groupCap、Cap、Size 与 Go 公式一致。
fn synthetic_runtime_layout_calculations_match_go_formulas() {
    assert_eq!(mapGroupSlots, 8);
    assert_eq!(groupCap(136, 16), 8);

    let mut tables = [mapTable {
        used: 1,
        capacity: 16,
        growthLeft: 15,
        localDepth: 0,
        index: 0,
        groups: groupsReference {
            data: std::ptr::null_mut(),
            lengthMask: 1,
        },
    }];
    let mut directory = [tables.as_mut_ptr(), tables.as_mut_ptr()];
    let map = mapData {
        Used: 1,
        seed: 0,
        dirPtr: directory.as_mut_ptr().cast::<c_void>(),
        dirLen: directory.len() as isize,
        globalDepth: 1,
        globalShift: 63,
        writing: 0,
        tombstonePossible: false,
        clearSeq: 0,
    };
    assert_eq!(unsafe { map.Cap() }, 16);
    assert_eq!(
        unsafe { map.Size(136) },
        super::map_abi_go126::mapSize
            + 2 * super::map_abi_go126::sizeofPtr
            + super::map_abi_go126::mapTableSize
            + 2 * 136
    );
}

#[test]
// map_abi_test.go:121-135: the ninth entry allocates a directory and table.
fn ninth_entry_real_bytes_matches_go_swiss_table() {
    let mut go126 = NewMemAwareMap::<i64, i64>(0);
    let mut go125 = go125::NewMemAwareMap::<i64, i64>(0);
    let empty = (go125.RealBytes(), go126.RealBytes());
    for key in 0..8 {
        go125.Set(key, key);
        go126.Set(key, key);
    }
    let eight = (go125.RealBytes(), go126.RealBytes());
    go125.Set(9, 9);
    go126.Set(9, 9);
    let nine = (go125.RealBytes(), go126.RealBytes());
    assert_eq!([empty, eight, nine], [(184, 184), (184, 184), (360, 360)]);
}

#[test]
/// Go build tags expose exactly one MemAwareMap API at package root.
fn crate_root_exports_one_active_mem_aware_map_api() {
    let mut map = super::NewMemAwareMap::<i32, i32>(0);
    assert!(map.Empty());
    assert_eq!(map.SetExt(1, 2).1, true);
}

#[test]
/// Go slot structs align the value after the key and pad the complete slot.
fn mixed_alignment_map_uses_go_slot_padding() {
    let go126 = super::map_abi_go126::NewMemAwareMap::<i8, i64>(0);
    assert_eq!(go126.RealBytes(), super::map_abi_go126::mapSize + 136);

    let go125 = super::map_abi::NewMemAwareMap::<i8, i64>(0);
    assert_eq!(go125.RealBytes(), super::map_abi::swissMapSize + 136);
}
