// Copyright 2026 AsterSQL.

use super::map_abi_go126::{abiType, groupsReference, mapData, mapTable, mapType};
use std::mem::{offset_of, size_of};

#[test]
fn go126_runtime_mirrors_keep_go_field_offsets() {
    assert_eq!(size_of::<groupsReference>(), 16);
    assert_eq!(offset_of!(groupsReference, data), 0);
    assert_eq!(offset_of!(groupsReference, lengthMask), 8);

    assert_eq!(size_of::<mapTable>(), 32);
    assert_eq!(offset_of!(mapTable, used), 0);
    assert_eq!(offset_of!(mapTable, capacity), 2);
    assert_eq!(offset_of!(mapTable, growthLeft), 4);
    assert_eq!(offset_of!(mapTable, localDepth), 6);
    assert_eq!(offset_of!(mapTable, index), 8);
    assert_eq!(offset_of!(mapTable, groups), 16);

    assert_eq!(size_of::<mapData>(), 48);
    assert_eq!(offset_of!(mapData, Used), 0);
    assert_eq!(offset_of!(mapData, seed), 8);
    assert_eq!(offset_of!(mapData, dirPtr), 16);
    assert_eq!(offset_of!(mapData, dirLen), 24);
    assert_eq!(offset_of!(mapData, globalDepth), 32);
    assert_eq!(offset_of!(mapData, globalShift), 33);
    assert_eq!(offset_of!(mapData, writing), 34);
    assert_eq!(offset_of!(mapData, tombstonePossible), 35);
    assert_eq!(offset_of!(mapData, clearSeq), 40);

    assert_eq!(size_of::<abiType>(), 48);
    assert_eq!(offset_of!(abiType, Size_), 0);
    assert_eq!(offset_of!(abiType, PtrBytes), 8);
    assert_eq!(offset_of!(abiType, Hash), 16);
    assert_eq!(offset_of!(abiType, Equal), 24);
    assert_eq!(offset_of!(abiType, GCData), 32);
    assert_eq!(offset_of!(abiType, Str), 40);
    assert_eq!(offset_of!(abiType, PtrToThis), 44);

    assert_eq!(size_of::<mapType>(), 112);
    assert_eq!(offset_of!(mapType, abiType), 0);
    assert_eq!(offset_of!(mapType, Key), 48);
    assert_eq!(offset_of!(mapType, Elem), 56);
    assert_eq!(offset_of!(mapType, Group), 64);
    assert_eq!(offset_of!(mapType, Hasher), 72);
    assert_eq!(offset_of!(mapType, GroupSize), 80);
    assert_eq!(offset_of!(mapType, SlotSize), 88);
    assert_eq!(offset_of!(mapType, ElemOff), 96);
    assert_eq!(offset_of!(mapType, Flags), 104);
}
