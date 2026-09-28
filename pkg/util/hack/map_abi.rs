// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Go 1.25 Swiss map 的 runtime ABI 镜像与内存感知 map 包装。
//
// 对应 Go `pkg/util/hack/map_abi.go`（build tag: go1.25 && !go1.26）。
// 镜像 runtime table/Map/group 布局，估算/计算 map 内存，并提供 `MemAwareMap`。
// 依赖 Go runtime 内部结构，版本升级后需人工复核 ABI。

// 本文件由 pkg/util/hack/map_abi.go 迁移而来，保留 Go 实现结构。
// 这个文件描述 Go 1.25 swiss map 的 runtime ABI 镜像，以及基于该 ABI 估算 map 内存的包装类型。
// Go 原实现对 runtime 内部结构的依赖，后续必须由人工重新校验。
// Go build tag: go1.25 && !go1.26。

use crate::swiss_map::SwissMap;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hash;
use std::mem;

// Maximum size of a table before it is split at the directory level.
/// table 在 directory 层拆分前的最大容量。
pub const maxTableCapacity: u64 = 1024;

// Number of bits in the group.slot count.
/// 每个 group 槽位数的 bit 宽度（log2）。
pub const swissMapGroupSlotsBits: u64 = 3;

// Number of slots in a group.
/// 每个 group 的槽位数（固定为 8）。
pub const swissMapGroupSlots: u64 = 1 << swissMapGroupSlotsBits; // 8

// $GOROOT/src/internal/runtime/maps/table.go:`type table struct`
// swissMapTable 镜像 Go runtime 内部 table 布局；字段顺序必须跟 Go 注释来源保持一致。
/// Go runtime `table` 布局镜像；字段顺序须与 GOROOT 一致。
pub struct swissMapTable {
    // The number of filled slots (i.e. the number of elements in the table).
    used: u16,

    // The total number of slots (always 2^N). Equal to
    // `(groups.lengthMask+1)*abi.SwissMapGroupSlots`.
    capacity: u16,

    // The number of slots we can still fill without needing to rehash.
    // We rehash when used + tombstones > loadFactor*capacity, including
    // tombstones so the table doesn't overfill with tombstones. This field
    // counts down remaining empty slots before the next rehash.
    growthLeft: u16,

    // The number of bits used by directory lookups above this table. Note
    // that this may be less then globalDepth, if the directory has grown
    // but this table has not yet been split.
    localDepth: u8,

    // Index of this table in the Map directory. This is the index of the
    // _first_ location in the directory. The table may occur in multiple
    // sequential indicies.
    // index is -1 if the table is stale (no longer installed in the
    // directory).
    index: isize,

    // groups is an array of slot groups. Each group holds abi.SwissMapGroupSlots
    // key/elem slots and their control bytes. A table has a fixed size
    // groups array. The table is replaced (in rehash) when more space is
    // required.
    // TODO(prattmic): keys and elements are interleaved to maximize
    // locality, but it comes at the expense of wasted space for some types
    // (consider uint8 key, uint64 element). Consider placing all keys
    // together in these cases to save space.
    groups: groupsReference,
}

// groupsReference is a wrapper type describing an array of groups stored at
// data.
// groupsReference 保存 Go runtime 中 group 数组的起始地址和 lengthMask；这不是 Rust 安全切片。
/// group 数组引用：起始地址 + lengthMask（非安全切片）。
pub struct groupsReference {
    // data points to an array of groups. See groupReference above for the
    // definition of group.
    data: *mut c_void, // data *[length]typ.Group

    // lengthMask is the number of groups in data minus one (note that
    // length must be a power of two). This allows computing i%length
    // quickly using bitwise AND.
    lengthMask: u64,
}

// $GOROOT/src/internal/runtime/maps/map.go:`type Map struct`
// swissMap 镜像 Go runtime Map；Used 必须位于首字段，因为 Go len() 内建依赖这一布局。
/// Go runtime `Map` 头布局镜像；`Used` 须为首字段以贴合 len()。
pub struct swissMap {
    // The number of filled slots (i.e. the number of elements in all
    // tables). Excludes deleted slots.
    // Must be first (known by the compiler, for len() builtin).
    pub Used: u64,

    // seed is the hash seed, computed as a unique random number per map.
    seed: usize,

    // The directory of tables.
    // Normally dirPtr points to an array of table pointers
    // dirPtr *[dirLen]*table
    // The length (dirLen) of this array is `1 << globalDepth`. Multiple
    // entries may point to the same table. See top-level comment for more
    // details.
    // Small map optimization: if the map always contained
    // abi.SwissMapGroupSlots or fewer entries, it fits entirely in a
    // single group. In that case dirPtr points directly to a single group.
    // dirPtr *group
    // In this case, dirLen is 0. used counts the number of used slots in
    // the group. Note that small maps never have deleted slots (as there
    // is no probe sequence to maintain).
    dirPtr: *mut c_void,
    dirLen: isize,

    // The number of bits to use in table directory lookups.
    globalDepth: u8,

    // The number of bits to shift out of the hash for directory lookups.
    // On 64-bit systems, this is 64 - globalDepth.
    globalShift: u8,

    // writing is a flag that is toggled (XOR 1) while the map is being
    // written. Normally it is set to 1 when writing, but if there are
    // multiple concurrent writers, then toggling increases the probability
    // that both sides will detect the race.
    writing: u8,

    // tombstonePossible is false if we know that no table in this map
    // contains a tombstone.
    tombstonePossible: bool,

    // clearSeq is a sequence counter of calls to Clear. It is used to
    // detect map clears during iteration.
    clearSeq: u64,
}

impl swissMap {
    // directoryAt 按 Go 指针运算从 dirPtr 中取第 i 个 table 指针。
    /// 按指针运算从 dirPtr 取第 i 个 table 指针。
    pub unsafe fn directoryAt(&self, i: usize) -> *mut swissMapTable {
        // dirPtr 指向 table 指针数组；这里的二级指针解引用完全依赖 Go runtime ABI。
        let ptr = unsafe {
            (self.dirPtr as *mut u8).add(sizeofPtr as usize * i) as *mut *mut swissMapTable
        };
        unsafe { *ptr }
    }

    // Size returns the accurate memory size of the swissMap including all its tables.
    // Size 统计 map 自身、目录和每个唯一 table 的 group 内存；dirLen==0 时走小 map 单 group 快路径。
    /// 统计 map 自身、目录与各唯一 table groups 的准确内存。
    pub unsafe fn Size(&self, groupSize: u64) -> u64 {
        let mut sz = 0;
        sz += swissMapSize;
        sz += sizeofPtr * self.dirLen as u64;
        if self.dirLen == 0 {
            sz += groupSize;
            return sz;
        }

        let mut lastTab: *mut swissMapTable = std::ptr::null_mut();
        for i in 0..self.dirLen {
            let t = unsafe { self.directoryAt(i as usize) };
            if t == lastTab {
                // Go directory 可能有连续项指向同一 table，重复项不应重复计入内存。
                continue;
            }
            lastTab = t;
            sz += swissTableSize;
            sz += groupSize * (unsafe { (*t).groups.lengthMask } + 1);
        }
        sz
    }

    // Cap returns the total capacity of the swissMap.
    // Cap 累加所有唯一 table 的 capacity；小 map 直接返回单 group 的槽位数。
    /// 累加所有唯一 table 的 capacity；小 map 返回单 group 槽位数。
    pub unsafe fn Cap(&self) -> u64 {
        if self.dirLen == 0 {
            return swissMapGroupSlots;
        }
        let mut capacity = 0;
        let mut lastTab: *mut swissMapTable = std::ptr::null_mut();
        for i in 0..self.dirLen {
            let t = unsafe { self.directoryAt(i as usize) };
            if t == lastTab {
                continue;
            }
            lastTab = t;
            capacity += unsafe { (*t).capacity as u64 };
        }
        capacity
    }

    // MockSeedForTest sets the seed of the swissMap
    // MockSeedForTest 只用于测试固定 hash seed；map 非空时 panic，避免破坏正在使用的哈希表。
    /// 仅测试：空 map 时改写 hash seed，返回原 seed。
    pub fn MockSeedForTest(&mut self, seed: u64) -> u64 {
        if self.Used != 0 {
            panic!("MockSeedForTest can only be called on empty map");
        }
        let oriSeed = self.seed as u64;
        self.seed = seed as usize;
        oriSeed
    }
}

// Size returns the accurate memory size
// SwissMapWrap::Size 保留 Go 方法：根据 Type.GroupSize 调用底层 swissMap.Size。
impl SwissMapWrap {
    /// 根据 Type.GroupSize 调用底层 swissMap.Size。
    pub unsafe fn Size(&self) -> u64 {
        unsafe { (*self.Data).Size((*self.Type).GroupSize as u64) }
    }
}

/// `swissMap` 结构体字节大小。
pub const swissMapSize: u64 = mem::size_of::<swissMap>() as u64;
/// `swissMapTable` 结构体字节大小。
pub const swissTableSize: u64 = mem::size_of::<swissMapTable>() as u64;
/// 指针宽度（usize）。
pub const sizeofPtr: u64 = mem::size_of::<usize>() as u64;

// TODO: use a more accurate size calculation if necessary
// approxSize 保留 Go 的经验系数估算；该函数不读取真实 map 结构，只做近似计算。
/// 经验系数估算 map 内存，避免每次 Set 后昂贵的真实遍历。
pub fn approxSize(groupSize: u64, maxLen: u64) -> u64 {
    // 204 can fit the `split`/`rehash` behavior of different kinds of swisstable
    let ratio = 204;
    groupSize * maxLen * ratio / 1000
}

/// group 控制字类型别名。
pub type ctrlGroup = u64;

/// 单个 group 的指针引用。
pub struct groupReference {
    // data points to the group, which is described by typ.Group and has
    // layout:
    // type group struct {
    // 	ctrls ctrlGroup
    // 	slots [abi.SwissMapGroupSlots]slot
    // }
    // type slot struct {
    // key typ.Key
    // 	elem typ.Elem
    // }
    data: *mut c_void, // data *typ.Group
}

impl groupsReference {
    // group 根据 groupSize 做裸指针偏移，得到第 i 个 group 的引用包装。
    /// 按 GroupSize 偏移得到第 i 个 group。
    pub unsafe fn group(&self, typ: *const swissMapType, i: u64) -> groupReference {
        // TODO(prattmic): Do something here about truncation on cast to
        // uintptr on 32-bit systems?
        let offset = i as usize * unsafe { (*typ).GroupSize };

        groupReference {
            data: unsafe { (self.data as *mut u8).add(offset) as *mut c_void },
        }
    }
}

// $GOROOT/src/internal/abi/type.go:`type Type struct`
// abiType 镜像 Go internal/abi.Type；函数指针和 GCData 都是 runtime 内部 ABI，不能当普通 Rust 类型使用。
/// Go `internal/abi.Type` 镜像。
pub struct abiType {
    pub Size: usize,
    pub PtrBytes: usize, // number of (prefix) bytes in the type that can contain pointers
    pub Hash: u32,       // hash of type; avoids computation in hash tables
    pub TFlag: u8,       // extra type information flags
    pub Align: u8,       // alignment of variable with this type
    pub FieldAlign: u8,  // alignment of struct field with this type
    pub Kind: u8,        // enumeration for C
    // function for comparing objects of this type
    // (ptr to object A, ptr to object B) -> ==?
    pub Equal: Option<unsafe fn(*mut c_void, *mut c_void) -> bool>,
    // GCData stores the GC type data for the garbage collector.
    // Normally, GCData points to a bitmask that describes the
    // ptr/nonptr fields of the type. The bitmask will have at
    // least PtrBytes/ptrSize bits.
    // If the TFlagGCMaskOnDemand bit is set, GCData is instead a
    // **byte and the pointer to the bitmask is one dereference away.
    // The runtime will build the bitmask if needed.
    // (See runtime/type.go:getGCMask.)
    // Note: multiple types may have the same value of GCData,
    // including when TFlagGCMaskOnDemand is set. The types will, of course,
    // have the same pointer layout (but not necessarily the same size).
    pub GCData: *mut u8,
    pub Str: i32,       // string form
    pub PtrToThis: i32, // type for pointer to this type, may be zero
}

// $GOROOT/src/internal/abi/map_swiss.go:`type SwissMapType struct`
// swissMapType 镜像 Go internal/abi.SwissMapType；Go 的嵌入 abiType 在 实现中显式命名为 abiType。
/// Go `internal/abi.SwissMapType` 镜像。
pub struct swissMapType {
    pub abiType: abiType,
    pub Key: *mut abiType,
    pub Elem: *mut abiType,
    pub Group: *mut abiType, // internal type representing a slot group
    // function for hashing keys (ptr to key, seed) -> hash
    pub Hasher: Option<unsafe fn(*mut c_void, usize) -> usize>,
    pub GroupSize: usize, // == Group.Size_
    pub SlotSize: usize,  // size of key/elem slot
    pub ElemOff: usize, // offset of elem in key/elem slot; aka key size; elem size: SlotSize - ElemOff;
    pub Flags: u32,
}

// SwissMapWrap is a wrapper of map to access its internal structure.
// SwissMapWrap 是对 Go map interface 内部二元组的重解释包装；Type/Data 都来自 Go runtime 布局。
/// 将 Go map interface 内部二元组重解释为 Type/Data 包装。
pub struct SwissMapWrap {
    pub Type: *mut swissMapType,
    pub Data: *mut swissMap,
}

impl SwissMapWrap {
    /// Builds an ABI view from pointers supplied by a Go-runtime FFI boundary.
    ///
    /// # Safety
    /// Both pointers must refer to matching Go 1.25 runtime map structures and
    /// remain valid for every operation performed through the returned view.
    pub unsafe fn from_raw_parts(Type: *mut swissMapType, Data: *mut swissMap) -> Self {
        assert!(!Type.is_null() && !Data.is_null());
        Self { Type, Data }
    }
}

// ToSwissMap converts a map to SwissMapWrap.
// ToSwissMap 在 Go 中通过 any(m) 和 unsafe.Pointer 重解释 map；只保留这一危险转换的形状。
/// Go 侧 unsafe 转换入口的占位；Rust HashMap 无法重解释为 runtime map。
pub unsafe fn ToSwissMap<K: Eq + Hash, V>(_m: &mut HashMap<K, V>) -> SwissMapWrap {
    panic!(
        "a Rust HashMap cannot be reinterpreted as a Go runtime map; use SwissMapWrap::from_raw_parts at an FFI boundary"
    )
}

/// 控制字占用字节数。
pub const ctrlGroupsSize: usize = mem::size_of::<ctrlGroup>();
/// group 内 slots 相对起始的字节偏移。
pub const groupSlotsOffset: usize = ctrlGroupsSize;

impl groupReference {
    /// 返回该 group 可容纳的 key/elem 槽位数。
    pub fn cap(&self, typ: *const swissMapType) -> u64 {
        let _ = self;
        unsafe { groupCap((*typ).GroupSize as u64, (*typ).SlotSize as u64) }
    }
}

/// `(groupSize - groupSlotsOffset) / slotSize` 容量公式。
pub fn groupCap(groupSize: u64, slotSize: u64) -> u64 {
    (groupSize - groupSlotsOffset as u64) / slotSize
}

impl groupReference {
    // key returns a pointer to the key at index i.
    // key 按 Go group slot 布局计算第 i 个 key 的地址；返回值仍是裸指针。
    /// 按 slot 布局返回第 i 个 key 的裸指针。
    pub unsafe fn key(&self, typ: *const swissMapType, i: usize) -> *mut c_void {
        let offset = groupSlotsOffset + i * unsafe { (*typ).SlotSize };
        unsafe { (self.data as *mut u8).add(offset) as *mut c_void }
    }

    // elem returns a pointer to the element at index i.
    // elem 在 key 偏移基础上加 ElemOff，得到第 i 个 value 的地址。
    /// 在 key 偏移上加 ElemOff，返回第 i 个 value 指针。
    pub unsafe fn elem(&self, typ: *const swissMapType, i: usize) -> *mut c_void {
        let offset = groupSlotsOffset + i * unsafe { (*typ).SlotSize } + unsafe { (*typ).ElemOff };
        unsafe { (self.data as *mut u8).add(offset) as *mut c_void }
    }
}

// MemAwareMap is a map with memory usage tracking.
// MemAwareMap 包装 map 并维护近似内存字节数；由 SwissMap 维护 Go 表布局和分配历史。
/// 带近似内存跟踪的 map 包装；内部由 SwissMap 维护 Go 表布局和分配历史。
pub struct MemAwareMap<K: Eq + Hash, V> {
    pub M: SwissMap<K, V>,
    groupSize: u64,
    nextCheckpoint: u64, // every `maxTableCapacity` increase in Used
    pub Bytes: u64,
}

/// 测试用固定 hash seed。
pub const mockSeedForTest: u64 = 4992862800126241206;

impl<K: Eq + Hash, V> MemAwareMap<K, V> {
    // MockSeedForTest sets the seed of the swissMap for testing.
    // It should only be used in tests and should not be called on maps that are already in use, as it may cause issues with map operations.
    // The map should be empty before calling this function.
    // MockSeedForTest 只为测试固定 hash seed；调用前要求 map 为空。
    /// 仅测试：要求 map 为空后再固定 seed。
    pub fn MockSeedForTest(&mut self) {
        self.M.set_seed(mockSeedForTest);
    }

    // Count returns the number of elements in the map.
    /// 返回元素个数（对齐 Go `len(m.M)`）。
    pub fn Count(&self) -> usize {
        self.M.len()
    }

    // Empty returns true if the map is empty.
    /// 判断 map 是否为空。
    pub fn Empty(&self) -> bool {
        self.M.is_empty()
    }

    // Exist returns true if the key exists in the map.
    /// 判断 key 是否存在。
    pub fn Exist(&self, val: &K) -> bool {
        self.M.contains_key(val)
    }

    // unwrap 对应 Go 里把 map 头重解释为 *swissMap；这是整个文件最依赖 runtime ABI 的入口。
    unsafe fn unwrap(&mut self) -> *mut swissMap {
        panic!("a Rust HashMap has no Go runtime map header")
    }

    // Set sets the value for the key in the map and returns the memory delta.
    // Set 写入键值后按 checkpoint 更新近似内存；Go 先拿 sm 指针再写 map，这里保留该顺序。
    /// 写入键值；达 checkpoint 时更新近似内存并返回增量。
    pub fn Set(&mut self, key: K, value: V) -> i64 {
        self.M.insert(key, value);
        let used = self.M.len() as u64;
        if used >= self.nextCheckpoint {
            let newBytes = self.Bytes.max(approxSize(self.groupSize, used));
            let deltaBytes = newBytes as i64 - self.Bytes as i64;
            self.Bytes = newBytes;
            self.nextCheckpoint = used.min(maxTableCapacity) + used;
            return deltaBytes;
        }
        0
    }

    // SetExt sets the value for the key in the map and returns the memory delta and whether it's an insert.
    // SetExt 通过比较写入前后的 Used 判断是否是插入新键。
    /// 写入并返回内存增量与是否为新插入。
    pub fn SetExt(&mut self, key: K, value: V) -> (i64, bool) {
        let insert = !self.M.contains_key(&key);
        let deltaBytes = self.Set(key, value);
        (deltaBytes, insert)
    }

    // Init initializes the MemAwareMap with the given map and returns the initial memory size.
    // The input map should NOT be nil.
    // Init 接收外部 map 并计算初始内存；Rust HashMap 没有 nil 状态，因此只在注释中保留 Go nil 约束。
    /// 用外部 map 初始化并返回初始内存字节数。
    pub fn Init(&mut self, v: impl Into<SwissMap<K, V>>) -> i64 {
        self.M = v.into();
        self.groupSize = crate::swiss_map::group_size::<K, V>();
        self.Bytes = self.real_bytes_for_capacity();
        let used = self.M.len() as u64;
        if used <= swissMapGroupSlots {
            self.nextCheckpoint = swissMapGroupSlots * 2;
        } else {
            self.nextCheckpoint = used.min(maxTableCapacity) + used;
        }
        self.Bytes as i64
    }

    // RealBytes returns the real memory size of the map.
    // Compute the real size is expensive, so do not call it frequently.
    // Make sure the `seed` is same when testing the memory size.
    // RealBytes 重新遍历真实 swiss map table，代价比 checkpoint 估算更高。
    /// 估算真实内存（相对 checkpoint 更贵，勿频繁调用）。
    pub fn RealBytes(&self) -> u64 {
        self.real_bytes_for_capacity()
    }

    // Get the value of the key.
    /// 查找键，返回 `(Option, ok)`。
    pub fn Get(&self, k: &K) -> (Option<&V>, bool) {
        let v = self.M.get(k);
        (v, v.is_some())
    }

    // Len returns the number of elements in the map.
    /// 返回元素个数。
    pub fn Len(&self) -> usize {
        self.M.len()
    }

    fn real_bytes_for_capacity(&self) -> u64 {
        self.M
            .size(swissMapSize, swissTableSize, sizeofPtr, self.groupSize)
    }
}

// NewMemAwareMap creates a new MemAwareMap with the given initial capacity.
// NewMemAwareMap 对应 Go 的 make(map[K]V, capacity) 后 Init。
/// 按初始容量创建 MemAwareMap 并 Init。
pub fn NewMemAwareMap<K: Eq + Hash, V>(capacity: usize) -> Box<MemAwareMap<K, V>> {
    let mut m = Box::new(MemAwareMap {
        M: SwissMap::with_capacity(capacity),
        groupSize: 0,
        nextCheckpoint: 0,
        Bytes: 0,
    });
    let initial = mem::take(&mut m.M);
    m.Init(initial);
    m
}

/// ABI 自检占位：Rust HashMap 独立于 Go runtime ABI。
pub fn checkMapABI() {
    // Rust's standard HashMap is intentionally independent of the Go runtime ABI.
}
