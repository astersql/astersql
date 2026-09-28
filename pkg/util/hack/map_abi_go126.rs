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

// Go 1.26 Swiss map 的 runtime ABI 镜像与内存感知 map 包装。
//
// 对应 Go `pkg/util/hack/map_abi_go126.go`（build tag: go1.26 && !go1.27）。
// 相对 1.25 变体，table/Map 命名改为 `mapTable`/`mapData`，布局语义同类。
// 依赖 Go runtime 内部结构，版本升级后需人工复核 ABI。

// 本文件由 pkg/util/hack/map_abi_go126.go 迁移而来，保留 Go 实现结构。
// hack 包在 Go 1.26 Swiss map ABI 下读取 runtime map 内部布局、
// 估算/计算 map 内存占用并提供 MemAwareMap 包装器的逻辑。
// Go build tag: go1.26 && !go1.27；用 cfg_attr 记录同样的版本约束意图。

use crate::swiss_map::SwissMap;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hash;

// Maximum size of a table before it is split at the directory level.
// maxTableCapacity 保留 Go runtime table split 的阈值。
/// table 在 directory 层拆分前的最大容量。
pub const maxTableCapacity: u64 = 1024;

// Number of bits in the group.slot count.
// mapGroupSlotsBits 对应 Go runtime 每组 slot 数的 bit 数。
/// 每个 group 槽位数的 bit 宽度（log2）。
pub const mapGroupSlotsBits: u64 = 3;

// Number of slots in a group.
// mapGroupSlots 保留 Go 的 `1 << mapGroupSlotsBits`，即每组 8 个 slot。
/// 每个 group 的槽位数（固定为 8）。
pub const mapGroupSlots: u64 = 1 << mapGroupSlotsBits;

// $GOROOT/src/internal/runtime/maps/table.go:`type table struct`
// mapTable 机械对应 Go runtime 内部 table 结构；字段顺序不能随意调整，否则 ABI 对照会失真。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// Go runtime `table` 布局镜像（Go 1.26 命名）。
pub struct mapTable {
    // The number of filled slots (i.e. the number of elements in the table).
    pub used: u16,

    // The total number of slots (always 2^N). Equal to
    // `(groups.lengthMask+1)*abi.MapGroupSlots`.
    pub capacity: u16,

    // The number of slots we can still fill without needing to rehash.
    // We rehash when used + tombstones > loadFactor*capacity, including
    // tombstones so the table doesn't overfill with tombstones. This field
    // counts down remaining empty slots before the next rehash.
    pub growthLeft: u16,

    // The number of bits used by directory lookups above this table. Note
    // that this may be less then globalDepth, if the directory has grown
    // but this table has not yet been split.
    pub localDepth: u8,

    // Index of this table in the Map directory. This is the index of the
    // _first_ location in the directory. The table may occur in multiple
    // sequential indicies.
    // index is -1 if the table is stale (no longer installed in the
    // directory).
    pub index: isize,

    // groups is an array of slot groups. Each group holds abi.MapGroupSlots
    // key/elem slots and their control bytes. A table has a fixed size
    // groups array. The table is replaced (in rehash) when more space is
    // required.
    // TODO(prattmic): keys and elements are interleaved to maximize
    // locality, but it comes at the expense of wasted space for some types
    // (consider uint8 key, uint64 element). Consider placing all keys
    // together in these cases to save space.
    pub groups: groupsReference,
}

// groupsReference is a wrapper type describing an array of groups stored at
// data.
// groupsReference 对应 runtime 里连续 group 数组的引用，data/lengthMask 都来自 Go 内部 ABI。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// group 数组引用：起始地址 + lengthMask。
pub struct groupsReference {
    // data points to an array of groups. See groupReference above for the
    // definition of group.
    pub data: *mut c_void, // data *[length]typ.Group

    // lengthMask is the number of groups in data minus one (note that
    // length must be a power of two). This allows computing i%length
    // quickly using bitwise AND.
    pub lengthMask: u64,
}

// $GOROOT/src/internal/runtime/maps/map.go:`type Map struct`
// mapData 机械对应 Go runtime map header；Used 必须保持第一字段以贴合 len() 相关布局说明。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// Go runtime map header 镜像；`Used` 须为首字段。
pub struct mapData {
    // The number of filled slots (i.e. the number of elements in all
    // tables). Excludes deleted slots.
    // Must be first (known by the compiler, for len() builtin).
    pub Used: u64,

    // seed is the hash seed, computed as a unique random number per map.
    pub seed: usize,

    // The directory of tables.
    // Normally dirPtr points to an array of table pointers
    // dirPtr *[dirLen]*table
    // The length (dirLen) of this array is `1 << globalDepth`. Multiple
    // entries may point to the same table. See top-level comment for more
    // details.
    // Small map optimization: if the map always contained
    // abi.MapGroupSlots or fewer entries, it fits entirely in a
    // single group. In that case dirPtr points directly to a single group.
    // dirPtr *group
    // In this case, dirLen is 0. used counts the number of used slots in
    // the group. Note that small maps never have deleted slots (as there
    // is no probe sequence to maintain).
    pub dirPtr: *mut c_void,
    pub dirLen: isize,

    // The number of bits to use in table directory lookups.
    pub globalDepth: u8,

    // The number of bits to shift out of the hash for directory lookups.
    // On 64-bit systems, this is 64 - globalDepth.
    pub globalShift: u8,

    // writing is a flag that is toggled (XOR 1) while the map is being
    // written. Normally it is set to 1 when writing, but if there are
    // multiple concurrent writers, then toggling increases the probability
    // that both sides will detect the race.
    pub writing: u8,

    // tombstonePossible is false if we know that no table in this map
    // contains a tombstone.
    pub tombstonePossible: bool,

    // clearSeq is a sequence counter of calls to Clear. It is used to
    // detect map clears during iteration.
    pub clearSeq: u64,
}

impl mapData {
    // directoryAt 按 Go 的 unsafe 指针算术读取目录里的第 i 个 table 指针。
    // 这里依赖 runtime ABI，保留 unsafe 形状，后续若 Go 版本变动必须重新校验布局。
    /// 按指针算术读取目录第 i 个 table 指针。
    pub unsafe fn directoryAt(&self, i: usize) -> *mut mapTable {
        let slot = (self.dirPtr as usize + sizeofPtr as usize * i) as *const *mut mapTable;
        unsafe { *slot }
    }

    // Size returns the accurate memory size of the map including all its tables.
    // Size 计算 map header、目录和 table groups 的真实内存占用；dirLen==0 时对应 small map 优化。
    /// 计算 map header、目录与 table groups 的真实内存。
    pub unsafe fn Size(&self, groupSize: u64) -> u64 {
        let mut sz = 0_u64;
        sz += mapSize;
        sz += sizeofPtr * self.dirLen as u64;
        if self.dirLen == 0 {
            sz += groupSize;
            return sz;
        }

        let mut lastTab: *mut mapTable = std::ptr::null_mut();
        for i in 0..self.dirLen {
            let t = unsafe { self.directoryAt(i as usize) };
            // Go 代码跳过连续重复 table 指针，避免同一个 split table 被重复计入。
            if t == lastTab {
                continue;
            }
            lastTab = t;
            sz += mapTableSize;
            sz += groupSize * (unsafe { (*t).groups.lengthMask } + 1);
        }
        sz
    }

    // Cap returns the total capacity of the map.
    // Cap 遍历目录中不同 table 的 capacity；small map 直接返回单组 slot 数。
    /// 累加唯一 table 的 capacity；小 map 返回单组槽位数。
    pub unsafe fn Cap(&self) -> u64 {
        if self.dirLen == 0 {
            return mapGroupSlots;
        }
        let mut capacity = 0_u64;
        let mut lastTab: *mut mapTable = std::ptr::null_mut();
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

    // MockSeedForTest sets the seed of the map internals.
    // MockSeedForTest 只用于测试：空 map 才允许改写 hash seed，非空时保持 Go 的 panic 语义。
    /// 仅测试：空 map 时改写 hash seed。
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
// SwissMapWrap::Size 转发到底层 mapData.Size；unsafe 来自读取 Go runtime map header。
impl SwissMapWrap {
    /// 转发到底层 mapData.Size。
    pub unsafe fn Size(&self) -> u64 {
        unsafe { (*self.Data).Size((*self.Type).GroupSize as u64) }
    }
}

/// `mapData` 结构体字节大小。
pub const mapSize: u64 = std::mem::size_of::<mapData>() as u64;
/// `mapTable` 结构体字节大小。
pub const mapTableSize: u64 = std::mem::size_of::<mapTable>() as u64;
/// 指针宽度（usize）。
pub const sizeofPtr: u64 = std::mem::size_of::<usize>() as u64;

// TODO: use a more accurate size calculation if necessary
// approxSize 保留 Go 的经验比例估算，用于减少每次 Set 后昂贵的真实 ABI 遍历。
/// 经验比例估算 map 内存。
pub fn approxSize(groupSize: u64, maxLen: u64) -> u64 {
    // 204 can fit the `split`/`rehash` behavior of different kinds of map tables.
    let ratio = 204_u64;
    groupSize * maxLen * ratio / 1000
}

/// group 控制字类型别名。
pub type ctrlGroup = u64;

// groupReference 对应单个 group 的指针引用；slots 布局由 mapType 的 Group/SlotSize/ElemOff 决定。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// 单个 group 的指针引用。
pub struct groupReference {
    // data points to the group, which is described by typ.Group and has
    // layout:
    // type group struct {
    // 	ctrls ctrlGroup
    // 	slots [abi.MapGroupSlots]slot
    // }
    // type slot struct {
    // key typ.Key
    // 	elem typ.Elem
    // }
    pub data: *mut c_void, // data *typ.Group
}

impl groupsReference {
    // group 按 groupSize 计算第 i 个 group 的起始地址。
    /// 按 GroupSize 计算第 i 个 group 起始地址。
    pub unsafe fn group(&self, typ: *const mapType, i: u64) -> groupReference {
        // TODO(prattmic): Do something here about truncation on cast to
        // uintptr on 32-bit systems?
        let offset = i as usize * unsafe { (*typ).GroupSize };

        groupReference {
            // Go 使用 unsafe.Pointer(uintptr(g.data)+offset)；这里保留同样的指针偏移语义。
            data: (self.data as usize + offset) as *mut c_void,
        }
    }
}

// $GOROOT/src/internal/abi/type.go:`type Type struct`
// abiType 机械对应 Go internal/abi.Type，供 mapType 嵌入。
#[repr(C)]
#[derive(Clone, Copy)]
/// Go `internal/abi.Type` 镜像。
pub struct abiType {
    pub Size_: usize,
    pub PtrBytes: usize, // number of (prefix) bytes in the type that can contain pointers
    pub Hash: u32,       // hash of type; avoids computation in hash tables
    pub TFlag: u8,       // extra type information flags
    pub Align_: u8,      // alignment of variable with this type
    pub FieldAlign_: u8, // alignment of struct field with this type
    pub Kind_: u8,       // enumeration for C
    // function for comparing objects of this type
    // (ptr to object A, ptr to object B) -> ==?
    pub Equal: Option<fn(*mut c_void, *mut c_void) -> bool>,
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

// $GOROOT/src/internal/abi/map.go:`type MapType struct`
// mapType 对应 Go internal/abi.MapType，保存 key/elem/group 类型和 slot 偏移信息。
#[repr(C)]
#[derive(Clone, Copy)]
/// Go `internal/abi.MapType` 镜像。
pub struct mapType {
    pub abiType: abiType,
    pub Key: *mut abiType,
    pub Elem: *mut abiType,
    pub Group: *mut abiType, // internal type representing a slot group
    // function for hashing keys (ptr to key, seed) -> hash
    pub Hasher: Option<fn(*mut c_void, usize) -> usize>,
    pub GroupSize: usize, // == Group.Size_
    pub SlotSize: usize,  // size of key/elem slot
    pub ElemOff: usize, // offset of elem in key/elem slot; aka key size; elem size: SlotSize - ElemOff;
    pub Flags: u32,
}

// SwissMapWrap is a wrapper of map to access its internal structure.
// SwissMapWrap 对应 Go 中把 interface{} 解释成 mapType/mapData 的包装器。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
/// 将 Go map 解释为 mapType/mapData 的包装器。
pub struct SwissMapWrap {
    pub Type: *mut mapType,
    pub Data: *mut mapData,
}

impl SwissMapWrap {
    /// Builds an ABI view from pointers supplied by a Go-runtime FFI boundary.
    ///
    /// # Safety
    /// Both pointers must refer to matching Go 1.26 runtime map structures and
    /// remain valid for every operation performed through the returned view.
    pub unsafe fn from_raw_parts(Type: *mut mapType, Data: *mut mapData) -> Self {
        assert!(!Type.is_null() && !Data.is_null());
        Self { Type, Data }
    }
}

// ToSwissMap converts a map to SwissMapWrap.
// ToSwissMap 保留 Go 的 unsafe 转换入口：Go 通过 any(m) 的接口布局拿到 map 类型和数据指针。
/// Go unsafe 转换入口占位；Rust HashMap 无法重解释为 runtime map。
pub unsafe fn ToSwissMap<K, V>(_m: &HashMap<K, V>) -> SwissMapWrap
where
    K: Eq + Hash,
{
    // Rust 标准 HashMap 布局与 Go map 完全不同；这里不虚构转换，只保留 Go ABI 入口的占位。
    // 后续如果需要真正运行，必须由 Go runtime 侧或 FFI 提供等价的 map header。
    panic!(
        "a Rust HashMap cannot be reinterpreted as a Go runtime map; use SwissMapWrap::from_raw_parts at an FFI boundary"
    )
}

/// 控制字占用字节数。
pub const ctrlGroupsSize: usize = std::mem::size_of::<ctrlGroup>();
/// group 内 slots 相对起始的字节偏移。
pub const groupSlotsOffset: usize = ctrlGroupsSize;

impl groupReference {
    // cap 返回一个 group 内可容纳的 key/elem slot 数，计算方式来自 Go runtime group 布局。
    /// 返回 group 内可容纳的 slot 数。
    pub unsafe fn cap(&self, typ: *const mapType) -> u64 {
        let _ = self;
        groupCap(unsafe { (*typ).GroupSize as u64 }, unsafe {
            (*typ).SlotSize as u64
        })
    }

    // key returns a pointer to the key at index i.
    // key 根据 slot 偏移返回第 i 个 key 地址；这是纯指针算术，调用方必须保证 i 在 group 容量内。
    /// 返回第 i 个 key 地址。
    pub unsafe fn key(&self, typ: *const mapType, i: usize) -> *mut c_void {
        let offset = groupSlotsOffset + i * unsafe { (*typ).SlotSize };
        (self.data as usize + offset) as *mut c_void
    }

    // elem returns a pointer to the element at index i.
    // elem 在 key 偏移基础上加 ElemOff，得到第 i 个元素地址。
    /// 返回第 i 个 elem 地址。
    pub unsafe fn elem(&self, typ: *const mapType, i: usize) -> *mut c_void {
        let offset = groupSlotsOffset + i * unsafe { (*typ).SlotSize } + unsafe { (*typ).ElemOff };
        (self.data as usize + offset) as *mut c_void
    }
}

// groupCap 保留 Go 里 `(groupSize - groupSlotsOffset) / slotSize` 的容量计算。
/// `(groupSize - groupSlotsOffset) / slotSize` 容量公式。
pub fn groupCap(groupSize: u64, slotSize: u64) -> u64 {
    (groupSize - groupSlotsOffset as u64) / slotSize
}

// MemAwareMap is a map with memory usage tracking.
// MemAwareMap 对应 Go 泛型结构体，M 是业务 map，Bytes/nextCheckpoint 用于近似跟踪内存变化。
#[derive(Clone, Debug)]
/// 带近似内存跟踪的 map 包装。
pub struct MemAwareMap<K, V>
where
    K: Eq + Hash,
{
    pub M: SwissMap<K, V>,
    pub groupSize: u64,
    pub nextCheckpoint: u64, // every `maxTableCapacity` increase in Used
    pub Bytes: u64,
}

/// 测试用固定 hash seed。
pub const mockSeedForTest: u64 = 4992862800126241206;

impl<K, V> MemAwareMap<K, V>
where
    K: Eq + Hash,
{
    // MockSeedForTest sets the seed of the swissMap for testing.
    // It should only be used in tests and should not be called on maps that are already in use, as it may cause issues with map operations.
    // The map should be empty before calling this function.
    // MockSeedForTest 转发到底层 mapData.MockSeedForTest；只能用于空 map 测试，避免破坏 map 哈希状态。
    /// 仅测试：要求 map 为空。
    pub fn MockSeedForTest(&mut self) {
        self.M.set_seed(mockSeedForTest);
    }

    // Count returns the number of elements in the map.
    // Count 对应 Go 的 len(m.M)。
    /// 返回元素个数。
    pub fn Count(&self) -> usize {
        self.M.len()
    }

    // Empty returns true if the map is empty.
    // Empty 保留 Go 的 `len(m.M)==0` 语义。
    /// 判断是否为空。
    pub fn Empty(&self) -> bool {
        self.M.is_empty()
    }

    // Exist returns true if the key exists in the map.
    // Exist 对应 Go 的 map 双返回值查找，只报告 key 是否存在。
    /// 判断 key 是否存在。
    pub fn Exist(&self, val: &K) -> bool {
        self.M.contains_key(val)
    }

    // unwrap 对应 Go 的 `*(**mapData)(unsafe.Pointer(&m.M))`。
    // Rust HashMap 不是 Go map；这里保留 ABI 读取点，提醒后续真实实现必须替换。
    unsafe fn unwrap(&self) -> *mut mapData {
        panic!("a Rust HashMap has no Go runtime map header")
    }

    // Set sets the value for the key in the map and returns the memory delta.
    // Set 写入 map 后，在 Used 达到 checkpoint 时更新 Bytes 并返回内存变化量。
    /// 写入后按 checkpoint 更新 Bytes，返回内存增量。
    pub fn Set(&mut self, key: K, value: V) -> i64 {
        self.M.insert(key, value);
        let mut deltaBytes = 0_i64;
        let used = self.M.len() as u64;
        if used >= self.nextCheckpoint {
            let newBytes = self.Bytes.max(approxSize(self.groupSize, used));
            deltaBytes = newBytes as i64 - self.Bytes as i64;
            self.Bytes = newBytes;
            self.nextCheckpoint = used.min(maxTableCapacity) + used;
        }
        deltaBytes
    }

    // SetExt sets the value for the key in the map and returns the memory delta and whether it's an insert.
    // SetExt 在 Set 前后比较 Used，保留 Go 的“是否插入新 key”判断。
    /// 写入并返回增量与是否新插入。
    pub fn SetExt(&mut self, key: K, value: V) -> (i64, bool) {
        let insert = !self.M.contains_key(&key);
        let deltaBytes = self.Set(key, value);
        (deltaBytes, insert)
    }

    // Init initializes the MemAwareMap with the given map and returns the initial memory size.
    // The input map should NOT be nil.
    // Init 接收调用方提供的 map，读取 Go runtime groupSize 和初始 Size，并设置下一次 checkpoint。
    /// 初始化 groupSize/Bytes/checkpoint，返回初始内存。
    pub fn Init(&mut self, v: impl Into<SwissMap<K, V>>) -> i64 {
        self.M = v.into();
        self.groupSize = crate::swiss_map::group_size::<K, V>();
        self.Bytes = self.real_bytes_for_capacity();
        let used = self.M.len() as u64;
        if used <= mapGroupSlots {
            self.nextCheckpoint = mapGroupSlots * 2;
        } else {
            self.nextCheckpoint = used.min(maxTableCapacity) + used;
        }
        self.Bytes as i64
    }

    // RealBytes returns the real memory size of the map.
    // Compute the real size is expensive, so do not call it frequently.
    // Make sure the `seed` is same when testing the memory size.
    // RealBytes 直接遍历 mapData.Size，属于昂贵的真实 ABI 计算路径。
    /// 昂贵的真实内存估算路径。
    pub fn RealBytes(&self) -> u64 {
        self.real_bytes_for_capacity()
    }

    // Get the value of the key.
    // Get 对应 Go map 查找的 `(v, ok)` 返回；这里返回 Option 引用表达 ok。
    /// 查找键，返回 `(Option, ok)`。
    pub fn Get(&self, k: &K) -> (Option<&V>, bool) {
        let value = self.M.get(k);
        (value, value.is_some())
    }

    // Len returns the number of elements in the map.
    // Len 与 Count 一样返回当前 map 元素数量。
    /// 返回元素个数。
    pub fn Len(&self) -> usize {
        self.M.len()
    }

    fn real_bytes_for_capacity(&self) -> u64 {
        self.M
            .size(mapSize, mapTableSize, sizeofPtr, self.groupSize)
    }
}

// NewMemAwareMap creates a new MemAwareMap with the given initial capacity.
// NewMemAwareMap 创建带初始容量的 map 并调用 Init；Go 的 make(map[K]V, capacity) 对应 SwissMap::with_capacity。
/// 按初始容量创建并 Init。
pub fn NewMemAwareMap<K, V>(capacity: usize) -> MemAwareMap<K, V>
where
    K: Eq + Hash,
{
    let mut m = MemAwareMap {
        M: SwissMap::default(),
        groupSize: 0,
        nextCheckpoint: 0,
        Bytes: 0,
    };
    m.Init(SwissMap::with_capacity(capacity));
    m
}

// checkMapABI 在 Go 初始化路径中检查 runtime.Version 是否仍是 go1.26。
// 不调用真实 Go runtime，只保留升级 Go 版本时必须重新确认 ABI 的防线。
/// ABI 自检占位：提醒 Go 版本升级时复核。
pub fn checkMapABI() {
    // Rust's standard HashMap is intentionally independent of the Go runtime ABI.
}
